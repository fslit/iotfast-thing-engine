# P0 核心物模型与 UNS 数据服务引擎（D01–D08）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 构建通过 F0–F3、F6 可行性实测验证的 Rust 工业物模型与 UNS 数据服务核心引擎（覆盖研发路线 `D01` 至 `D08`），打通不可变物模型、资产树与 UNS 路径重定向、网关绑定版本状态机、工程值原子入站与状态事件分离、应用层 WAL 持久边界与故障恢复、DuckDB 2.0.0 单事务历史与 Latest 投影、以及受 Scope 约束的北向业务视图、历史查询与实时订阅衔接。

**Architecture:** 采用单进程模块化 Rust 引擎架构，底层由嵌入式 DuckDB 2.0.0（`duckdb = "1.20000.1"`）承载元数据、去重表、死区基准、实时投影与历史观测，配合独立的应用层顺序持久日志（48 字节 `IWAL` 帧头 + CRC32/SHA-256 + `File::sync_all`）实现 `accepted_durable` 与故障恢复。所有控制面与数据面入口强制接收服务端验证后的 `VerifiedScopeContext`（默认 fail-closed），通过统一有序状态域（`stream_epoch` + `stream_seq`）保障实时快照与增量订阅无缝衔接。

**Tech Stack:** Rust (`rustc 1.98.1`, Edition 2021), `duckdb = "1.20000.1"`, `serde = "1.0"`, `serde_json = "1.0"`, `sha2 = "0.10"`, `crc32fast = "1.5"`, `data-encoding = "2.11"`, `ryu = "1.0"`, `hex = "0.4"`.

**Spec:**
- `docs/architecture/data-engine-scope.md`
- `docs/architecture/design-specification.md`（第 18–28 节）
- `docs/specifications/service-identity-and-scope-contract.md`
- `docs/specifications/p0-telemetry-contract.md`
- `docs/specifications/northbound-api-and-subscription-contract.md`
- `docs/specifications/d00-integrated-implementation-blueprint.md`（PR #2 `R01–R10` 跨模块收敛规则）
- `docs/specifications/database-schema-design.md`
- `docs/development/roadmap.md`

## Global Constraints

- **架构职责边界**：只读数据服务与模型/资产/绑定管理；严禁内置最终用户 RBAC、业务告警工作流或物理设备控制下发（`ADR 0002`）。
- **版本与引擎约束**：Rust `1.98.1`，`duckdb = "1.20000.1"`，启动与健康检查必须通过 SQL `SELECT version()` 实测核验 `v2.0.0`。
- **工程值责任边界**：P0 默认 `normalization_owner = "gateway"`，网关上报工程值（如 `245.6`），引擎只校验模型与绑定版本，**严禁二次乘倍率**（`T01`）。
- **确定性标识与幂等摘要（D00）**：
  - `observation_id = "obs-" + base32_lower_nopad(sha256("iotfast.obs.v1\0" + tenant_id + "\0" + source_id + "\0" + source_epoch + "\0" + message_id + "\0" + property)[0..16])`
  - `payload_hash` 采用 `sha256-jcs-v1`，显式排除 `points[].delivery.replayed`、`X-Request-Id` 与服务端接收时间。
  - `wal_position` 固定为 33 字节 ASCII `"{segment_no:016x}:{record_offset:016x}"`。
- **测量与状态分离（`T06`、`R03`）**：`POST /v1/telemetry`（`iotfast.telemetry.v1`）禁止 `value: null`（返回 `422`）；通信超时/断连通过 `iotfast.source_state.v1`（独立幂等域 `("source_state", tenant_id, source_id, source_epoch, event_id)` + `event_hash`）上报。仅网关断连（`point_states = []`）只置 `source_connection_state = "disconnected"` 与 `freshness = "stale"`，不伪造全设备传感器 `bad`；带点位错误证据才置对应点位 `quality = "bad"`；网关重连不自动将 `bad` 点位恢复为 `good`。
- **绑定切换后的历史重试顺序（`R08`）**：验签与 `source_device_grant` 通过后，先查 `message_key` 去重表；若该 `message_key` 已存在且按原不可变模型计算的 `payload_hash` 一致，直接返回 `200 duplicate`，不因设备此后已切换到新 `binding_revision_id` 而误判为 `409 REVISION_CONFLICT`。
- **Scope 默认拒绝与字段级裁剪（fail-closed，`R05`、`R09`）**：各非空授权维度在租户内取显式并集并与动作求交；单点越权或未知资源统一返回 `404 RESOURCE_NOT_FOUND`；批量查询含越权资源整批返回 `404` 且不在 `details` 中回显具体失败 ID；`DeviceView` / `DeviceShadow` 对仅获部分 `resource_ids` 授权的调用方逐资源裁剪 `properties`；URI 前缀严格按路径段边界匹配（`uns://8iic/plant-a/` 不匹配 `uns://8iic/plant-ab/`）。
- **严格快照与历史水位诚实披露（`R04`、`R07`）**：WebSocket 严格快照边界 `S` 仅包含 `seq_r <= S` 的资源状态，失守或缓冲溢出立即下发 `resync_required`；历史查询 `coverage` 区分日志位置 `contiguous_wal_position` 与事件时间完整性 `event_time_complete_through_ms`（无法证明时为 `null` / `unknown`）。

## Review Focus

1. **多点遥测中部分属性合法、部分属性未绑定或为 `null`**：必须整条消息返回 `422 VALIDATION_FAILED`，已通过前置检查的合法点位不得写入 WAL、`latest_value` 或 `history_observation`（`T02`、`SC09`）。
2. **同毫秒内 `source_seq` 跨越十进制位数边界（`"999"` vs `"1000"`）**：若按字符串字典序比较会导致 `"999" > "1000"` 使新值被旧值压制；必须解析为 `u64` 比较（`SC06`）。
3. **未来时钟超前漂移污染 `latest_value`**：若一条 `source_at_ms > received_at_ms + max_future_skew_ms` 的异常数据写入 `latest_value`，后续所有正常实时数据都会被判定为“乱序旧数据”而永远无法刷新实时视图；必须在入站阶段拦截返回 `422 CLOCK_SKEW_EXCEEDED`（`SC07`）。
4. **应用层 WAL 中间帧损坏被静默跳过**：若磁盘中间记录 CRC32/SHA-256 校验失败但恢复器继续重放后续帧并宣称恢复成功，将造成隐蔽的数据缺口；必须在损坏偏移处立即停止自动重放并进入 `HaltedOnWalCorruption` 告警状态（`F05`）。
5. **资产迁址后历史 URI 被其他设备立刻抢占**：若设备迁址后原 `canonical_uri` 未在 `redirect_until_ms` 窗口内锁定，新设备注册同名路径会导致旧消费者读到错误设备数据；必须在 `redirect_until_ms` 内拒绝其他资源占用该 URI（`NB02`）。

---

### Task 1: D01 工程骨架、D00 公共契约、Scope 校验与健康检查

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/config.rs`
- Create: `src/error.rs`
- Create: `src/d00.rs`
- Create: `src/scope.rs`
- Create: `src/health.rs`
- Test: `tests/d01_health_and_d00_test.rs`

**Interfaces:**
- Produces:
  - `EngineConfig { data_dir: PathBuf, wal_dir: PathBuf, max_future_skew_ms: i64, wal_soft_watermark_bytes: u64, wal_hard_watermark_bytes: u64, redirect_retention_ms: i64 }`
  - `EngineError { request_id: String, code: ErrorCode, message: String, retryable: bool, retry_after_ms: Option<u64>, details: Vec<FieldIssue> }` (`http_status(&self) -> u16`)
  - `derive_observation_id(tenant_id: &str, source_id: &str, source_epoch: &str, message_id: &str, property: &str) -> Result<String, EngineError>`
  - `compute_telemetry_payload_hash(tenant_id: &str, msg: &TelemetryEnvelope, item_types: &BTreeMap<String, ValueType>) -> Result<String, EngineError>`
  - `WalPosition { segment_no: u64, record_offset: u64 }` (`encode(&self) -> String`, `parse(s: &str) -> Result<Self, EngineError>`)
  - `VerifiedScopeContext` (`require_action(&self, action: &str) -> Result<(), EngineError>`, `allows_resource(&self, tenant_id: &str, asset_chain: &[String], device_id: &str, resource_id: &str, canonical_uri: &str) -> bool`, `scope_hash(&self) -> String`)
  - `check_engine_health(conn: &duckdb::Connection, config: &EngineConfig) -> Result<HealthReport, EngineError>`

- [ ] **Step 1: Write the failing test in `tests/d01_health_and_d00_test.rs`**

```rust
use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::d00::{derive_observation_id, WalPosition};
use iotfast_thing_engine::health::check_engine_health;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};

#[test]
fn d01_reports_duckdb_v2_and_verifies_d00_and_scope_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let conn = duckdb::Connection::open(cfg.db_path()).unwrap();

    let health = check_engine_health(&conn, &cfg).unwrap();
    assert_eq!(health.duckdb_version, "v2.0.0");
    assert!(health.healthy);

    let obs_id = derive_observation_id(
        "ten-8iic",
        "gateway-01",
        "epoch-001",
        "m-20261105-000001",
        "zone1_temp",
    )
    .unwrap();
    assert_eq!(obs_id, "obs-hzg4jz6lu5qzfhygt5o3wtyrpi");

    let p1 = WalPosition { segment_no: 1, record_offset: 9 };
    let p2 = WalPosition { segment_no: 1, record_offset: 10 };
    assert!(p1.encode() < p2.encode());

    let ctx = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_service_id: "svc-scada".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 9_000,
        allowed_actions: vec!["view:read".into()],
        scope_filter: ScopeFilter {
            asset_ids: vec![],
            include_asset_descendants: false,
            device_ids: vec![],
            resource_ids: vec![],
            uri_prefixes: vec!["uns://8iic/plant-a/".into()],
            tenant_wide: false,
        },
        operator_id: None,
    };
    assert!(ctx.allows_resource(
        "ten-8iic",
        &[],
        "pump-01",
        "res-01",
        "uns://8iic/plant-a/room-1/pump-01"
    ));
    assert!(!ctx.allows_resource(
        "ten-8iic",
        &[],
        "pump-ab",
        "res-ab",
        "uns://8iic/plant-ab/pump-01"
    ));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d01_health_and_d00_test`
Expected: FAIL with missing `Cargo.toml` / unresolved module `iotfast_thing_engine`.

- [ ] **Step 3: Implement `Cargo.toml`, `src/config.rs`, `src/error.rs`, `src/d00.rs`, `src/scope.rs`, `src/health.rs`, and `src/lib.rs`**

Reuse the verified `observation_id`, `sha256-jcs-v1`, `WalPosition`, and `VerifiedScopeContext` logic proven in `feasibility/f1_d00_contract` and `feasibility/f0_f2_duckdb_tx`, and query `SELECT version(), source_id FROM pragma_version()` in `check_engine_health`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test d01_health_and_d00_test`
Expected: PASS

---

### Task 2: D02 核心表初始化与物模型生命周期（草稿、校验、不可变发布、版本兼容对比）

**Files:**
- Create: `src/schema.rs`
- Create: `src/model_catalog.rs`
- Modify: `src/lib.rs`
- Test: `tests/d02_model_lifecycle_test.rs`

**Interfaces:**
- Consumes: `VerifiedScopeContext`, `EngineError`
- Produces:
  - `init_core_schema(conn: &duckdb::Connection) -> Result<(), EngineError>`
  - `ModelCatalogService::create_product(conn, ctx, req) -> Result<ProductRecord, EngineError>`
  - `ModelCatalogService::create_model_draft(conn, ctx, req) -> Result<ModelRevisionRecord, EngineError>`
  - `ModelCatalogService::validate_model(conn, ctx, model_revision_id, base_revision_id) -> Result<ModelValidationReport, EngineError>`
  - `ModelCatalogService::publish_model(conn, ctx, model_revision_id, published_at_ms) -> Result<ModelRevisionRecord, EngineError>`
  - `ModelCatalogService::compare_revisions(conn, ctx, from_rev_id, to_rev_id) -> Result<CompatibilityKind, EngineError>` (`CompatibleExtension` | `BreakingChange`)

- [ ] **Step 1: Write the failing test in `tests/d02_model_lifecycle_test.rs`**

Test creating product `circulation-pump`, saving draft `pump-model-r1` (`running_state: bool`, `outlet_pressure: float64 (MPa)`), publishing it in a single DuckDB transaction (verifying `model_revision` + `model_item` rows and `audit_log` entry), rejecting any in-place edit or re-publish of `pump-model-r1` with `409 REVISION_CONFLICT` (`NB01`), classifying adding an optional property in `r2` as `CompatibleExtension`, and classifying changing `outlet_pressure` unit from `MPa` to `kPa` in `r3` as `BreakingChange`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d02_model_lifecycle_test`
Expected: FAIL with unresolved module `model_catalog`.

- [ ] **Step 3: Implement `src/schema.rs` and `src/model_catalog.rs`**

Initialize all P0 tables from `docs/specifications/database-schema-design.md` (`product`, `model_revision`, `model_item`, `asset`, `device`, `resource`, `resource_path`, `source_gateway`, `source_device_grant`, `binding_revision`, `point_binding`, `latest_value`, `point_quality_state`, `quality_event`, `history_observation`, `last_persisted_value`, `applied_message`, `ingest_status`, `target_checkpoint`, `delivery_gap`, `audit_log`) and implement transactional model validation, immutable publishing, and compatibility diffing.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test d02_model_lifecycle_test`
Expected: PASS

---

### Task 3: D03 资产树、设备实例化、稳定 Resource ID 与 UNS 路径重定向

**Files:**
- Create: `src/asset_uns.rs`
- Modify: `src/lib.rs`
- Test: `tests/d03_asset_and_uns_test.rs`

**Interfaces:**
- Consumes: `ModelCatalogService`, `VerifiedScopeContext`, `EngineError`
- Produces:
  - `AssetUnsService::create_asset(conn, ctx, req) -> Result<AssetRecord, EngineError>`
  - `AssetUnsService::instantiate_device(conn, ctx, req) -> Result<DeviceWithResources, EngineError>`
  - `AssetUnsService::relocate_device(conn, ctx, device_id, new_asset_id, now_ms, redirect_retention_ms) -> Result<Vec<ResourcePathRecord>, EngineError>`
  - `AssetUnsService::resolve_resource_by_uri(conn, ctx, uri, now_ms) -> Result<ResolvedResourceUri, EngineError>`

- [ ] **Step 1: Write the failing test in `tests/d03_asset_and_uns_test.rs`**

Test creating asset tree `plant-a -> room-1` and `room-2`, rejecting duplicate sibling `slug` or cyclic parent assignment, instantiating `pump-001` and `pump-002` from `pump-model-r1` (verifying distinct stable `resource_id`s per `BM01`/`AT01`), relocating `pump-001` from `room-1` to `room-2` (`NB02`), verifying `device_id` and `resource_id` remain unchanged, resolving the old URI returns `matched_path_state = "redirected"` with the new canonical URI, and attempting to create a new device occupying the redirecting old URI before `redirect_until_ms` fails with `409 REVISION_CONFLICT`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d03_asset_and_uns_test.rs`
Expected: FAIL with unresolved module `asset_uns`.

- [ ] **Step 3: Implement `src/asset_uns.rs`**

Implement cycle detection, sibling slug uniqueness, atomic device + `resource` + `resource_path` creation, and transactional path rotation with `redirect_until_ms` reservation.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test d03_asset_and_uns_test`
Expected: PASS

---

### Task 4: D04 采集源授权、点位绑定状态机与网关确认对账

**Files:**
- Create: `src/binding.rs`
- Modify: `src/lib.rs`
- Test: `tests/d04_binding_lifecycle_test.rs`

**Interfaces:**
- Consumes: `AssetUnsService`, `VerifiedScopeContext`, `EngineError`
- Produces:
  - `BindingService::register_source_and_grant(conn, ctx, source_id, device_id, valid_from_ms) -> Result<(), EngineError>`
  - `BindingService::create_binding_draft(conn, ctx, req) -> Result<BindingRevisionRecord, EngineError>`
  - `BindingService::validate_binding(conn, ctx, binding_revision_id) -> Result<BindingRevisionRecord, EngineError>`
  - `BindingService::activate_binding(conn, ctx, req) -> Result<BindingRevisionRecord, EngineError>`

- [ ] **Step 1: Write the failing test in `tests/d04_binding_lifecycle_test.rs`**

Test creating a binding draft for `pump-001`, rejecting server-side double scaling when `normalization_owner = "gateway"`, rejecting unit mismatch against `model_item`, validating transition `draft -> ready -> active`, superseding the previous active binding on `pump-001` when activating `r2` with a valid `gateway_ack` (matching `binding_revision_id` and `config_hash`), and rejecting activation when `expected_device_revision` mismatches (`409 REVISION_CONFLICT`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d04_binding_lifecycle_test`
Expected: FAIL with unresolved module `binding`.

- [ ] **Step 3: Implement `src/binding.rs`**

Implement `source_gateway`, `source_device_grant`, `binding_revision`, and `point_binding` (`source_kind = "gateway_measurement"`) validation and activation state machine.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test d04_binding_lifecycle_test`
Expected: PASS

---

### Task 5: D05 & D06 工程值遥测入站、状态分离、应用层 WAL 与故障恢复

**Files:**
- Create: `src/wal.rs`
- Create: `src/ingest.rs`
- Modify: `src/lib.rs`
- Test: `tests/d05_d06_ingest_and_wal_test.rs`

**Interfaces:**
- Consumes: `BindingService`, `WalPosition`, `derive_observation_id`, `compute_telemetry_payload_hash`
- Produces:
  - `AppWal::open(config: &EngineConfig) -> Result<Self, EngineError>`
  - `AppWal::append_durable(&mut self, entry: &WalRecord) -> Result<WalAppendReceipt, EngineError>`
  - `AppWal::scan_and_recover(&mut self, from_checkpoint: Option<&WalPosition>) -> Result<WalRecoveryScan, EngineError>`
  - `IngestService::validate_telemetry(conn, ctx, msg, now_ms, max_future_skew_ms) -> Result<ValidatedTelemetryBatch, EngineError>`
  - `IngestService::validate_source_state(conn, ctx, evt, now_ms) -> Result<ValidatedSourceStateEvent, EngineError>`

- [ ] **Step 1: Write the failing test in `tests/d05_d06_ingest_and_wal_test.rs`**

Test `T02` (1 valid point + 1 unknown property -> `422 VALIDATION_FAILED`), `SC09` (`value: null` in telemetry -> `422`), `SC07` (future clock skew -> `422 CLOCK_SKEW_EXCEEDED`), `T08` (stale binding revision outside active window -> `409 REVISION_CONFLICT`), `T07` (WAL hard watermark reached -> `503 DURABILITY_BACKPRESSURE` without silent downgrade to volatile), `F02` (incomplete tail frame truncated cleanly on recovery), and `F05` (middle record byte corruption halts recovery with `MiddleRecordCorrupted`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d05_d06_ingest_and_wal_test`
Expected: FAIL with unresolved modules `wal` and `ingest`.

- [ ] **Step 3: Implement `src/wal.rs` and `src/ingest.rs`**

Port the 48-byte `IWAL` frame header (`CRC32` + `SHA-256` + `File::sync_all`), tail truncation, middle-corruption halt, and watermark backpressure from `feasibility/f3_f6_wal_and_stream`, and combine with whole-message atomic validation against DuckDB metadata (`device`, `model_item`, `binding_revision`, `point_binding`, `source_device_grant`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test d05_d06_ingest_and_wal_test`
Expected: PASS

---

### Task 6: D07 DuckDB 单事务历史落库、死区/心跳过滤、乱序保护与崩溃重放

**Files:**
- Create: `src/storage_duckdb.rs`
- Modify: `src/lib.rs`
- Test: `tests/d07_history_and_latest_test.rs`

**Interfaces:**
- Consumes: `AppWal`, `IngestService`, `ValidatedTelemetryBatch`, `ValidatedSourceStateEvent`
- Produces:
  - `EngineRuntime::ingest_telemetry(&mut self, ctx: &VerifiedScopeContext, msg: TelemetryEnvelope, durability: DurabilityPreference, now_ms: i64) -> Result<IngestReceipt, EngineError>`
  - `EngineRuntime::report_source_state(&mut self, ctx: &VerifiedScopeContext, evt: SourceStateEnvelope, now_ms: i64) -> Result<SourceStateReceipt, EngineError>`
  - `EngineRuntime::recover_from_wal(&mut self) -> Result<RecoverySummary, EngineError>`

- [ ] **Step 1: Write the failing test in `tests/d07_history_and_latest_test.rs`**

Test `T01` (`245.6` stored as `245.6`), `T03`/`SC04` (`replayed=true` retry returns `200 duplicate` and original `ingest_id`, history count = 1), `T04`/`SC05` (same key modified value returns `409 IDEMPOTENCY_CONFLICT`), `T05`/`SC06` (out-of-order `10:00:02` `25.0` then `10:00:01` `24.0` writes 2 history rows while `latest_value` stays `25.0`; same-ms `source_seq="1000"` beats `"999"`), deadband filtering (`deadband = 0.5`: `245.6 -> 245.8` updates `latest_value` to `245.8` without adding a new history row until heartbeat or `>= 0.5` change, while marking `applied_message` in the same DuckDB transaction), `T06`/`SC08` (`report_source_state` timeout sets `point_quality_state = "bad"` while preserving `latest_value` `245.8` and its `value_sampled_at_ms`), and WAL crash recovery (`F03`/`F04`: WAL synced before DB commit or after DB commit before checkpoint update replays idempotently with zero duplicates).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d07_history_and_latest_test`
Expected: FAIL with unresolved module `storage_duckdb`.

- [ ] **Step 3: Implement `src/storage_duckdb.rs`**

Execute `applied_message` check/insert + deadband/heartbeat evaluation against `last_persisted_value` + `history_observation` insert + `latest_value` & `point_quality_state` projection inside a single DuckDB transaction, and advance `target_checkpoint` after commit.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test d07_history_and_latest_test`
Expected: PASS

---

### Task 7: D08 北向业务视图、实时影子、受限历史查询与 WebSocket 快照—增量衔接

**Files:**
- Create: `src/northbound.rs`
- Create: `src/subscription.rs`
- Modify: `src/lib.rs`
- Test: `tests/d08_northbound_and_subscription_test.rs`

**Interfaces:**
- Consumes: `EngineRuntime`, `VerifiedScopeContext`, `EngineError`
- Produces:
  - `EngineRuntime::get_device_view(&self, ctx: &VerifiedScopeContext, device_id: &str, now_ms: i64) -> Result<DeviceBusinessView, EngineError>`
  - `EngineRuntime::get_device_shadow(&self, ctx: &VerifiedScopeContext, device_id: &str, now_ms: i64) -> Result<DeviceShadowView, EngineError>`
  - `EngineRuntime::query_history(&self, ctx: &VerifiedScopeContext, req: HistoryQueryRequest) -> Result<HistoryQueryResponse, EngineError>`
  - `SubscriptionHub::subscribe(&mut self, ctx: &VerifiedScopeContext, req: SubscribeRequest, snapshot_provider: &EngineRuntime) -> Result<SubscriptionHandle, EngineError>`

- [ ] **Step 1: Write the failing test in `tests/d08_northbound_and_subscription_test.rs`**

Test `BM01`/`SC02`/`T10` (two pumps from the same model stay isolated; service scoped to `line-01` gets `404 RESOURCE_NOT_FOUND` when querying `line-02` view or history), `NB03`/`NB04` (unbound/unreported properties return `value: null, quality: "unknown"`; timeout preserves `last_known_value` and `value_sampled_at_ms`), `NB07`/`NB08`/`NB09`/`NB10` (`[start_ms, end_ms)` half-open interval, same-ms `(source_at_ms, observation_id)` cursor pagination, cross-version unit mismatch `422 CROSS_VERSION_INCOMPATIBLE`, tampered/cross-scope cursor `422 VALIDATION_FAILED`, and `delivery_gap` disclosure with `partial: true`), and `NB05`/`NB06`/`T09` (snapshot `boundary_seq = S` handoff to `delta(stream_seq > S)` with zero missed updates, plus bounded buffer overflow emitting `resync_required(code="BUFFER_OVERFLOW")` and terminating the subscription).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test d08_northbound_and_subscription_test`
Expected: FAIL with unresolved modules `northbound` and `subscription`.

- [ ] **Step 3: Implement `src/northbound.rs` and `src/subscription.rs`**

Wire `get_device_view`, `get_device_shadow`, `query_history` (with HMAC/SHA-256 scope-bound cursor and cross-version unit compatibility check), and `SubscriptionHub` into `EngineRuntime` so every ingested measurement or source-state change increments `stream_seq` and pushes to active scoped subscribers.

- [ ] **Step 4: Run full workspace test suite to verify all D01–D08 tests pass**

Run: `cargo test`
Expected: PASS (all `d01`..`d08` integration tests green, 0 warnings).
