use crate::d00::{
    compute_telemetry_payload_hash, derive_observation_id, parse_source_seq, validate_message_id,
    validate_source_epoch, TelemetryEnvelope, TypedPointValue, ValueType,
};
use crate::error::{EngineError, ErrorCode, FieldIssue};
use crate::model_catalog::ModelCatalogService;
use crate::scope::{CallerKind, VerifiedScopeContext};
use crate::wal::{WalPointRecord, WalRecord};
use duckdb::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidatedTelemetryBatch {
    pub tenant_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub message_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub payload_hash: String,
    pub received_at_ms: i64,
    pub observation_ids: BTreeMap<String, String>,
    pub points: Vec<WalPointRecord>,
}

impl ValidatedTelemetryBatch {
    pub fn to_wal_record(&self, ingest_id: &str) -> WalRecord {
        WalRecord {
            ingest_id: ingest_id.to_string(),
            tenant_id: self.tenant_id.clone(),
            source_id: self.source_id.clone(),
            source_epoch: self.source_epoch.clone(),
            message_id: self.message_id.clone(),
            device_id: self.device_id.clone(),
            model_revision_id: self.model_revision_id.clone(),
            binding_revision_id: self.binding_revision_id.clone(),
            payload_hash: self.payload_hash.clone(),
            received_at_ms: self.received_at_ms,
            points: self.points.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePointState {
    pub property: String,
    pub quality_state: String,
    pub quality_reason: String,
    pub source_protocol: Option<String>,
    pub source_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStateEnvelope {
    pub spec: String,
    pub event_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub device_id: String,
    pub binding_revision_id: String,
    pub observed_at_ms: i64,
    pub connection_state: String,
    pub point_states: Vec<SourcePointState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedSourceStateEvent {
    pub tenant_id: String,
    pub event_key: String,
    pub event_hash: String,
    pub envelope: SourceStateEnvelope,
}

pub struct IngestService;

impl IngestService {
    pub fn verify_source_grant(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        source_id: &str,
        device_id: &str,
        now_ms: i64,
    ) -> Result<(), EngineError> {
        ctx.validate_active(now_ms)?;
        if ctx.caller_kind != CallerKind::SouthboundSource || ctx.caller_service_id != source_id {
            return Err(EngineError::new(
                "req-ingest-auth",
                ErrorCode::SourceNotAuthorized,
                format!(
                    "Caller '{}' is not authorized as source '{}'",
                    ctx.caller_service_id, source_id
                ),
                vec![],
            ));
        }

        let granted: Option<(i64, Option<i64>)> = conn
            .query_row(
                "SELECT valid_from_ms, valid_to_ms FROM source_device_grant
                 WHERE tenant_id = ? AND source_id = ? AND device_id = ?",
                params![ctx.tenant_id, source_id, device_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();

        let Some((from_ms, to_ms)) = granted else {
            return Err(EngineError::new(
                "req-ingest-auth",
                ErrorCode::SourceNotAuthorized,
                format!(
                    "Source '{}' is not authorized to report for device '{}'",
                    source_id, device_id
                ),
                vec![],
            ));
        };
        if now_ms < from_ms {
            return Err(EngineError::new(
                "req-ingest-auth",
                ErrorCode::SourceNotAuthorized,
                "source_device_grant is not yet active (now_ms < valid_from_ms)",
                vec![],
            ));
        }
        if let Some(end_ms) = to_ms {
            if now_ms >= end_ms {
                return Err(EngineError::new(
                    "req-ingest-auth",
                    ErrorCode::SourceNotAuthorized,
                    "source_device_grant has expired",
                    vec![],
                ));
            }
        }
        Ok(())
    }

    pub fn validate_telemetry(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        msg: &TelemetryEnvelope,
        now_ms: i64,
        max_future_skew_ms: i64,
    ) -> Result<ValidatedTelemetryBatch, EngineError> {
        Self::validate_telemetry_inner(conn, ctx, msg, now_ms, max_future_skew_ms, true)
    }

    pub fn validate_telemetry_inner(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        msg: &TelemetryEnvelope,
        now_ms: i64,
        max_future_skew_ms: i64,
        require_active_binding_window: bool,
    ) -> Result<ValidatedTelemetryBatch, EngineError> {
        ctx.require_action("telemetry:ingest")?;
        Self::verify_source_grant(conn, ctx, &msg.source_id, &msg.device_id, now_ms)?;

        let mut issues = Vec::new();
        if msg.spec != "iotfast.telemetry.v1" {
            issues.push(FieldIssue {
                field: "spec".into(),
                issue: "UNSUPPORTED_SPEC".into(),
                description: format!("Expected 'iotfast.telemetry.v1', got '{}'", msg.spec),
            });
        }
        if !validate_message_id(&msg.message_id) {
            issues.push(FieldIssue {
                field: "message_id".into(),
                issue: "INVALID_MESSAGE_ID".into(),
                description: "Invalid message_id".into(),
            });
        }
        if !validate_source_epoch(&msg.source_epoch) {
            issues.push(FieldIssue {
                field: "source_epoch".into(),
                issue: "INVALID_SOURCE_EPOCH".into(),
                description: "Invalid source_epoch".into(),
            });
        }
        if msg.points.is_empty() {
            issues.push(FieldIssue {
                field: "points".into(),
                issue: "EMPTY_POINTS".into(),
                description: "points must not be empty".into(),
            });
        }
        if !issues.is_empty() {
            return Err(EngineError::new(
                "req-ingest",
                ErrorCode::ValidationFailed,
                "Telemetry envelope validation failed",
                issues,
            ));
        }

        let dev_row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT model_revision_id, active_binding_revision_id FROM device WHERE device_id = ? AND tenant_id = ?",
                params![msg.device_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();

        let (active_model_id, active_bind_id) = dev_row.ok_or_else(|| {
            EngineError::new(
                "req-ingest",
                ErrorCode::ResourceNotFound,
                format!("Device '{}' not found", msg.device_id),
                vec![],
            )
        })?;

        let bind_row: Option<(String, String, String, Option<i64>, Option<i64>)> = conn
            .query_row(
                "SELECT device_id, model_revision_id, status, valid_from_ms, valid_to_ms
                 FROM binding_revision WHERE binding_revision_id = ? AND tenant_id = ?",
                params![msg.binding_revision_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();

        let (bind_dev_id, bind_model_id, _bind_status, valid_from_ms, valid_to_ms) = bind_row
            .ok_or_else(|| {
                EngineError::new(
                    "req-ingest",
                    ErrorCode::RevisionConflict,
                    format!("Unknown binding_revision_id '{}'", msg.binding_revision_id),
                    vec![],
                )
            })?;

        if bind_dev_id != msg.device_id || bind_model_id != msg.model_revision_id {
            return Err(EngineError::new(
                "req-ingest",
                ErrorCode::RevisionConflict,
                "binding_revision_id does not match device_id or model_revision_id",
                vec![],
            ));
        }

        if require_active_binding_window
            && (active_bind_id.as_deref() != Some(msg.binding_revision_id.as_str())
                || active_model_id != msg.model_revision_id)
        {
            let window_ok = match (valid_from_ms, valid_to_ms) {
                (Some(from_ms), Some(to_ms)) => msg
                    .points
                    .iter()
                    .all(|p| p.source_at_ms >= from_ms && p.source_at_ms < to_ms),
                _ => false,
            };
            if !window_ok {
                return Err(EngineError::new(
                    "req-ingest",
                    ErrorCode::RevisionConflict,
                    format!(
                        "Binding '{}' is superseded and sampling window is not proven",
                        msg.binding_revision_id
                    ),
                    vec![],
                ));
            }
        }

        let props = ModelCatalogService::load_properties(conn, &msg.model_revision_id)?;
        let mut stmt = conn.prepare(
            "SELECT property, deadband, heartbeat_ms FROM point_binding
             WHERE binding_revision_id = ? AND source_kind = 'gateway_measurement'",
        )?;
        let rows = stmt.query_map(params![msg.binding_revision_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let mut bound_points = BTreeMap::new();
        for r in rows {
            let (p, db, hb) = r?;
            bound_points.insert(p, (db, hb));
        }

        let mut seen_props = BTreeSet::new();
        let mut clock_skew_issues = Vec::new();
        let mut item_types = BTreeMap::new();
        let mut wal_points = Vec::with_capacity(msg.points.len());
        let mut obs_ids = BTreeMap::new();

        for (idx, pt) in msg.points.iter().enumerate() {
            if !seen_props.insert(pt.property.clone()) {
                issues.push(FieldIssue {
                    field: format!("points[{}].property", idx),
                    issue: "DUPLICATE_PROPERTY".into(),
                    description: format!("Duplicate property '{}'", pt.property),
                });
                continue;
            }
            let Some(prop_spec) = props.get(&pt.property) else {
                issues.push(FieldIssue {
                    field: format!("points[{}].property", idx),
                    issue: "UNKNOWN_PROPERTY".into(),
                    description: format!("Unknown property '{}'", pt.property),
                });
                continue;
            };
            let Some((deadband, heartbeat_ms)) = bound_points.get(&pt.property).cloned() else {
                issues.push(FieldIssue {
                    field: format!("points[{}].property", idx),
                    issue: "UNBOUND_PROPERTY".into(),
                    description: format!("Property '{}' is not bound", pt.property),
                });
                continue;
            };

            if pt.value.is_null() {
                issues.push(FieldIssue {
                    field: format!("points[{}].value", idx),
                    issue: "NULL_VALUE_FORBIDDEN".into(),
                    description: "Telemetry value must not be null (SC09)".into(),
                });
                continue;
            }

            let typed_val = match (prop_spec.value_type, &pt.value) {
                (ValueType::Float64, Value::Number(n)) => {
                    if let Some(f) = n.as_f64() {
                        if !f.is_finite() {
                            issues.push(FieldIssue {
                                field: format!("points[{}].value", idx),
                                issue: "NON_FINITE_FLOAT".into(),
                                description: "float64 must be finite".into(),
                            });
                            continue;
                        }
                        if let Some(min_v) = prop_spec.min_value {
                            if f < min_v {
                                issues.push(FieldIssue {
                                    field: format!("points[{}].value", idx),
                                    issue: "OUT_OF_RANGE".into(),
                                    description: format!("{} < min {}", f, min_v),
                                });
                                continue;
                            }
                        }
                        if let Some(max_v) = prop_spec.max_value {
                            if f > max_v {
                                issues.push(FieldIssue {
                                    field: format!("points[{}].value", idx),
                                    issue: "OUT_OF_RANGE".into(),
                                    description: format!("{} > max {}", f, max_v),
                                });
                                continue;
                            }
                        }
                        TypedPointValue::Float64(if f == 0.0 { 0.0 } else { f })
                    } else {
                        issues.push(FieldIssue {
                            field: format!("points[{}].value", idx),
                            issue: "TYPE_MISMATCH".into(),
                            description: "Expected float64".into(),
                        });
                        continue;
                    }
                }
                (ValueType::Int64, Value::Number(n)) => {
                    if let Some(i) = n.as_i64() {
                        TypedPointValue::Int64(i)
                    } else {
                        issues.push(FieldIssue {
                            field: format!("points[{}].value", idx),
                            issue: "TYPE_MISMATCH".into(),
                            description: "Expected int64".into(),
                        });
                        continue;
                    }
                }
                (ValueType::Bool, Value::Bool(b)) => TypedPointValue::Bool(*b),
                (ValueType::Text, Value::String(s)) => TypedPointValue::Text(s.clone()),
                _ => {
                    issues.push(FieldIssue {
                        field: format!("points[{}].value", idx),
                        issue: "TYPE_MISMATCH".into(),
                        description: "Value type mismatch".into(),
                    });
                    continue;
                }
            };

            if !matches!(
                pt.source_time_origin.as_str(),
                "device_clock" | "protocol_timestamp" | "gateway_sample" | "external_source"
            ) {
                issues.push(FieldIssue {
                    field: format!("points[{}].source_time_origin", idx),
                    issue: "INVALID_TIME_ORIGIN".into(),
                    description: "Invalid source_time_origin".into(),
                });
                continue;
            }

            let seq_u64 = match parse_source_seq(&pt.source_seq) {
                Ok(v) => v,
                Err(e) => {
                    issues.push(FieldIssue {
                        field: format!("points[{}].source_seq", idx),
                        issue: "INVALID_SOURCE_SEQ".into(),
                        description: e,
                    });
                    continue;
                }
            };

            if !matches!(
                pt.quality.state.as_str(),
                "good" | "uncertain" | "bad" | "unknown"
            ) {
                issues.push(FieldIssue {
                    field: format!("points[{}].quality.state", idx),
                    issue: "INVALID_QUALITY_STATE".into(),
                    description: "Invalid quality.state".into(),
                });
                continue;
            }

            if pt.source_at_ms > now_ms + max_future_skew_ms {
                clock_skew_issues.push(FieldIssue {
                    field: format!("points[{}].source_at_ms", idx),
                    issue: "CLOCK_SKEW_EXCEEDED".into(),
                    description: "source_at_ms exceeds max_future_skew_ms (SC07)".into(),
                });
                continue;
            }

            let obs_id = derive_observation_id(
                &ctx.tenant_id,
                &msg.source_id,
                &msg.source_epoch,
                &msg.message_id,
                &pt.property,
            )?;
            obs_ids.insert(pt.property.clone(), obs_id.clone());
            item_types.insert(pt.property.clone(), prop_spec.value_type);

            let resource_id =
                crate::catalog::asset::AssetUnsService::lookup_resource_id(conn, &msg.device_id, &pt.property);

            let (val_f64, val_i64, val_bool, val_text) = match &typed_val {
                TypedPointValue::Float64(f) => (Some(*f), None, None, None),
                TypedPointValue::Int64(i) => (None, Some(*i), None, None),
                TypedPointValue::Bool(b) => (None, None, Some(*b), None),
                TypedPointValue::Text(s) => (None, None, None, Some(s.clone())),
            };

            wal_points.push(WalPointRecord {
                observation_id: obs_id,
                resource_id,
                property: pt.property.clone(),
                value_type: prop_spec.value_type.as_str().to_string(),
                value_json: typed_val.to_json_value().to_string(),
                value_f64: val_f64,
                value_i64: val_i64,
                value_bool: val_bool,
                value_text: val_text,
                unit_code: prop_spec.unit_code.clone(),
                unit_symbol: prop_spec.unit_symbol.clone(),
                source_at_ms: pt.source_at_ms,
                source_time_origin: pt.source_time_origin.clone(),
                source_seq: pt.source_seq.clone(),
                source_seq_u64: seq_u64,
                quality_state: pt.quality.state.clone(),
                quality_source_code: pt.quality.source_code.clone(),
                replayed: pt.delivery.replayed,
                deadband,
                heartbeat_ms,
            });
        }

        if !issues.is_empty() {
            return Err(EngineError::new(
                "req-ingest",
                ErrorCode::ValidationFailed,
                "Whole-message atomic validation failed (T02)",
                issues,
            ));
        }
        if !clock_skew_issues.is_empty() {
            return Err(EngineError::new(
                "req-ingest",
                ErrorCode::ClockSkewExceeded,
                "Future clock skew exceeded (SC07)",
                clock_skew_issues,
            ));
        }

        let payload_hash = compute_telemetry_payload_hash(&ctx.tenant_id, msg, &item_types)?;

        Ok(ValidatedTelemetryBatch {
            tenant_id: ctx.tenant_id.clone(),
            source_id: msg.source_id.clone(),
            source_epoch: msg.source_epoch.clone(),
            message_id: msg.message_id.clone(),
            device_id: msg.device_id.clone(),
            model_revision_id: msg.model_revision_id.clone(),
            binding_revision_id: msg.binding_revision_id.clone(),
            payload_hash,
            received_at_ms: now_ms,
            observation_ids: obs_ids,
            points: wal_points,
        })
    }

    pub fn validate_source_state(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        evt: &SourceStateEnvelope,
        now_ms: i64,
        max_future_skew_ms: i64,
    ) -> Result<ValidatedSourceStateEvent, EngineError> {
        ctx.require_action("source_state:report")?;
        Self::verify_source_grant(conn, ctx, &evt.source_id, &evt.device_id, now_ms)?;

        if evt.spec != "iotfast.source_state.v1" {
            return Err(EngineError::new(
                "req-source-state",
                ErrorCode::ValidationFailed,
                format!(
                    "Expected spec 'iotfast.source_state.v1', got '{}'",
                    evt.spec
                ),
                vec![],
            ));
        }

        // I2/SC07: Clock skew check on observed_at_ms
        if evt.observed_at_ms > now_ms + max_future_skew_ms {
            return Err(EngineError::new(
                "req-source-state",
                ErrorCode::ClockSkewExceeded,
                format!(
                    "observed_at_ms {} exceeds now {} + max_future_skew_ms {}",
                    evt.observed_at_ms, now_ms, max_future_skew_ms
                ),
                vec![],
            ));
        }

        // Validate event_id and source_epoch format
        if !crate::d00::validate_message_id(&evt.event_id) {
            return Err(EngineError::new(
                "req-source-state",
                ErrorCode::ValidationFailed,
                format!("Invalid event_id '{}'", evt.event_id),
                vec![],
            ));
        }
        if !crate::d00::validate_source_epoch(&evt.source_epoch) {
            return Err(EngineError::new(
                "req-source-state",
                ErrorCode::ValidationFailed,
                format!("Invalid source_epoch '{}'", evt.source_epoch),
                vec![],
            ));
        }

        // Validate connection_state enum
        match evt.connection_state.as_str() {
            "connected" | "degraded" | "disconnected" | "unknown" => {}
            other => {
                return Err(EngineError::new(
                    "req-source-state",
                    ErrorCode::ValidationFailed,
                    format!("Invalid connection_state '{}'", other),
                    vec![],
                ));
            }
        }

        let active_bind_id: Option<String> = conn
            .query_row(
                "SELECT active_binding_revision_id FROM device WHERE device_id = ? AND tenant_id = ?",
                params![evt.device_id, ctx.tenant_id],
                |r| r.get(0),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-source-state",
                    ErrorCode::ResourceNotFound,
                    format!("Device '{}' not found", evt.device_id),
                    vec![],
                )
            })?;

        if active_bind_id.as_deref() != Some(evt.binding_revision_id.as_str()) {
            return Err(EngineError::new(
                "req-source-state",
                ErrorCode::RevisionConflict,
                "binding_revision_id does not match active device binding",
                vec![],
            ));
        }

        // Validate point_states: check for duplicate properties and unknown properties
        let mut seen_props = std::collections::BTreeSet::new();
        if !evt.point_states.is_empty() {
            // Load bound properties for this binding
            let bind_id = active_bind_id.as_deref().unwrap();
            let mut bound_stmt =
                conn.prepare("SELECT property FROM point_binding WHERE binding_revision_id = ?")?;
            let bound_rows = bound_stmt.query_map(params![bind_id], |r| r.get::<_, String>(0))?;
            let mut bound_set = std::collections::BTreeSet::new();
            for r in bound_rows {
                bound_set.insert(r?);
            }

            for ps in &evt.point_states {
                if !seen_props.insert(ps.property.clone()) {
                    return Err(EngineError::new(
                        "req-source-state",
                        ErrorCode::ValidationFailed,
                        format!("Duplicate property '{}' in point_states", ps.property),
                        vec![],
                    ));
                }
                if !bound_set.contains(&ps.property) {
                    return Err(EngineError::new(
                        "req-source-state",
                        ErrorCode::ValidationFailed,
                        format!("Property '{}' is not bound in active binding", ps.property),
                        vec![],
                    ));
                }
                // Validate quality_state enum
                match ps.quality_state.as_str() {
                    "good" | "uncertain" | "bad" | "unknown" => {}
                    other => {
                        return Err(EngineError::new(
                            "req-source-state",
                            ErrorCode::ValidationFailed,
                            format!(
                                "Invalid quality_state '{}' for property '{}'",
                                other, ps.property
                            ),
                            vec![],
                        ));
                    }
                }
            }
        }

        let mut sorted_ps = evt.point_states.clone();
        sorted_ps.sort_by(|a, b| a.property.as_bytes().cmp(b.property.as_bytes()));
        let mut hasher = Sha256::new();
        hasher.update(b"iotfast.source_state.jcs.v1\0");
        hasher.update(
            serde_json::to_vec(&(
                &ctx.tenant_id,
                &evt.source_id,
                &evt.source_epoch,
                &evt.event_id,
                &evt.device_id,
                &evt.binding_revision_id,
                evt.observed_at_ms,
                &evt.connection_state,
                &sorted_ps,
            ))
            .unwrap(),
        );
        let event_hash = format!("sha256:{}", hex::encode(hasher.finalize()));
        let event_key = format!(
            "source_state:{}:{}:{}:{}",
            ctx.tenant_id, evt.source_id, evt.source_epoch, evt.event_id
        );

        Ok(ValidatedSourceStateEvent {
            tenant_id: ctx.tenant_id.clone(),
            event_key,
            event_hash,
            envelope: evt.clone(),
        })
    }
}
