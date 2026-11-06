use f1_d00_contract::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Helper to build a pre-configured `ContractVerificationEngine` with:
/// - Tenant `ten-8iic`
/// - Assets:
///   - `asset-plant-a` (`uns://8iic/plant-a`)
///   - `asset-line-01` (child of `asset-plant-a`, `uns://8iic/plant-a/line-01`)
///   - `asset-line-02` (`uns://8iic/plant-b/line-02`)
///   - `asset-plant-ab` (`uns://8iic/plant-ab`)
/// - Devices:
///   - `dev-oven-02` under `asset-line-01` (Model `mr-oven-1`, Binding `br-oven-12`, factor=0.1)
///   - `pump-001` under `asset-line-01` (Model `pump-model-r1`, Binding `pump-001-bind-r1`)
///   - `pump-b01` under `asset-line-02`
///   - `pump-ab01` under `asset-plant-ab` (`uns://8iic/plant-ab/pump-01`)
fn setup_fixture_engine() -> ContractVerificationEngine {
    let mut engine = ContractVerificationEngine::new();

    // Register assets
    engine.register_asset(AssetNode {
        asset_id: "asset-plant-a".to_string(),
        tenant_id: "ten-8iic".to_string(),
        parent_asset_id: None,
        slug: "plant-a".to_string(),
        asset_path: "plant-a".to_string(),
    });
    engine.register_asset(AssetNode {
        asset_id: "asset-line-01".to_string(),
        tenant_id: "ten-8iic".to_string(),
        parent_asset_id: Some("asset-plant-a".to_string()),
        slug: "line-01".to_string(),
        asset_path: "plant-a/line-01".to_string(),
    });
    engine.register_asset(AssetNode {
        asset_id: "asset-line-02".to_string(),
        tenant_id: "ten-8iic".to_string(),
        parent_asset_id: None,
        slug: "line-02".to_string(),
        asset_path: "plant-b/line-02".to_string(),
    });
    engine.register_asset(AssetNode {
        asset_id: "asset-plant-ab".to_string(),
        tenant_id: "ten-8iic".to_string(),
        parent_asset_id: None,
        slug: "plant-ab".to_string(),
        asset_path: "plant-ab".to_string(),
    });

    // Register oven model `mr-oven-1`
    let mut oven_props = BTreeMap::new();
    oven_props.insert(
        "zone1_temp".to_string(),
        PropertySpec {
            identifier: "zone1_temp".to_string(),
            name: "一温区实际温度".to_string(),
            data_type: ValueType::Float64,
            unit: Some(UnitSpec {
                code: "Cel".to_string(),
                symbol: "°C".to_string(),
            }),
            min_value: Some(-50.0),
            max_value: Some(500.0),
            source_kind: PropertySourceKind::GatewayMeasurement,
        },
    );
    oven_props.insert(
        "zone2_temp".to_string(),
        PropertySpec {
            identifier: "zone2_temp".to_string(),
            name: "二温区实际温度".to_string(),
            data_type: ValueType::Float64,
            unit: Some(UnitSpec {
                code: "Cel".to_string(),
                symbol: "°C".to_string(),
            }),
            min_value: Some(-50.0),
            max_value: Some(500.0),
            source_kind: PropertySourceKind::GatewayMeasurement,
        },
    );
    engine.register_model(ModelRevision {
        model_revision_id: "mr-oven-1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        product_id: "prod-oven".to_string(),
        product_key: "reflow-oven".to_string(),
        model_version: "1.0.0".to_string(),
        published: true,
        properties: oven_props,
    });

    // Register binding `br-oven-12` (factor: 0.1, normalization_owner: "gateway")
    let mut oven_mapped = BTreeMap::new();
    oven_mapped.insert(
        "zone1_temp".to_string(),
        BindingPointMapping {
            property: "zone1_temp".to_string(),
            register_address: "40001".to_string(),
            factor: 0.1,
            unit: Some(UnitSpec {
                code: "Cel".to_string(),
                symbol: "°C".to_string(),
            }),
        },
    );
    oven_mapped.insert(
        "zone2_temp".to_string(),
        BindingPointMapping {
            property: "zone2_temp".to_string(),
            register_address: "40002".to_string(),
            factor: 0.1,
            unit: Some(UnitSpec {
                code: "Cel".to_string(),
                symbol: "°C".to_string(),
            }),
        },
    );
    engine
        .validate_and_register_binding(
            "req-init-bind-oven",
            BindingRevision {
                binding_revision_id: "br-oven-12".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "dev-oven-02".to_string(),
                model_revision_id: "mr-oven-1".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: None,
                active: true,
                valid_from_ms: Some(1793800000000),
                valid_to_ms: None,
                mapped_points: oven_mapped.clone(),
            },
        )
        .unwrap();

    // Also register a newer binding `br-oven-13` for T08 revision conflict testing
    engine
        .validate_and_register_binding(
            "req-init-bind-oven-13",
            BindingRevision {
                binding_revision_id: "br-oven-13".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "dev-oven-02".to_string(),
                model_revision_id: "mr-oven-1".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: None,
                active: false,
                valid_from_ms: Some(1793870000000),
                valid_to_ms: None,
                mapped_points: oven_mapped,
            },
        )
        .unwrap();

    engine.instantiate_device(DeviceInstance {
        device_id: "dev-oven-02".to_string(),
        tenant_id: "ten-8iic".to_string(),
        device_key: "oven-02".to_string(),
        device_name: "2号回流焊炉".to_string(),
        product_id: "prod-oven".to_string(),
        product_key: "reflow-oven".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        active_binding_revision_id: "br-oven-12".to_string(),
        asset_id: "asset-line-01".to_string(),
        canonical_device_uri: "uns://8iic/plant-a/line-01/oven-02".to_string(),
        revision: 1,
    });

    // Register pump model r1 (`MPa`) and r0 (`kPa` for NB08 cross-version test)
    let mut pump_props_r1 = BTreeMap::new();
    pump_props_r1.insert(
        "running_state".to_string(),
        PropertySpec {
            identifier: "running_state".to_string(),
            name: "运行状态".to_string(),
            data_type: ValueType::Bool,
            unit: None,
            min_value: None,
            max_value: None,
            source_kind: PropertySourceKind::GatewayMeasurement,
        },
    );
    pump_props_r1.insert(
        "outlet_pressure".to_string(),
        PropertySpec {
            identifier: "outlet_pressure".to_string(),
            name: "出口压力".to_string(),
            data_type: ValueType::Float64,
            unit: Some(UnitSpec {
                code: "MPa".to_string(),
                symbol: "MPa".to_string(),
            }),
            min_value: Some(0.0),
            max_value: Some(10.0),
            source_kind: PropertySourceKind::GatewayMeasurement,
        },
    );
    pump_props_r1.insert(
        "maintenance_note".to_string(),
        PropertySpec {
            identifier: "maintenance_note".to_string(),
            name: "维护备注".to_string(),
            data_type: ValueType::Text,
            unit: None,
            min_value: None,
            max_value: None,
            source_kind: PropertySourceKind::Unbound,
        },
    );
    engine.register_model(ModelRevision {
        model_revision_id: "pump-model-r1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        product_id: "prod-pump".to_string(),
        product_key: "circulation-pump".to_string(),
        model_version: "1.0.0".to_string(),
        published: true,
        properties: pump_props_r1.clone(),
    });

    let mut pump_props_r2_kpa = pump_props_r1;
    pump_props_r2_kpa.get_mut("outlet_pressure").unwrap().unit = Some(UnitSpec {
        code: "kPa".to_string(),
        symbol: "kPa".to_string(),
    });
    pump_props_r2_kpa
        .get_mut("outlet_pressure")
        .unwrap()
        .max_value = Some(10000.0);
    engine.register_model(ModelRevision {
        model_revision_id: "pump-model-r2-kpa".to_string(),
        tenant_id: "ten-8iic".to_string(),
        product_id: "prod-pump".to_string(),
        product_key: "circulation-pump".to_string(),
        model_version: "2.0.0".to_string(),
        published: true,
        properties: pump_props_r2_kpa,
    });

    let mut pump_mapped = BTreeMap::new();
    pump_mapped.insert(
        "running_state".to_string(),
        BindingPointMapping {
            property: "running_state".to_string(),
            register_address: "00001".to_string(),
            factor: 1.0,
            unit: None,
        },
    );
    pump_mapped.insert(
        "outlet_pressure".to_string(),
        BindingPointMapping {
            property: "outlet_pressure".to_string(),
            register_address: "40010".to_string(),
            factor: 0.01,
            unit: Some(UnitSpec {
                code: "MPa".to_string(),
                symbol: "MPa".to_string(),
            }),
        },
    );
    engine
        .validate_and_register_binding(
            "req-init-bind-pump",
            BindingRevision {
                binding_revision_id: "pump-001-bind-r1".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "pump-001".to_string(),
                model_revision_id: "pump-model-r1".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: None,
                active: true,
                valid_from_ms: Some(1793800000000),
                valid_to_ms: None,
                mapped_points: pump_mapped.clone(),
            },
        )
        .unwrap();

    // Binding for r2-kpa
    let mut pump_mapped_r2 = pump_mapped.clone();
    pump_mapped_r2.get_mut("outlet_pressure").unwrap().unit = Some(UnitSpec {
        code: "kPa".to_string(),
        symbol: "kPa".to_string(),
    });
    engine
        .validate_and_register_binding(
            "req-init-bind-pump-r2",
            BindingRevision {
                binding_revision_id: "pump-001-bind-r2".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "pump-001".to_string(),
                model_revision_id: "pump-model-r2-kpa".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: None,
                active: false,
                valid_from_ms: Some(1793867000000),
                valid_to_ms: None,
                mapped_points: pump_mapped_r2,
            },
        )
        .unwrap();

    engine.instantiate_device(DeviceInstance {
        device_id: "pump-001".to_string(),
        tenant_id: "ten-8iic".to_string(),
        device_key: "pump-001".to_string(),
        device_name: "1号循环水泵".to_string(),
        product_id: "prod-pump".to_string(),
        product_key: "circulation-pump".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        active_binding_revision_id: "pump-001-bind-r1".to_string(),
        asset_id: "asset-line-01".to_string(),
        canonical_device_uri: "uns://8iic/plant-a/room-1/pump-001".to_string(),
        revision: 1,
    });

    // Instantiate `pump-b01` under `asset-line-02` (Plant B) for SC02 / T10
    engine
        .validate_and_register_binding(
            "req-init-bind-pump-b01",
            BindingRevision {
                binding_revision_id: "pump-b01-bind-r1".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "pump-b01".to_string(),
                model_revision_id: "pump-model-r1".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: None,
                active: true,
                valid_from_ms: Some(1793800000000),
                valid_to_ms: None,
                mapped_points: pump_mapped.clone(),
            },
        )
        .unwrap();
    engine.instantiate_device(DeviceInstance {
        device_id: "pump-b01".to_string(),
        tenant_id: "ten-8iic".to_string(),
        device_key: "pump-b01".to_string(),
        device_name: "二线循环水泵".to_string(),
        product_id: "prod-pump".to_string(),
        product_key: "circulation-pump".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        active_binding_revision_id: "pump-b01-bind-r1".to_string(),
        asset_id: "asset-line-02".to_string(),
        canonical_device_uri: "uns://8iic/plant-b/line-02/pump-b01".to_string(),
        revision: 1,
    });

    // Instantiate `pump-ab01` under `asset-plant-ab` (`uns://8iic/plant-ab/pump-01`) for SC03 URI boundary test
    engine
        .validate_and_register_binding(
            "req-init-bind-pump-ab01",
            BindingRevision {
                binding_revision_id: "pump-ab01-bind-r1".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "pump-ab01".to_string(),
                model_revision_id: "pump-model-r1".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: None,
                active: true,
                valid_from_ms: Some(1793800000000),
                valid_to_ms: None,
                mapped_points: pump_mapped,
            },
        )
        .unwrap();
    engine.instantiate_device(DeviceInstance {
        device_id: "pump-ab01".to_string(),
        tenant_id: "ten-8iic".to_string(),
        device_key: "pump-01".to_string(),
        device_name: "AB厂区水泵".to_string(),
        product_id: "prod-pump".to_string(),
        product_key: "circulation-pump".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        active_binding_revision_id: "pump-ab01-bind-r1".to_string(),
        asset_id: "asset-plant-ab".to_string(),
        canonical_device_uri: "uns://8iic/plant-ab/pump-01".to_string(),
        revision: 1,
    });

    // Grant gateway-01 access to dev-oven-02 and pump-001
    engine.grant_source_device("ten-8iic", "gateway-01", "dev-oven-02");
    engine.grant_source_device("ten-8iic", "gateway-01", "pump-001");

    engine
}

fn gateway_scope_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        spec: "iotfast.scope_context.v1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        project_id: Some("proj-smt-01".to_string()),
        caller_service_id: "svc-gateway-01".to_string(),
        caller_kind: CallerKind::SouthboundSource,
        bound_source_id: Some("gateway-01".to_string()),
        issued_at_ms: 1793869000000,
        expires_at_ms: 1793879000000,
        allowed_actions: vec![
            "telemetry:ingest".to_string(),
            "source_state:report".to_string(),
            "binding:ack".to_string(),
        ],
        scope_filter: ScopeFilter::default(),
        quota_profile: "southbound_default".to_string(),
    }
}

fn northbound_line01_scope_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        spec: "iotfast.scope_context.v1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        project_id: Some("proj-smt-01".to_string()),
        caller_service_id: "svc-scada-line01".to_string(),
        caller_kind: CallerKind::NorthboundService,
        bound_source_id: None,
        issued_at_ms: 1793869000000,
        expires_at_ms: 1793879000000,
        allowed_actions: vec![
            "catalog:read".to_string(),
            "view:read".to_string(),
            "shadow:read".to_string(),
            "subscription:connect".to_string(),
            "history:query".to_string(),
        ],
        scope_filter: ScopeFilter {
            asset_ids: vec!["asset-plant-a".to_string()],
            include_asset_descendants: true,
            device_ids: vec![],
            resource_ids: vec![],
            uri_prefixes: vec!["uns://8iic/plant-a/".to_string()],
            tenant_wide: false,
        },
        quota_profile: "standard_northbound".to_string(),
    }
}

// ============================================================================
// Test 1: D00 Identifier Formats & WAL Position Ordering
// ============================================================================

#[test]
fn test_d00_identifier_formats_and_wal_position() {
    assert!(validate_entity_id("ten-8iic"));
    assert!(validate_entity_id("dev-oven-02"));
    assert!(!validate_entity_id("ab")); // too short (< 3)
    assert!(!validate_entity_id("-bad-start"));
    assert!(!validate_entity_id("BadUpperCase"));

    assert!(validate_message_id("m-20261105-000001"));
    assert!(!validate_message_id(""));
    assert!(!validate_message_id("has space"));

    assert!(validate_source_epoch("epoch-2026.11:01"));
    assert!(!validate_source_epoch(""));
    assert!(!validate_source_epoch("bad/slash"));

    // Fixed-width WAL position comparison
    let p1 = WalPosition {
        segment_no: 9,
        record_offset: 255,
    };
    let p2 = WalPosition {
        segment_no: 10,
        record_offset: 1,
    };
    assert_eq!(p1.format_fixed(), "0000000000000009:00000000000000ff");
    assert_eq!(p2.format_fixed(), "000000000000000a:0000000000000001");
    assert!(p1 < p2);
    assert!(p1.format_fixed() < p2.format_fixed());
    assert_eq!(WalPosition::parse_fixed(&p1.format_fixed()).unwrap(), p1);
}

// ============================================================================
// Test 2: D00 Deterministic observation_id & JCS Number/String Canonicalization
// ============================================================================

#[test]
fn test_d00_deterministic_observation_id_and_jcs_canonicalization() {
    let obs1 = derive_observation_id(
        "ten-8iic",
        "gateway-01",
        "epoch-001",
        "m-20261105-000001",
        "zone1_temp",
    );
    let obs1_retry = derive_observation_id(
        "ten-8iic",
        "gateway-01",
        "epoch-001",
        "m-20261105-000001",
        "zone1_temp",
    );
    let obs2_prop = derive_observation_id(
        "ten-8iic",
        "gateway-01",
        "epoch-001",
        "m-20261105-000001",
        "zone2_temp",
    );

    assert_eq!(obs1, obs1_retry);
    assert_ne!(obs1, obs2_prop);
    assert!(obs1.starts_with("obs-"));
    // 16 bytes in base32_lower_nopad = 26 chars + 4 chars ("obs-") = 30 chars
    assert_eq!(obs1.len(), 30);
    assert!(obs1[4..]
        .bytes()
        .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));

    // RFC 8785 float formatting checks
    assert_eq!(format_jcs_f64(-0.0).unwrap(), "0");
    assert_eq!(format_jcs_f64(0.0).unwrap(), "0");
    assert_eq!(format_jcs_f64(25.0).unwrap(), "25");
    assert_eq!(format_jcs_f64(245.6).unwrap(), "245.6");
    assert_eq!(format_jcs_f64(0.42).unwrap(), "0.42");
    assert_eq!(format_jcs_f64(1e-6).unwrap(), "0.000001");
    assert_eq!(format_jcs_f64(1e-7).unwrap(), "1e-7");
    assert_eq!(format_jcs_f64(1e20).unwrap(), "100000000000000000000");
    assert_eq!(format_jcs_f64(1e21).unwrap(), "1e+21");
    assert!(format_jcs_f64(f64::NAN).is_err());
    assert!(format_jcs_f64(f64::INFINITY).is_err());
    assert!(format_jcs_f64(f64::NEG_INFINITY).is_err());
}

// ============================================================================
// Test 3: Golden Test Vectors Generation & Rust/Python 100% Cross-Verification
// ============================================================================

#[test]
fn test_d00_rust_and_python_golden_vectors_cross_verification() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vectors_path = manifest_dir.join("test_vectors.json");
    let suite = build_golden_vector_suite();
    let json_text =
        serde_json::to_string_pretty(&suite).expect("failed to serialize golden vector suite");
    fs::write(&vectors_path, format!("{}\n", json_text))
        .expect("failed to write test_vectors.json");

    // Verify in Rust
    assert_eq!(suite.vectors.len(), 7);
    let gv07 = suite
        .vectors
        .iter()
        .find(|v| v.vector_id == "GV-07-PR2-PROBE-SAMPLE")
        .unwrap();
    assert_eq!(
        gv07.expected_observation_ids.get("outlet_pressure").map(|s| s.as_str()),
        Some("obs-mfwh4ejs2b7rtlcyzrqkyufufa")
    );
    assert_eq!(
        gv07.expected_payload_hash,
        "sha256:8d7e884478d3420105475344f4b63187e937741eb68df08832b39b47e8958eae"
    );
    assert_eq!(gv07.expected_canonical_jcs.len(), 434);
    assert_eq!(
        suite.vectors[0].expected_payload_hash,
        suite.vectors[1].expected_payload_hash,
        "GV-01 and GV-02 (replayed false->true) must have identical payload_hash"
    );
    assert_ne!(
        suite.vectors[0].expected_payload_hash,
        suite.vectors[2].expected_payload_hash,
        "GV-01 and GV-03 (245.6->245.7) must have different payload_hash"
    );
    assert_eq!(
        suite.vectors[3].expected_payload_hash,
        suite.vectors[4].expected_payload_hash,
        "GV-04 and GV-05 (reordered points array) must have identical payload_hash"
    );

    // Execute Python 3 cross-verification script
    let script_path = manifest_dir.join("verify_vectors.py");
    let output = Command::new("python3")
        .arg(&script_path)
        .output()
        .expect("failed to execute python3 verify_vectors.py");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Python cross-verification failed!\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(stdout.contains("100% match"));
}

// ============================================================================
// Test 4: T01 — Gateway Normalized Engineering Value (No Secondary Scaling)
// ============================================================================

#[test]
fn test_t01_gateway_normalized_no_double_scaling() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // Also verify that attempting to register a binding with normalization_owner="gateway"
    // and server_scale_factor=Some(0.1) is rejected with 422 VALIDATION_FAILED
    let bad_binding_err = engine
        .validate_and_register_binding(
            "req-bad-bind",
            BindingRevision {
                binding_revision_id: "br-bad".to_string(),
                tenant_id: "ten-8iic".to_string(),
                device_id: "dev-oven-02".to_string(),
                model_revision_id: "mr-oven-1".to_string(),
                source_id: "gateway-01".to_string(),
                normalization_owner: "gateway".to_string(),
                server_scale_factor: Some(0.1),
                active: false,
                valid_from_ms: None,
                valid_to_ms: None,
                mapped_points: BTreeMap::new(),
            },
        )
        .unwrap_err();
    assert_eq!(bad_binding_err.status, 422);
    assert_eq!(bad_binding_err.code, ErrorCode::ValidationFailed);

    // Gateway reports engineering value 245.6 (raw register 2456 * factor 0.1 already applied by gateway)
    let msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-20261105-000001".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(245.6),
            source_at_ms: 1793869200120,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "1001".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };

    let ack = engine
        .ingest_telemetry("req-t01", Some(&gw_ctx), true, 1793869200150, &msg)
        .unwrap();
    assert_eq!(ack.http_status, 202);
    assert_eq!(ack.status, "accepted_durable");

    // Verify latest value in DeviceBusinessView is exactly 245.6 (NOT 24.56) and preserves binding_revision_id
    let view = engine
        .get_device_view("req-view-t01", Some(&nb_ctx), 1793869201000, "dev-oven-02")
        .unwrap();
    let z1 = view.properties.get("zone1_temp").unwrap();
    assert_eq!(z1.value, json!(245.6));
    assert_eq!(z1.binding_revision_id.as_deref(), Some("br-oven-12"));
    assert_eq!(z1.quality, "good");

    // Verify history observation is also 245.6
    let hist = engine
        .query_history(
            "req-hist-t01",
            Some(&nb_ctx),
            1793869201000,
            &HistoryQueryRequest {
                resource_ids: vec!["res-dev-oven-02-zone1-temp".to_string()],
                range: TimeRange {
                    start_ms: 1793869000000,
                    end_ms: 1793869300000,
                },
                mode: "raw".to_string(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 10,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(hist.items.len(), 1);
    assert_eq!(hist.items[0].value, json!(245.6));
    assert_eq!(hist.items[0].binding_revision_id, "br-oven-12");
    assert_eq!(hist.items[0].model_revision_id, "mr-oven-1");
}

// ============================================================================
// Test 5: T02 — Atomic Whole-Message Rejection (422 VALIDATION_FAILED)
// ============================================================================

#[test]
fn test_t02_whole_message_atomic_rejection_422() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // Message with 1 valid point (zone1_temp = 245.6) and 1 unknown property (zone9_temp = 180.0)
    let msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-20261105-t02".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![
            TelemetryPointWire {
                property: "zone1_temp".to_string(),
                value: json!(245.6),
                source_at_ms: 1793869200120,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: "1001".to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            },
            TelemetryPointWire {
                property: "zone9_temp".to_string(),
                value: json!(180.0),
                source_at_ms: 1793869200120,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: "1001".to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            },
        ],
    };

    let err = engine
        .ingest_telemetry("req-t02", Some(&gw_ctx), true, 1793869200150, &msg)
        .unwrap_err();
    assert_eq!(err.status, 422);
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(!err.retryable);
    assert_eq!(err.details.len(), 1);
    assert_eq!(err.details[0].field, "points[1].property");
    assert_eq!(err.details[0].issue, "UNKNOWN_PROPERTY");

    // Verify ZERO side effects: history count is 0 and zone1_temp remains unobserved (null)
    assert_eq!(engine.history_count(), 0);
    let view = engine
        .get_device_view("req-view-t02", Some(&nb_ctx), 1793869201000, "dev-oven-02")
        .unwrap();
    let z1 = view.properties.get("zone1_temp").unwrap();
    assert_eq!(z1.value, Value::Null);
    assert_eq!(z1.observation_state, "no_observation");
}

// ============================================================================
// Test 6: T03 & SC04 — Idempotent Duplicate Replay & Order Invariance (200 duplicate)
// ============================================================================

#[test]
fn test_t03_sc04_idempotent_duplicate_replay_invariance() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();

    let msg_initial = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-20261105-t03".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![
            TelemetryPointWire {
                property: "zone2_temp".to_string(),
                value: json!(250.0),
                source_at_ms: 1793869200120,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: "1001".to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            },
            TelemetryPointWire {
                property: "zone1_temp".to_string(),
                value: json!(245.6),
                source_at_ms: 1793869200120,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: "1001".to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            },
        ],
    };

    let ack1 = engine
        .ingest_telemetry(
            "req-first",
            Some(&gw_ctx),
            true,
            1793869200150,
            &msg_initial,
        )
        .unwrap();
    assert_eq!(ack1.http_status, 202);
    assert_eq!(ack1.status, "accepted_durable");
    assert_eq!(engine.history_count(), 2);

    // Retry with `replayed: true`, swapped `points` order, new `request_id`, and later `received_at_ms`
    let mut msg_retry = msg_initial.clone();
    msg_retry.points.reverse();
    msg_retry.points[0].delivery.replayed = true;
    msg_retry.points[1].delivery.replayed = true;

    let ack2 = engine
        .ingest_telemetry("req-retry", Some(&gw_ctx), true, 1793869205000, &msg_retry)
        .unwrap();
    assert_eq!(ack2.http_status, 200);
    assert_eq!(ack2.status, "duplicate");
    assert_eq!(ack2.ingest_id, ack1.ingest_id);
    assert_eq!(ack2.payload_hash, ack1.payload_hash);
    assert_eq!(ack2.observation_ids, ack1.observation_ids);
    // History still has exactly 2 rows (applied only once)
    assert_eq!(engine.history_count(), 2);
}

// ============================================================================
// Test 7: T04 & SC05 — Same Key with Altered Value Triggers 409 IDEMPOTENCY_CONFLICT
// ============================================================================

#[test]
fn test_t04_sc05_idempotency_conflict_409_on_value_change() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    let mut msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-20261105-t04".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(245.6),
            source_at_ms: 1793869200120,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "1001".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };

    engine
        .ingest_telemetry("req-t04-1", Some(&gw_ctx), true, 1793869200150, &msg)
        .unwrap();

    // Tamper temperature from 245.6 to 245.7 with same message_key
    msg.points[0].value = json!(245.7);
    let err = engine
        .ingest_telemetry("req-t04-2", Some(&gw_ctx), true, 1793869201000, &msg)
        .unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, ErrorCode::IdempotencyConflict);
    assert!(!err.retryable);

    // Original value 245.6 must not be overwritten
    let view = engine
        .get_device_view("req-view-t04", Some(&nb_ctx), 1793869202000, "dev-oven-02")
        .unwrap();
    assert_eq!(
        view.properties.get("zone1_temp").unwrap().value,
        json!(245.6)
    );
    assert_eq!(engine.history_count(), 1);
}

// ============================================================================
// Test 8: T05 & SC06 — Out-of-Order Backfill & u64 source_seq Ordering
// ============================================================================

#[test]
fn test_t05_sc06_out_of_order_replay_and_u64_source_seq_comparison() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // T05: First receive 10:00:02 (1793869202000) with value 25.0
    let msg_newer = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-t05-newer".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(25.0),
            source_at_ms: 1793869202000,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "998".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    engine
        .ingest_telemetry("req-t05-1", Some(&gw_ctx), true, 1793869202100, &msg_newer)
        .unwrap();

    // Later backfill 10:00:01 (1793869201000) with value 24.0 (replayed = true)
    let msg_older = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-t05-older".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(24.0),
            source_at_ms: 1793869201000,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "997".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: true },
        }],
    };
    engine
        .ingest_telemetry("req-t05-2", Some(&gw_ctx), true, 1793869203000, &msg_older)
        .unwrap();

    // History must contain both observations (24.0 and 25.0), while latest stays 25.0!
    assert_eq!(engine.history_count(), 2);
    let view = engine
        .get_device_view("req-view-t05", Some(&nb_ctx), 1793869203500, "dev-oven-02")
        .unwrap();
    let z1 = view.properties.get("zone1_temp").unwrap();
    assert_eq!(z1.value, json!(25.0));
    assert_eq!(z1.value_sampled_at_ms, Some(1793869202000));

    // SC06: Same millisecond (1793869210000), receive source_seq="1000" (25.0) first,
    // then source_seq="999" (24.0). In lexicographical order "999" > "1000",
    // but in u64 numeric order 1000 > 999, so "1000" (25.0) MUST win!
    let msg_seq_1000 = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-sc06-1000".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(25.0),
            source_at_ms: 1793869210000,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "1000".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    let msg_seq_999 = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-sc06-999".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(24.0),
            source_at_ms: 1793869210000,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "999".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: true },
        }],
    };

    engine
        .ingest_telemetry(
            "req-sc06-1",
            Some(&gw_ctx),
            true,
            1793869210100,
            &msg_seq_1000,
        )
        .unwrap();
    engine
        .ingest_telemetry(
            "req-sc06-2",
            Some(&gw_ctx),
            true,
            1793869210200,
            &msg_seq_999,
        )
        .unwrap();

    let view2 = engine
        .get_device_view("req-view-sc06", Some(&nb_ctx), 1793869210500, "dev-oven-02")
        .unwrap();
    assert_eq!(
        view2.properties.get("zone1_temp").unwrap().value,
        json!(25.0)
    );
}

// ============================================================================
// Test 9: T06, SC08, SC09, NB03, NB04 — State Event Separation & Null Rejection
// ============================================================================

#[test]
fn test_t06_sc08_sc09_nb03_nb04_state_separation_and_null_rejection() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // 1. NB03: Before any telemetry arrives on `pump-001`, check `/view`:
    // Bound properties (`outlet_pressure`, `running_state`) and unbound property (`maintenance_note`)
    // must have `value: null, quality: "unknown", observation_state: "no_observation"` (never 0 or false).
    let initial_view = engine
        .get_device_view("req-nb03", Some(&nb_ctx), 1793869050000, "pump-001")
        .unwrap();
    assert_eq!(initial_view.view_consistency, "per_resource_latest");

    let run_prop = initial_view.properties.get("running_state").unwrap();
    assert_eq!(run_prop.binding_state, "bound");
    assert_eq!(run_prop.observation_state, "no_observation");
    assert_eq!(run_prop.value, Value::Null);
    assert_eq!(run_prop.quality, "unknown");

    let note_prop = initial_view.properties.get("maintenance_note").unwrap();
    assert_eq!(note_prop.binding_state, "unbound");
    assert_eq!(note_prop.observation_state, "no_observation");
    assert_eq!(note_prop.value, Value::Null);
    assert_eq!(note_prop.quality, "unknown");
    assert_eq!(note_prop.quality_reason.as_deref(), Some("unbound_property"));

    // 2. SC09: Sending `value: null` to POST /v1/telemetry is rejected with 422 VALIDATION_FAILED
    let null_msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-sc09-null".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "pump-001".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        binding_revision_id: "pump-001-bind-r1".to_string(),
        points: vec![TelemetryPointWire {
            property: "outlet_pressure".to_string(),
            value: Value::Null,
            source_at_ms: 1793869100120,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "100".to_string(),
            quality: TelemetryQualityWire {
                state: "bad".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: Some("ETIMEDOUT".to_string()),
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    let null_err = engine
        .ingest_telemetry("req-sc09", Some(&gw_ctx), true, 1793869100145, &null_msg)
        .unwrap_err();
    assert_eq!(null_err.status, 422);
    assert_eq!(null_err.code, ErrorCode::ValidationFailed);
    assert_eq!(null_err.details[0].issue, "INVALID_POINT_VALUE");

    // 3. Report valid measurement `outlet_pressure = 0.42` MPa at `1793869100120`
    let valid_msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-pump-001".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "pump-001".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        binding_revision_id: "pump-001-bind-r1".to_string(),
        points: vec![TelemetryPointWire {
            property: "outlet_pressure".to_string(),
            value: json!(0.42),
            source_at_ms: 1793869100120,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "101".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    engine
        .ingest_telemetry("req-pump-1", Some(&gw_ctx), true, 1793869100145, &valid_msg)
        .unwrap();
    assert_eq!(engine.history_count(), 1);

    // 4. T06, SC08, NB04: Gateway experiences Modbus timeout at `1793869200000` and sends `iotfast.source_state.v1`
    let state_evt = SourceStateEventWire {
        spec: "iotfast.source_state.v1".to_string(),
        event_id: "evt-20261105-000089".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "pump-001".to_string(),
        binding_revision_id: "pump-001-bind-r1".to_string(),
        observed_at_ms: 1793869200000,
        connection_state: "disconnected".to_string(),
        point_states: vec![SourcePointStateWire {
            property: "outlet_pressure".to_string(),
            quality_state: "bad".to_string(),
            quality_reason: "modbus_read_timeout".to_string(),
            source_protocol: Some("modbus".to_string()),
            source_code: Some("ETIMEDOUT".to_string()),
        }],
    };
    let state_ack = engine
        .report_source_state("req-state-1", Some(&gw_ctx), 1793869200010, &state_evt)
        .unwrap();
    assert_eq!(state_ack.http_status, 202);

    // Verify `/view` and `/shadow`:
    // - `value` remains `0.42`
    // - `value_sampled_at_ms` remains `1793869100120`
    // - `quality` is `"bad"`, `quality_reason` is `"modbus_read_timeout"`
    // - `quality_observed_at_ms` is `1793869200000`
    // - `history_observation` count is STILL 1 (no fabricated measurement row!)
    assert_eq!(engine.history_count(), 1);
    assert_eq!(engine.quality_events().len(), 1);

    let after_view = engine
        .get_device_view("req-nb04-view", Some(&nb_ctx), 1793869205000, "pump-001")
        .unwrap();
    assert_eq!(after_view.source_connection_state, "disconnected");
    let p_view = after_view.properties.get("outlet_pressure").unwrap();
    assert_eq!(p_view.value, json!(0.42));
    assert_eq!(p_view.value_sampled_at_ms, Some(1793869100120));
    assert_eq!(p_view.quality, "bad");
    assert_eq!(
        p_view.quality_reason.as_deref(),
        Some("modbus_read_timeout")
    );
    assert_eq!(p_view.quality_observed_at_ms, Some(1793869200000));
    assert_eq!(p_view.freshness, "stale");

    let shadow = engine
        .get_device_shadow("req-nb04-shadow", Some(&nb_ctx), 1793869205000, "pump-001")
        .unwrap();
    let p_shadow = shadow.points.get("outlet_pressure").unwrap();
    assert_eq!(p_shadow.value, json!(0.42));
    assert_eq!(p_shadow.value_sampled_at_ms, Some(1793869100120));
    assert_eq!(p_shadow.quality_observed_at_ms, Some(1793869200000));
    assert_eq!(p_shadow.quality, "bad");
}

// ============================================================================
// Test 10: SC07 & T08 — Future Clock Skew (422) & Binding Revision Conflict (409)
// ============================================================================

#[test]
fn test_sc07_t08_future_clock_skew_422_and_binding_revision_conflict_409() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // SC07: Gateway reports source_at_ms 2 hours (7,200,000 ms) in the future
    let now_ms = 1793869200000;
    let future_msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-sc07-future".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(300.0),
            source_at_ms: now_ms + 7_200_000,
            source_time_origin: "device_clock".to_string(),
            source_seq: "2001".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    let skew_err = engine
        .ingest_telemetry("req-sc07", Some(&gw_ctx), true, now_ms, &future_msg)
        .unwrap_err();
    assert_eq!(skew_err.status, 422);
    assert_eq!(skew_err.code, ErrorCode::ClockSkewExceeded);
    assert_eq!(engine.history_count(), 0);
    let view = engine
        .get_device_view("req-sc07-view", Some(&nb_ctx), now_ms, "dev-oven-02")
        .unwrap();
    assert_eq!(view.properties.get("zone1_temp").unwrap().value, Value::Null);

    // T08: Activate new binding `br-oven-13` on `dev-oven-02`; old `br-oven-12` has `valid_to_ms = None`
    // (unproven historical window). Sending a message claiming `br-oven-12` returns 409 REVISION_CONFLICT.
    engine.activate_device_binding("dev-oven-02", "mr-oven-1", "br-oven-13");
    let stale_bind_msg = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-t08-old-bind".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![TelemetryPointWire {
            property: "zone1_temp".to_string(),
            value: json!(245.6),
            source_at_ms: now_ms,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "2002".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    let rev_err = engine
        .ingest_telemetry("req-t08", Some(&gw_ctx), true, now_ms, &stale_bind_msg)
        .unwrap_err();
    assert_eq!(rev_err.status, 409);
    assert_eq!(rev_err.code, ErrorCode::RevisionConflict);
    assert_eq!(engine.history_count(), 0);
}

// ============================================================================
// Test 11: SC01, SC02, SC03, T10 — Scope Isolation, URI Boundary & 404 Concealment
// ============================================================================

#[test]
fn test_sc01_sc02_sc03_t10_scope_isolation_uri_boundary_and_404_concealment() {
    let engine = setup_fixture_engine();
    let now_ms = 1793869200000;

    // SC01: Unauthenticated caller (None scope_ctx, even if client passed X-Tenant-Id / X-Role headers) -> 401 UNAUTHENTICATED
    let unauth_err = engine
        .get_device_view("req-sc01", None, now_ms, "pump-001")
        .unwrap_err();
    assert_eq!(unauth_err.status, 401);
    assert_eq!(unauth_err.code, ErrorCode::Unauthenticated);

    // SC03: URI segment boundary safety:
    // `uns://8iic/plant-a/` matches `uns://8iic/plant-a/room-1/pump-001/properties/outlet_pressure`
    // but NEVER matches `uns://8iic/plant-ab/pump-01/properties/outlet_pressure`
    assert!(uri_prefix_matches(
        "uns://8iic/plant-a/",
        "uns://8iic/plant-a/room-1/pump-001/properties/outlet_pressure"
    ));
    assert!(!uri_prefix_matches(
        "uns://8iic/plant-a/",
        "uns://8iic/plant-ab/pump-01/properties/outlet_pressure"
    ));

    let line01_ctx = northbound_line01_scope_ctx();
    let ok_res = engine
        .resolve_resource_by_uri(
            "req-sc03-ok",
            Some(&line01_ctx),
            now_ms,
            "uns://8iic/plant-a/room-1/pump-001/properties/outlet_pressure",
        )
        .unwrap();
    assert_eq!(ok_res.device_id, "pump-001");

    let boundary_err = engine
        .resolve_resource_by_uri(
            "req-sc03-deny",
            Some(&line01_ctx),
            now_ms,
            "uns://8iic/plant-ab/pump-01/properties/outlet_pressure",
        )
        .unwrap_err();
    assert_eq!(boundary_err.status, 404);
    assert_eq!(boundary_err.code, ErrorCode::ResourceNotFound);

    // SC02 & T10: AI / CatalogReader or NorthboundService authorized only for `plant-a` / `asset-line-01`
    // queries `pump-b01` (under `asset-line-02`) -> returns 404 RESOURCE_NOT_FOUND
    let ai_ctx = VerifiedScopeContext {
        spec: "iotfast.scope_context.v1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        project_id: Some("proj-smt-01".to_string()),
        caller_service_id: "svc-mcp-reader-line01".to_string(),
        caller_kind: CallerKind::CatalogReader,
        bound_source_id: None,
        issued_at_ms: 1793869000000,
        expires_at_ms: 1793879000000,
        allowed_actions: vec![
            "catalog:read".to_string(),
            "view:read".to_string(),
            "history:query".to_string(),
        ],
        scope_filter: ScopeFilter {
            asset_ids: vec!["asset-line-01".to_string()],
            include_asset_descendants: true,
            device_ids: vec![],
            resource_ids: vec![],
            uri_prefixes: vec![],
            tenant_wide: false,
        },
        quota_profile: "mcp_readonly".to_string(),
    };

    // Single-device view on unauthorized `pump-b01` -> 404 RESOURCE_NOT_FOUND
    let view_err = engine
        .get_device_view("req-t10-view", Some(&ai_ctx), now_ms, "pump-b01")
        .unwrap_err();
    assert_eq!(view_err.status, 404);
    assert_eq!(view_err.code, ErrorCode::ResourceNotFound);

    // Batch history query mixing authorized `res-pump-001-outlet-pressure` and unauthorized `res-pump-b01-outlet-pressure`
    // -> whole request rejected with 404 RESOURCE_NOT_FOUND (no silent partial drop!)
    let batch_err = engine
        .query_history(
            "req-sc02-batch",
            Some(&ai_ctx),
            now_ms,
            &HistoryQueryRequest {
                resource_ids: vec![
                    "res-pump-001-outlet-pressure".to_string(),
                    "res-pump-b01-outlet-pressure".to_string(),
                ],
                range: TimeRange {
                    start_ms: 1793860000000,
                    end_ms: 1793870000000,
                },
                mode: "raw".to_string(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 100,
                cursor: None,
            },
        )
        .unwrap_err();
    assert_eq!(batch_err.status, 404);
    assert_eq!(batch_err.code, ErrorCode::ResourceNotFound);

    // Catalog search filters by VerifiedScopeContext BEFORE keyword matching:
    // Searching for "水泵" only returns `pump-001` resources, never `pump-b01` or `pump-ab01`
    let catalog_hits = engine
        .search_catalog_resources("req-t10-search", Some(&ai_ctx), now_ms, Some("压力"))
        .unwrap();
    assert_eq!(catalog_hits.len(), 1);
    assert_eq!(catalog_hits[0].device_id, "pump-001");
}

// ============================================================================
// Test 12: NB07, NB08, NB09 — History Half-Open Range, Cross-Version Incompatible (422) & Cursor Binding (422)
// ============================================================================

#[test]
fn test_nb07_nb08_nb09_history_half_open_range_cross_version_422_and_cursor_scope_binding() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // Ingest 3 points on `pump-001` under `pump-model-r1` (`MPa`) at 1000, 1500, 1500 (same ms, different message_id), and 2000
    for (msg_id, ts, seq, val) in [
        ("m-hist-1", 1793866001000i64, "1", 0.40),
        ("m-hist-2", 1793866001500i64, "2", 0.42),
        ("m-hist-3", 1793866001500i64, "3", 0.44),
        ("m-hist-4", 1793866002000i64, "4", 0.46),
    ] {
        let m = TelemetryMessageWire {
            spec: "iotfast.telemetry.v1".to_string(),
            message_id: msg_id.to_string(),
            source_id: "gateway-01".to_string(),
            source_epoch: "epoch-001".to_string(),
            device_id: "pump-001".to_string(),
            model_revision_id: "pump-model-r1".to_string(),
            binding_revision_id: "pump-001-bind-r1".to_string(),
            points: vec![TelemetryPointWire {
                property: "outlet_pressure".to_string(),
                value: json!(val),
                source_at_ms: ts,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: seq.to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            }],
        };
        engine
            .ingest_telemetry(msg_id, Some(&gw_ctx), true, ts + 10, &m)
            .unwrap();
    }

    // NB07: Half-open range `[1793866001000, 1793866002000)` includes 1000 and both 1500 records (3 records),
    // and EXCLUDES 2000! Paginate with limit=2 to test same-millisecond tie-breaking by `observation_id`.
    let page1 = engine
        .query_history(
            "req-nb07-p1",
            Some(&nb_ctx),
            1793866005000,
            &HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".to_string()],
                range: TimeRange {
                    start_ms: 1793866001000,
                    end_ms: 1793866002000,
                },
                mode: "raw".to_string(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 2,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next_cursor.is_some());

    let page2 = engine
        .query_history(
            "req-nb07-p2",
            Some(&nb_ctx),
            1793866005000,
            &HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".to_string()],
                range: TimeRange {
                    start_ms: 1793866001000,
                    end_ms: 1793866002000,
                },
                mode: "raw".to_string(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 2,
                cursor: page1.next_cursor.clone(),
            },
        )
        .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert!(page2.next_cursor.is_none());
    // Ensure the two records at 1793866001500 across page1 and page2 are distinct and ordered by observation_id
    assert_eq!(page1.items[1].source_at_ms, 1793866001500);
    assert_eq!(page2.items[0].source_at_ms, 1793866001500);
    assert!(page1.items[1].observation_id < page2.items[0].observation_id);

    // NB09: Replay `page1.next_cursor` under a different VerifiedScopeContext (narrower scope `device_ids: ["pump-001"]`)
    // -> must reject with 422 VALIDATION_FAILED (`CURSOR_SCOPE_MISMATCH`)
    let other_scope_ctx = VerifiedScopeContext {
        spec: "iotfast.scope_context.v1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        project_id: Some("proj-smt-01".to_string()),
        caller_service_id: "svc-other-scope".to_string(),
        caller_kind: CallerKind::NorthboundService,
        bound_source_id: None,
        issued_at_ms: 1793860000000,
        expires_at_ms: 1793879000000,
        allowed_actions: vec!["history:query".to_string()],
        scope_filter: ScopeFilter {
            asset_ids: vec![],
            include_asset_descendants: false,
            device_ids: vec!["pump-001".to_string()],
            resource_ids: vec![],
            uri_prefixes: vec![],
            tenant_wide: false,
        },
        quota_profile: "standard_northbound".to_string(),
    };
    let cursor_err = engine
        .query_history(
            "req-nb09",
            Some(&other_scope_ctx),
            1793866005000,
            &HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".to_string()],
                range: TimeRange {
                    start_ms: 1793866001000,
                    end_ms: 1793866002000,
                },
                mode: "raw".to_string(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 2,
                cursor: page1.next_cursor.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(cursor_err.status, 422);
    assert_eq!(cursor_err.code, ErrorCode::ValidationFailed);
    assert_eq!(cursor_err.details[0].issue, "CURSOR_SCOPE_MISMATCH");

    // NB08: Upgrade `pump-001` to `pump-model-r2-kpa` (`kPa`) and `pump-001-bind-r2`, ingest 450.0 kPa at 1793867001000.
    // Querying `mode = "aggregate"` across `[1793866000000, 1793868000000)` spans `MPa` and `kPa`
    // -> must reject with 422 CROSS_VERSION_INCOMPATIBLE!
    engine.activate_device_binding("pump-001", "pump-model-r2-kpa", "pump-001-bind-r2");
    let m_kpa = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-hist-kpa-1".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "pump-001".to_string(),
        model_revision_id: "pump-model-r2-kpa".to_string(),
        binding_revision_id: "pump-001-bind-r2".to_string(),
        points: vec![TelemetryPointWire {
            property: "outlet_pressure".to_string(),
            value: json!(450.0),
            source_at_ms: 1793867001000,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: "5".to_string(),
            quality: TelemetryQualityWire {
                state: "good".to_string(),
                source_protocol: Some("modbus".to_string()),
                source_code: None,
            },
            delivery: TelemetryDeliveryWire { replayed: false },
        }],
    };
    engine
        .ingest_telemetry("req-kpa-1", Some(&gw_ctx), true, 1793867001020, &m_kpa)
        .unwrap();

    let agg_err = engine
        .query_history(
            "req-nb08",
            Some(&nb_ctx),
            1793868000000,
            &HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".to_string()],
                range: TimeRange {
                    start_ms: 1793866000000,
                    end_ms: 1793868000000,
                },
                mode: "aggregate".to_string(),
                aggregation: Some(AggregationSpec {
                    bucket_ms: 60_000,
                    functions: vec!["avg".to_string(), "min".to_string(), "max".to_string()],
                    empty_bucket_policy: "omit".to_string(),
                }),
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 100,
                cursor: None,
            },
        )
        .unwrap_err();
    assert_eq!(agg_err.status, 422);
    assert_eq!(agg_err.code, ErrorCode::CrossVersionIncompatible);
}


// ============================================================================
// Test 13: PR #2 Blueprint Rules (R03, R05, R08, R09)
// ============================================================================

#[test]
fn test_pr2_r03_r05_r08_r09_cross_module_blueprint_rules() {
    let mut engine = setup_fixture_engine();
    let gw_ctx = gateway_scope_ctx();
    let nb_ctx = northbound_line01_scope_ctx();

    // 1. Ingest initial measurement for zone1_temp (245.6) and zone2_temp (250.0) under br-oven-12
    let msg1 = TelemetryMessageWire {
        spec: "iotfast.telemetry.v1".to_string(),
        message_id: "m-r08-accepted-01".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        model_revision_id: "mr-oven-1".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        points: vec![
            TelemetryPointWire {
                property: "zone1_temp".to_string(),
                value: json!(245.6),
                source_at_ms: 1793869200000,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: "10".to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            },
            TelemetryPointWire {
                property: "zone2_temp".to_string(),
                value: json!(250.0),
                source_at_ms: 1793869200000,
                source_time_origin: "gateway_sample".to_string(),
                source_seq: "10".to_string(),
                quality: TelemetryQualityWire {
                    state: "good".to_string(),
                    source_protocol: Some("modbus".to_string()),
                    source_code: None,
                },
                delivery: TelemetryDeliveryWire { replayed: false },
            },
        ],
    };
    let ack1 = engine
        .ingest_telemetry("req-r08-1", Some(&gw_ctx), true, 1793869200100, &msg1)
        .unwrap();
    assert_eq!(ack1.http_status, 202);

    // 2. R03: Gateway disconnect WITHOUT point_states (`point_states: []`)
    // -> source_connection_state becomes "disconnected", freshness becomes "stale",
    //    but point quality remains "good" (not falsely marked "bad").
    //    Also test that event_id = "m-r08-accepted-01" does NOT collide with telemetry message_id!
    let disconnect_evt = SourceStateEventWire {
        spec: "iotfast.source_state.v1".to_string(),
        event_id: "m-r08-accepted-01".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        observed_at_ms: 1793869205000,
        connection_state: "disconnected".to_string(),
        point_states: vec![],
    };
    let s_ack1 = engine
        .report_source_state("req-r03-1", Some(&gw_ctx), 1793869205010, &disconnect_evt)
        .unwrap();
    assert_eq!(s_ack1.http_status, 202);
    assert_eq!(s_ack1.status, "accepted");

    // Duplicate retry of the same source_state event returns 200 duplicate
    let s_ack1_dup = engine
        .report_source_state("req-r03-1b", Some(&gw_ctx), 1793869205020, &disconnect_evt)
        .unwrap();
    assert_eq!(s_ack1_dup.http_status, 200);
    assert_eq!(s_ack1_dup.status, "duplicate");

    let view_disc = engine
        .get_device_view("req-r03-v1", Some(&nb_ctx), 1793869206000, "dev-oven-02")
        .unwrap();
    assert_eq!(view_disc.source_connection_state, "disconnected");
    assert_eq!(view_disc.properties["zone1_temp"].quality, "good");
    assert_eq!(view_disc.properties["zone1_temp"].freshness, "stale");

    // Point-level error on zone1_temp marks zone1_temp quality="bad", while zone2_temp stays "good"
    let point_err_evt = SourceStateEventWire {
        spec: "iotfast.source_state.v1".to_string(),
        event_id: "evt-r03-point-bad".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        observed_at_ms: 1793869207000,
        connection_state: "disconnected".to_string(),
        point_states: vec![SourcePointStateWire {
            property: "zone1_temp".to_string(),
            quality_state: "bad".to_string(),
            quality_reason: "modbus_read_timeout".to_string(),
            source_protocol: Some("modbus".to_string()),
            source_code: Some("ETIMEDOUT".to_string()),
        }],
    };
    engine
        .report_source_state("req-r03-2", Some(&gw_ctx), 1793869207010, &point_err_evt)
        .unwrap();

    // Gateway reconnect (`connection_state = "connected"`, `point_states: []`) restores connection
    // and zone2_temp freshness to "fresh", but does NOT falsely reset zone1_temp from "bad" to "good"!
    let reconnect_evt = SourceStateEventWire {
        spec: "iotfast.source_state.v1".to_string(),
        event_id: "evt-r03-reconnect".to_string(),
        source_id: "gateway-01".to_string(),
        source_epoch: "epoch-001".to_string(),
        device_id: "dev-oven-02".to_string(),
        binding_revision_id: "br-oven-12".to_string(),
        observed_at_ms: 1793869208000,
        connection_state: "connected".to_string(),
        point_states: vec![],
    };
    engine
        .report_source_state("req-r03-3", Some(&gw_ctx), 1793869208010, &reconnect_evt)
        .unwrap();

    let view_reconn = engine
        .get_device_view("req-r03-v2", Some(&nb_ctx), 1793869209000, "dev-oven-02")
        .unwrap();
    assert_eq!(view_reconn.source_connection_state, "connected");
    assert_eq!(view_reconn.properties["zone1_temp"].quality, "bad");
    assert_eq!(view_reconn.properties["zone1_temp"].freshness, "stale");
    assert_eq!(view_reconn.properties["zone2_temp"].quality, "good");
    assert_eq!(view_reconn.properties["zone2_temp"].freshness, "fresh");

    // 3. R09 & R05: Resource-scoped caller (`resource_ids = ["res-dev-oven-02-zone1-temp"]`)
    // calling get_device_view / get_device_shadow only sees `zone1_temp` (no sibling property leak),
    // and batch history query mixing authorized + unauthorized IDs returns 404 with empty details.
    let partial_res_ctx = VerifiedScopeContext {
        spec: "iotfast.scope_context.v1".to_string(),
        tenant_id: "ten-8iic".to_string(),
        project_id: Some("proj-smt-01".to_string()),
        caller_service_id: "svc-zone1-panel".to_string(),
        caller_kind: CallerKind::NorthboundService,
        bound_source_id: None,
        issued_at_ms: 1793869000000,
        expires_at_ms: 1793879000000,
        allowed_actions: vec![
            "view:read".to_string(),
            "shadow:read".to_string(),
            "history:query".to_string(),
        ],
        scope_filter: ScopeFilter {
            asset_ids: vec![],
            include_asset_descendants: false,
            device_ids: vec![],
            resource_ids: vec!["res-dev-oven-02-zone1-temp".to_string()],
            uri_prefixes: vec![],
            tenant_wide: false,
        },
        quota_profile: "standard_northbound".to_string(),
    };
    let partial_view = engine
        .get_device_view(
            "req-r09-view",
            Some(&partial_res_ctx),
            1793869209000,
            "dev-oven-02",
        )
        .unwrap();
    assert_eq!(partial_view.properties.len(), 1);
    assert!(partial_view.properties.contains_key("zone1_temp"));
    assert!(!partial_view.properties.contains_key("zone2_temp"));
    assert!(!partial_view.properties.contains_key("heater_fault_code"));

    let partial_shadow = engine
        .get_device_shadow(
            "req-r09-shadow",
            Some(&partial_res_ctx),
            1793869209000,
            "dev-oven-02",
        )
        .unwrap();
    assert_eq!(partial_shadow.points.len(), 1);
    assert!(partial_shadow.points.contains_key("zone1_temp"));

    let batch_err = engine
        .query_history(
            "req-r05-batch",
            Some(&partial_res_ctx),
            1793869209000,
            &HistoryQueryRequest {
                resource_ids: vec![
                    "res-dev-oven-02-zone1-temp".to_string(),
                    "res-dev-oven-02-zone2-temp".to_string(),
                ],
                range: TimeRange {
                    start_ms: 1793869000000,
                    end_ms: 1793869300000,
                },
                mode: "raw".to_string(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".to_string(),
                limit: 10,
                cursor: None,
            },
        )
        .unwrap_err();
    assert_eq!(batch_err.status, 404);
    assert_eq!(batch_err.code, ErrorCode::ResourceNotFound);
    assert!(
        batch_err.details.is_empty(),
        "R05: batch 404 must not echo which specific resource_ids were unauthorized"
    );

    // 4. R08: Rotate dev-oven-02 active binding to `br-oven-13` (while `br-oven-12` has no valid_to_ms).
    // Retrying the ALREADY ACCEPTED `m-r08-accepted-01` (with replayed=true) must return `200 duplicate`
    // (NOT `409 REVISION_CONFLICT`), whereas a NEW message under `br-oven-12` returns `409 REVISION_CONFLICT`.
    engine.activate_device_binding("dev-oven-02", "mr-oven-1", "br-oven-13");

    let mut msg1_retry = msg1.clone();
    msg1_retry.points[0].delivery.replayed = true;
    msg1_retry.points[1].delivery.replayed = true;
    let ack1_retry = engine
        .ingest_telemetry(
            "req-r08-retry",
            Some(&gw_ctx),
            true,
            1793869210000,
            &msg1_retry,
        )
        .unwrap();
    assert_eq!(ack1_retry.http_status, 200);
    assert_eq!(ack1_retry.status, "duplicate");
    assert_eq!(ack1_retry.ingest_id, ack1.ingest_id);

    let mut msg_new_old_bind = msg1.clone();
    msg_new_old_bind.message_id = "m-r08-brand-new-under-old-binding".to_string();
    let rev_err = engine
        .ingest_telemetry(
            "req-r08-new-fail",
            Some(&gw_ctx),
            true,
            1793869210100,
            &msg_new_old_bind,
        )
        .unwrap_err();
    assert_eq!(rev_err.status, 409);
    assert_eq!(rev_err.code, ErrorCode::RevisionConflict);
}
