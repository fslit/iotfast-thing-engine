use crate::error::{EngineError, ErrorCode};
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

#[derive(Debug, Clone)]
pub struct RelocateDeviceResult {
    pub records: Vec<ResourcePathRecord>,
    pub redirect_until_ms: i64,
}

pub struct AssetUnsService;

impl AssetUnsService {
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
                format!("Duplicate asset_id or sibling slug path '{}': {}", asset_path, e),
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
        let canonical_device_uri = format!(
            "uns://{}/{}/{}",
            ctx.tenant_id, asset_path, req.device_key
        );

        // NB02: Check if any target canonical URI is currently locked in resource_path
        // Allow reclaim of expired redirect entries (now_ms > redirect_until_ms)
        let now_ms = ctx.issued_at_ms; // Use credential issue time as reference
        for prop_id in props.keys() {
            let prop_uri = format!("{}/properties/{}", canonical_device_uri, prop_id);
            let existing: Option<(String, String, Option<i64>)> = conn
                .query_row(
                    "SELECT path_state, resource_id, redirect_until_ms FROM resource_path WHERE uri = ?",
                    params![prop_uri],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok();
            if let Some((path_state, _existing_res_id, redirect_until)) = existing {
                if path_state == "canonical" {
                    return Err(EngineError::new(
                        "req-device",
                        ErrorCode::RevisionConflict,
                        format!("URI '{}' is already occupied by another canonical resource", prop_uri),
                        vec![],
                    ));
                }
                // path_state == "redirected": check if lock window is still active
                if let Some(until_ms) = redirect_until {
                    if now_ms <= until_ms {
                        return Err(EngineError::new(
                            "req-device",
                            ErrorCode::RevisionConflict,
                            format!("URI '{}' is in active redirect lock window until {}", prop_uri, until_ms),
                            vec![],
                        ));
                    }
                    // Expired redirect — delete old entry so INSERT below succeeds
                    conn.execute("DELETE FROM resource_path WHERE uri = ?", params![prop_uri])?;
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
                let resource_id = format!(
                    "res-{}-{}",
                    req.device_id,
                    prop_id.replace('_', "-")
                );
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
                // NB02: Check if new_uri is occupied by ANOTHER resource (canonical or active redirect lock)
                let conflict: Option<(String, String, Option<i64>)> = conn
                    .query_row(
                        "SELECT resource_id, path_state, redirect_until_ms FROM resource_path WHERE uri = ?",
                        params![new_uri],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();
                if let Some((conflict_res_id, conflict_state, conflict_until)) = conflict {
                    if conflict_res_id != res_id {
                        let is_locked = conflict_state == "canonical"
                            || conflict_until.map_or(true, |u| now_ms <= u);
                        if is_locked {
                            return Err(EngineError::new(
                                "req-relocate",
                                ErrorCode::RevisionConflict,
                                format!("Target URI '{}' is occupied by resource '{}' (NB02)", new_uri, conflict_res_id),
                                vec![],
                            ));
                        }
                        // Expired redirect from another resource — safe to reclaim
                        conn.execute("DELETE FROM resource_path WHERE uri = ?", params![new_uri])?;
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
