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

    let p1 = WalPosition {
        segment_no: 1,
        record_offset: 9,
    };
    let p2 = WalPosition {
        segment_no: 1,
        record_offset: 10,
    };
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
