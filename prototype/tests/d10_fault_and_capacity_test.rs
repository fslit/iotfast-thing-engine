use duckdb::params;
use iotfast_thing_engine::asset_uns::{
    AssetUnsService, CreateAssetRequest, InstantiateDeviceRequest,
};
use iotfast_thing_engine::binding::{
    ActivateBindingRequest, BindingService, CreateBindingDraftRequest, GatewayBindingAck,
    PointBindingDraft,
};
use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::d00::{
    TelemetryDelivery, TelemetryEnvelope, TelemetryPoint, TelemetryQuality, ValueType,
};
use iotfast_thing_engine::error::ErrorCode;
use iotfast_thing_engine::health::EngineState;
use iotfast_thing_engine::ingest::{SourcePointState, SourceStateEnvelope};
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::northbound::{HistoryQueryRequest, TimeRange};
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::storage_duckdb::{DurabilityPreference, EngineRuntime};
use iotfast_thing_engine::wal::{AppWal, WalRecoveryOutcome};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::time::Instant;

#[derive(Debug, Clone)]
struct SentLedgerEntry {
    message_id: String,
    ingest_id: String,
    payload_hash: String,
    wal_position: String,
    observation_ids: BTreeMap<String, String>,
    expected_values: BTreeMap<String, f64>,
}

#[derive(Debug, Default)]
struct ExternalSentLedger {
    entries: BTreeMap<String, SentLedgerEntry>,
}

impl ExternalSentLedger {
    fn record_acked(
        &mut self,
        message_id: &str,
        ingest_id: &str,
        payload_hash: &str,
        wal_position: &str,
        observation_ids: BTreeMap<String, String>,
        expected_values: BTreeMap<String, f64>,
    ) {
        self.entries.insert(
            message_id.to_string(),
            SentLedgerEntry {
                message_id: message_id.to_string(),
                ingest_id: ingest_id.to_string(),
                payload_hash: payload_hash.to_string(),
                wal_position: wal_position.to_string(),
                observation_ids,
                expected_values,
            },
        );
    }

    fn total_messages(&self) -> usize {
        self.entries.len()
    }

    fn total_observations(&self) -> usize {
        self.entries.values().map(|e| e.observation_ids.len()).sum()
    }

    fn reconcile_with_db(&self, conn: &duckdb::Connection, tenant_id: &str) {
        let db_msg_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM applied_message WHERE tenant_id = ?",
                params![tenant_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            db_msg_count as usize,
            self.total_messages(),
            "ExternalSentLedger message count must match applied_message count (0 lost, 0 extra)"
        );

        let db_obs_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_observation WHERE tenant_id = ?",
                params![tenant_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            db_obs_count as usize,
            self.total_observations(),
            "ExternalSentLedger observation count must match history_observation count (0 lost, 0 duplicate)"
        );

        for (msg_id, entry) in &self.entries {
            assert_eq!(msg_id, &entry.message_id);
            let (db_ingest_id, db_hash, db_wal_pos, db_hist_status): (
                String,
                String,
                String,
                String,
            ) = conn
                .query_row(
                    "SELECT ingest_id, payload_hash, wal_position, history_status
                     FROM applied_message
                     WHERE tenant_id = ? AND message_id = ?",
                    params![tenant_id, msg_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap_or_else(|_| panic!("Missing applied_message row for {}", msg_id));

            assert_eq!(&db_ingest_id, &entry.ingest_id);
            assert_eq!(&db_hash, &entry.payload_hash);
            assert_eq!(&db_wal_pos, &entry.wal_position);
            assert_eq!(db_hist_status, "committed");

            for (prop, obs_id) in &entry.observation_ids {
                let db_val: f64 = conn
                    .query_row(
                        "SELECT value_f64 FROM history_observation
                         WHERE tenant_id = ? AND observation_id = ? AND property = ?",
                        params![tenant_id, obs_id, prop],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|_| {
                        panic!(
                            "Missing history_observation row for message={} prop={} obs_id={}",
                            msg_id, prop, obs_id
                        )
                    });
                let expected_val = entry.expected_values[prop];
                assert!(
                    (db_val - expected_val).abs() < 1e-9,
                    "Observation value mismatch for {}: got {}, expected {}",
                    obs_id,
                    db_val,
                    expected_val
                );
            }
        }
    }
}

fn mgmt_ctx(tenant: &str) -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: tenant.into(),
        project_id: "proj-d10".into(),
        caller_service_id: "svc-mgmt-d10".into(),
        caller_kind: CallerKind::ManagementService,
        issued_at_ms: 1_700_000_000_000,
        expires_at_ms: 1_700_090_000_000,
        allowed_actions: vec![
            "product:write".into(),
            "model:write".into(),
            "model:read".into(),
            "asset:write".into(),
            "device:write".into(),
            "binding:write".into(),
            "binding:read".into(),
            "telemetry:ingest".into(),
            "view:read".into(),
            "shadow:read".into(),
            "history:query".into(),
            "subscription:connect".into(),
            "source_state:report".into(),
        ],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: Some("op-d10".into()),
    }
}

fn gw_ctx(tenant: &str) -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: tenant.into(),
        project_id: "proj-d10".into(),
        caller_service_id: "gw-d10-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_700_000_000_000,
        expires_at_ms: 1_700_090_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter::default(),
        operator_id: None,
    }
}

fn setup_four_point_oven(rt: &mut EngineRuntime, m: &VerifiedScopeContext) {
    ModelCatalogService::create_product(
        rt.conn(),
        m,
        CreateProductRequest {
            product_id: "prod-oven-d10".into(),
            product_key: "reflow-oven-d10".into(),
            name: "D10 Reflow Oven".into(),
        },
    )
    .unwrap();

    ModelCatalogService::create_model_draft(
        rt.conn(),
        m,
        CreateModelDraftRequest {
            model_revision_id: "mr-oven-d10-v1".into(),
            product_id: "prod-oven-d10".into(),
            model_version: "1.0.0".into(),
            properties: vec![
                ModelPropertyDraft {
                    identifier: "zone1_temp".into(),
                    name: "Zone 1 Temperature".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("Cel".into()),
                    unit_symbol: Some("°C".into()),
                    min_value: Some(-40.0),
                    max_value: Some(450.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "zone2_temp".into(),
                    name: "Zone 2 Temperature".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("Cel".into()),
                    unit_symbol: Some("°C".into()),
                    min_value: Some(-40.0),
                    max_value: Some(450.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "conveyor_speed".into(),
                    name: "Conveyor Speed".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("m/s".into()),
                    unit_symbol: Some("m/s".into()),
                    min_value: Some(0.0),
                    max_value: Some(10.0),
                    required: false,
                },
                ModelPropertyDraft {
                    identifier: "chamber_pressure".into(),
                    name: "Chamber Pressure".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("kPa".into()),
                    unit_symbol: Some("kPa".into()),
                    min_value: Some(0.0),
                    max_value: Some(500.0),
                    required: false,
                },
            ],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(rt.conn(), m, "mr-oven-d10-v1", 1_700_000_001_000).unwrap();

    AssetUnsService::create_asset(
        rt.conn(),
        m,
        CreateAssetRequest {
            asset_id: "asset-smt-line-1".into(),
            parent_asset_id: None,
            slug: "smt-line-1".into(),
            name: "SMT Line 1".into(),
        },
    )
    .unwrap();

    AssetUnsService::instantiate_device(
        rt.conn(),
        m,
        InstantiateDeviceRequest {
            device_id: "dev-oven-d10".into(),
            device_key: "oven-d10".into(),
            device_name: "D10 Oven".into(),
            product_id: "prod-oven-d10".into(),
            model_revision_id: "mr-oven-d10-v1".into(),
            asset_id: "asset-smt-line-1".into(),
        },
    )
    .unwrap();

    BindingService::register_source_and_grant(
        rt.conn(),
        m,
        "gw-d10-01",
        "dev-oven-d10",
        1_700_000_002_000,
    )
    .unwrap();

    let draft = BindingService::create_binding_draft(
        rt.conn(),
        m,
        CreateBindingDraftRequest {
            binding_revision_id: "br-oven-d10-v1".into(),
            device_id: "dev-oven-d10".into(),
            model_revision_id: "mr-oven-d10-v1".into(),
            source_id: "gw-d10-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![
                PointBindingDraft {
                    property: "zone1_temp".into(),
                    register_address: "40001".into(),
                    factor: 0.1,
                    unit_code: Some("Cel".into()),
                    deadband: Some(0.1),
                    heartbeat_ms: Some(60_000),
                },
                PointBindingDraft {
                    property: "zone2_temp".into(),
                    register_address: "40002".into(),
                    factor: 0.1,
                    unit_code: Some("Cel".into()),
                    deadband: Some(0.1),
                    heartbeat_ms: Some(60_000),
                },
                PointBindingDraft {
                    property: "conveyor_speed".into(),
                    register_address: "40003".into(),
                    factor: 0.01,
                    unit_code: Some("m/s".into()),
                    deadband: Some(0.01),
                    heartbeat_ms: Some(60_000),
                },
                PointBindingDraft {
                    property: "chamber_pressure".into(),
                    register_address: "40004".into(),
                    factor: 0.1,
                    unit_code: Some("kPa".into()),
                    deadband: Some(0.1),
                    heartbeat_ms: Some(60_000),
                },
            ],
        },
    )
    .unwrap();

    let validated =
        BindingService::validate_binding(rt.conn(), m, &draft.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
        m,
        ActivateBindingRequest {
            binding_revision_id: "br-oven-d10-v1".into(),
            expected_device_revision: 1,
            activated_at_ms: 1_700_000_005_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gw-d10-01".into(),
                source_epoch: "epoch-d10-01".into(),
                binding_revision_id: "br-oven-d10-v1".into(),
                config_hash: validated.config_hash,
                acknowledged_at_ms: 1_700_000_004_900,
            },
        },
    )
    .unwrap();
}

fn make_two_point_envelope(
    message_id: &str,
    source_at_ms: i64,
    seq: u64,
    z1: f64,
    z2: f64,
    replayed: bool,
) -> TelemetryEnvelope {
    TelemetryEnvelope {
        spec: "iotfast.telemetry.v1".into(),
        message_id: message_id.into(),
        source_id: "gw-d10-01".into(),
        source_epoch: "epoch-d10-01".into(),
        device_id: "dev-oven-d10".into(),
        model_revision_id: "mr-oven-d10-v1".into(),
        binding_revision_id: "br-oven-d10-v1".into(),
        points: vec![
            TelemetryPoint {
                property: "zone1_temp".into(),
                value: json!(z1),
                source_at_ms,
                source_time_origin: "gateway_sample".into(),
                source_seq: seq.to_string(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed },
            },
            TelemetryPoint {
                property: "zone2_temp".into(),
                value: json!(z2),
                source_at_ms,
                source_time_origin: "gateway_sample".into(),
                source_seq: seq.to_string(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed },
            },
        ],
    }
}

fn make_four_point_envelope(
    message_id: &str,
    source_at_ms: i64,
    seq: u64,
    z1: f64,
    z2: f64,
    speed: f64,
    pressure: f64,
) -> (TelemetryEnvelope, BTreeMap<String, f64>) {
    let mut expected = BTreeMap::new();
    expected.insert("zone1_temp".to_string(), z1);
    expected.insert("zone2_temp".to_string(), z2);
    expected.insert("conveyor_speed".to_string(), speed);
    expected.insert("chamber_pressure".to_string(), pressure);

    let env = TelemetryEnvelope {
        spec: "iotfast.telemetry.v1".into(),
        message_id: message_id.into(),
        source_id: "gw-d10-01".into(),
        source_epoch: "epoch-d10-01".into(),
        device_id: "dev-oven-d10".into(),
        model_revision_id: "mr-oven-d10-v1".into(),
        binding_revision_id: "br-oven-d10-v1".into(),
        points: vec![
            TelemetryPoint {
                property: "zone1_temp".into(),
                value: json!(z1),
                source_at_ms,
                source_time_origin: "gateway_sample".into(),
                source_seq: seq.to_string(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            },
            TelemetryPoint {
                property: "zone2_temp".into(),
                value: json!(z2),
                source_at_ms,
                source_time_origin: "gateway_sample".into(),
                source_seq: seq.to_string(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            },
            TelemetryPoint {
                property: "conveyor_speed".into(),
                value: json!(speed),
                source_at_ms,
                source_time_origin: "gateway_sample".into(),
                source_seq: seq.to_string(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            },
            TelemetryPoint {
                property: "chamber_pressure".into(),
                value: json!(pressure),
                source_at_ms,
                source_time_origin: "gateway_sample".into(),
                source_seq: seq.to_string(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            },
        ],
    };
    (env, expected)
}

#[test]
fn d10_fault_matrix_f01_to_f05_and_wal_corruption_ledger_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let tenant = "ten-d10-fault";
    let m = mgmt_ctx(tenant);
    let gw = gw_ctx(tenant);

    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    setup_four_point_oven(&mut rt, &m);

    let mut ledger = ExternalSentLedger::default();

    // =========================================================================
    // F01: Pre-WAL rejection / interruption -> 0 WAL bytes, 0 DB rows; retry succeeds
    // =========================================================================
    let wal_before_f01 = AppWal::open(&cfg).unwrap().current_bytes();
    let mut msg_f01_bad =
        make_two_point_envelope("m-f01-rejected", 1_700_000_010_000, 1, 210.0, 215.0, false);
    msg_f01_bad.points[1].property = "unknown_zone99_temp".into();

    let err_f01 = rt
        .ingest_telemetry(
            &gw,
            msg_f01_bad,
            DurabilityPreference::Durable,
            1_700_000_010_050,
        )
        .unwrap_err();
    assert_eq!(err_f01.code, ErrorCode::ValidationFailed);
    assert_eq!(err_f01.http_status(), 422);
    assert_eq!(
        AppWal::open(&cfg).unwrap().current_bytes(),
        wal_before_f01,
        "F01: Rejected batch must not write any bytes to WAL"
    );
    assert_eq!(rt.history_count().unwrap(), 0);
    ledger.reconcile_with_db(rt.conn(), tenant);

    // Retry corrected F01 message -> accepted_durable
    let msg_f01_ok =
        make_two_point_envelope("m-f01-retry-ok", 1_700_000_010_000, 1, 210.5, 215.5, true);
    let rec_f01 = rt
        .ingest_telemetry(
            &gw,
            msg_f01_ok,
            DurabilityPreference::Durable,
            1_700_000_010_100,
        )
        .unwrap();
    assert_eq!(rec_f01.http_status, 202);
    assert_eq!(rec_f01.status, "accepted_durable");
    assert_eq!(rec_f01.history_status, "committed");
    ledger.record_acked(
        "m-f01-retry-ok",
        &rec_f01.ingest_id,
        &rec_f01.payload_hash,
        rec_f01.wal_position.as_deref().unwrap(),
        rec_f01.observation_ids.clone(),
        BTreeMap::from([
            ("zone1_temp".to_string(), 210.5),
            ("zone2_temp".to_string(), 215.5),
        ]),
    );
    ledger.reconcile_with_db(rt.conn(), tenant);

    // =========================================================================
    // F02: WAL incomplete tail frame written before fsync/ACK -> CleanWithTruncatedTail,
    // previously acknowledged messages suffer 0 loss
    // =========================================================================
    let msg_f02_acked =
        make_two_point_envelope("m-f02-acked", 1_700_000_020_000, 2, 220.0, 225.0, false);
    let rec_f02 = rt
        .ingest_telemetry(
            &gw,
            msg_f02_acked,
            DurabilityPreference::Durable,
            1_700_000_020_050,
        )
        .unwrap();
    assert_eq!(rec_f02.status, "accepted_durable");
    ledger.record_acked(
        "m-f02-acked",
        &rec_f02.ingest_id,
        &rec_f02.payload_hash,
        rec_f02.wal_position.as_deref().unwrap(),
        rec_f02.observation_ids.clone(),
        BTreeMap::from([
            ("zone1_temp".to_string(), 220.0),
            ("zone2_temp".to_string(), 225.0),
        ]),
    );
    drop(rt);

    let mut wal = AppWal::open(&cfg).unwrap();
    let valid_len_before_f02 = wal.current_bytes();
    let seg_path = wal.active_segment_path();
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        // Write an incomplete WAL header/payload simulating crash mid-write before fsync
        f.write_all(b"IWAL\x01\x00\x00\x00\xff\x00\x00\x00unacked_partial_tail")
            .unwrap();
        f.sync_all().unwrap();
    }
    assert!(wal.current_bytes() > valid_len_before_f02);

    let scan_f02 = wal.scan_and_recover(None).unwrap();
    assert_eq!(
        scan_f02.outcome,
        WalRecoveryOutcome::CleanWithTruncatedTail {
            truncated_from_offset: valid_len_before_f02
        }
    );
    assert_eq!(wal.current_bytes(), valid_len_before_f02);

    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    let rec_sum_f02 = rt.recover_from_wal().unwrap();
    assert_eq!(rec_sum_f02.replayed_messages, 0);
    ledger.reconcile_with_db(rt.conn(), tenant);

    // =========================================================================
    // F03: WAL fsync completed, crash before DuckDB commit -> recover_from_wal
    // automatically replays into DuckDB and reconciles 100% with ledger
    // =========================================================================
    let msg_f03 = make_two_point_envelope(
        "m-f03-wal-fsynced",
        1_700_000_030_000,
        3,
        235.0,
        240.0,
        false,
    );
    let f03_wal_pos = rt
        .simulate_wal_fsync_without_db_commit(&gw, &msg_f03, 1_700_000_030_050)
        .unwrap();
    // Before recovery, DB still only has 2 messages (4 observations)
    assert_eq!(rt.history_count().unwrap(), 4);
    drop(rt);

    // Restart EngineRuntime and run recover_from_wal()
    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    let rec_sum_f03 = rt.recover_from_wal().unwrap();
    assert_eq!(
        rec_sum_f03.replayed_messages, 1,
        "F03: Uncommitted WAL frame must be replayed on startup"
    );
    assert_eq!(
        rec_sum_f03.latest_checkpoint.as_deref(),
        Some(f03_wal_pos.encode().as_str())
    );

    // Client retries m-f03-wal-fsynced with replayed=true -> receives 200 duplicate with original receipt
    let mut msg_f03_retry = msg_f03.clone();
    msg_f03_retry.points[0].delivery.replayed = true;
    msg_f03_retry.points[1].delivery.replayed = true;
    let rec_f03_dup = rt
        .ingest_telemetry(
            &gw,
            msg_f03_retry,
            DurabilityPreference::Durable,
            1_700_000_030_200,
        )
        .unwrap();
    assert_eq!(rec_f03_dup.http_status, 200);
    assert_eq!(rec_f03_dup.status, "duplicate");
    assert_eq!(rec_f03_dup.history_status, "committed");

    // Verify get_ingest_status reports accepted_durable + committed (Q12)
    let f03_status = rt
        .get_ingest_status(&m, &rec_f03_dup.ingest_id, 1_700_000_030_300)
        .unwrap();
    assert_eq!(f03_status.status, "accepted_durable");
    assert_eq!(f03_status.history_status, "committed");
    assert_eq!(
        f03_status.wal_position.as_deref(),
        Some(f03_wal_pos.encode().as_str())
    );

    ledger.record_acked(
        "m-f03-wal-fsynced",
        &rec_f03_dup.ingest_id,
        &rec_f03_dup.payload_hash,
        rec_f03_dup.wal_position.as_deref().unwrap(),
        rec_f03_dup.observation_ids.clone(),
        BTreeMap::from([
            ("zone1_temp".to_string(), 235.0),
            ("zone2_temp".to_string(), 240.0),
        ]),
    );
    ledger.reconcile_with_db(rt.conn(), tenant);

    // =========================================================================
    // F05: DB committed, crash before target_checkpoint advances -> recover_from_wal
    // scans from earlier checkpoint, idempotently skips via applied_message, 0 duplicates
    // =========================================================================
    let msg_f05 = make_two_point_envelope(
        "m-f05-db-committed",
        1_700_000_040_000,
        4,
        245.0,
        250.0,
        false,
    );
    let rec_f05 = rt
        .ingest_telemetry(
            &gw,
            msg_f05,
            DurabilityPreference::Durable,
            1_700_000_040_050,
        )
        .unwrap();
    assert_eq!(rec_f05.status, "accepted_durable");
    ledger.record_acked(
        "m-f05-db-committed",
        &rec_f05.ingest_id,
        &rec_f05.payload_hash,
        rec_f05.wal_position.as_deref().unwrap(),
        rec_f05.observation_ids.clone(),
        BTreeMap::from([
            ("zone1_temp".to_string(), 245.0),
            ("zone2_temp".to_string(), 250.0),
        ]),
    );

    // Simulate crash where DB committed m-f05-db-committed, but target_checkpoint was still at segment 1 offset 0
    rt.conn()
        .execute("DELETE FROM target_checkpoint WHERE target_id = 'local-duckdb'", [])
        .unwrap();
    drop(rt);

    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    let rec_sum_f05 = rt.recover_from_wal().unwrap();
    assert_eq!(
        rec_sum_f05.replayed_messages, 0,
        "F05: Already-committed messages must be skipped idempotently via applied_message"
    );
    assert_eq!(
        rec_sum_f05.latest_checkpoint,
        rec_f05.wal_position,
        "F05: target_checkpoint must advance to the end of the last applied WAL frame"
    );
    ledger.reconcile_with_db(rt.conn(), tenant);
    drop(rt);

    // =========================================================================
    // WAL-CORRUPTION: Middle frame CRC / payload_len corrupted -> Engine enters
    // HaltedOnWalCorruption, does NOT truncate WAL, and refuses writes with 503
    // =========================================================================
    let mut wal = AppWal::open(&cfg).unwrap();
    let scan_before_corrupt = wal.scan_and_recover(None).unwrap();
    assert_eq!(scan_before_corrupt.records.len(), 4);
    let middle_frame_offset = scan_before_corrupt.records[1].0.record_offset;
    let wal_len_before_corrupt = wal.current_bytes();

    // Corrupt the CRC32 bytes (header offset + 12..16) of record #2 (a middle frame)
    let mut orig_crc_bytes = [0u8; 4];
    {
        use std::io::Read;
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&seg_path)
            .unwrap();
        f.seek(SeekFrom::Start(middle_frame_offset + 12)).unwrap();
        f.read_exact(&mut orig_crc_bytes).unwrap();
        f.seek(SeekFrom::Start(middle_frame_offset + 12)).unwrap();
        f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        f.sync_all().unwrap();
    }

    let mut rt_corrupt = EngineRuntime::open(cfg.clone()).unwrap();
    let recover_err = rt_corrupt.recover_from_wal().unwrap_err();
    assert_eq!(recover_err.code, ErrorCode::InternalError);
    assert_eq!(rt_corrupt.state(), EngineState::HaltedOnWalCorruption);
    assert_eq!(
        AppWal::open(&cfg).unwrap().current_bytes(),
        wal_len_before_corrupt,
        "WAL-CORRUPTION (CRC32): Middle frame corruption must NEVER truncate the WAL segment"
    );

    // Also verify middle frame payload_len corruption (header offset + 8..12) halts recovery
    drop(rt_corrupt);
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&seg_path)
            .unwrap();
        // Restore CRC32 and corrupt payload_len of record #2 to an oversized value (99_999_999)
        f.seek(SeekFrom::Start(middle_frame_offset + 12)).unwrap();
        f.write_all(&orig_crc_bytes).unwrap();
        f.seek(SeekFrom::Start(middle_frame_offset + 8)).unwrap();
        f.write_all(&99_999_999u32.to_le_bytes()).unwrap();
        f.sync_all().unwrap();
    }
    let mut rt_corrupt = EngineRuntime::open(cfg.clone()).unwrap();
    let recover_len_err = rt_corrupt.recover_from_wal().unwrap_err();
    assert_eq!(recover_len_err.code, ErrorCode::InternalError);
    assert_eq!(rt_corrupt.state(), EngineState::HaltedOnWalCorruption);
    assert_eq!(
        AppWal::open(&cfg).unwrap().current_bytes(),
        wal_len_before_corrupt,
        "WAL-CORRUPTION (payload_len): Middle frame payload_len corruption must NEVER truncate the WAL segment"
    );

    let health = rt_corrupt.check_health().unwrap();
    assert_eq!(health.http_status(), 503);

    // Subsequent telemetry ingest must be refused with 503 SERVICE_UNAVAILABLE
    let msg_post_halt =
        make_two_point_envelope("m-post-halt", 1_700_000_050_000, 5, 260.0, 265.0, false);
    let halt_ingest_err = rt_corrupt
        .ingest_telemetry(
            &gw,
            msg_post_halt,
            DurabilityPreference::Durable,
            1_700_000_050_050,
        )
        .unwrap_err();
    assert_eq!(halt_ingest_err.code, ErrorCode::ServiceUnavailable);
    assert_eq!(halt_ingest_err.http_status(), 503);

    // Subsequent source_state report must also be refused with 503 SERVICE_UNAVAILABLE
    let halt_state_err = rt_corrupt
        .report_source_state(
            &gw,
            SourceStateEnvelope {
                spec: "iotfast.source_state.v1".into(),
                event_id: "evt-post-halt".into(),
                source_id: "gw-d10-01".into(),
                source_epoch: "epoch-d10-01".into(),
                device_id: "dev-oven-d10".into(),
                binding_revision_id: "br-oven-d10-v1".into(),
                observed_at_ms: 1_700_000_050_000,
                connection_state: "disconnected".into(),
                point_states: vec![SourcePointState {
                    property: "zone1_temp".into(),
                    quality_state: "bad".into(),
                    quality_reason: "sensor_fault".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                }],
            },
            1_700_000_050_050,
        )
        .unwrap_err();
    assert_eq!(halt_state_err.code, ErrorCode::ServiceUnavailable);
    assert_eq!(halt_state_err.http_status(), 503);

    // Previously committed ledger remains 100% intact in DuckDB
    ledger.reconcile_with_db(rt_corrupt.conn(), tenant);
}

#[test]
fn d10_capacity_watermark_backpressure_and_latency_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let tenant = "ten-d10-cap";
    let m = mgmt_ctx(tenant);
    let gw = gw_ctx(tenant);

    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    setup_four_point_oven(&mut rt, &m);

    let resource_ids = vec![
        AssetUnsService::lookup_resource_id(rt.conn(), "dev-oven-d10", "zone1_temp"),
        AssetUnsService::lookup_resource_id(rt.conn(), "dev-oven-d10", "zone2_temp"),
        AssetUnsService::lookup_resource_id(rt.conn(), "dev-oven-d10", "conveyor_speed"),
        AssetUnsService::lookup_resource_id(rt.conn(), "dev-oven-d10", "chamber_pressure"),
    ];

    let mut ledger = ExternalSentLedger::default();
    let mut batch_latencies_us: Vec<u64> = Vec::with_capacity(50);

    let base_ts = 1_700_000_100_000i64;
    let mut snapshot_cursor: Option<String> = None;
    let mut paged_observation_ids: Vec<String> = Vec::new();
    let mut expected_snapshot_obs_ids: BTreeSet<String> = BTreeSet::new();

    for batch_idx in 1..=50u64 {
        let msg_id = format!("m-cap-{:03}", batch_idx);
        let sample_ts = base_ts + (batch_idx as i64) * 1_000;
        let recv_ts = sample_ts + 20;

        let z1 = 200.0 + (batch_idx as f64) * 0.5;
        let z2 = 210.0 + (batch_idx as f64) * 0.5;
        let speed = 1.0 + (batch_idx as f64) * 0.05;
        let pressure = 101.0 + (batch_idx as f64) * 0.5;

        let (env, expected_vals) =
            make_four_point_envelope(&msg_id, sample_ts, batch_idx, z1, z2, speed, pressure);

        let t0 = Instant::now();
        let receipt = rt
            .ingest_telemetry(&gw, env, DurabilityPreference::Durable, recv_ts)
            .unwrap();
        let elapsed_us = t0.elapsed().as_micros() as u64;
        batch_latencies_us.push(elapsed_us);

        assert_eq!(receipt.http_status, 202);
        assert_eq!(receipt.status, "accepted_durable");
        assert_eq!(receipt.history_status, "committed");
        assert_eq!(receipt.observation_ids.len(), 4);

        if batch_idx <= 25 {
            for obs_id in receipt.observation_ids.values() {
                expected_snapshot_obs_ids.insert(obs_id.clone());
            }
        }

        ledger.record_acked(
            &msg_id,
            &receipt.ingest_id,
            &receipt.payload_hash,
            receipt.wal_position.as_deref().unwrap(),
            receipt.observation_ids,
            expected_vals,
        );

        // At batch 25 (100 observations committed), open a paginated history query (limit 30)
        // capturing watermark_received_at_ms = base_ts + 25_020 (Q08)
        if batch_idx == 25 {
            let page1 = rt
                .query_history(
                    &m,
                    HistoryQueryRequest {
                        resource_ids: resource_ids.clone(),
                        range: TimeRange {
                            start_ms: base_ts,
                            end_ms: base_ts + 100_000,
                        },
                        mode: "raw".into(),
                        aggregation: None,
                        quality_states: vec!["good".into()],
                        order: "asc".into(),
                        limit: 30,
                        cursor: None,
                    },
                )
                .unwrap();
            assert_eq!(page1.items.len(), 30);
            assert!(page1.next_cursor.is_some());
            for item in &page1.items {
                paged_observation_ids.push(item.observation_id.clone());
            }
            snapshot_cursor = page1.next_cursor;
        }

        // Interleave subsequent pagination pages while batches 26..=50 are actively ingested
        if (batch_idx == 35 || batch_idx == 45 || batch_idx == 50) && snapshot_cursor.is_some() {
            while let Some(cur) = snapshot_cursor.take() {
                let page = rt
                    .query_history(
                        &m,
                        HistoryQueryRequest {
                            resource_ids: resource_ids.clone(),
                            range: TimeRange {
                                start_ms: base_ts,
                                end_ms: base_ts + 100_000,
                            },
                            mode: "raw".into(),
                            aggregation: None,
                            quality_states: vec!["good".into()],
                            order: "asc".into(),
                            limit: 30,
                            cursor: Some(cur),
                        },
                    )
                    .unwrap();
                for item in &page.items {
                    assert!(
                        item.received_at_ms <= base_ts + 25_020,
                        "Q08 watermark violation: item received_at_ms {} > watermark {}",
                        item.received_at_ms,
                        base_ts + 25_020
                    );
                    paged_observation_ids.push(item.observation_id.clone());
                }
                snapshot_cursor = page.next_cursor;
                // Only advance one page at batch 35 and 45, drain remaining at batch 50
                if batch_idx < 50 {
                    break;
                }
            }
        }
    }

    // Verify Q08 pagination stability: exactly 100 observations from batches 1..=25, 0 drift, 0 duplicates
    assert_eq!(
        paged_observation_ids.len(),
        100,
        "Paginated query across concurrent writes must return exactly the 100 observations at watermark"
    );
    let paged_unique: BTreeSet<String> = paged_observation_ids.into_iter().collect();
    assert_eq!(paged_unique, expected_snapshot_obs_ids);

    // Compute latency percentiles across the 50 batches (200 points)
    let mut sorted = batch_latencies_us.clone();
    sorted.sort_unstable();
    let min_us = sorted[0];
    let p50_us = sorted[24];
    let p95_us = sorted[47];
    let p99_us = sorted[49];
    let max_us = sorted[49];
    let avg_us = sorted.iter().sum::<u64>() as f64 / (sorted.len() as f64);
    let wal_bytes_50 = AppWal::open(&cfg).unwrap().current_bytes();

    println!(
        "[D10_CAPACITY_PROFILE] batches=50 points=200 min_us={} p50_us={} p95_us={} p99_us={} max_us={} avg_us={:.1} wal_bytes={}",
        min_us, p50_us, p95_us, p99_us, max_us, avg_us, wal_bytes_50
    );

    // Trigger WAL hard watermark backpressure (T07): set wal_hard_watermark_bytes below current WAL size + 1 frame
    let mut bp_cfg = cfg.clone();
    bp_cfg.wal_hard_watermark_bytes = wal_bytes_50 + 16;
    drop(rt);

    let mut rt_bp = EngineRuntime::open(bp_cfg).unwrap();
    let (env_bp, _) = make_four_point_envelope(
        "m-cap-051-backpressure",
        base_ts + 51_000,
        51,
        230.0,
        240.0,
        3.8,
        130.0,
    );
    let bp_err = rt_bp
        .ingest_telemetry(
            &gw,
            env_bp,
            DurabilityPreference::Durable,
            base_ts + 51_020,
        )
        .unwrap_err();
    assert_eq!(bp_err.code, ErrorCode::DurabilityBackpressure);
    assert_eq!(bp_err.http_status(), 503);
    assert_eq!(bp_err.retry_after_ms, Some(1_000));

    // Verify 100% reconciliation between ExternalSentLedger (50 batches, 200 observations) and DuckDB
    assert_eq!(ledger.total_messages(), 50);
    assert_eq!(ledger.total_observations(), 200);
    ledger.reconcile_with_db(rt_bp.conn(), tenant);
}
