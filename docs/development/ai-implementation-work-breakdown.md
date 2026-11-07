# AI 开发规范：工程目录、生成顺序与任务卡

状态：设计提案，待执行；基线 main `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`（2026-11-06）。本文件是同分支 [AI 执行计划](ai-execution-plan.md) 的工程化细化，并非已存在的目录或功能。执行前核对最新 main、工作区、PR 和本文件状态。工程定位和契约以根目录 AGENTS.md、docs/specifications/README.md 指定的权威文件为准；发现冲突先记录并修订权威契约，不擅自改变既定语义。

## 1. AI 启动与执行规则

1. 读取根 AGENTS.md → 本文 → ai-execution-plan.md → data-engine-scope.md → specifications/README.md → 所领任务涉及的权威契约、原型、测试及证据。对比实际 main、分支与未合并 PR，记下 SHA 和工作区改动。
2. 每次仅领取一张任务卡，建立独立分支；先说明文件范围、依赖、测试设计和已知风险，形成可审阅的最小增量。多个 AI 仅在接口与文件所有权确定、依赖满足且互不覆盖时并行。
3. 严禁生成空方法、伪数据冒充实现；不能因缺身份、原始网关报文或目标设备而虚构结论。区分 PASS / FAIL / BLOCKED / NOT_RUN，列环境、命令、退出码、日志或脱敏证据路径。
4. 正式 Rust 工程和旧 prototype 独立；不得删除、移动或覆盖 prototype 与历史证据。原型的代码可在核对许可证和行为后选择性迁移，逐模块列出来源、差异和测试。
5. 用户身份、角色、权限策略、页面、告警工作流、MCP Agent、设备写控制均属上层；引擎必须独立验证服务身份和可信数据范围。首期使用网关提供的工程值，不得二次换算。
6. 除文中列举的执行命令外，任何工具、依赖安装、版本与环境均以实际锁定配置为准。自动生成文件需要声明生成命令和源头，禁止手改产物后声称一致。

## 2. P0 结构决策

正式 workspace 的具体目录尚未经 ADR 冻结。建议 ADR 采用下列 `engine/` 布局；F02 开始前记录路径、crate 划分、依赖方向、目标 OS/CPU、Rust 工具链、DuckDB 链接模式、变更及回滚。若 ADR 选其他路径，须一次性同步本文、构建命令、CI、README；不要同时创建两套结构。各条目仅按任务逐步创建，不提前批量生成空目录。

```text
iotfast-thing-engine/
  AGENTS.md                       已有；AI 入口
  docs/                           已有；权威规范和验收
  prototype/                      已有；保留原型和证据
  engine/                         提案：正式 Rust workspace
    Cargo.toml                    workspace 与共享依赖
    Cargo.lock                    应用依赖锁文件
    rust-toolchain.toml           明确已验证工具链
    README.md                     安装、启动、验证、限制
    crates/
      contracts/                  版本化 DTO、错误、规范化、Schema
      domain/                     设备模型、资产、绑定等纯领域规则
      application/                用例、事务协调与端口
      adapters/                   DuckDB、WAL、认证等具体基础设施
      transport/                  HTTP/WS 入站、出站和错误映射
    apps/
      thing-engine/               二进制入口、依赖装配、关闭流程
    schemas/                      版本化 JSON Schema/OpenAPI 与生成源
    config/                       无凭据的最小示例配置
    tests/
      fixtures/                   虚构报文、契约 golden vectors
      integration/                跨模块、真实网络及故障测试
    scripts/                      可重复的初始化和本地验证脚本
```

实现时 Rust 集成测试默认归各 crate 的 `tests/`；上表 `engine/tests/integration/` 只有在 F02 同时注册为 workspace 内测试包或在某个包显式配置 `[[test]]` 时才生效，禁止创建 Cargo 不运行的测试。fixtures 单一来源、脚本引用路径以仓库根为基准。CI 配置位于根 `.github/workflows/`，不重复创建于 workspace。

### crate 边界和依赖方向

| 目标包 | 可以依赖 | 禁止承担 | 典型内容 |
|---|---|---|---|
| contracts | 序列化、基础类型等轻量库 | SQL、HTTP、凭据验证或 I/O | telemetry/source_state DTO、时间/ID、错误码 |
| domain | contracts | 网络、DuckDB、进程全局状态 | 模型发布状态机、资产关系、绑定约束 |
| application | domain、contracts | 具体 SQL / HTTP 类型 | 入站原子编排、服务鉴权结果传递、端口 trait |
| adapters | application、domain、contracts | 绕过用例直接公开业务路由 | WAL/恢复、DuckDB、身份验证器、时间/存储 |
| transport | application、contracts | 从 body 构造已认证身份 | 请求解析、错误响应、HTTP / WS 协议 |
| thing-engine | 上述包 | 内嵌业务规则 | 配置、依赖组装、启动/停机 |

trait 按消费端定义；保持单向依赖，domain 不引入 adapters；跨模块 DTO 和错误经 contracts，内部实体不可直接成为公共 API。优先 5 个职责包 + 1 个入口，不因每种设备或数据库增加一个 crate；实际依赖与新增依赖必须写 ADR/PR 理由。不能以目录名代替事务边界、安全边界或真实测试。

### 必须具备的源码模块（每项由对应任务创建）

| 包/路径（相对 engine） | 文件职责 | 任务 |
|---|---|---|
| crates/contracts/src/{lib,telemetry,source_state,identity,error}.rs | 公共报文、版本、稳定错误；不提供可由客户端构造的可信身份 | F03/F06 |
| crates/domain/src/{lib,model,asset,binding}.rs | 状态转换与不变量；无数据库依赖 | F05 |
| crates/application/src/{lib,ports,ingest,catalog,query}.rs | 权限上下文、用例与事务接口 | F04–F08 |
| crates/adapters/src/{lib,auth,wal,duckdb}.rs | 验证身份、持久化与恢复实现；含故障转换 | F04/F06/F07 |
| crates/transport/src/{lib,http,ws}.rs | 协议输入输出、错误码、受限订阅 | F04/F08 |
| apps/thing-engine/src/{main,config}.rs | 装配、配置校验、健康和安全停机 | F02/F08 |

大模块可拆分为同名目录；不得改变外部职责。不要提前生成 `todo!()` 或返回成功的假实现。任务未实现的接口不对外暴露；尚未接入身份验证时只允许 loopback 开发健康端点，生产入口保持关闭。

## 3. 任务依赖与合并规则

先 F00 → F01 → F02，之后 F03 / F05 可在冻结接口和文件所有权后并行；F04 依赖 F02/F03，F06 依赖 F03/F04/F05，F07 依赖 F06，F08 依赖 F04/F05/F07，F09 依赖 F08，F10 依赖 F09；F11 在已明确的版本与能力边界上独立 PoC，不能阻塞或冒充 P0。每卡一个 PR；子任务须先合并到目标分支并重新检查 SHA 后再启动依赖任务。保持计划中的 G0–G6；本卡 F 编号是细化步骤，与历史 F0/F2 可行性试验不是同一编号体系。

### F00 — 事实和复现台账（T00/T02）

- 输入：prototype/、当前规范、历史 evidence、未合并 PR 及 ai-execution-plan.md T00/T02。
- 改动范围：新增本次证据台账；不重写历史实测文件，不动功能代码。
- 动作：确认 Git SHA / 操作系统 / CPU / 文件系统 / Rust / Cargo / DuckDB 实际引擎版本，检查 lockfile 和依赖。先读脚本副作用，在临时副本复现可运行测试；逐项调查 volatile 持久状态、身份过期、审计、cursor 和时间来源。
- 验收：每项附具体路径、命令、结果与环境；不能复现标 BLOCKED/NOT_RUN，给出最小下一步。原型已有测试结果仅作历史报告。完成后建立风险归属表，不需声称已修复。

### F01 — 决策与工程目录规范冻结（T01/T03）

- 输入：F00、已批准的 P0 契约与黄金向量。产出新增 ADR（编号以最新仓库顺延）、最终目录图、依赖/事务与 OS 构建矩阵、原型迁移策略和兼容策略。
- 必须明确：SQLite/关系元数据与 DuckDB/WAL 各自所有权，进程/文件锁、原子性范围、同一 DuckDB 多连接并发限制以版本验证为准；如不能确认先做小型 PoC，不得声称跨库原子提交。
- 核对实体 ID、规范化字节、数字与 null、重试和绑定版本，关联官方/仓库规范与矛盾表；契约冲突先解决，后续 F03 不得自行发明新规则。
- 验收：ADR 写替代方案、决策、依赖、风险、可重复验证、回滚及未解决事项；批准目录后再生成正式代码。各任务的 schema/database 命名以权威规范和迁移为准。

### F02 — 可构建的工程骨架（T03）

- 前置：F01 目录 ADR。创建 workspace、5 个职责包、1 个二进制、工具链与锁文件；包名唯一，`cargo metadata` 可识别。每包只放实际使用的最小导出，不写业务假成功。
- 新增 example 配置、结构化日志、进程启动/优雅关闭和 loopback 健康检查。健康分别定义 live/ready；未装配认证和存储时不得输出 ready 或开放业务接口。
- 根 CI 添加 format、clippy、test；写可重复命令及构建依赖；明确 DuckDB crate 与引擎真实版本，禁止把示例密码写入配置。配置无效/端口占用启动需失败、退出码非零。
- 在仓库根执行（工具链及 lockfile 存在后）：
  `cargo metadata --manifest-path engine/Cargo.toml --locked --no-deps`；
  `cargo fmt --manifest-path engine/Cargo.toml --all --check`；
  `cargo clippy --manifest-path engine/Cargo.toml --workspace --all-targets --locked -- -D warnings`；
  `cargo test --manifest-path engine/Cargo.toml --workspace --locked`。
- 验收：真实构建和启动、健康响应、SIGTERM 退出、非法配置拒绝；提供环境及命令结果。CI 文件存在不等于 CI 已通过。

### F03 — D00 契约与跨语言向量（T01）

- 创建 contracts 的测量与状态 DTO、受限值类型、版本化错误与 schemas；复用权威样例，输入字段/限制/缺失 null 逐条可追溯。
- 实现确定性 ID、规范化与 hash：使用仓库冻结规则和黄金向量，不凭本卡定义新算法；拒绝重复键、非法数字、越界时间和不支持的版本。样例、Schema 和实现一致。
- 运行 Rust 正负向量，与已有 Python 实现在同一组 fixtures 比较规范化字节及摘要；实际网关语言未取得时单独 BLOCKED。
- 验收：相同消息重试、点位顺序、整数边界、Unicode、负零、缺失/null、重复键与变更冲突覆盖；版本化结果可重复，不泄露数据到日志。

### F04 — 认证与 Scope（T04）

- 实现生产入口凭据验证器接口与一种经 F01 确认的真实方式；密钥从配置或受控外部设施获取，不提交真实凭据。构造可信上下文只能来自验证器，不能直接反序列化请求。
- 所有操作先校验身份有效性、操作与资源范围，过滤在聚合/分页之前；缓存和游标带 scope、资源/查询、有效期，撤销时长连接重新鉴权或关闭。身份提供方尚不明确时只做接口和隔离测试，生产验收 BLOCKED。
- 真实 HTTP 测试覆盖缺凭据、伪造、过期、错误 issuer/audience、跨租户、部分属性及资源不存在响应，证明不可探测不属于范围的资源；认证失败写脱敏审计。
- 验收：逐入口权限矩阵和证据齐全，不能靠单元 mock 冒充真实签名验证或 mTLS 集成。

### F05 — 模型、资产、来源和绑定（T05）

- 依据数据库设计与 F01 事务决策生成版本化迁移和 repository 端口；产出产品/模型草稿与不可变发布、设备/资产关系、资源稳定 ID、来源与版本化绑定。
- 先测试非法模型、重复 ID、循环资产、跨范围访问、版本并发和事务失败；实现发布校验、审计和回滚。绑定切换时保留历史配置及旧消息重试解释；不得对已有工程值再缩放。
- 验收：虚构泵模板创建两台资产，发布并绑定，迁址后资源身份正确；无值显示 unknown/null，模型/绑定历史可追溯。完成前不开启对外可写管理端点。

### F06 — 入站、事件质量、去重和 WAL（T06）

- 实现认证→整批校验→稳定 ID/hash→并发去重/冲突→WAL→同步→ACK 的明确状态机；首次接受和重试按冻结契约处理，消息校验失败不产生部分观测。
- 区分 measurement 与 source_state 幂等域；断连/超时只产生来源状态，不虚构测量值、采样时间或点位质量。拒绝伪造 durable 和 WAL 位置；volatile 模式不能被误报为 durable。
- 测试同 key 并发、同 key 不同 payload、绑定位变更后重试、同步失败、空间不足、坏尾/中间损坏、重启/重放；每项检查 ACK、数据库、日志与最终结果。
- 验收：持久确认仅在规定同步点之后；kill -9 和真实断电分开报告，文件系统及设备条件不足时断电 BLOCKED。

### F07 — DuckDB 历史与 latest 投影（T07）

- 实现 adapter、迁移、批写、applied-message 去重、checkpoint、历史检索及 latest 投影。以实际持久化事务边界论证 exactly-once 效果，不把跨库写描述成原子。
- 恢复时 checkpoint 只推进连续完成前缀；同毫秒不同 observation_id 可查询；迟到旧值不覆盖新 latest，质量和新鲜度与值独立。
- 测试重复重放、事务中断、同毫秒、跨模型版本、坏质量、空窗及分页；游标绑定身份范围和请求参数，签名用标准 HMAC/AEAD 库与密钥轮换策略，不用自制拼接摘要。
- 验收：实际 DuckDB 版本/平台、SQL 迁移、故障结果与限制有记录；无稳定快照保证时按契约拒绝或明确不完整，不伪造完整性。

### F08 — 北向 API 与实时订阅（T08）

- 生成并验证 OpenAPI/Schema；实现资源目录、设备业务视图、shadow、限定历史查询和 WebSocket，统一鉴权、错误码、超时、分页及查询上限。
- 快照在一致的有序域选定 S，订阅捕获 S 后的变化并处理缓冲上限；不能保证时发送 resync_required。慢消费者合并仅承诺最终状态，不承诺事件无损。
- 真 TCP HTTP/WS 测试空历史、非法参数、跨租户、订阅竞争、断线重连、过期凭证和慢客户端；输出与公共契约一致的无敏感信息错误。
- 验收：客户端可从快照无缝过渡到符合契约的订阅或得到明确重同步，绝不向未授权 Scope 泄露视图、计数或游标。

### F09 — 非生产闭环、运维与故障演练（T09）

- 编写可重复模拟器及虚构设备数据，完整运行两泵：建立模型/资产/绑定→工程值上报→读视图/历史/订阅→source_state 断线→恢复与去重。
- 提供 README、无密钥示例配置、部署步骤、健康、备份/恢复、容量及故障处理，明确 WAL/checkpoint/历史备份一致点及恢复限制。
- 验收：真实进程与网络联调、模拟重启/失败后重试及恢复验证；逐项列命令和证据。模拟器不代表真实网关或现场。

### F10 — 真实网关、负载与发布候选（T10）

- 前置：用户授权隔离测试环境与网关版本/协议脱敏样本、稳定 ID 与重传能力、目标机器、点数/频率、保留和验收指标。
- 执行实际设备范围内的来源映射、状态、重传、时钟、重启；对目标设备测吞吐、P50/P95/P99、CPU、内存、磁盘、恢复、长时间运行；所有指标附负载生成和版本。
- 验收：按事先约定阈值形成可复现报告、已知问题和备份回滚演练；缺设备/阈值/授权标 BLOCKED，不接生产。

### F11 — 外部 TSDB 与派生指标预研（T11）

- 针对一个实际数据库及版本验证认证、写入部分失败、时间精度、同时间戳、quality、分页与去重；形成逐项能力矩阵，不能支持的请求明确拒绝或返回降级信息。
- 区分引擎管理写入与引用外部历史，分别定义数据所有权和可追溯性；计算示例记录时间窗口、输入质量、单位、算法版本与血缘。OEE/能源业务口径由 MES/EMS 上层定义；不得自动下发控制。
- 验收：真实实例测量与模拟分别标注；没有外部实例不宣称兼容或性能达标。

## 4. 每个 PR 的共同完成定义

- 对照任务卡列输入 SHA、设计与实际文件、迁移/兼容、风险及回滚；小范围变更，无无关格式化。
- 功能路径覆盖正常、错误、跨范围和重启恢复（适用于该任务）；测试必须由 Cargo / CI 实际发现并执行，记录测试数、退出码。
- 凭据/生产数据不入库；未经授权不连接现场。真实平台不足列 BLOCKED，不能用 mock 替代真实验收。
- 明确设计完成、代码完成、自动化验证、现场验证四种状态；未完成项创建可追踪任务；验收不满足不得宣称已交付。
- 若用户要求只做规范则仅提交文档；要开发时，先从 F00 开始，不因结构图一次性输出整个仓库。

## 5. 可直接发给 AI 的启动指令

```text
项目 fslit/iotfast-thing-engine。按仓库 AGENTS.md 和
docs/development/ai-implementation-work-breakdown.md 开展工作。
从最新 main 拉取并记录 SHA，对比未合并 PR，先领取 F00；
检查 prototype 及历史证据，不改动/删除它们。
逐项运行当前环境能运行的复现，不能运行的标 BLOCKED。
提供工作分支、变更范围、可复现命令、结果、风险和下一任务。
F00 未通过前不要宣称可直接开发完成，未经 F01 决策不要创建正式工程。
按 F00→F01→F02 的先后推进，每个任务单独提交和验收。
```
