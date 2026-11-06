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
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::northbound::{AggregationSpec, HistoryQueryRequest, TimeRange};
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::storage_duckdb::{DurabilityPreference, EngineRuntime};
use iotfast_thing_engine::subscription::{SubscribeRequest, SubscriptionEvent, SubscriptionHub};
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
        caller_kind: CallerKind::SouthboundSource,
        caller_service_id: "gateway-01".into(),
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter::default(),
        operator_id: None,
    }
}

fn line01_reader_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_service_id: "svc-scada-line01".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec![
            "view:read".into(),
            "shadow:read".into(),
            "history:query".into(),
            "subscription:connect".into(),
        ],
        scope_filter: ScopeFilter {
            asset_ids: vec!["asset-line-01".into()],
            include_asset_descendants: true,
            device_ids: vec![],
            resource_ids: vec![],
            uri_prefixes: vec!["uns://ten-8iic/line-01/".into()],
            tenant_wide: false,
        },
        operator_id: None,
    }
}

#[test]
fn d08_verifies_view_shadow_scope_r09_history_r07_nb07_to_nb10_and_subscription_s_handoff() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut rt = EngineRuntime::open(cfg).unwrap();

    let m_ctx = mgmt_ctx();
    let gw_ctx = gateway_ctx();
    let nb_ctx = line01_reader_ctx();

    // 1. Setup product, model r1 (MPa) & r2 (kPa), assets line-01 and line-02, devices pump-001 (line-01) and pump-b01 (line-02)
    ModelCatalogService::create_product(
        rt.conn(),
        &m_ctx,
        CreateProductRequest {
            product_id: "prod-pump".into(),
            product_key: "circulation-pump".into(),
            name: "循环水泵".into(),
        },
    )
    .unwrap();
    ModelCatalogService::create_model_draft(
        rt.conn(),
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r1".into(),
            product_id: "prod-pump".into(),
            model_version: "1.0.0".into(),
            properties: vec![
                ModelPropertyDraft {
                    identifier: "running_state".into(),
                    name: "运行状态".into(),
                    value_type: ValueType::Bool,
                    unit_code: None,
                    unit_symbol: None,
                    min_value: None,
                    max_value: None,
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "outlet_pressure".into(),
                    name: "出口压力".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("MPa".into()),
                    unit_symbol: Some("MPa".into()),
                    min_value: Some(0.0),
                    max_value: Some(2.5),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "maintenance_note".into(),
                    name: "维护备注".into(),
                    value_type: ValueType::Text,
                    unit_code: None,
                    unit_symbol: None,
                    min_value: None,
                    max_value: None,
                    required: false,
                },
            ],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(rt.conn(), &m_ctx, "pump-model-r1", 2_000).unwrap();

    ModelCatalogService::create_model_draft(
        rt.conn(),
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r2-kpa".into(),
            product_id: "prod-pump".into(),
            model_version: "2.0.0".into(),
            properties: vec![
                ModelPropertyDraft {
                    identifier: "running_state".into(),
                    name: "运行状态".into(),
                    value_type: ValueType::Bool,
                    unit_code: None,
                    unit_symbol: None,
                    min_value: None,
                    max_value: None,
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "outlet_pressure".into(),
                    name: "出口压力".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("kPa".into()),
                    unit_symbol: Some("kPa".into()),
                    min_value: Some(0.0),
                    max_value: Some(2500.0),
                    required: true,
                },
            ],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(rt.conn(), &m_ctx, "pump-model-r2-kpa", 3_000).unwrap();

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
    AssetUnsService::create_asset(
        rt.conn(),
        &m_ctx,
        CreateAssetRequest {
            asset_id: "asset-line-02".into(),
            parent_asset_id: None,
            slug: "line-02".into(),
            name: "二产线".into(),
        },
    )
    .unwrap();

    AssetUnsService::instantiate_device(
        rt.conn(),
        &m_ctx,
        InstantiateDeviceRequest {
            device_id: "pump-001".into(),
            device_key: "pump-001".into(),
            device_name: "1号泵".into(),
            product_id: "prod-pump".into(),
            model_revision_id: "pump-model-r1".into(),
            asset_id: "asset-line-01".into(),
        },
    )
    .unwrap();
    AssetUnsService::instantiate_device(
        rt.conn(),
        &m_ctx,
        InstantiateDeviceRequest {
            device_id: "pump-b01".into(),
            device_key: "pump-b01".into(),
            device_name: "二线泵".into(),
            product_id: "prod-pump".into(),
            model_revision_id: "pump-model-r1".into(),
            asset_id: "asset-line-02".into(),
        },
    )
    .unwrap();

    BindingService::register_source_and_grant(rt.conn(), &m_ctx, "gateway-01", "pump-001", 5_000)
        .unwrap();
    let b1 = BindingService::create_binding_draft(
        rt.conn(),
        &m_ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![
                PointBindingDraft {
                    property: "running_state".into(),
                    register_address: "00001".into(),
                    factor: 1.0,
                    unit_code: None,
                    deadband: None,
                    heartbeat_ms: None,
                },
                PointBindingDraft {
                    property: "outlet_pressure".into(),
                    register_address: "40010".into(),
                    factor: 0.01,
                    unit_code: Some("MPa".into()),
                    deadband: None,
                    heartbeat_ms: None,
                },
            ],
        },
    )
    .unwrap();
    let b1_ready =
        BindingService::validate_binding(rt.conn(), &m_ctx, &b1.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
        &m_ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            expected_device_revision: 1,
            activated_at_ms: 10_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "pump-001-bind-r1".into(),
                config_hash: b1_ready.config_hash,
                acknowledged_at_ms: 9_900,
            },
        },
    )
    .unwrap();

    // 2. NB03: Before any telemetry arrives, bound property is no_observation + null + unknown,
    // and unbound property (maintenance_note) is unbound + null + unknown.
    let v0 = rt.get_device_view(&nb_ctx, "pump-001", 20_000).unwrap();
    assert_eq!(v0.properties["outlet_pressure"].binding_state, "bound");
    assert_eq!(v0.properties["outlet_pressure"].observation_state, "no_observation");
    assert_eq!(v0.properties["outlet_pressure"].value, json!(null));
    assert_eq!(v0.properties["outlet_pressure"].quality, "unknown");
    assert_eq!(v0.properties["maintenance_note"].binding_state, "unbound");

    // SC02 / T10: Querying pump-b01 (in line-02) with line-01 Scope returns 404 RESOURCE_NOT_FOUND
    let err_404 = rt.get_device_view(&nb_ctx, "pump-b01", 20_000).unwrap_err();
    assert_eq!(err_404.code, ErrorCode::ResourceNotFound);
    assert_eq!(err_404.http_status(), 404);

    // 3. Ingest 3 points for history query & view verification:
    // - 100_000: 0.41 MPa (msg-h1)
    // - 101_000: 0.42 MPa (msg-h2-a)
    // - 101_000: 0.43 MPa (msg-h2-b, same millisecond as msg-h2-a!)
    // - 102_000: 0.44 MPa (msg-h3, at end_ms boundary)
    for (mid, ts, seq, val) in [
        ("msg-h1", 100_000i64, "1", 0.41f64),
        ("msg-h2-a", 101_000i64, "2", 0.42f64),
        ("msg-h2-b", 101_000i64, "3", 0.43f64),
        ("msg-h3", 102_000i64, "4", 0.44f64),
    ] {
        rt.ingest_telemetry(
            &gw_ctx,
            TelemetryEnvelope {
                spec: "iotfast.telemetry.v1".into(),
                message_id: mid.into(),
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                device_id: "pump-001".into(),
                model_revision_id: "pump-model-r1".into(),
                binding_revision_id: "pump-001-bind-r1".into(),
                points: vec![TelemetryPoint {
                    property: "outlet_pressure".into(),
                    value: json!(val),
                    source_at_ms: ts,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: seq.into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                }],
            },
            DurabilityPreference::Durable,
            ts + 10,
        )
        .unwrap();
    }

    // 4. R09: Partial-device caller only authorized for `res-pump-001-outlet-pressure`
    // calling get_device_view("pump-001") only sees `outlet_pressure` (not `running_state` or `maintenance_note`)
    let partial_ctx = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_service_id: "svc-pressure-widget".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec!["view:read".into(), "shadow:read".into(), "history:query".into()],
        scope_filter: ScopeFilter {
            resource_ids: vec!["res-pump-001-outlet-pressure".into()],
            ..Default::default()
        },
        operator_id: None,
    };
    let partial_view = rt.get_device_view(&partial_ctx, "pump-001", 103_000).unwrap();
    assert_eq!(partial_view.properties.len(), 1);
    assert_eq!(partial_view.properties["outlet_pressure"].value, json!(0.44));

    // R05: Batch history query mixing authorized + unauthorized resource_ids returns 404 with empty details
    let batch_err = rt
        .query_history(
            &partial_ctx,
            HistoryQueryRequest {
                resource_ids: vec![
                    "res-pump-001-outlet-pressure".into(),
                    "res-pump-001-running-state".into(),
                ],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 102_000,
                },
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 10,
                cursor: None,
            },
        )
        .unwrap_err();
    assert_eq!(batch_err.code, ErrorCode::ResourceNotFound);
    assert!(batch_err.details.is_empty());

    // 5. NB07 & NB09 & R07: Query half-open range [100_000, 102_000) with limit=2 -> returns 2 items on page 1,
    // 1 item on page 2 (excludes 102_000), and coverage separates contiguous_wal_position vs event_time_complete_through_ms=None.
    let page1 = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 102_000,
                },
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 2,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.coverage.contiguous_wal_position.is_some());
    assert_eq!(page1.coverage.event_time_complete_through_ms, None);

    let page2 = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 102_000,
                },
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 2,
                cursor: page1.next_cursor.clone(),
            },
        )
        .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.next_cursor, None);

    // NB09: Replaying page1.next_cursor under `partial_ctx` (different scope_hash) fails with 422 VALIDATION_FAILED
    let cursor_err = rt
        .query_history(
            &partial_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 102_000,
                },
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 2,
                cursor: page1.next_cursor.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(cursor_err.code, ErrorCode::ValidationFailed);

    // 6. NB05 / NB06 / T09 / R04: Subscription snapshot at boundary S + delta(> S) + overflow resync_required
    let mut hub = SubscriptionHub::new();
    let sub_id = hub
        .subscribe(
            &nb_ctx,
            SubscribeRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                include_initial_snapshot: true,
                max_pending_events: 2,
            },
            &rt,
            103_000,
        )
        .unwrap();

    let initial_events = hub.drain_events(&sub_id);
    assert_eq!(initial_events.len(), 3); // SnapshotBegin, SnapshotChunk(seq_r <= S), SnapshotEnd(S)
    let boundary_s = match &initial_events[2] {
        SubscriptionEvent::SnapshotEnd { boundary_seq } => *boundary_seq,
        other => panic!("Expected SnapshotEnd, got {:?}", other),
    };

    // Push 1 delta (> S) -> delivered in order with stream_seq == boundary_s + 1
    hub.publish_point_delta(
        "ten-8iic",
        "res-pump-001-outlet-pressure",
        boundary_s + 1,
        json!(0.45),
        "good",
        104_000,
    );
    let delta_evts = hub.drain_events(&sub_id);
    assert_eq!(delta_evts.len(), 1);
    assert!(matches!(
        &delta_evts[0],
        SubscriptionEvent::Delta { stream_seq, .. } if *stream_seq == boundary_s + 1
    ));

    // Push 3 deltas without draining (max_pending_events = 2) -> triggers ResyncRequired(BUFFER_OVERFLOW)
    for seq in (boundary_s + 2)..=(boundary_s + 4) {
        hub.publish_point_delta(
            "ten-8iic",
            "res-pump-001-outlet-pressure",
            seq,
            json!(0.46),
            "good",
            105_000,
        );
    }
    let overflow_evts = hub.drain_events(&sub_id);
    assert_eq!(overflow_evts.len(), 1);
    assert!(matches!(
        &overflow_evts[0],
        SubscriptionEvent::ResyncRequired { code, .. } if code == "BUFFER_OVERFLOW"
    ));

    // 7. NB08: Activate r2 (kPa), ingest 450.0 kPa at 110_000, and query aggregate across MPa + kPa
    // -> must reject with 422 CROSS_VERSION_INCOMPATIBLE
    let b2 = BindingService::create_binding_draft(
        rt.conn(),
        &m_ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "pump-001-bind-r2".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r2-kpa".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "outlet_pressure".into(),
                register_address: "40010".into(),
                factor: 10.0,
                unit_code: Some("kPa".into()),
                deadband: None,
                heartbeat_ms: None,
            }],
        },
    )
    .unwrap();
    let b2_ready =
        BindingService::validate_binding(rt.conn(), &m_ctx, &b2.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
        &m_ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r2".into(),
            expected_device_revision: 2,
            activated_at_ms: 108_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "pump-001-bind-r2".into(),
                config_hash: b2_ready.config_hash,
                acknowledged_at_ms: 107_900,
            },
        },
    )
    .unwrap();

    rt.ingest_telemetry(
        &gw_ctx,
        TelemetryEnvelope {
            spec: "iotfast.telemetry.v1".into(),
            message_id: "msg-kpa-1".into(),
            source_id: "gateway-01".into(),
            source_epoch: "epoch-001".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r2-kpa".into(),
            binding_revision_id: "pump-001-bind-r2".into(),
            points: vec![TelemetryPoint {
                property: "outlet_pressure".into(),
                value: json!(450.0),
                source_at_ms: 110_000,
                source_time_origin: "gateway_sample".into(),
                source_seq: "5".into(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            }],
        },
        DurabilityPreference::Durable,
        110_020,
    )
    .unwrap();

    let agg_err = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 120_000,
                },
                mode: "aggregate".into(),
                aggregation: Some(AggregationSpec {
                    bucket_ms: 60_000,
                    functions: vec!["avg".into()],
                }),
                quality_states: vec![],
                order: "asc".into(),
                limit: 50,
                cursor: None,
            },
        )
        .unwrap_err();
    assert_eq!(agg_err.code, ErrorCode::CrossVersionIncompatible);
    assert_eq!(agg_err.http_status(), 422);
}
