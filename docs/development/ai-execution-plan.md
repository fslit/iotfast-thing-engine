# AI 执行计划：独立设备数据引擎 D00–D10

> 文档任务包，不是新 wire 契约，也不是现场生产签收声明。截至 2026-11-11，**D00.1–D00.3（含 Q01–Q12、`SD-STORAGE-01`/`SD-STORAGE-02` 决议、JSON Schema 与 OpenAPI 3.1 机器契约 49 条路径 / 57 个操作 / 63 个组件 Schema / 66 项断言）、D01–D08 本地受控闭环（SQLite 14 张业务表 + DuckDB/TDengine 可插拔时序适配器 + `ST-02`/`ST-04`/凭据脱敏 + `D01.2` 单实例目录锁与 `stream_epoch` + `D10.1` 在线一致性快照备份与隔离沙箱恢复校验）、D09 离线网关回放 Fixture、D10 开发态 F01–F05 故障与容量水位矩阵、D11 只读 MCP 适配器、UX01–UX12 集成旅程及 P1 能力边界验证（共 30 项 Rust 测试通过）** 已完成；用户已确认外部 `taosd`（TDengine）实连验收（`ST-06`）、跨库回填（`ST-07`）与存量外部库直读（`P1-REF-READ`）归入二期范围（`SD-STORAGE-02`），真实工业网关现场联调（D09 现场门禁）与目标生产硬件物理掉电/容量签收（D10 现场门禁）仍待现场输入。

## 1. 权威来源、边界与当前事实

### 1.1 执行入口

按顺序阅读并在任务证据记录版本：

1. [AI 规范](../../AGENTS.md)、[数据引擎定位](../architecture/data-engine-scope.md)、[项目入口](../../README.md)、[规范入口与优先级](../specifications/README.md)、[ADR 0001](../adr/0001-mvp-boundaries.md)。
2. [设计规范](../architecture/design-specification.md)对应章节及全部 §18–28。
3. [路线图](roadmap.md)、[验收入口](acceptance.md)。
4. [D00 蓝图](../specifications/d00-integrated-implementation-blueprint.md)、[服务身份与公共契约](../specifications/service-identity-and-scope-contract.md)、[遥测契约](../specifications/p0-telemetry-contract.md)、[北向契约](../specifications/northbound-api-and-subscription-contract.md)。
5. [产品基线](../../PRODUCT.md)、[决策台账](specification-decisions.md)、本文 §2 的证据和实际代码入口；涉及表结构或设备语义时再读[表结构设计](../specifications/database-schema-design.md)、[设备业务模型](../specifications/device-business-model.md)。

职责以定位规范为准；同一 P0 行为以设计第二部分覆盖第一部分；蓝图、任务包和原型不能覆盖权威契约。第二部分内部或细化契约与正文存在冲突时，登记并请求设计负责人评审、同步权威文本后再实现受影响行为。本文编制者仅登记发现；Lead 的权威文档修订与决议状态见 §3。后续任务可在明确授权的文档 scope 内同步权威文本，但不得把文案收敛称为实现验收。

### 1.2 不可突破的边界

- 独立可复用的数据引擎：模型、设备、资源、绑定、观测、latest、历史和受信数据服务。最终用户/RBAC、业务告警、审批、UI、MCP Agent、设备控制与联锁属于上层，不在 D00–D10 偷渡实现。
- 只读现场闭环、复用网关；`gateway_normalized` 接工程值，绑定保留换算证据但禁止二次换算；`engine_raw` 不纳入本轮。
- 引擎必须验证服务身份、动作、时效与可信数据范围；内网、同进程、自报租户/角色或 AI 工具输入不是授权。
- 时间、质量、新鲜度、连接状态和模型/绑定 revision 必须可追溯；不把旧值、bad 或 stale 说成实时正常值。
- durable 只能在已声明并验证的持久边界后确认；WAL 接受与历史提交分开；不得用内存队列、原型 PASS 或 SIGKILL 推导断电无损。
- DuckDB 2.0.0 是默认目标，不从 crate 名称推定实际引擎版本；外部 TSDB 的能力单独验证，不偷偷切换元数据后端。
- 真实地址、凭据与生产数据不入库；现场资料受控保存，仓库只放批准的脱敏引用。

### 1.3 工作区与事实分层

编制时工作区已存在文档更正及原型目录重排的未提交改动，包括旧扁平模块删除、新分组模块和兼容导出。它们不是本文的修改；不回滚、不覆盖、不替他人提交。执行者每次开始都重新运行 `pwd`、`git status --short`、`git diff --stat`，记录 HEAD 与 dirty 状态，不仅记录提交 SHA。

当前实际工程在 [原型 manifest](../../prototype/Cargo.toml)，不是仓库根 Cargo 工程；[模块入口](../../prototype/src/lib.rs)保留旧公共路径兼容导出。源码分组如下，正式工程目录与复用策略仍待 D01 决定：

| 原型领域 | 实际入口 | 后续责任 |
|---|---|---|
| core | [config](../../prototype/src/core/config.rs)、[contract](../../prototype/src/core/contract.rs)、[scope](../../prototype/src/core/scope.rs)、[health](../../prototype/src/core/health.rs)、[error](../../prototype/src/core/error.rs) | D00/D01 公共契约与服务边界 |
| catalog | [model](../../prototype/src/catalog/model.rs)、[asset](../../prototype/src/catalog/asset.rs)、[binding](../../prototype/src/catalog/binding.rs)、[schema](../../prototype/src/catalog/schema.rs)、[sqlite_store](../../prototype/src/catalog/sqlite_store.rs) | D02–D04；SQLite 业务元数据持久化（`SD-STORAGE-01`） |
| ingest | [validate](../../prototype/src/ingest/validate.rs)、[wal](../../prototype/src/ingest/wal.rs)、[storage](../../prototype/src/ingest/storage.rs)、[ts_adapter](../../prototype/src/ingest/ts_adapter.rs) | D05–D07；WAL + 可插拔 `TimeSeriesStorageAdapter` |
| query | [northbound](../../prototype/src/query/northbound.rs)、[subscription](../../prototype/src/query/subscription.rs) | D08；水位线分页与实时订阅 Hub |
| server | [server/mod.rs](../../prototype/src/server/mod.rs)、[iotfast_server](../../prototype/src/bin/iotfast_server.rs) | HTTP/1.1 REST + RFC 6455 WebSocket 传输层 |

区分三类记录：**设计要求**（应当如此）、**现有证据**（特定代码/环境下的断言）、**待验证项**（需要新测试或现场输入）。

## 2. 已有测试入口与证据边界

### 2.1 可复用的现有入口（2026-11-11 全量 30 项测试基线）

下列命令从仓库根执行，供验证任务使用。单套件命令形式：`cargo test --locked --manifest-path prototype/Cargo.toml --test <target>`，`<target>` 精确取下表。当前根原型共 **15 个集成测试套件 + `dev_server` 单元测试（共 30 项测试全部通过）**：

| target / 现有文件 | 可核对的断言与对应报告 |
|---|---|
| `d01_health_and_d00_test`：[测试](../../prototype/tests/d01_health_and_d00_test.rs) | DuckDB `v2.0.0` 版本、`observation_id`、WAL 位置顺序、URI 段边界与 Scope 校验 |
| `d02_model_lifecycle_test`：[测试](../../prototype/tests/d02_model_lifecycle_test.rs) | 物模型草稿校验、发布冻结、重复发布拒绝与版本兼容性比对 |
| `d03_asset_and_uns_test`：[测试](../../prototype/tests/d03_asset_and_uns_test.rs) | 资产树、设备实例化、迁址 `resource_id` 稳定与重定向窗口保护 |
| `d04_binding_lifecycle_test`：[测试](../../prototype/tests/d04_binding_lifecycle_test.rs) | 禁止二次倍率、单位校验、`gateway_ack` 激活与旧绑定替代 |
| `d05_d06_ingest_and_wal_test`：[测试](../../prototype/tests/d05_d06_ingest_and_wal_test.rs) | 整批原子拒绝、未来时钟偏移、尾部半帧截断（F02）、中间帧损坏停机、硬水位背压（T07） |
| `d07_history_and_latest_test`：[测试](../../prototype/tests/d07_history_and_latest_test.rs) | 工程值原样、重放幂等与篡改冲突、乱序保护、死区/心跳过滤与 WAL 崩溃重放 |
| `d08_northbound_and_subscription_test`：[测试](../../prototype/tests/d08_northbound_and_subscription_test.rs) | R09 属性裁剪、半开区间与水位线游标分页、快照 `S` 与 `> S` 增量衔接、缓冲溢出 |
| `review_fixes_test`：[测试](../../prototype/tests/review_fixes_test.rs) | 7 项独立审查修复回归（WAL `magic/payload_len` 损坏停机、单调序号恢复、跨租户隔离等） |
| `mvp_thing_model_server_test`：[测试](../../prototype/tests/mvp_thing_model_server_test.rs) | 13 步 HTTP REST + RFC 6455 WebSocket 全链路闭环（`Q01–Q12`） |
| `storage_sqlite_duckdb_tdengine_test`：[测试](../../prototype/tests/storage_sqlite_duckdb_tdengine_test.rs) | 4 项测试：SQLite 14 表业务目录独立恢复 + DuckDB 默认落库 + TDengine/TimescaleDB/ClickHouse 适配器切换与查询下推 + `tdengine_config.password` 脱敏 + `ST-02` 能力握手 + `ST-04` 非连续投递空洞水位线 + `D01.2` 单实例目录锁与 `stream_epoch` + `D10.1` 在线快照备份与隔离沙箱恢复校验（[报告](evidence/storage-sqlite-duckdb-tdengine-report.md)） |
| `d09_gateway_replay_fixture_test`：[测试](../../prototype/tests/d09_gateway_replay_fixture_test.rs) | 离线网关断网缓存补传、乱序回放与绑定版本切换对账 Fixture（[报告](evidence/d09-offline-gateway-fixture-report.md)） |
| `d10_fault_and_capacity_test`：[测试](../../prototype/tests/d10_fault_and_capacity_test.rs) | 开发态 F01–F05 故障矩阵、`WAL-CORRUPTION` 账本对账与容量水位/延迟基线（[报告](evidence/d10-dev-fault-capacity-report.md)） |
| `d11_northbound_mcp_boundary_test`：[测试](../../prototype/tests/d11_northbound_mcp_boundary_test.rs) | 上层 AI/MCP 只读边界、R09 属性裁剪、越权 404 隐匿与控制指令拒绝（[报告](evidence/d11-mcp-readonly-adapter-report.md)） |
| `ux_journey_and_matrix_test`：[测试](../../prototype/tests/ux_journey_and_matrix_test.rs) | U01–U08 接入旅程与 UX01–UX12 异常状态矩阵端到端验证（[报告](evidence/ux01-ux12-integration-journey-report.md)） |
| `p1_capability_and_derived_metrics_test`：[测试](../../prototype/tests/p1_capability_and_derived_metrics_test.rs) | 可插拔 TSDB 能力边界、跨版本单位冲突防护与物模型派生指标（OEE / 单位能耗）（[报告](evidence/p1-capability-and-derived-metrics-report.md)） |

其他现有工程与可复现验证命令：

```bash
# 1. 根原型全量回归（30 passed）
cargo test --locked --manifest-path prototype/Cargo.toml
# 2. 独立契约黄金向量（13 Rust + 7 Python 向量）
cargo test --locked --manifest-path prototype/feasibility/f1_d00_contract/Cargo.toml
python3 prototype/feasibility/f1_d00_contract/verify_vectors.py
# 3. D00.3 机器契约（JSON Schema + OpenAPI 3.1，49 条路径 / 57 个操作 / 63 个组件 Schema / 58 项路由与 $ref 检查 / 66 项断言）
python3 scripts/verify-machine-contracts.py
# 4. D11 只读 MCP 适配器自检与 8 步 MVP 全链路演示
python3 scripts/iotfast_mcp_readonly_adapter.py --self-test
python3 scripts/mvp-thing-model-demo.py
```

运行前读测试/脚本，确认输出副作用；不得通过重生成黄金向量覆盖期望值来制造 PASS。

### 2.2 验证证据报告清单（`docs/development/evidence/`）

- [P0 核心数据引擎与全量回归测试总结](evidence/p0-core-engine-test-summary.md)：汇总 30 项 Rust 测试、黄金向量、机器契约与端到端脚本结果。
- [D00.3 机器契约验证报告](evidence/d00-3-machine-contract-verification.md)：4 份 JSON Schema 2020-12 与 OpenAPI 3.1（49 条路径、57 个操作、63 个组件 Schema、66 项正反断言）验证记录。
- [SD-STORAGE-01 / SD-STORAGE-02 存储分层与 TDengine 适配验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md)：SQLite `business_metadata.sqlite`（14 张 OLTP 表）+ DuckDB `engine.duckdb`（4 张核心时序表 + WAL/水位线状态表）+ 可插拔 `TimeSeriesStorageAdapter`（TDengine `STABLE`/Subtable、TimescaleDB、ClickHouse，含凭据脱敏、`ST-02` 能力握手、`ST-04` 空洞水位线对账、`D01.2` 单实例目录锁与 `D10.1` 在线一致性快照备份沙箱恢复）验证记录。
- [D09 离线网关回放 Fixture 验证报告](evidence/d09-offline-gateway-fixture-report.md)：断网缓存补传、乱序回放与绑定切换对账证据（真实工业网关联调仍待现场门禁）。
- [D10 开发态故障矩阵与容量基线报告](evidence/d10-dev-fault-capacity-report.md)：F01–F05 故障点、`WAL-CORRUPTION` 停机与背压水位基线（目标硬件断电与生产压测仍待现场环境）。
- [D11 上层只读 MCP 适配器验证报告](evidence/d11-mcp-readonly-adapter-report.md)：`scripts/iotfast_mcp_readonly_adapter.py` 与 `d11_northbound_mcp_boundary_test.rs` 只读与 Scope 边界证据。
- [U01–U08 接入旅程与 UX01–UX12 状态矩阵验证报告](evidence/ux01-ux12-integration-journey-report.md)：集成体验全流程与 12 类异常状态 API 事实证据。
- [P1 时序存储能力边界与派生指标验证报告](evidence/p1-capability-and-derived-metrics-report.md)：跨版本单位守卫与 OEE / 单位能耗派生计算验证记录。
- 历史可行性探针报告（保留背景）：[F1/D00 报告](evidence/f1-d00-contract-verification-report.md)、[F0/F2 报告](evidence/f0-f2-duckdb-transaction-report.md)、[F3/F6 报告](evidence/f3-f6-wal-and-subscription-report.md)。

## 3. 冲突与阻塞登记（不在本文自行修订协议）

下表 C01–C10 是本文的**局部审阅编号**，引用时写“任务包 Cxx”，不等于[决策台账](specification-decisions.md)中的同名编号。初读后 Lead 已在独立文档 scope 收敛部分权威文本：任务包 C01/C02/C09 的相应边界文案现为“设计已收敛、实现待验证”，不要求重复裁决；余项仍待冻结/复核。当前决议状态以决策台账 C/Q 项为入口。责任人是角色，实际姓名由负责人接单填写。闭合证据必须含决议、权威文档版本、兼容影响和测试映射；仅改任务状态不算解决。

| ID | 发现/冲突 | 责任与阻塞范围 | 关闭要求 |
|---|---|---|---|
| C01 | 初读蓝图的当前声明与历史候选混杂；Lead 已补历史状态提示（台账 C03） | 设计负责人；文案已收敛，D00 验证未完成 | 按专项契约核对字段，补机器契约和全向量；不再把历史 BLOCKED 当当前规则 |
| C02 | 初读连接超时与点位 bad 混淆；Lead 已收敛总体/T06/SC08 等文案（台账 C02） | 设计+网关负责人；设计已收敛，D05/D07/D09 待测 | 分别测纯网关断连和具体点位错误；无证据不自动降 bad，恢复连接不自动 good |
| C03 | 设计 §27 F05 是 DB 提交后 checkpoint 前；旧报告/原型将 F05 标作中间损坏，F04 又混含前后提交 | 测试负责人；D06/D10 可靠验收 | 按“注入时点”重新映射，不按同名 PASS 计算覆盖；中间损坏单列附加用例 |
| C04 | 公共契约 SC10/§3 要求 subscription_terminated；原型回归返回 ResyncRequired(CREDENTIAL_EXPIRED)，蓝图也存在泛化 resync 描述 | 接口负责人；D08 wire 冻结 | 评审过期/撤权/溢出的帧与终止语义，保留既有命名，不另造协议 |
| C05 | 设计 D03 写历史路径不复用；北向 §3.2 仅明确保留期内禁止复用 | 目录负责人；D03 到期重用 | 明确保留期后规则、授权重定向与迁移影响，补正反例 |
| C06 | [原型状态结构](../../prototype/src/ingest/validate.rs#L57-L68)缺公共契约状态事件的 source_time_origin/source_seq/delivery；源授权代码读取 valid_from 后需复核是否执行生效下界 | 入站+安全负责人；D04/D05 | 核对字段/摘要/排序及授权时间矩阵；不得把 Rust struct 当正式 wire Schema |
| C07 | 更正摘要仍列 volatile 标记、资源 ID 碰撞、控制面有效期/跨租户/审计、WAL 长度损坏、订阅写入衔接风险；历史“Minor 不影响正确性”无依据 | 各模块 owner；D01–D08 对应验收 | 先新增失败用例并复现，按风险逐项关闭，不沿用旧严重度结论 |
| C08 | 14 函数、历史 15、独立 12/6 与13/7、旧路径及“全部修复”口径混杂 | 证据负责人；所有完成声明 | 绑定源码/dirty 状态、函数、断言、环境、命令；保留历史不冒充本次重跑 |
| C09 | 初读旧任务包含控制/UI/MCP、外部库阶段含混；Lead 已按定位收敛边界和 P1 外部库（台账 C01/C05） | 项目负责人；设计边界已收敛 | D09 沿用现有网关，不开发 Modbus 模拟器/驱动；外部库单列后续验收，不声称全目标已达成 |
| C10 | 尚无完整可执行 Schema/OpenAPI 冻结、真实认证/HTTP/WS 服务验收、正式工程复用决议 | 设计+工程负责人；D00/D01/D08 | 将缺口纳入正式任务，不因原型把服务安全和传输永久列为 P0 范围外 |

### 3.1 局部发现与统一决策的映射

后续任务只在[决策台账](specification-decisions.md)维护决议状态；本表用于定位历史发现，不建立第二套状态机。同名 C 编号必须带文档名，不能跨文档直接匹配。

| 任务包发现 | 统一台账入口 | 下一步 |
|---|---|---|
| 任务包 C01 | 台账 C03 | 文案已收敛；继续 Q03 机器契约核对 |
| 任务包 C02 | 台账 C02、Q04 | 文案已收敛；补测量 / 状态事件正反测试 |
| 任务包 C03 | 台账 §5、Q07 | 按实际故障注入时点核验，不按旧编号推断覆盖 |
| 任务包 C04 | Q05 | 冻结 WS 终止与重同步行为 |
| 任务包 C05 | Q06 | 冻结历史路径保留期后的行为 |
| 任务包 C06 | Q02、Q04 | 核对来源授权有效区间和状态字段 / 摘要 |
| 任务包 C07 | Q02、Q07、Q08、Q11、Q12 | 分模块建立失败用例，分别关闭风险 |
| 任务包 C08 | 台账 C04、§5 | 绑定实际源码和断言，保留历史证据限制 |
| 任务包 C09 | 台账 C01、C05 | 边界已收敛，不重新扩大引擎范围 |
| 任务包 C10 | Q01、Q02、Q03 | 工程 / 认证决议及机器契约冻结 |

Q09 绑定恢复、Q10 容量与保留策略虽不对应一个独立局部发现，仍是必需决策；不得因本表没有一对一发现而漏掉。Q11 资源跨版本身份、Q12 接收 / 历史状态必须纳入 D00.2，不只处理旧 C 清单。

另外需 D00 字段级核对：未知字段/重复 JSON key、复杂值类型、整数编码、状态事件排序与 ACK、property 标识、管理动作名、游标失效策略。规范未唯一决定的部分记录待决，不能由实现者“合理猜测”。

## 4. DoR、DoD 与证据格式

### 4.1 Definition of Ready（开始每张卡前全部检查）

1. 阅读 §1 入口，记录权威章节/版本；明确本卡输入属于事实、合成假设还是待决。
2. 前置卡经审阅完成；允许并行的仅限 §7 约定；没有关闭的相关冲突则只做调查/测试设计，不做该行为实现。
3. 建立 shared task：subject、依赖、明确 write scope、owner；先 list/get，再按最新 revision claim。scope 重叠必须先协调。
4. 清点现有 dirty 改动；不覆盖他人修改。将拟改文件、不可改边界、测试副作用、回滚方式写入任务。
5. 权威规则、正反样例、可观察输出、验收负责人确定；现场未给数据则只用标注的合成 fixture，不虚构现场输入。
6. 工具链/依赖/测试数据可获得；涉及网络、生产、破坏性故障先取得对应授权。缺条件记 BLOCKED 并提出具体问题。

### 4.2 Definition of Done（不能只看 cargo test）

- 交付在批准 scope 内；设计与接口有可追溯评审，代码/配置/文档一致；没有引入用户体系、控制或 UI 责任。
- 每个验收行都有正例、反例、无副作用检查、实际执行命令、退出码、测试断言和原始输出位置。新增测试先证明可捕获目标缺陷，再验证修复；不能把待新增测试写成现有。
- 保存环境（OS/CPU/内存/磁盘/文件系统）、Rust/Cargo、crate/链接引擎、HEAD+dirty、负载/确认语义、输入来源、预期/实际/限制。未知项显式写未知。
- 通过相关回归与跨模块集成；只报告被证明的范围。durable/安全/性能结论必须有对应真实实现和目标环境证据。
- 追踪表按“未执行/部分证据/通过/失败/阻塞/不在本阶段”更新，并由负责人复核；本次表初始均未正式验收。
- 完成 diff 审查及敏感信息检查，列未完成项；get 最新 shared task revision 后 complete。阻塞卡不得 complete 伪装成功。

证据记录模板（未来任务产物）：任务/子任务与验收 ID；owner/reviewer；依赖与决议；输入和合成/现场分类；环境与版本；命令和退出码；断言/日志；正例和反例实际结果；副作用；已知限制；失败/阻塞与下一步。

## 5. D00–D10 可执行任务卡

以下每张卡的步骤必须按子编号顺序推进；输出位置由该任务审批确定，不能据本文自动创建新的工程目录或改已有文档。各卡“现有回归”引用 §2 精确 target；“新增”均表示 **待新增、当前不可运行且未通过**。

### D00 — 契约冻结与证据收敛

- **输入**：§1 全部入口、§2 证据、§3 冲突表；设计 §18–28、蓝图 I01–I03/R01–R10。
- **依赖**：无；现场未齐允许完成文档对账，不能宣称现场 profile 冻结。
- **范围/步骤**：D00.1 对照正文/细化契约/原型/证据逐字段建立差异；D00.2 组织 C01–C10 中影响协议的决议，列兼容迁移；D00.3 获授权后从既有契约产出可执行 Schema/OpenAPI、错误/Scope/遥测/状态/北向样例及黄金向量；D00.4 冻结版本与验收映射。不重新设计 ID/hash/wire 格式。
- **输出**：决议清单、字段来源矩阵、机器契约与正反 fixture 清单、冻结签认；本轮仅提供计划，不创建这些实现产物。
- **测试**：现有 F1 Rust 与 Python 命令见 §2。待新增机器契约验证：未知字段、重复 key、null、数值边界/Unicode/-0、多点排序、事件摘要/顺序、错误信封逐项；确定工具与路径后写实际命令，禁止空过滤器 PASS。
- **正验收**：每个对外字段/枚举/错误可追到唯一权威来源；既有 ID/hash 黄金结果逐字节一致；示例与 Schema 同版本。
- **反验收**：非法/歧义 fixture 被拒绝；未认证上下文不能成为可信 Scope；修改 ignored 传输字段不改变摘要，改业务值必冲突。
- **停止条件**：冲突未裁定、需要新 wire 设计却无授权、黄金结果不一致、现场假设被当事实；仅交差异报告，不放行 D01。

### D01 — 正式工程、配置、认证基础与健康

- **输入**：D00 冻结包、目标部署信息、现有 core 与 manifest、F0 报告。
- **依赖**：D00；正式路径/复用策略和认证方式评审后才可写代码。
- **范围/步骤**：D01.1 确定复用原型或新工程并固定 scope；D01.2 锁定构建、配置校验、单实例与启动恢复健康；D01.3 建立真实服务凭据验证到不可伪造 Scope 的边界与错误映射。无生产凭据、无用户/RBAC 服务。
- **输出**：可重复构建/启动证据、配置模板、实际 DuckDB 版本、认证中间层与端点接入责任清单；传输通道由 D01 建立，D02–D08 逐端点接入测试。
- **测试**：现有 `d01_health_and_d00_test`；待新增启动/配置/权限目录/单实例/恢复未就绪，以及真实 mTLS 或签名令牌的伪造、受众、过期、吊销、空 Scope 测试；正式构建命令随工程决议固定，不能把原型 library 构建称服务启动。
- **正验收**：目标平台空目录启动并重启读回，运行时 `SELECT version()` 有证据；合法服务生成经验证上下文。
- **反验收**：非法配置、不可写目录、错误凭据或普通自报头 fail-closed；恢复未完成不接收生产数据；健康不泄露密钥。
- **停止条件**：仅能在开发机运行、认证协议未定、拟绕过鉴权、依赖版本或目录漂移未解释。

### D02 — 产品与不可变模型

- **输入**：冻结模型/控制面契约、设计 §19–20、D01 身份与元数据事务入口。
- **依赖**：D01。
- **范围/步骤**：D02.1 草稿结构/类型/单位/重复属性校验；D02.2 同事务发布 body/hash/items 与审计；D02.3 版本比较、引用保护及受权 HTTP 控制面。共享 schema 由指定 owner 提交。
- **输出**：模型服务与接口、版本快照、审计记录、正反测试证据。
- **测试**：现有 `d02_model_lifecycle_test`；待新增非法模型、发布事务中断、过期/跨租户/动作不足、管理幂等及历史引用删除保护测试。
- **正验收**：合法草稿发布且重读内容/hash稳定；可选扩展与破坏性变更分类可解释。
- **反验收**：重复属性、错误单位/类型拒绝；已发布版本不能原位改写；未经授权发布无元数据与审计以外的数据副作用，不泄露他租户存在性。
- **停止条件**：字段类型或兼容定义未冻结、发布无法原子化、用模型修改替代实时值更新。

### D03 — 资产、设备、资源与 UNS

- **输入**：D02 已发布模型，目录/路径规则，C05/C07 决议。
- **依赖**：D02。
- **范围/步骤**：D03.1 资产树和同租户关系；D03.2 同一模型实例化两台设备，事务分配稳定不可复用 ID；D03.3 迁址/改名/旧路径映射和授权后解析；D03.4 管理幂等与目录分页隔离。
- **输出**：设备资源目录、路径生效映射、迁址审计；不把 UI 资产管理纳入引擎。
- **测试**：现有 `d03_asset_and_uns_test`；待新增 ID 碰撞、树环/跨租户挂接、迁址事务失败、过期 Scope、资产改名、历史路径到期与并发争用测试。
- **正验收**：两设备资源和值互不串扰；迁址前后稳定 ID 相同；仅授权者可解析旧路径到新路径。
- **反验收**：同级冲突/环/跨租户关系拒绝；未授权错误、计数、路径和模型字段均不泄露；历史引用不级联删除。
- **停止条件**：C05 未闭合时停止到期重用；ID 冲突或无法保证元数据原子创建；不擅改 ID wire 规则。

### D04 — 来源授权与绑定切换

- **输入**：D02 模型、D03 设备/资源、D01 真实身份、网关能力表（未知项显式标记）。
- **依赖**：D02、D03。
- **范围/步骤**：D04.1 注册来源与有效期授权；D04.2 映射/单位/转换责任校验；D04.3 可信 ACK 或显式人工停采窗口激活；D04.4 并发 revision、崩溃后对账、生效区间与审计。不开发协议驱动。
- **输出**：不可变绑定快照、激活操作记录、停采缺口/待同步状态、来源准入测试。
- **测试**：现有 `d04_binding_lifecycle_test`；待新增授权未生效/过期/撤销、伪造 ACK/hash/source、两个并发 active、网关先切换后服务崩溃、人工窗口证据测试。
- **正验收**：全设备至多一个 active；版本和 hash 对账一致后准入；失败保留旧有效版本或明确停采。
- **反验收**：未经授权来源、单位不匹配、服务二次倍率、不可证明版本拒绝；没有 ACK 不能伪称无缝切换。
- **停止条件**：网关版本确认未知且无批准停采方案；凭据/现场地址拟入库；激活跨进程原子性被假定。

### D05 — 测量与状态原子准入

- **输入**：D04 来源/绑定与 D00 字段/错误/摘要规则，设计 §21–22，C02/C06 决议。
- **依赖**：D04。
- **范围/步骤**：D05.1 字节/深度/点数与解析限制；D05.2 认证授权后按 key 查重，旧已接收请求按首次快照比较，新 key 才校验生效绑定；D05.3 全点校验与固定 ID；D05.4 独立状态事件命名域/摘要/排序。D06 完成前只能提供明确 volatile 行为。
- **输出**：标准化批次、独立状态事件、字段错误与无副作用证明，真实入站接口适配。
- **测试**：现有 `d05_d06_ingest_and_wal_test`、`d07_history_and_latest_test`；待新增解析级重复 key/未知字段、非有限数/u64边界、整批 WAL/DB/latest 零副作用、旧绑定成功请求重试、撤权重试、事件同 key 冲突与跨 epoch 测试。
- **正验收**：T01 工程值原样；T02 全批原子；值、时间、质量、版本齐全；连接状态与测量分别处理。
- **反验收**：null/未定义属性/未绑定点/未来时钟拒绝或按冻结策略隔离；网关断线无点位证据不把所有点 bad；恢复连接不凭空 good；状态不新增数值历史。
- **停止条件**：字段/事件 ACK 或顺序仍歧义；实现偷改摘要；只测校验函数却宣称 HTTP 全链路原子。

### D06 — 应用 WAL、确认、幂等与恢复

- **输入**：D05 已验证批次、设计 §23、外部发送清单、C03 故障映射、保留期/重试窗口策略。
- **依赖**：D05。
- **范围/步骤**：D06.1 持久帧/版本/边界和单 key 并发协调；D06.2 durable ACK 与 history pending 分离；D06.3 尾部恢复/中间损坏停机、checkpoint、安全回收；D06.4 故障注入及清单核对。与 D07 事务边界集成后才签可靠验收。
- **输出**：WAL/恢复/背压与诊断、每个 ACK 边界证据、故障报告；保留 volatile 明确限制。
- **测试**：现有 D05/D06、D07 与回归测试；独立 F3/F6 命令仅作探针。待新增正式进程 F01–F05、ACK 已知/未知、并发重复、sync 失败、磁盘满/只读、payload_len 损坏、中间 CRC/magic 损坏、分段元数据同步与重放幂等测试。
- **正验收**：每个 durable ACK 晚于自身 sync；恢复保持原 ingest/observation ID；同 key 并发只接受一次；历史未提交仍 pending。
- **反验收**：硬水位 T07/sync 失败不降级成功；中间损坏停止且不截断成“合法尾部”；volatile 不伪标 durable；不越过未提交消费者回收日志。
- **停止条件**：任一已确认样本丢失/重复、损坏误截断、保留期小于重试窗口、使用模拟文件事务替代联合 DB 证据。

### D07 — 历史事务、latest 与质量投影

- **输入**：D06 WAL 与恢复、冻结顺序/死区/心跳、不可变模型绑定快照、DuckDB 实测能力。
- **依赖**：D06 的 WAL 组件门禁（帧/同步/幂等/恢复接口及组件测试）通过即可启动 D07；不要求先完成依赖 D07 的联合可靠验收。D07 集成后再联合签认 D06/D07 可靠性，未签认不得开放正式 durable 承诺。
- **范围/步骤**：D07.1 历史/过滤基准/applied 标记同事务；D07.2 latest 按源时间/数值 seq/epoch 生效域投影；D07.3 状态质量与测量时间分离、重启恢复基准；D07.4 checkpoint、去重保留与版本历史解释。
- **输出**：可恢复的存储投影、历史事实与最新视图、联合故障对账。
- **测试**：现有 `d07_history_and_latest_test` 与 `review_fixes_test`；待新增 deadband=0/首条/心跳边界、过滤后重启、事务中断、跨 epoch、同毫秒多观测、状态乱序与连接恢复、模型变更不改历史、volatile 状态语义测试。
- **正验收**：T03/T04 去重与冲突，T05 历史保留合法旧观测而 latest 不倒退；历史过滤也持久标记已处理；质量时间独立。
- **反验收**：DB 事务失败不得留下半消息或已应用假标记；未知顺序/未来时间不覆盖 latest；旧历史不按新单位/倍率重解释。
- **停止条件**：事务跨文件却声称原子、恢复基准丢失、latest 被旧缓存覆盖、事件时间完整性从 WAL checkpoint 推断。

### D08 — 受权北向 API、快照订阅与历史

- **输入**：D07 投影、D01 真实传输认证、北向契约、C04 决议与限额策略。
- **依赖**：D07。
- **范围/步骤**：D08.1 目录/view/shadow 逐资源过滤；D08.2 受限历史 AST、范围/质量/稳定游标/能力声明；D08.3 把真实入站更新接到有序 Hub，严格 S 快照和有界 delta；D08.4 凭证时效/撤权、缓存/游标失效与限流。只输出数据，不开发上层页面或 MCP。
- **输出**：真实 HTTP/WS 端点、序列化契约、端到端客户端重建及安全报告。
- **测试**：现有 `d08_northbound_and_subscription_test` 与回归；待新增真实网络认证、实际入站并发 S 边界、空闲连接过期/撤权、资产迁址权限失效、缓存/游标篡改与过期、质量过滤、分页中迟到数据、限额、聚合不支持、缺口披露测试。
- **正验收**：无值为 null/unknown；view 各点时间/质量齐全；snapshot 每项≤S、delta>S，客户端收敛；半开区间和同毫秒分页稳定；未知事件时间覆盖为 null/unknown。
- **反验收**：T10/SC01–03 跨范围无数据/名称/计数泄露；溢出 T09 必终止并 resync；过期按评审的 SC10 终止；不能用手工 publish 测试替代写入衔接；不支持能力显式拒绝。
- **停止条件**：没有真实认证/网络端到端证据、S 快照混入新版本、游标无可验证防篡改、对 weak 历史声称 strong、C04 未定。

### D09 — 既有网关只读端到端联调

- **输入**：D08 服务、§6 所有必需现场输入、网关语言/固件/重试/ACK资料、批准的只读窗口。
- **依赖**：D08；资料未齐只能准备差距表，不能宣布现场通过。
- **范围/步骤**：D09.1 脱敏报文与实际网关语言契约对照；D09.2 配置/版本/工程值核对；D09.3 断网缓存、原 key 补传、状态事件/绑定切换联调；D09.4 上层展示读取引擎质量/时间/缺口，分别记录模拟、实验、真实现场来源。不开发 Modbus 模拟器/主站或控制功能。
- **输出**：现场签认的能力差距、只读链路与边界报告；需桥接时另立任务明确持久责任，不在这里临时虚构适配。
- **测试**：待新增真实网关 fixture 回放和现场脚本，命令须在网关型号、语言、认证和受控环境确认后填写；目前不存在通用可运行现场命令。重用冻结黄金向量，不替换期望值。
- **正验收**：指定网关→引擎→latest/历史→上层读取全链路有 trace/版本/时间质量对应；两设备数据隔离；断网补传与 ACK 行为可对账。
- **反验收**：无稳定 ID/持久重试只标 volatile/尽力交付；无法证明绑定时段拒绝；没有新测量不伪造采样；真实地址/凭据不提交。
- **停止条件**：现场许可、点表或网关协议缺失；发现设备写入风险；身份链路不可信；把合成回放结果充作现场验收。

### D10 — 故障、容量与运维验收

- **输入**：D09 报告、目标硬件/OS/磁盘、负载/保留/丢失窗口/RTO 目标、独立发送清单、批准故障环境。
- **依赖**：D09；目标指标未签认可记录测量值，不能判定生产达标。
- **范围/步骤**：D10.1 目标构建/配置/备份恢复/权限；D10.2 正式进程故障矩阵和目标硬件物理掉电（独立授权）；D10.3 稳态/峰值/补传/查询并发/磁盘水位压测；D10.4 运维 runbook、恢复演练与限制签认。
- **输出**：可复现命令和配置、P50/P95/P99、吞吐/资源/WAL空间、丢失/重复/恢复时间、未通过项、部署与回滚说明。
- **测试**：待新增目标环境故障/负载/恢复脚本与统计校验器；当前不提供捏造命令或阈值。以 §6 签认输入固定实际命令并归档；复用 F01–F05 时必须是正式联合引擎而非仅独立探针。
- **正验收**：目标指标逐项实际满足、恢复账本可重现；备份还原保留模型/绑定/WAL可解释性；配置与操作可由另一执行者复现。
- **反验收**：磁盘满/损坏不假 ACK、不跨损坏重放；故障时健康明确拒绝；未测试物理掉电必须写未测；禁止宣称标准兼容或外部 TSDB 能力一致。
- **停止条件**：任一目标缺少输入/测量、破坏性测试未授权、已确认数据丢失、无法复现或脱敏失败；保留失败证据，不降低阈值掩盖失败。

## 6. 验收追踪与现场输入门禁

### 6.1 规范向量追踪（初始状态全部“待正式验收”）

“部分证据”仅表示 §2 有相关断言/历史报告，未核验完整场景；每行在未来完成时补执行命令、断言位置、环境和 reviewer。缺一不能升级“通过”。

| 验收 ID / 原规范 | owner 卡 | 现有证据及 2026-11-10 阶段一落地状态（D09/D10 现场门禁仍保留） |
|---|---|---|
| T01 工程值 | D04/D05/D07/D09 | **本地已验证**：`d07_history_and_latest_test`、`mvp_thing_model_server_test` 验证 `normalization_owner="gateway"` 原样存储与拒绝二次换算；真实网关对账待 D09 |
| T02 整消息拒绝 | D05 | **本地已验证**：`d05_d06_ingest_and_wal_test` 与 `orca-mvp-smoke.py` 验证单点非法时整批 `422` 且 WAL/DB/latest 零副作用 |
| T03 同 key 同内容 | D06/D07 | **本地已验证**：`d07_history_and_latest_test` 验证 `replayed: true` 幂等返回 `200 duplicate` 及绑定切换后重试（R08） |
| T04 同 key 改内容 | D05–D07 | **本地已验证**：`d07_history_and_latest_test` 验证同 key 篡改内容返回 `409 IDEMPOTENCY_CONFLICT` |
| T05 乱序 | D07 | **本地已验证**：`d07_history_and_latest_test` 验证旧时间戳写 `history_observation` 但不覆盖 `latest_value` |
| T06 超时无新值 | D05/D07/D09 | **本地已验证**：`d05_d06_ingest_and_wal_test`、`review_fixes_test::test_i6` 验证网关断连仅置 `freshness="stale"`，点位故障才置 `quality="bad"`（Q04/C02） |
| T07 硬水位 | D06/D10 | **本地已验证**：`d05_d06_ingest_and_wal_test` 与 `d10_fault_and_capacity_test` 验证超过 `wal_hard_watermark_bytes` 返回 `503 DURABILITY_BACKPRESSURE`（[D10 报告](evidence/d10-dev-fault-capacity-report.md)） |
| T08 旧绑定时段 | D04/D05 | **本地已验证**：`d04_binding_lifecycle_test`、`mvp_thing_model_server_test` 与 `d09_gateway_replay_fixture_test` 验证 `gateway_ack` 与 `manual_stop_window` 双模式激活及断网切换对账（Q09，[D09 报告](evidence/d09-offline-gateway-fixture-report.md)） |
| T09 溢出 | D08 | **本地已验证**：`d08_northbound_and_subscription_test` 与 `ux_journey_and_matrix_test` 验证增量缓冲溢出下发 `resync_required(BUFFER_OVERFLOW)`（Q05） |
| T10 范围隔离 | D01/D04/D08 | **本地已验证**：`mvp_thing_model_server_test`、`d08_northbound_and_subscription_test`、`d11_northbound_mcp_boundary_test`、`review_fixes_test::test_i5` 验证 HMAC 令牌、`[valid_from_ms, valid_to_ms)` 区间、伪造头拒绝与属性级裁剪（Q02，[D11 报告](evidence/d11-mcp-readonly-adapter-report.md)） |
| F01 WAL写前 | D06/D10 | **开发态已验证**：`d10_fault_and_capacity_test` 验证 WAL 写前中断零残留并与客户端发送账本对账（[D10 报告](evidence/d10-dev-fault-capacity-report.md)）；目标硬件签收待 D10 现场环境 |
| F02 写后sync前 | D06/D10 | **本地已验证**：`d05_d06_ingest_and_wal_test` 与 `d10_fault_and_capacity_test` 验证尾部半帧安全截断（`CleanWithTruncatedTail`） |
| F03 sync后ACK前 | D06/D07/D10 | **本地已验证**：`d07_history_and_latest_test` 与 `d10_fault_and_capacity_test` 验证 `simulate_wal_fsync_without_db_commit` 后 `recover_from_wal` 幂等重放（物理断电待现场硬件） |
| F04 DB事务中 | D06/D07/D10 | **本地已验证**：`dev_server::tests::failed_commit_rolls_back_and_stops_new_admission`、`d10_fault_and_capacity_test` 与 `f0_f2_duckdb_tx` 验证事务回滚无脏读 |
| F05 DB提交后checkpoint前 | D06/D07/D10 | **本地已验证**：`d07_history_and_latest_test` 与 `d10_fault_and_capacity_test` 验证 WAL 恢复重放已提交批次时的 `applied_message` 幂等判重 |
| §23 checkpoint后日志未清理；尾部/中间损坏 | D06/D10 | **本地已验证**：`review_fixes_test::test_c1_i1`、`d10_fault_and_capacity_test` 与 `ingest/wal.rs` 验证 `magic` / `version` / `payload_len` / CRC32 损坏转 `HaltedOnWalCorruption`（Q07） |
| SC01–SC03 / SC10 | D01/D04/D08 | **本地已验证**：`mvp_thing_model_server_test` 与 `ux_journey_and_matrix_test` 在真实 HTTP/WS 帧层验证 HMAC 签名、自报头 `422`、越权 `404` 隐匿及凭据过期 `subscription_terminated`（Q02/Q05） |
| SC04–SC09 | D00/D05/D07 | **本地已验证**：`f1_d00_contract`（13 项黄金向量）、`scripts/verify-machine-contracts.py`（66 项断言，[D00.3 报告](evidence/d00-3-machine-contract-verification.md)）与 `d05_d06`/`d07` 全覆盖 |
| NB01–NB02 | D02/D03 | **本地已验证**：`mvp_thing_model_server_test` 验证锁期内重定向、过期后 `retired (404)`、永久禁止他设备抢占（`409`）与原设备迁回激活（Q06） |
| NB03–NB04 | D07/D08 | **本地已验证**：`mvp_thing_model_server_test` 与 `ux_journey_and_matrix_test` 验证 `bound / bound_static / unbound` 属性展开及时间/质量/新鲜度分离（[UX 报告](evidence/ux01-ux12-integration-journey-report.md)） |
| NB05–NB06 | D08 | **本地已验证**：`mvp_thing_model_server_test` 与 `d08` 验证 `snapshot_begin(S) -> chunk -> end(S) -> delta(>S)` |
| NB07–NB10 | D08 | **本地已验证**：`mvp_thing_model_server_test`、`d08` 与 `p1_capability_and_derived_metrics_test` 验证 `[start_ms, end_ms)`、`watermark_received_at_ms` 并发迟到稳定分页（Q08）、跨版本单位冲突 `422` 与 HMAC 游标绑定 |
| 定位最低闭环 | D02–D09 | **本地已验证**：`scripts/mvp-thing-model-demo.py`、`mvp_thing_model_server_test` 与 `d09_gateway_replay_fixture_test` 完整跑通物模型演进、资产树+跨树拓扑、设备平滑升级、动态+静态引用、耐久入站、WS 推送、历史查询与离线网关回放；真实网关待 D09 |
| 存储分层、备份恢复与外部 TSDB 适配（`SD-STORAGE-01` / `SD-STORAGE-02` / `ST-02` / `ST-04` / `D01.2` / `D10.1` / P1） | 独立验证 | **本地适配器与能力边界已验证**：`storage_sqlite_duckdb_tdengine_test`（4 项测试，[存储报告](evidence/storage-sqlite-duckdb-tdengine-report.md)）与 `p1_capability_and_derived_metrics_test`（3 项测试，[P1 报告](evidence/p1-capability-and-derived-metrics-report.md)）验证 SQLite 14 表业务存储 + DuckDB/TDengine/TimescaleDB/ClickHouse 适配器 + `tdengine_config.password` 脱敏 + `ST-02` 能力握手 + `ST-04` 空洞水位线对账 + `D01.2` 单实例目录锁 + `D10.1` 在线一致性快照备份与沙箱恢复；用户已确认外部 `taosd` 生产集群实连（`ST-06`）、跨库回填（`ST-07`）与存量直读（`P1-REF-READ`）归入二期（`SD-STORAGE-02`） |

### 6.2 现场输入登记表

每项必须填写提供者、确认者、受控证据引用、版本/日期、状态（缺失/合成/已核实）与影响；目前均 **待项目方确认**，不得把示例阈值填成事实。

| 输入 | 提供/确认角色 | 门禁与缺失时允许工作 |
|---|---|---|
| 网关型号/固件/语言、原始报文、质量/采样时间/稳定ID/epoch/seq、缓存重传、绑定ACK | 网关负责人+集成负责人 | 阻塞D09；允许D00对账、合成fixture；不能承诺可靠接入 |
| 首设备点表、地址基准、类型/字节序/倍率/单位/周期、转换责任 | 现场设备负责人 | 阻塞真实采集；只做离线格式验证，不自动测试连接 |
| mTLS或令牌、issuer/audience/算法、信任根、scope断言/撤销/租户映射 | 上层身份负责人+安全负责人 | 阻塞D01真实认证与上线；可做隔离测试方案，不用自报头替代 |
| OS/CPU/内存/磁盘/文件系统/链接方式/离线部署 | 部署负责人 | 阻塞目标F0/D10；开发机结果仅限开发机 |
| 设备/点数、频率/峰值、每批点数、查询并发、保留、最长离线、限额 | 项目负责人+运维 | 阻塞容量达标判断；仅报告测量值，不自定SLA |
| durable需求、可丢失窗口、RTO、重试/去重/快照保留期、磁盘水位 | 项目负责人+可靠性负责人 | 阻塞可靠模式签认；禁止静默降级durable请求 |
| 只读许可、联调窗口、故障/掉电许可、隔离与恢复负责人 | 现场负责人+安全负责人 | 阻塞D09/D10破坏性步骤；合成测试不能替签认 |
| 首选外部库/版本、互通标准与目标对象 | 产品/集成负责人 | 后续范围门禁；不扩大当前P0，不宣称全存储通用 |

## 7. 并行与首个可执行任务

### 7.1 依赖与并行策略

主链保持路线定义：`D00 → D01 → D02 → D03 → D04 → D05 → D06 → D07 → D08 → D09 → D10`；D04 同时依赖 D02/D03。允许提前做测试设计、资料收集或只读审查，不等于提前签验收或越过依赖实现。

- D00 可并行只读核对“身份/契约”“存储/故障”“北向/证据”，但由单一契约 owner 汇总；公共字段、错误、Schema 冻结前禁止各自独立 wire 草案。
- D01 后可并行准备 D02/D03/D04 的 fixture 与评审；实际实现仍遵循依赖。D06 WAL 与 D07 事务测试设计可并行，最终联合验证串行。
- 未来任务按实际目录分 scope；公共 contract、schema、manifest/lock、模块入口和共享 fixture 只能有单一 owner，其他任务提交需求而非抢写。
- 每个执行者先 list/get/claim，完成前 get/complete；write scope 重叠先协调。未收到前置产物不凭猜测继续。读到外部变更先重读/rebase，不覆盖。
- 独立探针若写相同证据文件、数据库或 target 缓存，使用批准的隔离输出目录或串行；Lead 最终审查合并 diff 并运行统一回归，不能把多个代理的单独 PASS 拼成集成 PASS。
- 本轮只交付规范，不实现功能。后续按各卡获准的 write scope 工作：D00.2 可以包含决议与权威文档同步，D00.3 可以包含机器契约和验证产物；代码与现场连接分别另立明确授权，不沿用文档任务权限。

### 7.2 首个后续可执行任务：D00.2（文档决议，不写功能代码）

D00.1 的对账与向量映射已经形成本任务包，仍待负责人评审；不是 D00 已冻结。下一执行者不要重复创建本次审计，先审阅对账并执行以下任务：

1. 创建并 claim D00.2 shared task，明确允许修改的决策台账、相关权威文档和样例 scope；禁止代码、依赖、生产连接。记录工作区基线、入口版本与现有改动。
2. 以[决策台账](specification-decisions.md)Q01–Q12 为待决入口，与任务包局部 C 项交叉核对。已收敛的职责、点位证据和 P1 边界直接作为基线，不反复立案。
3. 按字段来源与兼容影响收敛 Q03–Q06/Q08 等技术歧义；工程/认证选择先输出方案与所需批准，现场输入保持缺失状态。不能凭原型行为定协议。
4. 获得设计负责人的明确决议后，在该任务授权 scope 内同步权威正文、错误/样例/版本说明并记录迁移影响；未获决议保留具体阻塞，不私自放宽。
5. 验证决议、样例和本文追踪表一致；输出 JSON 语法检查与文档链接检查命令/结果及未决项。这些检查不是功能测试。审阅后按最新 revision complete 已完成的文档任务。
6. D00.2 相关决议关闭后，另 claim D00.3：按现有 wire 规则产出可执行 Schema/OpenAPI、正反 fixture 和黄金向量验证，审批具体工具与路径，然后才运行对应验证。所有机器契约和新增测试当前仍待创建。

**停止条件**：需要现场事实却未提供、需改变 wire 但无兼容评审、公共定义仍有多解、scope 不够或他人同时改相同文件。只阻塞受影响项并上报；不以文档 PASS 宣称 D00 完整冻结，更不进入未经授权的 D01 实现。
