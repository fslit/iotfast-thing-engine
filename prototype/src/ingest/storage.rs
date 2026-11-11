use crate::catalog::sqlite_store::SqliteBusinessStore;
use crate::config::EngineConfig;
use crate::d00::{TelemetryEnvelope, WalPosition};
use crate::error::{EngineError, ErrorCode, FieldIssue};
use crate::health::{check_engine_health, EngineState, HealthStatus};
use crate::ingest::ts_adapter::{
    capability_snapshot_for_kind, CapabilityLevel, DeliveryCheckpointSummary, DeliveryHoleRecord,
    DuckDbTimeSeriesAdapter, ExtensibleSqlTimeSeriesAdapter, ReconcileDeliveryHoleRequest,
    StorageArchitectureStatus, StorageCapabilitiesResponse, TdengineConfig,
    TdengineTimeSeriesAdapter, TimeSeriesBackendKind, TimeSeriesQueryPlan,
    TimeSeriesStorageAdapter,
};
use crate::ingest::{IngestService, SourceStateEnvelope};
use crate::schema::init_core_schema;
use crate::scope::VerifiedScopeContext;
use crate::wal::{AppWal, WalPointRecord, WalRecord, WalRecoveryOutcome};
use duckdb::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DurabilityPreference {
    Volatile,
    Durable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestReceipt {
    pub http_status: u16,
    pub ingest_id: String,
    pub message_id: String,
    pub status: String,
    pub history_status: String,
    pub payload_hash: String,
    pub wal_position: Option<String>,
    pub observation_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStateReceipt {
    pub http_status: u16,
    pub event_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySummary {
    pub replayed_messages: usize,
    pub latest_checkpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFileEntry {
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub backup_id: String,
    pub created_at_ms: i64,
    pub stream_epoch: String,
    pub wal_checkpoint: Option<String>,
    pub sqlite_table_counts: BTreeMap<String, usize>,
    pub duckdb_timeseries_counts: BTreeMap<String, usize>,
    pub files: Vec<BackupFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupVerifyReport {
    pub backup_id: String,
    pub verified: bool,
    pub checked_files: usize,
    pub restored_sqlite_tables: usize,
    pub restored_devices: usize,
    pub restored_history_observations: usize,
    pub restored_latest_values: usize,
    pub wal_replayed_messages: usize,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBackupRequest {
    pub backup_id: Option<String>,
}

static NEXT_EPOCH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(unix)]
extern "C" {
    fn flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}

fn acquire_instance_lock(data_dir: &std::path::Path) -> Result<std::fs::File, EngineError> {
    std::fs::create_dir_all(data_dir).map_err(|e| {
        EngineError::new(
            "req-engine-lock",
            ErrorCode::InternalError,
            format!(
                "Failed to create data directory '{}': {}",
                data_dir.display(),
                e
            ),
            vec![],
        )
    })?;
    let lock_path = data_dir.join("engine.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| {
            EngineError::new(
                "req-engine-lock",
                ErrorCode::InternalError,
                format!(
                    "Failed to open engine lock file '{}': {}",
                    lock_path.display(),
                    e
                ),
                vec![],
            )
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        const LOCK_EX: std::os::raw::c_int = 2;
        const LOCK_NB: std::os::raw::c_int = 4;
        let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc != 0 {
            return Err(EngineError::new(
                "req-engine-lock",
                ErrorCode::RevisionConflict,
                format!(
                    "Data directory '{}' is already locked by another active EngineRuntime instance",
                    data_dir.display()
                ),
                vec![],
            ));
        }
    }
    Ok(file)
}

fn validate_backup_id(raw: &str) -> Result<String, EngineError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(EngineError::new(
            "req-backup",
            ErrorCode::ValidationFailed,
            format!(
                "Invalid backup_id '{}'; must be 1..=64 alphanumeric, '-', or '_' chars",
                raw
            ),
            vec![FieldIssue {
                field: "backup_id".into(),
                issue: "INVALID_BACKUP_ID".into(),
                description: "backup_id must be 1..=64 characters of [A-Za-z0-9_-]".into(),
            }],
        ));
    }
    Ok(trimmed.to_string())
}

fn sha256_file_hex(path: &std::path::Path) -> Result<(u64, String), std::io::Error> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let size = bytes.len() as u64;
    let hex = data_encoding::HEXLOWER.encode(&Sha256::digest(&bytes));
    Ok((size, hex))
}

pub struct EngineRuntime {
    pub config: EngineConfig,
    _instance_lock: std::fs::File,
    conn: duckdb::Connection,
    sqlite_business: SqliteBusinessStore,
    ts_adapter: Box<dyn TimeSeriesStorageAdapter>,
    wal: AppWal,
    next_ingest_seq: u64,
    pub stream_epoch: String,
    pub stream_seq: u64,
    state: EngineState,
}

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        let _ = self.sqlite_business.sync_from_catalog(&self.conn);
    }
}

impl EngineRuntime {
    pub fn open(config: EngineConfig) -> Result<Self, EngineError> {
        config.validate()?;
        let _ = std::fs::create_dir_all(&config.wal_dir);
        let instance_lock = acquire_instance_lock(&config.data_dir)?;
        let sqlite_business = SqliteBusinessStore::open(&config.sqlite_business_db_path())?;
        let conn = duckdb::Connection::open(config.db_path())?;
        init_core_schema(&conn)?;
        // Restore business metadata from SQLite if present, then ensure SQLite mirrors active catalog
        let _ = sqlite_business.restore_to_catalog(&conn)?;
        let _ = sqlite_business.sync_from_catalog(&conn)?;
        let wal = AppWal::open(&config)?;

        // C3: Restore monotonic sequences from existing DB state to survive restarts
        let max_ingest_seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(CAST(SUBSTR(ingest_id, 5) AS BIGINT)), 0) FROM applied_message",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let max_stream_seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(resource_revision), 0) FROM latest_value",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let seq = NEXT_EPOCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let stream_epoch = format!("se-{}-{:04}", now_unix_ms, seq);

        Ok(Self {
            config,
            _instance_lock: instance_lock,
            conn,
            sqlite_business,
            ts_adapter: Box::new(DuckDbTimeSeriesAdapter::new()),
            wal,
            next_ingest_seq: (max_ingest_seq + 1) as u64,
            stream_epoch,
            stream_seq: max_stream_seq as u64,
            state: EngineState::Running,
        })
    }

    pub fn sqlite_business(&self) -> &SqliteBusinessStore {
        &self.sqlite_business
    }

    pub fn sync_business_metadata(&self) -> Result<(), EngineError> {
        self.sqlite_business.sync_from_catalog(&self.conn)
    }

    pub fn timeseries_adapter(&self) -> &dyn TimeSeriesStorageAdapter {
        self.ts_adapter.as_ref()
    }

    pub fn switch_to_duckdb_adapter(&mut self) {
        self.ts_adapter = Box::new(DuckDbTimeSeriesAdapter::new());
    }

    pub fn switch_to_tdengine_adapter(&mut self, cfg: TdengineConfig) -> Result<(), EngineError> {
        let adapter = TdengineTimeSeriesAdapter::new(cfg)?;
        self.ts_adapter = Box::new(adapter);
        Ok(())
    }

    pub fn switch_timeseries_backend(
        &mut self,
        kind: TimeSeriesBackendKind,
        tdengine_cfg: Option<TdengineConfig>,
    ) -> Result<(), EngineError> {
        match kind {
            TimeSeriesBackendKind::Duckdb => {
                self.switch_to_duckdb_adapter();
                Ok(())
            }
            TimeSeriesBackendKind::Tdengine => {
                self.switch_to_tdengine_adapter(tdengine_cfg.unwrap_or_default())
            }
            TimeSeriesBackendKind::Timescaledb | TimeSeriesBackendKind::Clickhouse => {
                self.ts_adapter = Box::new(ExtensibleSqlTimeSeriesAdapter::new(kind));
                Ok(())
            }
        }
    }

    pub fn switch_timeseries_backend_checked(
        &mut self,
        kind: TimeSeriesBackendKind,
        tdengine_cfg: Option<TdengineConfig>,
        strict_lossless: bool,
    ) -> Result<(), EngineError> {
        if strict_lossless {
            let target_caps = capability_snapshot_for_kind(kind);
            if target_caps.same_timestamp_distinct != CapabilityLevel::Supported
                || target_caps.exact_id_roundtrip != CapabilityLevel::Supported
            {
                return Err(EngineError::new(
                    "req-ts-adapter",
                    ErrorCode::CapabilityUnsupported,
                    format!(
                        "Backend '{}' does not guarantee lossless same-timestamp distinct observations or exact observation_id roundtrip",
                        kind.as_str()
                    ),
                    vec![FieldIssue {
                        field: "strict_lossless".into(),
                        issue: "LOSSLESS_SAME_TIMESTAMP_UNSUPPORTED".into(),
                        description: "Target backend capability level is not 'supported' for same_timestamp_distinct and exact_id_roundtrip".into(),
                    }],
                ));
            }
        }
        self.switch_timeseries_backend(kind, tdengine_cfg)
    }

    pub fn storage_capabilities(&self) -> StorageCapabilitiesResponse {
        StorageCapabilitiesResponse {
            active_backend: self.ts_adapter.backend_kind().as_str().to_string(),
            active_capabilities: self.ts_adapter.describe_capabilities(),
            backends: vec![
                capability_snapshot_for_kind(TimeSeriesBackendKind::Duckdb),
                capability_snapshot_for_kind(TimeSeriesBackendKind::Tdengine),
                capability_snapshot_for_kind(TimeSeriesBackendKind::Timescaledb),
                capability_snapshot_for_kind(TimeSeriesBackendKind::Clickhouse),
            ],
        }
    }

    pub fn record_target_delivery_outcome(
        &self,
        target_backend: &str,
        wal_position: &str,
        ingest_id: &str,
        status: &str,
        reason: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), EngineError> {
        match status {
            "committed" | "unknown" | "retryable" | "permanent_rejected" | "isolated_gap" => {}
            other => {
                return Err(EngineError::new(
                    "req-delivery-ledger",
                    ErrorCode::ValidationFailed,
                    format!("Unsupported delivery ledger status '{}'", other),
                    vec![],
                ));
            }
        }
        self.conn.execute(
            "INSERT INTO storage_delivery_ledger (
                target_backend, wal_position, ingest_id, status, reason, operator_id, recorded_at_ms
             ) VALUES (?, ?, ?, ?, ?, NULL, ?)
             ON CONFLICT (target_backend, wal_position) DO UPDATE SET
                ingest_id = excluded.ingest_id,
                status = excluded.status,
                reason = excluded.reason,
                operator_id = NULL,
                recorded_at_ms = excluded.recorded_at_ms",
            params![
                target_backend,
                wal_position,
                ingest_id,
                status,
                reason,
                recorded_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn delivery_checkpoint_summary(
        &self,
        target_backend: Option<&str>,
    ) -> Result<DeliveryCheckpointSummary, EngineError> {
        let target = target_backend.unwrap_or_else(|| self.ts_adapter.backend_kind().as_str());
        let mut q = self.conn.prepare(
            "SELECT wal_position, ingest_id, status, reason, operator_id, recorded_at_ms
             FROM storage_delivery_ledger
             WHERE target_backend = ?
             ORDER BY wal_position ASC",
        )?;
        let mut rows = q.query(params![target])?;

        let mut contiguous_checkpoint_wal_pos: Option<String> = None;
        let mut highest_seen_wal_pos: Option<String> = None;
        let mut committed_count = 0usize;
        let mut isolated_gap_count = 0usize;
        let mut unknown_count = 0usize;
        let mut retryable_count = 0usize;
        let mut permanent_rejected_count = 0usize;
        let mut prefix_broken = false;
        let mut holes = Vec::new();

        while let Some(r) = rows.next()? {
            let wal_pos: String = r.get(0)?;
            let ingest_id: String = r.get(1)?;
            let status: String = r.get(2)?;
            let reason: Option<String> = r.get(3)?;
            let operator_id: Option<String> = r.get(4)?;
            let recorded_at_ms: i64 = r.get(5)?;

            highest_seen_wal_pos = Some(wal_pos.clone());
            match status.as_str() {
                "committed" => {
                    committed_count += 1;
                    if !prefix_broken {
                        contiguous_checkpoint_wal_pos = Some(wal_pos);
                    }
                }
                "isolated_gap" => {
                    isolated_gap_count += 1;
                    if !prefix_broken {
                        contiguous_checkpoint_wal_pos = Some(wal_pos.clone());
                    }
                    holes.push(DeliveryHoleRecord {
                        wal_position: wal_pos,
                        ingest_id,
                        target_backend: target.to_string(),
                        status,
                        reason,
                        operator_id,
                        recorded_at_ms,
                    });
                }
                "unknown" => {
                    unknown_count += 1;
                    prefix_broken = true;
                    holes.push(DeliveryHoleRecord {
                        wal_position: wal_pos,
                        ingest_id,
                        target_backend: target.to_string(),
                        status,
                        reason,
                        operator_id,
                        recorded_at_ms,
                    });
                }
                "retryable" => {
                    retryable_count += 1;
                    prefix_broken = true;
                    holes.push(DeliveryHoleRecord {
                        wal_position: wal_pos,
                        ingest_id,
                        target_backend: target.to_string(),
                        status,
                        reason,
                        operator_id,
                        recorded_at_ms,
                    });
                }
                _ => {
                    permanent_rejected_count += 1;
                    prefix_broken = true;
                    holes.push(DeliveryHoleRecord {
                        wal_position: wal_pos,
                        ingest_id,
                        target_backend: target.to_string(),
                        status,
                        reason,
                        operator_id,
                        recorded_at_ms,
                    });
                }
            }
        }

        let has_unreconciled_holes =
            (unknown_count + retryable_count + permanent_rejected_count) > 0;

        Ok(DeliveryCheckpointSummary {
            target_backend: target.to_string(),
            contiguous_checkpoint_wal_pos,
            highest_seen_wal_pos,
            committed_count,
            isolated_gap_count,
            unknown_count,
            retryable_count,
            permanent_rejected_count,
            has_unreconciled_holes,
            holes,
        })
    }

    pub fn reconcile_delivery_hole(
        &self,
        req: ReconcileDeliveryHoleRequest,
        now_ms: i64,
    ) -> Result<DeliveryCheckpointSummary, EngineError> {
        if req.reason.trim().is_empty() || req.operator_id.trim().is_empty() {
            return Err(EngineError::new(
                "req-reconcile-hole",
                ErrorCode::ValidationFailed,
                "Both reason and operator_id must be non-empty when reconciling a delivery hole",
                vec![FieldIssue {
                    field: "reason".into(),
                    issue: "EMPTY_RECONCILIATION_AUDIT_FIELD".into(),
                    description: "Non-empty reason and operator_id are required".into(),
                }],
            ));
        }
        let new_status = match req.resolution.as_str() {
            "mark_committed" => "committed",
            "isolate_gap" => "isolated_gap",
            other => {
                return Err(EngineError::new(
                    "req-reconcile-hole",
                    ErrorCode::ValidationFailed,
                    format!(
                        "Unsupported resolution '{}'; expected 'mark_committed' or 'isolate_gap'",
                        other
                    ),
                    vec![FieldIssue {
                        field: "resolution".into(),
                        issue: "INVALID_HOLE_RESOLUTION".into(),
                        description: "Supported resolutions: 'mark_committed', 'isolate_gap'"
                            .into(),
                    }],
                ));
            }
        };
        let target = req
            .target_backend
            .as_deref()
            .unwrap_or_else(|| self.ts_adapter.backend_kind().as_str());
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT status FROM storage_delivery_ledger WHERE target_backend = ? AND wal_position = ?",
                params![target, req.wal_position],
                |r| r.get(0),
            )
            .ok();
        if exists.is_none() {
            return Err(EngineError::new(
                "req-reconcile-hole",
                ErrorCode::ResourceNotFound,
                format!(
                    "No delivery ledger entry found for target '{}' at wal_position '{}'",
                    target, req.wal_position
                ),
                vec![],
            ));
        }
        self.conn.execute(
            "UPDATE storage_delivery_ledger
             SET status = ?, reason = ?, operator_id = ?, recorded_at_ms = ?
             WHERE target_backend = ? AND wal_position = ?",
            params![
                new_status,
                req.reason.trim(),
                req.operator_id.trim(),
                now_ms,
                target,
                req.wal_position
            ],
        )?;
        self.delivery_checkpoint_summary(Some(target))
    }

    pub fn create_consistent_backup(
        &mut self,
        backup_id: Option<&str>,
        now_ms: i64,
    ) -> Result<BackupManifest, EngineError> {
        self.require_running()?;
        let id = match backup_id {
            Some(raw) => validate_backup_id(raw)?,
            None => format!("bkp-{}", now_ms),
        };
        let bkp_dir = self.config.data_dir.join("backups").join(&id);
        if bkp_dir.exists() {
            return Err(EngineError::new(
                "req-backup",
                ErrorCode::RevisionConflict,
                format!("Backup '{}' already exists", id),
                vec![],
            ));
        }
        let bkp_data_dir = bkp_dir.join("data");
        let bkp_wal_dir = bkp_dir.join("wal");
        std::fs::create_dir_all(&bkp_data_dir).map_err(|e| {
            EngineError::new("req-backup", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        std::fs::create_dir_all(&bkp_wal_dir).map_err(|e| {
            EngineError::new("req-backup", ErrorCode::InternalError, e.to_string(), vec![])
        })?;

        // Step A: SQLite business metadata
        self.sqlite_business.sync_from_catalog(&self.conn)?;
        self.sqlite_business
            .vacuum_into(&bkp_data_dir.join("business_metadata.sqlite"))?;

        // Step B: DuckDB embedded time-series store
        self.conn.execute_batch("CHECKPOINT;")?;
        if self.config.db_path().exists() {
            std::fs::copy(self.config.db_path(), bkp_data_dir.join("engine.duckdb")).map_err(
                |e| EngineError::new("req-backup", ErrorCode::InternalError, e.to_string(), vec![]),
            )?;
        }

        // Step C: AppWal segments
        if let Ok(entries) = std::fs::read_dir(&self.config.wal_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(fname) = path.file_name() {
                        let _ = std::fs::copy(&path, bkp_wal_dir.join(fname));
                    }
                }
            }
        }

        let mut files = Vec::new();
        for (prefix, dir) in [("data", &bkp_data_dir), ("wal", &bkp_wal_dir)] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(fname) = path.file_name().and_then(|s| s.to_str()) {
                            if let Ok((size_bytes, sha256_hex)) = sha256_file_hex(&path) {
                                files.push(BackupFileEntry {
                                    relative_path: format!("{}/{}", prefix, fname),
                                    size_bytes,
                                    sha256_hex,
                                });
                            }
                        }
                    }
                }
            }
        }
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let sqlite_table_counts = self.sqlite_business.status_snapshot()?.row_counts;
        let duckdb_timeseries_counts = self
            .ts_adapter
            .status_snapshot(&self.config.db_path().to_string_lossy(), &self.conn)?
            .row_counts;
        let wal_checkpoint: Option<String> = self
            .conn
            .query_row(
                "SELECT contiguous_wal_position FROM target_checkpoint WHERE target_id = 'local-duckdb'",
                [],
                |r| r.get(0),
            )
            .ok();

        let manifest = BackupManifest {
            backup_id: id,
            created_at_ms: now_ms,
            stream_epoch: self.stream_epoch.clone(),
            wal_checkpoint,
            sqlite_table_counts,
            duckdb_timeseries_counts,
            files,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| {
            EngineError::new("req-backup", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        std::fs::write(bkp_dir.join("backup_manifest.json"), manifest_bytes).map_err(|e| {
            EngineError::new("req-backup", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        Ok(manifest)
    }

    pub fn list_backups(&self) -> Result<Vec<BackupManifest>, EngineError> {
        let backups_root = self.config.data_dir.join("backups");
        let mut items = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&backups_root) {
            for entry in entries.flatten() {
                let manifest_path = entry.path().join("backup_manifest.json");
                if manifest_path.is_file() {
                    if let Ok(bytes) = std::fs::read(&manifest_path) {
                        if let Ok(m) = serde_json::from_slice::<BackupManifest>(&bytes) {
                            items.push(m);
                        }
                    }
                }
            }
        }
        items.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.backup_id.cmp(&b.backup_id))
        });
        Ok(items)
    }

    pub fn verify_backup_in_isolation(
        &self,
        backup_id: &str,
    ) -> Result<BackupVerifyReport, EngineError> {
        let id = validate_backup_id(backup_id)?;
        let bkp_dir = self.config.data_dir.join("backups").join(&id);
        let manifest_path = bkp_dir.join("backup_manifest.json");
        if !manifest_path.is_file() {
            return Err(EngineError::new(
                "req-backup-verify",
                ErrorCode::ResourceNotFound,
                format!("Backup '{}' not found", id),
                vec![],
            ));
        }
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|e| {
            EngineError::new(
                "req-backup-verify",
                ErrorCode::InternalError,
                e.to_string(),
                vec![],
            )
        })?;
        let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes).map_err(|e| {
            EngineError::new(
                "req-backup-verify",
                ErrorCode::InternalError,
                format!("Corrupt backup_manifest.json: {}", e),
                vec![],
            )
        })?;

        let mut issues = Vec::new();
        for f in &manifest.files {
            let fpath = bkp_dir.join(&f.relative_path);
            match sha256_file_hex(&fpath) {
                Ok((actual_size, actual_sha)) => {
                    if actual_size != f.size_bytes {
                        issues.push(format!(
                            "File '{}' size mismatch: expected {}, got {}",
                            f.relative_path, f.size_bytes, actual_size
                        ));
                    }
                    if actual_sha != f.sha256_hex {
                        issues.push(format!(
                            "File '{}' SHA-256 mismatch: expected {}, got {}",
                            f.relative_path, f.sha256_hex, actual_sha
                        ));
                    }
                }
                Err(e) => {
                    issues.push(format!("Missing or unreadable file '{}': {}", f.relative_path, e));
                }
            }
        }

        let mut restored_sqlite_tables = 0usize;
        let mut restored_devices = 0usize;
        let mut restored_history_observations = 0usize;
        let mut restored_latest_values = 0usize;
        let mut wal_replayed_messages = 0usize;

        if issues.is_empty() {
            let seq = NEXT_EPOCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let verify_dir = std::env::temp_dir().join(format!(
                "iotfast-bkp-verify-{}-{}-{}",
                std::process::id(),
                id,
                seq
            ));
            let _ = std::fs::remove_dir_all(&verify_dir);
            let v_data = verify_dir.join("data");
            let v_wal = verify_dir.join("wal");
            let _ = std::fs::create_dir_all(&v_data);
            let _ = std::fs::create_dir_all(&v_wal);

            for (src_dir, dst_dir) in [(bkp_dir.join("data"), &v_data), (bkp_dir.join("wal"), &v_wal)] {
                if let Ok(entries) = std::fs::read_dir(&src_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            if let Some(fname) = p.file_name() {
                                let _ = std::fs::copy(&p, dst_dir.join(fname));
                            }
                        }
                    }
                }
            }

            match EngineRuntime::open(EngineConfig::for_test(&verify_dir)) {
                Ok(mut isolated_rt) => {
                    match isolated_rt.recover_from_wal() {
                        Ok(rec_sum) => {
                            wal_replayed_messages = rec_sum.replayed_messages;
                        }
                        Err(e) => {
                            issues.push(format!("WAL recovery failed in isolated restore: {}", e));
                        }
                    }
                    if let Ok(arch) = isolated_rt.storage_architecture_status() {
                        restored_sqlite_tables = arch.business_metadata_store.tables.len();
                        restored_devices = *arch
                            .business_metadata_store
                            .row_counts
                            .get("device")
                            .unwrap_or(&0);
                        restored_history_observations = *arch
                            .device_timeseries_store
                            .row_counts
                            .get("history_observation")
                            .unwrap_or(&0);
                        restored_latest_values = *arch
                            .device_timeseries_store
                            .row_counts
                            .get("latest_value")
                            .unwrap_or(&0);

                        for (tbl, &expected_cnt) in &manifest.sqlite_table_counts {
                            let actual_cnt = *arch
                                .business_metadata_store
                                .row_counts
                                .get(tbl)
                                .unwrap_or(&0);
                            if actual_cnt != expected_cnt {
                                issues.push(format!(
                                    "SQLite table '{}' row count mismatch: expected {}, got {}",
                                    tbl, expected_cnt, actual_cnt
                                ));
                            }
                        }
                    } else {
                        issues.push("Failed to read storage_architecture_status in isolated restore".into());
                    }
                }
                Err(e) => {
                    issues.push(format!("Failed to open EngineRuntime in isolated restore: {}", e));
                }
            }

            let _ = std::fs::remove_dir_all(&verify_dir);
        }

        Ok(BackupVerifyReport {
            backup_id: id,
            verified: issues.is_empty(),
            checked_files: manifest.files.len(),
            restored_sqlite_tables,
            restored_devices,
            restored_history_observations,
            restored_latest_values,
            wal_replayed_messages,
            issues,
        })
    }

    pub fn active_timeseries_backend(&self) -> TimeSeriesBackendKind {
        self.ts_adapter.backend_kind()
    }

    pub fn build_history_query_plan(
        &self,
        tenant_id: &str,
        resource_ids: &[String],
        start_ms: i64,
        end_ms: i64,
        bucket_ms: Option<i64>,
    ) -> TimeSeriesQueryPlan {
        self.ts_adapter
            .build_history_query_plan(tenant_id, resource_ids, start_ms, end_ms, bucket_ms)
    }

    pub fn storage_architecture_status(&self) -> Result<StorageArchitectureStatus, EngineError> {
        self.sqlite_business.sync_from_catalog(&self.conn)?;
        let biz = self.sqlite_business.status_snapshot()?;
        let ts = self
            .ts_adapter
            .status_snapshot(&self.config.db_path().to_string_lossy(), &self.conn)?;
        Ok(StorageArchitectureStatus {
            architecture: "sqlite_business_metadata + pluggable_timeseries_adapter".to_string(),
            business_metadata_store: biz,
            device_timeseries_store: ts,
        })
    }

    pub fn conn(&self) -> &duckdb::Connection {
        &self.conn
    }

    pub fn state(&self) -> EngineState {
        self.state
    }

    pub fn check_health(&self) -> Result<HealthStatus, EngineError> {
        let report = check_engine_health(&self.conn, &self.config)?;
        Ok(HealthStatus {
            state: self.state,
            report,
        })
    }

    fn require_running(&self) -> Result<(), EngineError> {
        if self.state != EngineState::Running {
            return Err(EngineError::new(
                "req-engine",
                ErrorCode::ServiceUnavailable,
                "Engine is halted due to WAL corruption; manual intervention required",
                vec![],
            ));
        }
        Ok(())
    }

    pub fn history_count(&self) -> Result<usize, EngineError> {
        let cnt: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM history_observation", [], |r| r.get(0))?;
        Ok(cnt as usize)
    }

    pub fn latest_f64(&self, resource_id: &str) -> Result<Option<f64>, EngineError> {
        let val: Option<f64> = self
            .conn
            .query_row(
                "SELECT value_f64 FROM latest_value WHERE resource_id = ?",
                params![resource_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(val)
    }

    pub fn ingest_telemetry(
        &mut self,
        ctx: &VerifiedScopeContext,
        msg: TelemetryEnvelope,
        durability: DurabilityPreference,
        now_ms: i64,
    ) -> Result<IngestReceipt, EngineError> {
        self.require_running()?;
        // R08: First verify caller identity & source_device_grant, then check if message_key
        // already exists in applied_message BEFORE enforcing current active binding window!
        ctx.require_action("telemetry:ingest")?;
        IngestService::verify_source_grant(
            &self.conn,
            ctx,
            &msg.source_id,
            &msg.device_id,
            now_ms,
        )?;

        let existing: Option<(String, String, String, String, String)> = self
            .conn
            .query_row(
                "SELECT ingest_id, payload_hash, wal_position, history_status, observation_ids_json
                 FROM applied_message
                 WHERE tenant_id = ? AND source_id = ? AND source_epoch = ? AND message_id = ?",
                params![
                    ctx.tenant_id,
                    msg.source_id,
                    msg.source_epoch,
                    msg.message_id
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();

        let is_known_key = existing.is_some();
        let batch = IngestService::validate_telemetry_inner(
            &self.conn,
            ctx,
            &msg,
            now_ms,
            self.config.max_future_skew_ms,
            !is_known_key,
        )?;

        if let Some((orig_ingest_id, orig_hash, orig_pos, orig_hist_status, obs_json)) = existing {
            if orig_hash == batch.payload_hash {
                let obs_ids: BTreeMap<String, String> =
                    serde_json::from_str(&obs_json).unwrap_or_default();
                return Ok(IngestReceipt {
                    http_status: 200,
                    ingest_id: orig_ingest_id,
                    message_id: msg.message_id,
                    status: "duplicate".to_string(),
                    history_status: orig_hist_status,
                    payload_hash: orig_hash,
                    wal_position: Some(orig_pos),
                    observation_ids: obs_ids,
                });
            } else {
                return Err(EngineError::new(
                    "req-ingest",
                    ErrorCode::IdempotencyConflict,
                    format!(
                        "Message '{}' already exists with different payload_hash",
                        msg.message_id
                    ),
                    vec![],
                ));
            }
        }

        let ingest_id = format!("ing-{:06}", self.next_ingest_seq);
        self.next_ingest_seq += 1;
        let wal_rec = batch.to_wal_record(&ingest_id);

        let wal_pos_str = match durability {
            DurabilityPreference::Durable => {
                let receipt = self.wal.append_durable(&wal_rec)?;
                receipt.wal_position.encode()
            }
            DurabilityPreference::Volatile => "0000000000000000:0000000000000000".to_string(),
        };

        self.apply_wal_record_in_tx(&wal_rec, &wal_pos_str, true)?;

        let status = match durability {
            DurabilityPreference::Durable => "accepted_durable",
            DurabilityPreference::Volatile => "accepted_volatile",
        };

        Ok(IngestReceipt {
            http_status: 202,
            ingest_id,
            message_id: msg.message_id,
            status: status.to_string(),
            history_status: "committed".to_string(),
            payload_hash: batch.payload_hash,
            wal_position: Some(wal_pos_str),
            observation_ids: batch.observation_ids,
        })
    }

    pub fn simulate_wal_fsync_without_db_commit(
        &mut self,
        ctx: &VerifiedScopeContext,
        msg: &TelemetryEnvelope,
        now_ms: i64,
    ) -> Result<WalPosition, EngineError> {
        let batch = IngestService::validate_telemetry(
            &self.conn,
            ctx,
            msg,
            now_ms,
            self.config.max_future_skew_ms,
        )?;
        let ingest_id = format!("ing-{:06}", self.next_ingest_seq);
        self.next_ingest_seq += 1;
        let wal_rec = batch.to_wal_record(&ingest_id);
        let receipt = self.wal.append_durable(&wal_rec)?;
        Ok(receipt.wal_position)
    }

    fn apply_wal_record_in_tx(
        &mut self,
        rec: &WalRecord,
        wal_pos_str: &str,
        advance_checkpoint: bool,
    ) -> Result<bool, EngineError> {
        let already_applied: Option<String> = self
            .conn
            .query_row(
                "SELECT payload_hash FROM applied_message
                 WHERE tenant_id = ? AND source_id = ? AND source_epoch = ? AND message_id = ?",
                params![
                    rec.tenant_id,
                    rec.source_id,
                    rec.source_epoch,
                    rec.message_id
                ],
                |r| r.get(0),
            )
            .ok();

        if already_applied.is_some() {
            if advance_checkpoint && wal_pos_str != "0000000000000000:0000000000000000" {
                self.conn.execute(
                    "INSERT INTO target_checkpoint (target_id, contiguous_wal_position, updated_at_ms)
                     VALUES ('local-duckdb', ?, ?)
                     ON CONFLICT (target_id) DO UPDATE SET contiguous_wal_position = ?, updated_at_ms = ?",
                    params![wal_pos_str, rec.received_at_ms, wal_pos_str, rec.received_at_ms],
                )?;
            }
            return Ok(false);
        }

        let mut obs_map = BTreeMap::new();
        for pt in &rec.points {
            obs_map.insert(pt.property.clone(), pt.observation_id.clone());
        }
        let obs_json = serde_json::to_string(&obs_map).unwrap();
        let mut persisted_history_points: Vec<WalPointRecord> = Vec::new();

        self.conn.execute_batch("BEGIN TRANSACTION")?;
        let tx_res = (|| -> Result<(), EngineError> {
            for pt in &rec.points {
                // Evaluate deadband and heartbeat against `last_persisted_value`
                let last_persisted: Option<(Option<f64>, String, String, i64)> = self
                    .conn
                    .query_row(
                        "SELECT value_f64, value_json, quality_state, persisted_at_ms FROM last_persisted_value WHERE resource_id = ?",
                        params![pt.resource_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .ok();

                let should_persist_history = match &last_persisted {
                    None => true,
                    Some((last_f64, last_json, last_quality, last_at_ms)) => {
                        let (last_f64, last_at_ms) = (*last_f64, *last_at_ms);
                        // I3: Quality state change always triggers history persist
                        let quality_changed = pt.quality_state != *last_quality;
                        let hb_expired = match pt.heartbeat_ms {
                            Some(hb) if hb > 0 => (pt.source_at_ms - last_at_ms).abs() >= hb,
                            _ => false,
                        };
                        // I3: Replayed points follow standard deadband/heartbeat rules
                        if quality_changed || hb_expired {
                            true
                        } else if let (Some(curr_f), Some(prev_f), Some(db)) =
                            (pt.value_f64, last_f64, pt.deadband)
                        {
                            (curr_f - prev_f).abs() >= db
                        } else {
                            &pt.value_json != last_json
                        }
                    }
                };

                if should_persist_history {
                    persisted_history_points.push(pt.clone());
                    self.conn.execute(
                        "INSERT INTO history_observation (
                            observation_id, tenant_id, resource_id, device_id, property,
                            model_revision_id, binding_revision_id, source_at_ms, received_at_ms,
                            source_time_origin, source_seq, value_type, value_json, value_f64,
                            unit_code, unit_symbol, quality, source_quality_code, replayed, wal_position
                         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        params![
                            pt.observation_id,
                            rec.tenant_id,
                            pt.resource_id,
                            rec.device_id,
                            pt.property,
                            rec.model_revision_id,
                            rec.binding_revision_id,
                            pt.source_at_ms,
                            rec.received_at_ms,
                            pt.source_time_origin,
                            pt.source_seq,
                            pt.value_type,
                            pt.value_json,
                            pt.value_f64,
                            pt.unit_code,
                            pt.unit_symbol,
                            pt.quality_state,
                            pt.quality_source_code,
                            pt.replayed,
                            wal_pos_str
                        ],
                    )?;

                    // Update last_persisted_value if this sample is newer
                    let update_lpv = match last_persisted {
                        None => true,
                        Some((_, _, _, last_at_ms)) => pt.source_at_ms >= last_at_ms,
                    };
                    if update_lpv {
                        self.conn.execute(
                            "INSERT INTO last_persisted_value (resource_id, value_f64, value_json, quality_state, persisted_at_ms)
                             VALUES (?, ?, ?, ?, ?)
                             ON CONFLICT (resource_id) DO UPDATE SET value_f64 = ?, value_json = ?, quality_state = ?, persisted_at_ms = ?",
                            params![
                                pt.resource_id,
                                pt.value_f64,
                                pt.value_json,
                                pt.quality_state,
                                pt.source_at_ms,
                                pt.value_f64,
                                pt.value_json,
                                pt.quality_state,
                                pt.source_at_ms
                            ],
                        )?;
                    }
                }

                // Ordering rule for `latest_value` (T05 & SC06): compare (source_at_ms, source_seq_u64)
                let curr_latest: Option<(i64, String, u64)> = self
                    .conn
                    .query_row(
                        "SELECT value_sampled_at_ms, source_epoch, source_seq_u64 FROM latest_value WHERE resource_id = ?",
                        params![pt.resource_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();

                let should_update_latest = match curr_latest {
                    None => true,
                    Some((curr_ts, curr_epoch, curr_seq)) => {
                        if curr_epoch == rec.source_epoch {
                            (pt.source_at_ms, pt.source_seq_u64) >= (curr_ts, curr_seq)
                        } else {
                            !pt.replayed && pt.source_at_ms >= curr_ts
                        }
                    }
                };

                if should_update_latest {
                    self.stream_seq += 1;
                    let new_res_rev = self.stream_seq as i64;
                    self.conn.execute(
                        "INSERT INTO latest_value (
                            resource_id, tenant_id, device_id, property, model_revision_id,
                            binding_revision_id, value_type, value_json, value_f64, value_i64,
                            value_bool, value_text, value_sampled_at_ms, received_at_ms,
                            source_time_origin, source_epoch, source_seq_u64, resource_revision
                         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                         ON CONFLICT (resource_id) DO UPDATE SET
                            model_revision_id = excluded.model_revision_id,
                            binding_revision_id = excluded.binding_revision_id,
                            value_type = excluded.value_type,
                            value_json = excluded.value_json,
                            value_f64 = excluded.value_f64,
                            value_i64 = excluded.value_i64,
                            value_bool = excluded.value_bool,
                            value_text = excluded.value_text,
                            value_sampled_at_ms = excluded.value_sampled_at_ms,
                            received_at_ms = excluded.received_at_ms,
                            source_time_origin = excluded.source_time_origin,
                            source_epoch = excluded.source_epoch,
                            source_seq_u64 = excluded.source_seq_u64,
                            resource_revision = excluded.resource_revision",
                        params![
                            pt.resource_id,
                            rec.tenant_id,
                            rec.device_id,
                            pt.property,
                            rec.model_revision_id,
                            rec.binding_revision_id,
                            pt.value_type,
                            pt.value_json,
                            pt.value_f64,
                            pt.value_i64,
                            pt.value_bool,
                            pt.value_text,
                            pt.source_at_ms,
                            rec.received_at_ms,
                            pt.source_time_origin,
                            rec.source_epoch,
                            pt.source_seq_u64,
                            new_res_rev
                        ],
                    )?;

                    // Update point_quality_state if newer or equal to existing quality_observed_at_ms
                    let curr_q_at: Option<i64> = self
                        .conn
                        .query_row(
                            "SELECT quality_observed_at_ms FROM point_quality_state WHERE resource_id = ?",
                            params![pt.resource_id],
                            |r| r.get(0),
                        )
                        .ok();
                    if curr_q_at.map_or(true, |q_at| pt.source_at_ms >= q_at) {
                        self.conn.execute(
                            "INSERT INTO point_quality_state (resource_id, tenant_id, device_id, property, quality_state, quality_reason, quality_observed_at_ms)
                             VALUES (?, ?, ?, ?, ?, ?, ?)
                             ON CONFLICT (resource_id) DO UPDATE SET
                                quality_state = excluded.quality_state,
                                quality_reason = excluded.quality_reason,
                                quality_observed_at_ms = excluded.quality_observed_at_ms",
                            params![
                                pt.resource_id,
                                rec.tenant_id,
                                rec.device_id,
                                pt.property,
                                pt.quality_state,
                                pt.quality_source_code,
                                pt.source_at_ms
                            ],
                        )?;
                    }
                }
            }

            self.conn.execute(
                "INSERT INTO applied_message (
                    tenant_id, source_id, source_epoch, message_id, ingest_id,
                    payload_hash, wal_position, history_status, observation_ids_json
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, 'committed', ?)",
                params![
                    rec.tenant_id,
                    rec.source_id,
                    rec.source_epoch,
                    rec.message_id,
                    rec.ingest_id,
                    rec.payload_hash,
                    wal_pos_str,
                    obs_json
                ],
            )?;

            let durability_status = if wal_pos_str == "0000000000000000:0000000000000000" {
                "accepted_volatile"
            } else {
                "accepted_durable"
            };
            self.conn.execute(
                "INSERT INTO ingest_status (ingest_id, tenant_id, message_id, status, history_status, wal_position, updated_at_ms)
                 VALUES (?, ?, ?, ?, 'committed', ?, ?)",
                params![
                    rec.ingest_id,
                    rec.tenant_id,
                    rec.message_id,
                    durability_status,
                    wal_pos_str,
                    rec.received_at_ms
                ],
            )?;

            if advance_checkpoint && wal_pos_str != "0000000000000000:0000000000000000" {
                self.conn.execute(
                    "INSERT INTO target_checkpoint (target_id, contiguous_wal_position, updated_at_ms)
                     VALUES ('local-duckdb', ?, ?)
                     ON CONFLICT (target_id) DO UPDATE SET contiguous_wal_position = ?, updated_at_ms = ?",
                    params![wal_pos_str, rec.received_at_ms, wal_pos_str, rec.received_at_ms],
                )?;
            }
            Ok(())
        })();

        match tx_res {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                let active_backend = self.ts_adapter.backend_kind().as_str().to_string();
                match self.ts_adapter.on_telemetry_committed(
                    rec,
                    wal_pos_str,
                    &persisted_history_points,
                ) {
                    Ok(_) => {
                        if wal_pos_str != "0000000000000000:0000000000000000" {
                            let _ = self.record_target_delivery_outcome(
                                &active_backend,
                                wal_pos_str,
                                &rec.ingest_id,
                                "committed",
                                None,
                                rec.received_at_ms,
                            );
                        }
                    }
                    Err(adapter_err) => {
                        if wal_pos_str != "0000000000000000:0000000000000000" {
                            let _ = self.record_target_delivery_outcome(
                                &active_backend,
                                wal_pos_str,
                                &rec.ingest_id,
                                "unknown",
                                Some(&adapter_err.message),
                                rec.received_at_ms,
                            );
                        } else {
                            return Err(adapter_err);
                        }
                    }
                }
                Ok(true)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn report_source_state(
        &mut self,
        ctx: &VerifiedScopeContext,
        evt: SourceStateEnvelope,
        now_ms: i64,
    ) -> Result<SourceStateReceipt, EngineError> {
        self.require_running()?;
        let validated = IngestService::validate_source_state(
            &self.conn,
            ctx,
            &evt,
            now_ms,
            self.config.max_future_skew_ms,
        )?;

        // Check independent source_state idempotency domain (R03)
        let existing_hash: Option<String> = self
            .conn
            .query_row(
                "SELECT event_hash FROM quality_event WHERE event_key = ?",
                params![validated.event_key],
                |r| r.get(0),
            )
            .ok();

        if let Some(orig_hash) = existing_hash {
            if orig_hash == validated.event_hash {
                return Ok(SourceStateReceipt {
                    http_status: 200,
                    event_id: evt.event_id,
                    status: "duplicate".to_string(),
                });
            } else {
                return Err(EngineError::new(
                    "req-source-state",
                    ErrorCode::IdempotencyConflict,
                    format!(
                        "Source state event_id '{}' already exists with different payload",
                        evt.event_id
                    ),
                    vec![],
                ));
            }
        }

        self.conn.execute_batch("BEGIN TRANSACTION")?;
        let res = (|| -> Result<(), EngineError> {
            self.conn.execute(
                "UPDATE source_gateway SET connection_state = ? WHERE tenant_id = ? AND source_id = ?",
                params![evt.connection_state, ctx.tenant_id, evt.source_id],
            )?;

            if evt.point_states.is_empty() {
                self.conn.execute(
                    "INSERT INTO quality_event (
                        event_key, tenant_id, source_id, source_epoch, event_id, device_id,
                        property, observed_at_ms, connection_state, quality_state, quality_reason, event_hash
                     ) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, NULL, NULL, ?)",
                    params![
                        validated.event_key,
                        ctx.tenant_id,
                        evt.source_id,
                        evt.source_epoch,
                        evt.event_id,
                        evt.device_id,
                        evt.observed_at_ms,
                        evt.connection_state,
                        validated.event_hash
                    ],
                )?;
            } else {
                for (idx, ps) in evt.point_states.iter().enumerate() {
                    let row_key = if idx == 0 {
                        validated.event_key.clone()
                    } else {
                        format!("{}:{}", validated.event_key, ps.property)
                    };
                    let resource_id = crate::catalog::asset::AssetUnsService::lookup_resource_id(
                        &self.conn,
                        &evt.device_id,
                        &ps.property,
                    );
                    self.stream_seq += 1;
                    // I2: Guard quality_observed_at_ms monotonicity — only update if newer or equal
                    let curr_q_at: Option<i64> = self.conn
                        .query_row(
                            "SELECT quality_observed_at_ms FROM point_quality_state WHERE resource_id = ?",
                            params![resource_id],
                            |r| r.get(0),
                        )
                        .ok();
                    if curr_q_at.map_or(true, |q_at| evt.observed_at_ms >= q_at) {
                        self.conn.execute(
                            "INSERT INTO point_quality_state (
                                resource_id, tenant_id, device_id, property,
                                quality_state, quality_reason, quality_observed_at_ms
                             ) VALUES (?, ?, ?, ?, ?, ?, ?)
                             ON CONFLICT (resource_id) DO UPDATE SET
                                quality_state = excluded.quality_state,
                                quality_reason = excluded.quality_reason,
                                quality_observed_at_ms = excluded.quality_observed_at_ms",
                            params![
                                resource_id,
                                ctx.tenant_id,
                                evt.device_id,
                                ps.property,
                                ps.quality_state,
                                ps.quality_reason,
                                evt.observed_at_ms
                            ],
                        )?;
                    }
                    // Also bump resource_revision on latest_value if present so subscribers see quality change
                    self.conn.execute(
                        "UPDATE latest_value SET resource_revision = ? WHERE resource_id = ?",
                        params![self.stream_seq as i64, resource_id],
                    )?;
                    self.conn.execute(
                        "INSERT INTO quality_event (
                            event_key, tenant_id, source_id, source_epoch, event_id, device_id,
                            property, observed_at_ms, connection_state, quality_state, quality_reason, event_hash
                         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        params![
                            row_key,
                            ctx.tenant_id,
                            evt.source_id,
                            evt.source_epoch,
                            evt.event_id,
                            evt.device_id,
                            ps.property,
                            evt.observed_at_ms,
                            evt.connection_state,
                            ps.quality_state,
                            ps.quality_reason,
                            validated.event_hash
                        ],
                    )?;
                }
            }
            Ok(())
        })();

        match res {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                let _ = self.ts_adapter.on_source_state_committed(&ctx.tenant_id, &evt);
                let _ = self.sqlite_business.sync_from_catalog(&self.conn);
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

        Ok(SourceStateReceipt {
            http_status: 202,
            event_id: evt.event_id,
            status: "accepted".to_string(),
        })
    }

    pub fn recover_from_wal(&mut self) -> Result<RecoverySummary, EngineError> {
        let cp_str: Option<String> = self
            .conn
            .query_row(
                "SELECT contiguous_wal_position FROM target_checkpoint WHERE target_id = 'local-duckdb'",
                [],
                |r| r.get(0),
            )
            .ok();
        let cp_pos = match &cp_str {
            Some(s) => Some(WalPosition::parse(s)?),
            None => None,
        };

        let scan = self.wal.scan_and_recover(cp_pos.as_ref())?;
        if let WalRecoveryOutcome::HaltedMiddleRecordCorrupted {
            corrupted_offset,
            reason,
        } = scan.outcome
        {
            self.state = EngineState::HaltedOnWalCorruption;
            return Err(EngineError::new(
                "req-wal-recover",
                ErrorCode::InternalError,
                format!(
                    "Halted WAL recovery at corrupted offset {}: {}",
                    corrupted_offset, reason
                ),
                vec![],
            ));
        }

        let mut replayed = 0usize;
        let mut last_pos = cp_str;
        for (pos, rec) in scan.records {
            let pos_str = pos.encode();
            if self.apply_wal_record_in_tx(&rec, &pos_str, true)? {
                replayed += 1;
            }
            last_pos = Some(pos_str);
        }

        // Re-sync monotonic counters after replay
        let max_seq: i64 = self.conn
            .query_row("SELECT COALESCE(MAX(CAST(SUBSTR(ingest_id, 5) AS BIGINT)), 0) FROM applied_message", [], |r| r.get(0))
            .unwrap_or(0);
        if (max_seq + 1) as u64 > self.next_ingest_seq {
            self.next_ingest_seq = (max_seq + 1) as u64;
        }
        let max_rev: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(resource_revision), 0) FROM latest_value",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if max_rev as u64 > self.stream_seq {
            self.stream_seq = max_rev as u64;
        }

        Ok(RecoverySummary {
            replayed_messages: replayed,
            latest_checkpoint: last_pos,
        })
    }

    pub fn get_ingest_status(
        &self,
        ctx: &VerifiedScopeContext,
        ingest_id: &str,
        now_ms: i64,
    ) -> Result<IngestStatusRecord, EngineError> {
        ctx.validate_active(now_ms)?;
        let row: Option<IngestStatusRecord> = self
            .conn
            .query_row(
                "SELECT ingest_id, tenant_id, message_id, status, history_status, wal_position, updated_at_ms
                 FROM ingest_status
                 WHERE ingest_id = ? AND tenant_id = ?",
                params![ingest_id, ctx.tenant_id],
                |r| {
                    Ok(IngestStatusRecord {
                        ingest_id: r.get(0)?,
                        tenant_id: r.get(1)?,
                        message_id: r.get(2)?,
                        status: r.get(3)?,
                        history_status: r.get(4)?,
                        wal_position: r.get(5)?,
                        updated_at_ms: r.get(6)?,
                    })
                },
            )
            .ok();
        row.ok_or_else(|| {
            EngineError::new(
                "req-ingest-status",
                ErrorCode::ResourceNotFound,
                format!("Ingest status '{}' not found", ingest_id),
                vec![],
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestStatusRecord {
    pub ingest_id: String,
    pub tenant_id: String,
    pub message_id: String,
    pub status: String,
    pub history_status: String,
    pub wal_position: Option<String>,
    pub updated_at_ms: i64,
}
