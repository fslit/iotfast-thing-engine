use iotfast_thing_engine::asset_uns::{
    AssetUnsService, CreateAssetRequest, InstantiateDeviceRequest,
};
use iotfast_thing_engine::binding::{
    ActivateBindingRequest, BindingService, CreateBindingDraftRequest, GatewayBindingAck,
    PointBindingDraft, StaticPropertyBindingDraft,
};
use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::d00::{
    TelemetryDelivery, TelemetryEnvelope, TelemetryPoint, TelemetryQuality, ValueType,
};
use iotfast_thing_engine::error::ErrorCode;
use iotfast_thing_engine::model_catalog::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use iotfast_thing_engine::northbound::{
    AggregationSpec, DeviceBusinessView, HistoryObservationItem, HistoryQueryRequest,
    HistoryQueryResponse, TimeRange,
};
use iotfast_thing_engine::scope::{CallerKind, ScopeFilter, VerifiedScopeContext};
use iotfast_thing_engine::storage_duckdb::{DurabilityPreference, EngineRuntime};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

fn gateway_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_kind: CallerKind::SouthboundSource,
        caller_service_id: "gateway-01".into(),
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec!["telemetry:ingest".into(), "source_state:report".into()],
        scope_filter: ScopeFilter::default(),
        operator_id: None,
    }
}

fn line01_reader_ctx() -> VerifiedScopeContext {
    VerifiedScopeContext {
        tenant_id: "ten-8iic".into(),
        project_id: "proj-01".into(),
        caller_service_id: "svc-ems-oee-line01".into(),
        caller_kind: CallerKind::NorthboundService,
        issued_at_ms: 1_000,
        expires_at_ms: 90_000_000,
        allowed_actions: vec![
            "view:read".into(),
            "shadow:read".into(),
            "history:query".into(),
        ],
        scope_filter: ScopeFilter {
            asset_ids: vec!["asset-line-01".into()],
            include_asset_descendants: true,
            device_ids: vec![],
            resource_ids: vec![],
            uri_prefixes: vec!["uns://ten-8iic/line-01/".into()],
            tenant_wide: false,
        },
        operator_id: None,
    }
}

/// 测试 1：存储能力边界、事件时间完整性诚实披露（R07）、跨版本单位冲突防护（NB08）与已知缺口披露（NB10）
#[test]
fn p1_storage_capability_boundaries_and_cross_version_unit_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut rt = EngineRuntime::open(cfg).unwrap();

    let m_ctx = mgmt_ctx();
    let gw_ctx = gateway_ctx();
    let nb_ctx = line01_reader_ctx();

    // 1. 创建产品与两个跨版本单位变更的物模型修订：r1 使用 kPa，r2 使用 MPa
    ModelCatalogService::create_product(
        rt.conn(),
        &m_ctx,
        CreateProductRequest {
            product_id: "prod-pump".into(),
            product_key: "circulation-pump".into(),
            name: "循环水泵".into(),
        },
    )
    .unwrap();

    ModelCatalogService::create_model_draft(
        rt.conn(),
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r1-kpa".into(),
            product_id: "prod-pump".into(),
            model_version: "1.0.0".into(),
            properties: vec![ModelPropertyDraft {
                identifier: "outlet_pressure".into(),
                name: "出口压力".into(),
                value_type: ValueType::Float64,
                unit_code: Some("kPa".into()),
                unit_symbol: Some("kPa".into()),
                min_value: Some(0.0),
                max_value: Some(2500.0),
                required: true,
            }],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(rt.conn(), &m_ctx, "pump-model-r1-kpa", 2_000).unwrap();

    ModelCatalogService::create_model_draft(
        rt.conn(),
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "pump-model-r2-mpa".into(),
            product_id: "prod-pump".into(),
            model_version: "2.0.0".into(),
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
    ModelCatalogService::publish_model(rt.conn(), &m_ctx, "pump-model-r2-mpa", 3_000).unwrap();

    AssetUnsService::create_asset(
        rt.conn(),
        &m_ctx,
        CreateAssetRequest {
            asset_id: "asset-line-01".into(),
            parent_asset_id: None,
            slug: "line-01".into(),
            name: "一产线".into(),
        },
    )
    .unwrap();

    AssetUnsService::instantiate_device(
        rt.conn(),
        &m_ctx,
        InstantiateDeviceRequest {
            device_id: "pump-001".into(),
            device_key: "pump-001".into(),
            device_name: "1号循环泵".into(),
            product_id: "prod-pump".into(),
            model_revision_id: "pump-model-r1-kpa".into(),
            asset_id: "asset-line-01".into(),
        },
    )
    .unwrap();

    BindingService::register_source_and_grant(rt.conn(), &m_ctx, "gateway-01", "pump-001", 5_000)
        .unwrap();

    // 2. 激活 r1 绑定（kPa）并写入 100_000ms 与 110_000ms 两条观测
    let b1 = BindingService::create_binding_draft(
        rt.conn(),
        &m_ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r1-kpa".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "outlet_pressure".into(),
                register_address: "40010".into(),
                factor: 10.0,
                unit_code: Some("kPa".into()),
                deadband: None,
                heartbeat_ms: None,
            }],
        },
    )
    .unwrap();
    let b1_ready =
        BindingService::validate_binding(rt.conn(), &m_ctx, &b1.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
        &m_ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r1".into(),
            expected_device_revision: 1,
            activated_at_ms: 10_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                binding_revision_id: "pump-001-bind-r1".into(),
                config_hash: b1_ready.config_hash,
                acknowledged_at_ms: 9_900,
            },
        },
    )
    .unwrap();

    for (mid, ts, seq, val) in [
        ("msg-kpa-1", 100_000i64, "1", 410.0f64),
        ("msg-kpa-2", 110_000i64, "2", 430.0f64),
    ] {
        rt.ingest_telemetry(
            &gw_ctx,
            TelemetryEnvelope {
                spec: "iotfast.telemetry.v1".into(),
                message_id: mid.into(),
                source_id: "gateway-01".into(),
                source_epoch: "epoch-001".into(),
                device_id: "pump-001".into(),
                model_revision_id: "pump-model-r1-kpa".into(),
                binding_revision_id: "pump-001-bind-r1".into(),
                points: vec![TelemetryPoint {
                    property: "outlet_pressure".into(),
                    value: json!(val),
                    source_at_ms: ts,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: seq.into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                }],
            },
            DurabilityPreference::Durable,
            ts + 15,
        )
        .unwrap();
    }

    // 3. 切换并激活 r2 绑定（MPa），在 130_000ms 写入 0.45 MPa
    let b2 = BindingService::create_binding_draft(
        rt.conn(),
        &m_ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "pump-001-bind-r2".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r2-mpa".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![PointBindingDraft {
                property: "outlet_pressure".into(),
                register_address: "40010".into(),
                factor: 0.01,
                unit_code: Some("MPa".into()),
                deadband: None,
                heartbeat_ms: None,
            }],
        },
    )
    .unwrap();
    let b2_ready =
        BindingService::validate_binding(rt.conn(), &m_ctx, &b2.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
        &m_ctx,
        ActivateBindingRequest {
            binding_revision_id: "pump-001-bind-r2".into(),
            expected_device_revision: 2,
            activated_at_ms: 120_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-002".into(),
                binding_revision_id: "pump-001-bind-r2".into(),
                config_hash: b2_ready.config_hash,
                acknowledged_at_ms: 119_900,
            },
        },
    )
    .unwrap();

    rt.ingest_telemetry(
        &gw_ctx,
        TelemetryEnvelope {
            spec: "iotfast.telemetry.v1".into(),
            message_id: "msg-mpa-1".into(),
            source_id: "gateway-01".into(),
            source_epoch: "epoch-002".into(),
            device_id: "pump-001".into(),
            model_revision_id: "pump-model-r2-mpa".into(),
            binding_revision_id: "pump-001-bind-r2".into(),
            points: vec![TelemetryPoint {
                property: "outlet_pressure".into(),
                value: json!(0.45),
                source_at_ms: 130_000,
                source_time_origin: "gateway_sample".into(),
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
        130_020,
    )
    .unwrap();

    // 4. 验证 R07（本地默认目标 local-duckdb 能力边界）与跨版本 raw 查询保留各自历史版本的真实单位
    let raw_resp = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 140_000,
                },
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 50,
                cursor: None,
            },
        )
        .unwrap();

    assert_eq!(raw_resp.target_id, "local-duckdb");
    assert_eq!(raw_resp.consistency, "checkpoint_consistent");
    assert!(!raw_resp.partial);
    assert!(raw_resp.known_gaps.is_empty());
    // R07: 禁止用 WAL checkpoint 伪造事件时间完整性
    assert!(raw_resp.coverage.contiguous_wal_position.is_some());
    assert_eq!(raw_resp.coverage.event_time_completeness, "unknown");
    assert_eq!(raw_resp.coverage.event_time_complete_through_ms, None);

    // 跨版本 raw 模式保留各自时期的真实 unit_code 与 model_revision_id
    assert_eq!(raw_resp.items.len(), 3);
    assert_eq!(raw_resp.items[0].model_revision_id, "pump-model-r1-kpa");
    assert_eq!(raw_resp.items[0].unit_code.as_deref(), Some("kPa"));
    assert_eq!(raw_resp.items[0].value, json!(410.0));
    assert_eq!(raw_resp.items[1].model_revision_id, "pump-model-r1-kpa");
    assert_eq!(raw_resp.items[1].unit_code.as_deref(), Some("kPa"));
    assert_eq!(raw_resp.items[1].value, json!(430.0));
    assert_eq!(raw_resp.items[2].model_revision_id, "pump-model-r2-mpa");
    assert_eq!(raw_resp.items[2].unit_code.as_deref(), Some("MPa"));
    assert_eq!(raw_resp.items[2].value, json!(0.45));

    // 5. 验证 NB08：跨不兼容单位区间 [100_000, 140_000) 执行 aggregate 查询必须返回 422 CROSS_VERSION_INCOMPATIBLE
    let cross_agg_err = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 140_000,
                },
                mode: "aggregate".into(),
                aggregation: Some(AggregationSpec {
                    bucket_ms: 60_000,
                    functions: vec!["avg".into(), "min".into(), "max".into()],
                }),
                quality_states: vec![],
                order: "asc".into(),
                limit: 50,
                cursor: None,
            },
        )
        .unwrap_err();
    assert_eq!(cross_agg_err.code, ErrorCode::CrossVersionIncompatible);
    assert_eq!(cross_agg_err.http_status(), 422);

    // 而在同一版本单位区间 [100_000, 120_000) 内执行 aggregate 查询正常成功
    let single_rev_agg = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 120_000,
                },
                mode: "aggregate".into(),
                aggregation: Some(AggregationSpec {
                    bucket_ms: 20_000,
                    functions: vec!["avg".into()],
                }),
                quality_states: vec![],
                order: "asc".into(),
                limit: 50,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(single_rev_agg.buckets.len(), 1);
    assert_eq!(single_rev_agg.buckets[0].count, 2);
    assert_eq!(single_rev_agg.buckets[0].avg, Some(420.0));

    // 6. 验证 NB10：登记 delivery_gap 后查询覆盖该区间的历史，必须诚实标记 partial = true 并返回 known_gaps 明细
    rt.conn()
        .execute(
            "INSERT INTO delivery_gap (gap_id, tenant_id, resource_id, start_ms, end_ms, reason)
             VALUES ('gap-pump-001-01', 'ten-8iic', 'res-pump-001-outlet-pressure', 112_000, 118_000, 'gateway_buffer_overflow_isolated')",
            [],
        )
        .unwrap();

    let gap_resp = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-pump-001-outlet-pressure".into()],
                range: TimeRange {
                    start_ms: 100_000,
                    end_ms: 140_000,
                },
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 50,
                cursor: None,
            },
        )
        .unwrap();
    assert!(gap_resp.partial);
    assert_eq!(gap_resp.known_gaps.len(), 1);
    assert_eq!(gap_resp.known_gaps[0].gap_id, "gap-pump-001-01");
    assert_eq!(
        gap_resp.known_gaps[0].resource_id,
        "res-pump-001-outlet-pressure"
    );
    assert_eq!(gap_resp.known_gaps[0].start_ms, 112_000);
    assert_eq!(gap_resp.known_gaps[0].end_ms, 118_000);
    assert_eq!(
        gap_resp.known_gaps[0].reason,
        "gateway_buffer_overflow_isolated"
    );
}

// ============================================================================
// P1 确定性衍生指标（OEE / 能耗 / 单位产品能耗）契约结构与计算器
// 严格遵循 docs/specifications/derived-metrics-oee-energy.md §3 - §8
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricLineageRef {
    pub input_resource_id: String,
    pub property: String,
    pub source_kind: String, // "collected" | "static" | "external_fact"
    pub observation_id: Option<String>,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub quality: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OeeComponents {
    pub availability_ratio: f64,
    pub performance_ratio: f64,
    pub quality_ratio: f64,
    pub availability_pct: f64,
    pub performance_pct: f64,
    pub quality_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedMetricSnapshot {
    pub resource_id: String,
    pub scope_id: String,
    pub metric: String,
    pub value: Option<f64>,
    pub display_pct: Option<f64>,
    pub unit: String,
    pub quality: String,            // "good" | "uncertain"
    pub calculation_status: String, // "final" | "uncertain" | "uncalculable"
    pub error_reason: Option<String>,
    pub algorithm_id: String,
    pub algorithm_version: String,
    pub window: TimeRange,
    pub components: Option<OeeComponents>,
    pub lineage: Vec<MetricLineageRef>,
}

/// 基于物模型历史观测计算区间耗电量（kWh）与单位合格品能耗（kWh/件）
fn evaluate_energy_and_specific_energy(
    scope_id: &str,
    window: TimeRange,
    energy_history: &HistoryQueryResponse,
    good_output_count: i64,
    good_output_quality: &str,
) -> (DerivedMetricSnapshot, DerivedMetricSnapshot) {
    let build_uncalculable =
        |metric: &str, unit: &str, status: &str, reason: &str, lineage: Vec<MetricLineageRef>| {
            DerivedMetricSnapshot {
                resource_id: format!("res-{}-{}", scope_id, metric.replace('_', "-")),
                scope_id: scope_id.to_string(),
                metric: metric.to_string(),
                value: None,
                display_pct: None,
                unit: unit.to_string(),
                quality: "uncertain".to_string(),
                calculation_status: status.to_string(),
                error_reason: Some(reason.to_string()),
                algorithm_id: format!("{}.standard", metric),
                algorithm_version: "1.0.0".to_string(),
                window: window.clone(),
                components: None,
                lineage,
            }
        };

    if energy_history.partial || !energy_history.known_gaps.is_empty() {
        let e = build_uncalculable(
            "energy_consumption",
            "kWh",
            "uncertain",
            "WINDOW_HAS_KNOWN_DELIVERY_GAP",
            vec![],
        );
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncertain",
            "WINDOW_HAS_KNOWN_DELIVERY_GAP",
            vec![],
        );
        return (e, s);
    }

    if energy_history.items.len() < 2 {
        let e = build_uncalculable(
            "energy_consumption",
            "kWh",
            "uncalculable",
            "INSUFFICIENT_BOUNDARY_OBSERVATIONS",
            vec![],
        );
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncalculable",
            "INSUFFICIENT_BOUNDARY_OBSERVATIONS",
            vec![],
        );
        return (e, s);
    }

    let first_obs: &HistoryObservationItem = energy_history.items.first().unwrap();
    let last_obs: &HistoryObservationItem = energy_history.items.last().unwrap();

    let lineage = vec![
        MetricLineageRef {
            input_resource_id: first_obs.resource_id.clone(),
            property: first_obs.property.clone(),
            source_kind: "collected".into(),
            observation_id: Some(first_obs.observation_id.clone()),
            model_revision_id: first_obs.model_revision_id.clone(),
            binding_revision_id: first_obs.binding_revision_id.clone(),
            quality: first_obs.quality.clone(),
        },
        MetricLineageRef {
            input_resource_id: last_obs.resource_id.clone(),
            property: last_obs.property.clone(),
            source_kind: "collected".into(),
            observation_id: Some(last_obs.observation_id.clone()),
            model_revision_id: last_obs.model_revision_id.clone(),
            binding_revision_id: last_obs.binding_revision_id.clone(),
            quality: last_obs.quality.clone(),
        },
    ];

    // 检查输入质量：任意点位非 good 时拒绝输出伪造正常结果
    if energy_history.items.iter().any(|it| it.quality != "good") {
        let e = build_uncalculable(
            "energy_consumption",
            "kWh",
            "uncertain",
            "INPUT_OBSERVATION_QUALITY_NOT_GOOD",
            lineage.clone(),
        );
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncertain",
            "INPUT_OBSERVATION_QUALITY_NOT_GOOD",
            lineage,
        );
        return (e, s);
    }

    // 检查单位一致性
    if first_obs.unit_code.as_deref() != Some("kWh") || last_obs.unit_code.as_deref() != Some("kWh")
    {
        let e = build_uncalculable(
            "energy_consumption",
            "kWh",
            "uncalculable",
            "INCOMPATIBLE_ENERGY_UNIT",
            lineage.clone(),
        );
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncalculable",
            "INCOMPATIBLE_ENERGY_UNIT",
            lineage,
        );
        return (e, s);
    }

    let start_kwh = first_obs.value.as_f64().unwrap_or(f64::NAN);
    let end_kwh = last_obs.value.as_f64().unwrap_or(f64::NAN);
    let delta_kwh = end_kwh - start_kwh;

    // §6.1: 电表清零/回卷负差值绝不能简单取绝对值，必须标记 uncalculable
    if !delta_kwh.is_finite() || delta_kwh < 0.0 {
        let e = build_uncalculable(
            "energy_consumption",
            "kWh",
            "uncalculable",
            "NEGATIVE_METER_DELTA_ROLLOVER_OR_RESET",
            lineage.clone(),
        );
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncalculable",
            "NEGATIVE_METER_DELTA_ROLLOVER_OR_RESET",
            lineage,
        );
        return (e, s);
    }

    let energy_snap = DerivedMetricSnapshot {
        resource_id: format!("res-{}-energy-consumption", scope_id),
        scope_id: scope_id.to_string(),
        metric: "energy_consumption".to_string(),
        value: Some(delta_kwh),
        display_pct: None,
        unit: "kWh".to_string(),
        quality: "good".to_string(),
        calculation_status: "final".to_string(),
        error_reason: None,
        algorithm_id: "energy_consumption.standard".to_string(),
        algorithm_version: "1.0.0".to_string(),
        window: window.clone(),
        components: None,
        lineage: lineage.clone(),
    };

    if good_output_quality != "good" {
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncertain",
            "OUTPUT_COUNT_QUALITY_NOT_GOOD",
            lineage,
        );
        return (energy_snap, s);
    }

    if good_output_count <= 0 {
        let s = build_uncalculable(
            "specific_energy",
            "kWh/件",
            "uncalculable",
            "ZERO_OR_NEGATIVE_GOOD_OUTPUT_DENOMINATOR",
            lineage,
        );
        return (energy_snap, s);
    }

    let specific_val = ((delta_kwh / (good_output_count as f64)) * 1e6).round() / 1e6;
    let specific_snap = DerivedMetricSnapshot {
        resource_id: format!("res-{}-specific-energy", scope_id),
        scope_id: scope_id.to_string(),
        metric: "specific_energy".to_string(),
        value: Some(specific_val),
        display_pct: None,
        unit: "kWh/件".to_string(),
        quality: "good".to_string(),
        calculation_status: "final".to_string(),
        error_reason: None,
        algorithm_id: "specific_energy.standard".to_string(),
        algorithm_version: "1.0.0".to_string(),
        window,
        components: None,
        lineage,
    };

    (energy_snap, specific_snap)
}

/// 基于物模型视图静态配置（ideal_cycle_min）+ 采集产量历史观测 + 班次上下文计算 OEE
fn evaluate_oee_over_thing_model(
    scope_id: &str,
    window: TimeRange,
    device_view: &DeviceBusinessView,
    production_obs: &HistoryObservationItem,
    planned_production_min: f64,
    actual_run_min: f64,
    good_count: i64,
    context_quality: &str,
) -> DerivedMetricSnapshot {
    let mut lineage = Vec::new();

    let ideal_cycle_prop = device_view.properties.get("ideal_cycle_min");
    if let Some(prop) = ideal_cycle_prop {
        lineage.push(MetricLineageRef {
            input_resource_id: prop.resource_id.clone(),
            property: "ideal_cycle_min".into(),
            source_kind: "static".into(),
            observation_id: None,
            model_revision_id: device_view.model_revision_id.clone(),
            binding_revision_id: prop
                .binding_revision_id
                .clone()
                .unwrap_or_else(|| "unbound".into()),
            quality: prop.quality.clone(),
        });
    }

    lineage.push(MetricLineageRef {
        input_resource_id: production_obs.resource_id.clone(),
        property: production_obs.property.clone(),
        source_kind: "collected".into(),
        observation_id: Some(production_obs.observation_id.clone()),
        model_revision_id: production_obs.model_revision_id.clone(),
        binding_revision_id: production_obs.binding_revision_id.clone(),
        quality: production_obs.quality.clone(),
    });

    let reject = |status: &str, reason: &str, lin: Vec<MetricLineageRef>| DerivedMetricSnapshot {
        resource_id: format!("res-{}-oee", scope_id),
        scope_id: scope_id.to_string(),
        metric: "oee".to_string(),
        value: None,
        display_pct: None,
        unit: "ratio".to_string(),
        quality: "uncertain".to_string(),
        calculation_status: status.to_string(),
        error_reason: Some(reason.to_string()),
        algorithm_id: "oee.standard".to_string(),
        algorithm_version: "1.0.0".to_string(),
        window: window.clone(),
        components: None,
        lineage: lin,
    };

    let Some(ideal_prop) = ideal_cycle_prop else {
        return reject("uncalculable", "MISSING_STATIC_IDEAL_CYCLE_MIN", lineage);
    };

    if ideal_prop.quality != "good"
        || production_obs.quality != "good"
        || context_quality != "good"
    {
        return reject("uncertain", "INPUT_QUALITY_NOT_GOOD", lineage);
    }

    let ideal_cycle_min = ideal_prop.value.as_f64().unwrap_or(0.0);
    let total_count = production_obs.value.as_i64().unwrap_or(0);

    // §4 & §5.1: 分母为 0 返回 uncalculable 及原因，禁止输出 0% 掩盖异常
    if planned_production_min <= 0.0
        || actual_run_min <= 0.0
        || total_count <= 0
        || ideal_cycle_min <= 0.0
        || good_count < 0
        || good_count > total_count
    {
        return reject("uncalculable", "INVALID_OR_ZERO_DENOMINATOR", lineage);
    }

    let availability = actual_run_min / planned_production_min;
    let performance = (ideal_cycle_min * (total_count as f64)) / actual_run_min;
    let quality_rate = (good_count as f64) / (total_count as f64);
    let oee_ratio = availability * performance * quality_rate;

    let round2 = |v: f64| (v * 100.0).round() / 100.0;
    let round4 = |v: f64| (v * 10_000.0).round() / 10_000.0;

    DerivedMetricSnapshot {
        resource_id: format!("res-{}-oee", scope_id),
        scope_id: scope_id.to_string(),
        metric: "oee".to_string(),
        value: Some(round4(oee_ratio)),
        display_pct: Some(round2(oee_ratio * 100.0)),
        unit: "ratio".to_string(),
        quality: "good".to_string(),
        calculation_status: "final".to_string(),
        error_reason: None,
        algorithm_id: "oee.standard".to_string(),
        algorithm_version: "1.0.0".to_string(),
        window,
        components: Some(OeeComponents {
            availability_ratio: availability,
            performance_ratio: performance,
            quality_ratio: quality_rate,
            availability_pct: round2(availability * 100.0),
            performance_pct: round2(performance * 100.0),
            quality_pct: round2(quality_rate * 100.0),
        }),
        lineage,
    }
}

/// 测试 2：基于物模型动态采集属性 + 静态配置属性执行 OEE 与能耗/单位产品能耗契约验证
#[test]
fn p1_derived_metrics_oee_and_specific_energy_over_thing_model() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let mut rt = EngineRuntime::open(cfg).unwrap();

    let m_ctx = mgmt_ctx();
    let gw_ctx = gateway_ctx();
    let nb_ctx = line01_reader_ctx();

    // 1. 创建产线能效物模型：包含 collected 属性（energy_total, active_power, production_count）
    //    与 static 属性（ideal_cycle_min, rated_power）
    ModelCatalogService::create_product(
        rt.conn(),
        &m_ctx,
        CreateProductRequest {
            product_id: "prod-line-unit".into(),
            product_key: "smart-pump-assembly-unit".into(),
            name: "智能水泵装配产线单元".into(),
        },
    )
    .unwrap();

    ModelCatalogService::create_model_draft(
        rt.conn(),
        &m_ctx,
        CreateModelDraftRequest {
            model_revision_id: "line-unit-model-r1".into(),
            product_id: "prod-line-unit".into(),
            model_version: "1.0.0".into(),
            properties: vec![
                ModelPropertyDraft {
                    identifier: "energy_total".into(),
                    name: "累计电能".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("kWh".into()),
                    unit_symbol: Some("kWh".into()),
                    min_value: Some(0.0),
                    max_value: Some(1_000_000.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "active_power".into(),
                    name: "有功功率".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("kW".into()),
                    unit_symbol: Some("kW".into()),
                    min_value: Some(0.0),
                    max_value: Some(500.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "production_count".into(),
                    name: "班次累计产量".into(),
                    value_type: ValueType::Int64,
                    unit_code: Some("pcs".into()),
                    unit_symbol: Some("件".into()),
                    min_value: Some(0.0),
                    max_value: Some(100_000.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "ideal_cycle_min".into(),
                    name: "理想单件节拍".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("min_per_pcs".into()),
                    unit_symbol: Some("min/件".into()),
                    min_value: Some(0.01),
                    max_value: Some(60.0),
                    required: true,
                },
                ModelPropertyDraft {
                    identifier: "rated_power".into(),
                    name: "额定功率".into(),
                    value_type: ValueType::Float64,
                    unit_code: Some("kW".into()),
                    unit_symbol: Some("kW".into()),
                    min_value: Some(0.1),
                    max_value: Some(500.0),
                    required: true,
                },
            ],
        },
    )
    .unwrap();
    ModelCatalogService::publish_model(rt.conn(), &m_ctx, "line-unit-model-r1", 2_000).unwrap();

    AssetUnsService::create_asset(
        rt.conn(),
        &m_ctx,
        CreateAssetRequest {
            asset_id: "asset-line-01".into(),
            parent_asset_id: None,
            slug: "line-01".into(),
            name: "一产线".into(),
        },
    )
    .unwrap();

    AssetUnsService::instantiate_device(
        rt.conn(),
        &m_ctx,
        InstantiateDeviceRequest {
            device_id: "line-01-unit".into(),
            device_key: "line-01-unit".into(),
            device_name: "一产线主单元".into(),
            product_id: "prod-line-unit".into(),
            model_revision_id: "line-unit-model-r1".into(),
            asset_id: "asset-line-01".into(),
        },
    )
    .unwrap();

    BindingService::register_source_and_grant(
        rt.conn(),
        &m_ctx,
        "gateway-01",
        "line-01-unit",
        5_000,
    )
    .unwrap();

    // 2. 创建含动态点位与静态配置点位的绑定：ideal_cycle_min = 0.5 min/件, rated_power = 60.0 kW
    let bind = BindingService::create_binding_draft_with_statics(
        rt.conn(),
        &m_ctx,
        CreateBindingDraftRequest {
            binding_revision_id: "line-01-unit-bind-r1".into(),
            device_id: "line-01-unit".into(),
            model_revision_id: "line-unit-model-r1".into(),
            source_id: "gateway-01".into(),
            normalization_owner: "gateway".into(),
            server_scale_factor: None,
            points: vec![
                PointBindingDraft {
                    property: "energy_total".into(),
                    register_address: "40100".into(),
                    factor: 0.1,
                    unit_code: Some("kWh".into()),
                    deadband: None,
                    heartbeat_ms: None,
                },
                PointBindingDraft {
                    property: "active_power".into(),
                    register_address: "40102".into(),
                    factor: 0.1,
                    unit_code: Some("kW".into()),
                    deadband: None,
                    heartbeat_ms: None,
                },
                PointBindingDraft {
                    property: "production_count".into(),
                    register_address: "40200".into(),
                    factor: 1.0,
                    unit_code: Some("pcs".into()),
                    deadband: None,
                    heartbeat_ms: None,
                },
            ],
        },
        vec![
            StaticPropertyBindingDraft {
                property: "ideal_cycle_min".into(),
                value: json!(0.5),
                unit_code: Some("min_per_pcs".into()),
            },
            StaticPropertyBindingDraft {
                property: "rated_power".into(),
                value: json!(60.0),
                unit_code: Some("kW".into()),
            },
        ],
    )
    .unwrap();
    let bind_ready =
        BindingService::validate_binding(rt.conn(), &m_ctx, &bind.binding_revision_id).unwrap();
    BindingService::activate_binding(
        rt.conn(),
        &m_ctx,
        ActivateBindingRequest {
            binding_revision_id: "line-01-unit-bind-r1".into(),
            expected_device_revision: 1,
            activated_at_ms: 10_000,
            gateway_ack: GatewayBindingAck {
                source_id: "gateway-01".into(),
                source_epoch: "epoch-shift-01".into(),
                binding_revision_id: "line-01-unit-bind-r1".into(),
                config_hash: bind_ready.config_hash,
                acknowledged_at_ms: 9_900,
            },
        },
    )
    .unwrap();

    // 3. 上报班次期初（200_000ms）与期末（228_800ms）遥测观测
    //    - 累计电能：期初 10,250.0 kWh -> 期末 10,730.0 kWh（净耗电 480.0 kWh）
    //    - 班次总产量（OEE 向量）：720 件
    rt.ingest_telemetry(
        &gw_ctx,
        TelemetryEnvelope {
            spec: "iotfast.telemetry.v1".into(),
            message_id: "msg-shift-start".into(),
            source_id: "gateway-01".into(),
            source_epoch: "epoch-shift-01".into(),
            device_id: "line-01-unit".into(),
            model_revision_id: "line-unit-model-r1".into(),
            binding_revision_id: "line-01-unit-bind-r1".into(),
            points: vec![
                TelemetryPoint {
                    property: "energy_total".into(),
                    value: json!(10250.0),
                    source_at_ms: 200_000,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "1".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
                TelemetryPoint {
                    property: "active_power".into(),
                    value: json!(58.5),
                    source_at_ms: 200_000,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "2".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
            ],
        },
        DurabilityPreference::Durable,
        200_010,
    )
    .unwrap();

    rt.ingest_telemetry(
        &gw_ctx,
        TelemetryEnvelope {
            spec: "iotfast.telemetry.v1".into(),
            message_id: "msg-shift-end".into(),
            source_id: "gateway-01".into(),
            source_epoch: "epoch-shift-01".into(),
            device_id: "line-01-unit".into(),
            model_revision_id: "line-unit-model-r1".into(),
            binding_revision_id: "line-01-unit-bind-r1".into(),
            points: vec![
                TelemetryPoint {
                    property: "energy_total".into(),
                    value: json!(10730.0),
                    source_at_ms: 228_800,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "3".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
                TelemetryPoint {
                    property: "active_power".into(),
                    value: json!(59.2),
                    source_at_ms: 228_800,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "4".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
                TelemetryPoint {
                    property: "production_count".into(),
                    value: json!(720),
                    source_at_ms: 228_800,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "5".into(),
                    quality: TelemetryQuality {
                        state: "good".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: None,
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
            ],
        },
        DurabilityPreference::Durable,
        228_810,
    )
    .unwrap();

    // 4. 从引擎读取业务视图与历史观测
    let view = rt
        .get_device_view(&nb_ctx, "line-01-unit", 230_000)
        .unwrap();
    assert_eq!(
        view.properties["ideal_cycle_min"].binding_state,
        "bound_static"
    );
    assert_eq!(view.properties["ideal_cycle_min"].value, json!(0.5));
    assert_eq!(view.properties["rated_power"].binding_state, "bound_static");
    assert_eq!(view.properties["rated_power"].value, json!(60.0));

    let shift_window = TimeRange {
        start_ms: 200_000,
        end_ms: 230_000,
    };

    let energy_hist = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-line-01-unit-energy-total".into()],
                range: shift_window.clone(),
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 20,
                cursor: None,
            },
        )
        .unwrap();

    let prod_hist = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-line-01-unit-production-count".into()],
                range: shift_window.clone(),
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 20,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(prod_hist.items.len(), 1);

    // 5. 验证 §6.2 标准能耗与单位产品能耗验收向量：
    //    期初 10,250.0 kWh, 期末 10,730.0 kWh -> energy_consumption = 480.0 kWh
    //    合格产量 800 件 -> specific_energy = 0.60 kWh/件
    let (energy_res, specific_res) = evaluate_energy_and_specific_energy(
        "line-01",
        shift_window.clone(),
        &energy_hist,
        800,
        "good",
    );
    assert_eq!(energy_res.calculation_status, "final");
    assert_eq!(energy_res.quality, "good");
    assert_eq!(energy_res.value, Some(480.0));
    assert_eq!(energy_res.unit, "kWh");
    assert_eq!(energy_res.lineage.len(), 2);
    assert!(energy_res.lineage[0].observation_id.is_some());
    assert_eq!(
        energy_res.lineage[0].model_revision_id,
        "line-unit-model-r1"
    );

    assert_eq!(specific_res.calculation_status, "final");
    assert_eq!(specific_res.quality, "good");
    assert_eq!(specific_res.value, Some(0.60));
    assert_eq!(specific_res.unit, "kWh/件");

    // 6. 验证 §5.2 标准 OEE 验收向量：
    //    计划时间 480 min、实际运行 400 min、理想节拍 0.5 min/件（来自物模型 static 属性）、总产 720 件、良品 684 件
    //    -> A = 83.33%, P = 90.00%, Q = 95.00%, OEE = 71.25% (ratio = 0.7125)
    let oee_res = evaluate_oee_over_thing_model(
        "line-01",
        shift_window.clone(),
        &view,
        &prod_hist.items[0],
        480.0,
        400.0,
        684,
        "good",
    );
    assert_eq!(oee_res.calculation_status, "final");
    assert_eq!(oee_res.quality, "good");
    assert_eq!(oee_res.value, Some(0.7125));
    assert_eq!(oee_res.display_pct, Some(71.25));
    let comp = oee_res.components.as_ref().unwrap();
    assert_eq!(comp.availability_pct, 83.33);
    assert_eq!(comp.performance_pct, 90.00);
    assert_eq!(comp.quality_pct, 95.00);
    // 血缘同时包含 static 属性与 collected 观测
    assert_eq!(oee_res.lineage.len(), 2);
    assert_eq!(oee_res.lineage[0].source_kind, "static");
    assert_eq!(oee_res.lineage[0].property, "ideal_cycle_min");
    assert_eq!(oee_res.lineage[1].source_kind, "collected");
    assert_eq!(oee_res.lineage[1].property, "production_count");
    assert!(oee_res.lineage[1].observation_id.is_some());

    // 7. 验证异常边界防线 1：电表回卷/清零未登记（期末 10,100.0 < 期初 10,730.0，负差值）
    //    严禁取绝对值伪造正能耗，必须返回 uncalculable
    rt.ingest_telemetry(
        &gw_ctx,
        TelemetryEnvelope {
            spec: "iotfast.telemetry.v1".into(),
            message_id: "msg-rollover".into(),
            source_id: "gateway-01".into(),
            source_epoch: "epoch-shift-01".into(),
            device_id: "line-01-unit".into(),
            model_revision_id: "line-unit-model-r1".into(),
            binding_revision_id: "line-01-unit-bind-r1".into(),
            points: vec![TelemetryPoint {
                property: "energy_total".into(),
                value: json!(10100.0),
                source_at_ms: 250_000,
                source_time_origin: "gateway_sample".into(),
                source_seq: "6".into(),
                quality: TelemetryQuality {
                    state: "good".into(),
                    source_protocol: Some("modbus".into()),
                    source_code: None,
                },
                delivery: TelemetryDelivery { replayed: false },
            }],
        },
        DurabilityPreference::Durable,
        250_010,
    )
    .unwrap();

    let rollover_window = TimeRange {
        start_ms: 220_000,
        end_ms: 260_000,
    };
    let rollover_hist = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-line-01-unit-energy-total".into()],
                range: rollover_window.clone(),
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 20,
                cursor: None,
            },
        )
        .unwrap();
    let (rollover_energy, rollover_specific) = evaluate_energy_and_specific_energy(
        "line-01",
        rollover_window,
        &rollover_hist,
        800,
        "good",
    );
    assert_eq!(rollover_energy.calculation_status, "uncalculable");
    assert_eq!(rollover_energy.quality, "uncertain");
    assert_eq!(rollover_energy.value, None);
    assert_eq!(
        rollover_energy.error_reason.as_deref(),
        Some("NEGATIVE_METER_DELTA_ROLLOVER_OR_RESET")
    );
    assert_eq!(rollover_specific.calculation_status, "uncalculable");
    assert_eq!(rollover_specific.value, None);

    // 8. 验证异常边界防线 2：输入包含 bad 质量观测时拒算并标记 uncertain
    rt.ingest_telemetry(
        &gw_ctx,
        TelemetryEnvelope {
            spec: "iotfast.telemetry.v1".into(),
            message_id: "msg-bad-quality".into(),
            source_id: "gateway-01".into(),
            source_epoch: "epoch-shift-01".into(),
            device_id: "line-01-unit".into(),
            model_revision_id: "line-unit-model-r1".into(),
            binding_revision_id: "line-01-unit-bind-r1".into(),
            points: vec![
                TelemetryPoint {
                    property: "energy_total".into(),
                    value: json!(10900.0),
                    source_at_ms: 270_000,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "7".into(),
                    quality: TelemetryQuality {
                        state: "bad".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: Some("CRC_ERROR".into()),
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
                TelemetryPoint {
                    property: "production_count".into(),
                    value: json!(750),
                    source_at_ms: 270_000,
                    source_time_origin: "gateway_sample".into(),
                    source_seq: "8".into(),
                    quality: TelemetryQuality {
                        state: "bad".into(),
                        source_protocol: Some("modbus".into()),
                        source_code: Some("SENSOR_DISCONNECTED".into()),
                    },
                    delivery: TelemetryDelivery { replayed: false },
                },
            ],
        },
        DurabilityPreference::Durable,
        270_010,
    )
    .unwrap();

    let bad_window = TimeRange {
        start_ms: 245_000,
        end_ms: 280_000,
    };
    let bad_energy_hist = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-line-01-unit-energy-total".into()],
                range: bad_window.clone(),
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 20,
                cursor: None,
            },
        )
        .unwrap();
    let (bad_energy, _) = evaluate_energy_and_specific_energy(
        "line-01",
        bad_window.clone(),
        &bad_energy_hist,
        500,
        "good",
    );
    assert_eq!(bad_energy.calculation_status, "uncertain");
    assert_eq!(bad_energy.quality, "uncertain");
    assert_eq!(bad_energy.value, None);
    assert_eq!(
        bad_energy.error_reason.as_deref(),
        Some("INPUT_OBSERVATION_QUALITY_NOT_GOOD")
    );

    let bad_prod_hist = rt
        .query_history(
            &nb_ctx,
            HistoryQueryRequest {
                resource_ids: vec!["res-line-01-unit-production-count".into()],
                range: bad_window.clone(),
                mode: "raw".into(),
                aggregation: None,
                quality_states: vec![],
                order: "asc".into(),
                limit: 20,
                cursor: None,
            },
        )
        .unwrap();
    let bad_oee = evaluate_oee_over_thing_model(
        "line-01",
        bad_window,
        &view,
        &bad_prod_hist.items[0],
        480.0,
        400.0,
        700,
        "good",
    );
    assert_eq!(bad_oee.calculation_status, "uncertain");
    assert_eq!(bad_oee.quality, "uncertain");
    assert_eq!(bad_oee.value, None);
    assert_eq!(
        bad_oee.error_reason.as_deref(),
        Some("INPUT_QUALITY_NOT_GOOD")
    );

    // 9. 验证异常边界防线 3：分母为 0（合格产量为 0 或计划生产时间为 0）时返回 uncalculable
    let (_, zero_denom_specific) = evaluate_energy_and_specific_energy(
        "line-01",
        shift_window.clone(),
        &energy_hist,
        0,
        "good",
    );
    assert_eq!(zero_denom_specific.calculation_status, "uncalculable");
    assert_eq!(zero_denom_specific.value, None);
    assert_eq!(
        zero_denom_specific.error_reason.as_deref(),
        Some("ZERO_OR_NEGATIVE_GOOD_OUTPUT_DENOMINATOR")
    );

    let zero_planned_oee = evaluate_oee_over_thing_model(
        "line-01",
        shift_window,
        &view,
        &prod_hist.items[0],
        0.0,
        400.0,
        684,
        "good",
    );
    assert_eq!(zero_planned_oee.calculation_status, "uncalculable");
    assert_eq!(zero_planned_oee.value, None);
    assert_eq!(
        zero_planned_oee.error_reason.as_deref(),
        Some("INVALID_OR_ZERO_DENOMINATOR")
    );
}

#[test]
fn p1_http_adapters_derived_metrics_and_ping_probes_work() {
    use iotfast_thing_engine::server::ServerHandle;
    use std::io::{Read, Write};

    let tmp = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::for_test(tmp.path());
    let secret = "p1-http-secret";
    let srv = ServerHandle::spawn(cfg, secret).unwrap();

    let mut admin_ctx = mgmt_ctx();
    admin_ctx.allowed_actions.push("view:read".into());
    admin_ctx.allowed_actions.push("telemetry:ingest".into());
    let token = admin_ctx.issue_signed_token(secret);
    let gw_token = gateway_ctx().issue_signed_token(secret);

    let send_http_with_token =
        |tok: &str, method: &str, path: &str, body: Option<serde_json::Value>| -> (u16, serde_json::Value) {
            let mut conn = srv.connect();
            let body_bytes = body
                .map(|b| serde_json::to_vec(&b).unwrap())
                .unwrap_or_default();
            let req = format!(
                "{} {} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nX-Engine-Now-Ms: 25000\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
        send_http_with_token(&token, method, path, body)
    };

    // 1. Verify GET /v1/ping and GET /v1/ingest/ping
    let (ping_code, ping_json) = send_http("GET", "/v1/ping", None);
    assert_eq!(ping_code, 200);
    assert_eq!(ping_json["status"], json!("ok"));
    assert_eq!(ping_json["plane"], json!("northbound"));
    assert_eq!(ping_json["tenant_id"], json!("ten-8iic"));

    let (iping_code, iping_json) = send_http_with_token(&gw_token, "GET", "/v1/ingest/ping", None);
    assert_eq!(iping_code, 200);
    assert_eq!(iping_json["status"], json!("ok"));
    assert_eq!(iping_json["plane"], json!("southbound_ingest"));
    assert_eq!(iping_json["wal_ready"], json!(true));

    // 2. Register and validate edge adapter capability declaration
    let (ad_code, ad_json) = send_http(
        "POST",
        "/v1/adapters",
        Some(json!({
            "adapter_id": "adapter-modbus-gw01",
            "protocol": "modbus_tcp",
            "supported_value_types": ["float64", "int64", "bool"],
            "supported_transforms": ["gateway_normalized", "scale_offset"],
            "max_batch_size": 500,
            "supports_deadband": true
        })),
    );
    assert_eq!(ad_code, 201);
    assert_eq!(ad_json["adapter_id"], json!("adapter-modbus-gw01"));

    let (list_ad_code, list_ad_json) = send_http("GET", "/v1/adapters", None);
    assert_eq!(list_ad_code, 200);
    assert_eq!(list_ad_json["items"].as_array().unwrap().len(), 1);

    let (val_ok_code, val_ok_json) = send_http(
        "POST",
        "/v1/adapters/adapter-modbus-gw01:validate-binding",
        Some(json!({
            "value_type": "float64",
            "transform": "gateway_normalized",
            "batch_size": 100,
            "requires_deadband": true
        })),
    );
    assert_eq!(val_ok_code, 200);
    assert_eq!(val_ok_json["compatible"], json!(true));

    let (val_bad_code, _val_bad_json) = send_http(
        "POST",
        "/v1/adapters/adapter-modbus-gw01:validate-binding",
        Some(json!({
            "value_type": "float64",
            "transform": "custom_polynomial",
            "batch_size": 1000,
            "requires_deadband": true
        })),
    );
    assert_eq!(val_bad_code, 422);

    // 3. Provision product -> model -> asset -> device -> binding -> telemetry for derived metric evaluation
    assert_eq!(
        send_http(
            "POST",
            "/v1/products",
            Some(json!({
                "product_id": "prod-pump",
                "product_key": "pump_unit",
                "name": "Pump Unit"
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
                "model_revision_id": "pump-model-r1",
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
                        "max_value": 10.0,
                        "required": true
                    },
                    {
                        "identifier": "rated_power",
                        "name": "Rated Power",
                        "value_type": "float64",
                        "unit_code": "kW",
                        "unit_symbol": "kW",
                        "min_value": 0.0,
                        "max_value": 500.0,
                        "required": true
                    }
                ]
            }))
        )
        .0,
        201
    );
    assert_eq!(
        send_http("POST", "/v1/models/pump-model-r1:publish", None).0,
        200
    );
    assert_eq!(
        send_http(
            "POST",
            "/v1/assets",
            Some(json!({
                "asset_id": "asset-line-01",
                "parent_asset_id": null,
                "slug": "line-01",
                "name": "Line 01"
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
                "device_name": "Pump 001",
                "product_id": "prod-pump",
                "model_revision_id": "pump-model-r1",
                "asset_id": "asset-line-01"
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
                "source_id": "gateway-01",
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
                "model_revision_id": "pump-model-r1",
                "source_id": "gateway-01",
                "normalization_owner": "gateway",
                "points": [
                    {
                        "property": "outlet_pressure",
                        "register_address": "40001",
                        "factor": 0.01,
                        "unit_code": "MPa",
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
            &gw_token,
            "POST",
            "/v1/telemetry",
            Some(json!({
                "spec": "iotfast.telemetry.v1",
                "message_id": "msg-dm-1",
                "source_id": "gateway-01",
                "source_epoch": "ep-1",
                "device_id": "pump-001",
                "model_revision_id": "pump-model-r1",
                "binding_revision_id": "bind-pump-001-r1",
                "points": [
                    {
                        "property": "outlet_pressure",
                        "value": 1.5,
                        "source_at_ms": 20000,
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

    // 4. Create, list, and evaluate derived metric (45.0 / 1.5 = 30.0)
    let (dm_code, dm_json) = send_http(
        "POST",
        "/v1/derived-metrics",
        Some(json!({
            "metric_id": "metric-specific-power",
            "name": "单位压力功率比",
            "product_id": "prod-pump",
            "model_revision_id": "pump-model-r1",
            "expression": "rated_power / outlet_pressure",
            "input_properties": ["rated_power", "outlet_pressure"],
            "output_unit_code": "kW/MPa",
            "quality_policy": "reject_on_bad"
        })),
    );
    assert_eq!(dm_code, 201);
    assert_eq!(dm_json["metric_id"], json!("metric-specific-power"));

    let (list_dm_code, list_dm_json) = send_http("GET", "/v1/derived-metrics", None);
    assert_eq!(list_dm_code, 200);
    assert_eq!(list_dm_json["items"].as_array().unwrap().len(), 1);

    let (eval_code, eval_json) = send_http(
        "POST",
        "/v1/derived-metrics/metric-specific-power:evaluate",
        Some(json!({
            "device_id": "pump-001",
            "evaluated_at_ms": 25000
        })),
    );
    assert_eq!(eval_code, 200);
    assert_eq!(eval_json["computed_value"], json!(30.0));
    assert_eq!(eval_json["unit_code"], json!("kW/MPa"));
    assert_eq!(eval_json["quality"], json!("good"));
    assert!(eval_json["inputs_lineage"]["rated_power"].is_object());
    assert!(eval_json["inputs_lineage"]["outlet_pressure"].is_object());
}
