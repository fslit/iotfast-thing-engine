use crate::core::error::{EngineError, ErrorCode, FieldIssue};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub data_dir: PathBuf,
    pub wal_dir: PathBuf,
    pub max_future_skew_ms: i64,
    pub stale_threshold_ms: i64,
    pub wal_soft_watermark_bytes: u64,
    pub wal_hard_watermark_bytes: u64,
    pub redirect_retention_ms: i64,
    pub cursor_signing_secret: String,
}

impl EngineConfig {
    pub fn for_test(base_dir: &Path) -> Self {
        let data_dir = base_dir.join("data");
        let wal_dir = base_dir.join("wal");
        let _ = fs::create_dir_all(&data_dir);
        let _ = fs::create_dir_all(&wal_dir);
        Self {
            data_dir,
            wal_dir,
            max_future_skew_ms: 60_000,
            stale_threshold_ms: 30_000,
            wal_soft_watermark_bytes: 64 * 1024,
            wal_hard_watermark_bytes: 256 * 1024,
            redirect_retention_ms: 86_400_000,
            cursor_signing_secret: "iotfast-d00-cursor-secret-v1".to_string(),
        }
    }

    pub fn for_production(base_dir: &Path) -> Self {
        let data_dir = base_dir.join("data");
        let wal_dir = base_dir.join("wal");
        let _ = fs::create_dir_all(&data_dir);
        let _ = fs::create_dir_all(&wal_dir);
        let wal_soft_watermark_bytes = std::env::var("IOTFAST_WAL_SOFT_WATERMARK_BYTES")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(64 * 1024 * 1024);
        let wal_hard_watermark_bytes = std::env::var("IOTFAST_WAL_HARD_WATERMARK_BYTES")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(256 * 1024 * 1024);
        let stale_threshold_ms = std::env::var("IOTFAST_STALE_THRESHOLD_MS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(30_000);
        let max_future_skew_ms = std::env::var("IOTFAST_MAX_FUTURE_SKEW_MS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(60_000);
        let cursor_signing_secret = std::env::var("IOTFAST_CURSOR_SECRET")
            .unwrap_or_else(|_| "iotfast-d00-cursor-secret-v1".to_string());
        Self {
            data_dir,
            wal_dir,
            max_future_skew_ms,
            stale_threshold_ms,
            wal_soft_watermark_bytes,
            wal_hard_watermark_bytes,
            redirect_retention_ms: 86_400_000,
            cursor_signing_secret,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("engine.duckdb")
    }

    pub fn sqlite_business_db_path(&self) -> PathBuf {
        self.data_dir.join("business_metadata.sqlite")
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        let mut issues = Vec::new();
        if self.max_future_skew_ms <= 0 {
            issues.push(FieldIssue {
                field: "max_future_skew_ms".into(),
                issue: "INVALID_CONFIG_VALUE".into(),
                description: "max_future_skew_ms must be > 0".into(),
            });
        }
        if self.stale_threshold_ms <= 0 {
            issues.push(FieldIssue {
                field: "stale_threshold_ms".into(),
                issue: "INVALID_CONFIG_VALUE".into(),
                description: "stale_threshold_ms must be > 0".into(),
            });
        }
        if self.redirect_retention_ms <= 0 {
            issues.push(FieldIssue {
                field: "redirect_retention_ms".into(),
                issue: "INVALID_CONFIG_VALUE".into(),
                description: "redirect_retention_ms must be > 0".into(),
            });
        }
        if self.wal_soft_watermark_bytes == 0
            || self.wal_soft_watermark_bytes > self.wal_hard_watermark_bytes
        {
            issues.push(FieldIssue {
                field: "wal_soft_watermark_bytes".into(),
                issue: "INVALID_WATERMARK_ORDER".into(),
                description: "wal_soft_watermark_bytes must be > 0 and <= wal_hard_watermark_bytes"
                    .into(),
            });
        }
        if self.cursor_signing_secret.trim().is_empty() {
            issues.push(FieldIssue {
                field: "cursor_signing_secret".into(),
                issue: "EMPTY_SECRET".into(),
                description: "cursor_signing_secret must not be empty".into(),
            });
        }
        if !issues.is_empty() {
            return Err(EngineError::new(
                "req-config-validate",
                ErrorCode::ValidationFailed,
                "EngineConfig validation failed (Q10)",
                issues,
            ));
        }
        Ok(())
    }
}
