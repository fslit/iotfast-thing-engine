use iotfast_thing_engine::asset_uns::{
    AssetUnsService, CreateAssetRequest, InstantiateDeviceRequest,
};
use iotfast_thing_engine::binding::{
    ActivateBindingRequest, BindingService, CreateBindingDraftRequest, GatewayBindingAck,
    PointBindingDraft,
};
use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::d00::ValueType;
use iotfast_thing_engine::error::ErrorCode;
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::schema::init_core_schema;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};

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

#[test]
fn d04_manages_binding_lifecycle_forbids_double_scaling_and_supersedes_old_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let conn = duckdb::Connection::open(cfg.db_path()).unwrap();
    init_core_schema(&conn).unwrap();

    let ctx = mgmt_ctx();

    ModelCatalogService::create_product(
        &conn,
        &ctx,
        CreateProductRequest {
            product_id: "prod-pump".into(),
            product_key: "circulation-pump".into(),
            name: "循环水泵".into(),
        },
    )
    .unwrap();
    ModelCatalogService::create_model_draft(
        &conn,
        &ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r1".into(),
            product_id: "prod-pump".into(),
            model_version: "1.0.0".into(),
            properties: vec![ModelPropertyDraft {
                identifier: "outlet_pressure".into(),
                name: "出口压力".into(),
                value_type: ValueType::Float64,
                unit_code: Some("MPa".into()),
                unit_symbol: Some("MPa".into()),
                min_value: Some(0.0),
                max_value: Some(2.5),
                required: true,
            }],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(&conn, &ctx, "pump-model-r1", 5_000).unwrap();

    AssetUnsService::create_asset(
        &conn,
        &ctx,
        CreateAssetRequest {
            asset_id: "asset-room-1".into(),
            parent_asset_id: None,
            slug: "room-1".into(),
            name: "一号站房".into(),
        },
    )
    .unwrap();
    AssetUnsService::instantiate_device(
        &conn,
        &ctx,
        InstantiateDeviceRequest {
            device_id: "pump-001".into(),
            device_key: "pump-001".into(),
            device_name: "1号泵".into(),
            product_id: "prod-pump".into(),
            model_revision_id: "pump-model-r1".into(),
            asset_id: "asset-room-1".into(),
        },
    )
    .unwrap();

    BindingService::register_source_and_grant(&conn, &ctx, "gateway-01", "pump-001", 10_000)
        .unwrap();

    // 1. Reject double scaling when normalization_owner = "gateway"
    let double_scale_err = BindingService::create_binding_draft(
        &conn,
        &ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "bind-bad-scale".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: Some(0.01),
            points: vec![],
        },
    )
    .unwrap_err();
    assert_eq!(double_scale_err.code, ErrorCode::ValidationFailed);

    // 2. Reject unit mismatch against model_item ("kPa" != "MPa")
    let unit_mismatch_err = BindingService::create_binding_draft(
        &conn,
        &ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "bind-bad-unit".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "outlet_pressure".into(),
                register_address: "40010".into(),
                factor: 0.01,
                unit_code: Some("kPa".into()),
                deadband: Some(0.02),
                heartbeat_ms: Some(60_000),
            }],
        },
    )
    .unwrap_err();
    assert_eq!(unit_mismatch_err.code, ErrorCode::ValidationFailed);

    // 3. Create valid binding draft r1 -> validate (ready) -> activate (active)
    let b1 = BindingService::create_binding_draft(
        &conn,
        &ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "outlet_pressure".into(),
                register_address: "40010".into(),
                factor: 0.01,
                unit_code: Some("MPa".into()),
                deadband: Some(0.02),
                heartbeat_ms: Some(60_000),
            }],
        },
    )
    .unwrap();
    assert_eq!(b1.status, "draft");

    let b1_ready = BindingService::validate_binding(&conn, &ctx, "pump-001-bind-r1").unwrap();
    assert_eq!(b1_ready.status, "ready");

    // Wrong expected_device_revision must fail with 409 REVISION_CONFLICT
    let wrong_rev_err = BindingService::activate_binding(
        &conn,
        &ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            expected_device_revision: 99,
            activated_at_ms: 20_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "pump-001-bind-r1".into(),
                config_hash: b1_ready.config_hash.clone(),
                acknowledged_at_ms: 19_900,
            },
        },
    )
    .unwrap_err();
    assert_eq!(wrong_rev_err.code, ErrorCode::RevisionConflict);

    let b1_active = BindingService::activate_binding(
        &conn,
        &ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            expected_device_revision: 1,
            activated_at_ms: 20_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "pump-001-bind-r1".into(),
                config_hash: b1_ready.config_hash.clone(),
                acknowledged_at_ms: 19_900,
            },
        },
    )
    .unwrap();
    assert_eq!(b1_active.status, "active");

    // 4. Create r2 and activate it -> r1 must become "superseded" with valid_to_ms = 30_000
    let b2 = BindingService::create_binding_draft(
        &conn,
        &ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "pump-001-bind-r2".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "outlet_pressure".into(),
                register_address: "40012".into(),
                factor: 0.01,
                unit_code: Some("MPa".into()),
                deadband: Some(0.05),
                heartbeat_ms: Some(30_000),
            }],
        },
    )
    .unwrap();
    let b2_ready = BindingService::validate_binding(&conn, &ctx, &b2.binding_revision_id).unwrap();
    let b2_active = BindingService::activate_binding(
        &conn,
        &ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r2".into(),
            expected_device_revision: 2,
            activated_at_ms: 30_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "pump-001-bind-r2".into(),
                config_hash: b2_ready.config_hash.clone(),
                acknowledged_at_ms: 29_950,
            },
        },
    )
    .unwrap();
    assert_eq!(b2_active.status, "active");

    let b1_after = BindingService::get_binding(&conn, &mgmt_ctx(), "pump-001-bind-r1").unwrap();
    assert_eq!(b1_after.status, "superseded");
    assert_eq!(b1_after.valid_from_ms, Some(20_000));
    assert_eq!(b1_after.valid_to_ms, Some(30_000));
}
