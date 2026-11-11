use iotfast_thing_engine::asset_uns::{
    AssetUnsService, CreateAssetRequest, InstantiateDeviceRequest,
};
use iotfast_thing_engine::binding::{
    ActivateBindingWithManualWindowRequest, BindingService, CreateBindingDraftRequest,
    ManualStopWindow, PointBindingDraft, StaticPropertyBindingDraft,
};
use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::d00::{
    TelemetryDelivery, TelemetryEnvelope, TelemetryPoint, TelemetryQuality, ValueType,
};
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::storage_duckdb::{DurabilityPreference, EngineRuntime};
use iotfast_thing_engine::ts_adapter::{
    send_tdengine_http_over_stream, TdengineConfig, TdengineExecutionMode, TimeSeriesBackendKind,
};
use serde_json::json;
use std::io::{Cursor, Read, Write};

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
            "product:read".into(),
            "model:write".into(),
            "model:read".into(),
            "asset:write".into(),
            "asset:read".into(),
            "device:write".into(),
            "device:read".into(),
            "binding:write".into(),
            "binding:read".into(),
            "view:read".into(),
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
        caller_service_id: "gw-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter::default(),
        operator_id: None,
    }
}

#[test]
fn sqlite_saves_business_metadata_duckdb_saves_device_data_and_tdengine_adapter_executes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let m_ctx = mgmt_ctx();
    let g_ctx = gateway_ctx();

    // 1. Open EngineRuntime and create business metadata (Product -> Model -> Asset -> Device -> Binding)
    {
        let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
        ModelCatalogService::create_product(
            rt.conn(),
            &m_ctx,
            CreateProductRequest {
                product_id: "prod-pump".into(),
                product_key: "booster_pump".into(),
                name: "Booster Pump".into(),
            },
        )
        .unwrap();

        ModelCatalogService::create_model_draft(
            rt.conn(),
            &m_ctx,
            CreateModelDraftRequest {
                model_revision_id: "pump-r1".into(),
                product_id: "prod-pump".into(),
                model_version: "1.0.0".into(),
                properties: vec![
                    ModelPropertyDraft {
                        identifier: "outlet_pressure".into(),
                        name: "Outlet Pressure".into(),
                        value_type: ValueType::Float64,
                        unit_code: Some("MPa".into()),
                        unit_symbol: Some("MPa".into()),
                        min_value: Some(0.0),
                        max_value: Some(5.0),
                        required: true,
                    },
                    ModelPropertyDraft {
                        identifier: "rated_power".into(),
                        name: "Rated Power".into(),
                        value_type: ValueType::Float64,
                        unit_code: Some("kW".into()),
                        unit_symbol: Some("kW".into()),
                        min_value: Some(0.0),
                        max_value: Some(500.0),
                        required: false,
                    },
                ],
            },
        )
        .unwrap();
        ModelCatalogService::validate_model(rt.conn(), &m_ctx, "pump-r1", None).unwrap();
        ModelCatalogService::publish_model(rt.conn(), &m_ctx, "pump-r1", 2_000).unwrap();

        AssetUnsService::create_asset(
            rt.conn(),
            &m_ctx,
            CreateAssetRequest {
                asset_id: "asset-room-1".into(),
                parent_asset_id: None,
                slug: "room-1".into(),
                name: "Pump Room 1".into(),
            },
        )
        .unwrap();

        AssetUnsService::instantiate_device(
            rt.conn(),
            &m_ctx,
            InstantiateDeviceRequest {
                device_id: "pump-001".into(),
                device_key: "pump-001".into(),
                device_name: "Pump #1".into(),
                product_id: "prod-pump".into(),
                model_revision_id: "pump-r1".into(),
                asset_id: "asset-room-1".into(),
            },
        )
        .unwrap();

        BindingService::register_source_and_grant(rt.conn(), &m_ctx, "gw-01", "pump-001", 2_000)
            .unwrap();

        BindingService::create_binding_draft_with_statics(
            rt.conn(),
            &m_ctx,
            CreateBindingDraftRequest {
                binding_revision_id: "bind-pump-001-r1".into(),
                device_id: "pump-001".into(),
                model_revision_id: "pump-r1".into(),
                source_id: "gw-01".into(),
                normalization_owner: "gateway".into(),
                server_scale_factor: None,
                points: vec![PointBindingDraft {
                    property: "outlet_pressure".into(),
                    register_address: "40001".into(),
                    factor: 0.01,
                    unit_code: Some("MPa".into()),
                    deadband: Some(0.05),
                    heartbeat_ms: Some(60_000),
                }],
            },
            vec![StaticPropertyBindingDraft {
                property: "rated_power".into(),
                value: json!(45.0),
                unit_code: Some("kW".into()),
            }],
        )
        .unwrap();
        BindingService::validate_binding(rt.conn(), &m_ctx, "bind-pump-001-r1").unwrap();
        BindingService::activate_binding_with_manual_window(
            rt.conn(),
            &m_ctx,
            ActivateBindingWithManualWindowRequest {
                binding_revision_id: "bind-pump-001-r1".into(),
                expected_device_revision: 1,
                activated_at_ms: 3_000,
                manual_stop_window: ManualStopWindow {
                    window_start_ms: 2_500,
                    window_end_ms: 3_500,
                    operator_id: "op-alice".into(),
                },
            },
        )
        .unwrap();

        // Sync and verify SQLite business store vs DuckDB time-series store
        let status = rt.storage_architecture_status().unwrap();
        assert_eq!(status.business_metadata_store.engine, "sqlite");
        assert_eq!(status.business_metadata_store.journal_mode, "wal");
        assert!(cfg.sqlite_business_db_path().exists());
        assert_eq!(
            *status
                .business_metadata_store
                .row_counts
                .get("product")
                .unwrap(),
            1
        );
        assert_eq!(
            *status
                .business_metadata_store
                .row_counts
                .get("device")
                .unwrap(),
            1
        );
        assert_eq!(
            *status
                .business_metadata_store
                .row_counts
                .get("resource")
                .unwrap(),
            2
        );
        assert_eq!(
            *status
                .business_metadata_store
                .row_counts
                .get("point_binding")
                .unwrap(),
            2
        );
        assert_eq!(status.device_timeseries_store.active_backend, "duckdb");

        // Ingest telemetry into default DuckDB adapter
        let receipt = rt
            .ingest_telemetry(
                &g_ctx,
                TelemetryEnvelope {
                    spec: "iotfast.telemetry.v1".into(),
                    message_id: "msg-001".into(),
                    source_id: "gw-01".into(),
                    source_epoch: "ep-1".into(),
                    device_id: "pump-001".into(),
                    model_revision_id: "pump-r1".into(),
                    binding_revision_id: "bind-pump-001-r1".into(),
                    points: vec![TelemetryPoint {
                        property: "outlet_pressure".into(),
                        value: json!(1.55),
                        source_at_ms: 10_000,
                        source_time_origin: "device_clock".into(),
                        source_seq: "1".into(),
                        quality: TelemetryQuality {
                            state: "good".into(),
                            source_protocol: Some("modbus".into()),
                            source_code: None,
                        },
                        delivery: TelemetryDelivery { replayed: false },
                    }],
                },
                DurabilityPreference::Durable,
                10_005,
            )
            .unwrap();
        assert_eq!(receipt.status, "accepted_durable");
    }

    // 2. Prove business metadata is authoritatively saved in SQLite:
    // Remove the DuckDB file completely, reopen EngineRuntime, and verify business metadata
    // (product, model, asset, device, resources, bindings) is restored from `business_metadata.sqlite`!
    std::fs::remove_file(cfg.db_path()).unwrap();
    assert!(!cfg.db_path().exists());
    assert!(cfg.sqlite_business_db_path().exists());

    let mut rt = EngineRuntime::open(cfg.clone()).unwrap();
    let restored_dev = AssetUnsService::get_device(rt.conn(), &m_ctx, "pump-001").unwrap();
    assert_eq!(restored_dev.device_id, "pump-001");
    assert_eq!(restored_dev.resources.len(), 2);
    let view = rt.get_device_view(&m_ctx, "pump-001", 12_000).unwrap();
    assert_eq!(
        view.active_binding_revision_id.as_deref(),
        Some("bind-pump-001-r1")
    );
    let rated_power = view.properties.get("rated_power").unwrap();
    assert_eq!(rated_power.binding_state, "bound_static");
    assert_eq!(rated_power.value, json!(45.0));

    // 3. Test extensible TDengine TimeSeriesStorageAdapter (STABLE + Subtable SQL generation & HTTP REST wire format)
    let td_cfg = TdengineConfig {
        endpoint: "http://127.0.0.1:6041".into(),
        database: "iotfast_tsdb".into(),
        super_table: "stb_observations".into(),
        username: "root".into(),
        password: "taosdata".into(),
        execution_mode: TdengineExecutionMode::LocalMirrorWithSqlSpool,
    };
    rt.switch_to_tdengine_adapter(td_cfg.clone()).unwrap();
    assert_eq!(
        rt.active_timeseries_backend(),
        TimeSeriesBackendKind::Tdengine
    );

    // Ingest new telemetry sample and verify TDengine Subtable INSERT SQL is generated
    let receipt2 = rt
        .ingest_telemetry(
            &g_ctx,
            TelemetryEnvelope {
                spec: "iotfast.telemetry.v1".into(),
                message_id: "msg-002".into(),
                source_id: "gw-01".into(),
                source_epoch: "ep-1".into(),
                device_id: "pump-001".into(),
                model_revision_id: "pump-r1".into(),
                binding_revision_id: "bind-pump-001-r1".into(),
                points: vec![TelemetryPoint {
                    property: "outlet_pressure".into(),
                    value: json!(1.72),
                    source_at_ms: 20_000,
                    source_time_origin: "device_clock".into(),
                    source_seq: "2".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                }],
            },
            DurabilityPreference::Durable,
            20_005,
        )
        .unwrap();
    assert_eq!(receipt2.status, "accepted_durable");

    let arch_status = rt.storage_architecture_status().unwrap();
    assert_eq!(arch_status.device_timeseries_store.active_backend, "tdengine");
    let sql_list = &arch_status.device_timeseries_store.recent_tdengine_sql_log;
    assert_eq!(sql_list.len(), 2);
    assert!(sql_list[0].contains("CREATE STABLE IF NOT EXISTS iotfast_tsdb.stb_observations"));
    assert!(sql_list[1].contains("INSERT INTO iotfast_tsdb.tb_"));
    assert!(sql_list[1].contains("USING iotfast_tsdb.stb_observations TAGS ('ten-8iic'"));

    // Verify TDengine HTTP REST `/rest/sql/iotfast_tsdb` wire protocol exchange
    struct MockTaosHttpStream {
        written: Vec<u8>,
        read_cursor: Cursor<Vec<u8>>,
    }
    impl Read for MockTaosHttpStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.read_cursor.read(buf)
        }
    }
    impl Write for MockTaosHttpStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let taos_ok_resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"code\":0,\"column_meta\":[[\"affected_rows\",\"INT\",4]],\"data\":[[1]],\"rows\":1}".to_vec();
    let mut mock_stream = MockTaosHttpStream {
        written: Vec::new(),
        read_cursor: Cursor::new(taos_ok_resp),
    };
    send_tdengine_http_over_stream(
        &mut mock_stream,
        "127.0.0.1:6041",
        "",
        &td_cfg,
        &sql_list[1],
    )
    .unwrap();
    let wire_req = String::from_utf8_lossy(&mock_stream.written);
    assert!(wire_req.starts_with("POST /rest/sql/iotfast_tsdb HTTP/1.1\r\n"));
    assert!(wire_req.contains("Authorization: Basic "));
    assert!(wire_req.contains("INSERT INTO iotfast_tsdb.tb_"));

    // Verify TDengine time-bucket aggregation query pushdown SQL
    let agg_plan = rt.build_history_query_plan(
        "ten-8iic",
        &["res-pump-001-outlet-pressure".to_string()],
        10_000,
        70_000,
        Some(60_000),
    );
    assert_eq!(agg_plan.backend, "tdengine");
    assert_eq!(agg_plan.mode, "aggregate");
    assert!(agg_plan
        .native_sql
        .contains("PARTITION BY resource_id INTERVAL(60000a) FILL(NONE)"));

    // 4. Verify TimescaleDB and ClickHouse extensible SQL adapters
    rt.switch_timeseries_backend(TimeSeriesBackendKind::Timescaledb, None)
        .unwrap();
    assert_eq!(
        rt.active_timeseries_backend(),
        TimeSeriesBackendKind::Timescaledb
    );
    let ts_receipt = rt
        .ingest_telemetry(
            &g_ctx,
            TelemetryEnvelope {
                spec: "iotfast.telemetry.v1".into(),
                message_id: "msg-003".into(),
                source_id: "gw-01".into(),
                source_epoch: "ep-1".into(),
                device_id: "pump-001".into(),
                model_revision_id: "pump-r1".into(),
                binding_revision_id: "bind-pump-001-r1".into(),
                points: vec![TelemetryPoint {
                    property: "outlet_pressure".into(),
                    value: json!(1.88),
                    source_at_ms: 30_000,
                    source_time_origin: "device_clock".into(),
                    source_seq: "3".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                }],
            },
            DurabilityPreference::Durable,
            30_005,
        )
        .unwrap();
    assert_eq!(ts_receipt.status, "accepted_durable");
    let ts_status = rt.storage_architecture_status().unwrap();
    assert_eq!(
        ts_status.device_timeseries_store.active_backend,
        "timescaledb"
    );
    assert_eq!(
        ts_status.device_timeseries_store.supported_backends,
        vec!["duckdb", "tdengine", "timescaledb", "clickhouse"]
    );
    assert!(ts_status.device_timeseries_store.recent_tdengine_sql_log[0]
        .contains("create_hypertable('history_observation', 'ts')"));
    assert!(ts_status.device_timeseries_store.recent_tdengine_sql_log[1]
        .contains("INSERT INTO history_observation"));
    let ts_plan = rt.build_history_query_plan(
        "ten-8iic",
        &["res-pump-001-outlet-pressure".to_string()],
        10_000,
        70_000,
        Some(60_000),
    );
    assert_eq!(ts_plan.backend, "timescaledb");
    assert!(ts_plan.native_sql.contains("time_bucket("));

    rt.switch_timeseries_backend(TimeSeriesBackendKind::Clickhouse, None)
        .unwrap();
    assert_eq!(
        rt.active_timeseries_backend(),
        TimeSeriesBackendKind::Clickhouse
    );
    let ch_status = rt.storage_architecture_status().unwrap();
    assert_eq!(
        ch_status.device_timeseries_store.active_backend,
        "clickhouse"
    );
    assert!(ch_status.device_timeseries_store.recent_tdengine_sql_log[0]
        .contains("ENGINE = ReplacingMergeTree()"));
    let ch_plan = rt.build_history_query_plan(
        "ten-8iic",
        &["res-pump-001-outlet-pressure".to_string()],
        10_000,
        70_000,
        Some(60_000),
    );
    assert_eq!(ch_plan.backend, "clickhouse");
    assert!(ch_plan.native_sql.contains("toStartOfInterval("));
}

#[test]
fn http_storage_status_timeseries_adapter_switch_and_query_plan_work() {
    use iotfast_thing_engine::server::ServerHandle;

    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let secret = "storage-test-secret";
    let srv = ServerHandle::spawn(cfg, secret).unwrap();
    let token = mgmt_ctx().issue_signed_token(secret);

    let send_http = |method: &str, path: &str, body: Option<serde_json::Value>| -> (u16, serde_json::Value) {
        let mut conn = srv.connect();
        let body_bytes = body
            .map(|b| serde_json::to_vec(&b).unwrap())
            .unwrap_or_default();
        let req = format!(
            "{} {} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nX-Engine-Now-Ms: 5000\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            method,
            path,
            token,
            body_bytes.len()
        );
        conn.write_all(req.as_bytes()).unwrap();
        if !body_bytes.is_empty() {
            conn.write_all(&body_bytes).unwrap();
        }
        conn.flush().unwrap();
        let mut raw = String::new();
        conn.read_to_string(&mut raw).unwrap();
        let (hdr, b) = raw.split_once("\r\n\r\n").unwrap();
        let code: u16 = hdr
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (code, serde_json::from_str(b).unwrap())
    };

    // 0. GET /health
    let (h_code, h_json) = send_http("GET", "/health", None);
    assert_eq!(h_code, 200);
    assert!(!h_json["sqlite_version"].as_str().unwrap_or("").is_empty());
    assert_eq!(h_json["sqlite_business_db_writable"], json!(true));

    // 1. GET /v1/storage/status
    let (status_code, status_json) = send_http("GET", "/v1/storage/status", None);
    assert_eq!(status_code, 200);
    assert_eq!(
        status_json["business_metadata_store"]["engine"],
        json!("sqlite")
    );
    assert_eq!(
        status_json["device_timeseries_store"]["active_backend"],
        json!("duckdb")
    );
    assert_eq!(
        status_json["device_timeseries_store"]["supported_backends"],
        json!(["duckdb", "tdengine", "timescaledb", "clickhouse"])
    );

    // 2. POST /v1/storage/timeseries-adapter -> switch to tdengine, timescaledb, clickhouse
    let (sw_code, sw_json) = send_http(
        "POST",
        "/v1/storage/timeseries-adapter",
        Some(json!({ "backend": "timescaledb" })),
    );
    assert_eq!(sw_code, 200);
    assert_eq!(
        sw_json["device_timeseries_store"]["active_backend"],
        json!("timescaledb")
    );

    // 3. POST /v1/storage/query-plan
    let (qp_code, qp_json) = send_http(
        "POST",
        "/v1/storage/query-plan",
        Some(json!({
            "resource_ids": ["res-pump-001-outlet-pressure"],
            "start_ms": 10000,
            "end_ms": 70000,
            "bucket_ms": 60000
        })),
    );
    assert_eq!(qp_code, 200);
    assert_eq!(qp_json["backend"], json!("timescaledb"));
    assert_eq!(qp_json["mode"], json!("aggregate"));
    assert!(qp_json["native_sql"]
        .as_str()
        .unwrap()
        .contains("time_bucket("));
}

#[test]
fn st02_capabilities_password_redaction_and_st04_checkpoint_hole_reconciliation_work() {
    use iotfast_thing_engine::server::ServerHandle;

    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let secret = "st02-st04-secret";
    let srv = ServerHandle::spawn(cfg, secret).unwrap();
    let token = mgmt_ctx().issue_signed_token(secret);

    let send_http =
        |method: &str, path: &str, body: Option<serde_json::Value>| -> (u16, serde_json::Value) {
            let mut conn = srv.connect();
            let body_bytes = body
                .map(|b| serde_json::to_vec(&b).unwrap())
                .unwrap_or_default();
            let req = format!(
                "{} {} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nX-Engine-Now-Ms: 8000\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                method,
                path,
                token,
                body_bytes.len()
            );
            conn.write_all(req.as_bytes()).unwrap();
            if !body_bytes.is_empty() {
                conn.write_all(&body_bytes).unwrap();
            }
            conn.flush().unwrap();
            let mut raw = String::new();
            conn.read_to_string(&mut raw).unwrap();
            let (hdr, b) = raw.split_once("\r\n\r\n").unwrap();
            let code: u16 = hdr
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            (code, serde_json::from_str(b).unwrap())
        };

    // 1. Verify GET /v1/storage/capabilities returns all 4 backend profiles
    let (cap_code, cap_json) = send_http("GET", "/v1/storage/capabilities", None);
    assert_eq!(cap_code, 200);
    assert_eq!(cap_json["active_backend"], json!("duckdb"));
    assert_eq!(
        cap_json["active_capabilities"]["same_timestamp_distinct"],
        json!("supported")
    );
    let backends = cap_json["backends"].as_array().unwrap();
    assert_eq!(backends.len(), 4);
    assert_eq!(backends[0]["backend"], json!("duckdb"));
    assert_eq!(backends[1]["backend"], json!("tdengine"));
    assert_eq!(backends[1]["same_timestamp_distinct"], json!("conditional"));
    assert_eq!(backends[2]["backend"], json!("timescaledb"));
    assert_eq!(backends[3]["backend"], json!("clickhouse"));

    // 2. Verify strict_lossless: true rejects switching to tdengine (422 CAPABILITY_UNSUPPORTED) without mutating active backend
    let (strict_code, strict_err) = send_http(
        "POST",
        "/v1/storage/timeseries-adapter",
        Some(json!({
            "backend": "tdengine",
            "strict_lossless": true
        })),
    );
    assert_eq!(strict_code, 422);
    assert_eq!(strict_err["error"]["code"], json!("CAPABILITY_UNSUPPORTED"));
    assert_eq!(
        strict_err["error"]["details"][0]["issue"],
        json!("LOSSLESS_SAME_TIMESTAMP_UNSUPPORTED")
    );
    let (_, status_after_reject) = send_http("GET", "/v1/storage/status", None);
    assert_eq!(
        status_after_reject["device_timeseries_store"]["active_backend"],
        json!("duckdb")
    );

    // 3. Switch to tdengine (strict_lossless: false) and verify password redaction (SD-STORAGE-01)
    let (sw_code, sw_json) = send_http(
        "POST",
        "/v1/storage/timeseries-adapter",
        Some(json!({
            "backend": "tdengine",
            "tdengine": {
                "endpoint": "http://127.0.0.1:6041",
                "database": "iotfast_tsdb",
                "super_table": "stb_observations",
                "username": "root",
                "password": "super-secret-password",
                "execution_mode": "local_mirror_with_sql_spool"
            }
        })),
    );
    assert_eq!(sw_code, 200);
    assert_eq!(
        sw_json["device_timeseries_store"]["tdengine_config"]["password"],
        json!("***REDACTED***")
    );
    let (_, st_json) = send_http("GET", "/v1/storage/status", None);
    assert_eq!(
        st_json["device_timeseries_store"]["tdengine_config"]["password"],
        json!("***REDACTED***")
    );

    // 4. Verify ST-04 Non-Contiguous Delivery Checkpoint State Machine & Hole Reconciliation
    {
        let guard = srv.context().state.lock().unwrap();
        guard
            .runtime
            .record_target_delivery_outcome(
                "tdengine",
                "0000000000000001:0000000000000000",
                "ing-000001",
                "committed",
                None,
                5_000,
            )
            .unwrap();
        guard
            .runtime
            .record_target_delivery_outcome(
                "tdengine",
                "0000000000000002:0000000000000256",
                "ing-000002",
                "unknown",
                Some("HTTP timeout waiting for taosadapter response"),
                6_000,
            )
            .unwrap();
        guard
            .runtime
            .record_target_delivery_outcome(
                "tdengine",
                "0000000000000003:0000000000000512",
                "ing-000003",
                "committed",
                None,
                7_000,
            )
            .unwrap();
    }

    let (cp_code, cp_json) = send_http("GET", "/v1/storage/checkpoints?backend=tdengine", None);
    assert_eq!(cp_code, 200);
    assert_eq!(cp_json["target_backend"], json!("tdengine"));
    assert_eq!(
        cp_json["contiguous_checkpoint_wal_pos"],
        json!("0000000000000001:0000000000000000")
    );
    assert_eq!(
        cp_json["highest_seen_wal_pos"],
        json!("0000000000000003:0000000000000512")
    );
    assert_eq!(cp_json["has_unreconciled_holes"], json!(true));
    assert_eq!(cp_json["unknown_count"], json!(1));
    assert_eq!(cp_json["holes"].as_array().unwrap().len(), 1);

    // Reconcile hole 0000000000000002:0000000000000256 via isolate_gap and verify checkpoint advances to 0000000000000003:0000000000000512
    let (rec_code, rec_json) = send_http(
        "POST",
        "/v1/storage/checkpoints:reconcile",
        Some(json!({
            "target_backend": "tdengine",
            "wal_position": "0000000000000002:0000000000000256",
            "resolution": "isolate_gap",
            "reason": "Verified batch was corrupt at gateway; isolated gap for audit",
            "operator_id": "op-alice"
        })),
    );
    assert_eq!(rec_code, 200);
    assert_eq!(
        rec_json["contiguous_checkpoint_wal_pos"],
        json!("0000000000000003:0000000000000512")
    );
    assert_eq!(rec_json["has_unreconciled_holes"], json!(false));
    assert_eq!(rec_json["isolated_gap_count"], json!(1));
    assert_eq!(rec_json["holes"][0]["status"], json!("isolated_gap"));
    assert_eq!(rec_json["holes"][0]["operator_id"], json!("op-alice"));
}

#[test]
fn d01_single_instance_lock_stream_epoch_and_d10_consistent_backup_verify_work() {
    use iotfast_thing_engine::error::ErrorCode;
    use iotfast_thing_engine::server::ServerHandle;

    // 1. Invalid EngineConfig fails closed inside EngineRuntime::open
    let tmp_bad = tempfile::tempdir().unwrap();
    let mut bad_cfg = EngineConfig::for_production(tmp_bad.path());
    bad_cfg.max_future_skew_ms = 0;
    let err = EngineRuntime::open(bad_cfg).err().expect("invalid config must fail");
    assert_eq!(err.code, ErrorCode::ValidationFailed);

    // 2. Single-instance directory lock (D01.2) and unique stream_epoch across restarts
    let tmp_lock = tempfile::tempdir().unwrap();
    let lock_cfg = EngineConfig::for_test(tmp_lock.path());
    let rt1 = EngineRuntime::open(lock_cfg.clone()).unwrap();
    let epoch1 = rt1.stream_epoch.clone();
    let dup_err = EngineRuntime::open(lock_cfg.clone())
        .err()
        .expect("second active instance on same data_dir must fail");
    assert_eq!(dup_err.code, ErrorCode::RevisionConflict);
    assert_eq!(dup_err.http_status(), 409);
    drop(rt1);

    let rt2 = EngineRuntime::open(lock_cfg).unwrap();
    assert_ne!(rt2.stream_epoch, epoch1);
    drop(rt2);

    // 3. Consistent online backup and isolated restore verification over HTTP (D10.1)
    let tmp_srv = tempfile::tempdir().unwrap();
    let srv_cfg = EngineConfig::for_test(tmp_srv.path());
    let secret = "d10-backup-secret";
    let srv = ServerHandle::spawn(srv_cfg.clone(), secret).unwrap();
    let m_token = mgmt_ctx().issue_signed_token(secret);
    let g_token = gateway_ctx().issue_signed_token(secret);

    let send_http_with_token =
        |tok: &str, method: &str, path: &str, body: Option<serde_json::Value>| -> (u16, serde_json::Value) {
            let mut conn = srv.connect();
            let body_bytes = body
                .map(|b| serde_json::to_vec(&b).unwrap())
                .unwrap_or_default();
            let req = format!(
                "{} {} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nX-Engine-Now-Ms: 15000\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                method,
                path,
                tok,
                body_bytes.len()
            );
            conn.write_all(req.as_bytes()).unwrap();
            if !body_bytes.is_empty() {
                conn.write_all(&body_bytes).unwrap();
            }
            conn.flush().unwrap();
            let mut raw = String::new();
            conn.read_to_string(&mut raw).unwrap();
            let (hdr, b) = raw.split_once("\r\n\r\n").unwrap();
            let code: u16 = hdr
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            (code, serde_json::from_str(b).unwrap())
        };
    let send_http = |method: &str, path: &str, body: Option<serde_json::Value>| {
        send_http_with_token(&m_token, method, path, body)
    };

    assert_eq!(
        send_http(
            "POST",
            "/v1/products",
            Some(json!({
                "product_id": "prod-pump",
                "product_key": "booster_pump",
                "name": "Booster Pump"
            }))
        )
        .0,
        201
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/models",
            Some(json!({
                "model_revision_id": "pump-r1",
                "product_id": "prod-pump",
                "model_version": "1.0.0",
                "properties": [
                    {
                        "identifier": "outlet_pressure",
                        "name": "Outlet Pressure",
                        "value_type": "float64",
                        "unit_code": "MPa",
                        "unit_symbol": "MPa",
                        "min_value": 0.0,
                        "max_value": 5.0,
                        "required": true
                    }
                ]
            }))
        )
        .0,
        201
    );
    assert_eq!(
        send_http("POST", "/v1/models/pump-r1:publish", None).0,
        200
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/assets",
            Some(json!({
                "asset_id": "asset-room-1",
                "parent_asset_id": null,
                "slug": "room-1",
                "name": "Pump Room 1"
            }))
        )
        .0,
        201
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/devices",
            Some(json!({
                "device_id": "pump-001",
                "device_key": "pump-001",
                "device_name": "Pump #1",
                "product_id": "prod-pump",
                "model_revision_id": "pump-r1",
                "asset_id": "asset-room-1"
            }))
        )
        .0,
        201
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/sources/grants",
            Some(json!({
                "source_id": "gw-01",
                "device_id": "pump-001",
                "valid_from_ms": 1000
            }))
        )
        .0,
        200
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/bindings",
            Some(json!({
                "binding_revision_id": "bind-pump-001-r1",
                "device_id": "pump-001",
                "model_revision_id": "pump-r1",
                "source_id": "gw-01",
                "normalization_owner": "gateway",
                "points": [
                    {
                        "property": "outlet_pressure",
                        "register_address": "40001",
                        "factor": 0.01,
                        "unit_code": "MPa",
                        "heartbeat_ms": 60000
                    }
                ]
            }))
        )
        .0,
        201
    );
    assert_eq!(
        send_http("POST", "/v1/bindings/bind-pump-001-r1:validate", None).0,
        200
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/bindings/bind-pump-001-r1:activate",
            Some(json!({
                "expected_device_revision": 1,
                "activated_at_ms": 2000,
                "manual_stop_window": {
                    "window_start_ms": 1500,
                    "window_end_ms": 2500,
                    "operator_id": "op-alice"
                }
            }))
        )
        .0,
        200
    );
    assert_eq!(
        send_http_with_token(
            &g_token,
            "POST",
            "/v1/telemetry",
            Some(json!({
                "spec": "iotfast.telemetry.v1",
                "message_id": "msg-bkp-1",
                "source_id": "gw-01",
                "source_epoch": "ep-1",
                "device_id": "pump-001",
                "model_revision_id": "pump-r1",
                "binding_revision_id": "bind-pump-001-r1",
                "points": [
                    {
                        "property": "outlet_pressure",
                        "value": 1.65,
                        "source_at_ms": 12000,
                        "source_time_origin": "device_clock",
                        "source_seq": "1",
                        "quality": { "state": "good", "source_protocol": "modbus" },
                        "delivery": { "replayed": false }
                    }
                ]
            }))
        )
        .0,
        202
    );

    // Create online backup bkp-001
    let (bkp_code, bkp_json) = send_http(
        "POST",
        "/v1/storage/backups",
        Some(json!({ "backup_id": "bkp-001" })),
    );
    assert_eq!(bkp_code, 201);
    assert_eq!(bkp_json["backup_id"], json!("bkp-001"));
    assert!(bkp_json["files"].as_array().unwrap().len() >= 3);

    // List backups
    let (list_code, list_json) = send_http("GET", "/v1/storage/backups", None);
    assert_eq!(list_code, 200);
    assert_eq!(list_json["items"].as_array().unwrap().len(), 1);
    assert_eq!(list_json["items"][0]["backup_id"], json!("bkp-001"));

    // Verify backup in isolated restore directory
    let (ver_code, ver_json) = send_http("POST", "/v1/storage/backups/bkp-001:verify", None);
    assert_eq!(ver_code, 200);
    assert_eq!(ver_json["verified"], json!(true));
    assert_eq!(ver_json["restored_sqlite_tables"], json!(14));
    assert_eq!(ver_json["restored_devices"], json!(1));
    assert_eq!(ver_json["restored_history_observations"], json!(1));
    assert!(ver_json["issues"].as_array().unwrap().is_empty());

    // Corrupt a byte in backups/bkp-001/data/business_metadata.sqlite and confirm SHA-256 verification fails
    let sqlite_bkp_file = srv_cfg
        .data_dir
        .join("backups")
        .join("bkp-001")
        .join("data")
        .join("business_metadata.sqlite");
    let mut raw_sqlite = std::fs::read(&sqlite_bkp_file).unwrap();
    let last_idx = raw_sqlite.len() - 1;
    raw_sqlite[last_idx] ^= 0xFF;
    std::fs::write(&sqlite_bkp_file, raw_sqlite).unwrap();

    let (ver2_code, ver2_json) = send_http("POST", "/v1/storage/backups/bkp-001:verify", None);
    assert_eq!(ver2_code, 200);
    assert_eq!(ver2_json["verified"], json!(false));
    let issues = ver2_json["issues"].as_array().unwrap();
    assert!(!issues.is_empty());
    assert!(issues[0].as_str().unwrap().contains("SHA-256 mismatch"));
}
