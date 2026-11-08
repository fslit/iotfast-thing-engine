# AI 执行计划：从设计验证原型到可交付的设备数据引擎

状态：研发计划，待执行。规划基线：`fslit/iotfast-thing-engine` 的 main `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`（2026-11-06）。执行前必须读取最新 main、重算基线并检查本计划与当前代码的差异；不能在旧 SHA 上盲目覆盖新工作。本计划为实施顺序，不代表正式服务已完成。

正式工程目录、逐文件职责和可领取任务卡详见 [AI 工程生成规范与任务拆解](ai-implementation-work-breakdown.md)。

## 0. 给 AI 的唯一入口

先读仓库根目录 AGENTS.md → 本文件 → README.md 与 prototype/README.md → docs/architecture/data-engine-scope.md → docs/specifications/README.md → 本任务引用的具体规范和实际代码 / 测试 → 任务依赖和证据报告。代码、配置与运行结果均以当前目标分支为准；已合并 main、仅在 PR 的内容及本地改动必须分开报告。

用户目标：Rust 独立设备业务模型与数据服务，标准化设备和点位、latest / 质量、历史查询与订阅，供八位魔方 / SCADA / MES / EMS / AI 上层调用。P0 复用现有网关上报工程值；用户 / 角色 / 业务权限、页面、告警工作流、MCP Agent 和物理控制属于上层；引擎仍必须校验服务身份和可信数据范围，不能裸暴露。

## 1. 当前事实与证据边界

| 观察 | 证据 | 判断 |
|---|---|---|
| main 以 prototype/ 隔离了 Rust 原型 | 根 README、prototype/README、prototype/Cargo.toml | 是设计验证原型，不是可部署生产服务；根目录尚无正式 Cargo workspace |
| 文档记录 15/15 集成测试、可行性探针和 Rust / Python 黄金向量 | docs/development/evidence/ | 属于当时环境 / 命令与测试范围的报告；不等于本次复现、真实网关联调、真实断电或完整安全验收 |
| prototype Cargo 声明 duckdb crate 1.20000.1 | prototype/Cargo.toml | crate 编号不可单独证明链接的引擎版本；运行 SELECT version() 并保存结果 |
| 现有 roadmap 多项标记 ✅ | docs/development/roadmap.md、prototype/README | 这些应按原型 / 报告范围解读，不等同正式工程及所有要求完成 |
| evidence 中记录若干已知缺口 | p0-core-engine-test-summary §4 | volatile 状态、source_time_origin、实体 ID、控制面身份过期检查和审计覆盖均需复核；涉及错误持久承诺与权限，不能简单按低风险处理 |
| 原型 scope 结构可从外部数据反序列化 | prototype/src/scope.rs | 它是授权上下文数据类型，不是经过验证的网络认证实现；生产入口必须验证凭据并控制构造 |
| 原型 cursor 使用 SHA256(secret + 0 + payload) | prototype/src/northbound.rs | 不等同规范中命名的 HMAC；正式设计应评估并采用标准消息认证方案，密钥来自安全配置 |
| 当前检视环境 | 2026-11-06 本次：rustc、cargo 不在 PATH | 本次未运行原型测试；不把此前报告重述为本次通过 |

早期证据文档中的 feasibility/ 相对路径和部分命令产生于目录迁移之前；当前文件实际位于 prototype/feasibility/。执行前用当前目录重算命令并校验链接 / 脚本会不会修改证据文件，保留原始记录，不能因路径失效自行伪造修复结果。

现有设计 PR #1（旧 D00 草稿）与 PR #2（基于更早 main 的契约对齐）仍为未合并草稿，当前 main 已含部分 D00 规范与原型。开始新任务前逐项比较内容、状态和 SHA；不得声称两个 PR 已合并，也不要直接合并旧候选覆盖新实现。未确认的代码风险仅作调查项，证实后附最小复现。

## 2. 执行规范和交付证据

每一任务在独立工作分支：检查当前 cwd / 仓库 / git status / HEAD / main 与未合并 PR；保留用户改动。写变更说明、运行相关测试、提供提交 SHA / 环境 / 精确命令 / exit code / 通过、失败、阻塞清单。验收失败不得勾选通过；外部环境、真实网关、目标硬件或凭据缺失记 BLOCKED，继续完成不依赖它的任务。

- 不把示例配置、测试 token、模拟网络请求、mock TSDB 和内存锁结果描述为真实接入或安全保障。
- P0 不建设用户 / 角色系统，不下发 PLC / 远程控制；不要连接客户生产现场，除非具备具体授权和隔离环境。
- 客户地址、真实设备数据、账号密码、token、证书私钥和签名密钥不得提交。
- 修改原型与创建正式工程各用单独任务和 PR；不能“清理”或覆盖 prototype/ 及证据。任何迁移应记录功能、测试、来源、行为差异和回滚。
- Schema / OpenAPI / JSON 样例以可执行验证为准；规范演进必须说明版本兼容、持久数据 / WAL 和旧订阅兼容。
- 设计冻结、代码实现、自动化测试、真实设备联调、部署验收分别计状态，不用一个 ✅ 概括。

## 3. 技术与仓库布局决策

当前仓库根目录刻意保持文档为主；prototype/ 是自包含 crate。正式工程建议 `engine/` 作为独立 Rust workspace（最终路径应先通过仓库 ADR 确定），避免改变 prototype 编译路径。建议 workspace 包按边界组织：

- `contracts`：版本化 DTO、错误码、JSON Schema / OpenAPI；所有 API 输入按正式版本。
- `domain` / `catalog`：模型、设备、资源、绑定、版本及兼容性。
- `runtime` / `wal`：观测、质量、latest、WAL、恢复 / 幂等。
- `storage` / `duckdb-adapter`：能力接口、单目标持久化、查询；不预设外部库同等能力。
- `api` / `server`：HTTP、WebSocket、服务身份验证、Scope 执行和启动 / 健康。
- `tests` / `fixtures`：契约、网络、故障、授权与跨语言黄金向量。

以上是候选，不强制先拆成很多 crates；需要 ADR 确认所有权、build features、依赖锁定、平台和代码迁移策略。正式工程 Rust，禁止把 AIOS 仓库的 Bun / Hono 技术栈套入本项目。

## 4. 执行依赖与门禁

1. **G0 事实 / 基线复核** → 2. **G1 契约与安全风险裁决** → 3. **G2 正式 Rust 工程和环境** → 4. **G3 可认证的只读最小纵切** → 5. **G4 WAL / 历史 / 故障可靠性** → 6. **G5 模拟网关与北向订阅** → 7. **G6 真网关 / 性能 / 交付**。各门禁都应留有证据；明确 BLOCKED 不等于允许跳过。

外部 TSDB、OEE、EMS、只读 MCP、资产关系等保持后续阶段；其设计须不破坏 P0 契约。外部连接可以独立做无生产数据的 PoC，但不能作为 P0 已通过或具备实际接入能力的证据。

## 5. 可领取的任务卡

### T00 — 基线、证据与原型差异复核（G0）

- 输入：本计划 §1 所列文件、两个旧草稿 PR、最新 main 与团队当前环境。
- 范围：只读检查；允许新增或修正文档链接和证据索引，不改原历史证据的实测结论。
- 执行：建立 clean checkout / 独立 worktree；记录操作系统 / CPU / 文件系统、Rust / Cargo、git SHA、DuckDB crate / 特性 / SELECT version()、构建依赖；先检查 Cargo.lock、构建是否拉取库。用临时目标与临时工作目录执行测试，避免覆盖 evidence.json。
- 建议命令（有完整工具链及依赖后，从仓库根执行；命令是待执行步骤而非本次通过记录）：
  ```bash
  rustc --version
  cargo --version
  git status --short
  git rev-parse HEAD
  cargo test --locked --manifest-path prototype/Cargo.toml
  cargo test --locked --manifest-path prototype/feasibility/f1_d00_contract/Cargo.toml
  python3 prototype/feasibility/f1_d00_contract/verify_vectors.py
  ```
- 对 verify_vectors.py 先阅读输入 / 输出路径及副作用；若依赖当前目录，进入实际需要目录运行。F0/F2 / F3/F6 报告写明会更新证据文件或含平台假设，必须先检查代码、在临时副本运行、注明 OS 限制。
- 产出：报告 PASS / FAIL / BLOCKED / NOT_RUN，区分“历史报告”和“本次复现”，列出旧链接、旧 PR 处理建议和差异；保留失败输出。无 cargo / 网络 / 本机 DuckDB 时报告 BLOCKED，不安装不明二进制替代。
- 完成条件：记录每个可重复命令、版本、SHA、实际结果及未运行项，不更改对用户的事实陈述。

### T01 — D00 契约、质量与互操作性缺口收敛（G1）

- 输入：service-identity-and-scope-contract.md、p0-telemetry-contract.md、northbound-api-and-subscription-contract.md、d00-integrated-implementation-blueprint.md、prototype/src/d00.rs、ingest.rs 和 feasibility/f1_d00_contract/test_vectors.json。
- 动作：核对规范与真实代码 / golden vectors；明确 measurements / source-state 独立事件与去重域、tenant/source/epoch/message ID、按 property 排序、缺失与 null、u64 / int64 / binary64、时间来源与时钟异常、binding revision 切换后重试。
- 安全：真实网关未支持稳定 ID / epoch / 版本时明示不支持可靠语义或使用经验证的持久桥接器；不能自行生成每次重试不同的 key。
- 产出：版本化 JSON Schema / OpenAPI（正式工程目录确立后）、正负 fixture、跨语言测试与字段迁移决议；不得把旧 PR #1 的保序候选覆盖 main 当前按 property 排序的规则。
- 测试：Rust 与网关实际语言输出相同规范化字节 / ID / digest；包括多点换序、重复 JSON 键、Unicode、-0、指数、int64 边界、质量 null / 缺省、跨 scope 重试。未取得网关语言 / 报文时跨真实网关标 BLOCKED。
- 验收：已发布契约与运行样例一致，剩余业务口径和版本策略明确；规范统一与实际运行是两个独立状态。

### T02 — 原型缺陷复现、分级与修复建议（G1）

- 范围：先以只读审查 + 回归测试复现，不做无依据的产品修复；记录根因、影响和任务归属。
- 核查 A：volatile 的接收返回、DB 状态、WAL position 与重启表现是否一致；不得以 durable 或伪 WAL position 表示 volatile。
- 核查 B：南向 source_time_origin 是否错误接受 external_source；应与 P0 仅允许的来源及真实契约对齐。
- 核查 C：实体 ID / resource ID 组合冲突、带分隔符字符串拼接和长度 / 字符约束；不凭字符串碰撞当作已发生事故。
- 核查 D：所有控制面入口是否执行调用方 active / action / 范围、租户和资源关系校验，以及审计是否在同事务提交；逐入口建立覆盖表。
- 核查 E：cursor 的 `SHA256(secret || 0 || payload)` 与 HMAC 的差异、密钥来源、scope / 查询参数 / snapshot / 过期 / 撤销绑定；用标准 HMAC 或经过评审的签名 / AEAD，不自行设计密码协议。
- 每项先有能失败的测试或明确不可复现说明，归入相应后续任务；安全、错误 durable 声明、数据破坏均属于发布阻塞，不沿用现有报告中的 Minor 结论。

### T03 — 正式工程初始化（G2）

- 前置：T00 复核和架构 ADR；T01 可并行完成，但外部契约未冻结前不得宣称稳定 API。
- 交付：独立的 Rust workspace、版本锁、格式与 lint、CI、配置校验、结构化日志、错误映射、健康检查和最小进程启停；不覆盖 prototype/。
- 环境：锁定受支持 OS / CPU、Rust 版本、DuckDB 库加载方式及编译特性；检查离线 / 运行时依赖、许可证、目标架构，生成构建记录；设计不同平台失败 / 缺依赖行为。
- 验收命令在实际 workspace 建立后执行：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked`；列实际 manifest 路径、版本和结果。禁止在 CI 宣称已运行但只提交配置。

### T04 — 服务身份、Scope 与缓存隔离（G3）

- 覆盖：HTTP / WS / 内部调用及管理、上报、view、目录和历史每个入口；生产边界只能由可信验证器构造 VerifiedScopeContext，不能反序列化客户端 body 当已认证上下文。
- 实现边界：选择受控 mTLS 或短期签名令牌，冻结 issuer / audience / 时间 / 签名算法、重放和 key rotation；上层完成用户业务授权，引擎负责服务身份与可信范围的强制执行。
- 防越权：租户、动作、资源过滤先于分页 / 聚合 / 统计；device view 逐属性；URI 归一化段边界；缓存与 cursor 绑定 scope、目标、请求及过期；长连接范围变化 / 撤销有明确再鉴权或终止策略。
- 测试：伪造上下文、过期 / 未生效 / 错 aud、跨租户、部分资源、资产迁移、旧 cursor / WS 撤销、无凭证；检查响应不能泄露被隐藏资源。按目标部署完成真实凭据验证，而不是仅 mock VerifiedScopeContext。
- 前置依赖：真实上层身份签发机制未确认时先定义可替换验证器及本地测试，生产认证验收保持 BLOCKED。

### T05 — 模型、资产、绑定与真实数据视图（G3）

- 发布不可变模型版本、模型 / 设备 / 属性资源稳定 ID、单设备一个 active 绑定；资产迁址不重用旧资源含义；旧版本与历史按生效快照解释。
- 建立配置事务、发布与审计原子边界；单独处理网关 ACK 和元数据无法分布式原子提交。停采 / 对账 / 重启和绑定切换跨 epoch 的策略必须有测试。
- 场景：同一水泵模板创建两台设备，以虚构网关输入展示 pressure；对 unbound / no_observation 返回 unknown / null，网关断线不伪造新测量。
- 验收：至少 BM01–BM08、NB01–NB04、跨范围 / 事务回滚和版本并发通过；历史模型切换和资产迁移有明确恢复结果。

### T06 — 测量与质量事件、去重、WAL（G4）

- 编排：认证与范围 → 整消息结构 / 模型 / 绑定校验 → 既有 key 与历史 profile 检查 → 单一并发接受 → observation_id 固化 → WAL 写入与真实同步 → durable ACK。
- 源状态事件独立去重域；没有新测量时不能写 history_observation 或改采样时间。点位 bad 必须有点位级证据，网关连接异常与传感器质量分开。
- WAL 执行顺序、文件创建 / 段切换及目录持久化、CRC / 损坏判别、磁盘水位、checkpoint / 重放与中途故障均需写出精确行为和实验；不能仅因调用 sync_all 就推断任意硬件断电后必然持久。
- 验收：同 key 并发、payload 改变冲突、replayed 不改变摘要、旧绑定成功消息重试、授权撤销、WAL 空间不足、同步失败、脏尾 / 中间损坏；断电、kill -9 分开报告。

### T07 — DuckDB 历史、latest 和查询（G4）

- 数据：typed observation、模型 / 绑定版本、质量、时间来源、稳定 observation_id；同毫秒不同观测可区分。
- 历史写入、applied_message 去重及死区 / 心跳基准在同一已验证事务域，checkpoint 仅推进连续已确认前缀。若 metadata 与 history 分库，不得宣称跨库事务。
- latest / quality / freshness 独立，乱序 / 补传不覆盖新值；历史事件时间水位只有真实来源与迟到策略能证明时才输出，WAL 水位不能替代。
- 查询：半开窗口、质量过滤、按 (source_at_ms, observation_id) 分页；游标签名和稳定性 / snapshot 能力明确，无法证明完整时返回 partial / unknown 或拒绝。
- 验收：T01–T10、同毫秒、跨版本单位变化、空窗 / 迟到 / 质量无值、大小写和时间跨度超限；F0/F2 只代表具体测试环境。

### T08 — 北向 HTTP、WebSocket 与可观测性（G5）

- 按正式 OpenAPI / Schema 实现只读资源目录、device view / shadow、history query、telemetry / source-state、健康及明确错误；模型管理 API 受 management_service 范围与审计约束。
- 实时快照在同一有序域捕获边界 S 的不可变状态；缓冲 seq>S，snapshot_end 后顺序发送；无法保证或溢出时 resync_required。慢客户端 coalesced 仅表示最终状态收敛，不代表事件无丢失。
- 限额、超时、查询成本和 401/403/404 隐匿策略；服务不内置用户角色体系 / AI / 设备控制。
- 验收：真实 TCP HTTP / WS 测试、并发更新、断线 / 重连、撤权、慢消费、恶意请求 / 跨 scope 缓存；不以单元方法调用冒充网络实测。

### T09 — 模拟网关端到端与运维（G5）

- 集成限定为非生产模拟器，固定模型 → 两台水泵 → 来源绑定 → 工程值上报 → 视图 / 历史 / 订阅；包含不做二次倍率和质量事件。
- 验收包含 CLI / 现场部署启动、生成完全虚构测试数据、读回每项值和状态、故障 / 重启恢复、资源清理；记录完整操作步骤与环境。安全连接和真实上层身份必须单独验证。
- 提供配置样例、日志 / 指标（写入状态、WAL 长度、最老消息、滞后与 gap）、备份 / 恢复演练；不得存真实凭据。

### T10 — 真实网关、负载与发布评估（G6）

- 阻塞输入：用户提供 / 授权的隔离环境、网关版本与真实但已脱敏的协议样例、绑定与 source_seq / epoch 能力、目标系统、指标与故障注入许可。
- 可先准备试验方案，不触碰生产设备；记录模拟、测试设备和生产现场各自证据。尚无目标指标时仅报告测量值，不判定达到生产要求。
- 测量：点数与分布、批次、采集 / 入库延迟 P50/P95/P99、CPU / RAM / 磁盘 / WAL、漏重、恢复时间、长时间运行、硬件缓存 / 断电；按目标机型和 OS 明确版本。
- 发布需完整的回滚、备份恢复、权限与脱敏、资源配额和签字，不因测试数量达标自动放行。

### T11 — 外部 TSDB 与算法能力（P1/P2）

- 选择**一个真实目标版本**，先区分 managed_ingest 与 referenced_read，读取能力声明 / 时间精度 / 质量 / observation_id / 分页及丢失语义；不能把虚拟 ID、missing quality 当作已实现能力。
- 出具同毫秒多观测、部分写入、ACK 不确定、source unavailable 和能力拒绝的测试。P0 不因这些特性增加未经验证的数据库依赖。
- 压差等小型确定性派生在基础模型质量稳定后评估；OEE、能耗业务口径和策略由 MES / EMS / 上层提供，结果带质量、窗口、版本及血缘；不建设自动控制。

## 6. 可复制给 AI 的单任务执行卡

```text
项目：fslit/iotfast-thing-engine，Rust；先读取 AGENTS.md 和 docs/development/ai-execution-plan.md。
任务：<Txx 与具体目标>；以最新 main 的实际 SHA 为基线，新建独立分支。
先检查工作区和已有 PR，列出需要读取的规范 / 源码；保留所有已有改动。
仅实现任务范围；先创建能验证行为 / 故障的测试，再实现或修订设计。
不得把模拟、旧报告或测试配置当成真实网关、安全和断电结果。
执行相应检查，分别列命令、环境、SHA、PASS / FAIL / BLOCKED / NOT_RUN。
交付：明确变更、审阅入口、证据和风险；未满足验收不关闭任务。
```

## 7. 必须由项目提供 / 确认的输入

现有网关脱敏报文和 source_state / epoch / 重传能力；可信上层身份及 Scope 断言方式；首个设备与地址 / 单位 / 倍率经现场确认的样例；目标部署 OS、CPU、内存、磁盘与预计点位 / 秒；保留期限、最大断网和可容忍丢失；外部 TSDB 首选和版本。等待输入时 T00–T03 可在明确范围内推进，但不能把缺失的现场事实自行标成通过。

## 8. 本次计划验证

本次通过 GitHub 读取指定 main 的仓库、主要规范、原型和证据摘要，定位到 prototype 路径迁移与报告 / 实现边界。当前执行容器未发现 rustc 与 cargo，因此未本地构建或复现原型；没有声称历史报告失实，也没有将历史结果视为本次 PASS。独立分支用于提交计划，是否合并依 PR 状态和项目流程确认。
