# 研发路线与任务

权威任务定义及详细验收见 [设计规范第 27 节](../architecture/design-specification.md#27-功能任务拆解与验收用例)。本路线是工作顺序：截至 2026-11-11，**D00–D08 已完成一期单机嵌入式 SQLite 业务存储（`business_metadata.sqlite`，14 张 OLTP 目录表）+ 嵌入式 DuckDB `v2.0.0`（`engine.duckdb`，4 张设备时序表）+ 可插拔 `TimeSeriesStorageAdapter`（预置 `tdengine` / `timescaledb` / `clickhouse` 适配接口、`ST-02` 能力握手、`ST-04` 非连续投递空洞水位线与凭据脱敏；用户已确认 `taosd` 外部实连归入二期 `SD-STORAGE-02`）+ 预写式 `AppWal` + 单实例目录锁与在线一致性备份恢复（`D01.2`/`D10.1`）+ `iotfast_server`（HTTP REST / RFC 6455 WebSocket）受控闭环实现与 30 项自动化回归验证**（见 [MVP 运行与验收手册](mvp-thing-model-runbook.md) 与 [存储分层验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md)）；**D09 离线网关回放 Fixture、D10 开发态 F01–F05 故障与容量水位矩阵、D11 上层只读 MCP 适配参考实现及 UX01–UX12 集成旅程均已完成本地验证**，但 **D09（真实工业网关联调）与 D10（目标生产硬件故障/容量签收）仍待现场输入门禁**，不将本地受控闭环宣称为现场生产验收完成。

| 任务 | 依赖 | 交付与核心验收 | 当前状态（2026-11-11） |
|---|---|---|---|
| D00 契约冻结（含 `D00.3` 机器契约） | 无 | 梳理术语、字段、JSON 样例、错误及冲突；完成 `Q01–Q12` 决议、跨语言黄金向量与 OpenAPI 3.1 / JSON Schema 机器契约（`scripts/verify-machine-contracts.py`，49 条 HTTP/WS 路径、57 个操作、63 个组件 Schema、66 项正反断言）。 | ✅ 本地契约、黄金向量与机器契约已验证（13 Rust + 7 Python 向量 + 66 项 Schema/OpenAPI 断言 100% 通过） |
| D01 工程、构建与存储分层（`SD-STORAGE-01` / `SD-STORAGE-02` / `D01.2`） | D00 | Rust 工程、配置强制校验（`Q10`）、`engine.lock` 单实例独占锁（`409 VERSION_CONFLICT`）与 `stream_epoch`（`D01.2`）、健康检查；双存储架构：SQLite（`business_metadata.sqlite`，14 张 OLTP 表）+ DuckDB `v2.0.0`（`engine.duckdb`，默认 TSDB）+ 可插拔 `TimeSeriesStorageAdapter`（`tdengine` / `timescaledb` / `clickhouse`，含 `ST-02` 能力握手、`ST-04` 水位线空洞对账与凭据脱敏）。 | ✅ 本地受控闭环已验证（`d01_health_and_d00_test` + `storage_sqlite_duckdb_tdengine_test` 4 项测试） |
| D02 产品和物模型 | D01 | 产品目录、模型草稿、验证、发布不可变版本、版本兼容性比对（`:diff` / `:compare`）。 | ✅ 本地受控闭环已验证（`d02_model_lifecycle_test` + HTTP 路由） |
| D03 资产与 UNS | D02 | 资产树、跨树拓扑关系（`asset_relation`）、设备实例化、平滑模型升级（`Q11`）、历史路径永久防抢占（`Q06`）。 | ✅ 本地受控闭环已验证（`d03_asset_and_uns_test` + `mvp_thing_model_server_test`） |
| D04 来源和绑定 | D02、D03 | HMAC Scope 令牌鉴权（`Q02`）、`[valid_from_ms, valid_to_ms)` 来源授权、动态采集 + 静态配置属性引用、双模式激活（`Q09`）。 | ✅ 本地受控闭环已验证（`d04_binding_lifecycle_test` + `mvp_thing_model_server_test`） |
| D05 数据入站 | D04 | 工程值原样入站、质量、整消息原子校验、绑定版本核对与独立状态事件（`Q03`/`Q04`）。 | ✅ 本地受控闭环已验证（`d05_d06_ingest_and_wal_test`） |
| D06 持久与幂等 | D05 | 应用层 WAL 帧校验（`Q07`）、持久确认、去重、尾部截断（F02）与中间帧损坏停机（`WAL-CORRUPTION`）。 | ✅ 本地受控闭环已验证（物理掉电待 D10 现场硬件验证） |
| D07 状态与历史 | D06 | DuckDB 单事务历史落库、死区/心跳过滤、乱序保护与双维入站状态查询（`Q12` `GET /v1/ingestions/{id}`）。 | ✅ 本地受控闭环已验证（`d07_history_and_latest_test`） |
| D08 北向查询与集成体验 | D07 | 物模型完整展开视图（`bound / bound_static / unbound`）、设备影子、WS 快照衔接订阅（`Q05`）、水位线稳定分页历史查询（`Q08`）、U01–U08 接入旅程与 UX01–UX12 状态矩阵。 | ✅ 本地受控闭环已验证（`d08_northbound_and_subscription_test` + `mvp_thing_model_server_test` + `ux_journey_and_matrix_test`） |
| D09 网关联调 | D08 | 离线断网缓存补传、乱序回放与绑定切换对账 Fixture（`d09_gateway_replay_fixture_test.rs`）；真实工业网关对接及端到端闭环待现场输入。 | ⏳ 离线回放 Fixture 已验证；真实网关联调待现场输入 |
| D10 验收和运维 | D09 | 在线一致性快照备份与隔离沙箱恢复校验（`D10.1` `POST/GET /v1/storage/backups`、`POST /v1/storage/backups/{id}:verify`）、开发态 F01–F05 故障矩阵与容量水位/延迟基线（`d10_fault_and_capacity_test.rs`）；目标环境物理断电、性能签收与运维演练待现场环境。 | ⏳ 在线备份恢复（`D10.1`）与开发态 F01–F05/容量水位已验证；生产硬件签收待现场环境 |
| D11 上层 MCP 集成 | D09 | 上层只读 MCP 适配器参考实现（`scripts/iotfast_mcp_readonly_adapter.py --self-test`）与北向只读/Scope 边界验证（`d11_northbound_mcp_boundary_test.rs`）；引擎不内置 Agent 或控制工具。 | ✅ 只读 MCP 适配器与安全边界已验证（上层生产集成按需接入） |
| D12 设备控制 | 上层另行规划 | 控制、审批和联锁不属于本引擎。 | 🚫 不属本引擎范围 |

## 存储适配器与 P1 扩展研发轨道（对齐 `storage-adapter-contract.md` / `derived-metrics-oee-energy.md` / `external-tsdb-reference-mapping.md`）

| 轨道编号 | 权威规范依据 | 核心交付与契约入口 | 当前状态（2026-11-11） |
|---|---|---|---|
| `ST-01` 确定性 ID 与摘要 | [D00 公共契约](../specifications/service-identity-and-scope-contract.md) | `observation_id`、`sha256-jcs-v1`、`WalPosition` 跨后端稳定不变量。 | ✅ 已验证（`f1_d00_contract` 13 项 Rust + 7 组 Python 黄金向量 100% 一致） |
| `ST-02` 能力握手快照与严格门禁 | [存储适配器契约 §4](../specifications/storage-adapter-contract.md)、[TSDB 共性接口 §5](../specifications/tsdb-common-extension-interface.md) | `GET /v1/storage/capabilities`、`strict_lossless: true` 无损门禁（拒绝非无损同毫秒多观测后端 `422 CAPABILITY_UNSUPPORTED`）与 `tdengine_config.password` 脱敏。 | ✅ 本地已验证（`storage_sqlite_duckdb_tdengine_test`） |
| `ST-03` DuckDB 默认时序与 SQLite 业务分层 | [决策台账 `SD-STORAGE-01`](specification-decisions.md)、[库表设计](../specifications/database-schema-design.md) | SQLite WAL 持久化 14 张业务目录表（删 DuckDB 文件重启自愈）+ DuckDB `v2.0.0` 单事务时序落库 + `D01.2` 单实例锁 + `D10.1` 在线一致性快照备份与沙箱恢复校验。 | ✅ 本地已验证（`storage_sqlite_duckdb_tdengine_test` 4 项测试 + `d07`） |
| `ST-04` 非连续投递检查点与空洞对账 | [存储适配器契约 §5–6](../specifications/storage-adapter-contract.md) | `storage_delivery_ledger`、`GET /v1/storage/checkpoints`、`POST /v1/storage/checkpoints:reconcile`（`101 committed + 102 unknown + 103 committed => checkpoint 101`）。 | ✅ 本地已验证（`storage_sqlite_duckdb_tdengine_test`） |
| `ST-05` 统一查询契约与聚合下推规划 | [北向契约 §7](../specifications/northbound-api-and-subscription-contract.md) | `POST /v1/history/query`（水位线 HMAC 游标、跨版本单位冲突 `422`）与 `POST /v1/storage/query-plan`（DuckDB / TDengine `INTERVAL` / TimescaleDB `time_bucket` / ClickHouse SQL 下推）。 | ✅ 本地已验证（`d08` + `p1_capability_and_derived_metrics_test` + `storage_sqlite_duckdb_tdengine_test`） |
| `ST-06` 外部 TSDB 实例实连与版本锁定验收 | [决策台账 `C05`/`SD-STORAGE-02`](specification-decisions.md)、[时序存储 §9](../specifications/timeseries-storage.md) | TDengine `STABLE`/子表 DDL 与 `/rest/sql/{db}` 线协议已实现并完成内存流验证；外部真实 `taosd` / TimescaleDB / ClickHouse 实例 `A01–A10` / `S01–S11` 实连压测纳入二期。 | 🗓️ 二期范围（用户已确认 taosd 放下一期；一期已完成适配器接口与线协议预备） |
| `ST-07` 跨库平滑迁移、回填与缺口运维 | [决策台账 `SD-STORAGE-02`](specification-decisions.md)、[时序存储 §8](../specifications/timeseries-storage.md) | 基于固定水位的双写回填、`observation_id` 对账与回滚演练（依赖二期 `ST-06` 真实目标库确定后实施）。 | 🗓️ 二期范围（用户已确认 taosd 放下一期；一期已完成适配器接口与线协议预备） |
| `P1-ADAPTER` 边缘适配器能力注册与校验 | [资产模板与数据引用](../specifications/asset-template-and-source-reference.md) | `POST/GET /v1/adapters`、`POST /v1/adapters/{id}:validate-binding`（协议、归一化归属与单位兼容校验，持久化于 SQLite `edge_adapter`）。 | ✅ 本地已验证（`p1_capability_and_derived_metrics_test`） |
| `P1-METRICS` 物模型派生指标与 OEE/能耗计算 | [衍生指标与算法设计](../specifications/derived-metrics-oee-energy.md) | `POST/GET /v1/derived-metrics`、`POST /v1/derived-metrics/{id}:evaluate`（受限确定性求值、坏质量拒算策略、输入血缘追溯，持久化于 SQLite `derived_metric`）。 | ✅ 本地已验证（`p1_capability_and_derived_metrics_test`） |
| `P1-REF-READ` 存量外部时序库只读引用映射 | [决策台账 `SD-STORAGE-02`](specification-decisions.md)、[外部时序库引用映射](../specifications/external-tsdb-reference-mapping.md) | `SourceConnection`、`AttributeReferenceRevision`（`mode="referenced_read"`，安全白名单标识与线性量纲换算，不虚构本地 WAL，`R01–R10`）。 | 🗓️ 二期范围（用户已确认 taosd 放下一期；一期已完成适配器接口与线协议预备） |

## 任务级执行与里程碑门禁

上表 D00–D10 为稳定编号；具体输入、改动范围、依赖、正反验收和停止条件统一见 [AI 执行任务包](ai-execution-plan.md)，不再按大编号直接生成一轮“大实现”。技术边界见[实现架构](../architecture/implementation-architecture.md)，产品及 UX 验收见[体验规范](../product/integration-experience.md)，未决输入见[决策台账](specification-decisions.md)。

进度按产物与证据推进，不虚构日期或人日：契约可校验 → 受控模拟闭环 → 故障恢复与北向完整性 → 真实网关联调 → 目标环境签收。D09 缺现场输入时可以完成离线 fixture，但不能标真实闭环已通过。D11 / D12 属上层项目，不阻塞引擎 P0，也不由引擎任务自动启动。

## 真实项目待确认

现有网关接口及重传机制、首个实际设备点表、目标硬件与负载、数据保留与允许丢失窗口、身份系统、外部 TSDB 首选。可先用明确标注的模拟数据推进，不得当成现场结论。
