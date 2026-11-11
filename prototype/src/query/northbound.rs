use crate::asset_uns::AssetUnsService;
use crate::error::{EngineError, ErrorCode, FieldIssue};
use crate::model_catalog::ModelCatalogService;
use crate::scope::VerifiedScopeContext;
use crate::storage_duckdb::EngineRuntime;
use data_encoding::BASE32_NOPAD;
use duckdb::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicePropertyView {
    pub resource_id: String,
    pub name: String,
    pub data_type: String,
    pub unit_code: Option<String>,
    pub unit_symbol: Option<String>,
    pub binding_state: String,
    pub observation_state: String,
    pub value: Value,
    pub quality: String,
    pub quality_reason: Option<String>,
    pub freshness: String,
    pub value_sampled_at_ms: Option<i64>,
    pub quality_observed_at_ms: Option<i64>,
    pub received_at_ms: Option<i64>,
    pub source_time_origin: Option<String>,
    pub binding_revision_id: Option<String>,
    pub resource_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceBusinessView {
    pub device_id: String,
    pub device_key: String,
    pub device_name: String,
    pub product_id: String,
    pub model_revision_id: String,
    pub asset_id: String,
    pub canonical_device_uri: String,
    pub active_binding_revision_id: Option<String>,
    pub source_connection_state: String,
    pub view_generated_at_ms: i64,
    pub view_consistency: String,
    pub properties: BTreeMap<String, DevicePropertyView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceShadowView {
    pub device_id: String,
    pub revision: i64,
    pub source_connection_state: String,
    pub properties: BTreeMap<String, DevicePropertyView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregationSpec {
    pub bucket_ms: i64,
    pub functions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryQueryRequest {
    pub resource_ids: Vec<String>,
    pub range: TimeRange,
    pub mode: String,
    pub aggregation: Option<AggregationSpec>,
    pub quality_states: Vec<String>,
    pub order: String,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryObservationItem {
    pub observation_id: String,
    pub resource_id: String,
    pub device_id: String,
    pub property: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub source_at_ms: i64,
    pub received_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub value_type: String,
    pub value: Value,
    pub unit_code: Option<String>,
    pub unit_symbol: Option<String>,
    pub quality: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryCoverage {
    /// Log progress position (`R07`: does NOT imply event-time watermark completeness)
    pub contiguous_wal_position: Option<String>,
    /// Always `None` unless source event-time watermark is provable (`R07`)
    pub event_time_complete_through_ms: Option<i64>,
    pub event_time_completeness: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryGapInfo {
    pub gap_id: String,
    pub resource_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregationBucket {
    pub bucket_start_ms: i64,
    pub bucket_end_ms: i64,
    pub count: usize,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub first: Option<f64>,
    pub last: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryQueryResponse {
    pub target_id: String,
    pub mode: String,
    pub consistency: String,
    pub partial: bool,
    pub coverage: HistoryCoverage,
    pub known_gaps: Vec<DeliveryGapInfo>,
    pub items: Vec<HistoryObservationItem>,
    pub buckets: Vec<AggregationBucket>,
    pub next_cursor: Option<String>,
}

impl EngineRuntime {
    pub fn is_resource_in_scope(
        &self,
        ctx: &VerifiedScopeContext,
        resource_id: &str,
    ) -> Result<bool, EngineError> {
        let row: Option<(String, String, String)> = self
            .conn()
            .query_row(
                "SELECT tenant_id, device_id, canonical_uri FROM resource WHERE resource_id = ?",
                params![resource_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((tenant_id, device_id, canonical_uri)) = row else {
            return Ok(false);
        };
        let asset_id: String = self.conn().query_row(
            "SELECT asset_id FROM device WHERE device_id = ?",
            params![device_id],
            |r| r.get(0),
        )?;
        let asset_chain = AssetUnsService::load_asset_chain(self.conn(), &asset_id)?;
        Ok(ctx.allows_resource(
            &tenant_id,
            &asset_chain,
            &device_id,
            resource_id,
            &canonical_uri,
        ))
    }

    pub fn get_device_view(
        &self,
        ctx: &VerifiedScopeContext,
        device_id: &str,
        now_ms: i64,
    ) -> Result<DeviceBusinessView, EngineError> {
        ctx.validate_active(now_ms)?;
        ctx.require_action("view:read")?;

        let dev_row: Option<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        )> = self
            .conn()
            .query_row(
                "SELECT tenant_id, device_key, device_name, product_id, model_revision_id,
                        active_binding_revision_id, asset_id, canonical_device_uri
                 FROM device WHERE device_id = ? AND tenant_id = ?",
                params![device_id, ctx.tenant_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .ok();

        let (
            tenant_id,
            device_key,
            device_name,
            product_id,
            model_revision_id,
            active_binding_revision_id,
            asset_id,
            canonical_device_uri,
        ) = dev_row.ok_or_else(|| {
            EngineError::new(
                "req-view",
                ErrorCode::ResourceNotFound,
                format!("Device '{}' not found", device_id),
                vec![],
            )
        })?;

        let asset_chain = AssetUnsService::load_asset_chain(self.conn(), &asset_id)?;
        let props = ModelCatalogService::load_properties(self.conn(), &model_revision_id)?;

        let mut bound_set = BTreeSet::new();
        let mut static_map: BTreeMap<String, Value> = BTreeMap::new();
        let mut bind_valid_from_ms: Option<i64> = None;
        let mut conn_state = "unknown".to_string();
        if let Some(ref bind_id) = active_binding_revision_id {
            let mut stmt = self.conn().prepare(
                "SELECT property FROM point_binding WHERE binding_revision_id = ? AND source_kind = 'gateway_measurement'",
            )?;
            let rows = stmt.query_map(params![bind_id], |r| r.get::<_, String>(0))?;
            for r in rows {
                bound_set.insert(r?);
            }
            let mut st_stmt = self.conn().prepare(
                "SELECT property, static_value_json FROM point_binding WHERE binding_revision_id = ? AND source_kind = 'static_config'",
            )?;
            let st_rows = st_stmt.query_map(params![bind_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            for r in st_rows {
                let (prop, v_json) = r?;
                let parsed = v_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                static_map.insert(prop, parsed);
            }
            if let Ok((source_id, vf)) = self.conn().query_row(
                "SELECT source_id, valid_from_ms FROM binding_revision WHERE binding_revision_id = ?",
                params![bind_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            ) {
                bind_valid_from_ms = vf;
                if let Ok(cs) = self.conn().query_row(
                    "SELECT connection_state FROM source_gateway WHERE tenant_id = ? AND source_id = ?",
                    params![tenant_id, source_id],
                    |r| r.get::<_, String>(0),
                ) {
                    conn_state = cs;
                }
            }
        }

        // R09: Filter properties per resource Scope (`allows_resource`); if 0 properties are in Scope, return 404.
        let mut view_props = BTreeMap::new();
        for (prop_id, prop_spec) in &props {
            let resource_id = AssetUnsService::lookup_resource_id(self.conn(), device_id, prop_id);
            let canonical_uri = format!("{}/properties/{}", canonical_device_uri, prop_id);
            if !ctx.allows_resource(
                &tenant_id,
                &asset_chain,
                device_id,
                &resource_id,
                &canonical_uri,
            ) {
                continue;
            }

            if let Some(static_val) = static_map.get(prop_id) {
                view_props.insert(
                    prop_id.clone(),
                    DevicePropertyView {
                        resource_id,
                        name: prop_spec.name.clone(),
                        data_type: prop_spec.value_type.as_str().to_string(),
                        unit_code: prop_spec.unit_code.clone(),
                        unit_symbol: prop_spec.unit_symbol.clone(),
                        binding_state: "bound_static".to_string(),
                        observation_state: "static_configured".to_string(),
                        value: static_val.clone(),
                        quality: "good".to_string(),
                        quality_reason: None,
                        freshness: "static".to_string(),
                        value_sampled_at_ms: bind_valid_from_ms,
                        quality_observed_at_ms: bind_valid_from_ms,
                        received_at_ms: bind_valid_from_ms,
                        source_time_origin: Some("static_config".to_string()),
                        binding_revision_id: active_binding_revision_id.clone(),
                        resource_revision: 0,
                    },
                );
                continue;
            }

            if !bound_set.contains(prop_id) {
                view_props.insert(
                    prop_id.clone(),
                    DevicePropertyView {
                        resource_id,
                        name: prop_spec.name.clone(),
                        data_type: prop_spec.value_type.as_str().to_string(),
                        unit_code: prop_spec.unit_code.clone(),
                        unit_symbol: prop_spec.unit_symbol.clone(),
                        binding_state: "unbound".to_string(),
                        observation_state: "no_observation".to_string(),
                        value: Value::Null,
                        quality: "unknown".to_string(),
                        quality_reason: Some("unbound_property".to_string()),
                        freshness: "unknown".to_string(),
                        value_sampled_at_ms: None,
                        quality_observed_at_ms: None,
                        received_at_ms: None,
                        source_time_origin: None,
                        binding_revision_id: None,
                        resource_revision: 0,
                    },
                );
                continue;
            }

            let latest_row: Option<(String, i64, i64, String, String, i64)> = self
                .conn()
                .query_row(
                    "SELECT value_json, value_sampled_at_ms, received_at_ms, source_time_origin, binding_revision_id, resource_revision
                     FROM latest_value WHERE resource_id = ?",
                    params![resource_id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    },
                )
                .ok();

            let q_row: Option<(String, Option<String>, i64)> = self
                .conn()
                .query_row(
                    "SELECT quality_state, quality_reason, quality_observed_at_ms
                     FROM point_quality_state WHERE resource_id = ?",
                    params![resource_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok();

            match latest_row {
                None => {
                    let (q_state, q_reason, q_at) = match q_row {
                        Some((qs, qr, qa)) => (qs, qr, Some(qa)),
                        None => ("unknown".to_string(), None, None),
                    };
                    view_props.insert(
                        prop_id.clone(),
                        DevicePropertyView {
                            resource_id,
                            name: prop_spec.name.clone(),
                            data_type: prop_spec.value_type.as_str().to_string(),
                            unit_code: prop_spec.unit_code.clone(),
                            unit_symbol: prop_spec.unit_symbol.clone(),
                            binding_state: "bound".to_string(),
                            observation_state: "no_observation".to_string(),
                            value: Value::Null,
                            quality: q_state,
                            quality_reason: q_reason,
                            freshness: "unknown".to_string(),
                            value_sampled_at_ms: None,
                            quality_observed_at_ms: q_at,
                            received_at_ms: None,
                            source_time_origin: None,
                            binding_revision_id: active_binding_revision_id.clone(),
                            resource_revision: 0,
                        },
                    );
                }
                Some((val_json, sampled_at, recv_at, origin, bind_rev_id, res_rev)) => {
                    let (q_state, q_reason, q_at) = match q_row {
                        Some((qs, qr, qa)) => (qs, qr, qa),
                        None => ("good".to_string(), None, sampled_at),
                    };
                    let freshness = if q_state == "bad"
                        || conn_state == "disconnected"
                        || (now_ms - sampled_at) > self.config.stale_threshold_ms
                    {
                        "stale"
                    } else {
                        "fresh"
                    };
                    let parsed_val: Value = serde_json::from_str(&val_json).unwrap_or(Value::Null);
                    view_props.insert(
                        prop_id.clone(),
                        DevicePropertyView {
                            resource_id,
                            name: prop_spec.name.clone(),
                            data_type: prop_spec.value_type.as_str().to_string(),
                            unit_code: prop_spec.unit_code.clone(),
                            unit_symbol: prop_spec.unit_symbol.clone(),
                            binding_state: "bound".to_string(),
                            observation_state: "observed".to_string(),
                            value: parsed_val,
                            quality: q_state,
                            quality_reason: q_reason,
                            freshness: freshness.to_string(),
                            value_sampled_at_ms: Some(sampled_at),
                            quality_observed_at_ms: Some(q_at),
                            received_at_ms: Some(recv_at),
                            source_time_origin: Some(origin),
                            binding_revision_id: Some(bind_rev_id),
                            resource_revision: res_rev,
                        },
                    );
                }
            }
        }

        if view_props.is_empty() {
            return Err(EngineError::new(
                "req-view",
                ErrorCode::ResourceNotFound,
                format!("Device '{}' not found", device_id),
                vec![],
            ));
        }

        Ok(DeviceBusinessView {
            device_id: device_id.to_string(),
            device_key,
            device_name,
            product_id,
            model_revision_id,
            asset_id,
            canonical_device_uri,
            active_binding_revision_id,
            source_connection_state: conn_state,
            view_generated_at_ms: now_ms,
            view_consistency: "per_resource_latest".to_string(),
            properties: view_props,
        })
    }

    pub fn get_device_shadow(
        &self,
        ctx: &VerifiedScopeContext,
        device_id: &str,
        now_ms: i64,
    ) -> Result<DeviceShadowView, EngineError> {
        ctx.validate_active(now_ms)?;
        ctx.require_action("shadow:read")?;
        let mut view_ctx = ctx.clone();
        if !view_ctx.allowed_actions.iter().any(|a| a == "view:read") {
            view_ctx.allowed_actions.push("view:read".to_string());
        }
        let view = self.get_device_view(&view_ctx, device_id, now_ms)?;
        let rev: i64 = self.conn().query_row(
            "SELECT revision FROM device WHERE device_id = ?",
            params![device_id],
            |r| r.get(0),
        )?;
        Ok(DeviceShadowView {
            device_id: view.device_id,
            revision: rev,
            source_connection_state: view.source_connection_state,
            properties: view.properties,
        })
    }

    fn query_fingerprint(req: &HistoryQueryRequest) -> String {
        let mut r_ids = req.resource_ids.clone();
        r_ids.sort();
        let mut q_states = req.quality_states.clone();
        q_states.sort();
        let mut hasher = Sha256::new();
        hasher.update(b"iotfast.query_fp.v1\0");
        for r in &r_ids {
            hasher.update(r.as_bytes());
            hasher.update(b",");
        }
        hasher.update(
            format!(
                "\0{}:{}\0{}\0{}\0",
                req.range.start_ms, req.range.end_ms, req.mode, req.order
            )
            .as_bytes(),
        );
        for q in &q_states {
            hasher.update(q.as_bytes());
            hasher.update(b",");
        }
        hex::encode(hasher.finalize())
    }

    fn encode_cursor(
        &self,
        tenant_id: &str,
        scope_hash: &str,
        query_fp: &str,
        watermark_received_at_ms: i64,
        last_source_at_ms: i64,
        last_obs_id: &str,
    ) -> String {
        let payload = format!(
            "{}|{}|{}|{}|{}|{}",
            tenant_id, scope_hash, query_fp, watermark_received_at_ms, last_source_at_ms, last_obs_id
        );
        let sig = hex::encode(crate::core::scope::hmac_sha256(
            self.config.cursor_signing_secret.as_bytes(),
            payload.as_bytes(),
        ));
        let full = format!("{}|{}", payload, sig);
        BASE32_NOPAD.encode(full.as_bytes()).to_ascii_lowercase()
    }

    fn decode_cursor(
        &self,
        cursor_str: &str,
        expected_tenant: &str,
        expected_scope_hash: &str,
        expected_query_fp: &str,
    ) -> Result<(i64, i64, String), EngineError> {
        let bytes = BASE32_NOPAD
            .decode(cursor_str.to_ascii_uppercase().as_bytes())
            .map_err(|_| {
                EngineError::new(
                    "req-history",
                    ErrorCode::ValidationFailed,
                    "Invalid cursor base32",
                    vec![],
                )
            })?;
        let text = String::from_utf8(bytes).map_err(|_| {
            EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Invalid cursor UTF-8",
                vec![],
            )
        })?;
        let parts: Vec<&str> = text.split('|').collect();
        if parts.len() != 7 {
            return Err(EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Invalid cursor structure",
                vec![],
            ));
        }
        let payload = parts[..6].join("|");
        let sig = parts[6];

        let expected_sig = hex::encode(crate::core::scope::hmac_sha256(
            self.config.cursor_signing_secret.as_bytes(),
            payload.as_bytes(),
        ));
        if sig != expected_sig {
            return Err(EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Cursor signature mismatch",
                vec![],
            ));
        }

        if parts[0] != expected_tenant
            || parts[1] != expected_scope_hash
            || parts[2] != expected_query_fp
        {
            return Err(EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Cursor scope or query fingerprint mismatch (NB09)",
                vec![FieldIssue {
                    field: "cursor".into(),
                    issue: "CURSOR_SCOPE_MISMATCH".into(),
                    description: "Cursor is bound to a different scope or query".into(),
                }],
            ));
        }

        let watermark_received_at_ms = parts[3].parse::<i64>().map_err(|_| {
            EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Invalid cursor watermark_received_at_ms",
                vec![],
            )
        })?;
        let last_ts = parts[4].parse::<i64>().map_err(|_| {
            EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Invalid cursor timestamp",
                vec![],
            )
        })?;
        Ok((watermark_received_at_ms, last_ts, parts[5].to_string()))
    }

    pub fn query_history(
        &self,
        ctx: &VerifiedScopeContext,
        req: HistoryQueryRequest,
    ) -> Result<HistoryQueryResponse, EngineError> {
        if ctx.tenant_id.is_empty()
            || ctx.caller_service_id.is_empty()
            || ctx.expires_at_ms <= ctx.issued_at_ms
        {
            return Err(EngineError::new(
                "req-history-scope",
                ErrorCode::ValidationFailed,
                "Invalid VerifiedScopeContext in query_history",
                vec![],
            ));
        }
        ctx.require_action("history:query")?;
        if req.resource_ids.is_empty() || req.range.start_ms >= req.range.end_ms || req.limit == 0 {
            return Err(EngineError::new(
                "req-history",
                ErrorCode::ValidationFailed,
                "Invalid history query parameters",
                vec![],
            ));
        }

        // R05: Batch authorization check — if ANY requested resource_id is missing or out of Scope,
        // reject the entire request with 404 RESOURCE_NOT_FOUND and empty details!
        for res_id in &req.resource_ids {
            if !self.is_resource_in_scope(ctx, res_id)? {
                return Err(EngineError::new(
                    "req-history",
                    ErrorCode::ResourceNotFound,
                    "One or more requested resource_ids were not found",
                    vec![],
                ));
            }
        }

        let scope_hash = ctx.scope_hash();
        let query_fp = Self::query_fingerprint(&req);
        let (watermark_received_at_ms, cursor_bound) = match &req.cursor {
            Some(c) => {
                let (wm, last_ts, last_obs) =
                    self.decode_cursor(c, &ctx.tenant_id, &scope_hash, &query_fp)?;
                (wm, Some((last_ts, last_obs)))
            }
            None => {
                let wm: i64 = self
                    .conn()
                    .query_row(
                        "SELECT COALESCE(MAX(received_at_ms), 9223372036854775807) FROM history_observation WHERE tenant_id = ?",
                        params![ctx.tenant_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(i64::MAX);
                (wm, None)
            }
        };

        let mut stmt = self.conn().prepare(
            "SELECT observation_id, resource_id, device_id, property, model_revision_id,
                    binding_revision_id, source_at_ms, received_at_ms, source_time_origin,
                    source_seq, value_type, value_json, unit_code, unit_symbol, quality, replayed
             FROM history_observation
             WHERE tenant_id = ? AND source_at_ms >= ? AND source_at_ms < ? AND received_at_ms <= ?
             ORDER BY source_at_ms ASC, observation_id ASC",
        )?;
        let rows = stmt.query_map(
            params![
                ctx.tenant_id,
                req.range.start_ms,
                req.range.end_ms,
                watermark_received_at_ms
            ],
            |r| {
                let val_str: String = r.get(11)?;
                Ok(HistoryObservationItem {
                    observation_id: r.get(0)?,
                    resource_id: r.get(1)?,
                    device_id: r.get(2)?,
                    property: r.get(3)?,
                    model_revision_id: r.get(4)?,
                    binding_revision_id: r.get(5)?,
                    source_at_ms: r.get(6)?,
                    received_at_ms: r.get(7)?,
                    source_time_origin: r.get(8)?,
                    source_seq: r.get(9)?,
                    value_type: r.get(10)?,
                    value: serde_json::from_str(&val_str).unwrap_or(Value::Null),
                    unit_code: r.get(12)?,
                    unit_symbol: r.get(13)?,
                    quality: r.get(14)?,
                    replayed: r.get(15)?,
                })
            },
        )?;

        let req_set: BTreeSet<&str> = req.resource_ids.iter().map(|s| s.as_str()).collect();
        let mut filtered = Vec::new();
        for r in rows {
            let item = r?;
            if !req_set.contains(item.resource_id.as_str()) {
                continue;
            }
            if !req.quality_states.is_empty()
                && !req.quality_states.iter().any(|q| q == &item.quality)
            {
                continue;
            }
            filtered.push(item);
        }

        // Query delivery_gap for overlapping gaps (NB10/I4)
        let mut known_gaps = Vec::new();
        for res_id in &req.resource_ids {
            if let Ok(mut gs) = self.conn().prepare(
                "SELECT gap_id, resource_id, start_ms, end_ms, reason FROM delivery_gap
                 WHERE tenant_id = ? AND resource_id = ? AND start_ms < ? AND end_ms > ?",
            ) {
                if let Ok(rows) = gs.query_map(
                    params![ctx.tenant_id, res_id, req.range.end_ms, req.range.start_ms],
                    |r| {
                        Ok(DeliveryGapInfo {
                            gap_id: r.get(0)?,
                            resource_id: r.get(1)?,
                            start_ms: r.get(2)?,
                            end_ms: r.get(3)?,
                            reason: r.get(4)?,
                        })
                    },
                ) {
                    for r in rows {
                        if let Ok(g) = r {
                            known_gaps.push(g);
                        }
                    }
                }
            }
        }
        let partial = !known_gaps.is_empty();

        // NB08: In aggregate mode, check cross-version unit/type compatibility per resource_id
        if req.mode == "aggregate" {
            let mut seen_unit_type: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
            for item in &filtered {
                let entry = seen_unit_type
                    .entry(item.resource_id.clone())
                    .or_insert_with(|| (item.value_type.clone(), item.unit_code.clone()));
                if entry.0 != item.value_type || entry.1 != item.unit_code {
                    return Err(EngineError::new(
                        "req-history",
                        ErrorCode::CrossVersionIncompatible,
                        format!(
                            "Resource '{}' spans incompatible units/types ({:?} vs {:?}) in aggregate range (NB08)",
                            item.resource_id, entry.1, item.unit_code
                        ),
                        vec![],
                    ));
                }
            }
        }

        // Cursor filtering
        if let Some((last_ts, ref last_obs)) = cursor_bound {
            if req.order == "desc" {
                filtered.retain(|it| {
                    (it.source_at_ms, it.observation_id.as_str()) < (last_ts, last_obs.as_str())
                });
            } else {
                filtered.retain(|it| {
                    (it.source_at_ms, it.observation_id.as_str()) > (last_ts, last_obs.as_str())
                });
            }
        }

        // I4: Honor req.order for sorting
        if req.order == "desc" {
            filtered.sort_by(|a, b| {
                b.source_at_ms
                    .cmp(&a.source_at_ms)
                    .then_with(|| b.observation_id.cmp(&a.observation_id))
            });
        }

        // Compute aggregation buckets if mode == "aggregate"
        let mut buckets = Vec::new();
        if req.mode == "aggregate" {
            if let Some(ref agg) = req.aggregation {
                if agg.bucket_ms > 0 {
                    let mut bucket_map: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
                    for item in &filtered {
                        let bucket_start = req.range.start_ms
                            + ((item.source_at_ms - req.range.start_ms) / agg.bucket_ms)
                                * agg.bucket_ms;
                        let vals = bucket_map.entry(bucket_start).or_default();
                        if let Some(f) = item.value.as_f64() {
                            vals.push(f);
                        }
                    }
                    for (bs, vals) in &bucket_map {
                        let count = vals.len();
                        let avg = if count > 0 {
                            Some(vals.iter().sum::<f64>() / count as f64)
                        } else {
                            None
                        };
                        let min_v = vals.iter().copied().reduce(f64::min);
                        let max_v = vals.iter().copied().reduce(f64::max);
                        let first = vals.first().copied();
                        let last = vals.last().copied();
                        buckets.push(AggregationBucket {
                            bucket_start_ms: *bs,
                            bucket_end_ms: bs + agg.bucket_ms,
                            count,
                            avg,
                            min: min_v,
                            max: max_v,
                            first,
                            last,
                        });
                    }
                }
            }
        }

        let has_more = filtered.len() > req.limit;
        let page_items: Vec<HistoryObservationItem> =
            filtered.into_iter().take(req.limit).collect();
        let next_cursor = if has_more {
            let last = page_items.last().unwrap();
            Some(self.encode_cursor(
                &ctx.tenant_id,
                &scope_hash,
                &query_fp,
                watermark_received_at_ms,
                last.source_at_ms,
                &last.observation_id,
            ))
        } else {
            None
        };

        let cp: Option<String> = self
            .conn()
            .query_row(
                "SELECT contiguous_wal_position FROM target_checkpoint WHERE target_id = 'local-duckdb'",
                [],
                |r| r.get(0),
            )
            .ok();

        Ok(HistoryQueryResponse {
            target_id: "local-duckdb".to_string(),
            mode: req.mode.clone(),
            consistency: "checkpoint_consistent".to_string(),
            partial,
            coverage: HistoryCoverage {
                contiguous_wal_position: cp,
                event_time_complete_through_ms: None,
                event_time_completeness: "unknown".to_string(),
            },
            known_gaps,
            items: if req.mode == "aggregate" {
                Vec::new()
            } else {
                page_items
            },
            buckets,
            next_cursor,
        })
    }
}
