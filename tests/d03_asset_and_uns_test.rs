use iotfast_thing_engine::asset_uns::{
    AssetUnsService, CreateAssetRequest, InstantiateDeviceRequest,
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
            "catalog:read".into(),
        ],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: Some("op-alice".into()),
    }
}

#[test]
fn d03_instantiates_devices_preserves_stable_ids_on_relocation_and_protects_redirect_uri() {
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

    // Create asset tree: plant-a -> room-1, room-2
    let plant_a = AssetUnsService::create_asset(
        &conn,
        &ctx,
        CreateAssetRequest {
            asset_id: "asset-plant-a".into(),
            parent_asset_id: None,
            slug: "plant-a".into(),
            name: "一厂区".into(),
        },
    )
    .unwrap();
    assert_eq!(plant_a.asset_path, "plant-a");

    let room_1 = AssetUnsService::create_asset(
        &conn,
        &ctx,
        CreateAssetRequest {
            asset_id: "asset-room-1".into(),
            parent_asset_id: Some("asset-plant-a".into()),
            slug: "room-1".into(),
            name: "一号站房".into(),
        },
    )
    .unwrap();
    assert_eq!(room_1.asset_path, "plant-a/room-1");

    // Duplicate sibling slug under plant-a must be rejected
    let dup_slug_err = AssetUnsService::create_asset(
        &conn,
        &ctx,
        CreateAssetRequest {
            asset_id: "asset-room-1-dup".into(),
            parent_asset_id: Some("asset-plant-a".into()),
            slug: "room-1".into(),
            name: "重复站房".into(),
        },
    )
    .unwrap_err();
    assert_eq!(dup_slug_err.code, ErrorCode::RevisionConflict);

    let room_2 = AssetUnsService::create_asset(
        &conn,
        &ctx,
        CreateAssetRequest {
            asset_id: "asset-room-2".into(),
            parent_asset_id: Some("asset-plant-a".into()),
            slug: "room-2".into(),
            name: "二号站房".into(),
        },
    )
    .unwrap();
    assert_eq!(room_2.asset_path, "plant-a/room-2");

    // Instantiate pump-001 and pump-002 from same model (BM01 / AT01)
    let p1 = AssetUnsService::instantiate_device(
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
    let p2 = AssetUnsService::instantiate_device(
        &conn,
        &ctx,
        InstantiateDeviceRequest {
            device_id: "pump-002".into(),
            device_key: "pump-002".into(),
            device_name: "2号泵".into(),
            product_id: "prod-pump".into(),
            model_revision_id: "pump-model-r1".into(),
            asset_id: "asset-room-1".into(),
        },
    )
    .unwrap();
    assert_ne!(p1.resources[0].resource_id, p2.resources[0].resource_id);
    assert_eq!(
        p1.resources[0].canonical_uri,
        "uns://ten-8iic/plant-a/room-1/pump-001/properties/outlet_pressure"
    );

    // Relocate pump-001 from room-1 to room-2 (NB02)
    let old_uri = p1.resources[0].canonical_uri.clone();
    let orig_res_id = p1.resources[0].resource_id.clone();

    let paths = AssetUnsService::relocate_device(
        &conn,
        &ctx,
        "pump-001",
        "asset-room-2",
        20_000,
        86_400_000,
    )
    .unwrap();
    assert_eq!(paths.records.len(), 1);
    let new_uri = "uns://ten-8iic/plant-a/room-2/pump-001/properties/outlet_pressure";
    assert_eq!(paths.records[0].canonical_uri, new_uri);
    assert_eq!(paths.records[0].resource_id, orig_res_id);

    // Resolving old URI within retention window returns matched_path_state = "redirected"
    let resolved_old =
        AssetUnsService::resolve_resource_by_uri(&conn, &ctx, &old_uri, 30_000).unwrap();
    assert_eq!(resolved_old.resource_id, orig_res_id);
    assert_eq!(resolved_old.matched_path_state, "redirected");
    assert_eq!(resolved_old.canonical_uri, new_uri);

    // Attempting to instantiate another device in room-1 that would hijack the redirecting URI
    // before redirect_until_ms must be rejected with 409 REVISION_CONFLICT!
    let hijack_err = AssetUnsService::instantiate_device(
        &conn,
        &ctx,
        InstantiateDeviceRequest {
            device_id: "pump-hijack".into(),
            device_key: "pump-001".into(),
            device_name: "抢占路径泵".into(),
            product_id: "prod-pump".into(),
            model_revision_id: "pump-model-r1".into(),
            asset_id: "asset-room-1".into(),
        },
    )
    .unwrap_err();
    assert_eq!(hijack_err.code, ErrorCode::RevisionConflict);
}
