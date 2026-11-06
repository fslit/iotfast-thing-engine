use crate::error::{EngineError, ErrorCode, FieldIssue};
use crate::model_catalog::ModelCatalogService;
use crate::scope::VerifiedScopeContext;
use duckdb::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointBindingDraft {
    pub property: String,
    pub register_address: String,
    pub factor: f64,
    pub unit_code: Option<String>,
    pub deadband: Option<f64>,
    pub heartbeat_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateBindingDraftRequest {
    pub binding_revision_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub source_id: String,
    pub normalization_owner: String,
    pub server_scale_factor: Option<f64>,
    pub points: Vec<PointBindingDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayBindingAck {
    pub source_id: String,
    pub source_epoch: String,
    pub binding_revision_id: String,
    pub config_hash: String,
    pub acknowledged_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateBindingRequest {
    pub binding_revision_id: String,
    pub expected_device_revision: i64,
    pub activated_at_ms: i64,
    pub gateway_ack: GatewayBindingAck,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingRevisionRecord {
    pub binding_revision_id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub source_id: String,
    pub normalization_owner: String,
    pub server_scale_factor: Option<f64>,
    pub status: String,
    pub config_hash: String,
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
}

pub struct BindingService;

impl BindingService {
    pub fn register_source_and_grant(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        source_id: &str,
        device_id: &str,
        valid_from_ms: i64,
    ) -> Result<(), EngineError> {
        ctx.require_action("binding:write")?;
        conn.execute(
            "INSERT INTO source_gateway (tenant_id, source_id, status, connection_state)
             VALUES (?, ?, 'active', 'connected')
             ON CONFLICT (tenant_id, source_id) DO UPDATE SET status = 'active'",
            params![ctx.tenant_id, source_id],
        )?;
        conn.execute(
            "INSERT INTO source_device_grant (tenant_id, source_id, device_id, valid_from_ms, valid_to_ms)
             VALUES (?, ?, ?, ?, NULL)
             ON CONFLICT (tenant_id, source_id, device_id) DO UPDATE SET valid_from_ms = ?, valid_to_ms = NULL",
            params![ctx.tenant_id, source_id, device_id, valid_from_ms, valid_from_ms],
        )?;
        Ok(())
    }

    pub fn create_binding_draft(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: CreateBindingDraftRequest,
    ) -> Result<BindingRevisionRecord, EngineError> {
        ctx.require_action("binding:write")?;

        if req.normalization_owner == "gateway" && req.server_scale_factor.is_some() {
            return Err(EngineError::new(
                "req-binding-draft",
                ErrorCode::ValidationFailed,
                "Secondary server scaling is forbidden when normalization_owner is 'gateway'",
                vec![FieldIssue {
                    field: "server_scale_factor".to_string(),
                    issue: "DOUBLE_SCALING_FORBIDDEN".to_string(),
                    description: "server_scale_factor must be null for gateway_normalized".to_string(),
                }],
            ));
        }

        let props = ModelCatalogService::load_properties(conn, &req.model_revision_id)?;
        for pt in &req.points {
            let Some(prop_spec) = props.get(&pt.property) else {
                return Err(EngineError::new(
                    "req-binding-draft",
                    ErrorCode::ValidationFailed,
                    format!("Unknown property '{}' in binding", pt.property),
                    vec![],
                ));
            };
            if pt.unit_code != prop_spec.unit_code {
                return Err(EngineError::new(
                    "req-binding-draft",
                    ErrorCode::ValidationFailed,
                    format!(
                        "Unit mismatch for property '{}': binding={:?}, model={:?}",
                        pt.property, pt.unit_code, prop_spec.unit_code
                    ),
                    vec![],
                ));
            }
        }

        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&req.points).unwrap());
        let config_hash = format!("sha256:{}", hex::encode(hasher.finalize()));

        conn.execute_batch("BEGIN TRANSACTION")?;
        let res = (|| -> Result<(), EngineError> {
            conn.execute(
                "INSERT INTO binding_revision (binding_revision_id, tenant_id, device_id, model_revision_id, source_id, normalization_owner, server_scale_factor, status, config_hash, valid_from_ms, valid_to_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'draft', ?, NULL, NULL)",
                params![
                    req.binding_revision_id,
                    ctx.tenant_id,
                    req.device_id,
                    req.model_revision_id,
                    req.source_id,
                    req.normalization_owner,
                    req.server_scale_factor,
                    config_hash
                ],
            )?;
            for pt in &req.points {
                conn.execute(
                    "INSERT INTO point_binding (binding_revision_id, property, source_kind, register_address, factor, unit_code, deadband, heartbeat_ms)
                     VALUES (?, ?, 'gateway_measurement', ?, ?, ?, ?, ?)",
                    params![
                        req.binding_revision_id,
                        pt.property,
                        pt.register_address,
                        pt.factor,
                        pt.unit_code,
                        pt.deadband,
                        pt.heartbeat_ms
                    ],
                )?;
            }
            Ok(())
        })();

        match res {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

        Ok(BindingRevisionRecord {
            binding_revision_id: req.binding_revision_id,
            tenant_id: ctx.tenant_id.clone(),
            device_id: req.device_id,
            model_revision_id: req.model_revision_id,
            source_id: req.source_id,
            normalization_owner: req.normalization_owner,
            server_scale_factor: req.server_scale_factor,
            status: "draft".to_string(),
            config_hash,
            valid_from_ms: None,
            valid_to_ms: None,
        })
    }

    pub fn get_binding(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        binding_revision_id: &str,
    ) -> Result<BindingRevisionRecord, EngineError> {
        conn.query_row(
            "SELECT binding_revision_id, tenant_id, device_id, model_revision_id, source_id, normalization_owner, server_scale_factor, status, config_hash, valid_from_ms, valid_to_ms
             FROM binding_revision WHERE binding_revision_id = ? AND tenant_id = ?",
            params![binding_revision_id, ctx.tenant_id],
            |r| {
                Ok(BindingRevisionRecord {
                    binding_revision_id: r.get(0)?,
                    tenant_id: r.get(1)?,
                    device_id: r.get(2)?,
                    model_revision_id: r.get(3)?,
                    source_id: r.get(4)?,
                    normalization_owner: r.get(5)?,
                    server_scale_factor: r.get(6)?,
                    status: r.get(7)?,
                    config_hash: r.get(8)?,
                    valid_from_ms: r.get(9)?,
                    valid_to_ms: r.get(10)?,
                })
            },
        )
        .map_err(|_| {
            EngineError::new(
                "req-get-binding",
                ErrorCode::ResourceNotFound,
                format!("Binding revision '{}' not found", binding_revision_id),
                vec![],
            )
        })
    }

    pub fn validate_binding(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        binding_revision_id: &str,
    ) -> Result<BindingRevisionRecord, EngineError> {
        ctx.require_action("binding:write")?;
        let mut rec = Self::get_binding(conn, ctx, binding_revision_id)?;
        if rec.status != "draft" && rec.status != "ready" {
            return Err(EngineError::new(
                "req-validate-binding",
                ErrorCode::RevisionConflict,
                format!("Cannot validate binding in status '{}'", rec.status),
                vec![],
            ));
        }
        conn.execute(
            "UPDATE binding_revision SET status = 'ready' WHERE binding_revision_id = ?",
            params![binding_revision_id],
        )?;
        rec.status = "ready".to_string();
        Ok(rec)
    }

    pub fn activate_binding(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: ActivateBindingRequest,
    ) -> Result<BindingRevisionRecord, EngineError> {
        ctx.require_action("binding:write")?;
        let mut rec = Self::get_binding(conn, ctx, &req.binding_revision_id)?;
        if rec.status != "ready" {
            return Err(EngineError::new(
                "req-activate-binding",
                ErrorCode::RevisionConflict,
                format!("Binding '{}' must be in 'ready' state before activation", req.binding_revision_id),
                vec![],
            ));
        }

        if req.gateway_ack.binding_revision_id != req.binding_revision_id
            || req.gateway_ack.config_hash != rec.config_hash
            || req.gateway_ack.source_id != rec.source_id
        {
            return Err(EngineError::new(
                "req-activate-binding",
                ErrorCode::ValidationFailed,
                "gateway_ack does not match binding revision or config_hash",
                vec![],
            ));
        }

        let (current_rev, old_active_bind): (i64, Option<String>) = conn
            .query_row(
                "SELECT revision, active_binding_revision_id FROM device WHERE device_id = ? AND tenant_id = ?",
                params![rec.device_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-activate-binding",
                    ErrorCode::ResourceNotFound,
                    format!("Device '{}' not found", rec.device_id),
                    vec![],
                )
            })?;

        if current_rev != req.expected_device_revision {
            return Err(EngineError::new(
                "req-activate-binding",
                ErrorCode::RevisionConflict,
                format!(
                    "expected_device_revision={} does not match current revision={}",
                    req.expected_device_revision, current_rev
                ),
                vec![],
            ));
        }

        conn.execute_batch("BEGIN TRANSACTION")?;
        let res = (|| -> Result<(), EngineError> {
            if let Some(old_id) = old_active_bind {
                conn.execute(
                    "UPDATE binding_revision SET status = 'superseded', valid_to_ms = ? WHERE binding_revision_id = ?",
                    params![req.activated_at_ms, old_id],
                )?;
            }
            conn.execute(
                "UPDATE binding_revision SET status = 'active', valid_from_ms = ?, valid_to_ms = NULL WHERE binding_revision_id = ?",
                params![req.activated_at_ms, req.binding_revision_id],
            )?;
            conn.execute(
                "UPDATE device SET active_binding_revision_id = ?, model_revision_id = ?, revision = ? WHERE device_id = ?",
                params![
                    req.binding_revision_id,
                    rec.model_revision_id,
                    current_rev + 1,
                    rec.device_id
                ],
            )?;
            Ok(())
        })();

        match res {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

        rec.status = "active".to_string();
        rec.valid_from_ms = Some(req.activated_at_ms);
        rec.valid_to_ms = None;
        Ok(rec)
    }
}
