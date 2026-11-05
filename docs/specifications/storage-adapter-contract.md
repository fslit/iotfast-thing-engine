# 存储适配器与可靠投递详细契约

> **职责边界更新**：本引擎定位为设备业务数据模型与标准数据服务；用户 / 角色 / 菜单、业务权限、审批、告警与物理控制属于上层。引擎保留服务身份验证及可信数据范围隔离。原文相关内容为历史候选，不构成本引擎实施范围，详见 [职责与模型规范](../architecture/data-engine-scope.md)。


状态：研发设计草案，尚未实现或验证。本文细化 [时序数据库设计](timeseries-storage.md) 和 [总体设计](../architecture/design-specification.md)；如果本文与总体设计第二部分存在冲突，先登记并解决，不直接实现。D00 完成前本文不能替代经过评审的机器可读契约。

## 1. 目标、参与者与边界

协调器统一调度本地 DuckDB 和外部 TSDB，适配器负责连接、能力声明、批写与查询翻译。应用 WAL 存放标准化消息和投递状态来源；DuckDB 作为默认历史存储目标；外部 TSDB 是独立目标。所有目标必须通过自己的能力验证，不能仅凭数据库品牌启用可靠模式。

资源目录、权限、模型 / 绑定版本仍由引擎管理；外部时序库不是目录或权限的可信来源。本文的接口名称是设计词汇，不规定 Rust crate API 或插件 ABI。

## 2. 关键身份和映射

| 标识 | 生成时点 | 保持不变范围 | 作用 |
|---|---|---|---|
| message_key | 已认证入站时 | 网关重试与重放 | 区分重试消息 |
| observation_id | 消息首次标准化并写入 WAL 前 | 所有后端、重放和迁移 | 单个点位事实身份 |
| wal_position | WAL 追加时 | 对应日志记录生命周期 | 有序恢复、检查点 |
| target_id | 配置创建目标时 | 重启、凭据轮换 | 区分目标进度 |
| batch_id / attempt_id | 调度投递时 | batch 固定，attempt 每次变化 | 审计和关联响应 |

message_key 包含 tenant、认证来源、epoch、message_id；同一消息包含多个点时每点 observation_id 必须不同且在首次持久化后固定。实现可以在首次接受时生成并写入 WAL；不可在每次重放时重新生成。ID 的实际编码和生成算法为 D00 决策，冲突检验和版本需测试。复制到外部时须保留 ID；若目标无法存储 ID，标记能力不足，禁止承诺可核对的无损迁移。

## 3. 观测与投递实体

Observation 包含：observation_id、message_key、tenant_id、resource_id、device_id、property_id、model_revision_id、binding_revision_id、source_at_ms 和 time_origin、received_at_ms、source_epoch / source_seq、typed value、quality、source_quality_code、replayed。时间与值的含义不能因后端存储类型而改变。

Delivery 关联 target_id、wal_position、observation_id、payload_hash、status、attempt_count、last_error、next_retry_at；本地持久保存。目标端去重需要稳定 ID 与完整 payload 语义，不能只在内存中维护本地缓存。

## 4. 能力握手

启动和配置变更时读取静态驱动声明及目标实例探测结果，交叉核对形成 capability snapshot，附 target_id、驱动版本、服务端版本、配置摘要、测试报告引用和 checked_at。实例可读版本并不证明写权限、幂等或性能。

| 能力 | 说明 | 不具备时 |
|---|---|---|
| exact_id_roundtrip | 查询时完整保留 observation_id | 禁止声称精确对账与无损迁移 |
| same_timestamp_distinct | 同资源同毫秒不同 ID 可分别保存 | 禁止无损模式；仅允许显式批准的有损策略 |
| replay_idempotent | 相同 ID + 相同内容重复投递不改变最终事实 | 远端结果未知时停止自动重投，进入 reconciliation_required |
| duplicate_conflict_detect | 相同 ID + 不同内容可识别为冲突 | 引擎侧校验，仍需考虑外部写入冲突；不宣称目标端保护 |
| batch_result_scope | 单条 / 整批 / 未知，且与真实响应一致 | 未能确认的项一律 unknown |
| snapshot_pagination | 分页稳定性和边界能力 | 标记弱一致或拒绝需要稳定快照的请求 |
| quality_roundtrip | 质量、时间来源和原始质量可恢复 | 不可宣称完整语义支持 |

每项有 supported / unsupported / conditional / unverified，以及测试编号与证据。连接断开、版本变化或配置变化会使有关探测结论过期；不因连接成功就把 unverified 升级为 supported。启动严格模式时，缺少必需能力拒绝该目标进入 ready；已缓存的 WAL 不丢弃。

## 5. 批量写请求 / 返回契约

批次包含 target_id、batch_id、协议 / schema 版本、观测列表及每条 observation_id / payload_hash；同一批内不得重复 ID。适配器返回关联 batch_id 和每条状态，或明确声明整个批次的共同状态。状态仅有：

- committed：按此目标声明的提交边界已确认；不是自动代表复制到多个节点。
- retryable：有证据证明尚未提交且可以重试，返回原因和可选 backoff。
- permanent_rejected：确定无法按当前配置写入，保留原因并进入隔离处理。
- unknown：超时、连接中断、部分结果不明等，禁止假设已失败。

如果驱动只返回整体成功则按其文档与测试约束标记所有 committed；如果可能部分成功且无法定位，则未明确确认的所有条目均为 unknown。适配器不得把网络 2xx 一概解释成目标已持久提交；各数据库具体 ACK 边界需单独验证和说明。

## 6. 恢复状态机与检查点

每个 target 的每项逻辑状态：pending → inflight → committed / retryable / permanent_rejected / unknown。进程重启时 inflight 变 unknown；仅在目标 replay_idempotent 已验证或确认未写入时可重试。unknown 需目标端按 observation_id 查询并对比 hash；能验证相同内容则 committed，不存在且具有足够一致性保证时才可重试，不可判断则保持 unknown 并告警。

检查点是该 target **连续完成前缀**，不能越过 unknown、retryable 或 permanent_rejected 的空洞。示例 positions 101 committed、102 unknown、103 committed：连续 checkpoint 至多 101；103 的已完成状态需要持久记录以避免重启后错误重写，102 处置后再推进。分片 / 并发顺序需在任务实现前定义，不得用最大完成 offset 替代连续前缀。

permanent_rejected 不能悄悄丢弃并推进；修复后重试或经明确授权将其隔离为已记录缺口。隔离需要持久记录原 observation_id、目标、原因、操作者 / 策略、时间和可恢复来源，并在查询结果 / 运维界面披露。该项不等于成功，不得把存在缺口的结果标成完整。

## 7. 故障和回收判定

| 场景 | 状态 | 后续 |
|---|---|---|
| 请求发送前连接失败且能确认未发送 | retryable | 有界退避，保留 WAL |
| 远端已提交但响应丢失 | unknown | 若已证明幂等则安全重放，否则查回 / 人工对账 |
| 一批 10 条，仅确认 6 条 | 6 committed、其余按证据分类 | 不跨越未决空洞推进前缀 |
| 目标拒绝一个不支持的类型 | permanent_rejected | 修正映射或记录经授权缺口，不静默转换 |
| 本地 WAL 已 ACK，但目标长时间离线 | pending / retryable | 报告延迟；空间达到上限时拒绝新的 durable 接收 |
| 最后目标 checkpoint 已持久，但文件删除前崩溃 | committed | 恢复后可重新计算回收条件 |

WAL 只能在所有必需目标完成前缀、相关去重 / 隔离记录安全保存且 retention 策略允许时回收。目标移除 / 跳过是显式管理员操作，必须先列出数据范围和影响、持久审计，并在后续结果披露缺口。已 ACK 的原始记录不能为腾空间静默删除。

## 8. 查询契约

查询先按当前认证主体对资源 ID 做权限过滤，随后以 typed AST 传给选定目标；适配器不得接收不受限制的客户端 SQL。必选：资源 ID、UTC [start,end)、raw / aggregate、quality_states、sort、limit；可选 cursor。数据返回 observation_id、resource_id、采样 / 接收时间、值与质量、模型 / 绑定版本、结果类型、目标来源。

响应元信息包括 target_id、consistency、partial、warnings、coverage（水位与已知缺口）、next_cursor。cursor 绑定查询条件、目标和授权范围，有效期及服务版本要约定；不得使用客户端提供的任意 SQL 或路径拼接数据库标识。raw 分页默认按 (source_at_ms, observation_id)；无法保证稳定快照时明确标记 weak 并提供重查建议。弱一致下无法承诺翻页不漏数据。

对于聚合，应固定窗口边界、过滤质量、空桶、数值类型和单位；单位或模型语义不兼容时拒绝跨版本聚合。插值必须显式指定并标记为合成结果，不适用于 bool / enum / 状态量。local_pending + remote 的合并查询必须有目标端 ID 完整性、覆盖范围及去重证据；否则返回 partial 或拒绝，不进行无标识 UNION。

## 9. 配置与状态示例

以下仅展示契约方向，不是现有配置文件或已实现 API。实际 schema / 默认限额尚需确定。

```json
{
  "mode": "dual",
  "targets": [
    {"id": "local-duckdb", "kind": "duckdb", "required": true},
    {"id": "remote-tsdb", "kind": "tdengine", "required": true, "credential_ref": "secret://tsdb-prod"}
  ],
  "ack_policy": "local_durable",
  "history_read_target": "local-duckdb"
}
```

状态返回应包括 local_durable、目标级 pending / committed / unknown 数、连续检查点、最老待同步时刻、known_gaps、health / last_error（脱敏）。secret 引用仅用于说明，不代表实现了某种密钥服务。

## 10. 适配器一致性验收

每一目标按服务端版本、部署模式、驱动版本和配置分别测试，能力报告须关联原始证据。基本向量：

- A01 类型和质量往返：数值、文本、布尔、坏质量、source quality、模型 / 绑定版本与时间一致。
- A02 两个不同 ID 相同点位 / 毫秒同时存在；相同 ID 重放保持同一事实。
- A03 相同 ID、不同 hash 的冲突；A04 远端成功后断开响应并重试 / 查回。
- A05 批次部分提交；A06 非连续检查点空洞和进程中断恢复。
- A07 限流、永久拒绝、磁盘满和未知目标状态；不能静默删除已确认记录。
- A08 模型版本单位变化、迟到、质量过滤和半开时间边界。
- A09 查询跨页并有并发补传，标明一致性；无权限资源不可见。
- A10 迁移与回滚期间新写、历史回填和观测 ID / 质量对账。

通过必须附输入清单、实际目标返回、重启前后检查点和目标侧查询结果；不具备完整能力时明确禁止的模式及替代方案。吞吐、存储成本、tag 基数和恢复速度还需单独负载测试，不能从逻辑用例推断。

## 11. 后续任务拆分

| 编号 | 交付 | 验收 |
|---|---|---|
| ST-01 | 固定 observation_id / payload hash 规则与测试向量 | 重试、重放、跨目标 ID 稳定；避免编码歧义 |
| ST-02 | 定义能力快照和严格模式 | unverified 不得当 supported |
| ST-03 | DuckDB 适配及事务边界实测 | 已提交、失败和恢复与独立清单一致 |
| ST-04 | 投递状态、非连续 checkpoint 与 WAL 回收 | A04–A07 通过 |
| ST-05 | 统一查询契约 / 授权 / 分页 | A08–A09 通过 |
| ST-06 | 选择一个外部库并实现原型 | A01–A10 和实际版本限制记录 |
| ST-07 | 同步、迁移和运维界面 | 缺口、目标进度、容量告警可见 |

以上为设计和任务，未创建实际代码、真实测试或 GitHub Issues。目标版本、首个外部库和性能目标待现场确认。
