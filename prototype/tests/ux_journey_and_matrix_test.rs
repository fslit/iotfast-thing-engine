use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::server::{
    read_ws_text_frame, write_ws_masked_text_frame, IoStream, ServerHandle,
};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};

const TEST_SECRET: &str = "ux-journey-and-matrix-hmac-secret-2026";

fn http_call(
    srv: &ServerHandle,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> (u16, Value) {
    let mut stream = srv.connect();
    let body_bytes = body
        .map(|v| serde_json::to_vec(v).unwrap())
        .unwrap_or_default();
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        method,
        target,
        body_bytes.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    if !body_bytes.is_empty() {
        stream.write_all(&body_bytes).unwrap();
    }
    stream.flush().unwrap();

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).unwrap();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut resp_body = Vec::new();
    reader.read_to_end(&mut resp_body).unwrap();
    let parsed: Value = serde_json::from_slice(&resp_body).unwrap_or(Value::Null);
    (status, parsed)
}

fn ws_connect(srv: &ServerHandle, token: &str, now_ms: i64) -> Box<dyn IoStream> {
    let mut stream = srv.connect();
    let req = format!(
        "GET /v1/subscriptions HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {}\r\nX-Engine-Now-Ms: {}\r\n\r\n",
        token, now_ms
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut header_buf = Vec::new();
    let mut byte = [0u8; 1];
    while !header_buf.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        header_buf.push(byte[0]);
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    assert!(
        header_str.starts_with("HTTP/1.1 101"),
        "Expected 101 Switching Protocols, got: {}",
        header_str
    );
    stream
}

#[test]
fn u01_to_u08_integration_journey_and_ux01_to_ux12_state_matrix_verification() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut srv = ServerHandle::spawn(cfg, TEST_SECRET).unwrap();
    let addr = &srv;

    // Mint signed service tokens for management, gateway, and scoped northbound reader
    let mgmt_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-ux".into(),
        caller_service_id: "svc-control-plane".into(),
        caller_kind: CallerKind::ManagementService,
        issued_at_ms: 1_000,
        expires_at_ms: 500_000_000,
        allowed_actions: vec![
            "product:write".into(),
            "model:write".into(),
            "asset:write".into(),
            "device:write".into(),
            "binding:write".into(),
            "catalog:read".into(),
            "view:read".into(),
            "shadow:read".into(),
            "history:query".into(),
            "subscription:connect".into(),
        ],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: Some("op-integrator".into()),
    }
    .issue_signed_token(TEST_SECRET);

    let gw_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-ux".into(),
        caller_service_id: "gw-ux-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_000,
        expires_at_ms: 900_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: None,
    }
    .issue_signed_token(TEST_SECRET);

    let zone_a_reader_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-ux".into(),
        caller_service_id: "svc-zone-a-dashboard".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 500_000_000,
        allowed_actions: vec![
            "catalog:read".into(),
            "view:read".into(),
            "shadow:read".into(),
            "history:query".into(),
            "subscription:connect".into(),
        ],
        scope_filter: ScopeFilter {
            asset_ids: vec!["asset-zone-a".into()],
            include_asset_descendants: true,
            ..Default::default()
        },
        operator_id: None,
    }
    .issue_signed_token(TEST_SECRET);

    let auth_mgmt = format!("Bearer {}", mgmt_token);
    let auth_gw = format!("Bearer {}", gw_token);
    let auth_zone_a = format!("Bearer {}", zone_a_reader_token);

    // =========================================================================
    // U01 环境与身份 (Environment Readiness & Service Identity Verification)
    // =========================================================================
    let (st_health, health_body) = http_call(addr, "GET", "/health", &[], None);
    assert_eq!(st_health, 200);
    assert_eq!(health_body["state"], "Running");
    assert_eq!(health_body["healthy"], true);
    assert_eq!(health_body["duckdb_version"], "v2.0.0");
    assert_eq!(health_body["data_dir_writable"], true);
    assert_eq!(health_body["wal_dir_writable"], true);

    // Missing token -> 401 UNAUTHENTICATED
    let (st_no_tok, err_no_tok) = http_call(addr, "GET", "/v1/products", &[], None);
    assert_eq!(st_no_tok, 401);
    assert_eq!(err_no_tok["error"]["code"], "UNAUTHENTICATED");

    // Forged signature token -> 401 UNAUTHENTICATED
    let (st_forged, err_forged) = http_call(
        addr,
        "GET",
        "/v1/products",
        &[(
            "Authorization",
            "Bearer iotfast1.eyJ0ZW5hbnRfaWQiOiJ0ZW4tOGlpYyJ9.deadbeef",
        )],
        None,
    );
    assert_eq!(st_forged, 401);
    assert_eq!(err_forged["error"]["code"], "UNAUTHENTICATED");

    // Client-supplied X-Role or X-Tenant-Id -> 422 VALIDATION_FAILED (SC02)
    for (forbidden_hdr, hdr_val) in [("X-Role", "admin"), ("X-Tenant-Id", "ten-hijack")] {
        let (st_hdr, err_hdr) = http_call(
            addr,
            "GET",
            "/v1/products",
            &[
                ("Authorization", &auth_mgmt),
                (forbidden_hdr, hdr_val),
                ("X-Engine-Now-Ms", "10000"),
            ],
            None,
        );
        assert_eq!(st_hdr, 422);
        assert_eq!(err_hdr["error"]["code"], "VALIDATION_FAILED");
        assert_eq!(
            err_hdr["error"]["details"][0]["issue"],
            "CLIENT_REPORTED_SCOPE_FORBIDDEN"
        );
    }

    // =========================================================================
    // U02 发布模型 (Product Creation, Draft Validation, Immutable Publish & Diff)
    // =========================================================================
    let (st_prod, prod_body) = http_call(
        addr,
        "POST",
        "/v1/products",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "10000")],
        Some(&json!({
            "product_id": "prod-temp-sensor",
            "product_key": "industrial-temp-sensor",
            "name": "工业温控传感器"
        })),
    );
    assert_eq!(st_prod, 201);
    assert_eq!(prod_body["product_id"], "prod-temp-sensor");

    // Negative check: draft with min_value > max_value fails with 422 VALIDATION_FAILED and field path
    let (st_bad_draft, bad_val_body) = http_call(
        addr,
        "POST",
        "/v1/products/prod-temp-sensor/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "10100")],
        Some(&json!({
            "model_revision_id": "temp-model-bad",
            "model_version": "0.1.0",
            "properties": [
                {
                    "identifier": "temperature",
                    "name": "温度",
                    "value_type": "float64",
                    "unit_code": "Cel",
                    "unit_symbol": "°C",
                    "min_value": 100.0,
                    "max_value": -40.0,
                    "required": true
                }
            ]
        })),
    );
    assert_eq!(st_bad_draft, 422);
    assert_eq!(bad_val_body["error"]["code"], "VALIDATION_FAILED");
    assert!(bad_val_body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("min_value > max_value"));

    // Create valid model revision r1
    let (st_m1, _) = http_call(
        addr,
        "POST",
        "/v1/products/prod-temp-sensor/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "10500")],
        Some(&json!({
            "model_revision_id": "temp-model-r1",
            "model_version": "1.0.0",
            "properties": [
                {
                    "identifier": "temperature",
                    "name": "腔体温度",
                    "value_type": "float64",
                    "unit_code": "Cel",
                    "unit_symbol": "°C",
                    "min_value": -40.0,
                    "max_value": 150.0,
                    "required": true
                },
                {
                    "identifier": "calibration_offset",
                    "name": "标定偏置",
                    "value_type": "float64",
                    "unit_code": "Cel",
                    "unit_symbol": "°C",
                    "min_value": -10.0,
                    "max_value": 10.0,
                    "required": false
                },
                {
                    "identifier": "maintenance_tag",
                    "name": "维护标签",
                    "value_type": "text",
                    "unit_code": null,
                    "unit_symbol": null,
                    "min_value": null,
                    "max_value": null,
                    "required": false
                }
            ]
        })),
    );
    assert_eq!(st_m1, 201);

    let (st_v1, v1_body) = http_call(
        addr,
        "POST",
        "/v1/models/temp-model-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "11000")],
        None,
    );
    assert_eq!(st_v1, 200);
    assert_eq!(v1_body["valid"], true);
    assert_eq!(v1_body["compatibility"], "initial");

    let (st_p1, p1_body) = http_call(
        addr,
        "POST",
        "/v1/models/temp-model-r1:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12000")],
        Some(&json!({"published_at_ms": 12000})),
    );
    assert_eq!(st_p1, 200);
    assert_eq!(p1_body["status"], "published");
    assert!(p1_body["model_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    // Publishing an already published revision must be rejected with 409 REVISION_CONFLICT
    let (st_p1_again, _) = http_call(
        addr,
        "POST",
        "/v1/models/temp-model-r1:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12100")],
        Some(&json!({"published_at_ms": 12100})),
    );
    assert_eq!(st_p1_again, 409);

    // Create compatible revision r2 and verify diff (:compare)
    let mut r2_props = http_call(
        addr,
        "GET",
        "/v1/models/temp-model-r1",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12200")],
        None,
    )
    .1["properties"]
        .as_array()
        .unwrap()
        .clone();
    r2_props.push(json!({
        "identifier": "ambient_humidity",
        "name": "环境湿度",
        "value_type": "float64",
        "unit_code": "%RH",
        "unit_symbol": "%RH",
        "min_value": 0.0,
        "max_value": 100.0,
        "required": false
    }));
    let (st_m2, _) = http_call(
        addr,
        "POST",
        "/v1/products/prod-temp-sensor/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12500")],
        Some(&json!({
            "model_revision_id": "temp-model-r2",
            "model_version": "1.1.0",
            "properties": r2_props
        })),
    );
    assert_eq!(st_m2, 201);

    let (st_cmp, cmp_body) = http_call(
        addr,
        "POST",
        "/v1/models:compare",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12600")],
        Some(&json!({
            "from_revision_id": "temp-model-r1",
            "to_revision_id": "temp-model-r2"
        })),
    );
    assert_eq!(st_cmp, 200);
    assert_eq!(cmp_body["compatibility"], "compatible_extension");
    assert_eq!(cmp_body["added_properties"], json!(["ambient_humidity"]));

    // =========================================================================
    // U03 实例化 (Instantiate Two Devices & Verify Stable Resource IDs)
    // =========================================================================
    for asset_req in [
        json!({"asset_id": "asset-plant-1", "parent_asset_id": null, "slug": "plant-1", "name": "一厂区"}),
        json!({"asset_id": "asset-zone-a", "parent_asset_id": "asset-plant-1", "slug": "zone-a", "name": "A温区"}),
        json!({"asset_id": "asset-zone-b", "parent_asset_id": "asset-plant-1", "slug": "zone-b", "name": "B温区"}),
    ] {
        let (st, _) = http_call(
            addr,
            "POST",
            "/v1/assets",
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "13000")],
            Some(&asset_req),
        );
        assert_eq!(st, 201);
    }

    let (st_d1, d1_body) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "14000")],
        Some(&json!({
            "device_id": "sensor-a01",
            "device_key": "sensor-a01",
            "device_name": "A区1号温控器",
            "product_id": "prod-temp-sensor",
            "model_revision_id": "temp-model-r1",
            "asset_id": "asset-zone-a"
        })),
    );
    assert_eq!(st_d1, 201);

    let (st_d2, d2_body) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "14100")],
        Some(&json!({
            "device_id": "sensor-a02",
            "device_key": "sensor-a02",
            "device_name": "A区2号温控器",
            "product_id": "prod-temp-sensor",
            "model_revision_id": "temp-model-r1",
            "asset_id": "asset-zone-a"
        })),
    );
    assert_eq!(st_d2, 201);

    // Also instantiate sensor-b01 in zone-b (outside zone_a_reader_token Scope)
    let (st_db1, _) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "14200")],
        Some(&json!({
            "device_id": "sensor-b01",
            "device_key": "sensor-b01",
            "device_name": "B区1号温控器",
            "product_id": "prod-temp-sensor",
            "model_revision_id": "temp-model-r1",
            "asset_id": "asset-zone-b"
        })),
    );
    assert_eq!(st_db1, 201);

    // Verify stable resource_ids do not collide across devices sharing the same model property
    let d1_res_ids: Vec<&str> = d1_body["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["resource_id"].as_str().unwrap())
        .collect();
    let d2_res_ids: Vec<&str> = d2_body["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["resource_id"].as_str().unwrap())
        .collect();
    assert!(d1_res_ids.contains(&"res-sensor-a01-temperature"));
    assert!(d2_res_ids.contains(&"res-sensor-a02-temperature"));
    for r_id in &d1_res_ids {
        assert!(
            !d2_res_ids.contains(r_id),
            "Resource ID {} must not collide across devices",
            r_id
        );
    }

    // Duplicate path conflict rejected with 409
    let (st_dup_path, _) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "14300")],
        Some(&json!({
            "device_id": "sensor-a01-dup",
            "device_key": "sensor-a01",
            "device_name": "路径冲突设备",
            "product_id": "prod-temp-sensor",
            "model_revision_id": "temp-model-r1",
            "asset_id": "asset-zone-a"
        })),
    );
    assert_eq!(st_dup_path, 409);

    // =========================================================================
    // U04 来源绑定 (Source Grant, Gateway + Static Binding, Validate & Activate)
    // =========================================================================
    for dev_id in ["sensor-a01", "sensor-a02"] {
        let (st_grant, _) = http_call(
            addr,
            "POST",
            "/v1/sources/grants",
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15000")],
            Some(&json!({
                "source_id": "gw-ux-01",
                "device_id": dev_id,
                "valid_from_ms": 15000
            })),
        );
        assert_eq!(st_grant, 200);
    }

    // Create binding draft for sensor-a01 with gateway_measurement + static_config
    let (st_b1, _) = http_call(
        addr,
        "POST",
        "/v1/devices/sensor-a01/bindings",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16000")],
        Some(&json!({
            "binding_revision_id": "bind-a01-r1",
            "model_revision_id": "temp-model-r1",
            "source_id": "gw-ux-01",
            "normalization_owner": "gateway",
            "server_scale_factor": null,
            "points": [
                {
                    "property": "temperature",
                    "register_address": "40001",
                    "factor": 0.1,
                    "unit_code": "Cel",
                    "deadband": 0.2,
                    "heartbeat_ms": 60000
                }
            ],
            "static_points": [
                {
                    "property": "calibration_offset",
                    "value": 0.5,
                    "unit_code": "Cel"
                }
            ]
        })),
    );
    assert_eq!(st_b1, 201);

    let (st_bv1, bv1_body) = http_call(
        addr,
        "POST",
        "/v1/bindings/bind-a01-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16500")],
        None,
    );
    assert_eq!(st_bv1, 200);
    assert_eq!(bv1_body["status"], "ready");
    let b1_config_hash = bv1_body["config_hash"].as_str().unwrap().to_string();

    // U04 guard: Validating binding (status="ready") does NOT activate it on the device
    let (_, pre_act_view) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=16600",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert!(
        pre_act_view["active_binding_revision_id"].is_null(),
        "U04: Binding validation must not activate binding before gateway_ack or manual_stop_window"
    );

    // Activate bind-a01-r1 with trusted gateway_ack
    let (st_ba1, ba1_body) = http_call(
        addr,
        "POST",
        "/v1/bindings/bind-a01-r1:activate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "17000")],
        Some(&json!({
            "expected_device_revision": 1,
            "activated_at_ms": 17000,
            "gateway_ack": {
                "source_id": "gw-ux-01",
                "source_epoch": "epoch-ux-01",
                "binding_revision_id": "bind-a01-r1",
                "config_hash": b1_config_hash,
                "acknowledged_at_ms": 16900
            }
        })),
    );
    assert_eq!(st_ba1, 200);
    assert_eq!(ba1_body["status"], "active");

    // Also bind and activate sensor-a02 via manual_stop_window (will remain with 0 telemetry for UX01/UX09)
    http_call(
        addr,
        "POST",
        "/v1/devices/sensor-a02/bindings",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "17100")],
        Some(&json!({
            "binding_revision_id": "bind-a02-r1",
            "model_revision_id": "temp-model-r1",
            "source_id": "gw-ux-01",
            "normalization_owner": "gateway",
            "server_scale_factor": null,
            "points": [
                {
                    "property": "temperature",
                    "register_address": "40002",
                    "factor": 0.1,
                    "unit_code": "Cel",
                    "deadband": 0.2,
                    "heartbeat_ms": 60000
                }
            ],
            "static_points": []
        })),
    );
    http_call(
        addr,
        "POST",
        "/v1/bindings/bind-a02-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "17200")],
        None,
    );
    let (st_ba2, _) = http_call(
        addr,
        "POST",
        "/v1/bindings/bind-a02-r1:activate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "17500")],
        Some(&json!({
            "expected_device_revision": 1,
            "activated_at_ms": 17500,
            "manual_stop_window": {
                "window_start_ms": 17300,
                "window_end_ms": 17600,
                "operator_id": "op-integrator"
            }
        })),
    );
    assert_eq!(st_ba2, 200);

    // =========================================================================
    // UX01 从未上报 (Never Reported: observation_state="no_observation", value=null)
    // =========================================================================
    let (st_ux01, ux01_view) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=18000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(st_ux01, 200);
    // Bound gateway property before any telemetry arrives:
    assert_eq!(
        ux01_view["properties"]["temperature"]["binding_state"],
        "bound"
    );
    assert_eq!(
        ux01_view["properties"]["temperature"]["observation_state"],
        "no_observation"
    );
    assert!(
        ux01_view["properties"]["temperature"]["value"].is_null(),
        "UX01: Never-reported property value must be null, never 0 or false"
    );
    assert_eq!(
        ux01_view["properties"]["temperature"]["quality"],
        "unknown"
    );
    assert_eq!(
        ux01_view["properties"]["temperature"]["freshness"],
        "unknown"
    );
    // Static property:
    assert_eq!(
        ux01_view["properties"]["calibration_offset"]["binding_state"],
        "bound_static"
    );
    assert_eq!(
        ux01_view["properties"]["calibration_offset"]["observation_state"],
        "static_configured"
    );
    assert_eq!(
        ux01_view["properties"]["calibration_offset"]["value"],
        json!(0.5)
    );
    assert_eq!(
        ux01_view["properties"]["calibration_offset"]["freshness"],
        "static"
    );
    // Unbound property:
    assert_eq!(
        ux01_view["properties"]["maintenance_tag"]["binding_state"],
        "unbound"
    );
    assert_eq!(
        ux01_view["properties"]["maintenance_tag"]["observation_state"],
        "no_observation"
    );
    assert!(ux01_view["properties"]["maintenance_tag"]["value"].is_null());

    // =========================================================================
    // U05 首次上报 (Whole-Message Atomic 422 Rejection & Valid 202 Ingest)
    // =========================================================================
    // 1. Whole-batch rejection when one point is invalid (T02): 1st point valid, 2nd point unknown
    let (st_u05_bad, u05_bad_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "20010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u05-atomic-fail",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 23.5,
                    "source_at_ms": 20000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                },
                {
                    "property": "nonexistent_point",
                    "value": 1.0,
                    "source_at_ms": 20000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_u05_bad, 422);
    assert_eq!(u05_bad_body["error"]["code"], "VALIDATION_FAILED");

    // Verify zero side effects on sensor-a01 after atomic rejection
    let (_, view_after_reject) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=20020",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(
        view_after_reject["properties"]["temperature"]["observation_state"],
        "no_observation"
    );
    assert!(view_after_reject["properties"]["temperature"]["value"].is_null());

    // 2. Valid first telemetry report -> 202 Accepted + ingest_id
    let (st_u05_ok, u05_ok_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "100010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u05-first-ok",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 23.5,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_u05_ok, 202);
    assert_eq!(u05_ok_body["status"], "accepted_durable");
    let first_ingest_id = u05_ok_body["ingest_id"].as_str().unwrap().to_string();
    assert!(!first_ingest_id.is_empty());

    // =========================================================================
    // U06 查看与订阅 & UX02 good + fresh
    // =========================================================================
    let (st_ux02, ux02_view) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=105000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(st_ux02, 200);
    let temp_ux02 = &ux02_view["properties"]["temperature"];
    assert_eq!(temp_ux02["observation_state"], "observed");
    assert_eq!(temp_ux02["value"], json!(23.5));
    assert_eq!(temp_ux02["unit_code"], json!("Cel"));
    assert_eq!(temp_ux02["unit_symbol"], json!("°C"));
    assert_eq!(temp_ux02["quality"], json!("good"));
    assert_eq!(temp_ux02["freshness"], json!("fresh"));
    assert_eq!(temp_ux02["value_sampled_at_ms"], json!(100000));

    // WebSocket subscription: strictly snapshot_begin -> snapshot_chunk -> snapshot_end before delta
    let mut ws = ws_connect(addr, &zone_a_reader_token, 105500);
    write_ws_masked_text_frame(
        &mut ws,
        &json!({
            "type": "subscribe",
            "sub_id": "csub-u06-01",
            "resource_ids": ["res-sensor-a01-temperature"],
            "include_initial_snapshot": true,
            "max_pending_events": 16
        })
        .to_string(),
    )
    .unwrap();

    let f_begin: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    let f_chunk: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    let f_end: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    assert_eq!(f_begin["type"], "snapshot_begin");
    assert_eq!(f_chunk["type"], "snapshot_chunk");
    assert_eq!(f_chunk["items"][0]["resource_id"], "res-sensor-a01-temperature");
    assert_eq!(f_chunk["items"][0]["value"], json!(23.5));
    assert_eq!(f_end["type"], "snapshot_end");
    let snap_boundary = f_end["snapshot_boundary_seq"].as_u64().unwrap();

    // Ingest second sample at 106_000 -> triggers delta frame with stream_seq > snap_boundary
    let (st_u06_ing, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "106010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u06-second-ok",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 24.2,
                    "source_at_ms": 106000,
                    "source_time_origin": "device_clock",
                    "source_seq": "2",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_u06_ing, 202);

    let f_delta: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    assert_eq!(f_delta["type"], "delta");
    assert_eq!(f_delta["sub_id"], "csub-u06-01");
    assert_eq!(f_delta["resource_id"], "res-sensor-a01-temperature");
    assert_eq!(f_delta["value"], json!(24.2));
    assert_eq!(f_delta["quality"], json!("good"));
    assert_eq!(f_delta["observed_at_ms"], json!(106000));
    assert!(f_delta["stream_seq"].as_u64().unwrap() > snap_boundary);
    drop(ws);

    // =========================================================================
    // U07 历史确认 & UX08 accepted_durable + history_status
    // =========================================================================
    let (st_ing_stat, ing_stat_body) = http_call(
        addr,
        "GET",
        &format!("/v1/ingestions/{}", first_ingest_id),
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "107000")],
        None,
    );
    assert_eq!(st_ing_stat, 200);
    assert_eq!(ing_stat_body["status"], "accepted_durable");
    assert_eq!(ing_stat_body["history_status"], "committed");

    // UX08: Also verify that when an ingestion has history_status="pending" in ingest_status,
    // GET /v1/ingestions/{id} explicitly reports status="accepted_durable" and history_status="pending"
    {
        let g = srv.context().state.lock().unwrap();
        g.runtime
            .conn()
            .execute(
                "INSERT INTO ingest_status (ingest_id, tenant_id, message_id, status, history_status, wal_position, updated_at_ms)
                 VALUES ('ing-pending-ux08', 'ten-8iic', 'msg-pending-ux08', 'accepted_durable', 'pending', '0000000000000001:0000000000000100', 107000)",
                [],
            )
            .unwrap();
    }
    let (st_ux08_pend, ux08_pend_body) = http_call(
        addr,
        "GET",
        "/v1/ingestions/ing-pending-ux08",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "107100")],
        None,
    );
    assert_eq!(st_ux08_pend, 200);
    assert_eq!(ux08_pend_body["status"], "accepted_durable");
    assert_eq!(
        ux08_pend_body["history_status"], "pending",
        "UX08: Engine must distinguish local WAL durability (accepted_durable) from history projection readiness (pending)"
    );

    // Half-open range query [100_000, 106_000) -> includes 100_000, excludes 106_000, carries revisions
    let (st_u07_hist, u07_hist_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_zone_a), ("X-Engine-Now-Ms", "108000")],
        Some(&json!({
            "resource_ids": ["res-sensor-a01-temperature"],
            "range": {"start_ms": 100000, "end_ms": 106000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_u07_hist, 200);
    let u07_items = u07_hist_body["items"].as_array().unwrap();
    assert_eq!(
        u07_items.len(),
        1,
        "U07: Half-open range [100000, 106000) must include 100000 and exclude 106000"
    );
    assert_eq!(u07_items[0]["source_at_ms"], json!(100000));
    assert_eq!(u07_items[0]["value"], json!(23.5));
    assert_eq!(u07_items[0]["model_revision_id"], json!("temp-model-r1"));
    assert_eq!(u07_items[0]["binding_revision_id"], json!("bind-a01-r1"));
    assert_eq!(u07_items[0]["unit_code"], json!("Cel"));

    // =========================================================================
    // UX03 good + stale (Exceeding stale_threshold_ms=30_000 preserves value & original source_at_ms)
    // =========================================================================
    let (st_ux03, ux03_view) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=150000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(st_ux03, 200);
    let temp_ux03 = &ux03_view["properties"]["temperature"];
    assert_eq!(temp_ux03["quality"], json!("good"));
    assert_eq!(
        temp_ux03["freshness"],
        json!("stale"),
        "UX03: When now_ms - value_sampled_at_ms > stale_threshold_ms, freshness must be stale"
    );
    assert_eq!(
        temp_ux03["value"],
        json!(24.2),
        "UX03: Stale point must retain last observed value"
    );
    assert_eq!(
        temp_ux03["value_sampled_at_ms"],
        json!(106000),
        "UX03: Stale point must retain original source_at_ms and never overwrite it with current time"
    );

    // =========================================================================
    // UX04 点位 bad / uncertain (Explicit quality + reason; gateway reconnect does NOT auto-reset good)
    // =========================================================================
    let (st_ux04_bad, _) = http_call(
        addr,
        "POST",
        "/v1/source-state",
        &[("Authorization", &auth_gw), ("X-Engine-Now-Ms", "155010")],
        Some(&json!({
            "spec": "iotfast.source_state.v1",
            "event_id": "evt-ux04-point-bad",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "binding_revision_id": "bind-a01-r1",
            "observed_at_ms": 155000,
            "connection_state": "connected",
            "point_states": [
                {
                    "property": "temperature",
                    "quality_state": "bad",
                    "quality_reason": "sensor_open_circuit",
                    "source_protocol": "modbus",
                    "source_code": "0x02"
                }
            ]
        })),
    );
    assert_eq!(st_ux04_bad, 202);

    let (_, ux04_view1) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=156000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    let temp_ux04_1 = &ux04_view1["properties"]["temperature"];
    assert_eq!(temp_ux04_1["quality"], json!("bad"));
    assert_eq!(temp_ux04_1["quality_reason"], json!("sensor_open_circuit"));
    assert_eq!(temp_ux04_1["freshness"], json!("stale"));
    assert_eq!(temp_ux04_1["quality_observed_at_ms"], json!(155000));
    assert_eq!(
        temp_ux04_1["value"],
        json!(24.2),
        "UX04: Point quality failure must preserve last valid measurement value"
    );
    assert_eq!(
        temp_ux04_1["value_sampled_at_ms"],
        json!(106000),
        "UX04: Point quality failure must distinguish value_sampled_at_ms from quality_observed_at_ms"
    );

    // Gateway reconnect event without point recovery must NOT reset point quality to good!
    let (st_ux04_reconn, _) = http_call(
        addr,
        "POST",
        "/v1/source-state",
        &[("Authorization", &auth_gw), ("X-Engine-Now-Ms", "158010")],
        Some(&json!({
            "spec": "iotfast.source_state.v1",
            "event_id": "evt-ux04-gw-reconn",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "binding_revision_id": "bind-a01-r1",
            "observed_at_ms": 158000,
            "connection_state": "connected",
            "point_states": []
        })),
    );
    assert_eq!(st_ux04_reconn, 202);

    let (_, ux04_view2) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=158500",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(
        ux04_view2["properties"]["temperature"]["quality"],
        json!("bad"),
        "UX04: Gateway connection_state=connected must NOT automatically restore point quality to good"
    );

    // Now report a fresh good measurement at 160_000 -> restores point quality to good
    let (st_ux04_recover, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "160010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-ux04-recover-good",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 25.0,
                    "source_at_ms": 160000,
                    "source_time_origin": "device_clock",
                    "source_seq": "3",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ux04_recover, 202);

    // =========================================================================
    // UX05 网关 disconnected (Gateway disconnected marks freshness="stale" while keeping quality="good")
    // =========================================================================
    let (st_ux05_disc, _) = http_call(
        addr,
        "POST",
        "/v1/source-state",
        &[("Authorization", &auth_gw), ("X-Engine-Now-Ms", "162010")],
        Some(&json!({
            "spec": "iotfast.source_state.v1",
            "event_id": "evt-ux05-gw-disc",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "binding_revision_id": "bind-a01-r1",
            "observed_at_ms": 162000,
            "connection_state": "disconnected",
            "point_states": []
        })),
    );
    assert_eq!(st_ux05_disc, 202);

    let (_, ux05_view) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=163000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(ux05_view["source_connection_state"], json!("disconnected"));
    assert_eq!(
        ux05_view["properties"]["temperature"]["freshness"],
        json!("stale"),
        "UX05: Gateway disconnect must mark point freshness as stale"
    );
    assert_eq!(
        ux05_view["properties"]["temperature"]["quality"],
        json!("good"),
        "UX05: Without point fault evidence, gateway disconnect must NOT overwrite point quality to bad"
    );

    // Reconnect gateway at 164_000
    http_call(
        addr,
        "POST",
        "/v1/source-state",
        &[("Authorization", &auth_gw), ("X-Engine-Now-Ms", "164010")],
        Some(&json!({
            "spec": "iotfast.source_state.v1",
            "event_id": "evt-ux05-gw-reconn",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "binding_revision_id": "bind-a01-r1",
            "observed_at_ms": 164000,
            "connection_state": "connected",
            "point_states": []
        })),
    );

    // =========================================================================
    // UX06 未来时间异常 (Future Clock Skew Rejected with 422 CLOCK_SKEW_EXCEEDED; latest untouched)
    // =========================================================================
    let (st_ux06, ux06_err) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "165000"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-ux06-future-skew",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 99.9,
                    "source_at_ms": 300000,
                    "source_time_origin": "device_clock",
                    "source_seq": "4",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ux06, 422);
    assert_eq!(ux06_err["error"]["code"], "CLOCK_SKEW_EXCEEDED");
    assert!(!ux06_err["error"]["request_id"].as_str().unwrap().is_empty());
    assert_eq!(
        ux06_err["error"]["details"][0]["field"],
        "points[0].source_at_ms"
    );

    let (_, view_after_ux06) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=165100",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(
        view_after_ux06["properties"]["temperature"]["value"],
        json!(25.0),
        "UX06: Rejected future clock skew telemetry must not refresh latest value"
    );
    assert_eq!(
        view_after_ux06["properties"]["temperature"]["value_sampled_at_ms"],
        json!(160000)
    );

    // =========================================================================
    // UX07 accepted_volatile (Prefer: durability=volatile returns & stores accepted_volatile)
    // =========================================================================
    let (st_ux07, ux07_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=volatile"),
            ("X-Engine-Now-Ms", "166010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-ux07-volatile",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 25.5,
                    "source_at_ms": 166000,
                    "source_time_origin": "device_clock",
                    "source_seq": "5",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ux07, 202);
    assert_eq!(ux07_body["status"], "accepted_volatile");
    let vol_ingest_id = ux07_body["ingest_id"].as_str().unwrap().to_string();

    let (st_ux07_stat, ux07_stat_body) = http_call(
        addr,
        "GET",
        &format!("/v1/ingestions/{}", vol_ingest_id),
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "166020")],
        None,
    );
    assert_eq!(st_ux07_stat, 200);
    assert_eq!(
        ux07_stat_body["status"], "accepted_volatile",
        "UX07: Volatile ingest must never be disguised as accepted_durable"
    );

    // =========================================================================
    // UX09 历史为空且查询成功 (Empty History Query Returns 200, items=[], partial=false)
    // =========================================================================
    let (st_ux09, ux09_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_zone_a), ("X-Engine-Now-Ms", "167000")],
        Some(&json!({
            "resource_ids": ["res-sensor-a02-temperature"],
            "range": {"start_ms": 100000, "end_ms": 170000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_ux09, 200);
    assert_eq!(ux09_body["items"], json!([]));
    assert_eq!(ux09_body["partial"], json!(false));
    assert_eq!(ux09_body["known_gaps"], json!([]));

    // =========================================================================
    // UX10 历史 partial / 覆盖未知 (Delivery Gap -> partial=true & event_time_completeness="unknown")
    // =========================================================================
    {
        let g = srv.context().state.lock().unwrap();
        g.runtime
            .conn()
            .execute(
                "INSERT INTO delivery_gap (gap_id, tenant_id, resource_id, start_ms, end_ms, reason)
                 VALUES ('gap-ux10-01', 'ten-8iic', 'res-sensor-a01-temperature', 120000, 140000, 'gateway_buffer_overflow')",
                [],
            )
            .unwrap();
    }
    let (st_ux10, ux10_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_zone_a), ("X-Engine-Now-Ms", "168000")],
        Some(&json!({
            "resource_ids": ["res-sensor-a01-temperature"],
            "range": {"start_ms": 100000, "end_ms": 170000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 20,
            "cursor": null
        })),
    );
    assert_eq!(st_ux10, 200);
    assert_eq!(
        ux10_body["partial"],
        json!(true),
        "UX10: Overlapping delivery_gap must mark response partial=true"
    );
    assert_eq!(ux10_body["known_gaps"].as_array().unwrap().len(), 1);
    assert_eq!(
        ux10_body["known_gaps"][0]["reason"],
        json!("gateway_buffer_overflow")
    );
    assert_eq!(
        ux10_body["coverage"]["event_time_completeness"],
        json!("unknown"),
        "UX10: WAL checkpoint position must not be conflated with event-time completeness"
    );
    assert!(ux10_body["coverage"]["event_time_complete_through_ms"].is_null());
    assert!(ux10_body["coverage"]["contiguous_wal_position"].is_string());
    // Available observations (100000, 106000, 160000, 166000) are preserved without zero-filling
    assert_eq!(ux10_body["items"].as_array().unwrap().len(), 4);

    // =========================================================================
    // U08 异常演练 & UX11 订阅重连 / resync (Duplicate, Conflict, Out-of-Order, Overflow Resync, Expiry)
    // =========================================================================
    // 1. Duplicate retry with identical payload -> 200 duplicate, same ingest_id
    let (st_dup, dup_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "169000"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u05-first-ok",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 23.5,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_dup, 200);
    assert_eq!(dup_body["status"], "duplicate");
    assert_eq!(dup_body["ingest_id"], first_ingest_id);

    // 2. Idempotency conflict: same message_id with mutated payload -> 409 IDEMPOTENCY_CONFLICT
    let (st_conflict, conflict_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "169100"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u05-first-ok",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 99.0,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_conflict, 409);
    assert_eq!(conflict_body["error"]["code"], "IDEMPOTENCY_CONFLICT");

    // 3. Out-of-order backfill (source_at_ms=130_000 < latest 166_000): stored in history, does NOT overwrite latest
    let (st_ooo, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "169500"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u08-ooo-backfill",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 21.0,
                    "source_at_ms": 130000,
                    "source_time_origin": "device_clock",
                    "source_seq": "10",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": true}
                }
            ]
        })),
    );
    assert_eq!(st_ooo, 202);

    let (_, view_after_ooo) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-a01/view?now_ms=169600",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(
        view_after_ooo["properties"]["temperature"]["value"],
        json!(25.5),
        "U08: Out-of-order older measurement must not overwrite newer latest value"
    );
    assert_eq!(
        view_after_ooo["properties"]["temperature"]["value_sampled_at_ms"],
        json!(166000)
    );

    // 4. UX11: Subscription buffer overflow emits resync_required(BUFFER_OVERFLOW), then client resyncs
    let mut ws_ux11 = ws_connect(addr, &zone_a_reader_token, 170000);
    write_ws_masked_text_frame(
        &mut ws_ux11,
        &json!({
            "type": "subscribe",
            "sub_id": "csub-ux11-overflow",
            "resource_ids": ["res-sensor-a01-temperature"],
            "include_initial_snapshot": true,
            "max_pending_events": 2
        })
        .to_string(),
    )
    .unwrap();
    // Drain initial snapshot (3 frames)
    for expected_type in ["snapshot_begin", "snapshot_chunk", "snapshot_end"] {
        let f: Value =
            serde_json::from_str(&read_ws_text_frame(&mut ws_ux11).unwrap().unwrap()).unwrap();
        assert_eq!(f["type"], expected_type);
    }

    // Trigger 3 pending deltas in a single lock acquisition (exceeding max_pending_events=2)
    {
        let mut g = srv.context().state.lock().unwrap();
        let base_seq = g.runtime.stream_seq;
        for i in 1..=3 {
            g.hub.publish_point_delta(
                "ten-8iic",
                "res-sensor-a01-temperature",
                base_seq + i,
                json!(26.0 + i as f64),
                "good",
                170000 + (i as i64) * 100,
            );
        }
        srv.context().ws_notify.notify_all();
    }

    let f_resync: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws_ux11).unwrap().unwrap()).unwrap();
    assert_eq!(f_resync["type"], "resync_required");
    assert_eq!(f_resync["sub_id"], "csub-ux11-overflow");
    assert_eq!(f_resync["reason"], "BUFFER_OVERFLOW");
    assert_eq!(
        f_resync["action"],
        "reconnect_and_resubscribe_with_snapshot"
    );
    drop(ws_ux11);

    // Reconnect and verify clean snapshot recovery (UX11) and credential expiry termination (U08)
    let mut ws_reconn = ws_connect(addr, &zone_a_reader_token, 171000);
    write_ws_masked_text_frame(
        &mut ws_reconn,
        &json!({
            "type": "subscribe",
            "sub_id": "csub-ux11-recovered",
            "resource_ids": ["res-sensor-a01-temperature"],
            "include_initial_snapshot": true,
            "max_pending_events": 16
        })
        .to_string(),
    )
    .unwrap();
    for expected_type in ["snapshot_begin", "snapshot_chunk", "snapshot_end"] {
        let f: Value =
            serde_json::from_str(&read_ws_text_frame(&mut ws_reconn).unwrap().unwrap()).unwrap();
        assert_eq!(f["type"], expected_type);
    }

    // U08: When subscriber token expires (observed_at_ms > 500_000_000), emit subscription_terminated(CREDENTIAL_EXPIRED)
    let (st_exp_ing, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "500001010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-u08-post-expiry",
            "source_id": "gw-ux-01",
            "source_epoch": "epoch-ux-01",
            "device_id": "sensor-a01",
            "model_revision_id": "temp-model-r1",
            "binding_revision_id": "bind-a01-r1",
            "points": [
                {
                    "property": "temperature",
                    "value": 26.8,
                    "source_at_ms": 500001000,
                    "source_time_origin": "device_clock",
                    "source_seq": "20",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_exp_ing, 202);

    let f_term: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws_reconn).unwrap().unwrap()).unwrap();
    assert_eq!(f_term["type"], "subscription_terminated");
    assert_eq!(f_term["reason"], "CREDENTIAL_EXPIRED");
    assert_eq!(f_term["action"], "reauthenticate_and_resubscribe");
    drop(ws_reconn);

    // =========================================================================
    // UX12 无权限或不存在 (Uniform 404 RESOURCE_NOT_FOUND with empty details=[])
    // =========================================================================
    // 1. Existing device outside caller Scope (sensor-b01 in zone-b) -> 404 with details=[]
    let (st_unauth_dev, err_unauth_dev) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-b01/view?now_ms=172000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(st_unauth_dev, 404);
    assert_eq!(err_unauth_dev["error"]["code"], "RESOURCE_NOT_FOUND");
    assert_eq!(err_unauth_dev["error"]["details"], json!([]));

    // 2. Non-existent device -> identical 404 with details=[]
    let (st_missing_dev, err_missing_dev) = http_call(
        addr,
        "GET",
        "/v1/devices/sensor-does-not-exist/view?now_ms=172000",
        &[("Authorization", &auth_zone_a)],
        None,
    );
    assert_eq!(st_missing_dev, 404);
    assert_eq!(err_missing_dev["error"]["code"], "RESOURCE_NOT_FOUND");
    assert_eq!(err_missing_dev["error"]["details"], json!([]));

    // 3. Batch history query mixing authorized + unauthorized resource_ids -> 404 with details=[] (R05)
    let (st_batch_404, err_batch_404) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_zone_a), ("X-Engine-Now-Ms", "172000")],
        Some(&json!({
            "resource_ids": ["res-sensor-a01-temperature", "res-sensor-b01-temperature"],
            "range": {"start_ms": 100000, "end_ms": 170000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_batch_404, 404);
    assert_eq!(err_batch_404["error"]["code"], "RESOURCE_NOT_FOUND");
    assert_eq!(
        err_batch_404["error"]["details"],
        json!([]),
        "UX12 / R05: Batch query with unauthorized resource must not leak which IDs exist or failed"
    );

    // 4. Directory listing under plant-1 with zone-a scoped token only returns zone-a resources (never leaks zone-b count)
    let (st_dir_scoped, dir_scoped_body) = http_call(
        addr,
        "GET",
        "/v1/resources?asset_id=asset-plant-1&include_descendants=true",
        &[("Authorization", &auth_zone_a), ("X-Engine-Now-Ms", "172000")],
        None,
    );
    assert_eq!(st_dir_scoped, 200);
    let scoped_items = dir_scoped_body["items"].as_array().unwrap();
    assert!(scoped_items
        .iter()
        .all(|item| item["device_id"] != "sensor-b01"));

    srv.shutdown();
}
