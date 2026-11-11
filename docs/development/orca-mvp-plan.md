# Orca 本地最小演示 profile：M0 → M1 → M2

版本：`orca-mvp-dev-v1`；决议日期：2026-11-10。M0 文档 owner 独占本文件；任务 `task_7c6ec9fcc2e6`。**本文冻结的是待实现、待实测的合成演示子集，不是已运行服务、完整 P0、D00–D10 验收或生产发布。** 用户本次明确授权本地最小实现，覆盖旧任务包“本轮仅文档”的轮次限制；不关闭旧台账的生产问题。

## 1. 范围、权威与执行分工

1. M0 只写本文与 [M0 证据](evidence/orca-mvp-m0.md)，不改现有规范、任务包、决策台账、status、代码、manifest 或公共文件。保留所有 dirty 基线，不 reset / clean / commit / 新建 checkout。目标文件若有他人写入，停止并向协调者协商。
2. M1 根据本文实现：单进程、loopback HTTP、固定合成目录与绑定、真实比较开发凭据、工程值上报、latest、DuckDB raw 历史及明确易失 ACK。具体代码 write scope 由协调者另授；本文不扩大写权限。M2 维护脚本 / fixture 并独立做 HTTP 验证，不能把 M0 文档检查当作功能 PASS。
3. 字段与语义选自 [总体设计第二部分 §18–24](../architecture/design-specification.md)、[遥测契约](../specifications/p0-telemetry-contract.md)、[公共身份 / 幂等 / 错误契约](../specifications/service-identity-and-scope-contract.md)、[北向契约 §5 / §7](../specifications/northbound-api-and-subscription-contract.md)。职责以 [引擎定位](../architecture/data-engine-scope.md) 为准；[产品基线](../../PRODUCT.md)、[执行任务包](ai-execution-plan.md)、[决策台账 Q01–Q12](specification-decisions.md) 仍保留完整生产范围。
4. 不迁就原型 serde wire、假 WAL 位置或同步 ACK。明确裁剪见 §8；未支持的能力拒绝，不以成功空响应伪装实现。生产 / 现场事实缺失仅阻塞对应生产项，不阻塞虚构数据演示。若演示必需 wire 仍有互斥解释，M1 停止该项并用 Orca ask 请协调者裁决，不自行放宽。

## 2. 工程与启动契约（M1 待交付）

复用 [现有原型 crate](../../prototype/Cargo.toml) 与 [重组后的模块入口](../../prototype/src/lib.rs)，保留旧公共路径，不创建正式生产工程，不撤销用户重组。新增演示二进制 target 名固定 `dev_server`，源路径固定 `prototype/src/bin/dev_server.rs`；HTTP 技术实现由 M1 在获准代码范围内选择，禁止将内部结构直接当作公共 JSON Schema。

在仓库根执行以下命令；**本段是将来启动接口，M0 未执行 cargo run**。三个变量必须由调用者先注入本次运行随机秘密，建议各使用 `openssl rand -hex 32`；不要将真实值提交、打印、写入 URL、证据或命令行参数。

```sh
test -n "$IOTFAST_DEV_INGEST_TOKEN" && test -n "$IOTFAST_DEV_READER_TOKEN" && test -n "$IOTFAST_DEV_ALL_TOKEN"
export IOTFAST_DEV_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/iotfast-mvp.XXXXXX")"
export IOTFAST_DEV_BIND=127.0.0.1:18080
cargo run --manifest-path prototype/Cargo.toml --bin dev_server
```

| 配置 / 路径 | 固定规则与失败行为 |
|---|---|
| `IOTFAST_DEV_BIND` | 缺省 `127.0.0.1:18080`；仅允许字面量 IPv4 `127.0.0.1:<port>`，port 为 1–65535。拒绝 `0.0.0.0`、外网 / 内网地址、hostname、IPv6、代理转发与隧道用途；非法值启动非零退出且不监听，不静默替换 |
| 三个 `*_TOKEN` | 均必填、互不相同，且各为 64 位十六进制字符串；缺失 / 无效启动非零退出，不使用内置口令。日志仅标服务代号；不允许 debug dump 请求头 / 环境 |
| `IOTFAST_DEV_DATA_DIR` | 必填绝对路径；只允许本次运行新建的空目录。非空或不可写则非零退出，不清理旧文件、不误用生产 DB；启动成功后目录 / 文件仅供当前用户访问 |
| 数据文件 | 该目录下 `engine.duckdb`；如复用运行时另需应用 WAL，置于该目录 `wal/`，不得因此宣称可靠接收。不得把路径回传 HTTP 错误 |
| 启动 fixture | 写入 §3 固定目录 / 模型 / 绑定 / grant；失败时不开放 HTTP，不从请求动态创建设备或补授权 |
| 时间与退出 | 启动时记录可信 UTC `started_at_ms`；凭据 / grant 生效区间为 `[started_at_ms, started_at_ms+3600000)`，到期每请求拒绝，不能滑动续期。Ctrl-C 正常停止并释放 DB；再次演示用新目录 / 新随机秘密，禁止自动恢复旧演示 |
| 构建环境 | 当前 manifest / lock 的 DuckDB crate 为 `1.20000.1`；实际链接引擎版本由 M1 构建后查询 `SELECT version()` 记录，不从 crate 名或缓存推定。需要动态库时显式配置其路径并记录，不能靠未知历史 target 缓存冒充干净构建 |

纯本地明文 HTTP 只供开发验证，不开放 CORS，不嵌入 UI，不支持远程访问或生产认证。启动配置 / 凭据无热更新 API；轮换需重启新会话。开发机上的其他进程仍可能接触本地服务，本 profile 不是对恶意本机用户的隔离保证。

## 3. 固定模型、设备、绑定与可信范围

所有 ID、值和地址均虚构。租户 `ten-demo`、项目 `proj-demo`、产品 `prod-demo`（key `demo-temperature`）、资产 `asset-demo`；模型 `mr-demo-1`、版本 `1.0.0`，启动时创建为不可变 published 快照。模型只有 `temperature`：`float64`、只读、范围 `[-100,1000]`、单位 `{"code":"Cel","symbol":"°C"}`；不支持任何复杂类型、文本或整数属性。历史 deadband 固定 0，不合成心跳观测。

| 设备（key 同 ID） | 名称 | 模型 | active 绑定 | 稳定属性资源 | canonical_device_uri |
|---|---|---|---|---|---|
| `demo-a` | 演示设备 A | `mr-demo-1` | `br-demo-a-1` | `res-demo-a-temperature` | `uns://demo/lab/demo-a` |
| `demo-b` | 演示设备 B | `mr-demo-1` | `br-demo-b-1` | `res-demo-b-temperature` | `uns://demo/lab/demo-b` |

两个绑定均引用 `temperature`，`source_kind=gateway_measurement`、`normalization_owner=gateway`、运行模式 `gateway_normalized`，来源 `gateway-demo`。绑定追溯元数据可固定虚构 Modbus 地址 100、factor 0.1 / offset 0；引擎不访问寄存器、不计算倍率、不声称反推 raw 协议值。上报 245.6 保存 / 返回 245.6，不变成 24.56。raw 历史在此指**未聚合的工程观测**，不是寄存器原始值。

模型 / 绑定内容及资源 ID 在该会话不变；设备不迁址、不升级、绑定不切换；采样有效期下界固定为 UTC 毫秒 1（合成 fixture，不是现场激活证据）。来源只支持固定活动 `source_epoch="demo-epoch-1"`，其他格式合法 epoch 返回 `422 CAPABILITY_UNSUPPORTED`，不进行 epoch 自动切换或 seq 跨 epoch 比较。重启新目录后是新易失会话，不承诺跨会话去重。

| 凭据代号 / 环境变量 | 服务端内部身份 | 动作与固定授权 |
|---|---|---|
| G / `IOTFAST_DEV_INGEST_TOKEN` | `southbound_source`，caller `gateway-demo`，tenant/project 如上 | `telemetry:ingest`；source `gateway-demo` 的 grant 仅 `demo-a`、`demo-b`，grant 的 `valid_from_ms` 与 `valid_to_ms` 均检查 |
| R / `IOTFAST_DEV_READER_TOKEN` | `northbound_service`，caller `svc-demo-reader` | `view:read`、`history:query`；仅显式资源 `res-demo-a-temperature` |
| ALL / `IOTFAST_DEV_ALL_TOKEN` | `northbound_service`，caller `svc-demo-all` | 同上；显式资源 `res-demo-a-temperature`、`res-demo-b-temperature`，不是 `*` / tenant-wide |

认证只接受单个 `Authorization: Bearer <token>`，逐请求实际验证 secret 与有效区间后从**服务端固定表**构建内部上下文；不能反序列化客户端 Scope，也不能从非空 token 推定有效。不支持转发身份信任。认证缺失 / 错误 / 到期 / 多重 Authorization → `401 UNAUTHENTICATED`；动作不符 → `403 FORBIDDEN_ACTION`。单资源读取对不存在与范围外均 `404 RESOURCE_NOT_FOUND`；历史混合授权与未授权资源时整次 404，不泄露任一数据。先验证当前身份 / action / 范围，后读缓存、去重或 DB，重试亦不能绕过到期 / grant 撤销。

拒绝客户端自报 Scope：body 未定义的 `tenant_id`、`scope`、`scope_filter`、`allowed_actions`、`caller_kind` 等按未知字段 422；任何查询参数均不受支持；`X-Tenant-Id`、`X-Role`、`X-Scope`、`X-Caller-Id`、`X-User-Id`、`X-Allowed-Actions` 头在有效认证后返回 `422 VALIDATION_FAILED`，不是合并权限。没有凭据只带伪造头仍优先 401。`X-Request-Id` 只追踪不授权；若提供，仅接受 1–128 位 `0x21..0x7E`，否则 422，缺失由服务端生成。

## 4. HTTP 端点与 wire

除健康探针外全部受 §3 认证保护。请求 JSON 仅 `Content-Type: application/json`（允许 UTF-8 charset）；不支持压缩。统一对所有对象层拒绝未知字段、重复 JSON key；结构先校验、后转换内部类型。每个 POST body 最大 65536 bytes、最大 JSON 嵌套深度 8，超限整次 `422 VALIDATION_FAILED`；空 body、非法 JSON、错误 Content-Type 亦 422。HTTP 层必须在耗尽内存前限制长度 / 读取时间，不记录 body 或秘密。

| 端点 | 成功与最低响应字段 | 限制 |
|---|---|---|
| `GET /health` | 200 JSON `{"status":"ok","profile":"orca-mvp-dev-v1","durability":"volatile"}` | 开发探针扩展；不返回路径 / secret / Scope / 数据。启动未就绪不监听 |
| `POST /v1/telemetry` | 新消息 202；重复 200，ACK 见 §5 | G；一次只对应一个设备，1 个合法点，任何非法点 / 重复属性整条拒绝 |
| `GET /v1/devices/{device_id}/view` | 200；按北向 §5.2 形状返回下述业务视图 | R / ALL；这是本演示 latest 的公共路径，不另造 `/latest`、不直接序列化原型 `unit_code` / `unit_symbol` |
| `POST /v1/history/query` | 200；按北向 §7.2 raw 响应，见 §6 | R / ALL；仅有界全结果，无分页、SQL、聚合或插值 |
| 已知但未实现的 `/v1/source-state`、`/v1/ingestions/{ingest_id}`、`/v1/devices/{id}/shadow`、控制面 / WS | `501 CAPABILITY_UNSUPPORTED`（有效凭据后） | 不调用内部旧实现。其他不存在路由统一 404；已知路由的未支持方法 501；均用统一错误体 |

### 4.1 遥测完整请求子集

保持权威字段名与嵌套；禁止 `{ "Float64":245.6 }` 等 Rust 枚举编码。下面时间仅为格式样例，M2 执行时用当前 UTC 毫秒替换以测试新鲜度，不称现场数据。

```json
{
  "spec": "iotfast.telemetry.v1",
  "message_id": "demo-msg-001",
  "source_id": "gateway-demo",
  "source_epoch": "demo-epoch-1",
  "device_id": "demo-a",
  "model_revision_id": "mr-demo-1",
  "binding_revision_id": "br-demo-a-1",
  "points": [{
    "property": "temperature",
    "value": 245.6,
    "source_at_ms": 1794240000000,
    "source_time_origin": "gateway_sample",
    "source_seq": "1001",
    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
    "delivery": {"replayed": false}
  }]
}
```

所有示例字段必填，唯 `quality.source_protocol` / `source_code` 允许缺失或 null；非空值为最长 128 字节 ASCII 字符串。`spec` 固定；`message_id` 1–128 位可打印 ASCII `0x21..0x7E`，epoch / ID 采用公共契约格式，设备 / 模型 / 绑定必须匹配 §3。`value` 是有限 binary64 JSON number（不是字符串、bool、对象或 null），且满足模型范围。`source_at_ms` 为 `1..=9007199254740991` 整数，服务端拒绝超过接收时刻 60000 ms 的未来值，`422 CLOCK_SKEW_EXCEEDED`。仅支持 `source_time_origin=gateway_sample`；其余权威合法来源在本 profile 返回 `422 CAPABILITY_UNSUPPORTED`，未知枚举 422 VALIDATION_FAILED。`source_seq` 为无前导零十进制 u64 **字符串**，范围 `0..=18446744073709551615`。`quality.state` 支持 good / uncertain / bad / unknown，`delivery.replayed` 必须 bool。

源 / grant 不符为 `403 SOURCE_NOT_AUTHORIZED`；合法设备上的模型 / 绑定 revision 不符为 `409 REVISION_CONFLICT`；未知属性、空 points、重复属性、缺字段、非法类型 / 标识、超范围、超过 1 点为 422 VALIDATION_FAILED。必须验证整个数组；即使超点数，也不允许写入第一个合法点。

### 4.2 latest 响应形状与时间质量

顶层固定包含 `request_id, device_id, device_key, device_name, product_id, product_key, model_revision_id, model_version, asset_id, canonical_device_uri, active_binding_revision_id, source_connection_state, view_generated_at_ms, view_consistency, properties`；元数据取 §3，`view_consistency="per_resource_latest"`，`source_connection_state="unknown"`（没有状态事件，不从测量或 HTTP 连接猜 connected）。

`properties.temperature` 固定字段为 `resource_id, name, data_type, unit, binding_state, observation_state, value, quality, quality_reason, freshness, value_sampled_at_ms, quality_observed_at_ms, received_at_ms, source_time_origin, binding_revision_id`。`data_type="float64"`、`name="温度"`、unit 为 Cel 对象、`binding_state="bound"`。无观测时 `observation_state="no_observation"`、value 与三个时间 / time origin 均 null、quality unknown、quality_reason `no_observation`、freshness unknown，binding_revision_id 仍为静态绑定；不能填 0。有观测时 `observation_state="observed"`，返回原工程值、原测量质量、采样 / 服务端接收时间、来源与绑定版本，quality_reason null。

比较仅限同固定 source/epoch/resource，排序 `(source_at_ms, u64(source_seq))`；严格较新才替换 latest，相等或较旧只留历史，不覆盖 latest。不得按字符串比较 999 / 1000。`value_sampled_at_ms` 和 `quality_observed_at_ms` 此子集均为该最新测量原始采样时间，不因 GET、补传或 stale 检测刷新。freshness 在每次 GET 用可信当前时间计算：`now_ms - value_sampled_at_ms > 30000` 为 stale，否则 fresh；质量与新鲜度独立，bad + fresh 仍 bad，good + stale 不可当实时正常。

bad / uncertain / unknown **非 null 实测值**可合法入库，latest 原样携带质量；无新测量的故障不得向 telemetry 发送 null 或复制旧值并改采样时间。本 profile 不实现状态事件、点位故障证据投影或连接恢复；对应 T06 / SC08 未实现，不能用 bad 实测值用例冒充通过。

## 5. 原子接收、去重、ACK 与 volatile 边界

1. 每请求先认证、有效期 / grant / action 检查，再结构校验；以认证 tenant 与 `(source_id, source_epoch, message_id)` 形成 message_key。同 key 并发串行化，既有 key 用首次不可变模型快照按公共契约 `sha256-jcs-v1` 比较内容；排序 property、缺失原质量字段补 null、排除 delivery.replayed / request_id / Prefer，binary64 负零规范化 0。observation_id 严格使用公共 §4.1 的 SHA-256 / Base32 确定性公式，不新造协议。已有 key 同内容返回 duplicate，不同内容 409，不能覆盖原值。
2. 新 key 完成全部属性 / 范围 / 版本 / 时钟校验后才分配可见接收记录 / 进入待处理队列。任何拒绝均不改模型、绑定、grant、历史、latest、去重、待处理消息或应用 WAL；只允许脱敏请求日志 / 计数。M2 需比较拒绝前后的状态与历史，并用同 message_id 的修正合法请求证明没有占用去重键。
3. **保持总体 §21.3 新 ACK 的 pending 语义**：先在进程内登记完整批次与 ingest_id 并排队，返回 202。由单一消费者在一个 DuckDB 事务提交历史、latest、去重提交记录与 history_status，再使查询可见；禁止将现有同步 `history_status=committed` 原型 receipt 原样返回新 ACK。是否进程排队后立刻被消费不影响 ACK 所表示的接收时点。
4. 新 ACK **精确核心字段**为 `request_id, ingest_id, message_id, status="accepted_volatile", history_status="pending"`。不输出虚构的 `wal_position=000…:000…`，不输出 accepted_durable。duplicate 为同样五字段，status duplicate，原 ingest_id，history_status 为当前 pending / committed / failed。无 ingestion 查询；客户端用有界 raw 查询确认已提交事实，不能把 ACK 视作可查询。ingest_id 为本会话内服务端生成且唯一、不承诺跨会话格式 / 可恢复。
5. 默认 / `Prefer: durability=volatile` 均易失；显式 `Prefer: durability=durable` **总是** `503 DURABILITY_BACKPRESSURE`、retryable true、retry_after_ms 1000（须先认证，且拒绝发生于任何接收副作用之前，包括重复消息请求）。提示此 profile 不支持 durable、需切换已验收配置，不是重试就会恢复；未知 / 冲突 Prefer → 422。不能成功降级。
6. session 去重 / pending / receipt 保留至进程退出，无定时清理；最多接受 10000 个唯一观测 / 消息，达到上限或待处理队列满时新 key `429 RATE_LIMITED`、retry_after_ms 1000；已知 key 仍可返回 duplicate。待处理队列上限 256 消息；限制为演示资源保护，不是容量结论。不承诺重启去重 / 重放 / 完整性。DB 提交失败须回滚整个事务，登记 failed 并停止新消息准入（503），不得返回虚假 committed 或在 latest 留半批；查询已有已提交数据不造补值。
7. 即使 DuckDB 已提交、存在内部 WAL 或文件恰好可读，也只承诺本次运行观察到的事实，不声称应用 WAL + DuckDB 联合故障可靠、断电无损或生产可恢复。客户端超时是未知结果；同进程内以原 key / 语义重试，不生成新 key。进程退出后的未提交消息可以丢失；新目录会话不能证明旧消息未曾被接受。

## 6. raw DuckDB 历史：有界、全结果、无游标

采用北向 §7.1 **原字段**：`resource_ids, range:{start_ms,end_ms}, mode, aggregation, quality_states, order, limit, cursor`。本 profile 八项均必填；`resource_ids` 为 1–2 个互不重复的固定资源，range 为上述安全整数内正 UTC 毫秒、`start_ms < end_ms`、跨度 ≤3600000 ms，严格 `[start_ms,end_ms)`。`mode="raw"`、`aggregation=null`、`cursor=null`；quality_states 为四种质量的非空无重复子集；`order` asc / desc；limit 整数 1–1000。不支持 SQL、表名、自由过滤表达式。字段 / 范围 / 重复值 / 配额参数错误 → 422 VALIDATION_FAILED；mode aggregate / 插值、非 null aggregation / cursor → `422 CAPABILITY_UNSUPPORTED`，不偷偷忽略。

先认证与全部显式资源授权，再开启本地 DuckDB **单次只读事务快照**，以受限参数查询 `limit+1` 条。总匹配数 ≤limit 才返回全结果；如多出一条，整次 `422 VALIDATION_FAILED`，details.field `limit`、issue `RESULT_LIMIT_EXCEEDED`，无 items / 无 cursor，让调用者缩小时间窗 / 资源集合（最多提高到 1000）。不得截断后返回 `partial=false`，不得返回无签名假游标或声称实现稳定分页。单次 snapshot 不是跨请求稳定快照；迟到观测可能改变相同查询的后续结果。

成功响应保持北向 §7.2 字段：`request_id, target_id="local-duckdb", source_mode="managed_ingest", mode="raw", consistency="strong_snapshot", partial=false, warnings=[], coverage, items, next_cursor=null`。这里 strong_snapshot **只指本次 DB 事务**。coverage 固定 `{"contiguous_wal_position":null,"event_time_complete_through_ms":null,"event_time_completeness":"unknown","known_gaps":[]}`；没有已知 gap 证据不代表事件时间完整，不覆盖进程丢失 / 尚 pending 数据。

每 item 必含 `observation_id, resource_id, device_id, property, model_revision_id, binding_revision_id, source_at_ms, received_at_ms, source_time_origin, value_type="float64", value, unit, quality, source_quality_code, replayed, record_kind="measured"`，按北向 §7.2 表达，不泄露物理表名。`quality` 为字符串而非入站嵌套对象，unit 为 Cel 对象，source_quality_code 取首次入站 quality.source_code（缺失 null），replayed 保存首次投递标记（重试变化不改已保存观测）。排序 `(source_at_ms, observation_id)`，desc 同时倒排两键，不按请求资源排列。首次合法观测与迟到观测全部入历史，重复 key 不新增；原始协议质量、source_seq 与 hash 所需原数据内部保留，不以追加必填外部字段改变权威历史 wire。

## 7. 错误信封与 M2 验证矩阵

所有错误统一 `{"request_id":"<safe-id>","code":"<CODE>","message":"<脱敏说明>","retryable":false,"retry_after_ms":null,"details":[]}`；逐字段错误为 `{field,issue,description}`。401 / 403 / 404 / 409 / 422 / 501 retryable false；429 / 503 true、retry_after_ms 1000。同 key 不同语义内容明确为 `409 IDEMPOTENCY_CONFLICT`，不是笼统409或版本冲突。内部错误用脱敏 `503 DURABILITY_BACKPRESSURE` 停止准入，不泄露 SQL、文件路径、secret 或他域资源。字段说明只回显调用方提交的安全字段路径，不附授权全集。正常 ACK / view / history 均有 request_id；HTTP body 与状态不得矛盾。

以下全部是 **M1 待实现 / M2 待实测**，M0 没有给任何功能 PASS。M2 每项记录完整命令、退出码、输入 / 预期 / 实际、OS / 工具 / DB 引擎版本；非零为失败，跳过不是通过。修改 fixture 时间用当前 UTC，stale 用独立的旧采样值测试，不靠无限 sleep / polling。

| ID | 正例 / 反例与核心断言 |
|---|---|
| M01 启动 | 正常启动 health 为 profile/volatile；非 loopback、缺失 / 相同凭据、非空数据目录均非零退出且未监听；不破坏旧目录 |
| M02 认证与时效 | G/R/ALL 真凭据成功；无 / 错 / 多重凭据与过期、grant 尚未生效 / 已到期均拒绝；用可注入可信时钟单测端点前置检查，HTTP 不提供改时钟入口 |
| M03 伪造与隔离 | 只有伪造 Scope 头 401；有效 token 加伪造头 / body / query 422；R 读取 B 404、R 查询 A+B 整次404、G 读 / R 写403；确认无响应数据 / 未授权属性名 / 数量泄露 |
| M04 两设备 / 工程值 | A=245.6、B=31.2 独立写入；GET view 不串值、不二次乘 0.1；fresh good、采样时间 / 单位 / 版本正确；初始空值为 null 而非0 |
| M05 wire 严格拒绝 | 合法点混未知点、重复属性、>1点、null、枚举型 Rust value、错类型 / 超范围、未知 / 重复 JSON key、未知spec、超长 / 过深、数值型 / 越界seq全部422，历史/latest/去重/队列/WAL无副作用；同key修正请求可成功 |
| M06 绑定 / 来源 | B用A绑定409；模型错误409；错source / 未授权device403；不支持epoch / 时间来源422 CAPABILITY_UNSUPPORTED；不能按新版本解释 |
| M07 顺序 / 质量 | 先新后旧均入历史、latest保新；同毫秒999/1000按u64；重放不改quality；bad实测仍bad；good旧采样stale且时间不变；future +60001ms整条422 CLOCK_SKEW_EXCEEDED |
| M08 去重 / 原子 | 同key同内容并发与replayed变化均原ingest_id、单条历史；改value409；其他拒绝修正后不占用key；比较JCS与公共observation_id黄金向量，不能只测JSON可解析 |
| M09 ACK / 背压 | 新202 accepted_volatile+pending；重复200当前history_status；durable请求含重复场景均503且无副作用，永不accepted_durable；满队列 / session配额429；受控DB失败原子回滚且准入停止 |
| M10 历史 | raw字段 / 类型 / 版本 / 单位、工程值、稳定单次排序、[start,end)边界、质量过滤、空结果、双设备正确；cursor/aggregate拒绝；时间跨度/资源数/limit越界422；limit+1超额422且无截断成功 |
| M11 可见性 / 文件 | ACK后用有限次数（最多50次、间隔100ms）查询验证history最终可见，超时失败；Ctrl-C后独立只读DuckDB核对观察行与发送清单；该操作不证明crash恢复 / durable |
| M12 未实现边界 | source-state / shadow / ingestion查询 / WS / 动态配置明确501；旧原型内部入口不得被HTTP绕过认证公开；无CORS、控制、任意SQL、生产宣称 |

## 8. 缩减、兼容影响与未完成项

| 项目 | 本次决议与兼容影响 |
|---|---|
| wire / 原型差异 | 入站仍是权威裸 JSON 工程值 + quality/delivery 对象；不是 `TypedPointValue` 的 serde 枚举。latest仍为 `/view` 的unit对象；ACK不泄露内部receipt。原型内部类型可以复用，必须显式转换 / 严格验证，不能要求外部客户端迁就原型 |
| Q03 字段子集 | 只支持一个float64属性、gateway_sample、一个固定epoch、1点、严格未知字段 / 重复key策略及本文限额；这是可拒绝的演示能力收窄，不宣称完整P0 JSON Schema/OpenAPI、复杂值或跨语言JCS全部覆盖 |
| Q12 ACK | 选择总体§21.3明确的202 pending，然后异步DB提交；不用原型同步committed作为新ACK，也不把durable混入history_status。GET ingestion状态仍未实现，因此不宣称完整P0接收状态查询 |
| Q08 历史 | 仅单次有界全结果、cursor必须null、超额整次拒绝；不是P0稳定分页 / 防篡改游标实现。调用方必须缩窗，不能按next_cursor翻页；后续完整分页另验收 |
| Q02 安全 | 实际验证临时共享开发秘密 + 固定显式资源映射 + 有效期；不是mTLS / JWT生产签发 / 撤销 / 安全网关方案。query_history必须逐请求校验当前时间，grant必须检查上下界，HTTP不得接收可反序列化Scope或* |
| Q04/Q05 | source-state与WS全部未实现；连接未知，freshness独立。SC08/T06、SC10/T09不能算通过；不触碰终止帧命名冲突 |
| Q06/Q09/Q11 | 固定不可变模型 / 绑定 / 设备 / 资源，无升级迁址 / 动态激活 / 网关ACK；不解决历史路径复用、版本迁移、绑定崩溃对账 |
| Q07/Q10 | only volatile、session限额、全新空数据目录；不关闭WAL损坏/同步/恢复、生产保留/容量/离线窗口问题。无可靠重启、故障联合可靠性或性能SLA |
| Q01 工程 | 仅复用原型作本地演示；真实构建、HTTP smoke、实际引擎版本仍由M1/M2给证据；不由本文认定正式生产工程决议完成 |
| 后续 / 范围外 | 生产认证安全、可靠性/恢复/性能、真实网关联调/点表签认、外部TSDB、WoT/Sparkplug、动态配置API、订阅、UI/MCP/控制均未实现或另属上层；不为了演示默认开启 |

M0 通过条件仅为：两份授权文档写入、相对链接可达、字段/端点/错误/测试矩阵明确、diff检查成功、证据如实记录及归档有回执。M1 通过须真实构建与实现验证，M2 通过须独立HTTP结果；任何一项非零不能以其他文档PASS抵消。协调者审核后按其卡推进，不将侧审尚未产出作为无限前置，也不将侧审建议反向覆盖本wire。
