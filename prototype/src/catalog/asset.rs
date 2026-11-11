use crate::error::{EngineError, ErrorCode, FieldIssue};
use crate::model_catalog::ModelCatalogService;
use crate::scope::VerifiedScopeContext;
use duckdb::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAssetRequest {
    pub asset_id: String,
    pub parent_asset_id: Option<String>,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub asset_id: String,
    pub tenant_id: String,
    pub parent_asset_id: Option<String>,
    pub slug: String,
    pub name: String,
    pub asset_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstantiateDeviceRequest {
    pub device_id: String,
    pub device_key: String,
    pub device_name: String,
    pub product_id: String,
    pub model_revision_id: String,
    pub asset_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub resource_id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub property: String,
    pub name: String,
    pub canonical_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceWithResources {
    pub device_id: String,
    pub tenant_id: String,
    pub device_key: String,
    pub device_name: String,
    pub product_id: String,
    pub model_revision_id: String,
    pub asset_id: String,
    pub canonical_device_uri: String,
    pub revision: i64,
    pub resources: Vec<ResourceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePathRecord {
    pub resource_id: String,
    pub canonical_uri: String,
    pub previous_uri: String,
    pub redirect_until_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedResourceUri {
    pub resource_id: String,
    pub device_id: String,
    pub property: String,
    pub canonical_uri: String,
    pub matched_path_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelocateDeviceResult {
    pub records: Vec<ResourcePathRecord>,
    pub redirect_until_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeDeviceModelRequest {
    pub device_id: String,
    pub target_model_revision_id: String,
    pub expected_device_revision: i64,
    pub upgraded_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAssetRelationRequest {
    pub relation_id: String,
    pub from_asset_id: String,
    pub relation_type: String,
    pub to_asset_id: String,
    pub valid_from_ms: i64,
    pub valid_to_ms: Option<i64>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRelationRecord {
    pub relation_id: String,
    pub tenant_id: String,
    pub from_asset_id: String,
    pub relation_type: String,
    pub to_asset_id: String,
    pub valid_from_ms: i64,
    pub valid_to_ms: Option<i64>,
    pub source: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResourcesFilter {
    pub asset_id: Option<String>,
    pub include_descendants: bool,
    pub device_id: Option<String>,
    pub uri_prefix: Option<String>,
}

pub struct AssetUnsService;

impl AssetUnsService {
    pub fn allocate_resource_id(
        conn: &duckdb::Connection,
        tenant_id: &str,
        device_id: &str,
        prop_id: &str,
    ) -> String {
        let candidate = format!("res-{}-{}", device_id, prop_id.replace('_', "-"));
        let occupied: Option<(String, String)> = conn
            .query_row(
                "SELECT device_id, property FROM resource WHERE resource_id = ?",
                params![candidate],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match occupied {
            None => candidate,
            Some((occ_dev, occ_prop)) if occ_dev == device_id && occ_prop == prop_id => candidate,
            Some(_) => crate::d00::derive_resource_id(tenant_id, device_id, "property", prop_id),
        }
    }

    pub fn lookup_resource_id(
        conn: &duckdb::Connection,
        device_id: &str,
        prop_id: &str,
    ) -> String {
        conn.query_row(
            "SELECT resource_id FROM resource WHERE device_id = ? AND property = ?",
            params![device_id, prop_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| format!("res-{}-{}", device_id, prop_id.replace('_', "-")))
    }
    pub fn load_asset_chain(
        conn: &duckdb::Connection,
        asset_id: &str,
    ) -> Result<Vec<String>, EngineError> {
        let mut chain = Vec::new();
        let mut cursor = Some(asset_id.to_string());
        while let Some(curr) = cursor {
            if chain.contains(&curr) {
                return Err(EngineError::new(
                    "req-asset-chain",
                    ErrorCode::ValidationFailed,
                    "Asset hierarchy cycle detected",
                    vec![],
                ));
            }
            chain.push(curr.clone());
            cursor = conn
                .query_row(
                    "SELECT parent_asset_id FROM asset WHERE asset_id = ?",
                    params![curr],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
        }
        chain.reverse();
        Ok(chain)
    }

    pub fn create_asset(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: CreateAssetRequest,
    ) -> Result<AssetRecord, EngineError> {
        ctx.require_action("asset:write")?;
        if req.slug.is_empty() || req.slug.contains('/') {
            return Err(EngineError::new(
                "req-asset",
                ErrorCode::ValidationFailed,
                "Invalid asset slug",
                vec![],
            ));
        }
        let asset_path = match &req.parent_asset_id {
            None => req.slug.clone(),
            Some(pid) => {
                if pid == &req.asset_id {
                    return Err(EngineError::new(
                        "req-asset",
                        ErrorCode::ValidationFailed,
                        "Asset cannot be its own parent",
                        vec![],
                    ));
                }
                let parent_path: String = conn
                    .query_row(
                        "SELECT asset_path FROM asset WHERE asset_id = ? AND tenant_id = ?",
                        params![pid, ctx.tenant_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| {
                        EngineError::new(
                            "req-asset",
                            ErrorCode::ResourceNotFound,
                            format!("Parent asset '{}' not found", pid),
                            vec![],
                        )
                    })?;
                format!("{}/{}", parent_path, req.slug)
            }
        };

        conn.execute(
            "INSERT INTO asset (asset_id, tenant_id, parent_asset_id, slug, name, asset_path)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                req.asset_id,
                ctx.tenant_id,
                req.parent_asset_id,
                req.slug,
                req.name,
                asset_path
            ],
        )
        .map_err(|e| {
            EngineError::new(
                "req-asset",
                ErrorCode::RevisionConflict,
                format!(
                    "Duplicate asset_id or sibling slug path '{}': {}",
                    asset_path, e
                ),
                vec![],
            )
        })?;

        Ok(AssetRecord {
            asset_id: req.asset_id,
            tenant_id: ctx.tenant_id.clone(),
            parent_asset_id: req.parent_asset_id,
            slug: req.slug,
            name: req.name,
            asset_path,
        })
    }

    pub fn instantiate_device(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: InstantiateDeviceRequest,
    ) -> Result<DeviceWithResources, EngineError> {
        ctx.require_action("device:write")?;

        let status: String = conn
            .query_row(
                "SELECT status FROM model_revision WHERE model_revision_id = ? AND tenant_id = ? AND product_id = ?",
                params![req.model_revision_id, ctx.tenant_id, req.product_id],
                |r| r.get(0),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-device",
                    ErrorCode::ResourceNotFound,
                    format!("Published model '{}' not found", req.model_revision_id),
                    vec![],
                )
            })?;
        if status != "published" {
            return Err(EngineError::new(
                "req-device",
                ErrorCode::ValidationFailed,
                "Device can only be instantiated from a published model revision",
                vec![],
            ));
        }

        let asset_path: String = conn
            .query_row(
                "SELECT asset_path FROM asset WHERE asset_id = ? AND tenant_id = ?",
                params![req.asset_id, ctx.tenant_id],
                |r| r.get(0),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-device",
                    ErrorCode::ResourceNotFound,
                    format!("Asset '{}' not found", req.asset_id),
                    vec![],
                )
            })?;

        let props = ModelCatalogService::load_properties(conn, &req.model_revision_id)?;
        let canonical_device_uri =
            format!("uns://{}/{}/{}", ctx.tenant_id, asset_path, req.device_key);

        // NB02 / Q06: Permanently forbid another resource_id from occupying any URI in resource_path
        for prop_id in props.keys() {
            let prop_uri = format!("{}/properties/{}", canonical_device_uri, prop_id);
            let target_res_id = Self::allocate_resource_id(conn, &ctx.tenant_id, &req.device_id, prop_id);
            let existing: Option<(String, String, Option<i64>)> = conn
                .query_row(
                    "SELECT path_state, resource_id, redirect_until_ms FROM resource_path WHERE uri = ?",
                    params![prop_uri],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok();
            if let Some((_path_state, existing_res_id, _redirect_until)) = existing {
                if existing_res_id != target_res_id {
                    return Err(EngineError::new(
                        "req-device",
                        ErrorCode::RevisionConflict,
                        format!(
                            "URI '{}' was already allocated to resource '{}' and cannot be claimed by another resource (Q06)",
                            prop_uri, existing_res_id
                        ),
                        vec![],
                    ));
                }
            }
        }

        conn.execute_batch("BEGIN TRANSACTION")?;
        let mut resources = Vec::with_capacity(props.len());
        let res = (|| -> Result<(), EngineError> {
            conn.execute(
                "INSERT INTO device (device_id, tenant_id, device_key, device_name, product_id, model_revision_id, active_binding_revision_id, asset_id, canonical_device_uri, revision)
                 VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, 1)",
                params![
                    req.device_id,
                    ctx.tenant_id,
                    req.device_key,
                    req.device_name,
                    req.product_id,
                    req.model_revision_id,
                    req.asset_id,
                    canonical_device_uri
                ],
            )
            .map_err(|e| {
                EngineError::new(
                    "req-device",
                    ErrorCode::RevisionConflict,
                    format!("Device already exists or conflicts: {}", e),
                    vec![],
                )
            })?;

            for (prop_id, prop_spec) in &props {
                let resource_id =
                    Self::allocate_resource_id(conn, &ctx.tenant_id, &req.device_id, prop_id);
                let canonical_uri = format!("{}/properties/{}", canonical_device_uri, prop_id);

                conn.execute(
                    "INSERT INTO resource (resource_id, tenant_id, device_id, property, name, canonical_uri)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    params![
                        resource_id,
                        ctx.tenant_id,
                        req.device_id,
                        prop_id,
                        prop_spec.name,
                        canonical_uri
                    ],
                )?;
                conn.execute(
                    "INSERT INTO resource_path (uri, tenant_id, resource_id, path_state, redirect_to_uri, redirect_until_ms)
                     VALUES (?, ?, ?, 'canonical', NULL, NULL)",
                    params![canonical_uri, ctx.tenant_id, resource_id],
                )?;

                resources.push(ResourceRecord {
                    resource_id,
                    tenant_id: ctx.tenant_id.clone(),
                    device_id: req.device_id.clone(),
                    property: prop_id.clone(),
                    name: prop_spec.name.clone(),
                    canonical_uri,
                });
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

        Ok(DeviceWithResources {
            device_id: req.device_id,
            tenant_id: ctx.tenant_id.clone(),
            device_key: req.device_key,
            device_name: req.device_name,
            product_id: req.product_id,
            model_revision_id: req.model_revision_id,
            asset_id: req.asset_id,
            canonical_device_uri,
            revision: 1,
            resources,
        })
    }

    pub fn relocate_device(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        device_id: &str,
        new_asset_id: &str,
        now_ms: i64,
        redirect_retention_ms: i64,
    ) -> Result<RelocateDeviceResult, EngineError> {
        ctx.require_action("device:write")?;

        let (tenant_id, device_key, rev): (String, String, i64) = conn
            .query_row(
                "SELECT tenant_id, device_key, revision FROM device WHERE device_id = ? AND tenant_id = ?",
                params![device_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-relocate",
                    ErrorCode::ResourceNotFound,
                    format!("Device '{}' not found", device_id),
                    vec![],
                )
            })?;

        let new_asset_path: String = conn
            .query_row(
                "SELECT asset_path FROM asset WHERE asset_id = ? AND tenant_id = ?",
                params![new_asset_id, tenant_id],
                |r| r.get(0),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-relocate",
                    ErrorCode::ResourceNotFound,
                    format!("Target asset '{}' not found", new_asset_id),
                    vec![],
                )
            })?;

        let new_device_uri = format!("uns://{}/{}/{}", tenant_id, new_asset_path, device_key);
        let redirect_until_ms = now_ms + redirect_retention_ms;

        let mut stmt = conn.prepare(
            "SELECT resource_id, property, canonical_uri FROM resource WHERE device_id = ? ORDER BY property ASC",
        )?;
        let rows = stmt.query_map(params![device_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut current_resources = Vec::new();
        for r in rows {
            current_resources.push(r?);
        }

        conn.execute_batch("BEGIN TRANSACTION")?;
        let mut records = Vec::with_capacity(current_resources.len());
        let res = (|| -> Result<(), EngineError> {
            conn.execute(
                "UPDATE device SET asset_id = ?, canonical_device_uri = ?, revision = ? WHERE device_id = ?",
                params![new_asset_id, new_device_uri, rev + 1, device_id],
            )?;

            for (res_id, prop, old_uri) in current_resources {
                let new_uri = format!("{}/properties/{}", new_device_uri, prop);
                conn.execute(
                    "UPDATE resource SET canonical_uri = ? WHERE resource_id = ?",
                    params![new_uri, res_id],
                )?;
                conn.execute(
                    "UPDATE resource_path SET path_state = 'redirected', redirect_to_uri = ?, redirect_until_ms = ? WHERE uri = ?",
                    params![new_uri, redirect_until_ms, old_uri],
                )?;
                // NB02 + Q06: Never allow another resource_id to hijack a canonical or historical redirect path!
                let conflict: Option<(String, String, Option<i64>)> = conn
                    .query_row(
                        "SELECT resource_id, path_state, redirect_until_ms FROM resource_path WHERE uri = ?",
                        params![new_uri],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();
                if let Some((conflict_res_id, _conflict_state, _conflict_until)) = conflict {
                    if conflict_res_id != res_id {
                        return Err(EngineError::new(
                            "req-relocate",
                            ErrorCode::RevisionConflict,
                            format!(
                                "Target URI '{}' is occupied or historically bound to resource '{}' (NB02/Q06)",
                                new_uri, conflict_res_id
                            ),
                            vec![],
                        ));
                    }
                }
                conn.execute(
                    "INSERT INTO resource_path (uri, tenant_id, resource_id, path_state, redirect_to_uri, redirect_until_ms)
                     VALUES (?, ?, ?, 'canonical', NULL, NULL)
                     ON CONFLICT (uri) DO UPDATE SET
                        resource_id = excluded.resource_id,
                        path_state = 'canonical',
                        redirect_to_uri = NULL,
                        redirect_until_ms = NULL",
                    params![new_uri, tenant_id, res_id],
                )?;
                records.push(ResourcePathRecord {
                    resource_id: res_id,
                    canonical_uri: new_uri,
                    previous_uri: old_uri,
                    redirect_until_ms,
                });
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

        Ok(RelocateDeviceResult {
            records,
            redirect_until_ms,
        })
    }

    /// Upgrade an existing device instance to a newer compatible published model revision
    /// without losing historical `resource_id`s or telemetry continuity (`BM01` / `AT04` / `Q11`).
    pub fn upgrade_device_model(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: UpgradeDeviceModelRequest,
    ) -> Result<DeviceWithResources, EngineError> {
        ctx.require_action("device:write")?;

        let (tenant_id, device_key, device_name, product_id, current_model_rev, asset_id, canonical_device_uri, current_rev): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT tenant_id, device_key, device_name, product_id, model_revision_id, asset_id, canonical_device_uri, revision
                 FROM device WHERE device_id = ? AND tenant_id = ?",
                params![req.device_id, ctx.tenant_id],
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
            .map_err(|_| {
                EngineError::new(
                    "req-upgrade-device-model",
                    ErrorCode::ResourceNotFound,
                    format!("Device '{}' not found", req.device_id),
                    vec![],
                )
            })?;

        if current_rev != req.expected_device_revision {
            return Err(EngineError::new(
                "req-upgrade-device-model",
                ErrorCode::RevisionConflict,
                format!(
                    "Device revision mismatch: expected {}, got {}",
                    req.expected_device_revision, current_rev
                ),
                vec![],
            ));
        }

        let (target_prod_id, target_status): (String, String) = conn
            .query_row(
                "SELECT product_id, status FROM model_revision WHERE model_revision_id = ? AND tenant_id = ?",
                params![req.target_model_revision_id, tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| {
                EngineError::new(
                    "req-upgrade-device-model",
                    ErrorCode::ResourceNotFound,
                    format!("Target model revision '{}' not found", req.target_model_revision_id),
                    vec![],
                )
            })?;

        if target_prod_id != product_id {
            return Err(EngineError::new(
                "req-upgrade-device-model",
                ErrorCode::CrossVersionIncompatible,
                format!(
                    "Target model revision '{}' belongs to product '{}', not '{}'",
                    req.target_model_revision_id, target_prod_id, product_id
                ),
                vec![],
            ));
        }
        if target_status != "published" {
            return Err(EngineError::new(
                "req-upgrade-device-model",
                ErrorCode::ValidationFailed,
                format!(
                    "Target model revision '{}' must be published before upgrading device",
                    req.target_model_revision_id
                ),
                vec![],
            ));
        }

        let diff = crate::catalog::model::ModelCatalogService::diff_revisions(
            conn,
            ctx,
            &current_model_rev,
            &req.target_model_revision_id,
        )?;
        if diff.compatibility == crate::catalog::model::CompatibilityKind::BreakingChange {
            return Err(EngineError::new(
                "req-upgrade-device-model",
                ErrorCode::CrossVersionIncompatible,
                format!(
                    "Cannot smooth-upgrade device '{}' from '{}' to '{}' due to breaking changes: {:?}",
                    req.device_id, current_model_rev, req.target_model_revision_id, diff.breaking_properties
                ),
                vec![],
            ));
        }

        let target_props =
            ModelCatalogService::load_properties(conn, &req.target_model_revision_id)?;

        conn.execute_batch("BEGIN TRANSACTION")?;
        let new_rev = current_rev + 1;
        let res = (|| -> Result<(), EngineError> {
            for prop_id in &diff.added_properties {
                let prop_name = target_props
                    .get(prop_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| prop_id.clone());
                let resource_id =
                    Self::allocate_resource_id(conn, &tenant_id, &req.device_id, prop_id);
                let canonical_uri = format!("{}/properties/{}", canonical_device_uri, prop_id);
                let existing_path: Option<String> = conn
                    .query_row(
                        "SELECT resource_id FROM resource_path WHERE uri = ?",
                        params![canonical_uri],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(existing_res) = existing_path {
                    if existing_res != resource_id {
                        return Err(EngineError::new(
                            "req-upgrade-device-model",
                            ErrorCode::RevisionConflict,
                            format!(
                                "URI '{}' is already bound to resource '{}'",
                                canonical_uri, existing_res
                            ),
                            vec![],
                        ));
                    }
                }
                conn.execute(
                    "INSERT INTO resource (resource_id, tenant_id, device_id, property, name, canonical_uri)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    params![
                        resource_id,
                        tenant_id,
                        req.device_id,
                        prop_id,
                        prop_name,
                        canonical_uri
                    ],
                )?;
                conn.execute(
                    "INSERT INTO resource_path (uri, tenant_id, resource_id, path_state, redirect_to_uri, redirect_until_ms)
                     VALUES (?, ?, ?, 'canonical', NULL, NULL)",
                    params![canonical_uri, tenant_id, resource_id],
                )?;
            }

            conn.execute(
                "UPDATE device SET model_revision_id = ?, revision = ? WHERE device_id = ?",
                params![req.target_model_revision_id, new_rev, req.device_id],
            )?;
            conn.execute(
                "INSERT INTO audit_log (audit_id, tenant_id, caller_service_id, operator_id, action, entity_id, occurred_at_ms)
                 VALUES (?, ?, ?, ?, 'device:upgrade_model', ?, ?)",
                params![
                    format!("aud-upgrade-{}-{}", req.device_id, new_rev),
                    tenant_id,
                    ctx.caller_service_id,
                    ctx.operator_id,
                    req.device_id,
                    req.upgraded_at_ms
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

        let mut stmt = conn.prepare(
            "SELECT resource_id, tenant_id, device_id, property, name, canonical_uri FROM resource WHERE device_id = ? ORDER BY property ASC",
        )?;
        let rows = stmt.query_map(params![req.device_id], |r| {
            Ok(ResourceRecord {
                resource_id: r.get(0)?,
                tenant_id: r.get(1)?,
                device_id: r.get(2)?,
                property: r.get(3)?,
                name: r.get(4)?,
                canonical_uri: r.get(5)?,
            })
        })?;
        let mut resources = Vec::new();
        for r in rows {
            resources.push(r?);
        }

        Ok(DeviceWithResources {
            device_id: req.device_id,
            tenant_id,
            device_key,
            device_name,
            product_id,
            model_revision_id: req.target_model_revision_id,
            asset_id,
            canonical_device_uri,
            revision: new_rev,
            resources,
        })
    }

    pub fn create_asset_relation(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        req: CreateAssetRelationRequest,
    ) -> Result<AssetRelationRecord, EngineError> {
        ctx.require_action("asset:write")?;
        const ALLOWED_RELATIONS: &[&str] = &["FEEDS", "MEASURED_BY", "UPSTREAM_OF", "PART_OF"];
        if !ALLOWED_RELATIONS.contains(&req.relation_type.as_str()) {
            return Err(EngineError::new(
                "req-create-relation",
                ErrorCode::ValidationFailed,
                format!("Unsupported relation_type '{}'", req.relation_type),
                vec![FieldIssue {
                    field: "relation_type".into(),
                    issue: "UNSUPPORTED_RELATION_TYPE".into(),
                    description: "must be one of FEEDS, MEASURED_BY, UPSTREAM_OF, PART_OF".into(),
                }],
            ));
        }
        if req.from_asset_id == req.to_asset_id {
            return Err(EngineError::new(
                "req-create-relation",
                ErrorCode::ValidationFailed,
                "Self-referencing asset relation is forbidden",
                vec![FieldIssue {
                    field: "to_asset_id".into(),
                    issue: "SELF_REFERENCE_FORBIDDEN".into(),
                    description: "from_asset_id and to_asset_id must be distinct".into(),
                }],
            ));
        }

        for (field, aid) in [
            ("from_asset_id", &req.from_asset_id),
            ("to_asset_id", &req.to_asset_id),
        ] {
            let exists: Option<String> = conn
                .query_row(
                    "SELECT asset_id FROM asset WHERE asset_id = ? AND tenant_id = ?",
                    params![aid, ctx.tenant_id],
                    |r| r.get(0),
                )
                .ok();
            if exists.is_none() {
                return Err(EngineError::new(
                    "req-create-relation",
                    ErrorCode::ResourceNotFound,
                    format!("Asset '{}' not found ({})", aid, field),
                    vec![],
                ));
            }
        }

        let dup: Option<String> = conn
            .query_row(
                "SELECT relation_id FROM asset_relation
                 WHERE relation_id = ? OR (tenant_id = ? AND from_asset_id = ? AND relation_type = ? AND to_asset_id = ? AND valid_to_ms IS NULL)",
                params![
                    req.relation_id,
                    ctx.tenant_id,
                    req.from_asset_id,
                    req.relation_type,
                    req.to_asset_id
                ],
                |r| r.get(0),
            )
            .ok();
        if dup.is_some() {
            return Err(EngineError::new(
                "req-create-relation",
                ErrorCode::RevisionConflict,
                "Asset relation already exists",
                vec![],
            ));
        }

        conn.execute(
            "INSERT INTO asset_relation (relation_id, tenant_id, from_asset_id, relation_type, to_asset_id, valid_from_ms, valid_to_ms, source, revision)
             VALUES (?, ?, ?, ?, ?, ?, NULL, ?, 1)",
            params![
                req.relation_id,
                ctx.tenant_id,
                req.from_asset_id,
                req.relation_type,
                req.to_asset_id,
                req.valid_from_ms,
                req.source
            ],
        )?;

        Ok(AssetRelationRecord {
            relation_id: req.relation_id,
            tenant_id: ctx.tenant_id.clone(),
            from_asset_id: req.from_asset_id,
            relation_type: req.relation_type,
            to_asset_id: req.to_asset_id,
            valid_from_ms: req.valid_from_ms,
            valid_to_ms: None,
            source: req.source,
            revision: 1,
        })
    }

    pub fn list_asset_relations(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        asset_id: &str,
        now_ms: i64,
    ) -> Result<Vec<AssetRelationRecord>, EngineError> {
        if !ctx.has_action("catalog:read") && !ctx.has_action("asset:write") {
            ctx.require_action("catalog:read")?;
        }
        let exists: Option<String> = conn
            .query_row(
                "SELECT asset_id FROM asset WHERE asset_id = ? AND tenant_id = ?",
                params![asset_id, ctx.tenant_id],
                |r| r.get(0),
            )
            .ok();
        if exists.is_none() {
            return Err(EngineError::new(
                "req-list-relations",
                ErrorCode::ResourceNotFound,
                format!("Asset '{}' not found", asset_id),
                vec![],
            ));
        }

        let mut stmt = conn.prepare(
            "SELECT relation_id, tenant_id, from_asset_id, relation_type, to_asset_id, valid_from_ms, valid_to_ms, source, revision
             FROM asset_relation
             WHERE tenant_id = ?
               AND (from_asset_id = ? OR to_asset_id = ?)
               AND valid_from_ms <= ?
               AND (valid_to_ms IS NULL OR valid_to_ms > ?)
             ORDER BY relation_id ASC",
        )?;
        let rows = stmt.query_map(
            params![ctx.tenant_id, asset_id, asset_id, now_ms, now_ms],
            |r| {
                Ok(AssetRelationRecord {
                    relation_id: r.get(0)?,
                    tenant_id: r.get(1)?,
                    from_asset_id: r.get(2)?,
                    relation_type: r.get(3)?,
                    to_asset_id: r.get(4)?,
                    valid_from_ms: r.get(5)?,
                    valid_to_ms: r.get(6)?,
                    source: r.get(7)?,
                    revision: r.get(8)?,
                })
            },
        )?;

        let mut out = Vec::new();
        for r in rows {
            let rec = r?;
            let from_chain = Self::load_asset_chain(conn, &rec.from_asset_id)?;
            let to_chain = Self::load_asset_chain(conn, &rec.to_asset_id)?;
            let from_allowed = ctx.scope_filter.tenant_wide
                || (!ctx.scope_filter.asset_ids.is_empty()
                    && from_chain
                        .iter()
                        .any(|a| ctx.scope_filter.asset_ids.contains(a)));
            let to_allowed = ctx.scope_filter.tenant_wide
                || (!ctx.scope_filter.asset_ids.is_empty()
                    && to_chain
                        .iter()
                        .any(|a| ctx.scope_filter.asset_ids.contains(a)));
            if from_allowed && to_allowed {
                out.push(rec);
            }
        }
        Ok(out)
    }

    pub fn list_assets(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
    ) -> Result<Vec<AssetRecord>, EngineError> {
        if !ctx.has_action("catalog:read") && !ctx.has_action("asset:write") {
            ctx.require_action("catalog:read")?;
        }
        let mut stmt = conn.prepare(
            "SELECT asset_id, tenant_id, parent_asset_id, slug, name, asset_path
             FROM asset WHERE tenant_id = ? ORDER BY asset_path ASC",
        )?;
        let rows = stmt.query_map(params![ctx.tenant_id], |r| {
            Ok(AssetRecord {
                asset_id: r.get(0)?,
                tenant_id: r.get(1)?,
                parent_asset_id: r.get(2)?,
                slug: r.get(3)?,
                name: r.get(4)?,
                asset_path: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            let rec = r?;
            let chain = Self::load_asset_chain(conn, &rec.asset_id)?;
            let allowed = ctx.scope_filter.tenant_wide
                || (!ctx.scope_filter.asset_ids.is_empty()
                    && chain.iter().any(|a| ctx.scope_filter.asset_ids.contains(a)));
            if allowed {
                out.push(rec);
            }
        }
        Ok(out)
    }

    pub fn get_device(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        device_id: &str,
    ) -> Result<DeviceWithResources, EngineError> {
        if !ctx.has_action("catalog:read")
            && !ctx.has_action("device:write")
            && !ctx.has_action("device:read")
        {
            ctx.require_action("catalog:read")?;
        }

        let (tenant_id, device_key, device_name, product_id, model_revision_id, asset_id, canonical_device_uri, revision): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT tenant_id, device_key, device_name, product_id, model_revision_id, asset_id, canonical_device_uri, revision
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
            .map_err(|_| {
                EngineError::new(
                    "req-get-device",
                    ErrorCode::ResourceNotFound,
                    format!("Device '{}' not found", device_id),
                    vec![],
                )
            })?;

        let asset_chain = Self::load_asset_chain(conn, &asset_id)?;
        let mut stmt = conn.prepare(
            "SELECT resource_id, tenant_id, device_id, property, name, canonical_uri FROM resource WHERE device_id = ? ORDER BY property ASC",
        )?;
        let rows = stmt.query_map(params![device_id], |r| {
            Ok(ResourceRecord {
                resource_id: r.get(0)?,
                tenant_id: r.get(1)?,
                device_id: r.get(2)?,
                property: r.get(3)?,
                name: r.get(4)?,
                canonical_uri: r.get(5)?,
            })
        })?;
        let mut resources = Vec::new();
        for r in rows {
            let rec = r?;
            if ctx.allows_resource(
                &tenant_id,
                &asset_chain,
                device_id,
                &rec.resource_id,
                &rec.canonical_uri,
            ) {
                resources.push(rec);
            }
        }
        if resources.is_empty() {
            return Err(EngineError::new(
                "req-get-device",
                ErrorCode::ResourceNotFound,
                format!("Device '{}' not found", device_id),
                vec![],
            ));
        }

        Ok(DeviceWithResources {
            device_id: device_id.to_string(),
            tenant_id,
            device_key,
            device_name,
            product_id,
            model_revision_id,
            asset_id,
            canonical_device_uri,
            revision,
            resources,
        })
    }

    pub fn list_devices(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        asset_id: Option<&str>,
    ) -> Result<Vec<DeviceWithResources>, EngineError> {
        if !ctx.has_action("catalog:read")
            && !ctx.has_action("device:write")
            && !ctx.has_action("device:read")
        {
            ctx.require_action("catalog:read")?;
        }
        let mut stmt = conn.prepare(
            "SELECT device_id FROM device WHERE tenant_id = ? ORDER BY device_id ASC",
        )?;
        let rows = stmt.query_map(params![ctx.tenant_id], |r| r.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        let mut out = Vec::new();
        for id in ids {
            if let Ok(dev) = Self::get_device(conn, ctx, &id) {
                if let Some(target_aid) = asset_id {
                    let chain = Self::load_asset_chain(conn, &dev.asset_id)?;
                    if !chain.iter().any(|a| a == target_aid) {
                        continue;
                    }
                }
                out.push(dev);
            }
        }
        Ok(out)
    }

    pub fn list_resources(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        filter: ListResourcesFilter,
    ) -> Result<Vec<ResourceRecord>, EngineError> {
        ctx.require_action("catalog:read")?;
        let mut stmt = conn.prepare(
            "SELECT r.resource_id, r.tenant_id, r.device_id, r.property, r.name, r.canonical_uri, d.asset_id
             FROM resource r
             JOIN device d ON r.device_id = d.device_id
             WHERE r.tenant_id = ?
             ORDER BY r.resource_id ASC",
        )?;
        let rows = stmt.query_map(params![ctx.tenant_id], |r| {
            let tenant_id: String = r.get(1)?;
            Ok((
                ResourceRecord {
                    resource_id: r.get(0)?,
                    tenant_id: tenant_id.clone(),
                    device_id: r.get(2)?,
                    property: r.get(3)?,
                    name: r.get(4)?,
                    canonical_uri: r.get(5)?,
                },
                r.get::<_, String>(6)?,
                tenant_id,
            ))
        })?;

        let mut out = Vec::new();
        for r in rows {
            let (rec, dev_asset_id, tenant_id) = r?;
            let asset_chain = Self::load_asset_chain(conn, &dev_asset_id)?;
            // NB06: Scope intersection BEFORE filtering & pagination!
            if !ctx.allows_resource(
                &tenant_id,
                &asset_chain,
                &rec.device_id,
                &rec.resource_id,
                &rec.canonical_uri,
            ) {
                continue;
            }
            if let Some(ref did) = filter.device_id {
                if &rec.device_id != did {
                    continue;
                }
            }
            if let Some(ref prefix) = filter.uri_prefix {
                if !rec.canonical_uri.starts_with(prefix) {
                    continue;
                }
            }
            if let Some(ref aid) = filter.asset_id {
                if filter.include_descendants {
                    if !asset_chain.iter().any(|a| a == aid) {
                        continue;
                    }
                } else if &dev_asset_id != aid {
                    continue;
                }
            }
            out.push(rec);
        }
        Ok(out)
    }

    pub fn resolve_resource_by_uri(
        conn: &duckdb::Connection,
        ctx: &VerifiedScopeContext,
        uri: &str,
        now_ms: i64,
    ) -> Result<ResolvedResourceUri, EngineError> {
        ctx.require_action("catalog:read")?;

        let row: Option<(String, String, String, Option<i64>)> = conn
            .query_row(
                "SELECT tenant_id, resource_id, path_state, redirect_until_ms FROM resource_path WHERE uri = ?",
                params![uri],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();

        let (tenant_id, resource_id, path_state, redirect_until_ms) = row.ok_or_else(|| {
            EngineError::new(
                "req-resolve-uri",
                ErrorCode::ResourceNotFound,
                format!("Resource URI '{}' not found", uri),
                vec![],
            )
        })?;

        if path_state == "redirected" {
            if let Some(until) = redirect_until_ms {
                if now_ms > until {
                    return Err(EngineError::new(
                        "req-resolve-uri",
                        ErrorCode::ResourceNotFound,
                        format!("Redirect URI '{}' has expired", uri),
                        vec![],
                    ));
                }
            }
        }

        let (device_id, property, canonical_uri): (String, String, String) = conn.query_row(
            "SELECT device_id, property, canonical_uri FROM resource WHERE resource_id = ?",
            params![resource_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;

        let asset_id: String = conn.query_row(
            "SELECT asset_id FROM device WHERE device_id = ?",
            params![device_id],
            |r| r.get(0),
        )?;
        let asset_chain = Self::load_asset_chain(conn, &asset_id)?;

        // Verify Scope before returning resource or redirect target!
        if !ctx.allows_resource(
            &tenant_id,
            &asset_chain,
            &device_id,
            &resource_id,
            &canonical_uri,
        ) {
            return Err(EngineError::new(
                "req-resolve-uri",
                ErrorCode::ResourceNotFound,
                format!("Resource URI '{}' not found", uri),
                vec![],
            ));
        }

        Ok(ResolvedResourceUri {
            resource_id,
            device_id,
            property,
            canonical_uri,
            matched_path_state: path_state,
        })
    }
}
