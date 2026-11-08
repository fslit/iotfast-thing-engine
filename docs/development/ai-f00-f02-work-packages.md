# AI 开发首批任务：F00–F02 可执行工作包

状态：计划，未执行。适用范围与任务依赖以 [工程生成规范](ai-implementation-work-breakdown.md) 和 [执行计划](ai-execution-plan.md) 为准。基线 main `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`；执行时重新确认最新 main 和本计划是否已合并。本文只描述首批工作，不表示文件、环境或测试已经存在。禁止把本文示例命令结果当成实际证据。

## 1. 工作流和任务状态

| 任务 | 输入门禁 | 产出 | 可并行性 |
|---|---|---|---|
| F00-A 清点 | 可访问当前 Git 和仓库 | 基线记录、文件清单、未合并 PR 差异 | 可与 F00-B 只读准备并行 |
| F00-B 环境和脚本审查 | 临时工作目录 | 环境表、副作用与可运行性记录 | 可与 F00-A 并行 |
| F00-C 原型复现 | A/B 已有可用基线、工具与依赖 | 独立运行结果、日志 | 允许环境不足标 BLOCKED |
| F00-D 风险核对 | A 完成，尽量有 C 结果 | 风险证据矩阵与后续任务 | 可与 C 并行做只读审查 |
| F01-A 结构 ADR | A/D 的事实和限制 | 目录、依赖和迁移决策 | 可与 F01-B 并行 |
| F01-B 存储/身份决策 | A/D 与契约 | 边界、状态与试验清单 | 缺现场输入不虚构最终选型 |
| F02-A workspace | F01 决定路径/边界且满足可构建前提 | Cargo workspace 可编译 | 顺序执行 |
| F02-B 运行配置/健康 | A 成功 | 安全启动、配置、关停、健康 | 依赖 A |
| F02-C 门禁与交接 | A/B 完成 | 测试、CI、运行说明 | 依赖 A/B |

完成定义：事实复核任务可以带未运行项交付，但必须列原因、替代证据和影响；如果缺失内容是下游任务的必要前提，则该下游项必须标 BLOCKED。分支或 CI 不通时不宣称合并或通过。风险不一定要求先修复全部原型后才可初始化骨架；涉及对外承诺和生产开放的阻塞问题必须解决后才能验收对应能力。

## 2. F00-A — 基线清点

**输入**：根目录 AGENTS.md、README.md、prototype/README.md、prototype/Cargo.toml、docs/specifications/README.md、docs/development/evidence/p0-core-engine-test-summary.md，以及当前 PR/提交状态。

**步骤**：

1. 确认仓库、目标分支、当前 HEAD、main SHA、工作区有无用户改动，记录已存在的 PR 与修改文件；不要清理、重置或覆盖任何改动。
2. 确认设计入口及其优先级，列出 F00–F02 相关契约及实际存在的 Cargo.lock、Cargo.toml、CI 配置。用文件清单而非猜测路径，记录历史脚本迁移到 prototype/ 后的相对路径。
3. 从证据报告提取待验证的事实和限制：测试数字属于历史记录；真实网络认证、控制面入口身份检查、审计、volatile 状态、cursor 等分别作为后续待核对项目。
4. 对未合并 PR：记录编号、base/head SHA、状态和冲突；仅比较，不擅自关闭或合并。当前计划本身若仅在 PR 中，需要将实际工作分支和文档来源写入报告。

**产出**：`docs/development/evidence/f00-baseline-<YYYYMMDD>.md`（若团队证据目录另有规则则遵从）；至少含版本/SHA、来源清单、状态和差异。时间使用实际执行时区和日期。

**验收**：报告引用可点击仓库路径、含当前 Git 状态；能区分当前事实与历史声称；没有未授权改动。

## 3. F00-B — 环境检查与脚本副作用

**步骤**：记录 OS/架构、磁盘/文件系统、Rust/Cargo/Python、网络与构建依赖的可用情况；DuckDB crate 版本、构建特性和通过 `SELECT version()` 取得的运行时版本分开记。先阅读 Cargo.lock、构建脚本、测试和 feasibility 脚本，确认下载、原生链接、输入输出、端口和是否覆盖 evidence。

**待执行示例**（记录完整输出和退出码；不存在的命令记 BLOCKED，不隐式安装）：

```bash
git rev-parse --show-toplevel
git rev-parse HEAD
git status --short
rustc --version
cargo --version
python3 --version
```

避免在真实历史证据目录内执行会写文件的脚本。可在临时副本设置独立 `CARGO_TARGET_DIR`，但先检查脚本、当前工作目录与环境变量是否仍指向原仓库。不要打印环境中的令牌、私钥或完整敏感配置。

**验收**：环境表每项标 PASS/FAIL/BLOCKED/NOT_RUN；工具可用不等于依赖可构建，不把 crate 版本当作引擎版本。

## 4. F00-C — 原型复现和向量

**前置**：确认各 manifest、lockfile、构建环境及命令可用。完整锁文件缺失时记录 BLOCKED，另行提出可控生成锁文件方案；不要绕过 `--locked` 后声称与历史依赖一致。

**执行**：在临时副本先运行 `cargo test --locked --manifest-path prototype/Cargo.toml`；再检查 F1 向量脚本实际工作目录、依赖与副作用后，从其要求目录运行 Rust/Python 验证。DuckDB 可执行后运行版本查询，记录链接模式和平台。测试失败保存首个错误、命令及退出码，不把重试后的通过覆盖最初失败。

**验收**：写原始结果链接/脱敏日志、测试数量、耗时与环境；区分可复现、未复现和不可比较。禁止修改源历史 evidence.json。没有真实网关、物理断电和真实身份系统时分别标 NOT_RUN 或 BLOCKED。

## 5. F00-D — 风险调查卡与可验证结论

| ID | 调查 | 起点 | 实测最小断言 | 后续 |
|---|---|---|---|---|
| R01 | volatile 状态是否误写 durable | prototype/src/ingest.rs、storage_duckdb.rs | 在隔离数据目录核对 ACK、持久行和重启；输出真实状态 | F06；误承诺须阻止生产 |
| R02 | 入口是否校验有效期和 Scope | prototype/src/scope.rs 及控制面实现 | 列各入口、过期凭证输入、实际结果；没有网络层说明仅为直接函数测试 | F04/F05 |
| R03 | 控制面审计覆盖与原子性 | 各控制面实现和 schema.rs | 每入口成功/失败/事务回滚下查 audit，说明事务边界 | F05 |
| R04 | cursor 完整性机制 | prototype/src/northbound.rs、北向契约 | 比较实际摘要与契约，测试篡改/跨 Scope；只输出脱敏结果 | F07/F08 |
| R05 | 时间来源及实体标识 | ingest、控制面和 D00 契约 | 输入非法枚举和边界标识，检查错误/副作用 | F03/F05/F06 |

**报告格式**：现象、复现路径、SHA、实际命令/结果、可影响范围、风险级别与理由、状态、负责后续任务；审查推测明确标「待验证」。无运行环境时允许只提交源码证据和未执行测试设计，不把推测写成验证结果。涉及错误持久承诺或跨范围泄露列为对应生产验收阻塞。

## 6. F01-A — 工程 ADR

**输入**：F00 证据、AGENTS.md、设计权威顺序、P0 设备业务边界、现有原型。确定编号前列出 docs/adr/ 现有文件，避免占用冲突编号。

**ADR 必备条目**：

1. 问题：原型独立保留，正式工程如何构建、打包与测试。
2. 备选：根 workspace、`engine/` 独立 workspace、直接演进 prototype；比较迁移成本、Cargo 命令、CI、发布、依赖冲突和回滚。
3. 决策：路径与 crate 列表、依赖方向、版本和 lockfile 策略、实际支持平台、测试发现机制、构建/交叉编译所需原生依赖。
4. 源码迁移表：原型模块 → 目标模块 → 规则/风险 → 所需测试；选择重写也须记录原因。样例：
   
   | 原型 | 候选目标 | 首要复核 |
   |---|---|---|
   | d00.rs | contracts | 向量与确定性 |
   | scope.rs | application/认证接口 | 不能让外部构造可信上下文 |
   | wal.rs | adapters/wal | 同步、损坏与恢复 |
   | storage_duckdb.rs | adapters/duckdb | 事务、幂等与锁 |
   | subscription.rs | application/transport | 快照边界与背压 |

5. 不包含尚未实现功能的虚假实现；各阶段变更与回滚；未确认项及依赖决策时点。

ADR 是仓库内可追踪决策，不要求臆造人工签字。若真实实现证据不足，标 proposed 并列最小验证；满足仓库工作规范且技术前提已定时可推进独立工程初始化，阻塞对应实际不确定项。

## 7. F01-B — 存储、认证和状态边界

产出决策表：元数据/配置、观测历史、WAL、latest、checkpoint 各自所有者与落盘位置；列读写者、事务与崩溃后恢复源。对每一项写唯一一致性承诺和不提供的能力。不得将独立数据库/文件的写入包装成一个数据库事务；跨域协调要说明顺序、幂等和恢复。

身份部分列入口（管理、南向、北向、WS、健康）、认证者、动作与 scope、过期/撤销及错误；真实发行方缺失时采用可替换验证端口，生产业务入口关闭。配置中引用凭证位置，不写任何真实私钥或令牌。

就绪状态至少覆盖：配置非法启动失败；WAL 恢复失败业务不可就绪；鉴权未装配业务不可就绪；优雅关闭停止接收新写并等待已有操作达到明确定义的边界。F02 仅需做骨架层可验证的子集，其他状态以未实现列出，不能返回成功。

## 8. F02-A — workspace 的可运行文件清单

只有 F01 对路径和构建前提作出决定后，AI 才生成以下文件（以 engine/ 方案为例，若 ADR 改路径应同步所有命令）：

| 文件 | 生成内容 | 验证 |
|---|---|---|
| engine/Cargo.toml | workspace members、resolver、共享元数据 | cargo metadata 列出全部实际包 |
| engine/Cargo.lock | Cargo 实际解析产生并提交 | `--locked` 重复执行不改文件 |
| engine/rust-toolchain.toml | 实际能构建的受支持版本及 components | rustc/cargo 版本证据 |
| engine/crates/*/Cargo.toml 和 src/lib.rs | 每个职责包真实包名及最小编译导出 | 依赖方向无环；无空业务成功接口 |
| engine/apps/thing-engine/{Cargo.toml,src/main.rs} | 组装与进程入口 | 构建、启动、退出 |
| engine/README.md | 系统前提、构建命令、现阶段限制 | 全新工作目录按文档复现 |
| 根 .github/workflows/ 的新或现有流程 | 与现存 CI 合并，设置实际路径 | 手动检查脚本并记录实际 CI 结果 |

实际包目录采用 contracts/domain/application/adapters/transport；如最小导出无需额外 crate，直接最小代码即可；新增依赖需说明功能、版本、许可证和原生构建条件。不要生成未使用的大量模块；不假设 target 环境已安装 DuckDB 运行库。

**检查顺序**：
1. `cargo metadata --manifest-path engine/Cargo.toml --locked --no-deps`；核实成员与 targets。
2. `cargo fmt --manifest-path engine/Cargo.toml --all --check`。
3. `cargo clippy --manifest-path engine/Cargo.toml --workspace --all-targets --locked -- -D warnings`。
4. `cargo test --manifest-path engine/Cargo.toml --workspace --locked`。
5. `git status --short`，核对生成物、锁文件和无误改。

网络、依赖或版本不可用时列 BLOCKED 和精确错误；不要把 CI 配置当作已执行的 CI。

## 9. F02-B — 配置、进程与健康

**配置 contract**：使用明确的解析/校验类型，声明每项名称、来源优先级、默认、范围及错误处理。监听地址、数据目录、日志级别必须可校验；不得在日志输出认证材料。示例只包含虚构值，密钥占位符不能被当成安全默认值。

**进程行为**：开发骨架仅允许 loopback 上的健康接口；业务路由未装配且默认不可用。非法配置、目录不可写或端口冲突应失败并给非零退出码。收到退出信号后停止服务并在约定期限内退出，超时行为明确，不将未实现持久化宣称为已刷新。

**健康语义**：live 表示进程可响应；ready 仅在所需组件初始化完成时为真。骨架未装配安全身份、业务数据和恢复时，对业务 ready 返回未就绪，并列已知原因，不暴露敏感路径或凭证。测试实际网络端口和非 loopback 配置被拒绝行为。

**验收测试**：有效最小配置启动；非法端口/未知必要配置/不可写目录；健康响应；业务端点不可用；占用端口；正常终止；记录具体平台/退出码。不可模拟的文件权限行为在相应系统上标 BLOCKED，不能用 root 的可写结果证明普通用户权限。

## 10. F02-C — CI、交付与交接卡

1. 将上节检查顺序接入实际 CI；固定运行环境和依赖策略，避免覆盖既有工作流；增加本地与 CI 相同的可复制命令。
2. 验证测试确实被 Cargo 发现且执行；如果集成测试置于 workspace 根部，必须注册 test target 或独立测试包，否则 Cargo 不会自动运行。
3. 输出 `F02` 交接：main SHA、PR/head SHA、实际结构、工具链和平台、命令/退出码、CI URL/结论、已知不支持项、F03 可用接口与边界。
4. 合并由项目流程决定。若仓库缺分支保护、CI 尚在运行或无法执行，本次注明限制，不能声称已合并或全部通过。

## 11. 任务报告模板

```text
任务 / 执行日期时区：
仓库 / main SHA / 工作分支 / head SHA：
输入规范和原型路径：
改动文件 / 不在范围的项目：
执行环境：
命令 | 工作目录 | 退出码 | PASS/FAIL/BLOCKED/NOT_RUN | 证据位置
风险 ID | 事实或待验证 | 影响 | 后续任务
验收结论：通过 / 部分完成 / 阻塞；理由：
下一步可独立推进：
```

敏感信息经脱敏后再提交。F00 可以不运行代码但必须如实标记；F02 的构建和真实进程检查未运行时不得标为完成。
