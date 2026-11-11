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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRevisionDetail {
    pub model_revision_id: String,
    pub tenant_id: String,
    pub product_id: String,
    pub model_version: String,
    pub status: String,
    pub model_hash: String,
    pub published_at_ms: Option<i64>,
    pub properties: Vec<ModelPropertyDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDiffReport {
    pub from_revision_id: String,
    pub to_revision_id: String,
    pub compatibility: CompatibilityKind,
    pub added_properties: Vec<String>,
    pub removed_properties: Vec<String>,
    pub breaking_properties: Vec<String>,
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

    fn require_catalog_or_model_read(ctx: &VerifiedScopeContext) -> Result<(), EngineError> {
        if ctx
            .allowed_actions
            .iter()
            .any(|a| a == "catalog:read" || a == "model:read" || a == "model:write" || a == "*")
        {
            Ok(())
        } else {
            ctx.require_action("catalog:read")
        }
    }

    pub fn list_products(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
    ) -> Result<Vec<ProductRecord>, EngineError> {
        Self::require_catalog_or_model_read(ctx)?;
        let mut stmt = conn.prepare(
            "SELECT product_id, tenant_id, product_key, name FROM product WHERE tenant_id = ? ORDER BY product_id ASC",
        )?;
        let rows = stmt.query_map(params![ctx.tenant_id], |r| {
            Ok(ProductRecord {
                product_id: r.get(0)?,
                tenant_id: r.get(1)?,
                product_key: r.get(2)?,
                name: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_models(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        product_id: Option<&str>,
    ) -> Result<Vec<ModelRevisionRecord>, EngineError> {
        Self::require_catalog_or_model_read(ctx)?;
        let mut out = Vec::new();
        if let Some(pid) = product_id {
            let mut stmt = conn.prepare(
                "SELECT model_revision_id, tenant_id, product_id, model_version, status, model_hash, published_at_ms
                 FROM model_revision WHERE tenant_id = ? AND product_id = ? ORDER BY model_revision_id ASC",
            )?;
            let rows = stmt.query_map(params![ctx.tenant_id, pid], |r| {
                Ok(ModelRevisionRecord {
                    model_revision_id: r.get(0)?,
                    tenant_id: r.get(1)?,
                    product_id: r.get(2)?,
                    model_version: r.get(3)?,
                    status: r.get(4)?,
                    model_hash: r.get(5)?,
                    published_at_ms: r.get(6)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT model_revision_id, tenant_id, product_id, model_version, status, model_hash, published_at_ms
                 FROM model_revision WHERE tenant_id = ? ORDER BY model_revision_id ASC",
            )?;
            let rows = stmt.query_map(params![ctx.tenant_id], |r| {
                Ok(ModelRevisionRecord {
                    model_revision_id: r.get(0)?,
                    tenant_id: r.get(1)?,
                    product_id: r.get(2)?,
                    model_version: r.get(3)?,
                    status: r.get(4)?,
                    model_hash: r.get(5)?,
                    published_at_ms: r.get(6)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn get_model_revision(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        model_revision_id: &str,
    ) -> Result<ModelRevisionDetail, EngineError> {
        Self::require_catalog_or_model_read(ctx)?;
        let row: Option<(String, String, String, String, String, Option<i64>)> = conn
            .query_row(
                "SELECT tenant_id, product_id, model_version, status, model_hash, published_at_ms
                 FROM model_revision WHERE model_revision_id = ? AND tenant_id = ?",
                params![model_revision_id, ctx.tenant_id],
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

        let (tenant_id, product_id, model_version, status, model_hash, published_at_ms) = row
            .ok_or_else(|| {
                EngineError::new(
                    "req-model-get",
                    ErrorCode::ResourceNotFound,
                    format!("Model revision '{}' not found", model_revision_id),
                    vec![],
                )
            })?;

        let props_map = Self::load_properties(conn, model_revision_id)?;
        let properties: Vec<ModelPropertyDraft> = props_map.into_values().collect();

        Ok(ModelRevisionDetail {
            model_revision_id: model_revision_id.to_string(),
            tenant_id,
            product_id,
            model_version,
            status,
            model_hash,
            published_at_ms,
            properties,
        })
    }

    pub fn diff_revisions(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        from_rev_id: &str,
        to_rev_id: &str,
    ) -> Result<ModelDiffReport, EngineError> {
        Self::require_catalog_or_model_read(ctx)?;
        let _ = Self::get_model_revision(conn, ctx, from_rev_id)?;
        let _ = Self::get_model_revision(conn, ctx, to_rev_id)?;

        let from_props = Self::load_properties(conn, from_rev_id)?;
        let to_props = Self::load_properties(conn, to_rev_id)?;

        let mut added_properties = Vec::new();
        let mut removed_properties = Vec::new();
        let mut breaking_properties = Vec::new();

        for (id, old_p) in &from_props {
            match to_props.get(id) {
                None => {
                    removed_properties.push(id.clone());
                    breaking_properties.push(id.clone());
                }
                Some(new_p) => {
                    if old_p.value_type != new_p.value_type || old_p.unit_code != new_p.unit_code {
                        breaking_properties.push(id.clone());
                    }
                }
            }
        }

        for (id, new_p) in &to_props {
            if !from_props.contains_key(id) {
                added_properties.push(id.clone());
                if new_p.required {
                    breaking_properties.push(id.clone());
                }
            }
        }

        let compatibility = if breaking_properties.is_empty() {
            CompatibilityKind::CompatibleExtension
        } else {
            CompatibilityKind::BreakingChange
        };

        Ok(ModelDiffReport {
            from_revision_id: from_rev_id.to_string(),
            to_revision_id: to_rev_id.to_string(),
            compatibility,
            added_properties,
            removed_properties,
            breaking_properties,
        })
    }
}
