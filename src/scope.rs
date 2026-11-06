use crate::error::{EngineError, ErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerKind {
    SouthboundSource,
    NorthboundService,
    CatalogReader,
    ManagementService,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeFilter {
    pub asset_ids: Vec<String>,
    pub include_asset_descendants: bool,
    pub device_ids: Vec<String>,
    pub resource_ids: Vec<String>,
    pub uri_prefixes: Vec<String>,
    #[serde(default)]
    pub tenant_wide: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedScopeContext {
    pub tenant_id: String,
    pub project_id: String,
    pub caller_service_id: String,
    pub caller_kind: CallerKind,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub allowed_actions: Vec<String>,
    pub scope_filter: ScopeFilter,
    pub operator_id: Option<String>,
}

/// Path-segment-aware URI prefix matching (`SC03`):
/// `uns://8iic/plant-a/` matches `uns://8iic/plant-a/room-1/pump-01`,
/// and NEVER matches `uns://8iic/plant-ab/pump-01`.
pub fn uri_prefix_matches(prefix: &str, candidate_uri: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    let norm_prefix = prefix.trim_end_matches('/');
    let norm_cand = candidate_uri.trim_end_matches('/');
    if norm_cand == norm_prefix {
        return true;
    }
    let with_slash = format!("{}/", norm_prefix);
    norm_cand.starts_with(&with_slash)
}

impl VerifiedScopeContext {
    pub fn validate_active(&self, now_ms: i64) -> Result<(), EngineError> {
        if self.tenant_id.is_empty() || self.caller_service_id.is_empty() {
            return Err(EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Missing tenant_id or caller_service_id in VerifiedScopeContext",
                vec![],
            ));
        }
        if now_ms >= self.expires_at_ms {
            return Err(EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Service credential has expired",
                vec![],
            ));
        }
        Ok(())
    }

    pub fn require_action(&self, action: &str) -> Result<(), EngineError> {
        if self.allowed_actions.iter().any(|a| a == action || a == "*") {
            Ok(())
        } else {
            Err(EngineError::new(
                "req-auth",
                ErrorCode::ForbiddenAction,
                format!(
                    "Service '{}' is not granted action '{}'",
                    self.caller_service_id, action
                ),
                vec![],
            ))
        }
    }

    /// Evaluates whether a resource belongs to this caller's `VerifiedScopeContext` (R05 & R09):
    /// - Tenant must match first;
    /// - If `tenant_wide == true`, grants all resources in `tenant_id`;
    /// - Otherwise, non-empty dimensions (`asset_ids`, `device_ids`, `resource_ids`, `uri_prefixes`)
    ///   form an explicit union within the tenant; all empty -> fail-closed (`false`).
    pub fn allows_resource(
        &self,
        resource_tenant_id: &str,
        asset_chain: &[String],
        device_id: &str,
        resource_id: &str,
        canonical_uri: &str,
    ) -> bool {
        if resource_tenant_id != self.tenant_id {
            return false;
        }
        let sf = &self.scope_filter;
        if sf.tenant_wide {
            return true;
        }
        if sf.asset_ids.is_empty()
            && sf.device_ids.is_empty()
            && sf.resource_ids.is_empty()
            && sf.uri_prefixes.is_empty()
        {
            return false;
        }
        if sf.resource_ids.iter().any(|r| r == resource_id) {
            return true;
        }
        if sf.device_ids.iter().any(|d| d == device_id) {
            return true;
        }
        if !sf.asset_ids.is_empty() && !asset_chain.is_empty() {
            let direct_asset = &asset_chain[asset_chain.len() - 1];
            if sf.asset_ids.iter().any(|a| a == direct_asset) {
                return true;
            }
            if sf.include_asset_descendants {
                if asset_chain
                    .iter()
                    .any(|ancestor| sf.asset_ids.iter().any(|a| a == ancestor))
                {
                    return true;
                }
            }
        }
        if sf
            .uri_prefixes
            .iter()
            .any(|p| uri_prefix_matches(p, canonical_uri))
        {
            return true;
        }
        false
    }

    pub fn scope_hash(&self) -> String {
        let mut actions = self.allowed_actions.clone();
        actions.sort();
        let mut assets = self.scope_filter.asset_ids.clone();
        assets.sort();
        let mut devices = self.scope_filter.device_ids.clone();
        devices.sort();
        let mut resources = self.scope_filter.resource_ids.clone();
        resources.sort();
        let mut prefixes = self.scope_filter.uri_prefixes.clone();
        prefixes.sort();

        let repr = serde_json::json!({
            "tenant_id": self.tenant_id,
            "project_id": self.project_id,
            "caller_service_id": self.caller_service_id,
            "actions": actions,
            "assets": assets,
            "descendants": self.scope_filter.include_asset_descendants,
            "devices": devices,
            "resources": resources,
            "prefixes": prefixes,
            "tenant_wide": self.scope_filter.tenant_wide,
        });
        let mut hasher = Sha256::new();
        hasher.update(repr.to_string().as_bytes());
        hex::encode(hasher.finalize())
    }
}
