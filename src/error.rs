use serde::{Deserialize, Serialize};
use std::fmt;

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
    InternalError,
    ServiceUnavailable,
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
            ErrorCode::InternalError => 500,
            ErrorCode::ServiceUnavailable => 503,
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
pub struct FieldIssue {
    pub field: String,
    pub issue: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    pub request_id: String,
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub details: Vec<FieldIssue>,
}

impl EngineError {
    pub fn new(
        request_id: impl Into<String>,
        code: ErrorCode,
        message: impl Into<String>,
        details: Vec<FieldIssue>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            code,
            message: message.into(),
            retryable: code.retryable(),
            retry_after_ms: None,
            details,
        }
    }

    pub fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    pub fn http_status(&self) -> u16 {
        self.code.http_status()
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{:?} {}] {}: {}",
            self.code,
            self.http_status(),
            self.request_id,
            self.message
        )
    }
}

impl std::error::Error for EngineError {}

impl From<duckdb::Error> for EngineError {
    fn from(err: duckdb::Error) -> Self {
        EngineError::new(
            "req-db",
            ErrorCode::InternalError,
            format!("DuckDB error: {}", err),
            vec![],
        )
    }
}
