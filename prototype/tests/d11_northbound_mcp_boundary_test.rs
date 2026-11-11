use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::server::ServerHandle;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};

const TEST_SECRET: &str = "d11-mcp-boundary-hmac-secret-2026";

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

#[test]
fn d11_northbound_ai_mcp_readonly_and_scope_boundary_verification() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut srv = ServerHandle::spawn(cfg, TEST_SECRET).unwrap();
    let addr = &srv;

    // 1. Issue tokens:
    //    - Control-plane management token (for test fixture setup)
    //    - Southbound gateway token (for initial telemetry fixture)
    //    - Restricted Northbound AI MCP Service Token:
    //      ONLY allowed_actions = ["view:read", "history:query"]
    //      ONLY scoped to device `pump-001` property `outlet_pressure` (`res-pump-001-outlet-pressure`)
    let mgmt_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-d11".into(),
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
        ],
        scope_filter: ScopeFilter {
            tenant_wide: true,
            ..Default::default()
        },
        operator_id: Some("op-admin".into()),
    }
    .issue_signed_token(TEST_SECRET);

    let gw_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-d11".into(),
        caller_service_id: "gw-d11-01".into(),
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

    let ai_mcp_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-d11".into(),
        caller_service_id: "svc-ai-mcp-readonly".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 500_000_000,
        allowed_actions: vec!["view:read".into(), "history:query".into()],
        scope_filter: ScopeFilter {
            asset_ids: vec![],
            include_asset_descendants: false,
            device_ids: vec![],
            resource_ids: vec!["res-pump-001-outlet-pressure".into()],
            uri_prefixes: vec![
                "uns://ten-8iic/plant-a/room-1/pump-001/properties/outlet_pressure".into(),
            ],
            tenant_wide: false,
        },
        operator_id: None,
    }
    .issue_signed_token(TEST_SECRET);

    let auth_mgmt = format!("Bearer {}", mgmt_token);
    let auth_gw = format!("Bearer {}", gw_token);
    let auth_ai = format!("Bearer {}", ai_mcp_token);

    // 2. Setup fixture: product, published model (outlet_pressure, motor_current, rated_power),
    //    assets, pump-001 and pump-002, bindings, and telemetry observations
    let (st, _) = http_call(
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
    assert_eq!(st, 201);

    let (st, _) = http_call(
        addr,
        "POST",
        "/v1/products/prod-pump/models",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "10500")],
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
                    "identifier": "motor_current",
                    "name": "电机电流",
                    "value_type": "float64",
                    "unit_code": "A",
                    "unit_symbol": "A",
                    "min_value": 0.0,
                    "max_value": 200.0,
                    "required": false
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
                }
            ]
        })),
    );
    assert_eq!(st, 201);

    http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "11000")],
        None,
    );
    http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r1:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12000")],
        Some(&json!({"published_at_ms": 12000})),
    );

    for asset_req in [
        json!({"asset_id": "asset-plant-a", "parent_asset_id": null, "slug": "plant-a", "name": "一厂区"}),
        json!({"asset_id": "asset-room-1", "parent_asset_id": "asset-plant-a", "slug": "room-1", "name": "一号泵房"}),
        json!({"asset_id": "asset-room-2", "parent_asset_id": "asset-plant-a", "slug": "room-2", "name": "二号泵房"}),
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

    for (dev_id, dev_name, asset_id) in [
        ("pump-001", "1号增压泵", "asset-room-1"),
        ("pump-002", "2号增压泵", "asset-room-2"),
    ] {
        let (st, _) = http_call(
            addr,
            "POST",
            "/v1/devices",
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "14000")],
            Some(&json!({
                "device_id": dev_id,
                "device_key": dev_id,
                "device_name": dev_name,
                "product_id": "prod-pump",
                "model_revision_id": "pump-model-r1",
                "asset_id": asset_id
            })),
        );
        assert_eq!(st, 201);

        http_call(
            addr,
            "POST",
            "/v1/sources/grants",
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15000")],
            Some(&json!({
                "source_id": "gw-d11-01",
                "device_id": dev_id,
                "valid_from_ms": 15000
            })),
        );

        let bind_id = format!("bind-{}-r1", dev_id);
        let (st_b, b_draft) = http_call(
            addr,
            "POST",
            &format!("/v1/devices/{}/bindings", dev_id),
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16000")],
            Some(&json!({
                "binding_revision_id": bind_id,
                "model_revision_id": "pump-model-r1",
                "source_id": "gw-d11-01",
                "normalization_owner": "gateway",
                "server_scale_factor": null,
                "points": [
                    {
                        "property": "outlet_pressure",
                        "register_address": "40001",
                        "factor": 0.01,
                        "unit_code": "MPa",
                        "deadband": 0.01,
                        "heartbeat_ms": 60000
                    },
                    {
                        "property": "motor_current",
                        "register_address": "40002",
                        "factor": 0.1,
                        "unit_code": "A",
                        "deadband": 0.1,
                        "heartbeat_ms": 60000
                    }
                ],
                "static_points": [
                    {
                        "property": "rated_power",
                        "value": 55.0,
                        "unit_code": "kW"
                    }
                ]
            })),
        );
        assert_eq!(st_b, 201);

        http_call(
            addr,
            "POST",
            &format!("/v1/bindings/{}:validate", bind_id),
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16500")],
            None,
        );
        let (st_act, _) = http_call(
            addr,
            "POST",
            &format!("/v1/bindings/{}:activate", bind_id),
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "17000")],
            Some(&json!({
                "expected_device_revision": 1,
                "activated_at_ms": 17000,
                "gateway_ack": {
                    "source_id": "gw-d11-01",
                    "source_epoch": "epoch-d11-01",
                    "binding_revision_id": bind_id,
                    "config_hash": b_draft["config_hash"],
                    "acknowledged_at_ms": 16900
                }
            })),
        );
        assert_eq!(st_act, 200);
    }

    // Ingest telemetry for both pump-001 and pump-002
    let (st_ing1, _) = http_call(
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
            "message_id": "msg-d11-pump001-01",
            "source_id": "gw-d11-01",
            "source_epoch": "epoch-d11-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-001-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.75,
                    "source_at_ms": 20000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                },
                {
                    "property": "motor_current",
                    "value": 38.2,
                    "source_at_ms": 20000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ing1, 202);

    let (st_ing2, _) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "20020"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-d11-pump002-01",
            "source_id": "gw-d11-01",
            "source_epoch": "epoch-d11-01",
            "device_id": "pump-002",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-002-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 2.10,
                    "source_at_ms": 20000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ing2, 202);

    // =========================================================================
    // 3. Verify AI Token Write Attempts are ALL Rejected with 403 FORBIDDEN_ACTION
    // =========================================================================
    let (st_w_model, err_w_model) = http_call(
        addr,
        "POST",
        "/v1/models",
        &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "25000")],
        Some(&json!({
            "model_revision_id": "pump-model-ai-hack",
            "product_id": "prod-pump",
            "model_version": "9.9.9",
            "properties": []
        })),
    );
    assert_eq!(st_w_model, 403);
    assert_eq!(err_w_model["error"]["code"], "FORBIDDEN_ACTION");

    let (st_w_dev, err_w_dev) = http_call(
        addr,
        "POST",
        "/v1/devices",
        &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "25000")],
        Some(&json!({
            "device_id": "pump-ai- rogue",
            "device_key": "pump-ai-rogue",
            "device_name": "AI伪造设备",
            "product_id": "prod-pump",
            "model_revision_id": "pump-model-r1",
            "asset_id": "asset-room-1"
        })),
    );
    assert_eq!(st_w_dev, 403);
    assert_eq!(err_w_dev["error"]["code"], "FORBIDDEN_ACTION");

    let (st_w_tel, err_w_tel) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_ai),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "25000"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-ai-spoof-01",
            "source_id": "gw-d11-01",
            "source_epoch": "epoch-d11-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-001-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 3.99,
                    "source_at_ms": 25000,
                    "source_time_origin": "device_clock",
                    "source_seq": "99",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_w_tel, 403);
    assert_eq!(err_w_tel["error"]["code"], "FORBIDDEN_ACTION");

    // Also verify other write endpoints are 403 FORBIDDEN_ACTION
    for (target, payload) in [
        (
            "/v1/products",
            json!({"product_id": "p-ai", "product_key": "k-ai", "name": "n"}),
        ),
        (
            "/v1/assets",
            json!({"asset_id": "a-ai", "parent_asset_id": null, "slug": "s-ai", "name": "n"}),
        ),
        (
            "/v1/bindings",
            json!({
                "binding_revision_id": "b-ai",
                "device_id": "pump-001",
                "model_revision_id": "pump-model-r1",
                "source_id": "gw-d11-01",
                "points": []
            }),
        ),
    ] {
        let (st_w, err_w) = http_call(
            addr,
            "POST",
            target,
            &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "25000")],
            Some(&payload),
        );
        assert_eq!(st_w, 403, "Expected 403 on POST {}", target);
        assert_eq!(err_w["error"]["code"], "FORBIDDEN_ACTION");
    }

    // =========================================================================
    // 4. Verify AI Token Control Route Attempts are Rejected with Zero Side Effects
    // =========================================================================
    for (ctrl_target, ctrl_payload) in [
        (
            "/v1/devices/pump-001/commands",
            json!({"command": "set_frequency", "target_hz": 50.0}),
        ),
        (
            "/v1/control",
            json!({"device_id": "pump-001", "property": "outlet_pressure", "write_value": 2.5}),
        ),
        (
            "/v1/devices/pump-001/write",
            json!({"register_address": "40001", "raw_value": 250}),
        ),
    ] {
        let (st_ctrl, err_ctrl) = http_call(
            addr,
            "POST",
            ctrl_target,
            &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "26000")],
            Some(&ctrl_payload),
        );
        assert_eq!(
            st_ctrl, 404,
            "Control route {} must not exist in read-only engine",
            ctrl_target
        );
        assert_eq!(err_ctrl["error"]["code"], "RESOURCE_NOT_FOUND");
    }

    // Verify client-supplied Scope/Tenant injection headers are rejected with 422 (SC02)
    for (forbidden_hdr, hdr_val) in [
        ("X-Tenant-Id", "ten-other"),
        ("X-Scope", "tenant_wide=true"),
        ("X-Allowed-Actions", "*"),
    ] {
        let (st_hdr, err_hdr) = http_call(
            addr,
            "GET",
            "/v1/devices/pump-001/view?now_ms=26500",
            &[("Authorization", &auth_ai), (forbidden_hdr, hdr_val)],
            None,
        );
        assert_eq!(st_hdr, 422);
        assert_eq!(err_hdr["error"]["code"], "VALIDATION_FAILED");
    }

    // =========================================================================
    // 5. Verify DeviceView Property Cropping (R09) & Unauthorized Device 404 Hiding
    // =========================================================================
    let (st_view_ok, view_ok) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-001/view?now_ms=27000",
        &[("Authorization", &auth_ai)],
        None,
    );
    assert_eq!(st_view_ok, 200);
    let props_obj = view_ok["properties"].as_object().unwrap();
    assert_eq!(
        props_obj.len(),
        1,
        "R09: Only authorized property 'outlet_pressure' may be returned in DeviceView"
    );
    assert!(props_obj.contains_key("outlet_pressure"));
    assert!(
        !props_obj.contains_key("motor_current"),
        "R09: Unauthorized property 'motor_current' must be cropped from DeviceView"
    );
    assert!(
        !props_obj.contains_key("rated_power"),
        "R09: Unauthorized static property 'rated_power' must be cropped from DeviceView"
    );
    // Verify outlet_pressure value & metadata are intact (zero side effects from rejected calls)
    assert_eq!(props_obj["outlet_pressure"]["value"], json!(1.75));
    assert_eq!(props_obj["outlet_pressure"]["quality"], "good");
    assert_eq!(props_obj["outlet_pressure"]["freshness"], "fresh");
    assert_eq!(props_obj["outlet_pressure"]["binding_state"], "bound");
    assert_eq!(props_obj["outlet_pressure"]["observation_state"], "observed");
    assert_eq!(props_obj["outlet_pressure"]["value_sampled_at_ms"], json!(20000));

    // Reading unauthorized device `pump-002` returns 404 RESOURCE_NOT_FOUND (existence hiding)
    let (st_view_unauth, err_view_unauth) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-002/view?now_ms=27000",
        &[("Authorization", &auth_ai)],
        None,
    );
    assert_eq!(st_view_unauth, 404);
    assert_eq!(err_view_unauth["error"]["code"], "RESOURCE_NOT_FOUND");

    // Verify History Query Scope enforcement (R05):
    // - Querying authorized `res-pump-001-outlet-pressure` succeeds (200)
    // - Querying unauthorized `res-pump-001-motor-current` or `res-pump-002-outlet-pressure` fails with 404 RESOURCE_NOT_FOUND
    let (st_hist_ok, hist_ok) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "28000")],
        Some(&json!({
            "resource_ids": ["res-pump-001-outlet-pressure"],
            "range": {"start_ms": 15000, "end_ms": 30000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_hist_ok, 200);
    let hist_items = hist_ok["items"].as_array().unwrap();
    assert_eq!(hist_items.len(), 1);
    assert_eq!(hist_items[0]["value"], json!(1.75));

    let (st_hist_unauth_prop, err_hist_unauth_prop) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "28000")],
        Some(&json!({
            "resource_ids": ["res-pump-001-outlet-pressure", "res-pump-001-motor-current"],
            "range": {"start_ms": 15000, "end_ms": 30000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_hist_unauth_prop, 404);
    assert_eq!(err_hist_unauth_prop["error"]["code"], "RESOURCE_NOT_FOUND");

    let (st_hist_unauth_dev, err_hist_unauth_dev) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_ai), ("X-Engine-Now-Ms", "28000")],
        Some(&json!({
            "resource_ids": ["res-pump-002-outlet-pressure"],
            "range": {"start_ms": 15000, "end_ms": 30000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 10,
            "cursor": null
        })),
    );
    assert_eq!(st_hist_unauth_dev, 404);
    assert_eq!(err_hist_unauth_dev["error"]["code"], "RESOURCE_NOT_FOUND");

    srv.shutdown();
}
