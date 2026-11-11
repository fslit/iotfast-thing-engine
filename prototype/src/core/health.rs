use crate::catalog::sqlite_store::SqliteBusinessStore;
use crate::config::EngineConfig;
use crate::error::{EngineError, ErrorCode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineState {
    Running,
    HaltedOnWalCorruption,
}

#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub state: EngineState,
    pub report: HealthReport,
}

impl HealthStatus {
    pub fn http_status(&self) -> u16 {
        match self.state {
            EngineState::Running => 200,
            EngineState::HaltedOnWalCorruption => 503,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HealthReport {
    pub healthy: bool,
    pub duckdb_version: String,
    pub duckdb_source_id: String,
    pub sqlite_version: String,
    pub sqlite_business_db_writable: bool,
    pub data_dir_writable: bool,
    pub wal_dir_writable: bool,
}

pub fn check_engine_health(
    conn: &duckdb::Connection,
    config: &EngineConfig,
) -> Result<HealthReport, EngineError> {
    let (ver, source_id): (String, String) = conn
        .query_row("SELECT version(), 'unknown'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(|e| {
            EngineError::new(
                "req-health",
                ErrorCode::InternalError,
                format!("DuckDB version check failed: {}", e),
                vec![],
            )
        })?;
    let data_ok = config.data_dir.exists() || std::fs::create_dir_all(&config.data_dir).is_ok();
    let wal_ok = config.wal_dir.exists() || std::fs::create_dir_all(&config.wal_dir).is_ok();
    let sqlite_path = config.sqlite_business_db_path();
    let sqlite_ok = sqlite_path.exists()
        || sqlite_path
            .parent()
            .map(|p| p.exists() || std::fs::create_dir_all(p).is_ok())
            .unwrap_or(false);
    let sqlite_ver = SqliteBusinessStore::sqlite_version();
    Ok(HealthReport {
        healthy: ver.starts_with("v2.") && data_ok && wal_ok && sqlite_ok,
        duckdb_version: ver,
        duckdb_source_id: source_id,
        sqlite_version: sqlite_ver,
        sqlite_business_db_writable: sqlite_ok,
        data_dir_writable: data_ok,
        wal_dir_writable: wal_ok,
    })
}
