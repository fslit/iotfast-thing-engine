# D00 集成规范与实现蓝图：身份、遥测、质量及北向服务

状态：基于 main `840c36f1b485266a595b6730aa5852b1b56ce126` 的跨模块对账与实施蓝图。其中 R01–R09 已同步收敛至 `service-identity-and-scope-contract.md` 与 `northbound-api-and-subscription-contract.md`，并在 `feasibility/` 下完成 Rust 1.98.1 + Python 3 跨语言 JCS 黄金向量、DuckDB v2.0.0 单事务与 SIGKILL 恢复、应用层 WAL F01–F05 与严格快照 S 的原型实测（见 [可行性验证计划](../development/feasibility-plan.md)）；真实网关（F4）、规模压测（F5）与硬件物理断电仍待现场执行。此文不覆盖 [数据引擎职责](../architecture/data-engine-scope.md)：用户权限、业务流程和设备控制由上层负责。

## 1. 目标与边界

统一控制面、服务身份、遥测、质量、WAL、设备业务视图和历史 / 订阅语义。P0：Rust 单服务、工程值入站、设备模型 / 实例、单设备一个 active 绑定、DuckDB 本地目标、应用 WAL；外部 TSDB 引用 / 双写与上层 MCP 属后续集成，不得假设已实现。DuckDB 2.0.0 是目标；不改变默认元数据存储决策。

权威约束：按请求认证服务身份并执行可信租户 / 资源 Scope；客户端不能自行填写角色 / tenant 授权；不依赖上层 UI 过滤。不存在同一进程或内网就自动豁免鉴权。最终用户认证与业务权限上层负责。

> 2026-11-10 状态澄清：本文件同时保存了早期审阅和后续收敛记录。§2 的“候选”、§4.2/4.4 的“未交叉验证”、§5 的“BLOCKED”、§10 的“未构建”是当时记录，不是当前统一状态。现行设计规则由[身份公共契约](service-identity-and-scope-contract.md)、[遥测契约](p0-telemetry-contract.md)和[北向契约](northbound-api-and-subscription-contract.md)承载；有限验证见[证据更正](../development/evidence/p0-core-engine-test-summary.md)。已有状态事件文字定义不等于原型已满足；已有有限向量不等于完整 Schema 或正式冻结。尚未收敛的问题与下一步见[规范决策台账](../development/specification-decisions.md)，遇冲突不得依据本文件历史候选放宽边界。

## 2. 主分支审阅后的歧义清单（历史发现，非当前阻塞状态）

| ID | 发现于 main | 问题 / 影响 | 本蓝图建议及状态 |
|---|---|---|---|
| R01 | service-identity §4.1 | observation_id 既说确定性派生，又允许首次随机生成；跨实现会不一致 | P0 只允许现有确定性公式，WAL 保存结果并校验；更改需要新版本。候选 |
| R02 | service-identity §4.2 | JCS 处理与另选原始十进制 / 位模式双重校验并列，未规定唯一输出；struct / array 未明确 | float64 先解析 binary64、有限数，投影按 RFC 8785；复杂类型 P0 明确限制或逐类型冻结；样例与跨语言验证未完成 |
| R03 | source-state §6 / telemetry §2 | 状态事件未定义完整去重域、排序 / ACK；描述“断连一律 bad”与源只能证明网关断线相冲突 | 区分网关连接事件和有依据的点位事件；事件可另设命名域；状态不得伪造测量。候选 |
| R04 | northbound §6.1 | 声称快照是 S 时刻却允许读到 seq>S，新客户端不能据此视作 S 精确快照 | P0 选取有界复制 / 版本快照，所有快照资源均为 ≤S 版本；无法保持则 resync_required；不可将较新状态称 S 快照 |
| R05 | 身份 Scope §2.2 | 非空维度“共同组成集合”未明确 OR / AND 和 tenant_wide 边界 | P0 定义为显式授予集并集，再与租户、动作和请求求交；tenant_wide 是经签名的同租户全部范围，不从空字段推导 |
| R06 | API 索引 / 旧范例 | identifier、格式、quality、错误码存在旧新表达，points 差异和 null 规则不一致 | 以现有 main 的新遥测 + 公共契约为基线；示例统一经过 Schema 测试；冲突需显式记录 |
| R07 | history 响应示例 | 从本地 WAL checkpoint 推导 event-time contiguous watermark 不成立；迟到数据可早于此时间 | watermark 分别声明 log_position 与 event-time 完整性；无法证明返回 null / unknown，不能写入虚构时间 |
| R08 | 确定性 observation ID | 检索请求时当前有效绑定、source_time 变更会拒绝之前成功消息 | 对已有 key 先重新认证 / 授权、使用首次接收时绑定和摘要 profile 对比；新 key 才做当前 / 历史生效校验 |
| R09 | 视图 / 字段级 Scope | 有权看 device 不等于有权看其全部资源；数据可能通过模型、点位数量、错误或订阅泄露 | DeviceView 逐资源过滤，禁止在没有资源级范围时附带未经授权属性 / 模型字段 |
| R10 | 冻结状态与测试证据 | 文本把规则标为冻结，但缺少可运行 Schema、黄金向量、真实网关与恢复结果 | 区分设计基线 / 接口规范冻结 / 实现与验证；不以文档认定生产可用 |

本表对 main 的矛盾做出面向实现的候选；是否修改现有对外契约及兼容影响须经评审后整体更新，不能偷偷改变已发布协议。

## 3. Scope：规范计算顺序

### 3.1 可信上下文

服务身份通过经配置验证的 mTLS 身份或可信签名令牌；校验 issuer、audience、签名算法白名单、有效期与吊销 / 配置版本策略。具体机制按部署锁定，不接受客户端自报普通头的 tenant / allowed_actions。认证通过后生成不可被下游覆盖的 VerifiedScopeContext：tenant_id、caller_service_id、caller_kind、allowed_actions、资源范围、expires_at、scope_revision / hash、quota_profile；operator_id 仅供审计。

### 3.2 范围候选语义

1. 租户先过滤。tenant_wide 仅当可信断言显式 true 时授予该 tenant 的全部资源，不能推导为跨租户；若策略禁止则拒绝。
2. 否则各非空 asset_ids（含明确的 include_descendants）、device_ids、resource_ids、uri_prefixes 解析为同租户的**并集**，所有维度为空为 empty scope。
3. 权限动作必须匹配调用端点；请求集合与已授权资源集合求交。但显式传入 resource_ids 的原始列表中有任何不可访问或未知 ID 时整批返回统一 404，不回显 ID 是否真实存在，也不在 details 中区分二者；目录扫描先过滤后排序分页。
4. asset / device 授权扩展到哪些属性由上层断言或部署协议明确。不能从 device 本身自动推导所有 property 均授权；如果上游只表达 device 范围，约定其语义为全属性需要显式测试并写在断言协议里。待身份实现决定后冻结。
5. 前缀只按规范化路径段匹配；对旧路径和重定向先验证稳定资源授权再返回当前路径。禁止泄露不可见的完整路径与节点计数。
6. 查询、视图、历史、聚合、缓存、订阅和游标都在同一已验证 scope 下执行；缓存 / 游标还要绑定 tenant、scope hash、动作、请求指纹、目标与配置版本、过期时间及签名 / 服务端令牌。范围变更 / 过期 / 撤销时拒绝旧游标，WS 需重新鉴权或断开，不能只在连接创建时校验一次。

Scope 并集是对 main 未明确词义的候选解释；不同类型范围应如何表达交集、例外 / 禁止等不在 P0 隐式猜测，应由统一断言格式提供显式表达并验证。授权更改、资产迁移与长连接竞态必须有失效策略；系统不能声称实时撤销直到实测并规定最大生效时间。

## 4. P0 测量与去重协议

### 4.1 消息键与观测 ID

测量消息键：经认证 tenant_id、source_id、source_epoch、message_id；质量事件使用独立类型域及自己的 event_id，禁止跨类型共用去重记录。对于测量点按 main 公式：
`obs- + Base32LowerNoPad(SHA256(UTF8("iotfast.obs.v1") || 0x00 || UTF8(tenant_id) || 0x00 || UTF8(source_id) || 0x00 || UTF8(source_epoch) || 0x00 || UTF8(message_id) || 0x00 || UTF8(property))[0:16])`。
`[0:16]` 为 SHA-256 digest 前 16 字节；Base32 按 RFC 4648 字母表转小写、无填充；不得哈希 hex 文本。分隔符 0x00；标识字段需在解码后拒绝 NUL 和不符合字段语法的字符，避免拼接歧义；property 语法 / 长度与规范版本必须冻结。相同 message_key / property 多次发送是同一逻辑 ID；同 ID 不同内容是冲突，不覆盖。UUID / 随机 ID 不属于该 P0 profile。

### 4.2 payload_hash

main 选定 `sha256-jcs-v1`：校验并按 property 字节序对唯一点排序，投影 spec、可信 tenant_id、source_id、source_epoch、device_id、model_revision_id、binding_revision_id 和 points；每点 property、由被引用的不可变模型确定的 value_type、value、source_at_ms、source_time_origin、source_seq、quality.state、source_protocol 与 source_code。缺失的两个可选质量字段归一为 JSON null；replayed / request_id / Prefer / received_at 不在摘要。source_seq 为 main 的十进制 u64 字符串。

选用 RFC 8785 JCS 和 SHA-256，JCS 不重排数组，因此 points 排序须在 JCS **之前**执行；对象键序按 RFC 8785，不能用本地语言默认排序代替。float64 按 binary64 + JCS 序列化，拒绝 NaN / Infinity，-0 映射为 0；int64 在语义投影中使用十进制字符串，范围 / 唯一拼写须由可执行 Schema 和正式测试冻结。字符串保留原始 Unicode，JCS 本身不进行 Unicode 标准化；重复 JSON key 必须在解析阶段拒绝。大整数不得经 JS Number 静默丢失。

main 将摘要 profile 称为冻结，但**没有可运行的跨语言完整黄金向量**；本文仅提供受限探针。旧草稿 PR #1 中“保留 points 顺序”的候选与 main 的排序冲突，不可将其直接合并覆盖；新设计基于 main 排序。

### 4.3 首次接受与重试

顺序：验证调用服务 / 当前 Scope → 结构合法且获得 key → 在同 key 的并发协调域查历史接受记录 → 如已存在，按首次记录所用 profile、模型和绑定快照构造 hash（不使用新绑定重新解释），一致返回同 ingest_id / 当前状态，冲突返回 409；引用快照缺失或已不在保留期时返回明确不可确认错误，不能伪造 duplicate → 新 key 才做完整生效期、source_seq、时钟、模型、数据源和整批校验 → 对所有点生成固定 ID → 接收。

撤销 scope 后不得借重复请求查询历史状态。持久接收的可恢复记录含 key、hash / profile、原已验证观测、映射版本、ingest_id 与 observation_ids；同 key 并发仅一个首次接收，WAL 同步完成后才 durable ACK。volatile 重启后不能承诺去重。去重保留期、旧模型快照保留期与最大重试窗口必须一致规划；过期不再宣称可识别。

### 4.4 受限黄金样例

输入字段：tenant=ten-8iic、source=gateway-01、epoch=epoch-001、message=msg-001、property=outlet_pressure。按 §4.1 派生：
`obs-mfwh4ejs2b7rtlcyzrqkyufufa`。

下面是摘要投影（已排序、可选质量字段归 null）；示例 hash 不包含代码块换行：

```json
{"binding_revision_id":"pump-bind-r1","device_id":"pump-001","model_revision_id":"pump-model-r1","points":[{"property":"outlet_pressure","quality":{"source_code":null,"source_protocol":null,"state":"good"},"source_at_ms":1700000000000,"source_seq":"42","source_time_origin":"gateway_sample","value":0.42,"value_type":"float64"}],"source_epoch":"epoch-001","source_id":"gateway-01","spec":"iotfast.telemetry.v1","tenant_id":"ten-8iic"}
```

长度 434 UTF-8 bytes，`sha256:8d7e884478d3420105475344f4b63187e937741eb68df08832b39b47e8958eae`。

本样例用 Python 3.12.14 的标准库对特定 ASCII 键 / 普通浮点计算，再用 Node.js v24.22.1 的预排序对象复核 hash；这两个方法**均不是完整 JCS 合规实现**。observation_id 由 Python 标准库计算，尚未用 Rust / Go 独立交叉验证。正式测试仍需 JCS 官方向量、Unicode、负零、指数、大整数、多点重排和重复键，并以真实网关语言核对。

## 5. 质量与状态事件

measurement 必须是非 null 的实际测量；接收状态事件 `iotfast.source_state.v1` 时不能写入测量历史或更新 value_sampled_at。应保存 connection_state 与 point_quality_state、各自 observed_at / 接收时间和来源证据。

- 网关断线只证明网关连接失联；缺少点位级证据时不自动宣称所有传感器 bad。可返回 connection=disconnected、freshness 按策略 stale / unknown；上层不把最后值显示为正常实时值。
- 若明确观测具体点位 Modbus 读取超时，可在 point_states 标为 bad，reason=modbus_read_timeout；同一事件恢复连接不自动证明新测量 good。
- 既有 source-state 示例缺少严格 event hash / time origin / epoch 顺序字段定义，不能仅凭 event_id 宣称 durable 幂等。P0 实施前需确定命名域、整事件校验、点位 / 设备粒度、重复冲突、时间与顺序、可信绑定、ACK 及原子恢复；暂列 BLOCKED。
- last_value.value_sampled_at 与 current_quality.quality_observed_at 分开。测量的原始质量不可被后来的状态事件修改；late events / source epoch 更新遵循明确排序，否则标 unknown 并记录冲突。

## 6. 模型、绑定和实体状态机

模型 draft → validated → published → deprecated / retired；published 的 model_body、model_item 在同一元数据事务提交并冻结，历史引用不能删除。设备实例与资源创建使用幂等 key，在单一元数据事务中创建稳定 ID / 路径和配置审计。

绑定 draft → ready → pending_gateway_ack → active → superseded；网关配置与服务元数据无法跨进程原子提交。服务端至少持久化 operation_id、old/new revision、config hash、期望设备 revision、阶段、网关可信 ACK、超时 / 停采边界。未确认 ACK 时旧版本继续有效或明确停采；网关先切换 / 服务崩溃后必须对账，无法确认时显示 binding_sync_pending 并拒绝无法证明版本的数据。人工停采模式需记录区间；两个 active 不能同时存在。旧消息是否在原有效采样时间允许写历史，按已验证绑定生效区间判断，无法证明则隔离 / 拒绝，不按新倍率处理。

事件时间、配置记录时间和接收时间不同；后改旧映射不等于原历史可重现。P0 只设一个权威 active，不自动双源切换。

## 7. 北向业务视图与订阅一致性

DeviceView 是 per_resource_latest，不承诺各点同采样周期。一个 device 含部分属性超出 Scope 时逐资源授权过滤；禁止未授权属性名、模型字段、计数或当前值通过 view 暴露。要求所有点时必须提供显式授权的资源集合，任何不可见 / 未知均返回统一不可见结果，不能返回看似完整的报表。latest 数值、质量、freshness、source 时间和状态事件时间独立。

### 7.1 严格快照协议

在同一有序更新域取得 S，并原子地捕获该时刻可见的资源版本（可选择短锁复制不可变状态或 MVCC 版本快照；实现应证明资源和内存边界）。注册缓冲收集 seq>S 后发 snapshot_begin、所有 ≤S 资源的 chunk、snapshot_end(S)、再按序发缓冲 delta。若无法捕获 S 快照、来源状态丢失、权限变更或缓冲溢出，终止本轮并 resync_required。禁止在 S 快照中混入 seq>S 且仍称其是 S 时刻的精确快照。

若将来改为“弱快照 + 按 resource_revision 幂等收敛”，必须使用不同 consistency 声明并规定客户端丢弃旧 revision；不与 strict_snapshot 混名。coalesce_slow_consumer 若可合并中间变化，标记 coalesced，只保证已授权资源最新状态收敛，不声称完整事件序列。断线重新拉快照，历史查已保存观测，不保证断线期间所有变化均进入历史（受 COV 策略影响）。

### 7.2 历史覆盖与水位

- WAL / 目标的连续 checkpoint 是**日志位置**，只能证明该流此前已处理到位置，不能证明某事件时间以前再无迟到数据。
- `event_time_complete_through` 仅当来源提供并保证水位、所有必要来源与迟到策略满足条件且可验证时返回；否则 null + unknown。不能用当前时间或最新采样时间代替。
- source / target / Scope / query fingerprint / snapshot boundary 必须绑定分页游标；无可验证稳定快照时说明 weak、可能重查 / 去重，不承诺跨页不漏。
- 当外部源缺失 observation_id / 质量、跨库不可达或已知 gap，报告 capability / partial；无法证明覆盖完整不能返回 complete=true 或无条件 200 空结果。

## 8. 接口矩阵与数据表边界

| 接口类别 | 例子 | 保护 / 核心语义 |
|---|---|---|
| 模型 / 设备管理 | 创建产品、草稿校验发布、设备实例化 | management_service + action / Scope，事务审计、幂等和条件修订 |
| 来源绑定 | validate / activate / ACK | 同设备最多一个 active；对账和操作追溯 |
| 南向 measurement | POST /v1/telemetry | scope + source grant；全批校验；可恢复 WAL / profile |
| 南向 source-state | POST /v1/source-state | scope + source grant；无新采样不写数值；事件契约待闭合 |
| 数据读取 | resource、view、shadow | 授权后返回可见字段、质量 / 时间 / 版本 |
| WS / history | snapshot + delta、受限 AST | 快照屏障 / scope 绑定，质量过滤与缺口披露 |
| MCP | 引擎无内置用户 / 角色 | 上层封装只读接口，继承 service scope 和限额 |

现有 product、model_revision、model_item、device、resource、binding_revision、point_binding、latest_value、point_quality_state、history_observation、applied_message、ingest_status、target_checkpoint 等逻辑表继续使用；质量事件、去重域与状态变化日志需要补充能验证的主键和 WAL 关联。数据表只提供存储结构，不代表约束已被数据库强制。模型、历史、状态、WAL 或外部库跨存储文件不得声称同一事务；具体 DuckDB 事务和崩溃语义须实测。

## 9. 交付任务与测试矩阵

| ID | 工作项 | 可验证结果 |
|---|---|---|
| I01 | 汇总 R01–R10 并更新主干权威规范 / 优先级 | 无含糊双定义；记录兼容变更 |
| I02 | 冻结可执行 telemetry / source-state / errors / Scope / northbound Schema 与示例 | 正负例验证；字段 / 限制 / 版本一致 |
| I03 | Rust + 实际网关语言跨语言 JCS / ID | 全量黄金向量逐字节 / hash / ID 相同；报告依赖版本 |
| I04 | 接收与事件状态机 | 重复 / 并发 / 已撤权 / 质量与测量顺序正确 |
| I05 | DuckDB + WAL 与重放 | sync、提交、checkpoint 各点强杀测试；断电单列 |
| I06 | scope / 防泄漏 | A / B 不同租户、部分属性、缓存 / cursor / WS、失效 / 资产迁址无越权 |
| I07 | 快照 S 并发 | 捕获时持续变化，所有 snapshot 版本 ≤S；溢出与撤权触发 resync |
| I08 | 历史与外部源 | 同毫秒、迟到、缺质量、未知 ID、partial 和水位不伪造 |
| I09 | 集成实际网关 / 业务 UI | 状态事件与 bindings 真正匹配；业务权限仍在上层 |

覆盖向量：点位乱序等价、null / 缺省、-0 / 大数 / Unicode、property / NUL、跨 epoch 旧点、旧绑定成功请求重试、部分 scope 视图、payload 多点部分失败全拒、写满无 durable ACK、网关断线无虚假测量、未来时钟不污染 latest、源恢复未测量不自动 good、订阅快照 S 期间发生多次同资源更新、late event 与 WAL checkpoint 分离。每项记录命令、二进制 / 库 / DB 版本、平台、预期与实际、限制及 commit SHA。

## 10. 本次审阅与证据

- main 的服务身份 / Scope、P0 遥测、北向 API / WS 和库表边界已读取并对照；用户提供的 main 新提交为基线。
- 样例 ID / hash 仅用 Python 3.12.14 计算，hash 的一个受限样例另由 Node.js v24.22.1 复核，均不是完整 JCS 实现或跨语言协议测试。
- 当前未构建 Rust / DuckDB，未执行网关、外部 TSDB、安全、故障、恢复或性能测试；项目现有草案与 PR #1 尚未合并状态应分别管理。
- 需补齐正式 Schema / OpenAPI、锁版本构建、真实网关样例与安全环境，再推进 D01 实施；本文不单方面宣称 D00 工程验收已完成。
