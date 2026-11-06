use crate::config::EngineConfig;
use crate::d00::{TelemetryEnvelope, WalPosition};
use crate::error::{EngineError, ErrorCode};
use crate::health::{check_engine_health, EngineState, HealthStatus};
use crate::ingest::{IngestService, SourceStateEnvelope};
use crate::schema::init_core_schema;
use crate::scope::VerifiedScopeContext;
use crate::wal::{AppWal, WalRecoveryOutcome, WalRecord};
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

pub struct EngineRuntime {
    pub config: EngineConfig,
    conn: duckdb::Connection,
    wal: AppWal,
    next_ingest_seq: u64,
    pub stream_epoch: String,
    pub stream_seq: u64,
    state: EngineState,
}

impl EngineRuntime {
    pub fn open(config: EngineConfig) -> Result<Self, EngineError> {
        let conn = duckdb::Connection::open(config.db_path())?;
        init_core_schema(&conn)?;
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

        Ok(Self {
            config,
            conn,
            wal,
            next_ingest_seq: (max_ingest_seq + 1) as u64,
            stream_epoch: "se-20261106-01".to_string(),
            stream_seq: max_stream_seq as u64,
            state: EngineState::Running,
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
        let cnt: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM history_observation",
            [],
            |r| r.get(0),
        )?;
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
        IngestService::verify_source_grant(&self.conn, ctx, &msg.source_id, &msg.device_id, now_ms)?;

        let existing: Option<(String, String, String, String, String)> = self
            .conn
            .query_row(
                "SELECT ingest_id, payload_hash, wal_position, history_status, observation_ids_json
                 FROM applied_message
                 WHERE tenant_id = ? AND source_id = ? AND source_epoch = ? AND message_id = ?",
                params![ctx.tenant_id, msg.source_id, msg.source_epoch, msg.message_id],
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
                params![rec.tenant_id, rec.source_id, rec.source_epoch, rec.message_id],
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

            self.conn.execute(
                "INSERT INTO ingest_status (ingest_id, tenant_id, message_id, status, history_status, wal_position, updated_at_ms)
                 VALUES (?, ?, ?, 'accepted_durable', 'committed', ?, ?)",
                params![
                    rec.ingest_id,
                    rec.tenant_id,
                    rec.message_id,
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
        let validated = IngestService::validate_source_state(&self.conn, ctx, &evt, now_ms, self.config.max_future_skew_ms)?;

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
                    let resource_id = format!(
                        "res-{}-{}",
                        evt.device_id,
                        ps.property.replace('_', "-")
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
            Ok(()) => self.conn.execute_batch("COMMIT")?,
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
        let max_rev: i64 = self.conn
            .query_row("SELECT COALESCE(MAX(resource_revision), 0) FROM latest_value", [], |r| r.get(0))
            .unwrap_or(0);
        if max_rev as u64 > self.stream_seq {
            self.stream_seq = max_rev as u64;
        }

        Ok(RecoverySummary {
            replayed_messages: replayed,
            latest_checkpoint: last_pos,
        })
    }
}
