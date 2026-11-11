# 设备物模型与 UNS 数据管理引擎 MVP 运行与验收手册

状态：2026-11-10 阶段一（D00.3–D08）受控闭环已验证。本文档面向集成开发者、实施工程师与验收人员，说明如何编译、启动、配置并验证 `iotfast_server`（完整设备物模型与 UNS 数据管理守护进程）。

> **职责与范围声明**：本手册覆盖 D00.3–D08 在本地嵌入式 SQLite（业务元数据 `business_metadata.sqlite`，WAL 模式）+ 可插拔 `TimeSeriesStorageAdapter`（默认本地 DuckDB `v2.0.0` `engine.duckdb`，可扩展 TDengine / TimescaleDB / ClickHouse）+ 预写式 WAL + HTTP/1.1 REST + RFC 6455 WebSocket 传输层上的受控闭环能力。真实工业网关联调（D09）、外部生产级 TSDB 集群实测（`C05`）与目标硬件物理断电/生产压测（D10）仍待现场输入门禁，不得将本地受控演示等同于现场生产签收。

---

## 1. 环境与构建要求

- **操作系统**：macOS (`aarch64-apple-darwin`) / Linux
- **Rust 工具链**：`1.98.1`（或兼容 stable 工具链）
- **存储引擎（`SD-STORAGE-01` 分层存储）**：
  - **业务/控制面元数据存储（OLTP）**：嵌入式 SQLite（`libsqlite3`，`<data_dir>/data/business_metadata.sqlite`，启用 `PRAGMA journal_mode=WAL`），权威持久化 14 张业务目录表（`product`、`model_revision`、`model_item`、`asset`、`device`、`resource`、`resource_path`、`source_gateway`、`source_device_grant`、`binding_revision`、`point_binding`、`asset_relation`、`edge_adapter`、`derived_metric`）
  - **设备时序数据存储（TSDB）**：通过统一 `TimeSeriesStorageAdapter` 路由 4 张设备时序表（`latest_value`、`point_quality_state`、`last_persisted_value`、`history_observation`），默认使用本地 DuckDB `v2.0.0`（`<data_dir>/data/engine.duckdb`，通过 `prototype/build.rs` 绑定本地动态库 `@rpath`），支持运行时扩展/切换至 TDengine（超级表 `STABLE` + 子表 `tb_<resource_id>`）、TimescaleDB 与 ClickHouse
- **Python 环境**：Python 3.10+（仅使用标准库，用于运行一键验收与跨语言黄金向量验证脚本）

### 编译守护进程

```bash
cargo build --locked --manifest-path prototype/Cargo.toml --bin iotfast_server
```

---

## 2. 启动参数与 Scope 令牌签发

`iotfast_server` 支援以下命令行参数（见 [`prototype/src/bin/iotfast_server.rs`](../../prototype/src/bin/iotfast_server.rs)）：

| 参数 | 默认值 / 示例 | 说明 |
| :--- | :--- | :--- |
| `--listen <HOST:PORT>` | `127.0.0.1:8080` | 监听的 IPv4 Loopback 地址与端口 |
| `--data-dir <PATH>` | `/tmp/iotfast-mvp-data` | SQLite 业务元数据库（`data/business_metadata.sqlite`）、DuckDB 时序库（`data/engine.duckdb`）与 WAL 目录（`wal/`）根路径 |
| `--auth-secret <SECRET>` | 必填或 `IOTFAST_AUTH_SECRET` | 用于校验 `VerifiedScopeContext` HMAC-SHA256 签名的服务端密钥 |
| `--issue-token-json <JSON>` | 可选（单次工具模式） | 传入 `VerifiedScopeContext` JSON，输出 `iotfast1.<base64url>.<hmac_hex>` 令牌后退出 |
| `--stdio-stream` | 可选（无网络沙箱模式） | 通过标准输入/输出承载完整 HTTP/1.1 + RFC 6455 WebSocket 帧，供禁网沙箱执行端到端验证 |

### 2.1 签发测试 Scope 令牌（`Q02`）

所有受保护的北向/控制面与南向接口均采用 `Authorization: Bearer <token>`（WebSocket 支持 `?token=<token>`），**严禁客户端通过 `X-Tenant-Id`、`X-Scope`、`X-Role` 等请求头自报权限**（发现即返回 `422 VALIDATION_FAILED`，`SC02`）。

```bash
# 签发控制面与北向读取令牌
cargo run --locked --manifest-path prototype/Cargo.toml --bin iotfast_server -- \
  --auth-secret "my-dev-hmac-secret-key" \
  --issue-token-json '{
    "tenant_id": "ten-8iic",
    "project_id": "proj-mvp",
    "caller_service_id": "svc-thing-admin",
    "caller_kind": "NorthboundService",
    "issued_at_ms": 1000,
    "expires_at_ms": 4102444800000,
    "allowed_actions": [
      "model:write", "model:read",
      "asset:write", "asset:read",
      "device:write", "device:read",
      "binding:write", "binding:read",
      "view:read", "shadow:read",
      "history:query", "subscription:connect"
    ],
    "scope_filter": {
      "tenant_wide": true,
      "uri_prefixes": [],
      "asset_ids": [],
      "device_ids": [],
      "allowed_properties": []
    },
    "operator_id": "op-admin"
  }'
```

### 2.2 启动 `iotfast_server`

```bash
cargo run --locked --manifest-path prototype/Cargo.toml --bin iotfast_server -- \
  --listen 127.0.0.1:18090 \
  --data-dir /tmp/iotfast-mvp-demo \
  --auth-secret "my-dev-hmac-secret-key"
```

启动后将在标准输出打印 JSON 就绪回执：

```json
{"status":"listening","listen_addr":"127.0.0.1:18090","data_dir":"/tmp/iotfast-mvp-demo"}
```

### 2.3 一键启动 Apifox 本地在线联调服务（`start-apifox-local-server.py`）

为方便在 Apifox 桌面端直接点击调试全部 REST / WebSocket 接口，仓库提供了一键启动脚本 [`scripts/start-apifox-local-server.py`](../../scripts/start-apifox-local-server.py)。该脚本自动编译并启动 `iotfast_server`（默认监听 `127.0.0.1:8080`）、签发全权限长效开发 Bearer Token，并自动预置标准演示数据集（`prod-pump`、`pump-model-r1`/`r2`、`asset-plant-a`/`asset-room-1`、`pump-001`、`gw-01`、`bind-pump-001-r1` 及初始遥测样本 `1.65 MPa`）：

```bash
# 启动 Apifox 本地联调守护进程（监听 http://127.0.0.1:8080 并打印全局 Bearer Token）
python3 scripts/start-apifox-local-server.py --port 8080

# 无阻塞自检模式（启动临时实例、注入种子数据、验证视图/存储/查询计划并退出）
python3 scripts/start-apifox-local-server.py --self-test
```

---

## 3. 核心 HTTP REST 与 WebSocket 接口速查

完整路由实现见 [`prototype/src/server/mod.rs`](../../prototype/src/server/mod.rs)。

### 3.1 健康检查与物模型目录（`D01` / `D02`）

| 方法与路径 | 所需 Action | 功能说明 |
| :--- | :--- | :--- |
| `GET /v1/health` | 免鉴权 | 返回引擎运行状态（`Running` / `HaltedOnWalCorruption`）、DuckDB `SELECT version()`（`v2.0.0`）及目录可写状态 |
| `POST /v1/products` | `model:write` | 创建产品（`product_id`, `name`, `description`） |
| `GET /v1/products` | `model:read` | 列出当前租户下的产品目录 |
| `POST /v1/models` | `model:write` | 创建物模型草稿（`model_revision_id`, `product_id`, `version`, `properties`） |
| `GET /v1/models` | `model:read` | 列出当前租户下的物模型版本摘要 |
| `GET /v1/models/{id}` | `model:read` | 获取指定物模型版本的完整定义与属性列表 |
| `POST /v1/models/{id}:validate` | `model:write` | 校验物模型草稿属性、类型与工程单位 |
| `POST /v1/models/{id}:publish` | `model:write` | 发布并冻结物模型版本（发布后不可原位修改，`BM01`） |
| `GET /v1/models/{base}:diff?target={target}` | `model:read` | 对比两个物模型版本的兼容性（`identical \| compatible_extension \| breaking_change`） |

### 3.2 资产树、跨树拓扑与设备生命周期（`D03` / `Q06` / `Q11`）

| 方法与路径 | 所需 Action | 功能说明 |
| :--- | :--- | :--- |
| `POST /v1/assets` | `asset:write` | 创建层级资产树节点（自动计算 `uns_path`，如 `/ten-8iic/plant-a/pump-room-1`） |
| `GET /v1/assets` | `asset:read` | 查询资产树节点列表 |
| `POST /v1/asset-relations` | `asset:write` | 创建跨树逻辑拓扑关系（如 `FEEDS`、`BACKUP_FOR`、`MONITORS`，拒绝自环与重复边） |
| `GET /v1/asset-relations` | `asset:read` | 按 `asset_id` 过滤查询关联拓扑边 |
| `POST /v1/devices` | `device:write` | 基于已发布物模型实例化设备，在 `resource` 表权威分配稳定 `resource_id` 与 `canonical_uri` |
| `GET /v1/devices` / `GET /v1/devices/{id}` | `device:read` | 查询设备实例详情与当前激活的模型/绑定版本 |
| `POST /v1/devices/{id}:relocate` | `device:write` | 设备跨资产节点迁址：`resource_id` 保持不变，旧路径置 `redirected`；即使过期退化为 `retired` 也永久禁止其他 `resource_id` 抢占（`Q06`） |
| `POST /v1/devices/{id}:upgrade-model` | `device:write` | 设备物模型平滑升级：仅允许 `identical` 或 `compatible_extension`，保留已有属性 `resource_id` 并为新增属性分配 `resource_id`（`Q11`） |
| `GET /v1/resources` | `view:read` | UNS 资源目录查询（支持 `?uri=` 精确解析或 `?asset_id=&include_descendants=true` 子树枚举） |

### 3.3 属性数据引用与南向来源授权（`D04` / `Q09`）

| 方法与路径 | 所需 Action | 功能说明 |
| :--- | :--- | :--- |
| `POST /v1/sources/grants` | `binding:write` | 授予网关 `source_id` 在 `[valid_from_ms, valid_to_ms)` 半开区间内上报指定 `device_id` 的权限（`Q02`） |
| `POST /v1/devices/{id}/bindings` | `binding:write` | 创建属性数据引用草稿：支持 `points`（`gateway_measurement` 动态采集点）与 `static_points`（`static_config` 静态铭牌属性）；默认 `normalization_owner = "gateway"` 禁止服务端二次倍率换算（`T01`） |
| `POST /v1/bindings/{id}:validate` | `binding:write` | 校验数据引用草稿与物模型属性类型/单位一致性 |
| `POST /v1/bindings/{id}:activate` | `binding:write` | 激活数据引用版本：支持网关回执 `gateway_ack` 或人工停采窗口 `manual_stop_window`（`Q09`） |

### 3.4 南向遥测、状态入站与接收状态查询（`D05`–`D07` / `Q03` / `Q04` / `Q07` / `Q12`）

| 方法与路径 | 所需 Action | 功能说明 |
| :--- | :--- | :--- |
| `POST /v1/telemetry` | `telemetry:ingest` | 整批原子校验并入站工程值遥测（`iotfast.telemetry.v1`）：支持 `Prefer: durability=durable\|volatile`，严格区分 `accepted_durable`、`accepted_volatile` 与 `duplicate` |
| `POST /v1/source-state` | `telemetry:ingest` | 独立幂等域上报网关连接状态与点位故障（`iotfast.source_state.v1`）：网关断连仅将 `freshness` 置为 `stale`，不篡改最后工程值质量（`Q04`/`SC08`） |
| `GET /v1/ingestions/{ingest_id}` | 有效租户 Scope | 查询指定入站批次的双维状态：`status`（`accepted_durable \| accepted_volatile \| duplicate`）与 `history_status`（`pending \| committed \| failed`）（`Q12`） |

### 3.5 北向业务视图、历史查询与 WebSocket 实时订阅（`D08` / `Q05` / `Q08`）

| 方法与路径 | 所需 Action | 功能说明 |
| :--- | :--- | :--- |
| `GET /v1/devices/{id}/view` | `view:read` | 返回按物模型完整展开的业务视图：区分 `bound`（实时采集）、`bound_static`（静态铭牌配置，`freshness="static"`）与 `unbound`（未配置，`value=null`） |
| `GET /v1/devices/{id}/shadow` | `shadow:read` | 返回携带设备 `revision` 与连接状态的设备影子视图 |
| `POST /v1/history/query` | `history:query` | 受限历史查询（半开区间 `[start_ms, end_ms)`、跨版本单位兼容性检查 `NB08`、绑定 `watermark_received_at_ms` 的 HMAC 签名分页游标 `Q08`） |
| `WS /v1/subscriptions` | `subscription:connect` | RFC 6455 WebSocket 实时订阅：严格执行 `snapshot_begin(S) -> snapshot_chunk(<=S) -> snapshot_end(S) -> delta(>S)`，并在线协议上区分 `subscription_terminated`（`CREDENTIAL_EXPIRED \| SCOPE_REVOKED`）与 `resync_required`（`BUFFER_OVERFLOW`）（`Q05`） |

### 3.6 存储分层架构、单实例目录锁、可插拔时序适配器与一致性备份恢复（`SD-STORAGE-01` / `SD-STORAGE-02` / `ST-02` / `ST-04` / `D01.2` / `D10.1`）

引擎将业务/控制面元数据与设备时序数据分层存储（详见 [存储分层验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md)）：

- **单实例目录锁与流纪元（`D01.2`）**：`EngineRuntime::open` 启动首步强制执行 `EngineConfig::validate()`（`iotfast_server` 默认采用 `EngineConfig::for_production` 水位配置），通过 POSIX `flock(LOCK_EX | LOCK_NB)` 独占锁定 `<data_dir>/engine.lock`（多进程并发双写同一目录立即返回 `409 VERSION_CONFLICT`），并在每次实例启动时分配全局唯一 `stream_epoch`（`se-<unix_ms>-<seq>`）。
- **业务元数据存储（SQLite WAL）**：`business_metadata.sqlite` 权威保存 14 张 OLTP 目录表（`product`、`model_revision`、`model_item`、`asset`、`device`、`resource`、`resource_path`、`source_gateway`、`source_device_grant`、`binding_revision`、`point_binding`、`asset_relation`、`edge_adapter`、`derived_metric`）。即使删除或重建时序库文件，重启后也能从 SQLite 完整恢复业务目录。
- **设备时序数据存储（`TimeSeriesStorageAdapter`）与二期边界（`SD-STORAGE-02`）**：一期正式存储基线采用嵌入式 DuckDB `v2.0.0`（`engine.duckdb`）写入 4 张核心时序表（`latest_value`、`point_quality_state`、`last_persisted_value`、`history_observation`）及 WAL/投递水位状态表（`applied_message`、`wal_checkpoint`、`storage_delivery_ledger`），并在代码中完整保留 **TDengine**（超级表 `STABLE <db>.stb_observations` + 单资源子表 `tb_<resource_id> USING ... TAGS (...)` 自动创建、`/rest/sql/{db}` HTTP REST 写入、`LAST_ROW` 最新值查询与 `PARTITION BY resource_id INTERVAL(...) FILL(NONE)` 降采样聚合）、**TimescaleDB** 与 **ClickHouse** 适配接口（用户已确认外部 `taosd` 生产实例实连 `ST-06`、跨库迁移 `ST-07` 与存量直读 `P1-REF-READ` 纳入二期范围）。所有诊断与状态响应中的 `tdengine_config.password` 均强制脱敏为 `"***REDACTED***"`。
- **在线一致性快照备份与隔离沙箱恢复校验（`D10.1`）**：通过 SQLite `VACUUM INTO` + DuckDB `CHECKPOINT` + `AppWal` 段拷贝生成含全文件 SHA-256 清单的 `backup_manifest.json`，并支持在临时隔离沙箱目录启动独立 `EngineRuntime` 重放 WAL、核对 14 张业务表与时序表行数及 SHA-256 摘要（篡改单字节即检出失败）。

| 方法与路径 | 所需鉴权 | 功能说明 |
| :--- | :--- | :--- |
| `GET /v1/storage/status` | 有效租户 Scope | 查询存储分层状态：返回 `business_metadata_store`（`engine="sqlite"`、`journal_mode="wal"`、14 张业务表行数）与 `device_timeseries_store`（当前 `active_backend`、支持后端列表、已脱敏 `tdengine_config.password="***REDACTED***"`、TDengine `STABLE` DDL 与最近生成的 SQL 日志、4 张时序表行数） |
| `POST /v1/storage/timeseries-adapter` | 有效租户 Scope | 在线切换时序存储适配器（支持 `"duckdb"`、`"tdengine"`、`"timescaledb"`、`"clickhouse"`；支持 `strict_lossless: true` `ST-02` 无损能力守卫，对 `same_timestamp_distinct` 或 `exact_id_roundtrip` 不为 `supported` 的后端返回 `422 CAPABILITY_UNSUPPORTED` / `LOSSLESS_SAME_TIMESTAMP_UNSUPPORTED`） |
| `GET /v1/storage/capabilities` | 有效租户 Scope | 查询 `ST-02` 时序后端能力握手矩阵（返回当前激活后端及全部候选后端在 `exact_id_roundtrip`、`same_timestamp_distinct`、`out_of_order_insert`、`idempotent_dedupe`、`watermark_Read_filter`、`half_open_time_range` 等维度的支持等级） |
| `GET /v1/storage/checkpoints` | 有效租户 Scope | 查询 `ST-04` 非连续投递水位线与空洞账本状态（返回 `contiguous_committed_ingest_seq`、`contiguous_committed_wal_pos`、`max_committed_ingest_seq`、`has_unresolved_holes` 与 `unresolved_holes` 列表） |
| `POST /v1/storage/checkpoints:reconcile` | 有效租户 Scope | 对账 `ST-04` 投递状态机（支持 `record_delivery`、`mark_committed`、`isolate_gap` 动作；严格保证 `101 committed + 102 unknown + 103 committed => checkpoint 101`，直至 `102` 补齐或显式隔离后才推进至 `103`） |
| `POST /v1/storage/query-plan` | 有效租户 Scope | 预览当前激活时序适配器针对给定 `resource_ids`、时间范围与 `bucket_ms` 生成的原生 SQL 查询下推计划 |
| `POST /v1/storage/backups` | 有效租户 Scope | 创建在线一致性快照备份（`D10.1`）：执行 SQLite `VACUUM INTO`、DuckDB `CHECKPOINT` 并拷贝 `AppWal` 段，生成含全文件 SHA-256 摘要与表行数的 `backup_manifest.json` |
| `GET /v1/storage/backups` | 有效租户 Scope | 列出当前数据目录下已创建的存储快照备份清单（`D10.1`） |
| `POST /v1/storage/backups/{backup_id}:verify` | 有效租户 Scope | 在隔离临时沙箱目录执行备份恢复演练与完整性校验（`D10.1`）：校验全部备份文件 SHA-256、独立启动 `EngineRuntime` 重放 WAL 并核对 14 张 SQLite 业务表与 DuckDB 历史/最新值条数 |

### 3.7 边缘适配器能力注册表、衍生指标引擎与容器探针（Stage 6 扩展）

引擎在控制面与数据面之上提供边缘协议适配器能力声明（`edge_adapter` 表）、物模型衍生指标定义与在线评估（`derived_metric` 表，覆盖 OEE / 区间能耗 / 单位能耗）以及标准容器探针与鉴权连通性探针（详见 [P1 能力与衍生指标报告](evidence/p1-capability-and-derived-metrics-report.md)）：

| 方法与路径 | 所需 Action / 鉴权 | 功能说明 |
| :--- | :--- | :--- |
| `POST /v1/adapters` | `binding:write` | 注册或更新边缘协议适配器能力声明（`adapter_id`、`protocol`、`supported_value_types`、`supports_gateway_ack`、`supports_offline_replay` 等），持久化至 SQLite `edge_adapter` 表 |
| `GET /v1/adapters` | `binding:read` | 列出当前租户已注册的边缘协议适配器能力目录 |
| `POST /v1/adapters/{adapter_id}:validate-binding` | `binding:write` | 对照指定边缘适配器的协议能力边界校验绑定草稿/版本兼容性 |
| `POST /v1/derived-metrics` | `model:write` | 创建物模型衍生指标定义（`metric_id`、`product_id`、`formula_kind`：`energy_consumption \| specific_energy \| oee`、输入属性映射与单位），持久化至 SQLite `derived_metric` 表 |
| `GET /v1/derived-metrics` | `model:read` | 列出当前租户已定义的物模型衍生指标目录 |
| `POST /v1/derived-metrics/{metric_id}:evaluate` | `history:query` | 在指定设备与时间窗口上评估衍生指标（结合实时/历史 `collected` 观测与 `static` 静态配置属性），输出计算值、`calculation_status`（`final \| uncertain \| uncalculable`）与 `observation_id` 血缘引用 |
| `GET /v1/ping` | 有效租户 Scope | 控制面/北向鉴权连通性探针，校验 Bearer Token 并返回已验证的 `tenant_id`、`caller_service_id` 与服务端时间戳 |
| `GET /v1/ingest/ping` | `telemetry:ingest` | 南向采集网关入站权限与连通性探针，校验南向 Token 是否具备 `telemetry:ingest` 权限 |
| `GET /health`、`GET /v1/health`、`GET /healthz`、`GET /readyz` | 免鉴权 | 容器存活（Liveness）与就绪（Readiness）探针，返回引擎状态、DuckDB `v2.0.0` 版本、SQLite 版本及 `data_dir` / `wal_dir` / `business_metadata.sqlite` 可写状态 |

---

## 4. 一键可复现验收命令

```bash
# 1. 运行一键端到端物模型、UNS 数据管理与存储适配器切换演示脚本（覆盖 8 大核心步骤）
python3 scripts/mvp-thing-model-demo.py

# 2. 运行 Apifox 本地在线联调启动器自检（或使用 --port 8080 常驻监听）
python3 scripts/start-apifox-local-server.py --self-test

# 3. 运行存储分层（SQLite 14 表业务目录恢复 + DuckDB 默认时序 + TDengine 适配 + ST-02/ST-04/脱敏 + D01.2 单实例锁 + D10.1 备份恢复，4 passed）专项集成测试
cargo test --locked --manifest-path prototype/Cargo.toml --test storage_sqlite_duckdb_tdengine_test

# 4. 运行全量 Rust 单元与集成测试套件（30 passed，含 HTTP/WS 全链路集成测试）
cargo test --locked --manifest-path prototype/Cargo.toml

# 5. 运行 D00 黄金向量契约测试与 Rust ↔ Python 跨语言一致性校验
cargo test --locked --manifest-path prototype/feasibility/f1_d00_contract/Cargo.toml
python3 prototype/feasibility/f1_d00_contract/verify_vectors.py
```
