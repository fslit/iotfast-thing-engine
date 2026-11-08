# F00-D 原型风险静态核查

状态：静态审查完成，运行复现未完成。源码与契约基线 `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`。本次环境缺 Rust/Cargo，不能编译、运行或断言攻击可行；本报告不改变历史证据，不代表正式服务存在相同缺陷。优先级是后续实现与发布门禁建议，非已发生事故。

## R01 — volatile 的状态与 WAL 位置不一致

- 证据：`prototype/src/storage_duckdb.rs:202` 按 durability 选择 WAL；volatile 使用全零哨兵；第 210 行调用同一 apply；第 212–224 行返回 `accepted_volatile` 且 wal_position 为 Some(哨兵)。同文件第 492–500 行写入 ingest_status 的固定字面值 `accepted_durable` 和该位置。
- 静态结论：上述代码路径存在初始返回值与状态表字面值不一致，且返回哨兵位置。尚未验证运行时查询暴露方式、失败行为与重启结果，不能据此断言物理数据丢失。
- 影响：可能使调用方误解可靠接收语义；生产实现对应能力的验收阻塞。
- 最小复现设计：在隔离 DuckDB/temp WAL 中构造有效消息、调用 volatile 入站、读取 receipt 和 ingest_status 及位置；重启后查询。另测 durable、重复消息、同步失败和恢复。按 F06 将语义、存储与 API 一并修正，不能把 sentinel 视为真实 WAL 确认。

## R02 — 控制面入口有效期与可信身份

- 证据：`prototype/src/scope.rs:25–35` 可信上下文实现 Deserialize 且字段公开；第 55–72 行的 `validate_active` 检查上下文，第 75–89 行的 `require_action` 只检查动作集合。 `prototype/src/model_catalog.rs:74–98` 的 create_product、`prototype/src/asset_uns.rs:114–120` 的 create_asset 和 `prototype/src/binding.rs:87–94` 的 create_binding_draft 各函数入口调用 require_action，未在所示入口调用 validate_active。入站 `prototype/src/ingest.rs:88` 则有调用。
- 静态结论：以上三条原型公开函数自身未完成有效期检查，传入已经构造的过期上下文存在被直接调用的风险；其具体影响取决于实际调用链。原型无真实 HTTP 认证层，不能据此声称已存在可远程利用入口。Deserialize 本身不是漏洞，风险在于不可信输入可否直接到达可信构造边界。
- 最小复现设计：创建 scope，allowed_actions 含所需动作但 expires_at_ms 已过去；直接调用三函数，分别断言拒绝且数据库无副作用；再针对真实 transport 验证凭据、audience、过期、空 scope、跨租户和 body 伪造。F04/F05 定义统一鉴权入口、不可伪造构造及服务层防御；记录实际外部身份方验证状态。

## R03 — 控制面审计覆盖

- 历史测试总结第 125 行报告：仅 publish_model 写 audit_log，其他控制面操作待补；`prototype/src/model_catalog.rs:322` 可见 publish_model 插入 audit_log。
- 静态边界：未对整个代码树做穷举搜索，也未运行事务回滚测试，不能断言其余所有调用路径都无审计或审计一定与写操作原子。
- 验证设计：枚举每个管理动作/入口，记录成功、拒绝、回滚时的审计行为、身份与对象，并在事务中途注入失败核对对象和审计状态。身份或范围不足应无业务副作用；审计字段需脱敏。归属 F05，管理操作的生产验收阻塞。

## R04 — cursor 签名算法与保护范围

- 证据：`prototype/src/northbound.rs:479–497` 和 `:534–542` 将 secret、NUL 和 payload 输入 SHA-256 后比较摘要。它不是 HMAC；报告只描述实现，不声称已验证某种攻击。当前 payload 包含 tenant、scope_hash、query_fp、末尾时间和 observation ID。
- 验证设计：分别测试单字节篡改、跨 scope/tenant/查询复用、过期、密钥轮换及不同进程；核对真实接口/配置、签名验证和日志脱敏。评估标准 HMAC 或经审查的认证加密库及版本化 token；不能自制密码协议。归属 F07/F08；生产 cursor 认证验收阻塞。

## R05 — 时间来源和资源 ID

- 时间证据：`prototype/src/ingest.rs:383–386` 南向入站接受 `external_source`；`docs/specifications/service-identity-and-scope-contract.md:136` 将该来源限定为外部引用或受控导入。静态对照表明允许集合比 P0 南向契约宽；需运行完整请求验证实际最终响应和副作用。
- ID 证据：`prototype/src/asset_uns.rs:302–307` 将 device_id 与 property 拼接并把 property 的下划线替换为连字符，存在表达式层面的潜在碰撞；能否被现有标识符和模型校验约束阻止尚未证实。不能宣称实际产生覆盖或跨租户泄露。
- 验证设计：构造合法范围内的相似 ID/属性（含连字符、下划线和分隔符），检查创建、约束、查询和迁移；先验证输入确为合法值。构造 source_time_origin 为 external_source 的 telemetry 与对应 source_state/引用模式分别测试，核对拒绝及无副作用。归属 F03/F05/F06。

## 交付状态

静态证据：R01、R02、R04、R05 的代码行已核对；R03 依赖历史报告并检查了一个插入点。实际运行复现、数据库状态、完整入口覆盖、网络攻击、真实身份系统和真实设备均 NOT_RUN/BLOCKED。待获得 Rust/Cargo 和隔离检出后，按每项最小复现写测试并记录环境、SHA、命令、退出码与实际结果；失败时不得标通过。
