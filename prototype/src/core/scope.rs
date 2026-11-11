use crate::error::{EngineError, ErrorCode};
use data_encoding::BASE64URL_NOPAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block_key = [0u8; 64];
    if key.len() > 64 {
        let mut h = Sha256::new();
        h.update(key);
        let hashed = h.finalize();
        block_key[..32].copy_from_slice(&hashed);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= block_key[i];
        opad[i] ^= block_key[i];
    }
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_digest);
    let out = outer.finalize();
    let mut res = [0u8; 32];
    res.copy_from_slice(&out);
    res
}

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

    pub fn has_action(&self, action: &str) -> bool {
        self.allowed_actions.iter().any(|a| a == action || a == "*")
    }

    pub fn require_action(&self, action: &str) -> Result<(), EngineError> {
        if self.has_action(action) {
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

    /// Issue an HMAC-SHA256 signed Scope token (`Q02` / `SC01`):
    /// Format: `iotfast1.<base64url_nopad(json)>.<hex(hmac_sha256)>`
    pub fn issue_signed_token(&self, secret: &str) -> String {
        let json_bytes = serde_json::to_vec(self).expect("serialize VerifiedScopeContext");
        let b64 = BASE64URL_NOPAD.encode(&json_bytes);
        let signing_input = format!("iotfast1.{}", b64);
        let mac = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
        format!("{}.{}", signing_input, hex::encode(mac))
    }

    /// Verify an HMAC-SHA256 signed Scope token and enforce `[issued_at_ms, expires_at_ms)` (`Q02` / `SC01`).
    pub fn verify_signed_token(
        token: &str,
        secret: &str,
        now_ms: i64,
    ) -> Result<Self, EngineError> {
        let raw = token
            .strip_prefix("Bearer ")
            .or_else(|| token.strip_prefix("bearer "))
            .unwrap_or(token)
            .trim();
        let parts: Vec<&str> = raw.split('.').collect();
        if parts.len() != 3 || parts[0] != "iotfast1" {
            return Err(EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Invalid signed scope token format",
                vec![],
            ));
        }
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let expected_mac = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
        let provided_mac = hex::decode(parts[2]).map_err(|_| {
            EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Invalid token signature hex",
                vec![],
            )
        })?;
        // Constant-time comparison
        if provided_mac.len() != expected_mac.len()
            || provided_mac
                .iter()
                .zip(expected_mac.iter())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                != 0
        {
            return Err(EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Scope token HMAC signature verification failed",
                vec![],
            ));
        }
        let payload_bytes = BASE64URL_NOPAD.decode(parts[1].as_bytes()).map_err(|_| {
            EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Invalid base64url payload in scope token",
                vec![],
            )
        })?;
        let ctx: Self = serde_json::from_slice(&payload_bytes).map_err(|_| {
            EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Invalid VerifiedScopeContext JSON in scope token",
                vec![],
            )
        })?;
        if now_ms < ctx.issued_at_ms {
            return Err(EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Service credential is not yet valid (now_ms < issued_at_ms)",
                vec![],
            ));
        }
        ctx.validate_active(now_ms)?;
        Ok(ctx)
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
