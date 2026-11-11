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
use iotfast_thing_engine::ingest::{SourcePointState, SourceStateEnvelope};
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::storage_duckdb::{DurabilityPreference, EngineRuntime};
use serde_json::json;

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
fn d07_executes_single_tx_ingest_deadband_out_of_order_r03_r08_and_wal_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();

    let m_ctx = mgmt_ctx();
    let gw_ctx = gateway_ctx();

    ModelCatalogService::create_product(
        rt.conn(),
        &m_ctx,
        CreateProductRequest {
            product_id: "prod-oven".into(),
            product_key: "reflow-oven".into(),
            name: "回流焊炉".into(),
        },
    )
    .unwrap();
    ModelCatalogService::create_model_draft(
        rt.conn(),
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "mr-oven-1".into(),
            product_id: "prod-oven".into(),
            model_version: "1.0.0".into(),
            properties: vec![ModelPropertyDraft {
                identifier: "zone1_temp".into(),
                name: "一区温度".into(),
                value_type: ValueType::Float64,
                unit_code: Some("Cel".into()),
                unit_symbol: Some("°C".into()),
                min_value: Some(-40.0),
                max_value: Some(400.0),
                required: true,
            }],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(rt.conn(), &m_ctx, "mr-oven-1", 5_000).unwrap();

    AssetUnsService::create_asset(
        rt.conn(),
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
        rt.conn(),
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

    BindingService::register_source_and_grant(
        rt.conn(),
        &m_ctx,
        "gateway-01",
        "dev-oven-02",
        10_000,
    )
    .unwrap();
    let b1 = BindingService::create_binding_draft(
        rt.conn(),
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
    let b1_ready =
        BindingService::validate_binding(rt.conn(), &m_ctx, &b1.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
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

    // 1. T01: Ingest 245.6 durable -> stored as 245.6 (no secondary factor * 0.1)
    let m1 = TelemetryEnvelope {
        spec: "iotfast.telemetry.v1".into(),
        message_id: "m-001".into(),
        source_id: "gateway-01".into(),
        source_epoch: "epoch-001".into(),
        device_id: "dev-oven-02".into(),
        model_revision_id: "mr-oven-1".into(),
        binding_revision_id: "br-oven-12".into(),
        points: vec![TelemetryPoint {
            property: "zone1_temp".into(),
            value: json!(245.6),
            source_at_ms: 100_000,
            source_time_origin: "gateway_sample".into(),
            source_seq: "999".into(),
            quality: TelemetryQuality {
                state: "good".into(),
                source_protocol: Some("modbus".into()),
                source_code: None,
            },
            delivery: TelemetryDelivery { replayed: false },
        }],
    };
    let r1 = rt
        .ingest_telemetry(&gw_ctx, m1.clone(), DurabilityPreference::Durable, 100_050)
        .unwrap();
    assert_eq!(r1.http_status, 202);
    assert_eq!(r1.status, "accepted_durable");
    assert_eq!(rt.history_count().unwrap(), 1);
    assert_eq!(
        rt.latest_f64("res-dev-oven-02-zone1-temp").unwrap(),
        Some(245.6)
    );

    // 2. T03 / SC04: Retry m-001 with replayed=true -> 200 duplicate, same ingest_id, history count stays 1
    let mut m1_retry = m1.clone();
    m1_retry.points[0].delivery.replayed = true;
    let r1_dup = rt
        .ingest_telemetry(&gw_ctx, m1_retry, DurabilityPreference::Durable, 100_100)
        .unwrap();
    assert_eq!(r1_dup.http_status, 200);
    assert_eq!(r1_dup.status, "duplicate");
    assert_eq!(r1_dup.ingest_id, r1.ingest_id);
    assert_eq!(rt.history_count().unwrap(), 1);

    // 3. T04 / SC05: Same message_id with modified value 245.7 -> 409 IDEMPOTENCY_CONFLICT
    let mut m1_tampered = m1.clone();
    m1_tampered.points[0].value = json!(245.7);
    let err_t04 = rt
        .ingest_telemetry(&gw_ctx, m1_tampered, DurabilityPreference::Durable, 100_150)
        .unwrap_err();
    assert_eq!(err_t04.code, ErrorCode::IdempotencyConflict);
    assert_eq!(err_t04.http_status(), 409);

    // 4. Deadband filtering (deadband = 0.5, heartbeat = 60_000):
    // 245.8 at 100_000 (source_seq = "1000" > "999", SC06) changes by 0.2 < 0.5
    // -> latest_value updates to 245.8, but history_observation stays at 1!
    let mut m2 = m1.clone();
    m2.message_id = "m-002".into();
    m2.points[0].value = json!(245.8);
    m2.points[0].source_seq = "1000".into();
    rt.ingest_telemetry(&gw_ctx, m2, DurabilityPreference::Durable, 100_200)
        .unwrap();
    assert_eq!(
        rt.latest_f64("res-dev-oven-02-zone1-temp").unwrap(),
        Some(245.8)
    );
    assert_eq!(rt.history_count().unwrap(), 1);

    // 5. T05: Out-of-order older measurement (source_at_ms = 90_000, value = 240.0, change = 5.6 >= 0.5)
    // -> writes to history_observation (count becomes 2), but does NOT overwrite newer latest_value (stays 245.8)!
    let mut m3_ooo = m1.clone();
    m3_ooo.message_id = "m-003-ooo".into();
    m3_ooo.points[0].source_at_ms = 90_000;
    m3_ooo.points[0].source_seq = "500".into();
    m3_ooo.points[0].value = json!(240.0);
    m3_ooo.points[0].delivery.replayed = true;
    rt.ingest_telemetry(&gw_ctx, m3_ooo, DurabilityPreference::Durable, 100_300)
        .unwrap();
    assert_eq!(rt.history_count().unwrap(), 2);
    assert_eq!(
        rt.latest_f64("res-dev-oven-02-zone1-temp").unwrap(),
        Some(245.8)
    );

    // 6. T06 / SC08 / R03: report_source_state with point error sets quality="bad" without altering latest value
    let evt = SourceStateEnvelope {
        spec: "iotfast.source_state.v1".into(),
        event_id: "evt-timeout-01".into(),
        source_id: "gateway-01".into(),
        source_epoch: "epoch-001".into(),
        device_id: "dev-oven-02".into(),
        binding_revision_id: "br-oven-12".into(),
        observed_at_ms: 105_000,
        connection_state: "disconnected".into(),
        point_states: vec![SourcePointState {
            property: "zone1_temp".into(),
            quality_state: "bad".into(),
            quality_reason: "modbus_read_timeout".into(),
            source_protocol: Some("modbus".into()),
            source_code: Some("ETIMEDOUT".into()),
        }],
    };
    let s_rec = rt
        .report_source_state(&gw_ctx, evt.clone(), 105_010)
        .unwrap();
    assert_eq!(s_rec.status, "accepted");
    let s_dup = rt.report_source_state(&gw_ctx, evt, 105_020).unwrap();
    assert_eq!(s_dup.status, "duplicate");
    assert_eq!(
        rt.latest_f64("res-dev-oven-02-zone1-temp").unwrap(),
        Some(245.8)
    );
    assert_eq!(rt.history_count().unwrap(), 2);

    // 7. F03 / F04 WAL crash recovery: Append a new valid record directly to AppWal (simulating crash after WAL fsync
    // before DuckDB commit), then call recover_from_wal() -> replays record into DuckDB idempotently!
    let mut m4_wal = m1.clone();
    m4_wal.message_id = "m-004-wal-replay".into();
    m4_wal.points[0].source_at_ms = 110_000;
    m4_wal.points[0].source_seq = "1001".into();
    m4_wal.points[0].value = json!(250.0);
    rt.simulate_wal_fsync_without_db_commit(&gw_ctx, &m4_wal, 110_050)
        .unwrap();
    assert_eq!(rt.history_count().unwrap(), 2);

    let summary = rt.recover_from_wal().unwrap();
    assert_eq!(summary.replayed_messages, 1);
    assert_eq!(rt.history_count().unwrap(), 3);
    assert_eq!(
        rt.latest_f64("res-dev-oven-02-zone1-temp").unwrap(),
        Some(250.0)
    );

    // Re-running recover_from_wal() after checkpoint advance replays 0 messages and produces 0 duplicates
    let summary2 = rt.recover_from_wal().unwrap();
    assert_eq!(summary2.replayed_messages, 0);
    assert_eq!(rt.history_count().unwrap(), 3);
}
