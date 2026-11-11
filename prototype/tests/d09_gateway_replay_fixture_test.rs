use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::server::ServerHandle;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};

const TEST_SECRET: &str = "d09-offline-fixture-hmac-secret-2026";

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
fn d09_offline_gateway_disconnect_backfill_and_binding_switch_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut srv = ServerHandle::spawn(cfg, TEST_SECRET).unwrap();
    let addr = &srv;

    // Mint signed Scope tokens (Q02)
    let mgmt_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-d09".into(),
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
        operator_id: Some("op-d09-verifier".into()),
    }
    .issue_signed_token(TEST_SECRET);

    let gw_token = VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-d09".into(),
        caller_service_id: "gw-edge-01".into(),
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

    let auth_mgmt = format!("Bearer {}", mgmt_token);
    let auth_gw = format!("Bearer {}", gw_token);

    // =========================================================================
    // Setup: Product, Thing Model, Asset, and Two Devices (pump-101 & pump-102)
    // =========================================================================
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

    let (st_model, _) = http_call(
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
    assert_eq!(st_model, 201);

    let (st_val_m, _) = http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r1:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "11000")],
        None,
    );
    assert_eq!(st_val_m, 200);

    let (st_pub_m, _) = http_call(
        addr,
        "POST",
        "/v1/models/pump-model-r1:publish",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "12000")],
        Some(&json!({"published_at_ms": 12000})),
    );
    assert_eq!(st_pub_m, 200);

    let (st_asset, _) = http_call(
        addr,
        "POST",
        "/v1/assets",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "13000")],
        Some(&json!({
            "asset_id": "asset-station-1",
            "parent_asset_id": null,
            "slug": "station-1",
            "name": "一号泵站"
        })),
    );
    assert_eq!(st_asset, 201);

    for (dev_id, dev_name) in [("pump-101", "101号增压泵"), ("pump-102", "102号增压泵")] {
        let (st_dev, _) = http_call(
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
                "asset_id": "asset-station-1"
            })),
        );
        assert_eq!(st_dev, 201);

        let (st_grant, _) = http_call(
            addr,
            "POST",
            "/v1/sources/grants",
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "15000")],
            Some(&json!({
                "source_id": "gw-edge-01",
                "device_id": dev_id,
                "valid_from_ms": 15000
            })),
        );
        assert_eq!(st_grant, 200);
    }

    // Negative test (T01): Reject server_scale_factor when normalization_owner = "gateway"
    let (st_bad_scale, bad_scale_body) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-101/bindings",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "16000")],
        Some(&json!({
            "binding_revision_id": "bind-bad-double-scale",
            "model_revision_id": "pump-model-r1",
            "source_id": "gw-edge-01",
            "normalization_owner": "gateway",
            "server_scale_factor": 0.01,
            "points": []
        })),
    );
    assert_eq!(st_bad_scale, 422);
    assert_eq!(bad_scale_body["error"]["code"], "VALIDATION_FAILED");

    // Create, validate, and activate bind-r1 for pump-101 and pump-102
    for (dev_id, bind_id, rated_kw) in [
        ("pump-101", "bind-pump-101-r1", 45.0),
        ("pump-102", "bind-pump-102-r1", 55.0),
    ] {
        let (st_b, _) = http_call(
            addr,
            "POST",
            &format!("/v1/devices/{}/bindings", dev_id),
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "17000")],
            Some(&json!({
                "binding_revision_id": bind_id,
                "model_revision_id": "pump-model-r1",
                "source_id": "gw-edge-01",
                "normalization_owner": "gateway",
                "server_scale_factor": null,
                "points": [
                    {
                        "property": "outlet_pressure",
                        "register_address": "40001",
                        "factor": 0.01,
                        "unit_code": "MPa",
                        "deadband": 0.0,
                        "heartbeat_ms": 60000
                    },
                    {
                        "property": "motor_current",
                        "register_address": "40002",
                        "factor": 0.1,
                        "unit_code": "A",
                        "deadband": 0.0,
                        "heartbeat_ms": 60000
                    }
                ],
                "static_points": [
                    {
                        "property": "rated_power",
                        "value": rated_kw,
                        "unit_code": "kW"
                    }
                ]
            })),
        );
        assert_eq!(st_b, 201);

        let (st_bv, bv_body) = http_call(
            addr,
            "POST",
            &format!("/v1/bindings/{}:validate", bind_id),
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "18000")],
            None,
        );
        assert_eq!(st_bv, 200);
        let cfg_hash = bv_body["config_hash"].as_str().unwrap().to_string();

        let (st_ba, _) = http_call(
            addr,
            "POST",
            &format!("/v1/bindings/{}:activate", bind_id),
            &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "19000")],
            Some(&json!({
                "expected_device_revision": 1,
                "activated_at_ms": 19000,
                "gateway_ack": {
                    "source_id": "gw-edge-01",
                    "source_epoch": "epoch-d09-01",
                    "binding_revision_id": bind_id,
                    "config_hash": cfg_hash,
                    "acknowledged_at_ms": 18900
                }
            })),
        );
        assert_eq!(st_ba, 200);
    }

    // =========================================================================
    // Scenario (a): Two devices under the same thing model reported by gw-edge-01
    // Verify engineering values are not cross-contaminated or double-scaled (T01)
    // =========================================================================
    let (st_ing_101, ing_101_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "100050"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-p101-live-01",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-101-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.25,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                },
                {
                    "property": "motor_current",
                    "value": 28.5,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ing_101, 202);
    assert_eq!(ing_101_body["status"], "accepted_durable");

    let (st_ing_102, ing_102_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "100060"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-p102-live-01",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-102",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-102-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 2.40,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                },
                {
                    "property": "motor_current",
                    "value": 41.2,
                    "source_at_ms": 100000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_ing_102, 202);
    assert_eq!(ing_102_body["status"], "accepted_durable");

    let (st_v101, v101) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-101/view?now_ms=100100",
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(st_v101, 200);
    assert_eq!(v101["properties"]["outlet_pressure"]["value"], json!(1.25));
    assert_eq!(v101["properties"]["motor_current"]["value"], json!(28.5));
    assert_eq!(v101["properties"]["rated_power"]["value"], json!(45.0));
    assert_eq!(v101["properties"]["outlet_pressure"]["freshness"], "fresh");
    assert_eq!(v101["properties"]["outlet_pressure"]["quality"], "good");

    let (st_v102, v102) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-102/view?now_ms=100100",
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(st_v102, 200);
    assert_eq!(v102["properties"]["outlet_pressure"]["value"], json!(2.40));
    assert_eq!(v102["properties"]["motor_current"]["value"], json!(41.2));
    assert_eq!(v102["properties"]["rated_power"]["value"], json!(55.0));

    // =========================================================================
    // Scenario (b): Gateway reports disconnected via POST /v1/source-state
    // Verify freshness == "stale", quality == "good" (not overwritten),
    // and static property rated_power retains freshness == "static" (Q04/SC08/T06)
    // =========================================================================
    let (st_disc, disc_body) = http_call(
        addr,
        "POST",
        "/v1/source-state",
        &[("Authorization", &auth_gw), ("X-Engine-Now-Ms", "105000")],
        Some(&json!({
            "spec": "iotfast.source_state.v1",
            "event_id": "evt-gw-disc-01",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "binding_revision_id": "bind-pump-101-r1",
            "observed_at_ms": 105000,
            "connection_state": "disconnected",
            "point_states": []
        })),
    );
    assert_eq!(st_disc, 202);
    assert_eq!(disc_body["status"], "accepted");

    let (st_v101_disc, v101_disc) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-101/view?now_ms=106000",
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(st_v101_disc, 200);
    assert_eq!(v101_disc["source_connection_state"], "disconnected");
    assert_eq!(
        v101_disc["properties"]["outlet_pressure"]["freshness"],
        "stale",
        "Q04/SC08/T06: Gateway disconnect must mark dynamic property freshness as stale"
    );
    assert_eq!(
        v101_disc["properties"]["outlet_pressure"]["quality"],
        "good",
        "Q04/SC08/T06: Gateway disconnect without point fault evidence must preserve quality=good"
    );
    assert_eq!(
        v101_disc["properties"]["outlet_pressure"]["value"],
        json!(1.25)
    );
    assert_eq!(
        v101_disc["properties"]["outlet_pressure"]["value_sampled_at_ms"],
        json!(100000),
        "C02: Disconnect must not alter original value_sampled_at_ms"
    );
    assert_eq!(
        v101_disc["properties"]["rated_power"]["freshness"],
        "static",
        "Static property freshness must remain static during gateway disconnect"
    );
    assert_eq!(v101_disc["properties"]["rated_power"]["quality"], "good");

    // =========================================================================
    // Scenario (c): Gateway reconnects and backfills 5 cached samples out-of-order
    // with original message_id + replayed: true, plus 2 duplicate retries (T03/T05)
    // =========================================================================
    let (st_conn, _) = http_call(
        addr,
        "POST",
        "/v1/source-state",
        &[("Authorization", &auth_gw), ("X-Engine-Now-Ms", "155000")],
        Some(&json!({
            "spec": "iotfast.source_state.v1",
            "event_id": "evt-gw-conn-02",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "binding_revision_id": "bind-pump-101-r1",
            "observed_at_ms": 155000,
            "connection_state": "connected",
            "point_states": []
        })),
    );
    assert_eq!(st_conn, 202);

    // 5 offline samples generated at source_at_ms = 110_000..=150_000 (seq 2..=6),
    // replayed out-of-order: [3 (130k), 1 (110k), 5 (150k), 2 (120k), 4 (140k)]
    let backfill_batches = [
        ("msg-p101-bf-03", 130_000i64, "4", 1.45f64, 156_010i64),
        ("msg-p101-bf-01", 110_000i64, "2", 1.30f64, 156_020i64),
        ("msg-p101-bf-05", 150_000i64, "6", 1.68f64, 156_030i64),
        ("msg-p101-bf-02", 120_000i64, "3", 1.38f64, 156_040i64),
        ("msg-p101-bf-04", 140_000i64, "5", 1.52f64, 156_050i64),
    ];

    for (msg_id, src_at_ms, seq_str, pressure_val, recv_now_ms) in backfill_batches {
        let (st_bf, bf_body) = http_call(
            addr,
            "POST",
            "/v1/telemetry",
            &[
                ("Authorization", &auth_gw),
                ("Prefer", "durability=durable"),
                ("X-Engine-Now-Ms", &recv_now_ms.to_string()),
            ],
            Some(&json!({
                "spec": "iotfast.telemetry.v1",
                "message_id": msg_id,
                "source_id": "gw-edge-01",
                "source_epoch": "epoch-d09-01",
                "device_id": "pump-101",
                "model_revision_id": "pump-model-r1",
                "binding_revision_id": "bind-pump-101-r1",
                "points": [
                    {
                        "property": "outlet_pressure",
                        "value": pressure_val,
                        "source_at_ms": src_at_ms,
                        "source_time_origin": "device_clock",
                        "source_seq": seq_str,
                        "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                        "delivery": {"replayed": true}
                    }
                ]
            })),
        );
        assert_eq!(st_bf, 202, "Backfill message {} should be accepted", msg_id);
        assert_eq!(bf_body["status"], "accepted_durable");
    }

    // Re-transmit 2 of the backfill messages (msg-p101-bf-01 and msg-p101-bf-05) -> 200 duplicate (T03)
    for (msg_id, src_at_ms, seq_str, pressure_val) in [
        ("msg-p101-bf-01", 110_000i64, "2", 1.30f64),
        ("msg-p101-bf-05", 150_000i64, "6", 1.68f64),
    ] {
        let (st_dup, dup_body) = http_call(
            addr,
            "POST",
            "/v1/telemetry",
            &[
                ("Authorization", &auth_gw),
                ("Prefer", "durability=durable"),
                ("X-Engine-Now-Ms", "156100"),
            ],
            Some(&json!({
                "spec": "iotfast.telemetry.v1",
                "message_id": msg_id,
                "source_id": "gw-edge-01",
                "source_epoch": "epoch-d09-01",
                "device_id": "pump-101",
                "model_revision_id": "pump-model-r1",
                "binding_revision_id": "bind-pump-101-r1",
                "points": [
                    {
                        "property": "outlet_pressure",
                        "value": pressure_val,
                        "source_at_ms": src_at_ms,
                        "source_time_origin": "device_clock",
                        "source_seq": seq_str,
                        "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                        "delivery": {"replayed": true}
                    }
                ]
            })),
        );
        assert_eq!(st_dup, 200);
        assert_eq!(dup_body["status"], "duplicate");
    }

    // Negative test (T04): Same message_id with mutated payload must be rejected with 409 IDEMPOTENCY_CONFLICT
    let (st_conflict, conflict_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "156150"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-p101-bf-05",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-101-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.99,
                    "source_at_ms": 150000,
                    "source_time_origin": "device_clock",
                    "source_seq": "6",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": true}
                }
            ]
        })),
    );
    assert_eq!(st_conflict, 409);
    assert_eq!(conflict_body["error"]["code"], "IDEMPOTENCY_CONFLICT");

    // Verify latest_value holds max (source_at_ms=150000, source_seq_u64=6) -> 1.68 MPa,
    // even though msg-p101-bf-04 (140000, 1.52 MPa) arrived last! (T05)
    let (st_v101_after, v101_after) = http_call(
        addr,
        "GET",
        "/v1/devices/pump-101/view?now_ms=156500",
        &[("Authorization", &auth_mgmt)],
        None,
    );
    assert_eq!(st_v101_after, 200);
    assert_eq!(v101_after["source_connection_state"], "connected");
    assert_eq!(
        v101_after["properties"]["outlet_pressure"]["value"],
        json!(1.68),
        "T05: latest_value must retain max (source_at_ms, source_seq_u64) sample (1.68 @ 150000)"
    );
    assert_eq!(
        v101_after["properties"]["outlet_pressure"]["value_sampled_at_ms"],
        json!(150000)
    );
    assert_eq!(
        v101_after["properties"]["outlet_pressure"]["freshness"],
        "fresh"
    );

    let p101_pressure_res_id = v101_after["properties"]["outlet_pressure"]["resource_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Reconcile via POST /v1/history/query: verify 1 initial + 5 backfilled = 6 points, 0 duplicates
    let (st_hist, hist_body) = http_call(
        addr,
        "POST",
        "/v1/history/query",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "157000")],
        Some(&json!({
            "resource_ids": [p101_pressure_res_id],
            "range": {"start_ms": 100000, "end_ms": 160000},
            "mode": "raw",
            "aggregation": null,
            "quality_states": ["good"],
            "order": "asc",
            "limit": 20,
            "cursor": null
        })),
    );
    assert_eq!(st_hist, 200);
    let items = hist_body["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        6,
        "All 6 samples (1 live + 5 offline backfilled) must be persisted with 0 duplicates"
    );
    let expected_series = [
        (100_000i64, "1", 1.25f64, false),
        (110_000i64, "2", 1.30f64, true),
        (120_000i64, "3", 1.38f64, true),
        (130_000i64, "4", 1.45f64, true),
        (140_000i64, "5", 1.52f64, true),
        (150_000i64, "6", 1.68f64, true),
    ];
    for (idx, (exp_ts, exp_seq, exp_val, exp_replayed)) in expected_series.iter().enumerate() {
        assert_eq!(items[idx]["source_at_ms"], json!(*exp_ts));
        assert_eq!(items[idx]["source_seq"], json!(*exp_seq));
        assert_eq!(items[idx]["value"], json!(*exp_val));
        assert_eq!(items[idx]["replayed"], json!(*exp_replayed));
    }

    // =========================================================================
    // Scenario (d): Switch binding from bind-r1 to bind-r2 (Q09), then verify:
    // 1. Old committed message_id from bind-r1 period retried -> 200 duplicate (R08/T08)
    // 2. New message_id claiming superseded bind-r1 -> 409 REVISION_CONFLICT (T08)
    // 3. New message_id under active bind-r2 -> 202 accepted_durable
    // =========================================================================
    let (st_b2, _) = http_call(
        addr,
        "POST",
        "/v1/devices/pump-101/bindings",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "160000")],
        Some(&json!({
            "binding_revision_id": "bind-pump-101-r2",
            "model_revision_id": "pump-model-r1",
            "source_id": "gw-edge-01",
            "normalization_owner": "gateway",
            "server_scale_factor": null,
            "points": [
                {
                    "property": "outlet_pressure",
                    "register_address": "40011",
                    "factor": 0.01,
                    "unit_code": "MPa",
                    "deadband": 0.0,
                    "heartbeat_ms": 30000
                },
                {
                    "property": "motor_current",
                    "register_address": "40012",
                    "factor": 0.1,
                    "unit_code": "A",
                    "deadband": 0.0,
                    "heartbeat_ms": 30000
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
    assert_eq!(st_b2, 201);

    let (st_b2v, b2v_body) = http_call(
        addr,
        "POST",
        "/v1/bindings/bind-pump-101-r2:validate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "161000")],
        None,
    );
    assert_eq!(st_b2v, 200);
    let b2_cfg_hash = b2v_body["config_hash"].as_str().unwrap().to_string();

    // Device revision is 2 after activating bind-pump-101-r1
    let (st_b2a, b2a_body) = http_call(
        addr,
        "POST",
        "/v1/bindings/bind-pump-101-r2:activate",
        &[("Authorization", &auth_mgmt), ("X-Engine-Now-Ms", "162000")],
        Some(&json!({
            "expected_device_revision": 2,
            "activated_at_ms": 162000,
            "gateway_ack": {
                "source_id": "gw-edge-01",
                "source_epoch": "epoch-d09-01",
                "binding_revision_id": "bind-pump-101-r2",
                "config_hash": b2_cfg_hash,
                "acknowledged_at_ms": 161900
            }
        })),
    );
    assert_eq!(st_b2a, 200);
    assert_eq!(b2a_body["status"], "active");

    // R08 / T08: Retrying already-committed message from bind-r1 period (msg-p101-bf-03)
    // MUST return 200 duplicate instead of 409 REVISION_CONFLICT!
    let (st_old_dup, old_dup_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "163000"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-p101-bf-03",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-101-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.45,
                    "source_at_ms": 130000,
                    "source_time_origin": "device_clock",
                    "source_seq": "4",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": true}
                }
            ]
        })),
    );
    assert_eq!(
        st_old_dup, 200,
        "R08/T08: Committed message from superseded binding must idempotently return 200 duplicate"
    );
    assert_eq!(old_dup_body["status"], "duplicate");

    // T08: Brand-new message_id still referencing superseded bind-pump-101-r1 -> 409 REVISION_CONFLICT
    let (st_stale_bind, stale_bind_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "163500"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-p101-stale-bind-new",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-101-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.70,
                    "source_at_ms": 163000,
                    "source_time_origin": "device_clock",
                    "source_seq": "7",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_stale_bind, 409);
    assert_eq!(stale_bind_body["error"]["code"], "REVISION_CONFLICT");

    // New telemetry under active bind-pump-101-r2 -> 202 accepted_durable
    let (st_r2_ok, r2_ok_body) = http_call(
        addr,
        "POST",
        "/v1/telemetry",
        &[
            ("Authorization", &auth_gw),
            ("Prefer", "durability=durable"),
            ("X-Engine-Now-Ms", "164050"),
        ],
        Some(&json!({
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-p101-r2-live-01",
            "source_id": "gw-edge-01",
            "source_epoch": "epoch-d09-01",
            "device_id": "pump-101",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-101-r2",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.75,
                    "source_at_ms": 164000,
                    "source_time_origin": "device_clock",
                    "source_seq": "7",
                    "quality": {"state": "good", "source_protocol": "modbus", "source_code": null},
                    "delivery": {"replayed": false}
                }
            ]
        })),
    );
    assert_eq!(st_r2_ok, 202);
    assert_eq!(r2_ok_body["status"], "accepted_durable");

    srv.shutdown();
}
