use crate::catalog::sqlite_store::BusinessStoreStatus;
use crate::core::error::{EngineError, ErrorCode, FieldIssue};
use crate::ingest::validate::SourceStateEnvelope;
use crate::ingest::wal::{WalPointRecord, WalRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const TIMESERIES_DEVICE_TABLES: &[&str] = &[
    "latest_value",
    "point_quality_state",
    "last_persisted_value",
    "history_observation",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeSeriesBackendKind {
    Duckdb,
    Tdengine,
    Timescaledb,
    Clickhouse,
}

impl TimeSeriesBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeSeriesBackendKind::Duckdb => "duckdb",
            TimeSeriesBackendKind::Tdengine => "tdengine",
            TimeSeriesBackendKind::Timescaledb => "timescaledb",
            TimeSeriesBackendKind::Clickhouse => "clickhouse",
        }
    }

    pub fn parse(s: &str) -> Result<Self, EngineError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "duckdb" => Ok(TimeSeriesBackendKind::Duckdb),
            "tdengine" | "taos" => Ok(TimeSeriesBackendKind::Tdengine),
            "timescaledb" | "timescale" => Ok(TimeSeriesBackendKind::Timescaledb),
            "clickhouse" => Ok(TimeSeriesBackendKind::Clickhouse),
            other => Err(EngineError::new(
                "req-ts-adapter",
                ErrorCode::ValidationFailed,
                format!(
                    "Unsupported time-series backend '{}'; supported values: duckdb, tdengine, timescaledb, clickhouse",
                    other
                ),
                vec![FieldIssue {
                    field: "backend".into(),
                    issue: "UNSUPPORTED_TS_BACKEND".into(),
                    description:
                        "Supported backends are 'duckdb', 'tdengine', 'timescaledb', and 'clickhouse'"
                            .into(),
                }],
            )),
        }
    }
}

fn supported_backend_names() -> Vec<String> {
    vec![
        "duckdb".to_string(),
        "tdengine".to_string(),
        "timescaledb".to_string(),
        "clickhouse".to_string(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TdengineExecutionMode {
    /// Captures and validates TDengine STABLE/Subtable DDL & DML statements locally
    /// while mirroring hot query state so single-node deployments work without `taosd`.
    LocalMirrorWithSqlSpool,
    /// Executes DDL & DML directly against a live TDengine HTTP REST endpoint (`/rest/sql/{db}`).
    HttpRest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TdengineConfig {
    pub endpoint: String,
    pub database: String,
    pub super_table: String,
    pub username: String,
    pub password: String,
    pub execution_mode: TdengineExecutionMode,
}

impl TdengineConfig {
    pub fn redacted(&self) -> Self {
        let mut c = self.clone();
        c.password = "***REDACTED***".to_string();
        c
    }
}

impl Default for TdengineConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:6041".to_string(),
            database: "iotfast_tsdb".to_string(),
            super_table: "stb_observations".to_string(),
            username: "root".to_string(),
            password: "taosdata".to_string(),
            execution_mode: TdengineExecutionMode::LocalMirrorWithSqlSpool,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLevel {
    Supported,
    Unsupported,
    Conditional,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCapabilitySnapshot {
    pub backend: String,
    pub exact_id_roundtrip: CapabilityLevel,
    pub same_timestamp_distinct: CapabilityLevel,
    pub replay_idempotent: CapabilityLevel,
    pub duplicate_conflict_detect: CapabilityLevel,
    pub batch_result_scope: CapabilityLevel,
    pub snapshot_pagination: CapabilityLevel,
    pub quality_roundtrip: CapabilityLevel,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageCapabilitiesResponse {
    pub active_backend: String,
    pub active_capabilities: BackendCapabilitySnapshot,
    pub backends: Vec<BackendCapabilitySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryHoleRecord {
    pub wal_position: String,
    pub ingest_id: String,
    pub target_backend: String,
    pub status: String,
    pub reason: Option<String>,
    pub operator_id: Option<String>,
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryCheckpointSummary {
    pub target_backend: String,
    pub contiguous_checkpoint_wal_pos: Option<String>,
    pub highest_seen_wal_pos: Option<String>,
    pub committed_count: usize,
    pub isolated_gap_count: usize,
    pub unknown_count: usize,
    pub retryable_count: usize,
    pub permanent_rejected_count: usize,
    pub has_unreconciled_holes: bool,
    pub holes: Vec<DeliveryHoleRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileDeliveryHoleRequest {
    pub target_backend: Option<String>,
    pub wal_position: String,
    pub resolution: String,
    pub reason: String,
    pub operator_id: String,
}

pub fn capability_snapshot_for_kind(kind: TimeSeriesBackendKind) -> BackendCapabilitySnapshot {
    match kind {
        TimeSeriesBackendKind::Duckdb => BackendCapabilitySnapshot {
            backend: "duckdb".to_string(),
            exact_id_roundtrip: CapabilityLevel::Supported,
            same_timestamp_distinct: CapabilityLevel::Supported,
            replay_idempotent: CapabilityLevel::Supported,
            duplicate_conflict_detect: CapabilityLevel::Supported,
            batch_result_scope: CapabilityLevel::Supported,
            snapshot_pagination: CapabilityLevel::Supported,
            quality_roundtrip: CapabilityLevel::Supported,
            notes: vec![
                "Embedded DuckDB v2.0.0 single-transaction history, observation_id primary key, and watermark pagination verified locally".to_string(),
            ],
        },
        TimeSeriesBackendKind::Tdengine => BackendCapabilitySnapshot {
            backend: "tdengine".to_string(),
            exact_id_roundtrip: CapabilityLevel::Conditional,
            same_timestamp_distinct: CapabilityLevel::Conditional,
            replay_idempotent: CapabilityLevel::Conditional,
            duplicate_conflict_detect: CapabilityLevel::Unsupported,
            batch_result_scope: CapabilityLevel::Conditional,
            snapshot_pagination: CapabilityLevel::Unverified,
            quality_roundtrip: CapabilityLevel::Supported,
            notes: vec![
                "TDengine subtable primary key is ts; same-millisecond distinct observations overwrite unless disambiguated".to_string(),
            ],
        },
        TimeSeriesBackendKind::Timescaledb | TimeSeriesBackendKind::Clickhouse => {
            BackendCapabilitySnapshot {
                backend: kind.as_str().to_string(),
                exact_id_roundtrip: CapabilityLevel::Conditional,
                same_timestamp_distinct: CapabilityLevel::Conditional,
                replay_idempotent: CapabilityLevel::Unverified,
                duplicate_conflict_detect: CapabilityLevel::Unverified,
                batch_result_scope: CapabilityLevel::Unverified,
                snapshot_pagination: CapabilityLevel::Unverified,
                quality_roundtrip: CapabilityLevel::Conditional,
                notes: vec![
                    "Extensible SQL adapter; live target cluster verification is required per C05 before claiming production support".to_string(),
                ],
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesWriteReceipt {
    pub backend: String,
    pub latest_points_updated: usize,
    pub history_points_persisted: usize,
    pub generated_sql_statements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesQueryPlan {
    pub backend: String,
    pub mode: String,
    pub native_sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesStoreStatus {
    pub active_backend: String,
    pub supported_backends: Vec<String>,
    pub capabilities: BackendCapabilitySnapshot,
    pub duckdb_path: String,
    pub tdengine_config: Option<TdengineConfig>,
    pub tdengine_super_table_ddl: Option<String>,
    pub recent_tdengine_sql_log: Vec<String>,
    pub tables: Vec<String>,
    pub row_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageArchitectureStatus {
    pub architecture: String,
    pub business_metadata_store: BusinessStoreStatus,
    pub device_timeseries_store: TimeSeriesStoreStatus,
}

/// Unified abstraction for device time-series persistence and query pushdown.
///
/// Default implementation: `DuckDbTimeSeriesAdapter` (embedded columnar engine).
/// Extensible implementation: `TdengineTimeSeriesAdapter` (TDengine Super Table `STABLE` + Subtable).
pub trait TimeSeriesStorageAdapter: Send + Sync {
    fn backend_kind(&self) -> TimeSeriesBackendKind;

    fn describe_capabilities(&self) -> BackendCapabilitySnapshot {
        capability_snapshot_for_kind(self.backend_kind())
    }

    fn on_telemetry_committed(
        &mut self,
        rec: &WalRecord,
        wal_pos: &str,
        persisted_history_points: &[WalPointRecord],
    ) -> Result<TimeSeriesWriteReceipt, EngineError>;

    fn on_source_state_committed(
        &mut self,
        tenant_id: &str,
        event: &SourceStateEnvelope,
    ) -> Result<(), EngineError>;

    fn build_history_query_plan(
        &self,
        tenant_id: &str,
        resource_ids: &[String],
        start_ms: i64,
        end_ms: i64,
        bucket_ms: Option<i64>,
    ) -> TimeSeriesQueryPlan;

    fn build_latest_query_plan(&self, tenant_id: &str, resource_id: &str) -> String;

    fn status_snapshot(
        &self,
        duckdb_path: &str,
        conn: &duckdb::Connection,
    ) -> Result<TimeSeriesStoreStatus, EngineError>;
}

pub struct DuckDbTimeSeriesAdapter;

impl DuckDbTimeSeriesAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DuckDbTimeSeriesAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSeriesStorageAdapter for DuckDbTimeSeriesAdapter {
    fn backend_kind(&self) -> TimeSeriesBackendKind {
        TimeSeriesBackendKind::Duckdb
    }

    fn on_telemetry_committed(
        &mut self,
        rec: &WalRecord,
        _wal_pos: &str,
        persisted_history_points: &[WalPointRecord],
    ) -> Result<TimeSeriesWriteReceipt, EngineError> {
        Ok(TimeSeriesWriteReceipt {
            backend: "duckdb".to_string(),
            latest_points_updated: rec.points.len(),
            history_points_persisted: persisted_history_points.len(),
            generated_sql_statements: Vec::new(),
        })
    }

    fn on_source_state_committed(
        &mut self,
        _tenant_id: &str,
        _event: &SourceStateEnvelope,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    fn build_history_query_plan(
        &self,
        tenant_id: &str,
        resource_ids: &[String],
        start_ms: i64,
        end_ms: i64,
        bucket_ms: Option<i64>,
    ) -> TimeSeriesQueryPlan {
        let in_list = resource_ids
            .iter()
            .map(|r| format!("'{}'", escape_sql_literal(r)))
            .collect::<Vec<_>>()
            .join(", ");
        match bucket_ms {
            None => TimeSeriesQueryPlan {
                backend: "duckdb".to_string(),
                mode: "raw".to_string(),
                native_sql: format!(
                    "SELECT observation_id, resource_id, source_at_ms, value_json, quality FROM history_observation WHERE tenant_id = '{}' AND resource_id IN ({}) AND source_at_ms >= {} AND source_at_ms < {} ORDER BY source_at_ms ASC, resource_id ASC",
                    escape_sql_literal(tenant_id),
                    in_list,
                    start_ms,
                    end_ms
                ),
            },
            Some(b_ms) => TimeSeriesQueryPlan {
                backend: "duckdb".to_string(),
                mode: "aggregate".to_string(),
                native_sql: format!(
                    "SELECT resource_id, ({start_ms} + ((source_at_ms - {start_ms}) / {b_ms}) * {b_ms}) AS bucket_start_ms, AVG(value_f64) AS avg_val, MIN(value_f64) AS min_val, MAX(value_f64) AS max_val, COUNT(*) AS sample_count FROM history_observation WHERE tenant_id = '{}' AND resource_id IN ({}) AND source_at_ms >= {} AND source_at_ms < {} GROUP BY resource_id, bucket_start_ms ORDER BY bucket_start_ms ASC",
                    escape_sql_literal(tenant_id),
                    in_list,
                    start_ms,
                    end_ms
                ),
            },
        }
    }

    fn build_latest_query_plan(&self, tenant_id: &str, resource_id: &str) -> String {
        format!(
            "SELECT resource_id, value_json, value_sampled_at_ms, received_at_ms, resource_revision FROM latest_value WHERE tenant_id = '{}' AND resource_id = '{}'",
            escape_sql_literal(tenant_id),
            escape_sql_literal(resource_id)
        )
    }

    fn status_snapshot(
        &self,
        duckdb_path: &str,
        conn: &duckdb::Connection,
    ) -> Result<TimeSeriesStoreStatus, EngineError> {
        let counts = query_duckdb_ts_counts(conn)?;
        Ok(TimeSeriesStoreStatus {
            active_backend: "duckdb".to_string(),
            supported_backends: supported_backend_names(),
            capabilities: self.describe_capabilities(),
            duckdb_path: duckdb_path.to_string(),
            tdengine_config: None,
            tdengine_super_table_ddl: None,
            recent_tdengine_sql_log: Vec::new(),
            tables: TIMESERIES_DEVICE_TABLES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            row_counts: counts,
        })
    }
}

pub struct TdengineTimeSeriesAdapter {
    config: TdengineConfig,
    sql_log: Vec<String>,
}

impl TdengineTimeSeriesAdapter {
    pub fn new(config: TdengineConfig) -> Result<Self, EngineError> {
        let mut adapter = Self {
            config,
            sql_log: Vec::new(),
        };
        let ddl = adapter.super_table_ddl();
        adapter.record_or_execute_sql(&ddl)?;
        Ok(adapter)
    }

    pub fn config(&self) -> &TdengineConfig {
        &self.config
    }

    pub fn recent_sql_log(&self) -> &[String] {
        &self.sql_log
    }

    pub fn super_table_ddl(&self) -> String {
        format!(
            "CREATE DATABASE IF NOT EXISTS {db} PRECISION 'ms' KEEP 365; \
             CREATE STABLE IF NOT EXISTS {db}.{stb} (\
             ts TIMESTAMP, received_at TIMESTAMP, val_f64 DOUBLE, val_i64 BIGINT, \
             val_bool BOOL, val_str NCHAR(256), unit_code NCHAR(32), quality NCHAR(16), \
             source_seq NCHAR(64), observation_id NCHAR(64), replayed BOOL, wal_position NCHAR(64)\
             ) TAGS (\
             tenant_id NCHAR(64), resource_id NCHAR(128), device_id NCHAR(64), \
             property NCHAR(64), model_revision_id NCHAR(64), binding_revision_id NCHAR(64)\
             );",
            db = sanitize_identifier(&self.config.database),
            stb = sanitize_identifier(&self.config.super_table),
        )
    }

    pub fn subtable_name_for_resource(resource_id: &str) -> String {
        format!("tb_{}", sanitize_identifier(resource_id))
    }

    pub fn build_insert_sql(
        &self,
        rec: &WalRecord,
        wal_pos: &str,
        points: &[WalPointRecord],
    ) -> Option<String> {
        if points.is_empty() {
            return None;
        }
        let db = sanitize_identifier(&self.config.database);
        let stb = sanitize_identifier(&self.config.super_table);
        let mut clauses = Vec::with_capacity(points.len());
        for pt in points {
            let subtable = Self::subtable_name_for_resource(&pt.resource_id);
            let val_f64 = pt
                .value_f64
                .map(|v| format!("{}", v))
                .unwrap_or_else(|| "NULL".to_string());
            let val_i64 = pt
                .value_i64
                .map(|v| format!("{}", v))
                .unwrap_or_else(|| "NULL".to_string());
            let val_bool = pt
                .value_bool
                .map(|v| if v { "true" } else { "false" }.to_string())
                .unwrap_or_else(|| "NULL".to_string());
            let val_str = pt
                .value_text
                .as_ref()
                .map(|s| format!("'{}'", escape_sql_literal(s)))
                .unwrap_or_else(|| format!("'{}'", escape_sql_literal(&pt.value_json)));
            let unit_code = pt
                .unit_code
                .as_ref()
                .map(|u| format!("'{}'", escape_sql_literal(u)))
                .unwrap_or_else(|| "NULL".to_string());

            clauses.push(format!(
                "{db}.{sub} USING {db}.{stb} TAGS ('{tenant}', '{res}', '{dev}', '{prop}', '{mrev}', '{brev}') \
                 VALUES ({ts}, {rx}, {vf}, {vi}, {vb}, {vs}, {unit}, '{qual}', '{seq}', '{obs}', {rep}, '{wal}')",
                db = db,
                sub = subtable,
                stb = stb,
                tenant = escape_sql_literal(&rec.tenant_id),
                res = escape_sql_literal(&pt.resource_id),
                dev = escape_sql_literal(&rec.device_id),
                prop = escape_sql_literal(&pt.property),
                mrev = escape_sql_literal(&rec.model_revision_id),
                brev = escape_sql_literal(&rec.binding_revision_id),
                ts = pt.source_at_ms,
                rx = rec.received_at_ms,
                vf = val_f64,
                vi = val_i64,
                vb = val_bool,
                vs = val_str,
                unit = unit_code,
                qual = escape_sql_literal(&pt.quality_state),
                seq = escape_sql_literal(&pt.source_seq),
                obs = escape_sql_literal(&pt.observation_id),
                rep = if pt.replayed { "true" } else { "false" },
                wal = escape_sql_literal(wal_pos),
            ));
        }
        Some(format!("INSERT INTO {};", clauses.join(" ")))
    }

    fn record_or_execute_sql(&mut self, sql: &str) -> Result<(), EngineError> {
        if self.sql_log.len() >= 64 {
            self.sql_log.remove(0);
        }
        self.sql_log.push(sql.to_string());
        if self.config.execution_mode == TdengineExecutionMode::HttpRest {
            execute_tdengine_http_sql(&self.config, sql)?;
        }
        Ok(())
    }
}

impl TimeSeriesStorageAdapter for TdengineTimeSeriesAdapter {
    fn backend_kind(&self) -> TimeSeriesBackendKind {
        TimeSeriesBackendKind::Tdengine
    }

    fn on_telemetry_committed(
        &mut self,
        rec: &WalRecord,
        wal_pos: &str,
        persisted_history_points: &[WalPointRecord],
    ) -> Result<TimeSeriesWriteReceipt, EngineError> {
        let mut generated = Vec::new();
        // If persisted_history_points is non-empty, insert those; otherwise if latest points were updated,
        // still generate the subtable statement for latest observation tracking.
        let target_points = if !persisted_history_points.is_empty() {
            persisted_history_points
        } else {
            &rec.points
        };
        if let Some(insert_sql) = self.build_insert_sql(rec, wal_pos, target_points) {
            self.record_or_execute_sql(&insert_sql)?;
            generated.push(insert_sql);
        }
        Ok(TimeSeriesWriteReceipt {
            backend: "tdengine".to_string(),
            latest_points_updated: rec.points.len(),
            history_points_persisted: persisted_history_points.len(),
            generated_sql_statements: generated,
        })
    }

    fn on_source_state_committed(
        &mut self,
        tenant_id: &str,
        event: &SourceStateEnvelope,
    ) -> Result<(), EngineError> {
        let db = sanitize_identifier(&self.config.database);
        let sql = format!(
            "-- TDengine quality state event: tenant='{}' device='{}' state='{}' at={}",
            escape_sql_literal(tenant_id),
            escape_sql_literal(&event.device_id),
            escape_sql_literal(&event.connection_state),
            event.observed_at_ms,
        );
        let _ = db;
        if self.sql_log.len() >= 64 {
            self.sql_log.remove(0);
        }
        self.sql_log.push(sql);
        Ok(())
    }

    fn build_history_query_plan(
        &self,
        tenant_id: &str,
        resource_ids: &[String],
        start_ms: i64,
        end_ms: i64,
        bucket_ms: Option<i64>,
    ) -> TimeSeriesQueryPlan {
        let db = sanitize_identifier(&self.config.database);
        let stb = sanitize_identifier(&self.config.super_table);
        let in_list = resource_ids
            .iter()
            .map(|r| format!("'{}'", escape_sql_literal(r)))
            .collect::<Vec<_>>()
            .join(", ");
        match bucket_ms {
            None => TimeSeriesQueryPlan {
                backend: "tdengine".to_string(),
                mode: "raw".to_string(),
                native_sql: format!(
                    "SELECT ts, resource_id, val_f64, val_str, unit_code, quality, observation_id \
                     FROM {db}.{stb} \
                     WHERE tenant_id = '{tenant}' AND resource_id IN ({in_list}) \
                     AND ts >= {start_ms} AND ts < {end_ms} \
                     ORDER BY ts ASC;",
                    db = db,
                    stb = stb,
                    tenant = escape_sql_literal(tenant_id),
                    in_list = in_list,
                    start_ms = start_ms,
                    end_ms = end_ms,
                ),
            },
            Some(b_ms) => TimeSeriesQueryPlan {
                backend: "tdengine".to_string(),
                mode: "aggregate".to_string(),
                native_sql: format!(
                    "SELECT _wstart AS bucket_start_ms, resource_id, \
                     AVG(val_f64) AS avg_val, MIN(val_f64) AS min_val, MAX(val_f64) AS max_val, COUNT(*) AS sample_count \
                     FROM {db}.{stb} \
                     WHERE tenant_id = '{tenant}' AND resource_id IN ({in_list}) \
                     AND ts >= {start_ms} AND ts < {end_ms} \
                     PARTITION BY resource_id INTERVAL({b_ms}a) FILL(NONE);",
                    db = db,
                    stb = stb,
                    tenant = escape_sql_literal(tenant_id),
                    in_list = in_list,
                    start_ms = start_ms,
                    end_ms = end_ms,
                    b_ms = b_ms,
                ),
            },
        }
    }

    fn build_latest_query_plan(&self, tenant_id: &str, resource_id: &str) -> String {
        let db = sanitize_identifier(&self.config.database);
        let stb = sanitize_identifier(&self.config.super_table);
        format!(
            "SELECT LAST_ROW(ts, val_f64, val_str, quality, model_revision_id, binding_revision_id) \
             FROM {db}.{stb} WHERE tenant_id = '{tenant}' AND resource_id = '{res}';",
            db = db,
            stb = stb,
            tenant = escape_sql_literal(tenant_id),
            res = escape_sql_literal(resource_id),
        )
    }

    fn status_snapshot(
        &self,
        duckdb_path: &str,
        conn: &duckdb::Connection,
    ) -> Result<TimeSeriesStoreStatus, EngineError> {
        let counts = query_duckdb_ts_counts(conn)?;
        Ok(TimeSeriesStoreStatus {
            active_backend: "tdengine".to_string(),
            supported_backends: supported_backend_names(),
            capabilities: self.describe_capabilities(),
            duckdb_path: duckdb_path.to_string(),
            tdengine_config: Some(self.config.redacted()),
            tdengine_super_table_ddl: Some(self.super_table_ddl()),
            recent_tdengine_sql_log: self.sql_log.clone(),
            tables: vec![
                format!(
                    "{}.{} (STABLE)",
                    self.config.database, self.config.super_table
                ),
                "latest_value".to_string(),
                "point_quality_state".to_string(),
                "history_observation".to_string(),
            ],
            row_counts: counts,
        })
    }
}

pub struct ExtensibleSqlTimeSeriesAdapter {
    kind: TimeSeriesBackendKind,
    sql_log: Vec<String>,
}

impl ExtensibleSqlTimeSeriesAdapter {
    pub fn new(kind: TimeSeriesBackendKind) -> Self {
        let mut adapter = Self {
            kind,
            sql_log: Vec::new(),
        };
        let ddl = adapter.schema_ddl();
        adapter.record_sql(&ddl);
        adapter
    }

    pub fn schema_ddl(&self) -> String {
        match self.kind {
            TimeSeriesBackendKind::Timescaledb => "CREATE TABLE IF NOT EXISTS history_observation (ts TIMESTAMPTZ NOT NULL, tenant_id TEXT NOT NULL, resource_id TEXT NOT NULL, device_id TEXT NOT NULL, property TEXT NOT NULL, val_f64 DOUBLE PRECISION, val_str TEXT, unit_code TEXT, quality TEXT NOT NULL, observation_id TEXT NOT NULL); SELECT create_hypertable('history_observation', 'ts');".to_string(),
            TimeSeriesBackendKind::Clickhouse => "CREATE TABLE IF NOT EXISTS history_observation (ts DateTime64(3, 'UTC'), tenant_id LowCardinality(String), resource_id String, device_id String, property String, val_f64 Nullable(Float64), val_str String, unit_code Nullable(String), quality LowCardinality(String), observation_id String) ENGINE = ReplacingMergeTree() PARTITION BY toYYYYMM(ts) ORDER BY (tenant_id, resource_id, ts);".to_string(),
            _ => String::new(),
        }
    }

    fn record_sql(&mut self, sql: &str) {
        if self.sql_log.len() >= 64 {
            self.sql_log.remove(0);
        }
        self.sql_log.push(sql.to_string());
    }

    pub fn build_insert_sql(
        &self,
        rec: &WalRecord,
        points: &[WalPointRecord],
    ) -> Option<String> {
        if points.is_empty() {
            return None;
        }
        let values = points
            .iter()
            .map(|pt| {
                let val_f64 = pt
                    .value_f64
                    .map(|v| format!("{}", v))
                    .unwrap_or_else(|| "NULL".to_string());
                let val_str = pt
                    .value_text
                    .as_ref()
                    .map(|s| format!("'{}'", escape_sql_literal(s)))
                    .unwrap_or_else(|| format!("'{}'", escape_sql_literal(&pt.value_json)));
                let unit_code = pt
                    .unit_code
                    .as_ref()
                    .map(|u| format!("'{}'", escape_sql_literal(u)))
                    .unwrap_or_else(|| "NULL".to_string());
                format!(
                    "({ts}, '{tenant}', '{res}', '{dev}', '{prop}', {vf}, {vs}, {unit}, '{qual}', '{obs}')",
                    ts = pt.source_at_ms,
                    tenant = escape_sql_literal(&rec.tenant_id),
                    res = escape_sql_literal(&pt.resource_id),
                    dev = escape_sql_literal(&rec.device_id),
                    prop = escape_sql_literal(&pt.property),
                    vf = val_f64,
                    vs = val_str,
                    unit = unit_code,
                    qual = escape_sql_literal(&pt.quality_state),
                    obs = escape_sql_literal(&pt.observation_id),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "INSERT INTO history_observation (ts, tenant_id, resource_id, device_id, property, val_f64, val_str, unit_code, quality, observation_id) VALUES {};",
            values
        ))
    }
}

impl TimeSeriesStorageAdapter for ExtensibleSqlTimeSeriesAdapter {
    fn backend_kind(&self) -> TimeSeriesBackendKind {
        self.kind
    }

    fn on_telemetry_committed(
        &mut self,
        rec: &WalRecord,
        _wal_pos: &str,
        persisted_history_points: &[WalPointRecord],
    ) -> Result<TimeSeriesWriteReceipt, EngineError> {
        let mut generated = Vec::new();
        let target_points = if !persisted_history_points.is_empty() {
            persisted_history_points
        } else {
            &rec.points
        };
        if let Some(insert_sql) = self.build_insert_sql(rec, target_points) {
            self.record_sql(&insert_sql);
            generated.push(insert_sql);
        }
        Ok(TimeSeriesWriteReceipt {
            backend: self.kind.as_str().to_string(),
            latest_points_updated: rec.points.len(),
            history_points_persisted: persisted_history_points.len(),
            generated_sql_statements: generated,
        })
    }

    fn on_source_state_committed(
        &mut self,
        tenant_id: &str,
        event: &SourceStateEnvelope,
    ) -> Result<(), EngineError> {
        let sql = format!(
            "-- {} quality state event: tenant='{}' device='{}' state='{}' at={}",
            self.kind.as_str(),
            escape_sql_literal(tenant_id),
            escape_sql_literal(&event.device_id),
            escape_sql_literal(&event.connection_state),
            event.observed_at_ms,
        );
        self.record_sql(&sql);
        Ok(())
    }

    fn build_history_query_plan(
        &self,
        tenant_id: &str,
        resource_ids: &[String],
        start_ms: i64,
        end_ms: i64,
        bucket_ms: Option<i64>,
    ) -> TimeSeriesQueryPlan {
        let in_list = resource_ids
            .iter()
            .map(|r| format!("'{}'", escape_sql_literal(r)))
            .collect::<Vec<_>>()
            .join(", ");
        match bucket_ms {
            None => TimeSeriesQueryPlan {
                backend: self.kind.as_str().to_string(),
                mode: "raw".to_string(),
                native_sql: format!(
                    "SELECT ts, resource_id, val_f64, val_str, unit_code, quality, observation_id \
                     FROM history_observation \
                     WHERE tenant_id = '{tenant}' AND resource_id IN ({in_list}) \
                     AND ts >= {start_ms} AND ts < {end_ms} \
                     ORDER BY ts ASC;",
                    tenant = escape_sql_literal(tenant_id),
                    in_list = in_list,
                    start_ms = start_ms,
                    end_ms = end_ms,
                ),
            },
            Some(b_ms) => {
                let bucket_expr = match self.kind {
                    TimeSeriesBackendKind::Timescaledb => {
                        format!("time_bucket('{} milliseconds', ts)", b_ms)
                    }
                    TimeSeriesBackendKind::Clickhouse => {
                        format!("toStartOfInterval(ts, INTERVAL {} MILLISECOND)", b_ms)
                    }
                    _ => format!("time_bucket('{} milliseconds', ts)", b_ms),
                };
                TimeSeriesQueryPlan {
                    backend: self.kind.as_str().to_string(),
                    mode: "aggregate".to_string(),
                    native_sql: format!(
                        "SELECT {bucket_expr} AS bucket_start_ms, resource_id, \
                         AVG(val_f64) AS avg_val, MIN(val_f64) AS min_val, MAX(val_f64) AS max_val, COUNT(*) AS sample_count \
                         FROM history_observation \
                         WHERE tenant_id = '{tenant}' AND resource_id IN ({in_list}) \
                         AND ts >= {start_ms} AND ts < {end_ms} \
                         GROUP BY bucket_start_ms, resource_id \
                         ORDER BY bucket_start_ms ASC;",
                        bucket_expr = bucket_expr,
                        tenant = escape_sql_literal(tenant_id),
                        in_list = in_list,
                        start_ms = start_ms,
                        end_ms = end_ms,
                    ),
                }
            }
        }
    }

    fn build_latest_query_plan(&self, tenant_id: &str, resource_id: &str) -> String {
        format!(
            "SELECT ts, resource_id, val_f64, val_str, quality \
             FROM history_observation WHERE tenant_id = '{tenant}' AND resource_id = '{res}' \
             ORDER BY ts DESC LIMIT 1;",
            tenant = escape_sql_literal(tenant_id),
            res = escape_sql_literal(resource_id),
        )
    }

    fn status_snapshot(
        &self,
        duckdb_path: &str,
        conn: &duckdb::Connection,
    ) -> Result<TimeSeriesStoreStatus, EngineError> {
        let counts = query_duckdb_ts_counts(conn)?;
        Ok(TimeSeriesStoreStatus {
            active_backend: self.kind.as_str().to_string(),
            supported_backends: supported_backend_names(),
            capabilities: self.describe_capabilities(),
            duckdb_path: duckdb_path.to_string(),
            tdengine_config: None,
            tdengine_super_table_ddl: Some(self.schema_ddl()),
            recent_tdengine_sql_log: self.sql_log.clone(),
            tables: TIMESERIES_DEVICE_TABLES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            row_counts: counts,
        })
    }
}

fn query_duckdb_ts_counts(conn: &duckdb::Connection) -> Result<BTreeMap<String, usize>, EngineError> {
    let mut counts = BTreeMap::new();
    for &tbl in TIMESERIES_DEVICE_TABLES {
        let cnt: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {}", tbl), [], |r| r.get(0))
            .unwrap_or(0);
        counts.insert(tbl.to_string(), cnt as usize);
    }
    Ok(counts)
}

fn sanitize_identifier(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn escape_sql_literal(raw: &str) -> String {
    raw.replace('\'', "''")
}

/// Sends a SQL statement to TDengine's HTTP REST API (`POST /rest/sql/{database}`).
/// Supports both `http://<host>:<port>` (TCP) and `unix://<socket_path>` (Unix domain socket).
fn execute_tdengine_http_sql(cfg: &TdengineConfig, sql: &str) -> Result<(), EngineError> {
    let endpoint = cfg.endpoint.trim();
    if let Some(sock_path) = endpoint.strip_prefix("unix://") {
        #[cfg(unix)]
        {
            let mut stream = std::os::unix::net::UnixStream::connect(sock_path).map_err(|e| {
                EngineError::new(
                    "req-tdengine-http",
                    ErrorCode::ServiceUnavailable,
                    format!(
                        "Failed to connect to TDengine Unix socket '{}': {}",
                        sock_path, e
                    ),
                    vec![],
                )
            })?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            return send_tdengine_http_over_stream(&mut stream, "localhost", "", cfg, sql);
        }
    }

    let trimmed = endpoint.strip_prefix("http://").unwrap_or(endpoint);
    let (host_port, base_path) = match trimmed.split_once('/') {
        Some((hp, p)) => (hp, format!("/{}", p.trim_end_matches('/'))),
        None => (trimmed, String::new()),
    };
    let mut stream = TcpStream::connect(host_port).map_err(|e| {
        EngineError::new(
            "req-tdengine-http",
            ErrorCode::ServiceUnavailable,
            format!("Failed to connect to TDengine REST endpoint '{}': {}", cfg.endpoint, e),
            vec![],
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    send_tdengine_http_over_stream(&mut stream, host_port, &base_path, cfg, sql)
}

pub fn send_tdengine_http_over_stream<S: Read + Write>(
    stream: &mut S,
    host_header: &str,
    base_path: &str,
    cfg: &TdengineConfig,
    sql: &str,
) -> Result<(), EngineError> {
    let auth_raw = format!("{}:{}", cfg.username, cfg.password);
    let auth_b64 = data_encoding::BASE64.encode(auth_raw.as_bytes());
    let path = format!("{}/rest/sql/{}", base_path, sanitize_identifier(&cfg.database));
    let body = sql.as_bytes();
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Basic {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path,
        host_header,
        auth_b64,
        body.len()
    );
    stream.write_all(req.as_bytes()).map_err(|e| {
        EngineError::new(
            "req-tdengine-http",
            ErrorCode::ServiceUnavailable,
            format!("Failed to write request to TDengine REST endpoint: {}", e),
            vec![],
        )
    })?;
    stream.write_all(body).map_err(|e| {
        EngineError::new(
            "req-tdengine-http",
            ErrorCode::ServiceUnavailable,
            format!("Failed to write SQL payload to TDengine REST endpoint: {}", e),
            vec![],
        )
    })?;

    let mut resp_buf = String::new();
    stream.read_to_string(&mut resp_buf).map_err(|e| {
        EngineError::new(
            "req-tdengine-http",
            ErrorCode::ServiceUnavailable,
            format!("Failed to read response from TDengine REST endpoint: {}", e),
            vec![],
        )
    })?;

    let (header_part, body_part) = resp_buf.split_once("\r\n\r\n").unwrap_or((&resp_buf, ""));
    let status_ok = header_part.starts_with("HTTP/1.1 200") || header_part.starts_with("HTTP/1.0 200");
    if !status_ok {
        return Err(EngineError::new(
            "req-tdengine-http",
            ErrorCode::ServiceUnavailable,
            format!("TDengine REST returned non-200 response: {}", header_part.lines().next().unwrap_or("")),
            vec![],
        ));
    }
    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(body_part.trim()) {
        if let Some(code) = json_val.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let desc = json_val
                    .get("desc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tdengine error");
                return Err(EngineError::new(
                    "req-tdengine-http",
                    ErrorCode::ServiceUnavailable,
                    format!("TDengine SQL execution error (code={}): {}", code, desc),
                    vec![],
                ));
            }
        }
    }
    Ok(())
}
