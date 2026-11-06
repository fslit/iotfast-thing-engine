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
use iotfast_thing_engine::northbound::{AggregationSpec, HistoryQueryRequest, TimeRange};
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::storage_duckdb::{DurabilityPreference, EngineRuntime};
use iotfast_thing_engine::subscription::{SubscribeRequest, SubscriptionEvent, SubscriptionHub};
use iotfast_thing_engine::wal::{AppWal, WalRecoveryOutcome};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

fn mgmt_ctx(tenant: &str) -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: tenant.into(),
        project_id: "proj-01".into(),
        caller_service_id: "svc-admin".into(),
        caller_kind: CallerKind::ManagementService,
        issued_at_ms: 1_700_000_000_000,
        expires_at_ms: 1_700_003_600_000,
        allowed_actions: vec![
            "product:write".into(), "model:write".into(), "model:read".into(),
            "asset:write".into(), "device:write".into(), "binding:write".into(),
            "binding:read".into(), "telemetry:ingest".into(), "view:read".into(),
            "shadow:read".into(), "history:query".into(), "subscription:connect".into(),
            "source_state:report".into(),
        ],
        scope_filter: ScopeFilter { tenant_wide: true, ..Default::default() },
        operator_id: Some("op-alice".into()),
    }
}

fn gw_ctx(tenant: &str) -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: tenant.into(),
        project_id: "proj-01".into(),
        caller_service_id: "gw-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_700_000_000_000,
        expires_at_ms: 1_700_003_600_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter::default(),
        operator_id: None,
    }
}

fn setup_pump(rt: &mut EngineRuntime, m: &VerifiedScopeContext) {
    ModelCatalogService::create_product(rt.conn(), m, CreateProductRequest {
        product_id: "prod-pump".into(), product_key: "pump".into(), name: "Pump".into(),
    }).unwrap();
    ModelCatalogService::create_model_draft(rt.conn(), m, CreateModelDraftRequest {
        model_revision_id: "pump-r1".into(), product_id: "prod-pump".into(),
        model_version: "1.0.0".into(),
        properties: vec![ModelPropertyDraft {
            identifier: "temperature".into(), name: "Temperature".into(),
            value_type: ValueType::Float64, unit_code: Some("Cel".into()),
            unit_symbol: Some("°C".into()), min_value: Some(-50.0), max_value: Some(500.0),
            required: true,
        }],
    }).unwrap();
    ModelCatalogService::publish_model(rt.conn(), m, "pump-r1", 1_700_000_000_100).unwrap();
    AssetUnsService::create_asset(rt.conn(), m, CreateAssetRequest {
        asset_id: "asset-line-1".into(), parent_asset_id: None, name: "Line 1".into(), slug: "line-1".into(),
    }).unwrap();
    AssetUnsService::instantiate_device(rt.conn(), m, InstantiateDeviceRequest {
        device_id: "dev-pump-01".into(), device_key: "pump-01".into(), device_name: "Pump 01".into(),
        product_id: "prod-pump".into(), model_revision_id: "pump-r1".into(), asset_id: "asset-line-1".into(),
    }).unwrap();
    BindingService::register_source_and_grant(rt.conn(), m, "gw-01", "dev-pump-01", 1_700_000_000_000).unwrap();
    let b = BindingService::create_binding_draft(rt.conn(), m, CreateBindingDraftRequest {
        binding_revision_id: "bind-r1".into(), device_id: "dev-pump-01".into(),
        model_revision_id: "pump-r1".into(), source_id: "gw-01".into(),
        normalization_owner: "gateway".into(), server_scale_factor: None,
        points: vec![PointBindingDraft {
            property: "temperature".into(), register_address: "40001".into(),
            factor: 0.1, unit_code: Some("Cel".into()), deadband: Some(0.5), heartbeat_ms: Some(60_000),
        }],
    }).unwrap();
    let br = BindingService::validate_binding(rt.conn(), m, &b.binding_revision_id).unwrap();
    BindingService::activate_binding(rt.conn(), m, ActivateBindingRequest {
        binding_revision_id: "bind-r1".into(), expected_device_revision: 1,
        activated_at_ms: 1_700_000_000_200,
        gateway_ack: GatewayBindingAck {
            source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
            binding_revision_id: "bind-r1".into(), config_hash: br.config_hash,
            acknowledged_at_ms: 1_700_000_000_190,
        },
    }).unwrap();
}

fn make_msg(msg_id: &str, val: f64, ts: i64, seq: &str) -> TelemetryEnvelope {
    TelemetryEnvelope {
        spec: "iotfast.telemetry.v1".into(), message_id: msg_id.into(),
        source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
        device_id: "dev-pump-01".into(), model_revision_id: "pump-r1".into(),
        binding_revision_id: "bind-r1".into(),
        points: vec![TelemetryPoint {
            property: "temperature".into(), value: json!(val), source_at_ms: ts,
            source_time_origin: "device_clock".into(), source_seq: seq.into(),
            quality: TelemetryQuality { state: "good".into(), source_protocol: None, source_code: None },
            delivery: TelemetryDelivery { replayed: false },
        }],
    }
}

fn make_msg_q(msg_id: &str, val: f64, ts: i64, seq: &str, quality: &str) -> TelemetryEnvelope {
    let mut m = make_msg(msg_id, val, ts, seq);
    m.points[0].quality.state = quality.into();
    m
}

// ===== C1+I1: WAL middle-record corruption halts, EngineRuntime enters HaltedOnWalCorruption =====
#[test]
fn test_c1_i1_wal_middle_corruption_halts_and_engine_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m = mgmt_ctx("8iic");
    let gw = gw_ctx("8iic");
    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    setup_pump(&mut rt, &m);

    // Ingest 3 messages to create 3 WAL records
    for i in 1..=3 {
        rt.ingest_telemetry(&gw, make_msg(&format!("m-{}", i), 100.0 + i as f64, 1_700_000_001_000 + i * 1000, &i.to_string()),
            DurabilityPreference::Durable, 1_700_000_001_010 + i * 1000).unwrap();
    }
    drop(rt);

    // Corrupt middle record (record 2) by overwriting its magic bytes
    let mut wal = AppWal::open(&cfg).unwrap();
    let seg_path = wal.active_segment_path();
    // Scan to find record positions
    let scan_clean = wal.scan_and_recover(None).unwrap();
    assert_eq!(scan_clean.records.len(), 3);
    let pos2_offset = scan_clean.records[1].0.record_offset;
    let orig_len = std::fs::metadata(&seg_path).unwrap().len();

    // Corrupt magic of record 2
    {
        let mut f = OpenOptions::new().read(true).write(true).open(&seg_path).unwrap();
        f.seek(SeekFrom::Start(pos2_offset)).unwrap();
        f.write_all(b"XXXX").unwrap();
        f.sync_all().unwrap();
    }

    let scan = wal.scan_and_recover(None).unwrap();
    assert!(matches!(scan.outcome, WalRecoveryOutcome::HaltedMiddleRecordCorrupted { corrupted_offset, .. }
        if corrupted_offset == pos2_offset), "Expected HaltedMiddleRecordCorrupted, got {:?}", scan.outcome);
    // File must NOT be truncated
    assert_eq!(std::fs::metadata(&seg_path).unwrap().len(), orig_len);

    // I1: EngineRuntime::recover_from_wal sets HaltedOnWalCorruption
    let mut rt2 = EngineRuntime::open(cfg).unwrap();
    let _err = rt2.recover_from_wal().unwrap_err();
    assert_eq!(rt2.state(), EngineState::HaltedOnWalCorruption);
    let health = rt2.check_health().unwrap();
    assert_eq!(health.http_status(), 503);

    // Subsequent writes must be refused
    let write_err = rt2.report_source_state(&gw, SourceStateEnvelope {
        spec: "iotfast.source_state.v1".into(), event_id: "evt-post-halt".into(),
        source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
        device_id: "dev-pump-01".into(), binding_revision_id: "bind-r1".into(),
        observed_at_ms: 1_700_000_005_000, connection_state: "disconnected".into(),
        point_states: vec![],
    }, 1_700_000_005_000).unwrap_err();
    assert_eq!(write_err.code, ErrorCode::ServiceUnavailable);
}

// ===== C3: Restart preserves monotonic ingest_seq and stream_seq =====
#[test]
fn test_c3_restart_preserves_monotonic_sequences() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m = mgmt_ctx("8iic");
    let gw = gw_ctx("8iic");
    {
        let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
        setup_pump(&mut rt, &m);
        let r1 = rt.ingest_telemetry(&gw, make_msg("m-restart-1", 100.0, 1_700_000_001_000, "1"),
            DurabilityPreference::Durable, 1_700_000_001_010).unwrap();
        assert_eq!(r1.ingest_id, "ing-000001");
        assert_eq!(rt.stream_seq, 1);
    }
    // Reopen
    let mut rt2 = EngineRuntime::open(cfg).unwrap();
    assert!(rt2.stream_seq >= 1, "stream_seq must be restored from DB, got {}", rt2.stream_seq);
    let r2 = rt2.ingest_telemetry(&gw, make_msg("m-restart-2", 105.0, 1_700_000_002_000, "2"),
        DurabilityPreference::Durable, 1_700_000_002_010).unwrap();
    assert_eq!(r2.ingest_id, "ing-000002", "ingest_id must not collide after restart");
    assert!(rt2.stream_seq >= 2);
}

// ===== I2: source_state clock-skew, unknown property, monotonic quality =====
#[test]
fn test_i2_source_state_validation_guards() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m = mgmt_ctx("8iic");
    let gw = gw_ctx("8iic");
    let mut rt = EngineRuntime::open(cfg).unwrap();
    setup_pump(&mut rt, &m);

    // Clock skew
    let skew_err = rt.report_source_state(&gw, SourceStateEnvelope {
        spec: "iotfast.source_state.v1".into(), event_id: "evt-future".into(),
        source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
        device_id: "dev-pump-01".into(), binding_revision_id: "bind-r1".into(),
        observed_at_ms: 1_700_000_100_000, connection_state: "disconnected".into(),
        point_states: vec![],
    }, 1_700_000_001_000).unwrap_err();
    assert_eq!(skew_err.code, ErrorCode::ClockSkewExceeded);

    // Unknown property
    let prop_err = rt.report_source_state(&gw, SourceStateEnvelope {
        spec: "iotfast.source_state.v1".into(), event_id: "evt-unk-prop".into(),
        source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
        device_id: "dev-pump-01".into(), binding_revision_id: "bind-r1".into(),
        observed_at_ms: 1_700_000_001_000, connection_state: "degraded".into(),
        point_states: vec![SourcePointState {
            property: "nonexistent".into(), quality_state: "bad".into(),
            quality_reason: "crc".into(), source_protocol: Some("modbus".into()),
            source_code: Some("ERR".into()),
        }],
    }, 1_700_000_001_000).unwrap_err();
    assert_eq!(prop_err.code, ErrorCode::ValidationFailed);

    // Monotonic quality: ingest good at t=2000, then source_state bad at t=1500 must NOT overwrite
    rt.ingest_telemetry(&gw, make_msg("m-q-2000", 200.0, 1_700_000_002_000, "10"),
        DurabilityPreference::Durable, 1_700_000_002_010).unwrap();

    rt.report_source_state(&gw, SourceStateEnvelope {
        spec: "iotfast.source_state.v1".into(), event_id: "evt-old-1500".into(),
        source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
        device_id: "dev-pump-01".into(), binding_revision_id: "bind-r1".into(),
        observed_at_ms: 1_700_000_001_500, connection_state: "connected".into(),
        point_states: vec![SourcePointState {
            property: "temperature".into(), quality_state: "bad".into(),
            quality_reason: "late_crc".into(), source_protocol: None, source_code: None,
        }],
    }, 1_700_000_002_020).unwrap();

    let view = rt.get_device_view(&m, "dev-pump-01", 1_700_000_002_050).unwrap();
    assert_eq!(view.properties["temperature"].quality, "good",
        "Out-of-order older source_state must not overwrite newer quality");
}

// ===== I3: Deadband persists quality transitions, replayed within deadband filtered =====
#[test]
fn test_i3_deadband_quality_transition_and_replayed_filtering() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m = mgmt_ctx("8iic");
    let gw = gw_ctx("8iic");
    let mut rt = EngineRuntime::open(cfg).unwrap();
    setup_pump(&mut rt, &m);

    // Initial good 100.0 -> history=1
    rt.ingest_telemetry(&gw, make_msg("m-db1", 100.0, 1_700_000_001_000, "1"),
        DurabilityPreference::Durable, 1_700_000_001_010).unwrap();
    assert_eq!(rt.history_count().unwrap(), 1);

    // Replayed 100.1 (within 0.5 deadband), same quality -> filtered, history stays 1
    let mut m2 = make_msg("m-db2", 100.1, 1_700_000_002_000, "2");
    m2.points[0].delivery.replayed = true;
    rt.ingest_telemetry(&gw, m2, DurabilityPreference::Durable, 1_700_000_002_010).unwrap();
    assert_eq!(rt.history_count().unwrap(), 1, "Replayed within deadband must be filtered");

    // Value 100.1 but quality changes to "uncertain" -> MUST persist, history=2
    rt.ingest_telemetry(&gw, make_msg_q("m-db3", 100.1, 1_700_000_003_000, "3", "uncertain"),
        DurabilityPreference::Durable, 1_700_000_003_010).unwrap();
    assert_eq!(rt.history_count().unwrap(), 2, "Quality transition must persist to history");
}

// ===== I4: History query desc order, delivery_gap, and aggregation =====
#[test]
fn test_i4_history_desc_delivery_gap_and_aggregation() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m = mgmt_ctx("8iic");
    let gw = gw_ctx("8iic");
    let mut rt = EngineRuntime::open(cfg).unwrap();
    setup_pump(&mut rt, &m);

    for (i, (ts, val)) in [(1_700_000_001_000i64, 10.0), (1_700_000_001_500, 20.0), (1_700_000_002_200, 30.0)]
        .iter().enumerate()
    {
        rt.ingest_telemetry(&gw, make_msg(&format!("m-h{}", i), *val, *ts, &(i+1).to_string()),
            DurabilityPreference::Durable, ts + 10).unwrap();
    }

    // Insert a delivery gap
    rt.conn().execute(
        "INSERT INTO delivery_gap (gap_id, tenant_id, resource_id, start_ms, end_ms, reason)
         VALUES ('gap-01', '8iic', 'res-dev-pump-01-temperature', 1700000001600, 1700000002000, 'buffer_overflow')",
        [],
    ).unwrap();

    // Desc order query
    let desc = rt.query_history(&m, HistoryQueryRequest {
        resource_ids: vec!["res-dev-pump-01-temperature".into()],
        range: TimeRange { start_ms: 1_700_000_000_000, end_ms: 1_700_000_005_000 },
        mode: "raw".into(), aggregation: None,
        quality_states: vec!["good".into()], order: "desc".into(), limit: 2, cursor: None,
    }).unwrap();
    assert!(desc.partial, "Overlapping delivery_gap must set partial=true");
    assert_eq!(desc.known_gaps.len(), 1);
    assert_eq!(desc.items[0].source_at_ms, 1_700_000_002_200, "First item in desc must be newest");
    assert_eq!(desc.items[1].source_at_ms, 1_700_000_001_500);

    // Aggregate mode
    let agg = rt.query_history(&m, HistoryQueryRequest {
        resource_ids: vec!["res-dev-pump-01-temperature".into()],
        range: TimeRange { start_ms: 1_700_000_001_000, end_ms: 1_700_000_003_000 },
        mode: "aggregate".into(),
        aggregation: Some(AggregationSpec { bucket_ms: 1000, functions: vec!["avg".into()] }),
        quality_states: vec!["good".into()], order: "asc".into(), limit: 10, cursor: None,
    }).unwrap();
    assert_eq!(agg.buckets.len(), 2, "Expected 2 buckets for 2 seconds");
    assert_eq!(agg.buckets[0].count, 2); // 10.0 and 20.0 in first second
    assert_eq!(agg.buckets[0].avg, Some(15.0));
    assert_eq!(agg.buckets[1].count, 1); // 30.0 in second second
}

// ===== I5: Cross-tenant isolation =====
#[test]
fn test_i5_cross_tenant_isolation() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m_a = mgmt_ctx("tenant-a");
    let m_b = mgmt_ctx("tenant-b");
    let mut rt = EngineRuntime::open(cfg).unwrap();
    setup_pump(&mut rt, &m_a);

    // Tenant B must NOT see Tenant A's binding
    let err = BindingService::get_binding(rt.conn(), &m_b, "bind-r1").unwrap_err();
    assert_eq!(err.code, ErrorCode::ResourceNotFound);

    // Tenant B must NOT validate Tenant A's model
    let err2 = ModelCatalogService::validate_model(rt.conn(), &m_b, "pump-r1", None).unwrap_err();
    assert_eq!(err2.code, ErrorCode::ResourceNotFound);
}

// ===== I6: Subscription credential expiry + disconnected snapshot freshness =====
#[test]
fn test_i6_subscription_credential_expiry_and_disconnected_freshness() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m = mgmt_ctx("8iic");
    let gw = gw_ctx("8iic");
    let mut rt = EngineRuntime::open(cfg).unwrap();
    setup_pump(&mut rt, &m);

    rt.ingest_telemetry(&gw, make_msg("m-sub1", 88.0, 1_700_000_001_000, "1"),
        DurabilityPreference::Durable, 1_700_000_001_010).unwrap();

    // Mark gateway disconnected
    rt.report_source_state(&gw, SourceStateEnvelope {
        spec: "iotfast.source_state.v1".into(), event_id: "evt-disc".into(),
        source_id: "gw-01".into(), source_epoch: "epoch-01".into(),
        device_id: "dev-pump-01".into(), binding_revision_id: "bind-r1".into(),
        observed_at_ms: 1_700_000_001_500, connection_state: "disconnected".into(),
        point_states: vec![],
    }, 1_700_000_001_500).unwrap();

    let mut hub = SubscriptionHub::new();
    let sub_id = hub.subscribe(&m, SubscribeRequest {
        resource_ids: vec!["res-dev-pump-01-temperature".into()],
        include_initial_snapshot: true, max_pending_events: 10,
    }, &rt, 1_700_000_001_600).unwrap();

    let snap_evts = hub.drain_events(&sub_id);
    match &snap_evts[1] {
        SubscriptionEvent::SnapshotChunk { items } => {
            assert_eq!(items[0].freshness, "stale",
                "Snapshot freshness must be stale when gateway is disconnected");
        }
        other => panic!("Expected SnapshotChunk, got {:?}", other),
    }

    // Publish delta AFTER credential expires_at_ms (1_700_003_600_000)
    hub.publish_point_delta("8iic", "res-dev-pump-01-temperature", 2, json!(90.0), "good", 1_700_003_600_001);
    let post_exp = hub.drain_events(&sub_id);
    assert!(!post_exp.is_empty(), "Must emit event after credential expiry");
    match &post_exp[0] {
        SubscriptionEvent::ResyncRequired { code, .. } => {
            assert_eq!(code, "CREDENTIAL_EXPIRED");
        }
        other => panic!("Expected ResyncRequired(CREDENTIAL_EXPIRED), got {:?}", other),
    }
}
