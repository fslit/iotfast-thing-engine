use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::server::{
    read_ws_text_frame, write_ws_masked_text_frame, IoStream, ServerHandle,
};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};

const TEST_SECRET: &str = "mvp-test-hmac-secret-2026";

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
fn mvp_thing_model_end_to_end_http_and_websocket_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut srv = ServerHandle::spawn(cfg, TEST_SECRET).unwrap();
    let addr = &srv;

    // 1. Health endpoint
    let (st, health) = http_call(addr, "GET", "/health", &[], None);
    assert_eq!(st, 200);
    assert_eq!(health["duckdb_version"], "v2.0.0");

    // Mint signed Scope tokens (Q02)
    let mgmt_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-mvp".into(),
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
        operator_id: Some("op-architect".into()),
    }
    .issue_signed_token(TEST_SECRET);

    let gw_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-mvp".into(),
        caller_service_id: "gw-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_000,
        expires_at_ms: 500_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: None,
    }
    .issue_signed_token(TEST_SECRET);

    let room1_only_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-mvp".into(),
        caller_service_id: "svc-room1-reader".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 500_000_000,
        allowed_actions: vec!["catalog:read".into(), "view:read".into()],
        scope_filter: ScopeFilter {
            asset_ids: vec!["asset-room-1".into()],
            include_asset_descendants: true,
            ..Default::default()
        },
        operator_id: None,
    }
    .issue_signed_token(TEST_SECRET);

    let auth_mgmt = format!("Bearer {}", mgmt_token);
    let auth_gw = format!("Bearer {}", gw_token);
    let auth_room1 = format!("Bearer {}", room1_only_token);

    // 2. Reject forged token (401) and forbidden client-reported X-Tenant-Id header (422 SC02)
    let (st_forged, _) = http_call(
        addr,
        "GET",
        "/v1/products",
        &[("Authorization", "Bearer iotfast1.eyJ0ZW5hbnRfaWQiOiJ0ZW4tOGlpYyJ9.deadbeef")],
        None,
    );
    assert_eq!(st_forged, 401);

    let (st_forbidden_hdr, err_hdr) = http_call(
        addr,
        "GET",
        "/v1/products",
        &[
            ("Authorization", &auth_mgmt),
            ("X-Tenant-Id", "ten-hijack"),
            ("X-Engine-Now-Ms", "10000"),
        ],
        None,
    );
    assert_eq!(st_forbidden_hdr, 422);
    assert_eq!(err_hdr["error"]["code"], "VALIDATION_FAILED");

    // 3. Product & Model Lifecycle (D02)
    let (st_prod, _) = http_call(
        addr,
        "POST",
        "/v1/products",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "10000")],
        Some(&json!({
            "product_id": "prod-pump",
            "product_key": "booster-pump",
            "name": "变频增压水泵"
        })),
    );
    assert_eq!(st_prod, 201);

    let (st_m1, _) = http_call(
        addr,
        "POST",
        "/v1/products/prod-pump/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "10000")],
        Some(&json!({
            "model_revision_id": "pump-model-r1",
            "model_version": "1.0.0",
            "properties": [
                {
                    "identifier": "outlet_pressure",
                    "name": "出口压力",
                    "value_type": "float64",
                    "unit_code": "MPa",
                    "unit_symbol": "MPa",
                    "min_value": 0.0,
                    "max_value": 4.0,
                    "required": true
                },
                {
                    "identifier": "rated_power",
                    "name": "额定功率",
                    "value_type": "float64",
                    "unit_code": "kW",
                    "unit_symbol": "kW",
                    "min_value": 0.0,
                    "max_value": 500.0,
                    "required": false
                },
                {
                    "identifier": "run_mode",
                    "name": "运行模式",
                    "value_type": "text",
                    "unit_code": null,
                    "unit_symbol": null,
                    "min_value": null,
                    "max_value": null,
                    "required": false
                },
                {
                    "identifier": "a_b",
                    "name": "碰撞测试点1",
                    "value_type": "float64",
                    "unit_code": null,
                    "unit_symbol": null,
                    "min_value": null,
                    "max_value": null,
                    "required": false
                },
                {
                    "identifier": "a-b",
                    "name": "碰撞测试点2",
                    "value_type": "float64",
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

    let (st_v1, _) = http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "11000")],
        None,
    );
    assert_eq!(st_v1, 200);

    let (st_p1, _) = http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r1:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12000")],
        Some(&json!({"published_at_ms": 12000})),
    );
    assert_eq!(st_p1, 200);

    // Create compatible r2 (adds optional motor_current) and breaking r3 (changes unit of outlet_pressure)
    let mut r2_props = http_call(
        addr,
        "GET",
        "/v1/models/pump-model-r1",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12500")],
        None,
    )
    .1["properties"]
        .as_array()
        .unwrap()
        .clone();
    r2_props.push(json!({
        "identifier": "motor_current",
        "name": "电机电流",
        "value_type": "float64",
        "unit_code": "A",
        "unit_symbol": "A",
        "min_value": 0.0,
        "max_value": 200.0,
        "required": false
    }));
    http_call(
        addr,
        "POST",
        "/v1/products/prod-pump/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "13000")],
        Some(&json!({
            "model_revision_id": "pump-model-r2",
            "model_version": "1.1.0",
            "properties": r2_props
        })),
    );
    let (st_cmp, cmp_body) = http_call(
        addr,
        "POST",
        "/v1/models:compare",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "13500")],
        Some(&json!({
            "from_revision_id": "pump-model-r1",
            "to_revision_id": "pump-model-r2"
        })),
    );
    assert_eq!(st_cmp, 200);
    assert_eq!(cmp_body["compatibility"], "compatible_extension");
    assert_eq!(cmp_body["added_properties"], json!(["motor_current"]));

    http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r2:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "14000")],
        Some(&json!({"previous_published_revision_id": "pump-model-r1"})),
    );
    http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r2:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15000")],
        Some(&json!({"published_at_ms": 15000})),
    );

    // Create breaking r3 (changes outlet_pressure unit to bar) and publish it as an independent revision
    let mut r3_props = r2_props.clone();
    r3_props.iter_mut().for_each(|p| {
        if p["identifier"] == "outlet_pressure" {
            p["unit_code"] = json!("bar");
        }
    });
    http_call(
        addr,
        "POST",
        "/v1/products/prod-pump/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15500")],
        Some(&json!({
            "model_revision_id": "pump-model-r3-breaking",
            "model_version": "2.0.0",
            "properties": r3_props
        })),
    );
    http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r3-breaking:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15600")],
        None,
    );
    http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r3-breaking:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15700")],
        None,
    );

    // 4. Asset Tree & Cross-Tree Topology Relations (D03)
    for asset_req in [
        json!({"asset_id": "asset-plant-a", "parent_asset_id": null, "slug": "plant-a", "name": "一厂区"}),
        json!({"asset_id": "asset-room-1", "parent_asset_id": "asset-plant-a", "slug": "room-1", "name": "一号泵房"}),
        json!({"asset_id": "asset-room-2", "parent_asset_id": "asset-plant-a", "slug": "room-2", "name": "二号泵房"}),
        json!({"asset_id": "asset-header-1", "parent_asset_id": "asset-plant-a", "slug": "header-1", "name": "总管网"}),
    ] {
        let (st, _) = http_call(
            addr,
            "POST",
            "/v1/assets",
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16000")],
            Some(&asset_req),
        );
        assert_eq!(st, 201);
    }

    // Create cross-tree topology relation: room-1 FEEDS header-1
    let (st_rel, rel_body) = http_call(
        addr,
        "POST",
        "/v1/assets/relations",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16500")],
        Some(&json!({
            "relation_id": "rel-room1-feeds-header1",
            "from_asset_id": "asset-room-1",
            "relation_type": "FEEDS",
            "to_asset_id": "asset-header-1",
            "valid_from_ms": 16000,
            "valid_to_ms": null,
            "source": "engineering_design"
        })),
    );
    assert_eq!(st_rel, 201);
    assert_eq!(rel_body["relation_type"], "FEEDS");

    // Management token sees the relation; room1_only_token (lacking header-1 in Scope) does NOT see it!
    let (_, rels_mgmt) = http_call(
        addr,
        "GET",
        "/v1/assets/asset-room-1/relations?now_ms=17000",
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(rels_mgmt["items"].as_array().unwrap().len(), 1);

    let (_, rels_room1) = http_call(
        addr,
        "GET",
        "/v1/assets/asset-room-1/relations?now_ms=17000",
        &[("Authorization", &auth_room1)],
        None,
    );
    assert_eq!(
        rels_room1["items"].as_array().unwrap().len(),
        0,
        "Cross-tree relation must be filtered when target asset is outside caller Scope"
    );

    // 5. Instantiate device pump-001 (verifying Q11 resource_id collision fallback for a_b and a-b)
    let (st_dev, dev_body) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "18000")],
        Some(&json!({
            "device_id": "pump-001",
            "device_key": "pump-001",
            "device_name": "1号增压泵",
            "product_id": "prod-pump",
            "model_revision_id": "pump-model-r1",
            "asset_id": "asset-room-1"
        })),
    );
    assert_eq!(st_dev, 201);
    assert_eq!(dev_body["resources"].as_array().unwrap().len(), 5);

    // Relocate pump-001 from room-1 to room-2, then verify Q06 permanent historical path protection
    let old_uri = "uns://ten-8iic/plant-a/room-1/pump-001/properties/outlet_pressure";
    let (st_reloc, _) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-001:relocate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "20000")],
        Some(&json!({
            "target_asset_id": "asset-room-2",
            "relocated_at_ms": 20000,
            "redirect_retention_ms": 10000
        })),
    );
    assert_eq!(st_reloc, 200);

    // Even AFTER redirect_until_ms (30000) expires at t=50000, another device MUST NOT hijack old_uri (Q06)
    let (st_hijack, _) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "50000")],
        Some(&json!({
            "device_id": "pump-hijack",
            "device_key": "pump-001",
            "device_name": "抢占设备",
            "product_id": "prod-pump",
            "model_revision_id": "pump-model-r1",
            "asset_id": "asset-room-1"
        })),
    );
    assert_eq!(
        st_hijack, 409,
        "Q06: Historical canonical_uri must never be hijacked by another resource_id even after redirect window"
    );

    // But pump-001 itself CAN relocate back to room-1 and reclaim its own canonical_uri!
    let (st_reloc_back, _) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-001:relocate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "55000")],
        Some(&json!({
            "target_asset_id": "asset-room-1",
            "relocated_at_ms": 55000,
            "redirect_retention_ms": 10000
        })),
    );
    assert_eq!(st_reloc_back, 200);
    let (st_res_uri, res_uri_body) = http_call(
        addr,
        "GET",
        &format!("/v1/resources?uri={}&now_ms=56000", old_uri),
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(st_res_uri, 200);
    assert_eq!(res_uri_body["matched_path_state"], "canonical");

    // 6. Smooth Device Model Upgrade (AT04 / BM01): reject breaking r3 (422), allow compatible r2 (200)
    let (st_upg_break, _) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-001:upgrade-model",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "60000")],
        Some(&json!({
            "target_model_revision_id": "pump-model-r3-breaking",
            "expected_device_revision": 3,
            "upgraded_at_ms": 60000
        })),
    );
    assert_eq!(st_upg_break, 422);

    let (st_upg_ok, upg_body) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-001:upgrade-model",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "61000")],
        Some(&json!({
            "target_model_revision_id": "pump-model-r2",
            "expected_device_revision": 3,
            "upgraded_at_ms": 61000
        })),
    );
    assert_eq!(st_upg_ok, 200);
    assert_eq!(upg_body["model_revision_id"], "pump-model-r2");
    assert_eq!(upg_body["revision"], 4);
    assert_eq!(upg_body["resources"].as_array().unwrap().len(), 6);

    // 7. Property Data References: Gateway Measurement + Static Config + Manual Stop Window (D04 / Q09 / AT03)
    http_call(
        addr,
        "POST",
        "/v1/sources/grants",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "62000")],
        Some(&json!({
            "source_id": "gw-01",
            "device_id": "pump-001",
            "valid_from_ms": 62000
        })),
    );

    let (st_bind, _) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-001/bindings",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "63000")],
        Some(&json!({
            "binding_revision_id": "bind-pump-r1",
            "model_revision_id": "pump-model-r2",
            "source_id": "gw-01",
            "normalization_owner": "gateway",
            "server_scale_factor": null,
            "points": [
                {
                    "property": "outlet_pressure",
                    "register_address": "40001",
                    "factor": 0.01,
                    "unit_code": "MPa",
                    "deadband": 0.02,
                    "heartbeat_ms": 60000
                },
                {
                    "property": "motor_current",
                    "register_address": "40002",
                    "factor": 0.1,
                    "unit_code": "A",
                    "deadband": 0.5,
                    "heartbeat_ms": 60000
                }
            ],
            "static_points": [
                {
                    "property": "rated_power",
                    "value": 45.0,
                    "unit_code": "kW"
                }
            ]
        })),
    );
    assert_eq!(st_bind, 201);

    http_call(
        addr,
        "POST",
        "/v1/bindings/bind-pump-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "64000")],
        None,
    );

    // Activate via Q09 manual_stop_window
    let (st_act, act_body) = http_call(
        addr,
        "POST",
        "/v1/bindings/bind-pump-r1:activate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "65000")],
        Some(&json!({
            "expected_device_revision": 4,
            "activated_at_ms": 65000,
            "manual_stop_window": {
                "window_start_ms": 64500,
                "window_end_ms": 66000,
                "operator_id": "op-maintenance-lead"
            }
        })),
    );
    assert_eq!(st_act, 200);
    assert_eq!(act_body["status"], "active");

    // 7b. Verify Q02 / S1: Telemetry before source_device_grant.valid_from_ms (62000) is rejected with 403!
    let (st_early_grant, early_grant_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "61500"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-early-grant",
            "source_id": "gw-01",
            "source_epoch": "epoch-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r2",
            "binding_revision_id": "bind-pump-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.60,
                    "source_at_ms": 61500,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_early_grant, 403);
    assert_eq!(early_grant_body["error"]["code"], "SOURCE_NOT_AUTHORIZED");

    // 8. Connect WebSocket subscriber BEFORE telemetry ingest, verify snapshot + live delta!
    let mut ws = ws_connect(addr, &mgmt_token, 66000);
    write_ws_masked_text_frame(
        &mut ws,
        &json!({
            "type": "subscribe",
            "sub_id": "csub-live-01",
            "resource_ids": ["res-pump-001-outlet-pressure"],
            "include_initial_snapshot": true,
            "max_pending_events": 16
        })
        .to_string(),
    )
    .unwrap();

    let f_begin: Value = serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    let f_chunk: Value = serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    let f_end: Value = serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    assert_eq!(f_begin["type"], "snapshot_begin");
    assert_eq!(f_chunk["type"], "snapshot_chunk");
    assert_eq!(f_end["type"], "snapshot_end");

    // 9. Ingest telemetry via HTTP POST /v1/telemetry
    let (st_ing, ing_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "70010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-http-001",
            "source_id": "gw-01",
            "source_epoch": "epoch-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r2",
            "binding_revision_id": "bind-pump-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.65,
                    "source_at_ms": 70000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                },
                {
                    "property": "motor_current",
                    "value": 32.4,
                    "source_at_ms": 70000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ing, 202);
    assert_eq!(ing_body["status"], "accepted_durable");
    let durable_ingest_id = ing_body["ingest_id"].as_str().unwrap().to_string();

    // Verify Q12: GET /v1/ingestions/{ingest_id} returns status=accepted_durable & history_status=committed
    let (st_ing_stat, ing_stat_body) = http_call(
        addr,
        "GET",
        &format!("/v1/ingestions/{}", durable_ingest_id),
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "70020")],
        None,
    );
    assert_eq!(st_ing_stat, 200);
    assert_eq!(ing_stat_body["status"], "accepted_durable");
    assert_eq!(ing_stat_body["history_status"], "committed");

    // Verify WebSocket subscriber immediately receives the live delta frame!
    let f_delta: Value = serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    assert_eq!(f_delta["type"], "delta");
    assert_eq!(f_delta["sub_id"], "csub-live-01");
    assert_eq!(f_delta["resource_id"], "res-pump-001-outlet-pressure");
    assert_eq!(f_delta["value"], json!(1.65));

    // Verify Q07 / Q12: Volatile telemetry stores status="accepted_volatile" in ingest_status
    let (st_vol, vol_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=volatile"),
            ("X-Engine-Now-Ms", "70100"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-http-vol-01",
            "source_id": "gw-01",
            "source_epoch": "epoch-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r2",
            "binding_revision_id": "bind-pump-r1",
            "points": [
                {
                    "property": "motor_current",
                    "value": 33.1,
                    "source_at_ms": 70090,
                    "source_time_origin": "device_clock",
                    "source_seq": "2",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_vol, 202);
    assert_eq!(vol_body["status"], "accepted_volatile");
    let vol_ingest_id = vol_body["ingest_id"].as_str().unwrap().to_string();
    let (st_vol_stat, vol_stat_body) = http_call(
        addr,
        "GET",
        &format!("/v1/ingestions/{}", vol_ingest_id),
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "70110")],
        None,
    );
    assert_eq!(st_vol_stat, 200);
    assert_eq!(vol_stat_body["status"], "accepted_volatile");
    assert_eq!(vol_stat_body["history_status"], "committed");

    // 10. Query DeviceView & ShadowView: verify gateway_measurement, static_config, and unbound properties!
    let (st_view, view_body) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-001/view?now_ms=70500",
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(st_view, 200);
    assert_eq!(
        view_body["properties"]["outlet_pressure"]["binding_state"],
        "bound"
    );
    assert_eq!(
        view_body["properties"]["outlet_pressure"]["observation_state"],
        "observed"
    );
    assert_eq!(view_body["properties"]["outlet_pressure"]["value"], json!(1.65));
    assert_eq!(
        view_body["properties"]["outlet_pressure"]["freshness"],
        "fresh"
    );

    assert_eq!(
        view_body["properties"]["rated_power"]["binding_state"],
        "bound_static"
    );
    assert_eq!(
        view_body["properties"]["rated_power"]["observation_state"],
        "static_configured"
    );
    assert_eq!(view_body["properties"]["rated_power"]["value"], json!(45.0));
    assert_eq!(
        view_body["properties"]["rated_power"]["freshness"],
        "static"
    );
    assert_eq!(
        view_body["properties"]["rated_power"]["source_time_origin"],
        "static_config"
    );

    assert_eq!(
        view_body["properties"]["run_mode"]["binding_state"],
        "unbound"
    );
    assert_eq!(
        view_body["properties"]["run_mode"]["observation_state"],
        "no_observation"
    );
    assert!(view_body["properties"]["run_mode"]["value"].is_null());

    // 11. Query History & Resource Directory
    let (st_hist, hist_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "71000")],
        Some(&json!({
            "resource_ids": ["res-pump-001-outlet-pressure", "res-pump-001-motor-current"],
            "range": {"start_ms": 60000, "end_ms": 80000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_hist, 200);
    assert_eq!(hist_body["items"].as_array().unwrap().len(), 3);

    // Verify Q08: History query pagination cursor with watermark_received_at_ms stability
    let (st_p1, p1_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "71100")],
        Some(&json!({
            "resource_ids": ["res-pump-001-outlet-pressure", "res-pump-001-motor-current"],
            "range": {"start_ms": 60000, "end_ms": 80000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 1,
            "cursor": null
        })),
    );
    assert_eq!(st_p1, 200);
    assert_eq!(p1_body["items"].as_array().unwrap().len(), 1);
    let cursor_p1 = p1_body["next_cursor"].as_str().unwrap().to_string();

    // Concurrent late write arrives at received_at_ms = 71500 (after watermark_received_at_ms = 70100)
    // with source_at_ms = 70050 (which falls inside the range before item #3 at 70090).
    let (st_late, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "71500"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-http-late-01",
            "source_id": "gw-01",
            "source_epoch": "epoch-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r2",
            "binding_revision_id": "bind-pump-r1",
            "points": [
                {
                    "property": "motor_current",
                    "value": 34.0,
                    "source_at_ms": 70050,
                    "source_time_origin": "device_clock",
                    "source_seq": "3",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": true}
                }
            ]
        })),
    );
    assert_eq!(st_late, 202);

    // Page 2 using cursor_p1 must only see records with received_at_ms <= watermark (70100),
    // so the 2 remaining records from the original 3-record snapshot are returned!
    let (st_p2, p2_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "71600")],
        Some(&json!({
            "resource_ids": ["res-pump-001-outlet-pressure", "res-pump-001-motor-current"],
            "range": {"start_ms": 60000, "end_ms": 80000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": cursor_p1
        })),
    );
    assert_eq!(st_p2, 200);
    let p2_items = p2_body["items"].as_array().unwrap();
    assert_eq!(
        p2_items.len(),
        2,
        "Q08: Late arrival after watermark_received_at_ms must not pollute in-flight pagination"
    );
    assert!(p2_items.iter().all(|it| it["received_at_ms"].as_i64().unwrap() <= 70100));

    let (st_dir, dir_body) = http_call(
        addr,
        "GET",
        "/v1/resources?asset_id=asset-plant-a&include_descendants=true",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "72000")],
        None,
    );
    assert_eq!(st_dir, 200);
    assert_eq!(dir_body["items"].as_array().unwrap().len(), 6);

    // 12. Verify Q05 / SC10: When subscriber token expires (observed_at_ms > 500_000_000),
    // WebSocket wire emits `subscription_terminated` with `reason = "CREDENTIAL_EXPIRED"`!
    let gw_long_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-mvp".into(),
        caller_service_id: "gw-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_000,
        expires_at_ms: 900_000_000,
        allowed_actions: vec!["telemetry:ingest".into()],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: None,
    }
    .issue_signed_token(TEST_SECRET);
    let auth_gw_long = format!("Bearer {}", gw_long_token);

    let (st_ing2, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw_long),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "500001010"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-http-002",
            "source_id": "gw-01",
            "source_epoch": "epoch-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r2",
            "binding_revision_id": "bind-pump-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.85,
                    "source_at_ms": 500001000,
                    "source_time_origin": "device_clock",
                    "source_seq": "2",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ing2, 202);

    let f_term: Value =
        serde_json::from_str(&read_ws_text_frame(&mut ws).unwrap().unwrap()).unwrap();
    assert_eq!(f_term["type"], "subscription_terminated");
    assert_eq!(f_term["reason"], "CREDENTIAL_EXPIRED");
    assert_eq!(f_term["action"], "reauthenticate_and_resubscribe");

    srv.shutdown();

    // 13. Verify Q10: EngineConfig::validate() rejects invalid watermarks/bounds
    let mut bad_cfg = EngineConfig::for_test(tmp.path());
    assert!(bad_cfg.validate().is_ok());
    bad_cfg.wal_soft_watermark_bytes = 512 * 1024;
    bad_cfg.wal_hard_watermark_bytes = 128 * 1024;
    assert!(bad_cfg.validate().is_err());
}
