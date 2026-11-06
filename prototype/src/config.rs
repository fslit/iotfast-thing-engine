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

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("engine.duckdb")
    }
}
