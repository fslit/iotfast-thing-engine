# P0 遥测入站接口契约草案

> **职责边界更新**：本引擎定位为设备业务数据模型与标准数据服务；用户 / 角色 / 菜单、业务权限、审批、告警与物理控制属于上层。引擎保留服务身份验证及可信数据范围隔离。原文相关内容为历史候选，不构成本引擎实施范围，详见 [职责与模型规范](../architecture/data-engine-scope.md)。


状态：设计草案，尚未实现；依据 [详细设计 21–23 节](../architecture/design-specification.md#21-数据入站幂等与-ack-契约) 与 [服务身份、可信数据范围与 D00 公共契约](service-identity-and-scope-contract.md)。若本文与详细设计冲突，以详细设计的第二部分为准。此处未发布独立 JSON Schema 或 OpenAPI 文件；进入 D01 代码实施前应根据本文与 D00 公共契约生成可自动校验的机器契约与测试向量。

## 1. 数据流与职责

已有网关负责协议读取、工程转换、来源时间和质量；引擎验证授权、绑定及模型，判断幂等，将可接收消息持久化或暂存，然后更新状态及历史。默认 `gateway_normalized`，一条消息不得二次换算。一次消息只对应一个设备、一个模型版本和一个绑定快照；P0 全部点位原子接受或全部拒绝。

当网关发生通信超时或断连、**没有新工程测量值**时，应通过状态事件契约（`POST /v1/source-state`，`spec = "iotfast.source_state.v1"`，见 [D00 公共契约第 6 节](service-identity-and-scope-contract.md#6-数值测量消息与连接质量状态事件分离契约)）上报质量与连接状态变化，不得向 `POST /v1/telemetry` 发送 `value: null` 或将旧值包装成新的采样时间戳。

## 2. 消息字段 (`POST /v1/telemetry`)

| 字段 | 必填 | 语义 / 校验 |
|---|---|---|
| `spec` | 是 | 固定 `iotfast.telemetry.v1`；未知版本拒绝（`422 VALIDATION_FAILED`） |
| `message_id` | 是 | 来源生成的消息标识（1–128 位可打印 ASCII）；同次重试保持不变 |
| `source_id` / `source_epoch` | 是 | 来源及其序列时期；由认证主体（`southbound_source`）授权且同次重试固定 |
| `device_id` | 是 | 设备 ID，在 `source_device_grant` 授权范围内 |
| `model_revision_id` / `binding_revision_id` | 是 | 真实使用的已发布模型和绑定快照 |
| `points` | 是 | 非空数组；单消息内 `property` 不得重复 |
| `points[].property` | 是 | 模型定义且在所声明绑定中已绑定（`source_kind = "gateway_measurement"`） |
| `points[].value` | 是 | 非 `null` 工程值；精确类型、有限数（禁止 `NaN/±Inf`）和范围根据模型校验 |
| `points[].source_at_ms` | 是 | 采样 UTC 毫秒；超出 `max_future_skew_ms` 时返回 `422 CLOCK_SKEW_EXCEEDED` 或隔离 |
| `points[].source_time_origin` | 是 | 枚举：`device_clock` / `protocol_timestamp` / `gateway_sample`（网关无设备时钟时必填 `gateway_sample`） |
| `points[].source_seq` | 是 | 十进制无符号 64 位整数字符串（`0..=18446744073709551615`）；同源同 epoch 同设备同点位内按整数语义单调递增 |
| `points[].quality.state` | 是 | `good` / `uncertain` / `bad` / `unknown`；无新测量值的通信故障走 `iotfast.source_state.v1` |
| `points[].quality.source_protocol` / `source_code` | 建议 | 源协议与原始质量码，支持缺失 / `null` |
| `points[].delivery.replayed` | 是 | 是否补传；不改变原始质量，不参与 `payload_hash` 业务内容摘要 |

`received_at_ms` 由服务端赋值，不由客户端信任。每个点位在首次通过校验入 WAL 前按 `(tenant_id, source_id, source_epoch, message_id, property)` 确定性派生稳定 `observation_id`，重放与多目标投递保持不变。

## 3. 原子接受与响应

整消息经身份、设备 / 绑定、类型 / 值域和资源约束校验后才接受；任一错误返回 `422 VALIDATION_FAILED` 和字段级错误列表，整条无副作用。来源未授权返回 `403 SOURCE_NOT_AUTHORIZED`，同 key 内容冲突返回 `409 IDEMPOTENCY_CONFLICT`，绑定版本冲突返回 `409 REVISION_CONFLICT`，限流或容量不足返回 `429 RATE_LIMITED` / `503 DURABILITY_BACKPRESSURE`；统一错误体包含 `request_id`、`code`、`message`、`retryable`、`retry_after_ms`、`details`。

- `accepted_volatile`：进入内存，重启可能丢失；HTTP `202`。
- `accepted_durable`：已达到服务端声明的应用日志持久边界，可重放；HTTP `202`。不是历史查询已提交。
- `duplicate`：相同 key 和语义内容返回原 `ingest_id` 与当前历史状态；HTTP `200`。
- `GET /v1/ingestions/{ingest_id}`：按授权范围返回历史处理状态（`pending` / `committed` / `failed`）；状态保留期限应在部署策略公开。

客户端通过 `Prefer: durability=durable` 请求可靠接收时不可静默降级；服务端无法达成则失败。请求超时后以原 key 和业务内容重试，不生成新 ID。

## 4. 幂等与版本

幂等范围由已鉴权 `tenant_id`、`source_id`、`source_epoch`、`message_id` 构成；服务端按 [D00 公共契约 4.2 节](service-identity-and-scope-contract.md#42-遥测业务摘要-payload_hash-canonicalization-规则) 的 `sha256-jcs-v1` 规范化规则对业务内容计算 `payload_hash`。`replayed`、`request_id`、服务端接收时间不属于摘要；相同 key 不同摘要返回 `409 IDEMPOTENCY_CONFLICT`。

去重元数据及历史写入 / 策略状态须按设计要求在同一事务应用；在去重保留期外不得声称仍能识别重复。旧绑定消息的采样时期无法证明时隔离 / 拒绝，不能拿新倍率重算。

## 5. 示例与反例

例子中来源标识、设备、数值和时间均为虚构；完整请求参考详细设计 21.1 节。

| 案例 | 预期 |
|---|---|
| 网关工程值 245.6、绑定匹配、质量 good | 验证通过后写入 245.6 |
| 属性不存在，消息另带一个合法点 | 整条 422；不能只接受合法点 |
| 同消息重试，replayed 从 false 变 true | 语义摘要不变，返回原 ingest_id |
| 相同 key 但温度从 245.6 改为 255.0 | 409；原记录不可覆盖 |
| 磁盘达到硬水位，客户端要求 durable | 503 DURABILITY_BACKPRESSURE，不返回 accepted_durable |
| 连接超时，没有新采样值 | 通过 `POST /v1/source-state` 更新质量与新鲜度；不把旧值或 `null` 包装成新测量 |

## 6. 待现场确认与 D01 衔接事项

1. 生成跨语言（Rust / Python / Node-RED）一致的 `sha256-jcs-v1` 与 `observation_id` 标准测试向量 JSON 文件。
2. 根据目标部署环境锁定单批最大点数、最大报文体积、默认 `max_future_skew_ms` 与 `ingest_status` 保留时长。
3. 核实现有网关能否直接发送 `binding_revision_id`、稳定 `message_id` 与独立 `iotfast.source_state.v1` 状态报文；若旧网关不支持，需在 D09 之前通过本地持久桥接器转换并明确桥接边界。

每项应提供正反样例、责任人、评审记录及对应自动测试后才能标记完成。
