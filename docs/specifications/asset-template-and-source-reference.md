# 资产模板、属性数据引用与关系设计

状态：设计草案，尚未实现或实测。参考 [IDMP 理念评估](../architecture/idmp-reference-assessment.md)，以 [引擎职责边界](../architecture/data-engine-scope.md)、[设备业务模型](device-business-model.md) 和 [遥测契约](p0-telemetry-contract.md) 为基础；冲突时先登记并解决。P0 继续使用网关上报工程值、一设备一个 active 绑定；外部库直接引用与多源切换为后续能力。

## 1. 要解决的问题

同一型号 100 台水泵应复用一个模型，但各台设备的站号、通道和历史来源不同；一处压力属性应具有统一名称、单位和数据质量，不因网关字段名称变化而改变其业务身份。资产树还需要表达水泵由某配电柜供电、某电表测量某产线等非层级关系。

## 2. 核心对象与已有设计映射

| 新概念 | 已有对象 / 关系 | 作用 |
|---|---|---|
| 资产模板版本 | 设备模型复用现有 product + model_revision | 描述一类设备的属性、约束与语义；已发布版本不可变 |
| 模板属性 | 现有 model_item(kind=property) | 类型、单位、读写描述、语义、默认策略 |
| 资产实例 | 现有 asset / device | 企业 / 厂区 / 产线 / 设备；非设备模型模板暂属后续 |
| 属性资源 | 现有 resource | 每台设备每个模型属性拥有稳定 resource_id |
| 数据引用 | 对现有 binding_revision / point_binding 增补 source_kind 的设计 | 设备属性到数据来源的版本化映射 |
| 资产关系 | P1 asset_relation | 表达供电、供水、测量和上下游等非层级关联 |

资产模板是建模概念，不额外创建一套与产品模型平行的 P0 权威表。static / gateway / derived / external 的契约逐步扩展；当前已实现能力须与规划状态分开。

## 3. 模板规范

设备模板由 product_key、model_version、属性清单与约束组成；建议补充 category、description、semantic tags / role，但这些语义字段不能授予权限或替代现场配置。属性定义中 identifier 稳定，示例：running_state、outlet_pressure、active_power、energy_total；每项带 data_type、unit、语义描述、freshness_policy 和 storage_policy。

| 属性 | 类型 | 单位 | 语义 |
|---|---|---|---|
| running_state | bool | 无 | true 的含义必须由模板明确为“运行” |
| outlet_pressure | float64 | MPa | 泵出口压力实际值 |
| active_power | float64 | kW | 实际有功功率 |
| energy_total | float64 | kWh | 经核实倍率后的累计电能 |

模板不得包含具体设备 IP、密码、站号、数据库凭据、实时值或用户角色。数据上报契约不因模板发布自动生成实测。字段 / 单位词表、精度与范围仍需正式 Schema 冻结。

### 3.1 生命周期

草稿 → 校验 → 发布不可变 revision → 弃用 / 退役；设备按 revision 固定引用，不随模板编辑静默更新。变更对属性 ID、数据类型、单位、量纲、业务含义及读写声明生成差异；影响绑定、历史、订阅、衍生算法时显示明确结果。先完成设备兼容性和迁移计划，再切换已发布版本；历史仍关联采集时 revision。display name 可以改但不应改变标识语义。

### 3.2 一次定义、多实例绑定

pump_model_r1 定义 pressure 等属性；pump_001 和 pump_002 使用该模型但各自创建 resource_id、binding_revision_id 与具体来源，保证“相同语义、不同设备、不同地址”。实例化、资源生成和设备版本关联必须在一致的元数据边界提交；失败回滚，重复请求按幂等规则返回原结果，不能生成第二组资源。

P0 不支持模板继承或实例覆盖模型类型 / 单位；允许哪些实例级显示字段和策略覆盖由后续契约定义。真实产品兼容性未验证，模板是设计方向。

## 4. 数据引用分类

数据引用描述“属性如何取得值”，不是允许任意连接或执行代码的配置。每次采用 binding_revision_id 指向不可变生效快照，字段及参数设计由正式契约规定。

| source_kind | 说明 | 阶段 | 质量与历史 |
|---|---|---|---|
| gateway_measurement | 网关完成协议解码和工程值转换，上报属性 | P0 | 按遥测契约，持久 / 重试取决于实际能力 |
| static_config | 已版本化的设备额定值等 | 后续设计 | 记录生效时间与配置来源，不伪装为新采样 |
| derived_metric | 公式 / 外部算法结果 | P1 规划 | 记录算法版本、输入、计算窗口与质量 |
| external_reference | 从现有数据库受控读取 | P1 可行性验证 | 连通 / 查询不代表支持订阅、补传或本地持久 |
| external_fact | MES 提供产量、班次等业务事实 | P2 规划 | 记录权威来源 / 修订 / 有效期；用户权限属于上层 |

### 4.1 P0 绑定示意

```json
{
  "device_id": "pump-001",
  "model_revision_id": "pump-model-r1",
  "binding_revision_id": "pump-bind-r1",
  "source_id": "gateway-01",
  "normalization_owner": "gateway",
  "points": [
    {
      "property": "outlet_pressure",
      "source_kind": "gateway_measurement",
      "source_point_ref": "gateway-01/pump-001/pressure",
      "engineering_unit": "MPa"
    }
  ]
}
```

source_point_ref 只是抽象示意，不替代现场 Modbus 的地址基数、站号、功能码、数据类型、字节 / 字顺序、比例等真实字段；网关输出工程值后引擎不再次换算。此 JSON 不是已发布 Schema；P0 上报仍以遥测契约必填字段为准。

### 4.2 接入、启用和更新

1. 草稿指定设备 / 已发布模型 / 来源 / 映射及版本。
2. 验证身份及可信范围、属性存在、源类型、转换归属、单位 / 类型兼容和资源唯一性。
3. 激活前网关确认实际快照 ID / hash；无法确认时执行人工停采窗口。
4. P0 同一设备仅一个 active 绑定，新绑定生效后上报必须携带真实使用的 revision；无法证明旧消息采集时段时隔离 / 拒绝。
5. 每条观测保存模型及绑定版本、source_time_origin、质量和原始质量（可用时），映射修改不改写既有历史。

批量实例化时，变量替换只能按声明的参数填充经验证的字段，预览每个实例的资源、绑定和冲突；不能通过模板注入 SQL / shell、凭据或越权路径。批处理默认不宣称跨所有设备原子；每实例结果、失败和可重试信息须保留。正式参数 / 幂等契约在实现前冻结。

## 5. 外部数据库引用的可行性门槛

不是新增通用数据库连接就等于可引用所有外部数据。实现前需验证：
- 数据身份：来源数据库 / 表或测点、时间列、值列、质量列、稳定标识，字段及单位映射。
- 查询边界：可信服务身份、可访问资源范围、连接密钥引用、白名单标识符及参数绑定；禁止由前端或 AI 输入自由 SQL。
- 时序语义：时间精度、迟到、同时间多条观测、稳定分页、质量缺失和断线表现。
- 能力区分：当前值读取、历史查询、推送 / 轮询、补传、本地持久和重算分别声明；未验证功能显示 unavailable，不能宣称实时或无损。
- 权威来源：区分 managed_ingest 与 referenced_read；禁止无稳定 observation_id / 去重证据就合并两个库查询结果。
- 断网：返回源不可用、最后成功时间及 stale / unknown，不静默返回旧值为实时数据。

若依赖某厂商特定虚拟表或专有接口，必须显式标注后端限制；不能作为多数据库通用承诺。

## 6. 资产树与关系网络

asset 主树支持组织和 canonical UNS 路径；P1 asset_relation 保存非层级关系，如 FEEDS、MEASURED_BY、UPSTREAM_OF。结构候选：tenant_id、relation_id、from_asset_id、relation_type、to_asset_id、valid_from_ms、valid_to_ms、source、revision。资产身份与当前路径分离，重命名不改 asset_id / resource_id。

| 关系 | 起点 | 终点 | 是否自动推断 |
|---|---|---|---|
| FEEDS | 配电柜 | 水泵 | 否，需现场确认 |
| MEASURED_BY | 水泵房 | 总电表 | 否；不代表可直接分摊到每台泵 |
| UPSTREAM_OF | 水泵 | 储水罐 | 否，方向及含义需定义 |

同一个对象可通过关系参与多个视图，但 P0 主树只维持一个父节点；跨树关系不更改主树。关系的有效期 / 修订让历史查询能够说明当时的拓扑；无历史记录时应返回 unknown。关系合法的方向、环、重复、删除及权限过滤需逐类型确定。

## 7. API 与数据存储草案

| 类别 | 草案能力 | 主要返回信息 |
|---|---|---|
| 模型 | 创建草稿 / 校验 / 发布 / 比较 | immutable revision、冲突、兼容性报告 |
| 设备实例 | 从指定 revision 创建 / 查询 | device_id、resource_id、版本及绑定状态 |
| 引用 | 编辑草稿 / 校验 / 激活 / 迁移 | source_kind、目标资源、状态、实际 binding revision |
| 资源查询 | 查询设备视图、资源和 UNS 路径 | 类型、单位、值、质量、新鲜度、来源及时间 |
| 关系查询 | 查询指定资产的一度关系（P1） | 类型、方向、有效期、来源；受可信范围约束 |

服务接口路径以现有 scope 文档为准，外部直读及关系 API 尚未冻结。需要关联既有 product、model_revision、model_item、asset、device、resource、binding_revision、point_binding；P1 才评估 asset_relation / 外部数据源连接表。历史保留 model_revision_id、binding_revision_id、observation_id。关系型元数据与历史时序库若分离，不得假设跨库事务原子。

## 8. 验收案例

- AT01 一模型两台水泵，同名属性具有不同资源与绑定 ID；获取视图不串值。
- AT02 模型升级不自动改写旧设备和旧历史；不兼容变更需迁移审查。
- AT03 模板未经声明的现场地址 / SQL / 凭据被拒绝；批量预览逐实例显示冲突。
- AT04 网关已转换 0.42 MPa 引擎仍输出 0.42；不能二次乘倍率。
- AT05 超时 / 质量 bad 时不把旧值变成新采样；路径移动保持 ID。
- AT06 不允许客户端自报租户或角色访问；关系查询不可泄露未授权端点。
- AT07 外部库质量 / 订阅能力未证实时返回 unavailable，不宣称实时和无损。
- AT08 水泵房公共电表不能自动归因到每台泵；派生结果带范围、窗口和来源。
- AT09 激活期间旧绑定消息无法证明有效期时隔离 / 拒绝；失败迁移可定位与回滚。

以上为待执行测试，不代表已有可运行实现、跨库集成或标准认证。
