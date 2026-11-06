use crate::wal::WalPosition;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Inflight,
    Committed,
    Retryable,
    PermanentRejected,
    Unknown,
    IsolatedGap,
}

impl DeliveryStatus {
    /// Only `Committed` or explicitly authorized `IsolatedGap` allows the contiguous prefix to advance past this item.
    pub fn allows_checkpoint_advance(&self) -> bool {
        matches!(self, DeliveryStatus::Committed | DeliveryStatus::IsolatedGap)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryStateRow {
    pub tenant_id: String,
    pub target_id: String,
    pub seq_no: u64,
    pub wal_position: WalPosition,
    pub observation_id: String,
    pub resource_id: String,
    pub source_at_ms: i64,
    pub value_f64: f64,
    pub payload_hash: String,
    pub state: DeliveryStatus,
    pub attempt_count: u32,
    pub last_error_code: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCheckpointRow {
    pub tenant_id: String,
    pub target_id: String,
    pub stream_id: String,
    pub contiguous_seq_no: u64,
    pub contiguous_wal_position: WalPosition,
    pub revision: u64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryGapRow {
    pub tenant_id: String,
    pub gap_id: String,
    pub target_id: String,
    pub seq_no: u64,
    pub observation_id: String,
    pub resource_id: String,
    pub source_at_ms: i64,
    pub wal_position: WalPosition,
    pub reason: String,
    pub decision: String,
    pub approved_by: String,
    pub created_at_ms: i64,
    pub closed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryQueryCoverage {
    pub contiguous_seq_no: u64,
    pub contiguous_wal_position: WalPosition,
    pub contiguous_watermark_ms: i64,
    pub known_gaps: Vec<DeliveryGapRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryQueryResponseWithCoverage {
    pub target_id: String,
    pub consistency: String,
    pub partial: bool,
    pub warnings: Vec<String>,
    pub coverage: HistoryQueryCoverage,
    pub items: Vec<DeliveryStateRow>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedCoordinatorStore {
    pub deliveries: BTreeMap<String, BTreeMap<u64, DeliveryStateRow>>,
    pub checkpoints: BTreeMap<String, TargetCheckpointRow>,
    pub gaps: BTreeMap<String, Vec<DeliveryGapRow>>,
}

pub struct NonContiguousCheckpointCoordinator {
    store_path: PathBuf,
    store: PersistedCoordinatorStore,
}

impl NonContiguousCheckpointCoordinator {
    pub fn open(store_path: PathBuf) -> Result<Self, String> {
        let store = if store_path.exists() {
            let bytes = fs::read(&store_path).map_err(|e| e.to_string())?;
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
        } else {
            PersistedCoordinatorStore::default()
        };
        let mut coord = Self { store_path, store };
        coord.normalize_inflight_on_restart()?;
        Ok(coord)
    }

    fn persist(&self) -> Result<(), String> {
        if let Some(parent) = self.store_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp_path = self.store_path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&self.store).map_err(|e| e.to_string())?;
        {
            let mut f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| e.to_string())?;
            f.write_all(&bytes).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp_path, &self.store_path).map_err(|e| e.to_string())?;
        if let Some(parent) = self.store_path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }

    /// Per Storage Adapter Contract Section 6: On process restart, any `inflight` item transitions to `unknown`.
    /// Already `committed` items (even beyond a hole, e.g. 103 when 102 is unknown) keep their `committed` status.
    fn normalize_inflight_on_restart(&mut self) -> Result<(), String> {
        let mut mutated = false;
        for target_map in self.store.deliveries.values_mut() {
            for row in target_map.values_mut() {
                if row.state == DeliveryStatus::Inflight {
                    row.state = DeliveryStatus::Unknown;
                    row.last_error_code = Some("RESTART_WHILE_INFLIGHT".to_string());
                    mutated = true;
                }
            }
        }
        if mutated {
            self.persist()?;
        }
        Ok(())
    }

    pub fn register_target(&mut self, tenant_id: &str, target_id: &str, now_ms: i64) -> Result<(), String> {
        self.store
            .checkpoints
            .entry(target_id.to_string())
            .or_insert_with(|| TargetCheckpointRow {
                tenant_id: tenant_id.to_string(),
                target_id: target_id.to_string(),
                stream_id: "default".to_string(),
                contiguous_seq_no: 0,
                contiguous_wal_position: WalPosition::ZERO,
                revision: 1,
                updated_at_ms: now_ms,
            });
        self.store
            .deliveries
            .entry(target_id.to_string())
            .or_default();
        self.store.gaps.entry(target_id.to_string()).or_default();
        self.persist()
    }

    pub fn record_delivery_state(&mut self, row: DeliveryStateRow) -> Result<TargetCheckpointRow, String> {
        let target_id = row.target_id.clone();
        let tenant_id = row.tenant_id.clone();
        let now_ms = row.updated_at_ms;
        self.store
            .deliveries
            .entry(target_id.clone())
            .or_default()
            .insert(row.seq_no, row);
        let cp = self.recompute_contiguous_checkpoint(&tenant_id, &target_id, now_ms)?;
        self.persist()?;
        Ok(cp)
    }

    /// Computes the contiguous completed prefix for `target_id`.
    /// Never crosses any hole where `state` is `Pending`, `Inflight`, `Retryable`, `PermanentRejected`, or `Unknown`.
    pub fn recompute_contiguous_checkpoint(
        &mut self,
        tenant_id: &str,
        target_id: &str,
        now_ms: i64,
    ) -> Result<TargetCheckpointRow, String> {
        let deliveries = self
            .store
            .deliveries
            .get(target_id)
            .ok_or_else(|| format!("unknown target_id: {}", target_id))?;

        let mut contiguous_seq = 0u64;
        let mut contiguous_pos = WalPosition::ZERO;
        let mut expected_next_seq: Option<u64> = None;

        for (&seq_no, row) in deliveries.iter() {
            if let Some(exp) = expected_next_seq {
                if seq_no != exp {
                    // Sequence gap in delivery table itself blocks advancement
                    break;
                }
            }
            if row.state.allows_checkpoint_advance() {
                contiguous_seq = seq_no;
                contiguous_pos = row.wal_position;
                expected_next_seq = Some(seq_no + 1);
            } else {
                // Hole encountered (e.g. 102 is Unknown or PermanentRejected): stop immediately!
                break;
            }
        }

        let cp = self
            .store
            .checkpoints
            .entry(target_id.to_string())
            .or_insert_with(|| TargetCheckpointRow {
                tenant_id: tenant_id.to_string(),
                target_id: target_id.to_string(),
                stream_id: "default".to_string(),
                contiguous_seq_no: 0,
                contiguous_wal_position: WalPosition::ZERO,
                revision: 0,
                updated_at_ms: now_ms,
            });

        if cp.contiguous_seq_no != contiguous_seq || cp.contiguous_wal_position != contiguous_pos {
            cp.contiguous_seq_no = contiguous_seq;
            cp.contiguous_wal_position = contiguous_pos;
            cp.revision += 1;
            cp.updated_at_ms = now_ms;
        }
        Ok(cp.clone())
    }

    pub fn get_checkpoint(&self, target_id: &str) -> Option<TargetCheckpointRow> {
        self.store.checkpoints.get(target_id).cloned()
    }

    pub fn get_delivery(&self, target_id: &str, seq_no: u64) -> Option<DeliveryStateRow> {
        self.store
            .deliveries
            .get(target_id)
            .and_then(|m| m.get(&seq_no))
            .cloned()
    }

    /// Returns items after `contiguous_seq_no` that are NOT already `Committed` or `IsolatedGap`.
    /// Demonstrates that `103 (Committed)` is NOT re-delivered after restart even when `102` is `Unknown`.
    pub fn items_requiring_recovery_or_retry(&self, target_id: &str) -> Vec<DeliveryStateRow> {
        let cp_seq = self
            .store
            .checkpoints
            .get(target_id)
            .map(|c| c.contiguous_seq_no)
            .unwrap_or(0);
        let Some(deliveries) = self.store.deliveries.get(target_id) else {
            return Vec::new();
        };
        deliveries
            .values()
            .filter(|row| row.seq_no > cp_seq && !row.state.allows_checkpoint_advance())
            .cloned()
            .collect()
    }

    /// Reconcile an `Unknown` delivery via target-side query-back (`observation_id` + `payload_hash` verification).
    pub fn reconcile_unknown_by_query_back(
        &mut self,
        target_id: &str,
        seq_no: u64,
        observed_remote_payload_hash: &str,
        now_ms: i64,
    ) -> Result<TargetCheckpointRow, String> {
        let tenant_id = {
            let deliveries = self
                .store
                .deliveries
                .get_mut(target_id)
                .ok_or_else(|| format!("unknown target_id: {}", target_id))?;
            let row = deliveries
                .get_mut(&seq_no)
                .ok_or_else(|| format!("unknown seq_no {} for target {}", seq_no, target_id))?;
            if row.payload_hash != observed_remote_payload_hash {
                return Err(format!(
                    "remote payload_hash mismatch for observation {}: expected {}, got {}",
                    row.observation_id, row.payload_hash, observed_remote_payload_hash
                ));
            }
            row.state = DeliveryStatus::Committed;
            row.last_error_code = None;
            row.updated_at_ms = now_ms;
            row.tenant_id.clone()
        };

        let cp = self.recompute_contiguous_checkpoint(&tenant_id, target_id, now_ms)?;
        self.persist()?;
        Ok(cp)
    }

    /// Explicitly authorize isolation of an unrecoverable hole (`Unknown` or `PermanentRejected`) as a `delivery_gap`.
    /// Requires non-empty `approved_by` and `reason`; records a durable `DeliveryGapRow` and discloses it in queries.
    pub fn authorize_delivery_gap(
        &mut self,
        gap_id: &str,
        target_id: &str,
        seq_no: u64,
        reason: &str,
        decision: &str,
        approved_by: &str,
        now_ms: i64,
    ) -> Result<(TargetCheckpointRow, DeliveryGapRow), String> {
        if approved_by.trim().is_empty() || reason.trim().is_empty() {
            return Err("Explicit authorization (approved_by and reason) is required to isolate a delivery_gap".to_string());
        }

        let (tenant_id, gap_row) = {
            let deliveries = self
                .store
                .deliveries
                .get_mut(target_id)
                .ok_or_else(|| format!("unknown target_id: {}", target_id))?;
            let row = deliveries
                .get_mut(&seq_no)
                .ok_or_else(|| format!("unknown seq_no {} for target {}", seq_no, target_id))?;
            if row.state == DeliveryStatus::Committed {
                return Err(format!(
                    "seq_no {} is already committed and cannot be marked as a gap",
                    seq_no
                ));
            }
            row.state = DeliveryStatus::IsolatedGap;
            row.updated_at_ms = now_ms;

            let gap = DeliveryGapRow {
                tenant_id: row.tenant_id.clone(),
                gap_id: gap_id.to_string(),
                target_id: target_id.to_string(),
                seq_no,
                observation_id: row.observation_id.clone(),
                resource_id: row.resource_id.clone(),
                source_at_ms: row.source_at_ms,
                wal_position: row.wal_position,
                reason: reason.to_string(),
                decision: decision.to_string(),
                approved_by: approved_by.to_string(),
                created_at_ms: now_ms,
                closed_at_ms: None,
            };
            (row.tenant_id.clone(), gap)
        };

        self.store
            .gaps
            .entry(target_id.to_string())
            .or_default()
            .push(gap_row.clone());

        let cp = self.recompute_contiguous_checkpoint(&tenant_id, target_id, now_ms)?;
        self.persist()?;
        Ok((cp, gap_row))
    }

    /// Returns the maximum WAL position safe for garbage collection across all required targets.
    pub fn safe_wal_recycle_watermark(&self, required_targets: &[&str]) -> WalPosition {
        let mut min_pos: Option<WalPosition> = None;
        for &target_id in required_targets {
            let pos = self
                .store
                .checkpoints
                .get(target_id)
                .map(|c| c.contiguous_wal_position)
                .unwrap_or(WalPosition::ZERO);
            min_pos = Some(match min_pos {
                None => pos,
                Some(cur) => cur.min(pos),
            });
        }
        min_pos.unwrap_or(WalPosition::ZERO)
    }

    /// Queries committed observations on `target_id` in `[start_ms, end_ms)` and discloses coverage + known `delivery_gap`s.
    pub fn query_history_with_coverage(
        &self,
        target_id: &str,
        resource_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<HistoryQueryResponseWithCoverage, String> {
        let cp = self
            .store
            .checkpoints
            .get(target_id)
            .ok_or_else(|| format!("unknown target_id: {}", target_id))?;
        let deliveries = self
            .store
            .deliveries
            .get(target_id)
            .ok_or_else(|| format!("unknown target_id: {}", target_id))?;

        let mut items: Vec<DeliveryStateRow> = deliveries
            .values()
            .filter(|r| {
                r.resource_id == resource_id
                    && r.source_at_ms >= start_ms
                    && r.source_at_ms < end_ms
                    && r.state == DeliveryStatus::Committed
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| {
            (a.source_at_ms, &a.observation_id).cmp(&(b.source_at_ms, &b.observation_id))
        });

        let known_gaps: Vec<DeliveryGapRow> = self
            .store
            .gaps
            .get(target_id)
            .map(|gaps| {
                gaps.iter()
                    .filter(|g| {
                        g.resource_id == resource_id
                            && g.source_at_ms >= start_ms
                            && g.source_at_ms < end_ms
                            && g.closed_at_ms.is_none()
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let has_unresolved_hole = deliveries.values().any(|r| {
            r.resource_id == resource_id
                && r.source_at_ms >= start_ms
                && r.source_at_ms < end_ms
                && !r.state.allows_checkpoint_advance()
        });

        let contiguous_watermark_ms = deliveries
            .get(&cp.contiguous_seq_no)
            .map(|r| r.source_at_ms)
            .unwrap_or(0);

        let mut warnings = Vec::new();
        if !known_gaps.is_empty() {
            warnings.push("DELIVERY_GAP_PRESENT".to_string());
        }
        if has_unresolved_hole {
            warnings.push("UNRESOLVED_DELIVERY_HOLE_PRESENT".to_string());
        }

        let partial = !known_gaps.is_empty() || has_unresolved_hole;

        Ok(HistoryQueryResponseWithCoverage {
            target_id: target_id.to_string(),
            consistency: "strong_snapshot".to_string(),
            partial,
            warnings,
            coverage: HistoryQueryCoverage {
                contiguous_seq_no: cp.contiguous_seq_no,
                contiguous_wal_position: cp.contiguous_wal_position,
                contiguous_watermark_ms,
                known_gaps,
            },
            items,
        })
    }
}
