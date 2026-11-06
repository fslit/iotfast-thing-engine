use crate::d00::ValueType;
use crate::error::{EngineError, ErrorCode, FieldIssue};
use crate::scope::VerifiedScopeContext;
use duckdb::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProductRequest {
    pub product_id: String,
    pub product_key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductRecord {
    pub product_id: String,
    pub tenant_id: String,
    pub product_key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPropertyDraft {
    pub identifier: String,
    pub name: String,
    pub value_type: ValueType,
    pub unit_code: Option<String>,
    pub unit_symbol: Option<String>,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateModelDraftRequest {
    pub model_revision_id: String,
    pub product_id: String,
    pub model_version: String,
    pub properties: Vec<ModelPropertyDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRevisionRecord {
    pub model_revision_id: String,
    pub tenant_id: String,
    pub product_id: String,
    pub model_version: String,
    pub status: String,
    pub model_hash: String,
    pub published_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityKind {
    Initial,
    CompatibleExtension,
    BreakingChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelValidationReport {
    pub model_revision_id: String,
    pub valid: bool,
    pub compatibility: CompatibilityKind,
    pub issues: Vec<FieldIssue>,
}

pub struct ModelCatalogService;

impl ModelCatalogService {
    pub fn create_product(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: CreateProductRequest,
    ) -> Result<ProductRecord, EngineError> {
        ctx.require_action("product:write")?;
        conn.execute(
            "INSERT INTO product (product_id, tenant_id, product_key, name, created_at_ms) VALUES (?, ?, ?, ?, ?)",
            params![req.product_id, ctx.tenant_id, req.product_key, req.name, ctx.issued_at_ms],
        )
        .map_err(|e| {
            EngineError::new(
                "req-product",
                ErrorCode::RevisionConflict,
                format!("Failed to create product: {}", e),
                vec![],
            )
        })?;
        Ok(ProductRecord {
            product_id: req.product_id,
            tenant_id: ctx.tenant_id.clone(),
            product_key: req.product_key,
            name: req.name,
        })
    }

    pub fn create_model_draft(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: CreateModelDraftRequest,
    ) -> Result<ModelRevisionRecord, EngineError> {
        ctx.require_action("model:write")?;

        // Check if revision already exists
        let existing_status: Option<String> = conn
            .query_row(
                "SELECT status FROM model_revision WHERE model_revision_id = ?",
                params![req.model_revision_id],
                |r| r.get(0),
            )
            .ok();

        if existing_status.is_some() {
            return Err(EngineError::new(
                "req-model-draft",
                ErrorCode::RevisionConflict,
                format!(
                    "Model revision '{}' already exists and cannot be overwritten",
                    req.model_revision_id
                ),
                vec![],
            ));
        }

        let mut seen = BTreeSet::new();
        for p in &req.properties {
            if p.identifier.is_empty() || !seen.insert(p.identifier.clone()) {
                return Err(EngineError::new(
                    "req-model-draft",
                    ErrorCode::ValidationFailed,
                    format!("Duplicate or empty property identifier '{}'", p.identifier),
                    vec![],
                ));
            }
            if let (Some(min_v), Some(max_v)) = (p.min_value, p.max_value) {
                if min_v > max_v {
                    return Err(EngineError::new(
                        "req-model-draft",
                        ErrorCode::ValidationFailed,
                        format!("Property '{}' min_value > max_value", p.identifier),
                        vec![],
                    ));
                }
            }
        }

        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&req.properties).unwrap());
        let model_hash = format!("sha256:{}", hex::encode(hasher.finalize()));

        conn.execute_batch("BEGIN TRANSACTION")?;
        let res = (|| -> Result<(), EngineError> {
            conn.execute(
                "INSERT INTO model_revision (model_revision_id, tenant_id, product_id, model_version, status, model_hash, published_at_ms)
                 VALUES (?, ?, ?, ?, 'draft', ?, NULL)",
                params![
                    req.model_revision_id,
                    ctx.tenant_id,
                    req.product_id,
                    req.model_version,
                    model_hash
                ],
            )?;
            for p in &req.properties {
                conn.execute(
                    "INSERT INTO model_item (model_revision_id, identifier, name, value_type, unit_code, unit_symbol, min_value, max_value, required)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        req.model_revision_id,
                        p.identifier,
                        p.name,
                        p.value_type.as_str(),
                        p.unit_code,
                        p.unit_symbol,
                        p.min_value,
                        p.max_value,
                        p.required
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

        Ok(ModelRevisionRecord {
            model_revision_id: req.model_revision_id,
            tenant_id: ctx.tenant_id.clone(),
            product_id: req.product_id,
            model_version: req.model_version,
            status: "draft".to_string(),
            model_hash,
            published_at_ms: None,
        })
    }

    pub fn load_properties(
        conn: &duckdb::Connection,
        model_revision_id: &str,
    ) -> Result<BTreeMap<String, ModelPropertyDraft>, EngineError> {
        let mut stmt = conn.prepare(
            "SELECT identifier, name, value_type, unit_code, unit_symbol, min_value, max_value, required
             FROM model_item WHERE model_revision_id = ? ORDER BY identifier ASC",
        )?;
        let rows = stmt.query_map(params![model_revision_id], |r| {
            let vt_str: String = r.get(2)?;
            Ok(ModelPropertyDraft {
                identifier: r.get(0)?,
                name: r.get(1)?,
                value_type: ValueType::from_str(&vt_str).unwrap_or(ValueType::Text),
                unit_code: r.get(3)?,
                unit_symbol: r.get(4)?,
                min_value: r.get(5)?,
                max_value: r.get(6)?,
                required: r.get(7)?,
            })
        })?;
        let mut map = BTreeMap::new();
        for row in rows {
            let item = row?;
            map.insert(item.identifier.clone(), item);
        }
        Ok(map)
    }

    pub fn validate_model(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        model_revision_id: &str,
        base_revision_id: Option<&str>,
    ) -> Result<ModelValidationReport, EngineError> {
        ctx.require_action("model:write")?;
        // I5: Verify model_revision belongs to caller's tenant
        let _exists: String = conn.query_row(
            "SELECT model_revision_id FROM model_revision WHERE model_revision_id = ? AND tenant_id = ?",
            params![model_revision_id, ctx.tenant_id],
            |r| r.get(0),
        ).map_err(|_| {
            EngineError::new(
                "req-model-validate",
                ErrorCode::ResourceNotFound,
                format!("Model revision '{}' not found", model_revision_id),
                vec![],
            )
        })?;
        let props = Self::load_properties(conn, model_revision_id)?;
        if props.is_empty() {
            return Err(EngineError::new(
                "req-model-validate",
                ErrorCode::ValidationFailed,
                "Model revision has no properties",
                vec![],
            ));
        }
        let compatibility = match base_revision_id {
            None => CompatibilityKind::Initial,
            Some(base_id) => Self::compare_revisions(conn, ctx, base_id, model_revision_id)?,
        };
        Ok(ModelValidationReport {
            model_revision_id: model_revision_id.to_string(),
            valid: true,
            compatibility,
            issues: vec![],
        })
    }

    pub fn publish_model(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        model_revision_id: &str,
        published_at_ms: i64,
    ) -> Result<ModelRevisionRecord, EngineError> {
        ctx.require_action("model:write")?;

        let row: Option<(String, String, String, String, String)> = conn
            .query_row(
                "SELECT tenant_id, product_id, model_version, status, model_hash
                 FROM model_revision WHERE model_revision_id = ? AND tenant_id = ?",
                params![model_revision_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();

        let (tenant_id, product_id, model_version, status, model_hash) = row.ok_or_else(|| {
            EngineError::new(
                "req-model-publish",
                ErrorCode::ResourceNotFound,
                format!("Model revision '{}' not found", model_revision_id),
                vec![],
            )
        })?;

        if status == "published" {
            return Err(EngineError::new(
                "req-model-publish",
                ErrorCode::RevisionConflict,
                format!(
                    "Model revision '{}' is already published and immutable",
                    model_revision_id
                ),
                vec![],
            ));
        }

        conn.execute_batch("BEGIN TRANSACTION")?;
        let res = (|| -> Result<(), EngineError> {
            conn.execute(
                "UPDATE model_revision SET status = 'published', published_at_ms = ? WHERE model_revision_id = ?",
                params![published_at_ms, model_revision_id],
            )?;
            let audit_id = format!("aud-pub-{}", model_revision_id);
            conn.execute(
                "INSERT INTO audit_log (audit_id, tenant_id, caller_service_id, operator_id, action, entity_id, occurred_at_ms)
                 VALUES (?, ?, ?, ?, 'model:publish', ?, ?)",
                params![
                    audit_id,
                    tenant_id,
                    ctx.caller_service_id,
                    ctx.operator_id,
                    model_revision_id,
                    published_at_ms
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

        Ok(ModelRevisionRecord {
            model_revision_id: model_revision_id.to_string(),
            tenant_id,
            product_id,
            model_version,
            status: "published".to_string(),
            model_hash,
            published_at_ms: Some(published_at_ms),
        })
    }

    pub fn compare_revisions(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        from_rev_id: &str,
        to_rev_id: &str,
    ) -> Result<CompatibilityKind, EngineError> {
        ctx.require_action("model:write")?;
        let from_props = Self::load_properties(conn, from_rev_id)?;
        let to_props = Self::load_properties(conn, to_rev_id)?;

        // Check if any existing property was removed or changed type/unit
        for (id, old_p) in &from_props {
            let Some(new_p) = to_props.get(id) else {
                return Ok(CompatibilityKind::BreakingChange);
            };
            if old_p.value_type != new_p.value_type || old_p.unit_code != new_p.unit_code {
                return Ok(CompatibilityKind::BreakingChange);
            }
        }
        // Check if any newly added property is marked required
        for (id, new_p) in &to_props {
            if !from_props.contains_key(id) && new_p.required {
                return Ok(CompatibilityKind::BreakingChange);
            }
        }
        Ok(CompatibilityKind::CompatibleExtension)
    }
}
