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
use iotfast_thing_engine::ingest::IngestService;
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::schema::init_core_schema;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::wal::{AppWal, WalRecoveryOutcome};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

fn mgmt_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_service_id: "svc-mgmt".into(),
        caller_kind: CallerKind::ManagementService,
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec![
            "product:write".into(),
            "model:write".into(),
            "asset:write".into(),
            "device:write".into(),
            "binding:write".into(),
        ],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: Some("op-alice".into()),
    }
}

fn gateway_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_service_id: "gateway-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter::default(),
        operator_id: None,
    }
}

#[test]
fn d05_d06_validates_telemetry_atomically_and_recovers_wal_f02_f05_t07() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = EngineConfig::for_test(tmp.path());
    let conn = duckdb::Connection::open(cfg.db_path()).unwrap();
    init_core_schema(&conn).unwrap();

    let m_ctx = mgmt_ctx();
    let gw_ctx = gateway_ctx();

    ModelCatalogService::create_product(
        &conn,
        &m_ctx,
        CreateProductRequest {
            product_id: "prod-oven".into(),
            product_key: "reflow-oven".into(),
            name: "回流焊炉".into(),
        },
    )
    .unwrap();
    ModelCatalogService::create_model_draft(
        &conn,
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "mr-oven-1".into(),
            product_id: "prod-oven".into(),
            model_version: "1.0.0".into(),
            properties: vec![
                ModelPropertyDraft {
                    identifier: "zone1_temp".into(),
                    name: "一区温度".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("Cel".into()),
                    unit_symbol: Some("°C".into()),
                    min_value: Some(-40.0),
                    max_value: Some(400.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "zone2_temp".into(),
                    name: "二区温度".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("Cel".into()),
                    unit_symbol: Some("°C".into()),
                    min_value: Some(-40.0),
                    max_value: Some(400.0),
                    required: true,
                },
            ],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(&conn, &m_ctx, "mr-oven-1", 5_000).unwrap();

    AssetUnsService::create_asset(
        &conn,
        &m_ctx,
        CreateAssetRequest {
            asset_id: "asset-line-01".into(),
            parent_asset_id: None,
            slug: "line-01".into(),
            name: "一产线".into(),
        },
    )
    .unwrap();
    AssetUnsService::instantiate_device(
        &conn,
        &m_ctx,
        InstantiateDeviceRequest {
            device_id: "dev-oven-02".into(),
            device_key: "oven-02".into(),
            device_name: "2号回流焊".into(),
            product_id: "prod-oven".into(),
            model_revision_id: "mr-oven-1".into(),
            asset_id: "asset-line-01".into(),
        },
    )
    .unwrap();

    BindingService::register_source_and_grant(&conn, &m_ctx, "gateway-01", "dev-oven-02", 10_000)
        .unwrap();
    let b1 = BindingService::create_binding_draft(
        &conn,
        &m_ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "br-oven-12".into(),
            device_id: "dev-oven-02".into(),
            model_revision_id: "mr-oven-1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "zone1_temp".into(),
                register_address: "40001".into(),
                factor: 0.1,
                unit_code: Some("Cel".into()),
                deadband: Some(0.5),
                heartbeat_ms: Some(60_000),
            }],
        },
    )
    .unwrap();
    let b1_ready = BindingService::validate_binding(&conn, &m_ctx, &b1.binding_revision_id).unwrap();
    BindingService::activate_binding(
        &conn,
        &m_ctx,
        ActivateBindingRequest {
            binding_revision_id: "br-oven-12".into(),
            expected_device_revision: 1,
            activated_at_ms: 20_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "br-oven-12".into(),
                config_hash: b1_ready.config_hash,
                acknowledged_at_ms: 19_900,
            },
        },
    )
    .unwrap();

    // T02: 1 valid point (zone1_temp) + 1 unknown property (zone9_temp) -> 422 VALIDATION_FAILED
    let msg_t02 = TelemetryEnvelope {
        spec: "iotfast.telemetry.v1".into(),
        message_id: "m-t02".into(),
        source_id: "gateway-01".into(),
        source_epoch: "epoch-001".into(),
        device_id: "dev-oven-02".into(),
        model_revision_id: "mr-oven-1".into(),
        binding_revision_id: "br-oven-12".into(),
        points: vec![
            TelemetryPoint {
                property: "zone1_temp".into(),
                value: json!(245.6),
                source_at_ms: 50_000,
                source_time_origin: "gateway_sample".into(),
                source_seq: "1".into(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            },
            TelemetryPoint {
                property: "zone9_temp".into(),
                value: json!(250.0),
                source_at_ms: 50_000,
                source_time_origin: "gateway_sample".into(),
                source_seq: "1".into(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: None,
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            },
        ],
    };
    let err_t02 = IngestService::validate_telemetry(
        &conn,
        &gw_ctx,
        &msg_t02,
        50_100,
        cfg.max_future_skew_ms,
    )
    .unwrap_err();
    assert_eq!(err_t02.code, ErrorCode::ValidationFailed);
    assert_eq!(err_t02.http_status(), 422);

    // SC09: value: null in telemetry -> 422 VALIDATION_FAILED
    let mut msg_sc09 = msg_t02.clone();
    msg_sc09.message_id = "m-sc09".into();
    msg_sc09.points.truncate(1);
    msg_sc09.points[0].value = json!(null);
    let err_sc09 = IngestService::validate_telemetry(
        &conn,
        &gw_ctx,
        &msg_sc09,
        50_100,
        cfg.max_future_skew_ms,
    )
    .unwrap_err();
    assert_eq!(err_sc09.code, ErrorCode::ValidationFailed);

    // SC07: future clock skew -> 422 CLOCK_SKEW_EXCEEDED
    let mut msg_sc07 = msg_t02.clone();
    msg_sc07.message_id = "m-sc07".into();
    msg_sc07.points.truncate(1);
    msg_sc07.points[0].source_at_ms = 50_100 + cfg.max_future_skew_ms + 5_000;
    let err_sc07 = IngestService::validate_telemetry(
        &conn,
        &gw_ctx,
        &msg_sc07,
        50_100,
        cfg.max_future_skew_ms,
    )
    .unwrap_err();
    assert_eq!(err_sc07.code, ErrorCode::ClockSkewExceeded);

    // T08: stale/unknown binding revision -> 409 REVISION_CONFLICT
    let mut msg_t08 = msg_t02.clone();
    msg_t08.message_id = "m-t08".into();
    msg_t08.points.truncate(1);
    msg_t08.binding_revision_id = "br-oven-old".into();
    let err_t08 = IngestService::validate_telemetry(
        &conn,
        &gw_ctx,
        &msg_t08,
        50_100,
        cfg.max_future_skew_ms,
    )
    .unwrap_err();
    assert_eq!(err_t08.code, ErrorCode::RevisionConflict);

    // Valid batch -> append to AppWal
    let mut msg_ok1 = msg_t02.clone();
    msg_ok1.message_id = "m-ok-1".into();
    msg_ok1.points.truncate(1);
    let batch1 = IngestService::validate_telemetry(
        &conn,
        &gw_ctx,
        &msg_ok1,
        50_100,
        cfg.max_future_skew_ms,
    )
    .unwrap();

    let mut msg_ok2 = msg_ok1.clone();
    msg_ok2.message_id = "m-ok-2".into();
    msg_ok2.points[0].source_seq = "2".into();
    msg_ok2.points[0].value = json!(246.2);
    let batch2 = IngestService::validate_telemetry(
        &conn,
        &gw_ctx,
        &msg_ok2,
        50_200,
        cfg.max_future_skew_ms,
    )
    .unwrap();

    let mut wal = AppWal::open(&cfg).unwrap();
    let r1 = wal.append_durable(&batch1.to_wal_record("ing-001")).unwrap();
    let r2 = wal.append_durable(&batch2.to_wal_record("ing-002")).unwrap();
    assert!(r1.wal_position < r2.wal_position);

    // F02: Append incomplete tail bytes to WAL segment -> scan_and_recover truncates tail cleanly
    let seg_path = wal.active_segment_path();
    {
        let mut f = OpenOptions::new().append(true).open(&seg_path).unwrap();
        f.write_all(b"IWAL\x00\x01partial_tail_garbage").unwrap();
        f.sync_all().unwrap();
    }
    let scan_f02 = wal.scan_and_recover(None).unwrap();
    assert_eq!(
        scan_f02.outcome,
        WalRecoveryOutcome::CleanWithTruncatedTail {
            truncated_from_offset: scan_f02.valid_end_offset
        }
    );
    assert_eq!(scan_f02.records.len(), 2);

    // T07: Set hard watermark below current WAL size -> append_durable returns 503 DURABILITY_BACKPRESSURE
    cfg.wal_hard_watermark_bytes = 64;
    let mut wal_bp = AppWal::open(&cfg).unwrap();
    let bp_err = wal_bp
        .append_durable(&batch1.to_wal_record("ing-bp"))
        .unwrap_err();
    assert_eq!(bp_err.code, ErrorCode::DurabilityBackpressure);
    assert_eq!(bp_err.http_status(), 503);

    // F05: Corrupt a byte inside the first record's payload (while a second valid record follows it)
    // -> scan_and_recover must HALT with MiddleRecordCorrupted and NEVER silently skip record 1!
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&seg_path)
            .unwrap();
        f.seek(SeekFrom::Start(52)).unwrap();
        f.write_all(&[0xFF]).unwrap();
        f.sync_all().unwrap();
    }
    let scan_f05 = wal.scan_and_recover(None).unwrap();
    assert!(matches!(
        scan_f05.outcome,
        WalRecoveryOutcome::HaltedMiddleRecordCorrupted {
            corrupted_offset: 0,
            ..
        }
    ));
}
