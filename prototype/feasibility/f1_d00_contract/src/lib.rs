use data_encoding::BASE32_NOPAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

// ============================================================================
// 1. Unified Error Envelope & Error Codes (D00 Section 7)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Unauthenticated,
    ForbiddenAction,
    SourceNotAuthorized,
    ResourceNotFound,
    IdempotencyConflict,
    RevisionConflict,
    ValidationFailed,
    ClockSkewExceeded,
    CrossVersionIncompatible,
    CapabilityUnsupported,
    RateLimited,
    DurabilityBackpressure,
    SourceUnavailable,
}

impl ErrorCode {
    pub fn http_status(&self) -> u16 {
        match self {
            ErrorCode::Unauthenticated => 401,
            ErrorCode::ForbiddenAction => 403,
            ErrorCode::SourceNotAuthorized => 403,
            ErrorCode::ResourceNotFound => 404,
            ErrorCode::IdempotencyConflict => 409,
            ErrorCode::RevisionConflict => 409,
            ErrorCode::ValidationFailed => 422,
            ErrorCode::ClockSkewExceeded => 422,
            ErrorCode::CrossVersionIncompatible => 422,
            ErrorCode::CapabilityUnsupported => 422,
            ErrorCode::RateLimited => 429,
            ErrorCode::DurabilityBackpressure => 503,
            ErrorCode::SourceUnavailable => 503,
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            ErrorCode::RateLimited
                | ErrorCode::DurabilityBackpressure
                | ErrorCode::SourceUnavailable
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub field: String,
    pub issue: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(skip_serializing)]
    pub status: u16,
    pub request_id: String,
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub details: Vec<ErrorDetail>,
}

impl ErrorEnvelope {
    pub fn new(
        request_id: impl Into<String>,
        code: ErrorCode,
        message: impl Into<String>,
        details: Vec<ErrorDetail>,
    ) -> Self {
        Self {
            status: code.http_status(),
            request_id: request_id.into(),
            code,
            message: message.into(),
            retryable: code.retryable(),
            retry_after_ms: None,
            details,
        }
    }
}

// ============================================================================
// 2. D00 Deterministic Identifiers, WAL Position & JCS Canonicalization
// ============================================================================

/// Validates tenant_id / device_id / resource_id / source_id:
/// ASCII `[a-z0-9][a-z0-9_-]{1,62}[a-z0-9]`, length 3..=64
pub fn validate_entity_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.len() < 3 || bytes.len() > 64 {
        return false;
    }
    let is_alnum_lower = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_alnum_lower(bytes[0]) || !is_alnum_lower(bytes[bytes.len() - 1]) {
        return false;
    }
    bytes[1..bytes.len() - 1]
        .iter()
        .all(|&b| is_alnum_lower(b) || b == b'_' || b == b'-')
}

/// Validates message_id: printable ASCII (0x21..=0x7E), length 1..=128
pub fn validate_message_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=128).contains(&bytes.len()) && bytes.iter().all(|&b| (0x21..=0x7E).contains(&b))
}

/// Validates source_epoch: ASCII `[A-Za-z0-9_.:-]{1,64}`
pub fn validate_source_epoch(epoch: &str) -> bool {
    let bytes = epoch.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes.iter().all(|&b| {
            b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'-'
        })
}

/// Validates and parses `source_seq`: `^(0|[1-9][0-9]{0,19})$` in `0..=u64::MAX`
pub fn parse_source_seq(seq: &str) -> Result<u64, String> {
    if seq.is_empty() || seq.len() > 20 {
        return Err(format!("Invalid source_seq length: '{}'", seq));
    }
    let bytes = seq.as_bytes();
    if bytes.len() > 1 && bytes[0] == b'0' {
        return Err(format!("source_seq must not have leading zeros: '{}'", seq));
    }
    if !bytes.iter().all(|b| b.is_ascii_digit()) {
        return Err(format!("source_seq must contain only ASCII digits: '{}'", seq));
    }
    seq.parse::<u64>()
        .map_err(|_| format!("source_seq overflows u64::MAX: '{}'", seq))
}

/// Fixed-width comparable WAL position `{segment_no:016x}:{record_offset:016x}`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WalPosition {
    pub segment_no: u64,
    pub record_offset: u64,
}

impl WalPosition {
    pub fn format_fixed(&self) -> String {
        format!("{:016x}:{:016x}", self.segment_no, self.record_offset)
    }

    pub fn parse_fixed(s: &str) -> Result<Self, String> {
        if s.len() != 33 || &s[16..17] != ":" {
            return Err(format!("Invalid wal_position format: '{}'", s));
        }
        let seg = &s[0..16];
        let off = &s[17..33];
        if !seg.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !off.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(format!("wal_position must be 16-char lowercase hex parts: '{}'", s));
        }
        let segment_no = u64::from_str_radix(seg, 16).map_err(|e| e.to_string())?;
        let record_offset = u64::from_str_radix(off, 16).map_err(|e| e.to_string())?;
        Ok(Self {
            segment_no,
            record_offset,
        })
    }
}

/// Deterministic `observation_id` derivation (D00 Section 4.1):
/// `"obs-" + Base32LowerNoPad(SHA-256("iotfast.obs.v1\0" + tenant_id + "\0" + source_id + "\0" + source_epoch + "\0" + message_id + "\0" + property)[0..16])`
pub fn derive_observation_id(
    tenant_id: &str,
    source_id: &str,
    source_epoch: &str,
    message_id: &str,
    property: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"iotfast.obs.v1\0");
    hasher.update(tenant_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_epoch.as_bytes());
    hasher.update(b"\0");
    hasher.update(message_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(property.as_bytes());
    let digest = hasher.finalize();
    let encoded = BASE32_NOPAD.encode(&digest[0..16]).to_ascii_lowercase();
    format!("obs-{}", encoded)
}

/// Deterministic collision-free `resource_id` fallback (Q11):
/// `"res-" + Base32LowerNoPad(SHA-256("iotfast.res.v1\0" + tenant_id + "\0" + device_id + "\0" + kind + "\0" + identifier)[0..16])`
pub fn derive_resource_id(
    tenant_id: &str,
    device_id: &str,
    kind: &str,
    identifier: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"iotfast.res.v1\0");
    hasher.update(tenant_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(device_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(kind.as_bytes());
    hasher.update(b"\0");
    hasher.update(identifier.as_bytes());
    let digest = hasher.finalize();
    let encoded = BASE32_NOPAD.encode(&digest[0..16]).to_ascii_lowercase();
    format!("res-{}", encoded)
}

/// RFC 8785 (JCS) / ECMA-262 Number::toString canonical formatting for IEEE 754 f64.
/// - Rejects NaN and ±Infinity.
/// - Normalizes -0.0 and +0.0 to "0".
/// - Formats shortest exact round-trip decimal representation per RFC 8785.
pub fn format_jcs_f64(value: f64) -> Result<String, String> {
    if value.is_nan() || value.is_infinite() {
        return Err("float64 value must be finite (NaN/Infinity forbidden)".to_string());
    }
    if value == 0.0 {
        return Ok("0".to_string());
    }

    let negative = value.is_sign_negative();
    let abs_val = value.abs();

    // Use ryu to get shortest round-trip decimal digits and exponent
    let mut buffer = ryu::Buffer::new();
    let ryu_str = buffer.format_finite(abs_val);

    // Parse ryu_str into significant digits (without decimal point or trailing zeros) and n_exp
    // where abs_val = 0.<digits> * 10^n_exp (so scientific exponent e = n_exp - 1).
    let (mantissa_str, exp_part) = if let Some(idx) = ryu_str.find('e') {
        (&ryu_str[..idx],ryu_str[idx + 1..].parse::<i32>().unwrap())
    } else {
        (ryu_str, 0)
    };

    let mut raw_digits = String::with_capacity(24);
    let mut frac_len: i32 = 0;
    let mut seen_dot = false;
    for ch in mantissa_str.chars() {
        if ch == '.' {
            seen_dot = true;
        } else {
            raw_digits.push(ch);
            if seen_dot {
                frac_len += 1;
            }
        }
    }

    // Strip leading zeros
    let trimmed_lead = raw_digits.trim_start_matches('0');
    let leading_zeros = (raw_digits.len() - trimmed_lead.len()) as i32;
    // Strip trailing zeros
    let digits = trimmed_lead.trim_end_matches('0');
    let k = digits.len() as i32;

    // Total number of integer digits before decimal point in mantissa_str was (raw_digits.len() as i32 - frac_len).
    // Adjusting for leading zeros and exp_part gives n_exp:
    let int_digits_in_mantissa = (raw_digits.len() as i32) - frac_len;
    let n_exp = int_digits_in_mantissa - leading_zeros + exp_part;

    let mut out = String::with_capacity(32);
    if negative {
        out.push('-');
    }

    if k <= n_exp && n_exp <= 21 {
        out.push_str(digits);
        for _ in 0..(n_exp - k) {
            out.push('0');
        }
    } else if 0 < n_exp && n_exp < k && n_exp <= 21 {
        let split = n_exp as usize;
        out.push_str(&digits[..split]);
        out.push('.');
        out.push_str(&digits[split..]);
    } else if (-5..=0).contains(&n_exp) {
        out.push_str("0.");
        for _ in 0..(-n_exp) {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        // Scientific notation: e = n_exp - 1
        let e = n_exp - 1;
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        if e >= 0 {
            out.push('+');
        }
        out.push_str(&e.to_string());
    }

    Ok(out)
}

/// RFC 8785 (JCS) canonical string serialization
pub fn format_jcs_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn format_jcs_opt_string(opt: &Option<String>) -> String {
    match opt {
        Some(s) => format_jcs_string(s),
        None => "null".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Float64,
    Int64,
    Bool,
    Text,
}

impl ValueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValueType::Float64 => "float64",
            ValueType::Int64 => "int64",
            ValueType::Bool => "bool",
            ValueType::Text => "text",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypedPointValue {
    Bool(bool),
    Int64(i64),
    Float64(f64),
    Text(String),
}

impl TypedPointValue {
    pub fn to_jcs_literal(&self) -> Result<String, String> {
        match self {
            TypedPointValue::Bool(b) => Ok(if *b { "true" } else { "false" }.to_string()),
            TypedPointValue::Int64(i) => Ok(i.to_string()),
            TypedPointValue::Float64(f) => format_jcs_f64(*f),
            TypedPointValue::Text(s) => Ok(format_jcs_string(s)),
        }
    }

    pub fn to_json_value(&self) -> Value {
        match self {
            TypedPointValue::Bool(b) => Value::Bool(*b),
            TypedPointValue::Int64(i) => Value::Number((*i).into()),
            TypedPointValue::Float64(f) => {
                let norm = if *f == 0.0 { 0.0 } else { *f };
                serde_json::Number::from_f64(norm)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
            TypedPointValue::Text(s) => Value::String(s.clone()),
        }
    }
}

/// Canonical point structure included in `sha256-jcs-v1` (D00 Section 4.2)
#[derive(Debug, Clone)]
pub struct CanonicalPointInput {
    pub property: String,
    pub value_type: ValueType,
    pub value: TypedPointValue,
    pub source_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub quality_state: String,
    pub quality_source_protocol: Option<String>,
    pub quality_source_code: Option<String>,
}

/// Canonicalizes a telemetry message according to `sha256-jcs-v1` (D00 Section 4.2)
/// Returns `(canonical_json_string, payload_hash_string)`.
pub fn compute_jcs_v1_payload_hash(
    spec: &str,
    tenant_id: &str,
    source_id: &str,
    source_epoch: &str,
    device_id: &str,
    model_revision_id: &str,
    binding_revision_id: &str,
    points: &[CanonicalPointInput],
) -> Result<(String, String), String> {
    // Sort points by `property` byte order ascending
    let mut sorted_points: Vec<&CanonicalPointInput> = points.iter().collect();
    sorted_points.sort_by(|a, b| a.property.as_bytes().cmp(b.property.as_bytes()));

    let mut points_jcs = Vec::with_capacity(sorted_points.len());
    for pt in sorted_points {
        let val_jcs = pt.value.to_jcs_literal()?;
        // Keys in point object sorted by UTF-16 code units (ASCII order):
        // "property", "quality", "source_at_ms", "source_seq", "source_time_origin", "value", "value_type"
        // Keys in quality object sorted:
        // "source_code", "source_protocol", "state"
        let pt_str = format!(
            "{{\"property\":{},\"quality\":{{\"source_code\":{},\"source_protocol\":{},\"state\":{}}},\"source_at_ms\":{},\"source_seq\":{},\"source_time_origin\":{},\"value\":{},\"value_type\":{}}}",
            format_jcs_string(&pt.property),
            format_jcs_opt_string(&pt.quality_source_code),
            format_jcs_opt_string(&pt.quality_source_protocol),
            format_jcs_string(&pt.quality_state),
            pt.source_at_ms,
            format_jcs_string(&pt.source_seq),
            format_jcs_string(&pt.source_time_origin),
            val_jcs,
            format_jcs_string(pt.value_type.as_str()),
        );
        points_jcs.push(pt_str);
    }

    // Top-level keys sorted by UTF-16 code units (ASCII order):
    // "binding_revision_id", "device_id", "model_revision_id", "points", "source_epoch", "source_id", "spec", "tenant_id"
    let canonical_json = format!(
        "{{\"binding_revision_id\":{},\"device_id\":{},\"model_revision_id\":{},\"points\":[{}],\"source_epoch\":{},\"source_id\":{},\"spec\":{},\"tenant_id\":{}}}",
        format_jcs_string(binding_revision_id),
        format_jcs_string(device_id),
        format_jcs_string(model_revision_id),
        points_jcs.join(","),
        format_jcs_string(source_epoch),
        format_jcs_string(source_id),
        format_jcs_string(spec),
        format_jcs_string(tenant_id),
    );

    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    let digest_hex = hex::encode(hasher.finalize());
    let payload_hash = format!("sha256:{}", digest_hex);

    Ok((canonical_json, payload_hash))
}

/// Computes `sha256-jcs-v1` canonical `event_hash` for `iotfast.source_state.v1` (Q04).
pub fn compute_source_state_event_hash(
    spec: &str,
    tenant_id: &str,
    source_id: &str,
    source_epoch: &str,
    device_id: &str,
    binding_revision_id: &str,
    observed_at_ms: i64,
    source_time_origin: &str,
    source_seq: &str,
    connection_state: &str,
    point_states: &[SourcePointStateWire],
) -> (String, String) {
    let mut sorted_pts: Vec<&SourcePointStateWire> = point_states.iter().collect();
    sorted_pts.sort_by(|a, b| a.property.as_bytes().cmp(b.property.as_bytes()));

    let mut pts_jcs = Vec::with_capacity(sorted_pts.len());
    for pt in sorted_pts {
        let pt_str = format!(
            "{{\"property\":{},\"quality_reason\":{},\"quality_state\":{},\"source_code\":{},\"source_protocol\":{}}}",
            format_jcs_string(&pt.property),
            format_jcs_string(&pt.quality_reason),
            format_jcs_string(&pt.quality_state),
            format_jcs_opt_string(&pt.source_code),
            format_jcs_opt_string(&pt.source_protocol),
        );
        pts_jcs.push(pt_str);
    }

    let canonical_json = format!(
        "{{\"binding_revision_id\":{},\"connection_state\":{},\"device_id\":{},\"observed_at_ms\":{},\"point_states\":[{}],\"source_epoch\":{},\"source_id\":{},\"source_seq\":{},\"source_time_origin\":{},\"spec\":{},\"tenant_id\":{}}}",
        format_jcs_string(binding_revision_id),
        format_jcs_string(connection_state),
        format_jcs_string(device_id),
        observed_at_ms,
        pts_jcs.join(","),
        format_jcs_string(source_epoch),
        format_jcs_string(source_id),
        format_jcs_string(source_seq),
        format_jcs_string(source_time_origin),
        format_jcs_string(spec),
        format_jcs_string(tenant_id),
    );

    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    let event_hash = format!("sha256:{}", hex::encode(hasher.finalize()));
    (canonical_json, event_hash)
}

// ============================================================================
// 3. Service Identity & VerifiedScopeContext (D00 Section 1-3)
// ============================================================================

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
    pub spec: String,
    pub tenant_id: String,
    pub project_id: Option<String>,
    pub caller_service_id: String,
    pub caller_kind: CallerKind,
    /// Bound `source_id` when `caller_kind == SouthboundSource`
    pub bound_source_id: Option<String>,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub allowed_actions: Vec<String>,
    pub scope_filter: ScopeFilter,
    pub quota_profile: String,
}

impl VerifiedScopeContext {
    pub fn validate_active(&self, request_id: &str, now_ms: i64) -> Result<(), ErrorEnvelope> {
        if self.spec != "iotfast.scope_context.v1" || !validate_entity_id(&self.tenant_id) {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Invalid scope context specification or tenant_id",
                vec![],
            ));
        }
        if now_ms >= self.expires_at_ms {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Service credential has expired",
                vec![],
            ));
        }
        Ok(())
    }

    pub fn require_action(&self, request_id: &str, action: &str) -> Result<(), ErrorEnvelope> {
        if !self.allowed_actions.iter().any(|a| a == action) {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ForbiddenAction,
                format!(
                    "Caller '{}' ({:?}) is not permitted to perform '{}'",
                    self.caller_service_id, self.caller_kind, action
                ),
                vec![],
            ));
        }
        Ok(())
    }

    /// Computes a deterministic hash of `(tenant_id, scope_filter)` for cursor and cache binding (D00 Section 3 & NB09)
    pub fn scope_hash(&self) -> String {
        let mut assets = self.scope_filter.asset_ids.clone();
        assets.sort();
        let mut devices = self.scope_filter.device_ids.clone();
        devices.sort();
        let mut resources = self.scope_filter.resource_ids.clone();
        resources.sort();
        let mut prefixes = self.scope_filter.uri_prefixes.clone();
        prefixes.sort();

        let mut hasher = Sha256::new();
        hasher.update(b"iotfast.scope_hash.v1\0");
        hasher.update(self.tenant_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(if self.scope_filter.tenant_wide { b"1" } else { b"0" });
        hasher.update(b"\0");
        hasher.update(if self.scope_filter.include_asset_descendants {
            b"1"
        } else {
            b"0"
        });
        hasher.update(b"\0");
        for a in &assets {
            hasher.update(a.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\0");
        for d in &devices {
            hasher.update(d.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\0");
        for r in &resources {
            hasher.update(r.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\0");
        for p in &prefixes {
            hasher.update(p.as_bytes());
            hasher.update(b",");
        }
        hex::encode(hasher.finalize())
    }
}

/// URI segment-boundary prefix matching (D00 Section 2.2 & SC03).
/// `uns://8iic/plant-a/` matches `uns://8iic/plant-a/pump-001`,
/// but NEVER matches `uns://8iic/plant-ab/pump-001`.
pub fn uri_prefix_matches(prefix: &str, target_uri: &str) -> bool {
    const SCHEME: &str = "uns://";
    if !prefix.starts_with(SCHEME) || !target_uri.starts_with(SCHEME) {
        return false;
    }
    let prefix_trimmed = prefix[SCHEME.len()..].trim_end_matches('/');
    let target_trimmed = target_uri[SCHEME.len()..].trim_end_matches('/');
    if prefix_trimmed.is_empty() {
        return false;
    }
    let prefix_segments: Vec<&str> = prefix_trimmed.split('/').collect();
    let target_segments: Vec<&str> = target_trimmed.split('/').collect();
    if prefix_segments.iter().any(|s| s.is_empty() || *s == "." || *s == "..")
        || target_segments.iter().any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return false;
    }
    if target_segments.len() < prefix_segments.len() {
        return false;
    }
    prefix_segments
        .iter()
        .zip(target_segments.iter())
        .all(|(p, t)| p == t)
}

// ============================================================================
// 4. Domain Model, Binding, Telemetry & Source State Wire Structures
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitSpec {
    pub code: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertySourceKind {
    GatewayMeasurement,
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertySpec {
    pub identifier: String,
    pub name: String,
    pub data_type: ValueType,
    pub unit: Option<UnitSpec>,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub source_kind: PropertySourceKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRevision {
    pub model_revision_id: String,
    pub tenant_id: String,
    pub product_id: String,
    pub product_key: String,
    pub model_version: String,
    pub published: bool,
    pub properties: BTreeMap<String, PropertySpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingPointMapping {
    pub property: String,
    pub register_address: String,
    /// Protocol-side scaling factor (already applied by gateway when normalization_owner == "gateway")
    pub factor: f64,
    pub unit: Option<UnitSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingRevision {
    pub binding_revision_id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub source_id: String,
    /// Must be `"gateway"` (`gateway_normalized`) in P0; server never applies `factor` a second time.
    pub normalization_owner: String,
    /// Forbidden when `normalization_owner == "gateway"`
    pub server_scale_factor: Option<f64>,
    pub active: bool,
    /// Optional proven validity window `[valid_from_ms, valid_to_ms)` for historical/superseded bindings
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
    pub mapped_points: BTreeMap<String, BindingPointMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetNode {
    pub asset_id: String,
    pub tenant_id: String,
    pub parent_asset_id: Option<String>,
    pub slug: String,
    pub asset_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInstance {
    pub device_id: String,
    pub tenant_id: String,
    pub device_key: String,
    pub device_name: String,
    pub product_id: String,
    pub product_key: String,
    pub model_revision_id: String,
    pub active_binding_revision_id: String,
    pub asset_id: String,
    pub canonical_device_uri: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceEntry {
    pub resource_id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub asset_id: String,
    pub property: String,
    pub name: String,
    pub canonical_uri: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryQualityWire {
    pub state: String,
    #[serde(default)]
    pub source_protocol: Option<String>,
    #[serde(default)]
    pub source_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryDeliveryWire {
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryPointWire {
    pub property: String,
    pub value: Value,
    pub source_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub quality: TelemetryQualityWire,
    pub delivery: TelemetryDeliveryWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryMessageWire {
    pub spec: String,
    pub message_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub points: Vec<TelemetryPointWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryIngestAck {
    #[serde(skip_serializing)]
    pub http_status: u16,
    pub request_id: String,
    pub ingest_id: String,
    pub message_id: String,
    pub status: String,
    pub history_status: String,
    pub payload_hash: String,
    pub observation_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePointStateWire {
    pub property: String,
    pub quality_state: String,
    pub quality_reason: String,
    pub source_protocol: Option<String>,
    pub source_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStateEventWire {
    pub spec: String,
    pub event_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub device_id: String,
    pub binding_revision_id: String,
    pub observed_at_ms: i64,
    pub connection_state: String,
    pub point_states: Vec<SourcePointStateWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStateAck {
    #[serde(skip_serializing)]
    pub http_status: u16,
    pub request_id: String,
    pub event_id: String,
    pub status: String,
}

// ============================================================================
// 5. Northbound View, Shadow, and History Query Structures
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicePropertyView {
    pub resource_id: String,
    pub name: String,
    pub data_type: String,
    pub unit: Option<UnitSpec>,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceBusinessView {
    pub request_id: String,
    pub device_id: String,
    pub device_key: String,
    pub device_name: String,
    pub product_id: String,
    pub product_key: String,
    pub model_revision_id: String,
    pub model_version: String,
    pub asset_id: String,
    pub canonical_device_uri: String,
    pub active_binding_revision_id: String,
    pub source_connection_state: String,
    pub view_generated_at_ms: i64,
    pub view_consistency: String,
    pub properties: BTreeMap<String, DevicePropertyView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceShadowPoint {
    pub resource_id: String,
    pub value: Value,
    pub value_sampled_at_ms: Option<i64>,
    pub quality_observed_at_ms: Option<i64>,
    pub received_at_ms: Option<i64>,
    pub quality: String,
    pub freshness: String,
    pub model_revision_id: String,
    pub history_status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceShadow {
    pub request_id: String,
    pub device_id: String,
    pub revision: u64,
    pub source_connection_state: String,
    pub points: BTreeMap<String, DeviceShadowPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryObservation {
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
    pub unit: Option<UnitSpec>,
    pub quality: String,
    pub source_quality_code: Option<String>,
    pub replayed: bool,
    pub record_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityEventRecord {
    pub event_id: String,
    pub device_id: String,
    pub property: String,
    pub observed_at_ms: i64,
    pub quality_state: String,
    pub quality_reason: String,
    pub connection_state: String,
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
    pub empty_bucket_policy: String,
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
pub struct AggregateBucketItem {
    pub resource_id: String,
    pub bucket_start_ms: i64,
    pub bucket_end_ms: i64,
    pub unit: Option<UnitSpec>,
    pub count: usize,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryQueryResponse {
    pub request_id: String,
    pub target_id: String,
    pub source_mode: String,
    pub mode: String,
    pub consistency: String,
    pub partial: bool,
    pub items: Vec<HistoryObservation>,
    pub aggregate_items: Vec<AggregateBucketItem>,
    pub next_cursor: Option<String>,
}

// ============================================================================
// 6. Contract Engine State & Verification Logic
// ============================================================================

#[derive(Debug, Clone)]
struct RuntimePointLatest {
    value: TypedPointValue,
    value_sampled_at_ms: i64,
    received_at_ms: i64,
    source_time_origin: String,
    source_epoch: String,
    source_seq_u64: u64,
    binding_revision_id: String,
    model_revision_id: String,
}

#[derive(Debug, Clone)]
struct RuntimePointQuality {
    quality_state: String,
    quality_reason: Option<String>,
    quality_observed_at_ms: i64,
}

#[derive(Debug, Clone)]
struct AppliedMessageEntry {
    ingest_id: String,
    payload_hash: String,
    history_status: String,
    observation_ids: BTreeMap<String, String>,
}

pub struct ContractVerificationEngine {
    pub max_future_skew_ms: i64,
    pub stale_threshold_ms: i64,
    pub durability_backpressure: bool,
    cursor_signing_secret: String,
    next_ingest_seq: u64,

    assets: BTreeMap<String, AssetNode>,
    models: BTreeMap<String, ModelRevision>,
    bindings: BTreeMap<String, BindingRevision>,
    devices: BTreeMap<String, DeviceInstance>,
    resources: BTreeMap<String, ResourceEntry>,
    /// `(tenant_id, source_id)` -> authorized `device_id` set
    source_device_grants: BTreeMap<(String, String), BTreeSet<String>>,

    /// `(tenant_id, source_id, source_epoch, message_id)` -> AppliedMessageEntry
    applied_messages: BTreeMap<(String, String, String, String), AppliedMessageEntry>,
    /// Independent dedup domain for `iotfast.source_state.v1`: `(tenant_id, source_id, source_epoch, event_id)` -> `event_hash` (R03)
    applied_source_states: BTreeMap<(String, String, String, String), String>,
    /// `resource_id` -> latest measurement
    latest_values: BTreeMap<String, RuntimePointLatest>,
    /// `resource_id` -> current quality state
    point_qualities: BTreeMap<String, RuntimePointQuality>,
    /// `device_id` -> connection state ("connected" | "disconnected" | "unknown")
    device_connection_states: BTreeMap<String, String>,

    history_observations: Vec<HistoryObservation>,
    quality_events: Vec<QualityEventRecord>,
}

impl Default for ContractVerificationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ContractVerificationEngine {
    pub fn new() -> Self {
        Self {
            max_future_skew_ms: 60_000,
            stale_threshold_ms: 30_000,
            durability_backpressure: false,
            cursor_signing_secret: "iotfast-d00-cursor-hmac-secret-v1".to_string(),
            next_ingest_seq: 1,
            assets: BTreeMap::new(),
            models: BTreeMap::new(),
            bindings: BTreeMap::new(),
            devices: BTreeMap::new(),
            resources: BTreeMap::new(),
            source_device_grants: BTreeMap::new(),
            applied_messages: BTreeMap::new(),
            applied_source_states: BTreeMap::new(),
            latest_values: BTreeMap::new(),
            point_qualities: BTreeMap::new(),
            device_connection_states: BTreeMap::new(),
            history_observations: Vec::new(),
            quality_events: Vec::new(),
        }
    }

    pub fn register_asset(&mut self, asset: AssetNode) {
        self.assets.insert(asset.asset_id.clone(), asset);
    }

    pub fn register_model(&mut self, model: ModelRevision) {
        self.models.insert(model.model_revision_id.clone(), model);
    }

    pub fn validate_and_register_binding(
        &mut self,
        request_id: &str,
        binding: BindingRevision,
    ) -> Result<(), ErrorEnvelope> {
        if binding.normalization_owner == "gateway" && binding.server_scale_factor.is_some() {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Secondary server scaling is forbidden when normalization_owner is 'gateway'",
                vec![ErrorDetail {
                    field: "server_scale_factor".to_string(),
                    issue: "DOUBLE_SCALING_FORBIDDEN".to_string(),
                    description: "normalization_owner='gateway' requires server_scale_factor=null"
                        .to_string(),
                }],
            ));
        }
        self.bindings
            .insert(binding.binding_revision_id.clone(), binding);
        Ok(())
    }

    pub fn grant_source_device(&mut self, tenant_id: &str, source_id: &str, device_id: &str) {
        self.source_device_grants
            .entry((tenant_id.to_string(), source_id.to_string()))
            .or_default()
            .insert(device_id.to_string());
    }

    pub fn instantiate_device(&mut self, device: DeviceInstance) {
        let model = self
            .models
            .get(&device.model_revision_id)
            .expect("model must exist before instantiating device")
            .clone();
        for prop in model.properties.values() {
            let resource_id = format!(
                "res-{}-{}",
                device.device_id,
                prop.identifier.replace('_', "-")
            );
            let canonical_uri = format!(
                "{}/properties/{}",
                device.canonical_device_uri, prop.identifier
            );
            self.resources.insert(
                resource_id.clone(),
                ResourceEntry {
                    resource_id,
                    tenant_id: device.tenant_id.clone(),
                    device_id: device.device_id.clone(),
                    asset_id: device.asset_id.clone(),
                    property: prop.identifier.clone(),
                    name: prop.name.clone(),
                    canonical_uri,
                },
            );
        }
        self.device_connection_states
            .insert(device.device_id.clone(), "connected".to_string());
        self.devices.insert(device.device_id.clone(), device);
    }

    pub fn activate_device_binding(
        &mut self,
        device_id: &str,
        new_model_revision_id: &str,
        new_binding_revision_id: &str,
    ) {
        if let Some(dev) = self.devices.get_mut(device_id) {
            let old_bind_id = dev.active_binding_revision_id.clone();
            if let Some(old_b) = self.bindings.get_mut(&old_bind_id) {
                old_b.active = false;
            }
            if let Some(new_b) = self.bindings.get_mut(new_binding_revision_id) {
                new_b.active = true;
            }
            dev.model_revision_id = new_model_revision_id.to_string();
            dev.active_binding_revision_id = new_binding_revision_id.to_string();
            dev.revision += 1;
        }
    }

    pub fn history_count(&self) -> usize {
        self.history_observations.len()
    }

    pub fn quality_events(&self) -> &[QualityEventRecord] {
        &self.quality_events
    }

    // ========================================================================
    // Scope Evaluation (D00 Section 2.2 & Section 3)
    // ========================================================================

    fn is_asset_in_scope(&self, ctx: &VerifiedScopeContext, target_asset_id: &str) -> bool {
        if ctx.scope_filter.asset_ids.iter().any(|a| a == target_asset_id) {
            return true;
        }
        if ctx.scope_filter.include_asset_descendants {
            let mut cursor = self
                .assets
                .get(target_asset_id)
                .and_then(|a| a.parent_asset_id.clone());
            while let Some(parent_id) = cursor {
                if ctx.scope_filter.asset_ids.iter().any(|a| a == &parent_id) {
                    return true;
                }
                cursor = self
                    .assets
                    .get(&parent_id)
                    .and_then(|a| a.parent_asset_id.clone());
            }
        }
        false
    }

    pub fn is_device_authorized(&self, ctx: &VerifiedScopeContext, dev: &DeviceInstance) -> bool {
        if dev.tenant_id != ctx.tenant_id {
            return false;
        }
        let sf = &ctx.scope_filter;
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
        if sf.device_ids.iter().any(|d| d == &dev.device_id) {
            return true;
        }
        if self.is_asset_in_scope(ctx, &dev.asset_id) {
            return true;
        }
        if sf
            .uri_prefixes
            .iter()
            .any(|p| uri_prefix_matches(p, &dev.canonical_device_uri))
        {
            return true;
        }
        false
    }

    pub fn is_resource_authorized(&self, ctx: &VerifiedScopeContext, res: &ResourceEntry) -> bool {
        if res.tenant_id != ctx.tenant_id {
            return false;
        }
        let sf = &ctx.scope_filter;
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
        if sf.resource_ids.iter().any(|r| r == &res.resource_id) {
            return true;
        }
        if sf.device_ids.iter().any(|d| d == &res.device_id) {
            return true;
        }
        if self.is_asset_in_scope(ctx, &res.asset_id) {
            return true;
        }
        if sf
            .uri_prefixes
            .iter()
            .any(|p| uri_prefix_matches(p, &res.canonical_uri))
        {
            return true;
        }
        false
    }

    // ========================================================================
    // Southbound Telemetry Ingest: POST /v1/telemetry (F1 & D00)
    // ========================================================================

    pub fn ingest_telemetry(
        &mut self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        prefer_durable: bool,
        received_at_ms: i64,
        msg: &TelemetryMessageWire,
    ) -> Result<TelemetryIngestAck, ErrorEnvelope> {
        // 1. Authenticate service identity (SC01)
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential; client-supplied headers are not trusted",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, received_at_ms)?;
        ctx.require_action(request_id, "telemetry:ingest")?;

        // 2. Verify southbound source & device grant (403 SOURCE_NOT_AUTHORIZED)
        if ctx.caller_kind != CallerKind::SouthboundSource
            || ctx.bound_source_id.as_deref() != Some(msg.source_id.as_str())
        {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::SourceNotAuthorized,
                format!("Caller is not authorized as source_id '{}'", msg.source_id),
                vec![],
            ));
        }
        let granted = self
            .source_device_grants
            .get(&(ctx.tenant_id.clone(), msg.source_id.clone()))
            .map(|set| set.contains(&msg.device_id))
            .unwrap_or(false);
        if !granted {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::SourceNotAuthorized,
                format!(
                    "Source '{}' is not authorized to report telemetry for device '{}'",
                    msg.source_id, msg.device_id
                ),
                vec![],
            ));
        }

        // 3. Envelope-level validation
        let mut details = Vec::new();
        if msg.spec != "iotfast.telemetry.v1" {
            details.push(ErrorDetail {
                field: "spec".to_string(),
                issue: "UNSUPPORTED_SPEC".to_string(),
                description: format!("Expected 'iotfast.telemetry.v1', got '{}'", msg.spec),
            });
        }
        if !validate_message_id(&msg.message_id) {
            details.push(ErrorDetail {
                field: "message_id".to_string(),
                issue: "INVALID_MESSAGE_ID".to_string(),
                description: "message_id must be 1..=128 printable ASCII chars".to_string(),
            });
        }
        if !validate_source_epoch(&msg.source_epoch) {
            details.push(ErrorDetail {
                field: "source_epoch".to_string(),
                issue: "INVALID_SOURCE_EPOCH".to_string(),
                description: "source_epoch must be 1..=64 valid ASCII chars".to_string(),
            });
        }
        if msg.points.is_empty() {
            details.push(ErrorDetail {
                field: "points".to_string(),
                issue: "EMPTY_POINTS".to_string(),
                description: "points array must not be empty".to_string(),
            });
        }
        if !details.is_empty() {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Telemetry envelope validation failed",
                details,
            ));
        }

        // 4. Device, Model & Binding Revision alignment check (T08: 409 REVISION_CONFLICT)
        let device = self.devices.get(&msg.device_id).ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Device '{}' not found", msg.device_id),
                vec![],
            )
        })?;

        let binding = self.bindings.get(&msg.binding_revision_id).ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::RevisionConflict,
                format!(
                    "Binding revision '{}' is unknown or not aligned with device '{}'",
                    msg.binding_revision_id, msg.device_id
                ),
                vec![ErrorDetail {
                    field: "binding_revision_id".to_string(),
                    issue: "UNKNOWN_BINDING_REVISION".to_string(),
                    description: format!(
                        "Active binding revision is '{}'",
                        device.active_binding_revision_id
                    ),
                }],
            )
        })?;

        if binding.device_id != device.device_id || binding.model_revision_id != msg.model_revision_id
        {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::RevisionConflict,
                "Reported model_revision_id or binding_revision_id conflicts with binding snapshot",
                vec![],
            ));
        }

        // R08: Check if this message_key was already accepted before enforcing current active binding window,
        // so retries of previously accepted messages after a binding rotation still return 200 duplicate.
        let message_key = (
            ctx.tenant_id.clone(),
            msg.source_id.clone(),
            msg.source_epoch.clone(),
            msg.message_id.clone(),
        );
        let is_known_message_key = self.applied_messages.contains_key(&message_key);

        // If this is a NEW message_key and binding is not the currently active binding, verify whether all points
        // fall within a proven historical validity window `[valid_from_ms, valid_to_ms)`.
        if !is_known_message_key
            && (msg.binding_revision_id != device.active_binding_revision_id
                || msg.model_revision_id != device.model_revision_id)
        {
            let window_proven = match (binding.valid_from_ms, binding.valid_to_ms) {
                (Some(from_ms), Some(to_ms)) => msg
                    .points
                    .iter()
                    .all(|p| p.source_at_ms >= from_ms && p.source_at_ms < to_ms),
                _ => false,
            };
            if !window_proven {
                return Err(ErrorEnvelope::new(
                    request_id,
                    ErrorCode::RevisionConflict,
                    format!(
                        "Binding revision '{}' is superseded by active '{}' and sampling window cannot be proven",
                        msg.binding_revision_id, device.active_binding_revision_id
                    ),
                    vec![ErrorDetail {
                        field: "binding_revision_id".to_string(),
                        issue: "UNPROVEN_HISTORICAL_BINDING_WINDOW".to_string(),
                        description: format!(
                            "Active binding is '{}', active model is '{}'",
                            device.active_binding_revision_id, device.model_revision_id
                        ),
                    }],
                ));
            }
        }

        let model = self.models.get(&msg.model_revision_id).ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::RevisionConflict,
                format!("Model revision '{}' not found", msg.model_revision_id),
                vec![],
            )
        })?;

        // 5. Point-by-point atomic validation (T02, SC07, SC09)
        let mut seen_properties = BTreeSet::new();
        let mut canonical_points = Vec::with_capacity(msg.points.len());
        let mut parsed_seqs = Vec::with_capacity(msg.points.len());
        let mut clock_skew_details = Vec::new();

        for (idx, pt) in msg.points.iter().enumerate() {
            if !seen_properties.insert(pt.property.clone()) {
                details.push(ErrorDetail {
                    field: format!("points[{}].property", idx),
                    issue: "DUPLICATE_PROPERTY".to_string(),
                    description: format!(
                        "Property '{}' appears more than once in the same message",
                        pt.property
                    ),
                });
                continue;
            }

            let Some(prop_spec) = model.properties.get(&pt.property) else {
                details.push(ErrorDetail {
                    field: format!("points[{}].property", idx),
                    issue: "UNKNOWN_PROPERTY".to_string(),
                    description: format!(
                        "Property '{}' is not defined in model_revision '{}'",
                        pt.property, model.model_revision_id
                    ),
                });
                continue;
            };

            if prop_spec.source_kind != PropertySourceKind::GatewayMeasurement
                || !binding.mapped_points.contains_key(&pt.property)
            {
                details.push(ErrorDetail {
                    field: format!("points[{}].property", idx),
                    issue: "UNBOUND_PROPERTY".to_string(),
                    description: format!(
                        "Property '{}' is not bound as gateway_measurement in '{}'",
                        pt.property, binding.binding_revision_id
                    ),
                });
                continue;
            }

            // SC09: value: null is strictly forbidden in telemetry messages
            if pt.value.is_null() {
                details.push(ErrorDetail {
                    field: format!("points[{}].value", idx),
                    issue: "INVALID_POINT_VALUE".to_string(),
                    description:
                        "Telemetry measurement value must not be null; use POST /v1/source-state for comm timeouts"
                            .to_string(),
                });
                continue;
            }

            // Validate value type and range
            let typed_val = match (&prop_spec.data_type, &pt.value) {
                (ValueType::Float64, Value::Number(n)) => {
                    if let Some(f) = n.as_f64() {
                        if f.is_nan() || f.is_infinite() {
                            details.push(ErrorDetail {
                                field: format!("points[{}].value", idx),
                                issue: "NON_FINITE_FLOAT".to_string(),
                                description: "float64 must be finite".to_string(),
                            });
                            continue;
                        }
                        if let Some(min_v) = prop_spec.min_value {
                            if f < min_v {
                                details.push(ErrorDetail {
                                    field: format!("points[{}].value", idx),
                                    issue: "VALUE_OUT_OF_RANGE".to_string(),
                                    description: format!("Value {} < min {}", f, min_v),
                                });
                                continue;
                            }
                        }
                        if let Some(max_v) = prop_spec.max_value {
                            if f > max_v {
                                details.push(ErrorDetail {
                                    field: format!("points[{}].value", idx),
                                    issue: "VALUE_OUT_OF_RANGE".to_string(),
                                    description: format!("Value {} > max {}", f, max_v),
                                });
                                continue;
                            }
                        }
                        TypedPointValue::Float64(if f == 0.0 { 0.0 } else { f })
                    } else {
                        details.push(ErrorDetail {
                            field: format!("points[{}].value", idx),
                            issue: "TYPE_MISMATCH".to_string(),
                            description: "Expected finite float64".to_string(),
                        });
                        continue;
                    }
                }
                (ValueType::Int64, Value::Number(n)) => {
                    if let Some(i) = n.as_i64() {
                        TypedPointValue::Int64(i)
                    } else {
                        details.push(ErrorDetail {
                            field: format!("points[{}].value", idx),
                            issue: "TYPE_MISMATCH".to_string(),
                            description: "Expected int64".to_string(),
                        });
                        continue;
                    }
                }
                (ValueType::Bool, Value::Bool(b)) => TypedPointValue::Bool(*b),
                (ValueType::Text, Value::String(s)) => TypedPointValue::Text(s.clone()),
                _ => {
                    details.push(ErrorDetail {
                        field: format!("points[{}].value", idx),
                        issue: "TYPE_MISMATCH".to_string(),
                        description: format!(
                            "Expected type '{}' for property '{}'",
                            prop_spec.data_type.as_str(),
                            pt.property
                        ),
                    });
                    continue;
                }
            };

            // Validate source_time_origin
            if !matches!(
                pt.source_time_origin.as_str(),
                "device_clock" | "protocol_timestamp" | "gateway_sample" | "external_source"
            ) {
                details.push(ErrorDetail {
                    field: format!("points[{}].source_time_origin", idx),
                    issue: "INVALID_TIME_ORIGIN".to_string(),
                    description: format!("Unknown source_time_origin '{}'", pt.source_time_origin),
                });
                continue;
            }

            // Validate source_seq as u64
            let seq_u64 = match parse_source_seq(&pt.source_seq) {
                Ok(v) => v,
                Err(e) => {
                    details.push(ErrorDetail {
                        field: format!("points[{}].source_seq", idx),
                        issue: "INVALID_SOURCE_SEQ".to_string(),
                        description: e,
                    });
                    continue;
                }
            };

            // Validate quality.state
            if !matches!(
                pt.quality.state.as_str(),
                "good" | "uncertain" | "bad" | "unknown"
            ) {
                details.push(ErrorDetail {
                    field: format!("points[{}].quality.state", idx),
                    issue: "INVALID_QUALITY_STATE".to_string(),
                    description: format!("Unknown quality.state '{}'", pt.quality.state),
                });
                continue;
            }

            // Validate source_at_ms & future clock skew (SC07)
            if pt.source_at_ms <= 0 {
                details.push(ErrorDetail {
                    field: format!("points[{}].source_at_ms", idx),
                    issue: "INVALID_TIMESTAMP".to_string(),
                    description: "source_at_ms must be positive".to_string(),
                });
                continue;
            }
            if pt.source_at_ms > received_at_ms + self.max_future_skew_ms {
                clock_skew_details.push(ErrorDetail {
                    field: format!("points[{}].source_at_ms", idx),
                    issue: "FUTURE_SKEW_EXCEEDED".to_string(),
                    description: format!(
                        "source_at_ms ({}) exceeds received_at_ms ({}) + max_future_skew_ms ({})",
                        pt.source_at_ms, received_at_ms, self.max_future_skew_ms
                    ),
                });
                continue;
            }

            parsed_seqs.push((pt.property.clone(), seq_u64, pt.delivery.replayed));
            canonical_points.push(CanonicalPointInput {
                property: pt.property.clone(),
                value_type: prop_spec.data_type,
                value: typed_val,
                source_at_ms: pt.source_at_ms,
                source_time_origin: pt.source_time_origin.clone(),
                source_seq: pt.source_seq.clone(),
                quality_state: pt.quality.state.clone(),
                quality_source_protocol: pt.quality.source_protocol.clone(),
                quality_source_code: pt.quality.source_code.clone(),
            });
        }

        // Whole-message atomic rejection if any validation error occurred (T02, SC09)
        if !details.is_empty() {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Telemetry message rejected due to point validation errors",
                details,
            ));
        }

        // Clock skew exceeded rejection (SC07: 422 CLOCK_SKEW_EXCEEDED)
        if !clock_skew_details.is_empty() {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ClockSkewExceeded,
                "Telemetry message rejected because source_at_ms exceeds max_future_skew_ms",
                clock_skew_details,
            ));
        }

        // 6. Compute D00 `sha256-jcs-v1` payload_hash and per-point deterministic `observation_id`
        let (_canonical_json, payload_hash) = compute_jcs_v1_payload_hash(
            &msg.spec,
            &ctx.tenant_id,
            &msg.source_id,
            &msg.source_epoch,
            &msg.device_id,
            &msg.model_revision_id,
            &msg.binding_revision_id,
            &canonical_points,
        )
        .map_err(|e| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                format!("JCS canonicalization failed: {}", e),
                vec![],
            )
        })?;

        let mut observation_ids = BTreeMap::new();
        for pt in &canonical_points {
            let obs_id = derive_observation_id(
                &ctx.tenant_id,
                &msg.source_id,
                &msg.source_epoch,
                &msg.message_id,
                &pt.property,
            );
            observation_ids.insert(pt.property.clone(), obs_id);
        }

        // 7. Idempotency check on `(tenant_id, source_id, source_epoch, message_id)` (T03, T04, SC04, SC05, R08)
        if let Some(existing) = self.applied_messages.get(&message_key) {
            if existing.payload_hash == payload_hash {
                return Ok(TelemetryIngestAck {
                    http_status: 200,
                    request_id: request_id.to_string(),
                    ingest_id: existing.ingest_id.clone(),
                    message_id: msg.message_id.clone(),
                    status: "duplicate".to_string(),
                    history_status: existing.history_status.clone(),
                    payload_hash,
                    observation_ids: existing.observation_ids.clone(),
                });
            } else {
                return Err(ErrorEnvelope::new(
                    request_id,
                    ErrorCode::IdempotencyConflict,
                    format!(
                        "Message key ({}, {}, {}, {}) already exists with different payload_hash",
                        ctx.tenant_id, msg.source_id, msg.source_epoch, msg.message_id
                    ),
                    vec![ErrorDetail {
                        field: "message_id".to_string(),
                        issue: "PAYLOAD_HASH_MISMATCH".to_string(),
                        description: format!(
                            "Existing hash '{}', incoming hash '{}'",
                            existing.payload_hash, payload_hash
                        ),
                    }],
                ));
            }
        }

        // 8. Check durability backpressure if client requested `Prefer: durability=durable`
        if prefer_durable && self.durability_backpressure {
            let mut env = ErrorEnvelope::new(
                request_id,
                ErrorCode::DurabilityBackpressure,
                "WAL hard watermark reached; cannot guarantee durable acceptance",
                vec![],
            );
            env.retry_after_ms = Some(1000);
            return Err(env);
        }

        // 9. Apply atomic state & history update (T01, T05, SC06)
        let ingest_id = format!("ing-{:06}", self.next_ingest_seq);
        self.next_ingest_seq += 1;

        let seq_map: BTreeMap<String, (u64, bool)> = parsed_seqs
            .into_iter()
            .map(|(k, seq, rep)| (k, (seq, rep)))
            .collect();

        for pt in &canonical_points {
            let obs_id = observation_ids.get(&pt.property).unwrap().clone();
            let (seq_u64, replayed) = *seq_map.get(&pt.property).unwrap();
            let prop_spec = model.properties.get(&pt.property).unwrap();
            let resource_id = format!(
                "res-{}-{}",
                msg.device_id,
                pt.property.replace('_', "-")
            );

            // Append to history_observations (T01: store exact engineering value without multiplying by factor again)
            self.history_observations.push(HistoryObservation {
                observation_id: obs_id,
                resource_id: resource_id.clone(),
                device_id: msg.device_id.clone(),
                property: pt.property.clone(),
                model_revision_id: msg.model_revision_id.clone(),
                binding_revision_id: msg.binding_revision_id.clone(),
                source_at_ms: pt.source_at_ms,
                received_at_ms,
                source_time_origin: pt.source_time_origin.clone(),
                source_seq: pt.source_seq.clone(),
                value_type: prop_spec.data_type.as_str().to_string(),
                value: pt.value.to_json_value(),
                unit: prop_spec.unit.clone(),
                quality: pt.quality_state.clone(),
                source_quality_code: pt.quality_source_code.clone(),
                replayed,
                record_kind: "measured".to_string(),
            });

            // Ordering rule for `latest` (D00 Section 5.2, Design Section 22.2, T05 & SC06):
            // Compare (source_at_ms, source_seq_u64) within same active source_epoch.
            let should_update_latest = match self.latest_values.get(&resource_id) {
                None => true,
                Some(curr) => {
                    if curr.source_epoch == msg.source_epoch {
                        (pt.source_at_ms, seq_u64)
                            >= (curr.value_sampled_at_ms, curr.source_seq_u64)
                    } else {
                        // Different epoch: only update if not an old replayed backfill with older timestamp
                        !replayed && pt.source_at_ms >= curr.value_sampled_at_ms
                    }
                }
            };

            if should_update_latest {
                self.latest_values.insert(
                    resource_id.clone(),
                    RuntimePointLatest {
                        value: pt.value.clone(),
                        value_sampled_at_ms: pt.source_at_ms,
                        received_at_ms,
                        source_time_origin: pt.source_time_origin.clone(),
                        source_epoch: msg.source_epoch.clone(),
                        source_seq_u64: seq_u64,
                        binding_revision_id: msg.binding_revision_id.clone(),
                        model_revision_id: msg.model_revision_id.clone(),
                    },
                );

                // Update quality if this observation is newer than or equal to current quality_observed_at_ms
                let should_update_quality = match self.point_qualities.get(&resource_id) {
                    None => true,
                    Some(q) => pt.source_at_ms >= q.quality_observed_at_ms,
                };
                if should_update_quality {
                    self.point_qualities.insert(
                        resource_id,
                        RuntimePointQuality {
                            quality_state: pt.quality_state.clone(),
                            quality_reason: pt.quality_source_code.clone(),
                            quality_observed_at_ms: pt.source_at_ms,
                        },
                    );
                }
            }
        }

        let status = if prefer_durable {
            "accepted_durable"
        } else {
            "accepted_volatile"
        };

        self.applied_messages.insert(
            message_key,
            AppliedMessageEntry {
                ingest_id: ingest_id.clone(),
                payload_hash: payload_hash.clone(),
                history_status: "committed".to_string(),
                observation_ids: observation_ids.clone(),
            },
        );

        Ok(TelemetryIngestAck {
            http_status: 202,
            request_id: request_id.to_string(),
            ingest_id,
            message_id: msg.message_id.clone(),
            status: status.to_string(),
            history_status: "pending".to_string(),
            payload_hash,
            observation_ids,
        })
    }

    // ========================================================================
    // Southbound Source State Event: POST /v1/source-state (D00 Section 6, T06, SC08, NB04)
    // ========================================================================

    pub fn report_source_state(
        &mut self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        now_ms: i64,
        evt: &SourceStateEventWire,
    ) -> Result<SourceStateAck, ErrorEnvelope> {
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, now_ms)?;
        ctx.require_action(request_id, "source_state:report")?;

        if ctx.caller_kind != CallerKind::SouthboundSource
            || ctx.bound_source_id.as_deref() != Some(evt.source_id.as_str())
        {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::SourceNotAuthorized,
                format!("Caller is not authorized as source_id '{}'", evt.source_id),
                vec![],
            ));
        }

        let granted = self
            .source_device_grants
            .get(&(ctx.tenant_id.clone(), evt.source_id.clone()))
            .map(|set| set.contains(&evt.device_id))
            .unwrap_or(false);
        if !granted {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::SourceNotAuthorized,
                format!(
                    "Source '{}' is not authorized for device '{}'",
                    evt.source_id, evt.device_id
                ),
                vec![],
            ));
        }

        if evt.spec != "iotfast.source_state.v1" {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                format!("Expected spec 'iotfast.source_state.v1', got '{}'", evt.spec),
                vec![],
            ));
        }

        let device = self.devices.get(&evt.device_id).ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Device '{}' not found", evt.device_id),
                vec![],
            )
        })?;

        if evt.binding_revision_id != device.active_binding_revision_id {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::RevisionConflict,
                "binding_revision_id does not match active device binding",
                vec![],
            ));
        }

        // R03: Independent idempotency domain `("source_state", tenant_id, source_id, source_epoch, event_id)`
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
        let event_key = (
            ctx.tenant_id.clone(),
            evt.source_id.clone(),
            evt.source_epoch.clone(),
            evt.event_id.clone(),
        );
        if let Some(existing_hash) = self.applied_source_states.get(&event_key) {
            if existing_hash == &event_hash {
                return Ok(SourceStateAck {
                    http_status: 200,
                    request_id: request_id.to_string(),
                    event_id: evt.event_id.clone(),
                    status: "duplicate".to_string(),
                });
            } else {
                return Err(ErrorEnvelope::new(
                    request_id,
                    ErrorCode::IdempotencyConflict,
                    format!(
                        "Source state event_id '{}' already exists with different payload hash",
                        evt.event_id
                    ),
                    vec![],
                ));
            }
        }
        self.applied_source_states.insert(event_key, event_hash);

        // Update connection state and point quality states WITHOUT modifying `latest_values`
        // or inserting any fake measurement into `history_observations`!
        self.device_connection_states
            .insert(evt.device_id.clone(), evt.connection_state.clone());

        for ps in &evt.point_states {
            let resource_id = format!(
                "res-{}-{}",
                evt.device_id,
                ps.property.replace('_', "-")
            );
            self.point_qualities.insert(
                resource_id,
                RuntimePointQuality {
                    quality_state: ps.quality_state.clone(),
                    quality_reason: Some(ps.quality_reason.clone()),
                    quality_observed_at_ms: evt.observed_at_ms,
                },
            );
            self.quality_events.push(QualityEventRecord {
                event_id: evt.event_id.clone(),
                device_id: evt.device_id.clone(),
                property: ps.property.clone(),
                observed_at_ms: evt.observed_at_ms,
                quality_state: ps.quality_state.clone(),
                quality_reason: ps.quality_reason.clone(),
                connection_state: evt.connection_state.clone(),
            });
        }

        Ok(SourceStateAck {
            http_status: 202,
            request_id: request_id.to_string(),
            event_id: evt.event_id.clone(),
            status: "accepted".to_string(),
        })
    }

    // ========================================================================
    // Northbound Device Business View & Shadow (NB03, NB04, SC01-SC03, T10)
    // ========================================================================

    pub fn get_device_view(
        &self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        now_ms: i64,
        device_id: &str,
    ) -> Result<DeviceBusinessView, ErrorEnvelope> {
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, now_ms)?;
        ctx.require_action(request_id, "view:read")?;

        // Uniform 404 concealment if device doesn't exist OR is outside caller's VerifiedScopeContext (SC02, T10)
        let dev = self.devices.get(device_id).ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Resource or device '{}' not found", device_id),
                vec![],
            )
        })?;

        let model = self.models.get(&dev.model_revision_id).unwrap();
        let binding = self.bindings.get(&dev.active_binding_revision_id).unwrap();
        let conn_state = self
            .device_connection_states
            .get(device_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        // R09: Filter properties by resource-level Scope (`is_resource_authorized`) so partial-device
        // callers (`resource_ids` or sub-property `uri_prefixes`) only see authorized properties.
        let mut properties = BTreeMap::new();
        for (prop_id, prop_spec) in &model.properties {
            let resource_id = format!("res-{}-{}", dev.device_id, prop_id.replace('_', "-"));
            let Some(res_entry) = self.resources.get(&resource_id) else {
                continue;
            };
            if !self.is_resource_authorized(ctx, res_entry) {
                continue;
            }
            let is_bound = prop_spec.source_kind == PropertySourceKind::GatewayMeasurement
                && binding.mapped_points.contains_key(prop_id);

            if !is_bound {
                // NB03: unbound property -> value: null, quality: "unknown", observation_state: "no_observation"
                properties.insert(
                    prop_id.clone(),
                    DevicePropertyView {
                        resource_id,
                        name: prop_spec.name.clone(),
                        data_type: prop_spec.data_type.as_str().to_string(),
                        unit: prop_spec.unit.clone(),
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
                    },
                );
                continue;
            }

            match self.latest_values.get(&resource_id) {
                None => {
                    // NB03: bound but not yet observed -> value: null, quality: "unknown", observation_state: "no_observation"
                    let (q_state, q_reason, q_at) = match self.point_qualities.get(&resource_id) {
                        Some(q) => (
                            q.quality_state.clone(),
                            q.quality_reason.clone(),
                            Some(q.quality_observed_at_ms),
                        ),
                        None => ("unknown".to_string(), None, None),
                    };
                    properties.insert(
                        prop_id.clone(),
                        DevicePropertyView {
                            resource_id,
                            name: prop_spec.name.clone(),
                            data_type: prop_spec.data_type.as_str().to_string(),
                            unit: prop_spec.unit.clone(),
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
                            binding_revision_id: Some(dev.active_binding_revision_id.clone()),
                        },
                    );
                }
                Some(latest) => {
                    let (q_state, q_reason, q_at) = match self.point_qualities.get(&resource_id) {
                        Some(q) => (
                            q.quality_state.clone(),
                            q.quality_reason.clone(),
                            q.quality_observed_at_ms,
                        ),
                        None => ("good".to_string(), None, latest.value_sampled_at_ms),
                    };
                    // R03: Gateway disconnect (`conn_state == "disconnected"`) marks freshness as "stale"
                    // without falsely mutating point `quality` to "bad".
                    let freshness = if q_state == "bad"
                        || conn_state == "disconnected"
                        || (now_ms - latest.value_sampled_at_ms) > self.stale_threshold_ms
                    {
                        "stale"
                    } else {
                        "fresh"
                    };

                    properties.insert(
                        prop_id.clone(),
                        DevicePropertyView {
                            resource_id,
                            name: prop_spec.name.clone(),
                            data_type: prop_spec.data_type.as_str().to_string(),
                            unit: prop_spec.unit.clone(),
                            binding_state: "bound".to_string(),
                            observation_state: "observed".to_string(),
                            value: latest.value.to_json_value(),
                            quality: q_state,
                            quality_reason: q_reason,
                            freshness: freshness.to_string(),
                            value_sampled_at_ms: Some(latest.value_sampled_at_ms),
                            quality_observed_at_ms: Some(q_at),
                            received_at_ms: Some(latest.received_at_ms),
                            source_time_origin: Some(latest.source_time_origin.clone()),
                            binding_revision_id: Some(latest.binding_revision_id.clone()),
                        },
                    );
                }
            }
        }

        if properties.is_empty() {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Resource or device '{}' not found", device_id),
                vec![],
            ));
        }

        Ok(DeviceBusinessView {
            request_id: request_id.to_string(),
            device_id: dev.device_id.clone(),
            device_key: dev.device_key.clone(),
            device_name: dev.device_name.clone(),
            product_id: dev.product_id.clone(),
            product_key: dev.product_key.clone(),
            model_revision_id: dev.model_revision_id.clone(),
            model_version: model.model_version.clone(),
            asset_id: dev.asset_id.clone(),
            canonical_device_uri: dev.canonical_device_uri.clone(),
            active_binding_revision_id: dev.active_binding_revision_id.clone(),
            source_connection_state: conn_state,
            view_generated_at_ms: now_ms,
            view_consistency: "per_resource_latest".to_string(),
            properties,
        })
    }

    pub fn get_device_shadow(
        &self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        now_ms: i64,
        device_id: &str,
    ) -> Result<DeviceShadow, ErrorEnvelope> {
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, now_ms)?;
        ctx.require_action(request_id, "shadow:read")?;

        let dev = self.devices.get(device_id).ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Resource or device '{}' not found", device_id),
                vec![],
            )
        })?;
        let conn_state = self
            .device_connection_states
            .get(device_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let model = self.models.get(&dev.model_revision_id).unwrap();
        let mut points = BTreeMap::new();
        for prop_id in model.properties.keys() {
            let resource_id = format!("res-{}-{}", dev.device_id, prop_id.replace('_', "-"));
            let Some(res_entry) = self.resources.get(&resource_id) else {
                continue;
            };
            if !self.is_resource_authorized(ctx, res_entry) {
                continue;
            }
            match self.latest_values.get(&resource_id) {
                None => {
                    points.insert(
                        prop_id.clone(),
                        DeviceShadowPoint {
                            resource_id,
                            value: Value::Null,
                            value_sampled_at_ms: None,
                            quality_observed_at_ms: None,
                            received_at_ms: None,
                            quality: "unknown".to_string(),
                            freshness: "unknown".to_string(),
                            model_revision_id: dev.model_revision_id.clone(),
                            history_status: "none".to_string(),
                        },
                    );
                }
                Some(latest) => {
                    let (q_state, q_at) = match self.point_qualities.get(&resource_id) {
                        Some(q) => (q.quality_state.clone(), q.quality_observed_at_ms),
                        None => ("good".to_string(), latest.value_sampled_at_ms),
                    };
                    let freshness = if q_state == "bad"
                        || conn_state == "disconnected"
                        || (now_ms - latest.value_sampled_at_ms) > self.stale_threshold_ms
                    {
                        "stale"
                    } else {
                        "fresh"
                    };
                    points.insert(
                        prop_id.clone(),
                        DeviceShadowPoint {
                            resource_id,
                            value: latest.value.to_json_value(),
                            value_sampled_at_ms: Some(latest.value_sampled_at_ms),
                            quality_observed_at_ms: Some(q_at),
                            received_at_ms: Some(latest.received_at_ms),
                            quality: q_state,
                            freshness: freshness.to_string(),
                            model_revision_id: latest.model_revision_id.clone(),
                            history_status: "committed".to_string(),
                        },
                    );
                }
            }
        }

        if points.is_empty() {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Resource or device '{}' not found", device_id),
                vec![],
            ));
        }

        Ok(DeviceShadow {
            request_id: request_id.to_string(),
            device_id: dev.device_id.clone(),
            revision: dev.revision,
            source_connection_state: conn_state,
            points,
        })
    }

    // ========================================================================
    // Catalog Browse / URI Resolution (SC01-SC03, T10)
    // ========================================================================

    pub fn resolve_resource_by_uri(
        &self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        now_ms: i64,
        uri: &str,
    ) -> Result<ResourceEntry, ErrorEnvelope> {
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, now_ms)?;
        ctx.require_action(request_id, "catalog:read")?;

        let found = self
            .resources
            .values()
            .find(|r| r.canonical_uri == uri && self.is_resource_authorized(ctx, r));

        found.cloned().ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                format!("Resource URI '{}' not found", uri),
                vec![],
            )
        })
    }

    /// Filters by `VerifiedScopeContext` FIRST, then applies keyword search and sorting by `resource_id`
    pub fn search_catalog_resources(
        &self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        now_ms: i64,
        keyword: Option<&str>,
    ) -> Result<Vec<ResourceEntry>, ErrorEnvelope> {
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, now_ms)?;
        ctx.require_action(request_id, "catalog:read")?;

        let mut results: Vec<ResourceEntry> = self
            .resources
            .values()
            .filter(|r| self.is_resource_authorized(ctx, r))
            .filter(|r| match keyword {
                None => true,
                Some(kw) => {
                    r.name.contains(kw) || r.property.contains(kw) || r.canonical_uri.contains(kw)
                }
            })
            .cloned()
            .collect();
        results.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
        Ok(results)
    }

    // ========================================================================
    // Northbound History Query (POST /v1/history/query, NB07, NB08, NB09, SC02, T10)
    // ========================================================================

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
        hasher.update(format!("\0{}:{}\0{}\0{}\0", req.range.start_ms, req.range.end_ms, req.mode, req.order).as_bytes());
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
        snapshot_boundary: usize,
        last_source_at_ms: i64,
        last_obs_id: &str,
    ) -> String {
        let payload = format!(
            "{}|{}|{}|{}|{}|{}",
            tenant_id, scope_hash, query_fp, snapshot_boundary, last_source_at_ms, last_obs_id
        );
        let mut hasher = Sha256::new();
        hasher.update(self.cursor_signing_secret.as_bytes());
        hasher.update(b"\0");
        hasher.update(payload.as_bytes());
        let sig = hex::encode(hasher.finalize());
        let full = format!("{}|{}", payload, sig);
        BASE32_NOPAD.encode(full.as_bytes()).to_ascii_lowercase()
    }

    fn verify_and_decode_cursor(
        &self,
        request_id: &str,
        cursor_str: &str,
        expected_tenant: &str,
        expected_scope_hash: &str,
        expected_query_fp: &str,
    ) -> Result<(usize, i64, String), ErrorEnvelope> {
        let decoded_bytes = BASE32_NOPAD
            .decode(cursor_str.to_ascii_uppercase().as_bytes())
            .map_err(|_| {
                ErrorEnvelope::new(
                    request_id,
                    ErrorCode::ValidationFailed,
                    "Invalid pagination cursor encoding",
                    vec![ErrorDetail {
                        field: "cursor".to_string(),
                        issue: "INVALID_CURSOR".to_string(),
                        description: "Cursor failed base32 decoding".to_string(),
                    }],
                )
            })?;
        let decoded = String::from_utf8(decoded_bytes).map_err(|_| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Invalid pagination cursor UTF-8",
                vec![],
            )
        })?;
        let parts: Vec<&str> = decoded.split('|').collect();
        if parts.len() != 7 {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Invalid pagination cursor structure",
                vec![],
            ));
        }
        let payload = parts[..6].join("|");
        let sig = parts[6];

        let mut hasher = Sha256::new();
        hasher.update(self.cursor_signing_secret.as_bytes());
        hasher.update(b"\0");
        hasher.update(payload.as_bytes());
        let expected_sig = hex::encode(hasher.finalize());

        if sig != expected_sig {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Pagination cursor signature verification failed",
                vec![ErrorDetail {
                    field: "cursor".to_string(),
                    issue: "CURSOR_SIGNATURE_INVALID".to_string(),
                    description: "Cursor signature mismatch".to_string(),
                }],
            ));
        }

        let (tenant_id, scope_hash, query_fp) = (parts[0], parts[1], parts[2]);
        if tenant_id != expected_tenant
            || scope_hash != expected_scope_hash
            || query_fp != expected_query_fp
        {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Pagination cursor scope or query binding mismatch (cross-scope replay rejected)",
                vec![ErrorDetail {
                    field: "cursor".to_string(),
                    issue: "CURSOR_SCOPE_MISMATCH".to_string(),
                    description:
                        "Cursor is bound to a different VerifiedScopeContext or query parameters"
                            .to_string(),
                }],
            ));
        }

        let snapshot_boundary = parts[3].parse::<usize>().map_err(|_| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Invalid cursor snapshot boundary",
                vec![],
            )
        })?;
        let last_source_at_ms = parts[4].parse::<i64>().map_err(|_| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Invalid cursor timestamp",
                vec![],
            )
        })?;
        let last_obs_id = parts[5].to_string();

        Ok((snapshot_boundary, last_source_at_ms, last_obs_id))
    }

    pub fn query_history(
        &self,
        request_id: &str,
        scope_ctx: Option<&VerifiedScopeContext>,
        now_ms: i64,
        req: &HistoryQueryRequest,
    ) -> Result<HistoryQueryResponse, ErrorEnvelope> {
        let ctx = scope_ctx.ok_or_else(|| {
            ErrorEnvelope::new(
                request_id,
                ErrorCode::Unauthenticated,
                "Missing verified service identity credential",
                vec![],
            )
        })?;
        ctx.validate_active(request_id, now_ms)?;
        ctx.require_action(request_id, "history:query")?;

        if req.resource_ids.is_empty() || req.range.start_ms >= req.range.end_ms || req.limit == 0 {
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ValidationFailed,
                "Invalid history query range, resource_ids, or limit",
                vec![],
            ));
        }

        // Batch resource authorization check (D00 Section 3 & SC02):
        // If ANY requested resource_id does not exist or is outside VerifiedScopeContext,
        // reject the ENTIRE request with 404 RESOURCE_NOT_FOUND!
        let mut unauthorized_or_missing = Vec::new();
        for res_id in &req.resource_ids {
            match self.resources.get(res_id) {
                Some(res) if self.is_resource_authorized(ctx, res) => {}
                _ => unauthorized_or_missing.push(res_id.clone()),
            }
        }
        if !unauthorized_or_missing.is_empty() {
            // R05: Do not echo which specific resource_ids failed in batch 404 response
            // to prevent differential probing when mixing authorized and unauthorized IDs.
            return Err(ErrorEnvelope::new(
                request_id,
                ErrorCode::ResourceNotFound,
                "One or more requested resource_ids were not found",
                vec![],
            ));
        }

        let scope_hash = ctx.scope_hash();
        let query_fp = Self::query_fingerprint(req);
        let cursor_state = if let Some(ref c) = req.cursor {
            Some(self.verify_and_decode_cursor(
                request_id,
                c,
                &ctx.tenant_id,
                &scope_hash,
                &query_fp,
            )?)
        } else {
            None
        };

        let snapshot_boundary = cursor_state
            .as_ref()
            .map(|(b, _, _)| *b)
            .unwrap_or(self.history_observations.len());

        let req_res_set: BTreeSet<&str> = req.resource_ids.iter().map(|s| s.as_str()).collect();
        let req_qual_set: BTreeSet<&str> = req.quality_states.iter().map(|s| s.as_str()).collect();

        // Filter observations within half-open interval `[start_ms, end_ms)` up to snapshot_boundary (NB07)
        let mut matched: Vec<HistoryObservation> = self.history_observations
            [..snapshot_boundary.min(self.history_observations.len())]
            .iter()
            .filter(|obs| {
                req_res_set.contains(obs.resource_id.as_str())
                    && obs.source_at_ms >= req.range.start_ms
                    && obs.source_at_ms < req.range.end_ms
                    && (req_qual_set.is_empty() || req_qual_set.contains(obs.quality.as_str()))
            })
            .cloned()
            .collect();

        // Stable ordering by `(source_at_ms, observation_id)`
        let desc = req.order.eq_ignore_ascii_case("desc");
        matched.sort_by(|a, b| {
            let cmp = a
                .source_at_ms
                .cmp(&b.source_at_ms)
                .then_with(|| a.observation_id.cmp(&b.observation_id));
            if desc {
                cmp.reverse()
            } else {
                cmp
            }
        });

        if req.mode == "aggregate" {
            // NB08: Check cross-version unit/type compatibility for each resource across the queried range
            for res_id in &req.resource_ids {
                let mut units_seen: BTreeSet<Option<String>> = BTreeSet::new();
                let mut types_seen: BTreeSet<String> = BTreeSet::new();
                let mut revisions_seen: BTreeSet<String> = BTreeSet::new();
                for obs in matched.iter().filter(|o| &o.resource_id == res_id) {
                    units_seen.insert(obs.unit.as_ref().map(|u| u.code.clone()));
                    types_seen.insert(obs.value_type.clone());
                    revisions_seen.insert(obs.model_revision_id.clone());
                }
                if units_seen.len() > 1 || types_seen.len() > 1 {
                    return Err(ErrorEnvelope::new(
                        request_id,
                        ErrorCode::CrossVersionIncompatible,
                        format!(
                            "Cannot aggregate resource '{}' across model revisions {:?} with incompatible units {:?} or types {:?}",
                            res_id, revisions_seen, units_seen, types_seen
                        ),
                        vec![ErrorDetail {
                            field: "aggregation".to_string(),
                            issue: "INCOMPATIBLE_MODEL_REVISIONS_IN_RANGE".to_string(),
                            description: format!(
                                "Resource '{}' changed unit codes {:?} across revisions {:?}",
                                res_id, units_seen, revisions_seen
                            ),
                        }],
                    ));
                }
            }

            let agg_spec = req.aggregation.as_ref().ok_or_else(|| {
                ErrorEnvelope::new(
                    request_id,
                    ErrorCode::ValidationFailed,
                    "aggregation specification is required when mode='aggregate'",
                    vec![],
                )
            })?;

            let mut aggregate_items = Vec::new();
            for res_id in &req.resource_ids {
                let mut bucket_start = req.range.start_ms;
                while bucket_start < req.range.end_ms {
                    let bucket_end = (bucket_start + agg_spec.bucket_ms).min(req.range.end_ms);
                    let vals: Vec<(&HistoryObservation, f64)> = matched
                        .iter()
                        .filter(|o| {
                            &o.resource_id == res_id
                                && o.source_at_ms >= bucket_start
                                && o.source_at_ms < bucket_end
                        })
                        .filter_map(|o| o.value.as_f64().map(|v| (o, v)))
                        .collect();

                    if !vals.is_empty() {
                        let count = vals.len();
                        let sum: f64 = vals.iter().map(|(_, v)| *v).sum();
                        let min = vals.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
                        let max = vals
                            .iter()
                            .map(|(_, v)| *v)
                            .fold(f64::NEG_INFINITY, f64::max);
                        let unit = vals[0].0.unit.clone();
                        aggregate_items.push(AggregateBucketItem {
                            resource_id: res_id.clone(),
                            bucket_start_ms: bucket_start,
                            bucket_end_ms: bucket_end,
                            unit,
                            count,
                            avg: Some(sum / (count as f64)),
                            min: Some(min),
                            max: Some(max),
                        });
                    }
                    bucket_start = bucket_end;
                }
            }

            return Ok(HistoryQueryResponse {
                request_id: request_id.to_string(),
                target_id: "in-memory-contract-verifier".to_string(),
                source_mode: "managed_ingest".to_string(),
                mode: "aggregate".to_string(),
                consistency: "strong_snapshot".to_string(),
                partial: false,
                items: vec![],
                aggregate_items,
                next_cursor: None,
            });
        }

        // Apply cursor seek on `(source_at_ms, observation_id)`
        let filtered: Vec<HistoryObservation> = if let Some((_, last_ts, ref last_id)) = cursor_state
        {
            matched
                .into_iter()
                .filter(|obs| {
                    let cmp = obs
                        .source_at_ms
                        .cmp(&last_ts)
                        .then_with(|| obs.observation_id.as_str().cmp(last_id.as_str()));
                    if desc {
                        cmp.is_lt()
                    } else {
                        cmp.is_gt()
                    }
                })
                .collect()
        } else {
            matched
        };

        let has_more = filtered.len() > req.limit;
        let page_items: Vec<HistoryObservation> = filtered.into_iter().take(req.limit).collect();
        let next_cursor = if has_more {
            let last = page_items.last().unwrap();
            Some(self.encode_cursor(
                &ctx.tenant_id,
                &scope_hash,
                &query_fp,
                snapshot_boundary,
                last.source_at_ms,
                &last.observation_id,
            ))
        } else {
            None
        };

        Ok(HistoryQueryResponse {
            request_id: request_id.to_string(),
            target_id: "in-memory-contract-verifier".to_string(),
            source_mode: "managed_ingest".to_string(),
            mode: "raw".to_string(),
            consistency: "strong_snapshot".to_string(),
            partial: false,
            items: page_items,
            aggregate_items: vec![],
            next_cursor,
        })
    }
}

// ============================================================================
// 7. Golden Test Vectors Structure (for Rust + Python Cross-Verification)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenPointVectorInput {
    pub property: String,
    pub value_type: ValueType,
    pub value: Value,
    pub source_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub quality_state: String,
    pub quality_source_protocol: Option<String>,
    pub quality_source_code: Option<String>,
    pub delivery_replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenVectorCase {
    pub vector_id: String,
    pub description: String,
    pub spec: String,
    pub tenant_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub message_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub request_id_header: String,
    pub points: Vec<GoldenPointVectorInput>,
    pub expected_canonical_jcs: String,
    pub expected_payload_hash: String,
    pub expected_observation_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenVectorSuite {
    pub spec: String,
    pub algorithm: String,
    pub observation_id_prefix: String,
    pub generated_by: String,
    pub vectors: Vec<GoldenVectorCase>,
}

pub fn build_golden_vector_suite() -> GoldenVectorSuite {
    struct RawCase {
        vector_id: &'static str,
        description: &'static str,
        tenant_id: &'static str,
        source_id: &'static str,
        source_epoch: &'static str,
        message_id: &'static str,
        device_id: &'static str,
        model_revision_id: &'static str,
        binding_revision_id: &'static str,
        request_id_header: &'static str,
        points: Vec<(
            &'static str,
            ValueType,
            TypedPointValue,
            i64,
            &'static str,
            &'static str,
            &'static str,
            Option<&'static str>,
            Option<&'static str>,
            bool,
        )>,
    }

    let raw_cases = vec![
        RawCase {
            vector_id: "GV-01-BASELINE-T01",
            description: "Single float64 engineering value 245.6 (T01 baseline, replayed=false)",
            tenant_id: "ten-8iic",
            source_id: "gateway-01",
            source_epoch: "epoch-001",
            message_id: "m-20261105-000001",
            device_id: "dev-oven-02",
            model_revision_id: "mr-oven-1",
            binding_revision_id: "br-oven-12",
            request_id_header: "req-001",
            points: vec![(
                "zone1_temp",
                ValueType::Float64,
                TypedPointValue::Float64(245.6),
                1793869200120,
                "gateway_sample",
                "1001",
                "good",
                Some("modbus"),
                None,
                false,
            )],
        },
        RawCase {
            vector_id: "GV-02-REPLAY-INVARIANCE-SC04",
            description: "Same message as GV-01 but delivery.replayed=true and different X-Request-Id; must yield identical canonical_jcs, payload_hash, and observation_id",
            tenant_id: "ten-8iic",
            source_id: "gateway-01",
            source_epoch: "epoch-001",
            message_id: "m-20261105-000001",
            device_id: "dev-oven-02",
            model_revision_id: "mr-oven-1",
            binding_revision_id: "br-oven-12",
            request_id_header: "req-retry-999",
            points: vec![(
                "zone1_temp",
                ValueType::Float64,
                TypedPointValue::Float64(245.6),
                1793869200120,
                "gateway_sample",
                "1001",
                "good",
                Some("modbus"),
                None,
                true,
            )],
        },
        RawCase {
            vector_id: "GV-03-TAMPERED-VALUE-SC05",
            description: "Same message_key as GV-01 but temperature altered to 245.7; must yield identical observation_id but different payload_hash (409 IDEMPOTENCY_CONFLICT)",
            tenant_id: "ten-8iic",
            source_id: "gateway-01",
            source_epoch: "epoch-001",
            message_id: "m-20261105-000001",
            device_id: "dev-oven-02",
            model_revision_id: "mr-oven-1",
            binding_revision_id: "br-oven-12",
            request_id_header: "req-003",
            points: vec![(
                "zone1_temp",
                ValueType::Float64,
                TypedPointValue::Float64(245.7),
                1793869200120,
                "gateway_sample",
                "1001",
                "good",
                Some("modbus"),
                None,
                false,
            )],
        },
        RawCase {
            vector_id: "GV-04-MULTIPOINT-ORDER-A",
            description: "Multi-point message with properties [zone2_temp, conveyor_speed_rpm, alarm_active] in unsorted input order",
            tenant_id: "ten-8iic",
            source_id: "gateway-01",
            source_epoch: "epoch-001",
            message_id: "m-20261105-000002",
            device_id: "dev-oven-02",
            model_revision_id: "mr-oven-1",
            binding_revision_id: "br-oven-12",
            request_id_header: "req-004a",
            points: vec![
                (
                    "zone2_temp",
                    ValueType::Float64,
                    TypedPointValue::Float64(250.0),
                    1793869201000,
                    "gateway_sample",
                    "1002",
                    "good",
                    Some("modbus"),
                    None,
                    false,
                ),
                (
                    "conveyor_speed_rpm",
                    ValueType::Int64,
                    TypedPointValue::Int64(1200),
                    1793869201000,
                    "gateway_sample",
                    "1002",
                    "good",
                    Some("modbus"),
                    None,
                    false,
                ),
                (
                    "alarm_active",
                    ValueType::Bool,
                    TypedPointValue::Bool(false),
                    1793869201000,
                    "gateway_sample",
                    "1002",
                    "good",
                    Some("modbus"),
                    None,
                    false,
                ),
            ],
        },
        RawCase {
            vector_id: "GV-05-MULTIPOINT-ORDER-B",
            description: "Same multi-point message as GV-04 with input points array reversed and replayed=true; must produce identical canonical_jcs and payload_hash",
            tenant_id: "ten-8iic",
            source_id: "gateway-01",
            source_epoch: "epoch-001",
            message_id: "m-20261105-000002",
            device_id: "dev-oven-02",
            model_revision_id: "mr-oven-1",
            binding_revision_id: "br-oven-12",
            request_id_header: "req-004b",
            points: vec![
                (
                    "alarm_active",
                    ValueType::Bool,
                    TypedPointValue::Bool(false),
                    1793869201000,
                    "gateway_sample",
                    "1002",
                    "good",
                    Some("modbus"),
                    None,
                    true,
                ),
                (
                    "conveyor_speed_rpm",
                    ValueType::Int64,
                    TypedPointValue::Int64(1200),
                    1793869201000,
                    "gateway_sample",
                    "1002",
                    "good",
                    Some("modbus"),
                    None,
                    true,
                ),
                (
                    "zone2_temp",
                    ValueType::Float64,
                    TypedPointValue::Float64(250.0),
                    1793869201000,
                    "gateway_sample",
                    "1002",
                    "good",
                    Some("modbus"),
                    None,
                    true,
                ),
            ],
        },
        RawCase {
            vector_id: "GV-06-JCS-FLOAT-AND-STRING-EDGES",
            description: "RFC 8785 JCS edge cases: -0.0 normalized to 0, 0.42 fractional, 1e-6 decimal vs 1e-7 exponential, and escaped string text",
            tenant_id: "ten-8iic",
            source_id: "gateway-02",
            source_epoch: "epoch-2026.11:01",
            message_id: "m-edge-0001",
            device_id: "pump-001",
            model_revision_id: "pump-model-r1",
            binding_revision_id: "pump-001-bind-r1",
            request_id_header: "req-edge-01",
            points: vec![
                (
                    "zero_drift",
                    ValueType::Float64,
                    TypedPointValue::Float64(-0.0),
                    1793869300000,
                    "device_clock",
                    "18446744073709551615",
                    "good",
                    Some("opcua"),
                    Some("0x00000000"),
                    false,
                ),
                (
                    "outlet_pressure",
                    ValueType::Float64,
                    TypedPointValue::Float64(0.42),
                    1793869300000,
                    "device_clock",
                    "18446744073709551615",
                    "uncertain",
                    Some("opcua"),
                    Some("SubNormal"),
                    false,
                ),
                (
                    "micro_vibration",
                    ValueType::Float64,
                    TypedPointValue::Float64(1e-7),
                    1793869300000,
                    "device_clock",
                    "18446744073709551615",
                    "good",
                    None,
                    None,
                    false,
                ),
                (
                    "batch_label",
                    ValueType::Text,
                    TypedPointValue::Text("line-\"A\"\\shift\n01".to_string()),
                    1793869300000,
                    "device_clock",
                    "18446744073709551615",
                    "good",
                    None,
                    None,
                    false,
                ),
            ],
        },
        RawCase {
            vector_id: "GV-07-PR2-PROBE-SAMPLE",
            description: "PR #2 blueprint §4.4 probe sample (ten-8iic / gateway-01 / epoch-001 / msg-001 / outlet_pressure=0.42)",
            tenant_id: "ten-8iic",
            source_id: "gateway-01",
            source_epoch: "epoch-001",
            message_id: "msg-001",
            device_id: "pump-001",
            model_revision_id: "pump-model-r1",
            binding_revision_id: "pump-bind-r1",
            request_id_header: "req-pr2-probe",
            points: vec![(
                "outlet_pressure",
                ValueType::Float64,
                TypedPointValue::Float64(0.42),
                1700000000000,
                "gateway_sample",
                "42",
                "good",
                None,
                None,
                true,
            )],
        },
    ];

    let mut vectors = Vec::with_capacity(raw_cases.len());
    for rc in raw_cases {
        let mut canon_pts = Vec::new();
        let mut wire_pts = Vec::new();
        let mut obs_ids = BTreeMap::new();

        for (prop, vtype, tval, ts, origin, seq, qstate, qproto, qcode, replayed) in rc.points {
            let obs_id = derive_observation_id(
                rc.tenant_id,
                rc.source_id,
                rc.source_epoch,
                rc.message_id,
                prop,
            );
            obs_ids.insert(prop.to_string(), obs_id);

            canon_pts.push(CanonicalPointInput {
                property: prop.to_string(),
                value_type: vtype,
                value: tval.clone(),
                source_at_ms: ts,
                source_time_origin: origin.to_string(),
                source_seq: seq.to_string(),
                quality_state: qstate.to_string(),
                quality_source_protocol: qproto.map(|s| s.to_string()),
                quality_source_code: qcode.map(|s| s.to_string()),
            });

            wire_pts.push(GoldenPointVectorInput {
                property: prop.to_string(),
                value_type: vtype,
                value: tval.to_json_value(),
                source_at_ms: ts,
                source_time_origin: origin.to_string(),
                source_seq: seq.to_string(),
                quality_state: qstate.to_string(),
                quality_source_protocol: qproto.map(|s| s.to_string()),
                quality_source_code: qcode.map(|s| s.to_string()),
                delivery_replayed: replayed,
            });
        }

        let (canonical_jcs, payload_hash) = compute_jcs_v1_payload_hash(
            "iotfast.telemetry.v1",
            rc.tenant_id,
            rc.source_id,
            rc.source_epoch,
            rc.device_id,
            rc.model_revision_id,
            rc.binding_revision_id,
            &canon_pts,
        )
        .expect("golden vector canonicalization must succeed");

        vectors.push(GoldenVectorCase {
            vector_id: rc.vector_id.to_string(),
            description: rc.description.to_string(),
            spec: "iotfast.telemetry.v1".to_string(),
            tenant_id: rc.tenant_id.to_string(),
            source_id: rc.source_id.to_string(),
            source_epoch: rc.source_epoch.to_string(),
            message_id: rc.message_id.to_string(),
            device_id: rc.device_id.to_string(),
            model_revision_id: rc.model_revision_id.to_string(),
            binding_revision_id: rc.binding_revision_id.to_string(),
            request_id_header: rc.request_id_header.to_string(),
            points: wire_pts,
            expected_canonical_jcs: canonical_jcs,
            expected_payload_hash: payload_hash,
            expected_observation_ids: obs_ids,
        });
    }

    GoldenVectorSuite {
        spec: "iotfast.d00.golden_vectors.v1".to_string(),
        algorithm: "sha256-jcs-v1".to_string(),
        observation_id_prefix: "iotfast.obs.v1\\0".to_string(),
        generated_by: "f1_d00_contract (Rust)".to_string(),
        vectors,
    }
}
