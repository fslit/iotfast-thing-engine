use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::d00::ValueType;
use iotfast_thing_engine::error::ErrorCode;
use iotfast_thing_engine::model_catalog::{
    CompatibilityKind, CreateModelDraftRequest, CreateProductRequest, ModelCatalogService,
    ModelPropertyDraft,
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
        allowed_actions: vec!["product:write".into(), "model:write".into()],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: Some("op-alice".into()),
    }
}

#[test]
fn d02_validates_publishes_freezes_and_diffs_model_revisions() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let conn = duckdb::Connection::open(cfg.db_path()).unwrap();
    init_core_schema(&conn).unwrap();

    let ctx = mgmt_ctx();

    let prod = ModelCatalogService::create_product(
        &conn,
        &ctx,
        CreateProductRequest {
            product_id: "prod-pump".into(),
            product_key: "circulation-pump".into(),
            name: "循环水泵".into(),
        },
    )
    .unwrap();
    assert_eq!(prod.product_key, "circulation-pump");

    let r1 = ModelCatalogService::create_model_draft(
        &conn,
        &ctx,
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
            ],
        },
    )
    .unwrap();
    assert_eq!(r1.status, "draft");

    let val_report =
        ModelCatalogService::validate_model(&conn, &ctx, "pump-model-r1", None).unwrap();
    assert!(val_report.valid);
    assert_eq!(val_report.compatibility, CompatibilityKind::Initial);

    let published =
        ModelCatalogService::publish_model(&conn, &ctx, "pump-model-r1", 10_000).unwrap();
    assert_eq!(published.status, "published");
    assert_eq!(published.published_at_ms, Some(10_000));

    // NB01: Re-publishing or mutating an already published revision must return 409 REVISION_CONFLICT
    let repub_err =
        ModelCatalogService::publish_model(&conn, &ctx, "pump-model-r1", 12_000).unwrap_err();
    assert_eq!(repub_err.code, ErrorCode::RevisionConflict);
    assert_eq!(repub_err.http_status(), 409);

    let overwrite_err = ModelCatalogService::create_model_draft(
        &conn,
        &ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r1".into(),
            product_id: "prod-pump".into(),
            model_version: "1.0.0".into(),
            properties: vec![],
        },
    )
    .unwrap_err();
    assert_eq!(overwrite_err.code, ErrorCode::RevisionConflict);

    // r2 adds an optional property -> CompatibleExtension
    ModelCatalogService::create_model_draft(
        &conn,
        &ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r2".into(),
            product_id: "prod-pump".into(),
            model_version: "1.1.0".into(),
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
                    identifier: "bearing_temp".into(),
                    name: "轴承温度".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("Cel".into()),
                    unit_symbol: Some("°C".into()),
                    min_value: Some(-20.0),
                    max_value: Some(150.0),
                    required: false,
                },
            ],
        },
    )
    .unwrap();
    let compat_r1_r2 =
        ModelCatalogService::compare_revisions(&conn, &ctx, "pump-model-r1", "pump-model-r2")
            .unwrap();
    assert_eq!(compat_r1_r2, CompatibilityKind::CompatibleExtension);

    // r3 changes outlet_pressure unit from MPa to kPa -> BreakingChange
    ModelCatalogService::create_model_draft(
        &conn,
        &ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r3".into(),
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
    let compat_r1_r3 =
        ModelCatalogService::compare_revisions(&conn, &ctx, "pump-model-r1", "pump-model-r3")
            .unwrap();
    assert_eq!(compat_r1_r3, CompatibilityKind::BreakingChange);
}
