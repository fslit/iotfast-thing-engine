# 规范收敛、决策与现场输入台账

状态：2026-11-10 D00.2 决议冻结。本文记录已收敛规则、D00.2 正式技术决议和现场输入门禁，不表示全部生产代码与现场联调已验收。职责优先级见[规范入口](../specifications/README.md)。

## 1. 状态词与变更规则

- **设计基线 / 已决议（D00.2 冻结）**：已完成规范级唯一规则裁定并同步权威正文，后续 D00.3–D08 必须严格按此实现与验证，不得由实现者自行放宽。
- **原型证据**：特定版本 / 环境 / 断言通过，不能外推到完整契约。
- **现场待确认**：只能由网关、部署或业务负责人提供输入；阻塞对应现场验收（D09/D10），不阻塞隔离的模拟与契约测试。
- **已验收**：任务的全部前置、测试及证据满足并经评审。

任何契约修改必须同步权威正文、Schema / 样例、正反测试、错误字典、版本和兼容说明；旧观察保留但明确时间和适用范围。若已有消费方依赖旧行为，变更者必须先调查部署与使用证据，不凭“原型”假设无人使用。

## 2. 已收敛的设计边界（C01–C05）

| ID | 问题 | 当前规则 / 本轮处理 | 实现与验证 |
|---|---|---|---|
| C01 | 旧任务把 MCP / 控制 / 页面纳入引擎 | 依职责规范归上层；已同步总体设计 §18/27 与验收入口；只读不禁止配置写入 | 上层独立验收；不得新增控制路由 |
| C02 | 网关断连与点位超时混淆 | 已修正总体 §22/T06、公共 SC08、北向 NB04 和数据库示例：仅有点位证据才设当前质量 bad，断连仅更新连接事实 | D05/D07 分别测试两类事件，原测量时间不可改变 |
| C03 | D00 蓝图历史候选与顶部新结论混杂 | 蓝图新增明确历史状态提示，当前字段规则回到专项契约，证据回到更正记录 | D00.3 产出机器契约及完整正反向量 |
| C04 | “14 个测试通过”被当成验收 | 产品及执行入口均标非生产、有限覆盖；历史报告不重写为本轮结果 | 按 D01–D08 补齐失败回归与全链路验证 |
| C05 | 外部库被写在最低验收但又是 P1 | P0/一期验本地 SQLite WAL + DuckDB `v2.0.0`；用户已确认外部 `taosd`（TDengine）实连验收纳入下一期（二期，`SD-STORAGE-02`） | 一期完成 `TimeSeriesStorageAdapter`、`ST-02`、`ST-04` 与在线快照备份恢复；不以本地通过推断外部集群通过 |

## 3. D00.2 已冻结技术决策（Q01–Q12）与存储分层决议（SD-STORAGE-01 / SD-STORAGE-02）

以下 Q01–Q12、`SD-STORAGE-01` 及 `SD-STORAGE-02` 已完成规范级唯一裁定并同步至对应权威文档；下游实现卡（D00.3–D10）必须按“冻结规则”落地代码与正反测试。

| ID | 冻结决议（唯一规则） | 权威文档落点 | 下游实现与验收门禁（2026-11-10 阶段一落地状态） |
|---|---|---|---|
| Q01 | **原位演进 `prototype/Cargo.toml`**：保持 `core / catalog / ingest / query` 四层结构并扩展 `server` 传输层，保留旧公共路径导出与 `prototype/feasibility/` 独立探针；配置构建脚本绑定本地 `v2.0.0` 动态库 `@rpath`；HTTP/WS 传输适配层与核心领域层解耦 | [实施架构](../architecture/implementation-architecture.md)、[`prototype/Cargo.toml`](../../prototype/Cargo.toml) | **已验证（本地 MVP）**：`d01_health_and_d00_test`、`mvp_thing_model_server_test` 验证干净启动与 `SELECT version() = v2.0.0` |
| Q02 | **不可伪造 `VerifiedScopeContext` 与显式来源映射**：通过 HMAC-SHA256 签名令牌（`iotfast1.<base64url>.<hmac_hex>`）及 `[issued_at_ms, expires_at_ms)` 校验构造上下文；客户端自报 Scope 头/体/查询参数一律 `422 VALIDATION_FAILED`；`source_device_grant` 严格校验 `[valid_from_ms, valid_to_ms)` 半开区间 | [公共契约 §2–3](../specifications/service-identity-and-scope-contract.md)、[`core/scope.rs`](../../prototype/src/core/scope.rs)、[`ingest/validate.rs`](../../prototype/src/ingest/validate.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test` 覆盖过期、未到 `valid_from_ms`（`403`）、伪造 `X-Tenant-Id` 头（`422`）及跨租户隔离（`SC01–SC03`） |
| Q03 | **严格裸 JSON Wire 与受限标量**：对外 HTTP/WS Wire 统一采用裸 JSON 工程值；所有 JSON 对象层拒绝未知字段与重复 key；P0 仅支持 `float64`、`int64`、`bool`、`string`（`text`），复杂类型返回 `422 CAPABILITY_UNSUPPORTED`；南向遥测 `source_time_origin` 仅允许 `device_clock \| protocol_timestamp \| gateway_sample` | [公共契约 §4–5](../specifications/service-identity-and-scope-contract.md)、[遥测契约 §2](../specifications/p0-telemetry-contract.md) | **已验证（本地 MVP）**：`f1_d00_contract`、`d05_d06_ingest_and_wal_test` 与 `orca-mvp-smoke.py` 验证未知字段、重复 key、复杂类型与非法 time origin 负例 |
| Q04 | **状态事件 `iotfast.source_state.v1` 完整闭环**：必填 `observed_at_ms`、`source_time_origin`、`source_seq`、`connection_state`、`point_states`、`delivery.replayed`；按 `sha256-jcs-v1` 对排序后的 `point_states` 计算 `event_hash`（排除 `delivery.replayed`）；断连仅将 `freshness` 置为 `stale` 而不覆盖点位 `quality` | [公共契约 §6](../specifications/service-identity-and-scope-contract.md)、[`ingest/validate.rs`](../../prototype/src/ingest/validate.rs) | **已验证（本地 MVP）**：`d05_d06_ingest_and_wal_test`、`review_fixes_test::test_i2/test_i6` 验证独立幂等域与断连/点位坏质量分离（`T06`/`SC08`） |
| Q05 | **WS 终止帧与重同步帧严格区分**：会话凭据过期或权限撤销时，服务端在线协议发送 `{"type":"subscription_terminated","reason":"CREDENTIAL_EXPIRED"\|"SCOPE_REVOKED",...}` 并关闭连接（SC10）；仅当增量缓冲溢出时发送 `{"type":"resync_required","reason":"BUFFER_OVERFLOW",...}`（T09/NB06） | [公共契约 §3](../specifications/service-identity-and-scope-contract.md)、[`query/subscription.rs`](../../prototype/src/query/subscription.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test` 与 `d08_northbound_and_subscription_test` 分别在 WS 帧级断言两类终止事件 |
| Q06 | **UNS 历史路径永久防跨资源串用**：设备迁址后旧 `canonical_uri` 在 `redirect_until_ms` 内处于 `redirecting`（解析返回 `matched_path_state="redirected"`）；超过 `redirect_until_ms` 后转为 `retired`（解析返回 `404 RESOURCE_NOT_FOUND`）；P0 **永久禁止其他 `resource_id` 占用任何曾被分配的 `canonical_uri`**，仅原 `resource_id` 迁回原位时允许重新激活该路径 | [北向契约 §3.2](../specifications/northbound-api-and-subscription-contract.md)、[`catalog/asset.rs`](../../prototype/src/catalog/asset.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test` 与 `d03_asset_and_uns_test` 完整验证锁期内重定向、到期后 404、他设备永久拒绝抢占（`409`）、原设备迁回成功（`NB02`） |
| Q07 | **WAL 帧边界、目录同步与恢复矩阵**：帧头同时校验 `magic`、版本、`payload_len` 上下界与 CRC32/SHA-256；仅文件末尾未写完的半帧允许截断（F02），任何中间记录损坏立即转 `HaltedOnWalCorruption` 并拒绝自动截断（`WAL-CORRUPTION`）；修复 `accepted_volatile` 落库状态标记 | [总体设计 §23](../architecture/design-specification.md)、[`ingest/wal.rs`](../../prototype/src/ingest/wal.rs)、[`ingest/storage.rs`](../../prototype/src/ingest/storage.rs) | **已验证（本地 MVP）**：`d05_d06_ingest_and_wal_test`、`review_fixes_test::test_c1_i1`、`mvp_thing_model_server_test` 验证尾部截断、中间帧损坏停机与 `accepted_volatile` 状态 |
| Q08 | **P0 历史查询接收水位快照与 HMAC 游标**：首次查询捕获当前已提交的最大 `watermark_received_at_ms`；`next_cursor` 绑定 `(tenant_id, scope_hash, query_fingerprint, watermark_received_at_ms, last_source_at_ms, last_observation_id)` 并附服务端 HMAC-SHA256 签名；后续分页仅读取 `received_at_ms <= watermark_received_at_ms` 的记录 | [北向契约 §7](../specifications/northbound-api-and-subscription-contract.md)、[`query/northbound.rs`](../../prototype/src/query/northbound.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test` 与 `d08_northbound_and_subscription_test` 验证翻页期间并发迟到写入不乱页、游标篡改/跨 Scope 拒绝（`NB07–NB10`） |
| Q09 | **绑定激活双模式与崩溃对账**：激活绑定必须提供 `expected_device_revision` 及（a）可信 `gateway_ack`（校验 `binding_revision_id` + `config_hash`）或（b）显式人工停采窗口 `manual_stop_window`（含 `window_start_ms, window_end_ms, operator_id`） | [北向契约 §4](../specifications/northbound-api-and-subscription-contract.md)、[`catalog/binding.rs`](../../prototype/src/catalog/binding.rs) | **已验证（本地 MVP）**：`d04_binding_lifecycle_test`（`gateway_ack`）与 `mvp_thing_model_server_test`（`manual_stop_window`）双模式验证通过 |
| Q10 | **统一配置项、单实例目录锁、流纪元与容量边界（`D01.2`）**：在 `EngineConfig` 中集中管理 `max_future_skew_ms`、`stale_threshold_ms`、`redirect_retention_ms`、WAL 软/硬水位与游标签名密钥，`EngineRuntime::open` 首步强制执行 `EngineConfig::validate()`（`iotfast_server` 默认采用 `EngineConfig::for_production`），通过 POSIX `flock(LOCK_EX \| LOCK_NB)` 独占锁定 `<data_dir>/engine.lock`（多实例并发双写同一目录立即返回 `409 VERSION_CONFLICT`），并在每次实例启动时分配全局唯一 `stream_epoch`（`se-<unix_ms>-<seq>`） | [实施架构 §6.1](../architecture/implementation-architecture.md)、[`core/config.rs`](../../prototype/src/core/config.rs)、[`ingest/storage.rs`](../../prototype/src/ingest/storage.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test`、`d05_d06_ingest_and_wal_test` 与 `storage_sqlite_duckdb_tdengine_test` 验证非法配置拒绝、硬水位背压（`T07`）、`engine.lock` 单实例独占锁（`409 VERSION_CONFLICT`）与 `stream_epoch` 单调分配 |
| Q11 | **稳定 `resource_id` 权威查表与无碰撞生成**：`resource` 表为 `(tenant_id, device_id, kind, identifier) -> resource_id` 的唯一事实来源，通过 `AssetUnsService::allocate_resource_id` 与 `lookup_resource_id` 权威查表并确定性防碰撞；兼容模型升级沿用原 `resource_id`，破坏性变更拒绝就地升级 | [公共契约 §4.1](../specifications/service-identity-and-scope-contract.md)、[`catalog/asset.rs`](../../prototype/src/catalog/asset.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test` 验证兼容模型升级保留原 `resource_id`、拒绝破坏性升级（`422`）及入站/查询统一查表 |
| Q12 | **入站接收状态与历史状态双维度契约**：入站 ACK 与 `GET /v1/ingestions/{ingest_id}` 统一区分接收边界 `status`（`accepted_durable \| accepted_volatile \| duplicate`）与历史投影进度 `history_status`（`pending \| committed \| failed`） | [遥测契约 §3](../specifications/p0-telemetry-contract.md)、[`ingest/storage.rs`](../../prototype/src/ingest/storage.rs)、[`server/mod.rs`](../../prototype/src/server/mod.rs) | **已验证（本地 MVP）**：`mvp_thing_model_server_test` 验证 durable 与 volatile 入站在 `GET /v1/ingestions/{ingest_id}` 下返回准确双维状态 |
| SD-STORAGE-01 | **业务元数据（SQLite WAL）与设备时序数据（可插拔 `TimeSeriesStorageAdapter`）分层存储架构及 `ST-02`/`ST-04`/凭据脱敏/在线备份恢复（`D10.1`）**：业务/控制面元数据（14 张 OLTP 目录表：`product`、`model_revision`、`model_item`、`asset`、`device`、`resource`、`resource_path`、`source_gateway`、`source_device_grant`、`binding_revision`、`point_binding`、`asset_relation`、`edge_adapter`、`derived_metric`）权威持久化于嵌入式 SQLite `<data_dir>/business_metadata.sqlite`（WAL 模式）；设备时序数据（4 张核心时序表：`latest_value`、`point_quality_state`、`last_persisted_value`、`history_observation` 及 WAL/投递水位状态表 `applied_message`、`wal_checkpoint`、`storage_delivery_ledger`）通过 `TimeSeriesStorageAdapter` 路由，默认使用嵌入式 DuckDB `<data_dir>/engine.duckdb`，并支持扩展至 TDengine（超级表 `STABLE <db>.stb_observations` + 单资源子表 `tb_<resource_id> USING ... TAGS (...)` 自动建表、`/rest/sql/{db}` HTTP REST 写入、`LAST_ROW` 与 `INTERVAL` 聚合下推）、TimescaleDB 与 ClickHouse；所有状态响应中的 `tdengine_config.password` 强制脱敏为 `"***REDACTED***"`；通过 `GET /v1/storage/status`、`POST /v1/storage/timeseries-adapter`（含 `ST-02` `strict_lossless: true` 守卫）、`GET /v1/storage/capabilities`（`ST-02` 能力握手）、`GET /v1/storage/checkpoints` 与 `POST /v1/storage/checkpoints:reconcile`（`ST-04` 非连续投递空洞水位线对账）、`POST /v1/storage/query-plan` 以及 `POST/GET /v1/storage/backups` 与 `POST /v1/storage/backups/{backup_id}:verify`（`D10.1` SQLite `VACUUM INTO` + DuckDB `CHECKPOINT` + `AppWal` 快照备份与隔离沙箱恢复校验）提供完整存储治理闭环 | [`catalog/sqlite_store.rs`](../../prototype/src/catalog/sqlite_store.rs)、[`ingest/ts_adapter.rs`](../../prototype/src/ingest/ts_adapter.rs)、[外部 TSDB 映射规范](../specifications/external-tsdb-reference-mapping.md)、[存储分层与 TDengine 适配验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md) | **已验证（本地 MVP）**：`storage_sqlite_duckdb_tdengine_test`（4 项测试）验证删除 DuckDB 文件后从 `business_metadata.sqlite`（14 张 OLTP 表）完整恢复业务目录、DuckDB 默认落库、TDengine `STABLE`/子表 SQL 与 `/rest/sql/{db}` 线协议、`tdengine_config.password = "***REDACTED***"` 脱敏、`ST-02` 能力握手与 `strict_lossless` 拒绝（`422`）、`ST-04` 非连续投递空洞不越位推进，以及 `D01.2` 单实例目录锁与 `D10.1` 在线一致性快照备份/沙箱恢复/单字节篡改拦截 |
| SD-STORAGE-02 | **外部 `taosd`（TDengine）集群实连、跨库迁移与存量外部库直读纳入二期范围（2026-11-11 用户确认）**：外部 `taosd`（TDengine）生产实例实连验收（`ST-06`）、跨库回填迁移（`ST-07`）与存量外部时序库直读映射（`P1-REF-READ`）**明确纳入下一期（二期）实施范围**；一期正式存储基线聚焦 **单机嵌入式 SQLite WAL（14 张 OLTP 业务目录表）+ 嵌入式 DuckDB `v2.0.0`（4 张设备时序表）+ 应用层 `AppWal` + 在线一致性快照备份与隔离沙箱恢复校验（`D10.1`）**，同时在代码中完整保留可插拔 `TimeSeriesStorageAdapter` 标准接口、`ST-02` 能力握手与 `ST-04` 连续检查点机制，确保二期无缝接入 `taosd` | [研发路线图](roadmap.md)、[存储适配器契约](../specifications/storage-adapter-contract.md)、[外部 TSDB 映射规范](../specifications/external-tsdb-reference-mapping.md)、[存储分层验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md) | **已决议并落盘（2026-11-11）**：一期单机嵌入式 SQLite WAL + DuckDB `v2.0.0` + `AppWal` + 在线快照备份恢复闭环已通过 30 项自动化测试验收；`taosd` 实连（`ST-06`）、跨库回填（`ST-07`）与存量直读（`P1-REF-READ`）作为二期独立里程碑推进 |

## 4. 现场输入与决策责任

“负责人”是职能，不虚构具体人员。AI 创建填写模板并检查完整性，不代替现场签认。

| 输入 | 提供者 | 缺失时可做 | 阻塞 |
|---|---|---|---|
| 脱敏网关消息、语言、ID/epoch持久化、ACK/重试/状态上报和绑定确认能力 | 网关负责人 | 合同 fixture、有限跨语言样例 | D09 网关验收及端到端可靠承诺 |
| 设备点表、单位、倍率、采样周期、地址基数、型号与读取证据 | 实施负责人 | 虚构工程值样例 | 真实绑定激活及现场语义正确性 |
| 服务认证、范围签发和撤销、网络边界与凭据分发 | 安全 / 平台负责人 | 测试签发器及拒绝用例 | 对外部署和真实服务接入 |
| OS/CPU/RAM/磁盘/文件系统、设备点数、平均峰值频率、离线窗口 | 运维 / 项目负责人 | 开发机功能与有限故障探针 | D10 容量、延迟、恢复时限签收 |
| RPO/RTO、保留期、备份恢复及允许丢失策略 | 项目与运维共同确认 | 解释 durable 边界，测试备份流程 | 生产可靠性承诺、清理策略上线 |
| 第一外部 TSDB 和所需查询能力 | 上层集成负责人 | Historian 接口和能力约束设计 | P1 后端选型与兼容宣称 |
| 上层管理台框架 / 平台、用户场景和可用性目标 | 上层产品负责人 | API事实与体验参考规范 | 上层页面实现及 UX 签收 |

## 5. 证据编号防混淆

总体规范 **F01–F05** 是 WAL / DB 的前五个崩溃时点；可行性 **F0–F6** 是研究项目编号，其中 F4 为网关、F5 为规模压测。中间帧损坏单列 `WAL-CORRUPTION`，不再称作总体 F05。旧报告编号保留作历史记录，新增报告必须写“命名空间 + 编号 + 具体故障位置”，不得按相近编号误判覆盖。

## 6. 本轮审查与一期收敛范围

- **D00.1–D00.2 文档审查阶段**：已阅读入口规范、总体 P0、专项契约、路线 / 验收、历史证据和原型模块入口，形成 `C01–C05`、`Q01–Q12`、`SD-STORAGE-01` 与 `SD-STORAGE-02` 冻结决议。
- **2026-11-11 一期（D00.3–D11、`SD-STORAGE-01`/`SD-STORAGE-02`、`ST-02`/`ST-04`、`D01.2`/`D10.1`）代码收敛、存储分层与 HTTP/WS 闭环验证**：
  - 已按 `Q01–Q12`、`SD-STORAGE-01` 与 `SD-STORAGE-02` 完成 `prototype/src/{core,catalog,ingest,query,server}/` 五层实现、单实例 `engine.lock` 独占文件锁与 `stream_epoch` 分配（`D01.2`）、SQLite 业务元数据存储（`business_metadata.sqlite`，14 张 OLTP 业务目录表）+ 嵌入式 DuckDB `v2.0.0`（`engine.duckdb`，4 张核心设备时序表）+ 可插拔 `TimeSeriesStorageAdapter`（预留 TDengine / TimescaleDB / ClickHouse，含 `ST-02` 能力握手、`ST-04` 非连续空洞水位线对账与凭据脱敏）、在线一致性快照备份与隔离沙箱恢复校验（`D10.1` `POST/GET /v1/storage/backups`、`POST /v1/storage/backups/{id}:verify`）以及 `iotfast_server` 独立守护进程（运行说明见 [MVP 运行与验收手册](mvp-thing-model-runbook.md) 与 [存储分层验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md)）。
  - `cargo test --locked --manifest-path prototype/Cargo.toml`（**30 项测试全部通过**，含 `storage_sqlite_duckdb_tdengine_test` 4 项测试与 `mvp_thing_model_server_test`）、`f1_d00_contract`（13 项黄金向量通过 + Python 跨语言 100% 匹配）、`python3 scripts/verify-machine-contracts.py`（49 条路径 / 57 个操作 / 63 个组件 Schema / 58 项路由与 `$ref` 检查 / 66 项正反断言全部通过）及 `python3 scripts/mvp-thing-model-demo.py`（端到端全链路、存储适配器切换与在线快照备份恢复校验演示）均已验证通过（退出码 `0`）。
  - **门禁保持与二期划分**：用户已确认外部 `taosd`（TDengine）集群实连（`ST-06`）、跨库回填迁移（`ST-07`）与存量外部时序库直读（`P1-REF-READ`）纳入下一期（二期，`SD-STORAGE-02`）；真实工业网关现场联调（D09）与目标硬件物理断电/生产级容量压测（D10）仍待现场环境门禁，不得将开发机受控闭环验证等同于现场生产签收。
