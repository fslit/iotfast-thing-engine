use crate::error::{EngineError, ErrorCode, FieldIssue};
use data_encoding::BASE32_NOPAD;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Bool,
    Int64,
    Float64,
    Text,
}

impl ValueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValueType::Bool => "bool",
            ValueType::Int64 => "int64",
            ValueType::Float64 => "float64",
            ValueType::Text => "text",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "bool" => Some(ValueType::Bool),
            "int64" => Some(ValueType::Int64),
            "float64" => Some(ValueType::Float64),
            "text" => Some(ValueType::Text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypedPointValue {
    Bool(bool),
    Int64(i64),
    Float64(f64),
    Text(String),
}

impl TypedPointValue {
    pub fn to_json_value(&self) -> Value {
        match self {
            TypedPointValue::Bool(b) => json!(*b),
            TypedPointValue::Int64(i) => json!(*i),
            TypedPointValue::Float64(f) => json!(if *f == 0.0 { 0.0 } else { *f }),
            TypedPointValue::Text(s) => json!(s),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WalPosition {
    pub segment_no: u64,
    pub record_offset: u64,
}

impl WalPosition {
    pub fn encode(&self) -> String {
        format!("{:016x}:{:016x}", self.segment_no, self.record_offset)
    }

    pub fn parse(s: &str) -> Result<Self, EngineError> {
        if s.len() != 33 || &s[16..17] != ":" {
            return Err(EngineError::new(
                "req-wal-pos",
                ErrorCode::ValidationFailed,
                format!("Invalid wal_position format: '{}'", s),
                vec![],
            ));
        }
        let seg_str = &s[0..16];
        let off_str = &s[17..33];
        if !seg_str
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !off_str
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(EngineError::new(
                "req-wal-pos",
                ErrorCode::ValidationFailed,
                format!("wal_position must be lowercase hex: '{}'", s),
                vec![],
            ));
        }
        let segment_no = u64::from_str_radix(seg_str, 16).map_err(|e| {
            EngineError::new("req-wal-pos", ErrorCode::ValidationFailed, e.to_string(), vec![])
        })?;
        let record_offset = u64::from_str_radix(off_str, 16).map_err(|e| {
            EngineError::new("req-wal-pos", ErrorCode::ValidationFailed, e.to_string(), vec![])
        })?;
        Ok(Self {
            segment_no,
            record_offset,
        })
    }
}

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

pub fn validate_message_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=128).contains(&bytes.len()) && bytes.iter().all(|&b| (0x21..=0x7E).contains(&b))
}

pub fn validate_source_epoch(epoch: &str) -> bool {
    let bytes = epoch.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes.iter().all(|&b| {
            b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'-'
        })
}

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

/// D00 deterministic `observation_id` (R01: no random UUID fallback):
/// `"obs-" + Base32LowerNoPad(SHA256("iotfast.obs.v1\0" + tenant_id + "\0" + source_id + "\0" + source_epoch + "\0" + message_id + "\0" + property)[0..16])`
pub fn derive_observation_id(
    tenant_id: &str,
    source_id: &str,
    source_epoch: &str,
    message_id: &str,
    property: &str,
) -> Result<String, EngineError> {
    for (name, val) in [
        ("tenant_id", tenant_id),
        ("source_id", source_id),
        ("source_epoch", source_epoch),
        ("message_id", message_id),
        ("property", property),
    ] {
        if val.is_empty() || val.as_bytes().contains(&0) {
            return Err(EngineError::new(
                "req-obs-id",
                ErrorCode::ValidationFailed,
                format!("Field '{}' must not be empty or contain NUL", name),
                vec![FieldIssue {
                    field: name.to_string(),
                    issue: "INVALID_IDENTIFIER".to_string(),
                    description: "Must be non-empty and free of NUL bytes".to_string(),
                }],
            ));
        }
    }
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
    Ok(format!("obs-{}", encoded))
}

pub fn format_jcs_f64(value: f64) -> Result<String, String> {
    if value.is_nan() || value.is_infinite() {
        return Err("float64 value must be finite (NaN/Infinity forbidden)".to_string());
    }
    if value == 0.0 {
        return Ok("0".to_string());
    }

    let negative = value.is_sign_negative();
    let abs_val = value.abs();

    let mut buffer = ryu::Buffer::new();
    let ryu_str = buffer.format_finite(abs_val);

    let (mantissa_str, exp_part) = if let Some(idx) = ryu_str.find(['e', 'E']) {
        let (m, e) = ryu_str.split_at(idx);
        let exp: i32 = e[1..].parse().map_err(|_| "invalid ryu exponent")?;
        (m, exp)
    } else {
        (ryu_str, 0)
    };

    let mut raw_digits = String::new();
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

    let trimmed_leading = raw_digits.trim_start_matches('0');
    let leading_zeros = (raw_digits.len() - trimmed_leading.len()) as i32;
    let digits = trimmed_leading.trim_end_matches('0');
    let k = digits.len() as i32;
    let int_digits_in_mantissa = (raw_digits.len() as i32) - frac_len;
    let n_exp = int_digits_in_mantissa - leading_zeros + exp_part;

    let mut out = String::new();
    if negative {
        out.push('-');
    }

    if k <= n_exp && n_exp <= 21 {
        out.push_str(digits);
        for _ in 0..(n_exp - k) {
            out.push('0');
        }
    } else if 0 < n_exp && n_exp < k && n_exp <= 21 {
        let split_idx = n_exp as usize;
        out.push_str(&digits[..split_idx]);
        out.push('.');
        out.push_str(&digits[split_idx..]);
    } else if -5 <= n_exp && n_exp <= 0 {
        out.push_str("0.");
        for _ in 0..(-n_exp) {
            out.push('0');
        }
        out.push_str(digits);
    } else {
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

pub fn format_jcs_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        let code = ch as u32;
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _c if code < 0x20 => out.push_str(&format!("\\u{:04x}", code)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn format_jcs_opt_str(opt: &Option<String>) -> String {
    match opt {
        Some(s) => format_jcs_string(s),
        None => "null".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryQuality {
    pub state: String,
    #[serde(default)]
    pub source_protocol: Option<String>,
    #[serde(default)]
    pub source_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryDelivery {
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryPoint {
    pub property: String,
    pub value: Value,
    pub source_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub quality: TelemetryQuality,
    pub delivery: TelemetryDelivery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryEnvelope {
    pub spec: String,
    pub message_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub points: Vec<TelemetryPoint>,
}

/// Computes `sha256-jcs-v1` canonical payload hash (R02: sort `points` by `property` UTF-8 bytes first,
/// exclude `delivery.replayed`, `X-Request-Id`, `Prefer`, `received_at_ms`).
pub fn compute_telemetry_payload_hash(
    tenant_id: &str,
    msg: &TelemetryEnvelope,
    item_types: &BTreeMap<String, ValueType>,
) -> Result<String, EngineError> {
    let mut sorted_pts = msg.points.clone();
    sorted_pts.sort_by(|a, b| a.property.as_bytes().cmp(b.property.as_bytes()));

    let mut pts_jcs = Vec::with_capacity(sorted_pts.len());
    for pt in &sorted_pts {
        let vtype = item_types.get(&pt.property).ok_or_else(|| {
            EngineError::new(
                "req-jcs",
                ErrorCode::ValidationFailed,
                format!("Missing ValueType for property '{}'", pt.property),
                vec![],
            )
        })?;

        let val_jcs = match (vtype, &pt.value) {
            (ValueType::Bool, Value::Bool(b)) => {
                if *b { "true".to_string() } else { "false".to_string() }
            }
            (ValueType::Int64, Value::Number(n)) => {
                let i = n.as_i64().ok_or_else(|| {
                    EngineError::new("req-jcs", ErrorCode::ValidationFailed, "Expected int64", vec![])
                })?;
                i.to_string()
            }
            (ValueType::Float64, Value::Number(n)) => {
                let f = n.as_f64().ok_or_else(|| {
                    EngineError::new("req-jcs", ErrorCode::ValidationFailed, "Expected float64", vec![])
                })?;
                format_jcs_f64(f).map_err(|e| {
                    EngineError::new("req-jcs", ErrorCode::ValidationFailed, e, vec![])
                })?
            }
            (ValueType::Text, Value::String(s)) => format_jcs_string(s),
            _ => {
                return Err(EngineError::new(
                    "req-jcs",
                    ErrorCode::ValidationFailed,
                    format!("Type mismatch for property '{}'", pt.property),
                    vec![],
                ))
            }
        };

        let pt_str = format!(
            "{{\"property\":{},\"quality\":{{\"source_code\":{},\"source_protocol\":{},\"state\":{}}},\"source_at_ms\":{},\"source_seq\":{},\"source_time_origin\":{},\"value\":{},\"value_type\":{}}}",
            format_jcs_string(&pt.property),
            format_jcs_opt_str(&pt.quality.source_code),
            format_jcs_opt_str(&pt.quality.source_protocol),
            format_jcs_string(&pt.quality.state),
            pt.source_at_ms,
            format_jcs_string(&pt.source_seq),
            format_jcs_string(&pt.source_time_origin),
            val_jcs,
            format_jcs_string(vtype.as_str())
        );
        pts_jcs.push(pt_str);
    }

    let canonical_json = format!(
        "{{\"binding_revision_id\":{},\"device_id\":{},\"model_revision_id\":{},\"points\":[{}],\"source_epoch\":{},\"source_id\":{},\"spec\":{},\"tenant_id\":{}}}",
        format_jcs_string(&msg.binding_revision_id),
        format_jcs_string(&msg.device_id),
        format_jcs_string(&msg.model_revision_id),
        pts_jcs.join(","),
        format_jcs_string(&msg.source_epoch),
        format_jcs_string(&msg.source_id),
        format_jcs_string(&msg.spec),
        format_jcs_string(tenant_id)
    );

    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}
