# P0 遥测入站接口契约草案

状态：设计草案，尚未实现；依据 [详细设计 21–23 节](../architecture/design-specification.md#21-数据入站幂等与-ack-契约)。若本文与详细设计冲突，以详细设计的第二部分为准。此处未发布 JSON Schema 或 OpenAPI；完成 D00 前应以评审后的正式契约及测试样例替换草案。

## 1. 数据流与职责

已有网关负责协议读取、工程转换、来源时间和质量；引擎验证授权、绑定及模型，判断幂等，将可接收消息持久化或暂存，然后更新状态及历史。默认 gateway_normalized，一条消息不得二次换算。一次消息只对应一个设备、一个模型版本和一个绑定快照；P0 全部点位原子接受或全部拒绝。

## 2. 消息字段

| 字段 | 必填 | 语义 / 校验 |
|---|---|---|
| spec | 是 | 固定 iotfast.telemetry.v1；未知版本拒绝 |
| message_id | 是 | 来源生成的消息标识；同次重试保持不变 |
| source_id / source_epoch | 是 | 来源及其序列时期；由认证主体授权且同次重试固定 |
| device_id | 是 | 设备 ID，在来源授权范围内 |
| model_revision_id / binding_revision_id | 是 | 真实使用的已发布模型和绑定快照 |
| points | 是 | 非空数组；单消息 property 不得重复 |
| points[].property | 是 | 模型定义且在所声明绑定中已绑定 |
| points[].value | 是 | 工程值；精确类型和范围根据模型校验 |
| points[].source_at_ms | 是 | 采样 UTC 毫秒；无设备时间时可用网关采样时间，但须标明来源 |
| points[].source_time_origin | 是 | 声明时间来源；允许值及漂移策略 D00 冻结 |
| points[].source_seq | 是 | 十进制字符串；同源同 epoch 同设备同点位内单调递增 |
| points[].quality.state | 是 | good / uncertain / bad / unknown；bad 不自动伪造一个新测量 |
| points[].quality.source_protocol / source_code | 建议 | 源协议与原始质量码，支持缺失 / null |
| points[].delivery.replayed | 是 | 是否补传；不改变原始质量，不参与业务内容摘要 |

received_at_ms 由服务端赋值，不由客户端信任。字符范围、长度、序列上限、批大小和时钟漂移阈值尚需 D00 确定并纳入正式 Schema；不要把示例值视为协议约束。

## 3. 原子接受与响应

整消息经身份、设备 / 绑定、类型 / 值域和资源约束校验后才接受；任一错误返回 422 和字段级错误列表，整条无副作用。来源未授权返回 403，同 key 内容冲突返回 409，限流或容量不足返回 429 / 503；错误体包含 code、message、details、request_id、retryable。

- accepted_volatile：进入内存，重启可能丢失；HTTP 202。
- accepted_durable：已达到服务端声明的应用日志持久边界，可重放；HTTP 202。不是历史查询已提交。
- duplicate：相同 key 和语义内容返回原 ingest_id 与当前历史状态；HTTP 200。
- GET /v1/ingestions/{ingest_id}：按授权返回历史处理状态；状态保留期限应在部署策略公开。

客户端请求 durable 时不可静默降级；服务端无法达成则失败。请求超时后以原 key 和业务内容重试，不生成新 ID。

## 4. 幂等与版本

幂等范围由已鉴权 tenant_id、source_id、source_epoch、message_id 构成；服务端对规范化业务内容计算摘要。replayed、request_id、服务端接收时间不属于摘要；相同 key 不同摘要返回 IDEMPOTENCY_CONFLICT。摘要 canonicalization 的具体算法、字段顺序、数值格式和版本必须在 D00 确定，不能用语言默认 JSON 序列化作为跨实现契约。

去重元数据及历史写入 / 策略状态须按设计要求在同一事务应用；在去重保留期外不得声称仍能识别重复。旧绑定消息的采样时期无法证明时隔离 / 拒绝，不能拿新倍率重算。

## 5. 示例与反例

例子中来源标识、设备、数值和时间均为虚构；完整请求参考详细设计 21.1 节。

| 案例 | 预期 |
|---|---|
| 网关工程值 245.6、绑定匹配、质量 good | 验证通过后写入 245.6 |
| 属性不存在，消息另带一个合法点 | 整条 422；不能只接受合法点 |
| 同消息重试，replayed 从 false 变 true | 语义摘要不变，返回原 ingest_id |
| 相同 key 但温度从 245.6 改为 255.0 | 409；原记录不可覆盖 |
| 磁盘达到硬水位，客户端要求 durable | 拒绝 / 背压，不返回 accepted_durable |
| 连接超时，没有新采样值 | 质量与新鲜度变化；不把旧值包装成新测量 |

## 6. 待冻结事项

1. canonicalization 细节、重复请求并发顺序及跨重启索引恢复。
2. source_seq 上限、source_time_origin 词表和时钟异常处理。
3. 空值 / bad 质量的状态消息与测量消息具体格式：设计已有“故障不产生虚假采样”的要求，但完整 wire schema 尚未冻结；在此之前不要把 null 当合法测量。
4. 接口消息大小、认证方案、查询状态保留和错误详情格式。
5. 外部网关无法提供 binding revision 或稳定 ID 时的降级与桥接能力。

每项应提供正反样例、责任人、评审记录及对应自动测试后才能标记完成。
