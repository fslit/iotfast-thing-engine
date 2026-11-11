# 设备物模型与 UNS 数据引擎实现（D00.3–D08 受控闭环）

> **边界说明：** 本目录承载 `iotfast-thing-engine` 的 D00.3–D08 核心领域实现、分层存储（SQLite 业务目录 + 可插拔 `TimeSeriesStorageAdapter`）、HTTP/1.1 REST + RFC 6455 WebSocket 服务端与自动化回归测试。当前已完成本地 SQLite WAL（`business_metadata.sqlite`）+ 默认 DuckDB `v2.0.0`（`engine.duckdb`）/ 可扩展 TDengine 时序适配器 + 预写式 WAL 受控闭环验证；真实工业网关接入（D09）、外部生产 TSDB 集群实测（`C05`）与目标硬件物理断电/生产压测（D10）仍待现场门禁。

## 状态（2026-11-11）

- **30 个自动化测试全部通过**：
  - 15 个阶段、存储分层与端到端集成测试套件（`d01`–`d11` + `mvp_thing_model_server_test` + `storage_sqlite_duckdb_tdengine_test`（4 项测试）+ `p1_capability_and_derived_metrics_test`（3 项测试）+ `ux_journey_and_matrix_test`）
  - 7 个独立审查修复回归测试（`review_fixes_test`）
  - 2 个 `dev_server` 授权边界与事务回滚单元测试
- **存储分层架构与一期工程加固（`SD-STORAGE-01` / `SD-STORAGE-02` / `ST-02` / `ST-04` / `D01.2` / `D10.1`）**：
  - **单实例目录锁与流纪元（`D01.2`）**：`EngineRuntime::open` 启动首步强制校验 `EngineConfig::validate()`（`iotfast_server` 默认采用 `EngineConfig::for_production`），通过 POSIX `flock(LOCK_EX | LOCK_NB)` 独占锁定 `<data_dir>/engine.lock`（多进程并发双写同一目录立即返回 `409 VERSION_CONFLICT`），并每次启动分配唯一 `stream_epoch`（`se-<unix_ms>-<seq>`）。
  - **业务元数据（SQLite WAL）**：`catalog/sqlite_store.rs` 将 14 张 OLTP 业务目录表（`product`、`model_revision`、`model_item`、`asset`、`device`、`resource`、`resource_path`、`source_gateway`、`source_device_grant`、`binding_revision`、`point_binding`、`asset_relation`、`edge_adapter`、`derived_metric`）权威持久化于 `<data_dir>/business_metadata.sqlite`；即使删除 `engine.duckdb`，重启时仍能从 SQLite 完整恢复业务目录与静态配置。
  - **设备时序数据（`TimeSeriesStorageAdapter`）与在线一致性备份恢复（`D10.1`）**：`ingest/ts_adapter.rs` 统一路由 4 张核心设备时序表（`latest_value`、`point_quality_state`、`last_persisted_value`、`history_observation`）及 WAL/投递水位状态表（`applied_message`、`wal_checkpoint`、`storage_delivery_ledger`），一期默认使用嵌入式 DuckDB `v2.0.0`（`<data_dir>/engine.duckdb`），并原生预置 `TdengineTimeSeriesAdapter`（超级表 `STABLE <db>.stb_observations` + 子表 `tb_<resource_id> USING ... TAGS (...)` 自动创建、`/rest/sql/{db}` HTTP REST 写入、`LAST_ROW` 最新值与 `INTERVAL` 聚合查询下推）、TimescaleDB 与 ClickHouse 适配接口（用户已确认外部 `taosd` 实连 `ST-06`、跨库迁移 `ST-07` 与存量直读 `P1-REF-READ` 纳入二期 `SD-STORAGE-02`）。通过 `GET /v1/storage/status`、`POST /v1/storage/timeseries-adapter`（含 `strict_lossless: true` 守卫与 `tdengine_config.password = "***REDACTED***"` 脱敏）、`GET /v1/storage/capabilities`（`ST-02`）、`GET /v1/storage/checkpoints` / `POST /v1/storage/checkpoints:reconcile`（`ST-04`）以及 `POST/GET /v1/storage/backups` / `POST /v1/storage/backups/{id}:verify`（`D10.1` 在线一致性快照备份与隔离沙箱恢复校验）提供完整存储治理能力。
- **D00 跨语言黄金向量（Rust 13 项测试 + Python 7 组黄金向量 / 14 个 `observation_id`）100% 一致**
- **双服务端入口并存**：
  - `iotfast_server`：完整设备物模型、资产树/跨树拓扑、设备平滑升级、动态+静态属性数据引用、耐久/易失遥测入站、北向视图/历史查询、WebSocket 实时订阅与可插拔存储管理守护进程（详见 [MVP 运行手册](../docs/development/mvp-thing-model-runbook.md)）
  - `dev_server`：最小只读开发 profile 烟雾验证服务（详见 [Orca MVP 运行手册](../docs/development/orca-mvp-runbook.md)）

## 目录结构

```
prototype/
├── Cargo.toml
├── build.rs            # 绑定本地 DuckDB v2.0.0 动态库 @rpath 与系统 sqlite3
├── src/
│   ├── core/           # EngineConfig 校验、D00 契约、错误码、健康检查与 HMAC Scope 令牌
│   ├── catalog/        # 产品与物模型版本 (:diff)、资产树与跨树拓扑、设备平滑升级、动态+静态绑定、SQLite 业务存储 (sqlite_store.rs)
│   ├── ingest/         # 入站原子校验、WAL 帧校验/恢复、DuckDB 单事务投影与可插拔 TimeSeriesStorageAdapter (ts_adapter.rs)
│   ├── query/          # 北向 DeviceView/Shadow、水位线稳定分页历史查询与实时订阅 Hub
│   ├── server/         # HTTP/1.1 REST + RFC 6455 WebSocket 服务端实现（含 /v1/storage/*）
│   ├── bin/
│   │   ├── iotfast_server.rs  # 完整物模型与 UNS 数据管理守护进程
│   │   └── dev_server.rs      # 最小只读开发 profile 守护进程
│   └── lib.rs          # 模块入口及向后兼容导出
├── tests/              # 15 个集成测试套件（共 30 项测试，含 mvp_thing_model_server_test 与 storage_sqlite_duckdb_tdengine_test）
└── feasibility/        # 独立可行性探针与跨语言黄金向量（f1_d00_contract 等）
```

## 运行测试与演示

```bash
# 1. 运行全量单元与集成测试（30 passed）
cargo test --locked --manifest-path prototype/Cargo.toml

# 2. 运行 SQLite 14 表业务存储 + DuckDB/TDengine 可插拔时序适配器 + ST-02/ST-04/脱敏 + D01.2 单实例锁 + D10.1 备份恢复专项测试（4 passed）
cargo test --locked --manifest-path prototype/Cargo.toml --test storage_sqlite_duckdb_tdengine_test

# 3. 运行 D00 黄金向量契约测试与 Python 跨语言验证
cargo test --locked --manifest-path prototype/feasibility/f1_d00_contract/Cargo.toml
python3 prototype/feasibility/f1_d00_contract/verify_vectors.py

# 4. 运行设备物模型、UNS 数据管理与存储适配器切换端到端演示
python3 scripts/mvp-thing-model-demo.py
```

详见 [MVP 运行与验收手册](../docs/development/mvp-thing-model-runbook.md)、[存储分层验证报告](../docs/development/evidence/storage-sqlite-duckdb-tdengine-report.md) 与 [测试结论总结](../docs/development/evidence/p0-core-engine-test-summary.md)。
