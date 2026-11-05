# 设备业务模型与实时业务视图详细设计

状态：设计草案，尚无服务实现、正式 Schema 或现场联调结果。职责以 [数据引擎定位](../architecture/data-engine-scope.md) 为准；入站与确认以 [P0 遥测契约](p0-telemetry-contract.md) 为准。本文示例的站号、地址、取值和标识均为虚构，不能直接应用于现场。

## 1. 设计目标与基本语义

统一描述一类设备可提供什么数据、每台设备实际接入哪些数据，以及当前数据的值、来源、时间和质量。上层可以通过 device_id、resource_id 或已授权的 UNS 路径发现数据，并按统一结构使用；引擎不实现上层用户 / 角色体系和控制流程。

本设计严格分离：
- **产品 / 模型定义**：静态可版本化的属性描述；发布后不可修改。
- **设备实例**：某台现场设备及其使用的模型版本和资产上下文。
- **绑定快照**：某设备每个模型属性如何映射到实际网关与测点。
- **观测记录**：有独立时间、质量和来源的测量事实。
- **实时业务视图**：动态读取模型、设备和最新观测形成的只读投影；不是新产生的采样。

## 2. 对象、身份与关系

| 对象 | 稳定标识及范围 | 内容 / 不变量 |
|---|---|---|
| 产品 Product | product_id；product_key 在租户内唯一 | 品类和显示名称；不包含实际协议地址 |
| 模型版本 ModelRevision | model_revision_id；product + model_version 唯一 | 发布后的属性清单、类型、单位及语义不可变 |
| 设备 Device | device_id；tenant 内唯一 | 关联产品、已发布模型版本、可选资产位置 |
| 属性 Resource | resource_id；设备 + kind + identifier 唯一 | 按设备实例生成；不能跨设备共享同一资源 ID |
| 绑定版本 BindingRevision | binding_revision_id | 固定设备、模型版本、来源、地址和转换归属 |
| 观测 Observation | observation_id | 同一次观察跨 WAL 重放及后端投影保持不变 |
| 业务视图 DeviceView | 由 device_id 与状态快照定位 | 多个属性各自含采样时间，不等于同一时刻原子测量 |

同一产品可有多台设备，一台设备在同一有效时刻只引用一个已发布模型版本与一个 active 绑定；一台设备的每个属性对应独立 resource_id。一个模型版本可被多台设备使用，但不同设备的现场地址、网关和倍率可不同。

## 3. 模型定义契约

模型正文分为 spec_version、product_key、model_version、name、description、properties，以及可选的事件 / 服务**能力描述**。P0 首先规范属性。属性定义建议包含：

| 字段 | 定义 | 规则 |
|---|---|---|
| identifier | 稳定英文标识 | 同模型版本内属性唯一；显示名称可变，不能用名称作为主键 |
| name / description | 人可读语义 | 描述用途；不能用作 AI 指令或权限策略 |
| data_type | 数值、布尔、枚举、文本等 | 类型、范围、精度 / 长度 / 枚举约束应可验证 |
| unit | 单位代码与展示符号 | 采用经约定的单位词表；不是任意字符串自动换算 |
| semantic | quantity、role、部位 | 例：pressure / actual；同义关系需明确词表 |
| access | read 或 read_write 元数据 | 不代表引擎执行写入；控制由上层 / 独立控制服务处理 |
| freshness_policy | stale 判定参数 | 由采集周期和业务要求配置；缺省策略待冻结 |
| storage_policy | 保存与心跳策略 | 与实时 latest 规则独立；对时间 / 质量变化保留正确语义 |

模型的 required 表示完整文档 / 入参要求，不能要求每条增量上报包含所有属性。浮点拒绝非有限数；int64 在 JSON 超过 JavaScript 安全整数范围时采用约定字符串编码。精度表示方式、数据范围、null、单位码和字段默认值仍需 D00 确定；下方示例不代表完整 JSON Schema。

```json
{
  "spec_version": "1.0.0",
  "product_key": "circulation-pump",
  "model_version": "1.0.0",
  "name": "循环水泵",
  "properties": [
    {"identifier": "running_state", "name": "运行状态", "access": "read", "data_type": {"type": "bool"}, "semantic": {"quantity": "equipment_state", "role": "actual"}},
    {"identifier": "pressure", "name": "出口压力", "access": "read", "data_type": {"type": "float64", "min": 0, "max": 2}, "unit": {"code": "MPa", "symbol": "MPa"}, "semantic": {"quantity": "pressure", "role": "actual"}},
    {"identifier": "current", "name": "电流", "access": "read", "data_type": {"type": "float64", "min": 0, "max": 100}, "unit": {"code": "A", "symbol": "A"}, "semantic": {"quantity": "electric_current", "role": "actual"}}
  ],
  "services": [],
  "events": []
}
```

本例单位代码只是项目内部示例，实施时须确定和验证统一词表。

## 4. 模型发布、变更和设备升级

1. 创建产品与模型草稿；校验标识唯一、类型、单位、值域、语义、数据量限制及发布版本唯一性。
2. 发布时在同一可验证事务保存不可变正文及索引，生成 model_revision_id 与内容摘要；失败保持草稿。
3. 按已发布版本创建设备，建立每个属性的资源 ID；绑定缺失时显示 unbound / unknown，不能伪造成正常值。
4. 新增模型版本先生成差异报告：属性增加、删除、重命名、类型 / 单位 / 语义变更、约束变化；识别对现有设备、绑定、订阅与历史查询的影响。
5. 设备升级必须显式生成新绑定草稿并做兼容性检查。属性标识、类型、单位和业务语义均保持兼容时可延续 resource_id；含义发生变化时新建 resource_id 并保留旧资源 / 历史关联。是否兼容必须有明确规则和审阅证据，不能只看标识。
6. 实施模型 / 绑定切换时按原规范先做网关版本确认；无法证明一致时停采并标记缺口。旧版本上报只有能证明其采样处于旧版本有效期才按旧快照接收，否则隔离 / 拒绝。
7. 回滚使用原已发布版本与有效绑定，不能删除新版本历史；历史每条按采集时 model_revision_id 和 binding_revision_id 解释。

状态：草稿 → 已发布 → 弃用 → 退役；已被设备 / 历史引用的版本不能物理删除。版本号格式、兼容性级别、禁止复用年限和变更审批均需在 D00 及上层流程中明确。

## 5. 水泵实例与绑定示例

两台水泵共享产品和 model_revision_id，但分别有独立 device_id / resource_id / binding_revision_id。假定 P0 使用已完成工程值转换的网关：

| 设备 | 模型属性 | 来源 / 示例地址 | 网关转换 | 引擎输入 |
|---|---|---|---|---|
| pump-001 | pressure | gateway-01 / holding register 零基 100 | uint16 × 0.01 | 0.42 MPa |
| pump-002 | pressure | gateway-02 / holding register 零基 200 | uint16 × 0.001 | 0.42 MPa |

网关输出值与模型单位需一致；绑定记录原始地址、类型、字 / 字节序及倍率以供回溯。示例仅是映射概念，真实 Modbus 必须确定 function code、register_count、unit_id、字节序、通道和转换配置；引擎不能再次乘倍率。实时输入未提供 raw_value 时不能假设能恢复原始寄存器。

## 6. 标准数据上报与处理

上层或网关身份经服务边界认证，且其可信范围覆盖 device_id。消息带 source_id / source_epoch、message_id、device_id、model_revision_id、binding_revision_id；每点带 property、工程值、source_at_ms、source_time_origin、source_seq、quality 和 replayed。

P0 整消息原子校验：未知设备、属性、非法类型、绑定不匹配等任一点失败则整条拒绝，结果有字段级错误；不产生部分已接受数据。身份 / 数据范围错误不得泄露其他租户资源。message_key 的相同业务内容重试返回原 ingest_id；相同 key 变更业务内容返回冲突。持久模式只有 WAL 到达同步边界才返回 accepted_durable；history_status=pending 时不能声称数据库已提交。

处理顺序：来源与范围验证 → 设备 / 模型 / 绑定校验 → 逐点规范化验证 → 稳定 observation_id → 根据请求达到 volatile 或 durable 接收 → 更新 latest / 按历史策略落库。详细格式与重试参照入站契约；业务视图的更新和历史提交可有延迟，不能表述为跨库同事务。

## 7. 设备业务视图的输入与输出

接口草案：GET /v1/devices/{device_id}/view。先验证调用服务及可信资源范围；资源不存在或未获授权的外部响应策略应一致，避免通过错误差异探测存在性。

成功结果应包含：
- device_id、device_name、product_id / key、model_revision_id、asset_id、canonical_uri。
- 属性 identifier、resource_id、name、data_type、unit、semantic 和绑定状态。
- 每点的 value / value_type、quality / reason、freshness、source_at_ms / source_time_origin、received_at_ms；无采样的 value 为 null，quality=unknown，明确 no_observation。
- 视图生成时间、状态 revision / snapshot 标识、模型与绑定版本、历史提交状态或可查询状态；不输出凭据、连接密钥或内部未授权配置。

```json
{
  "device_id": "pump-001",
  "device_name": "1号循环水泵",
  "product_key": "circulation-pump",
  "model_revision_id": "pump-model-r1",
  "asset_id": "plant-a-room-1",
  "view_generated_at_ms": 1793869200125,
  "view_consistency": "per_property_latest",
  "properties": {
    "pressure": {
      "resource_id": "res-pump-001-pressure",
      "name": "出口压力",
      "value_type": "float64",
      "value": 0.42,
      "unit": {"code": "MPa", "symbol": "MPa"},
      "quality": {"state": "good", "reason": null},
      "freshness": "fresh",
      "source_at_ms": 1793869200120,
      "source_time_origin": "gateway_sample",
      "received_at_ms": 1793869200125,
      "binding_revision_id": "pump-001-bind-r1"
    },
    "current": {
      "resource_id": "res-pump-001-current",
      "name": "电流",
      "value_type": "float64",
      "value": null,
      "unit": {"code": "A", "symbol": "A"},
      "quality": {"state": "unknown", "reason": "no_observation"},
      "freshness": "unknown",
      "source_at_ms": null,
      "source_time_origin": null,
      "received_at_ms": null,
      "binding_revision_id": "pump-001-bind-r1"
    }
  }
}
```

本视图按点位返回各自的最新值；不会隐含全部值来自同一采样周期。离线 / 超时只更新质量或新鲜度，可附 last_known_value 和原始采样时间；不生成新的数值观测。若需要多点一致采样，应由来源提供明确采集批次、采样时窗与完整性契约，另行设计，不根据接收时间猜测。

view_consistency 及响应字段属于方案示例，非已经发布的稳定接口。大模型属性集 / 设备列表须有上限、分页或字段筛选；动态值、路径和权限范围不同的请求不能共用不隔离的缓存。

## 8. 实时订阅与历史查询

发现资源后先建立有边界的快照与订阅：服务注册订阅并取得有序边界 S，发送一致快照，再发送 S 以后的有界增量。不能建立一致边界或缓冲满时返回 resync_required；断线重新拉取，订阅不承诺丢线期间完整历史。每项推送至少包含 resource_id、模型 / 绑定版本、值、质量及采样时间；状态更新与测量值更新须能够区分。

历史查询针对资源列表、[start_ms,end_ms)、质量过滤和分页；历史以采集时模型和绑定版本解释，同毫秒不同 observation_id 需可区分。外部时序库无法保证该语义时不得宣称无损支持；能力和同步缺口需随响应披露。聚合与插值不等于原始测量，必须标识。

## 9. 边界状态和错误

| 场景 | 实时视图 / 入站结果 |
|---|---|
| 已建模但未绑定 | 返回 binding_state=unbound、value=null、quality=unknown |
| 已绑定但未接收 | no_observation，不默认填 0 |
| 源超时 | 保留 last-known 值及原采样时间；质量 bad 或状态说明，freshness 按策略标注 |
| 迟到旧数据 | 符合采集版本和准入条件时可写历史；不得覆盖较新的 latest |
| 模型升级未完成 | 仍返回当前有效版本；新模型 / 绑定待生效不得冒充 active |
| 单消息属性不属于模型 | 整条拒绝；返回结构化字段错误 |
| 服务无该设备范围 | 拒绝或采用统一的存在性隐藏策略；不得只过滤 UI |
| WAL 达硬上限且要求 durable | 背压拒绝，不返回 durable 成功 |
| 远端时序库不可用 | 按存储策略继续本地队列或拒绝；报告远端 pending / unknown |

上表中的错误 code、HTTP 状态与状态枚举以正式 API / Schema 冻结为准。

## 10. 测试向量与交付拆解

- BM01：同一模型创建两台设备，pressure 资源 ID 不同，互不串值；未经授权的调用无法读取另一数据域。
- BM02：两网关各自转换 42 × 0.01 与 420 × 0.001 均得到 0.42 MPa，引擎不二次换算。
- BM03：新设备已建模但未上报，视图清晰返回无值 / unknown；不能以 0 或 false 充当默认采样。
- BM04：测量值 0.42 后发生超时，最近有效值的采样时间不变，质量 / freshness 单独变化。
- BM05：后收到的旧观测可以按策略进入历史，但 latest 不倒退。
- BM06：模型发布后原位修改失败；新版本新增可选属性时已有设备继续固定旧版本。
- BM07：模型属性单位或语义变更的资源不能静默沿用旧含义；旧历史仍能按旧版本解释。
- BM08：模型 / 绑定切换时，旧上报若不能证明旧采样范围则拒绝或隔离；不按新倍率解释。
- BM09：初始化订阅持续更新并强制缓冲溢出；必须报告 resync_required。
- BM10：持久日志确认后、数据库提交前后强杀及同 key 重试；需与独立发送清单核对，不能重复历史或误报已持久。

任务 BM-D01 规范冻结与样例审阅 → BM-D02 模型发布和兼容性 → BM-D03 设备 / 资源与版本绑定 → BM-D04 统一观测校验 → BM-D05 最新值 / 质量 → BM-D06 业务视图 → BM-D07 历史与订阅 → BM-D08 真实网关联调。每项交付需给出实际 API / Schema、测试结果与失败边界；以上只是设计与验收目标，当前均未执行。
