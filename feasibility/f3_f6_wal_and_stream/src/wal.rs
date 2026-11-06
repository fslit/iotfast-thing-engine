use crc32fast::Hasher as Crc32Hasher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const WAL_MAGIC: [u8; 4] = *b"IWAL";
pub const WAL_FORMAT_VERSION: u16 = 1;
pub const WAL_FRAME_HEADER_LEN: usize = 48; // 4(magic) + 2(ver) + 2(flags) + 4(len) + 4(crc32) + 32(sha256)

/// Fixed-width comparable WAL position: `"{segment_no:016x}:{record_offset:016x}"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WalPosition {
    pub segment_no: u64,
    pub record_offset: u64,
}

impl WalPosition {
    pub const ZERO: Self = Self {
        segment_no: 0,
        record_offset: 0,
    };

    pub fn new(segment_no: u64, record_offset: u64) -> Self {
        Self {
            segment_no,
            record_offset,
        }
    }

    pub fn format_fixed(&self) -> String {
        format!("{:016x}:{:016x}", self.segment_no, self.record_offset)
    }
}

impl fmt::Display for WalPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}:{:016x}", self.segment_no, self.record_offset)
    }
}

impl FromStr for WalPosition {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 33 || s.as_bytes()[16] != b':' {
            return Err(format!(
                "invalid wal_position length or separator (expected 33 chars '16hex:16hex'): {}",
                s
            ));
        }
        let seg_str = &s[0..16];
        let off_str = &s[17..33];
        if !seg_str
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !off_str
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!(
                "wal_position must use lowercase fixed-width hex: {}",
                s
            ));
        }
        let segment_no = u64::from_str_radix(seg_str, 16).map_err(|e| e.to_string())?;
        let record_offset = u64::from_str_radix(off_str, 16).map_err(|e| e.to_string())?;
        Ok(Self {
            segment_no,
            record_offset,
        })
    }
}

impl Serialize for WalPosition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.format_fixed())
    }
}

impl<'de> Deserialize<'de> for WalPosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        WalPosition::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncPolicy {
    SyncData,
    SyncAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DurabilityPreference {
    Durable,
    Volatile,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageKey {
    pub tenant_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub message_id: String,
}

impl MessageKey {
    pub fn canonical_string(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.tenant_id, self.source_id, self.source_epoch, self.message_id
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryPointInput {
    pub property: String,
    pub resource_id: String,
    pub value_f64: f64,
    pub unit_code: String,
    pub source_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub quality_state: String,
    pub source_quality_code: Option<String>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryIngestRequest {
    pub request_id: String,
    pub tenant_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub message_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub received_at_ms: i64,
    pub durability: DurabilityPreference,
    pub points: Vec<TelemetryPointInput>,
}

impl TelemetryIngestRequest {
    pub fn message_key(&self) -> MessageKey {
        MessageKey {
            tenant_id: self.tenant_id.clone(),
            source_id: self.source_id.clone(),
            source_epoch: self.source_epoch.clone(),
            message_id: self.message_id.clone(),
        }
    }

    /// Computes canonical `sha256-jcs-v1` payload hash excluding `replayed`, `request_id`, and `received_at_ms`.
    pub fn compute_payload_hash(&self) -> String {
        let mut sorted_points: Vec<serde_json::Value> = self
            .points
            .iter()
            .map(|p| {
                serde_json::json!({
                    "property": p.property,
                    "quality_state": p.quality_state,
                    "resource_id": p.resource_id,
                    "source_at_ms": p.source_at_ms,
                    "source_quality_code": p.source_quality_code,
                    "source_seq": p.source_seq,
                    "source_time_origin": p.source_time_origin,
                    "unit_code": p.unit_code,
                    "value_f64": format!("{:.6}", p.value_f64),
                })
            })
            .collect();
        sorted_points.sort_by(|a, b| a["property"].as_str().cmp(&b["property"].as_str()));

        let canonical = serde_json::json!({
            "binding_revision_id": self.binding_revision_id,
            "device_id": self.device_id,
            "message_id": self.message_id,
            "model_revision_id": self.model_revision_id,
            "points": sorted_points,
            "source_epoch": self.source_epoch,
            "source_id": self.source_id,
            "spec": "iotfast.telemetry.v1",
            "tenant_id": self.tenant_id,
        });
        let bytes = serde_json::to_vec(&canonical).expect("canonical JSON serialization");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("sha256-jcs-v1:{}", hex::encode(hasher.finalize()))
    }

    pub fn derive_ingest_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"iotfast.ingest_id.v1:");
        hasher.update(self.tenant_id.as_bytes());
        hasher.update(b":");
        hasher.update(self.source_id.as_bytes());
        hasher.update(b":");
        hasher.update(self.source_epoch.as_bytes());
        hasher.update(b":");
        hasher.update(self.message_id.as_bytes());
        let digest = hex::encode(hasher.finalize());
        format!("ing-{}", &digest[..24])
    }

    pub fn derive_observation_id(&self, property: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"iotfast.observation_id.v1:");
        hasher.update(self.tenant_id.as_bytes());
        hasher.update(b":");
        hasher.update(self.source_id.as_bytes());
        hasher.update(b":");
        hasher.update(self.source_epoch.as_bytes());
        hasher.update(b":");
        hasher.update(self.message_id.as_bytes());
        hasher.update(b":");
        hasher.update(property.as_bytes());
        let digest = hex::encode(hasher.finalize());
        format!("obs-{}", &digest[..26])
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedObservation {
    pub observation_id: String,
    pub tenant_id: String,
    pub resource_id: String,
    pub device_id: String,
    pub property_identifier: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub source_seq: String,
    pub source_at_ms: i64,
    pub received_at_ms: i64,
    pub source_time_origin: String,
    pub value_f64: f64,
    pub unit_code: String,
    pub quality_state: String,
    pub source_quality_code: Option<String>,
    pub replayed: bool,
    pub ingested_wal_position: WalPosition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalRecordPayload {
    pub wal_position: WalPosition,
    pub sequence_no: u64,
    pub ingest_id: String,
    pub message_key: MessageKey,
    pub payload_hash: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub received_at_ms: i64,
    pub observations: Vec<NormalizedObservation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalConfig {
    pub wal_dir: PathBuf,
    pub state_dir: PathBuf,
    pub sync_policy: SyncPolicy,
    pub soft_watermark_bytes: u64,
    pub hard_watermark_bytes: u64,
    pub max_record_bytes: u64,
    pub deadband_abs: f64,
}

impl WalConfig {
    pub fn new(wal_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            wal_dir,
            state_dir,
            sync_policy: SyncPolicy::SyncAll,
            soft_watermark_bytes: 64 * 1024,
            hard_watermark_bytes: 128 * 1024,
            max_record_bytes: 32 * 1024,
            deadband_abs: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultInjectionPoint {
    None,
    /// F01: Crash before any bytes are written to the WAL segment file.
    F01BeforeWalWrite,
    /// F02: Write frame header + partial payload without fsync, then crash before completion.
    F02PartialFrameBeforeSync { truncate_payload_bytes: usize },
    /// F03: Write complete frame and fsync to disk, then crash BEFORE returning HTTP 202 ACK or applying to DB.
    F03AfterFsyncBeforeAck,
    /// F04a: Crash during DB transaction before DB commit (WAL is synced, DB not yet committed).
    F04aDbRollbackBeforeCommit,
    /// F04b: DB transaction (`history_observations` + `applied_messages`) commits to disk, then crash BEFORE `contiguous_wal_position` checkpoint advances.
    F04bAfterDbCommitBeforeCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestSuccessResponse {
    pub http_status: u16,
    pub acceptance: String, // "accepted_durable" | "accepted_volatile" | "duplicate"
    pub ingest_id: String,
    pub wal_position: Option<WalPosition>,
    pub history_status: String, // "committed" | "pending"
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestErrorResponse {
    pub http_status: u16,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedMessageRow {
    pub message_key: MessageKey,
    pub payload_hash: String,
    pub ingest_id: String,
    pub wal_position: WalPosition,
    pub applied_at_ms: i64,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastPersistedBaseline {
    pub resource_id: String,
    pub last_observation_id: String,
    pub last_value_f64: f64,
    pub last_persisted_at_ms: i64,
    pub quality_state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatestValueRow {
    pub resource_id: String,
    pub observation_id: String,
    pub value_f64: f64,
    pub value_sampled_at_ms: i64,
    pub received_at_ms: i64,
    pub source_epoch: String,
    pub source_seq: u64,
    pub quality_state: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DurableDbState {
    pub applied_messages: BTreeMap<String, AppliedMessageRow>,
    pub history_observations: Vec<NormalizedObservation>,
    pub last_persisted_values: BTreeMap<String, LastPersistedBaseline>,
    pub latest_values: BTreeMap<String, LatestValueRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointState {
    pub target_id: String,
    pub stream_id: String,
    pub contiguous_wal_position: WalPosition,
    pub revision: u64,
    pub updated_at_ms: i64,
}

impl Default for CheckpointState {
    fn default() -> Self {
        Self {
            target_id: "local-duckdb".to_string(),
            stream_id: "default".to_string(),
            contiguous_wal_position: WalPosition::ZERO,
            revision: 0,
            updated_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryAction {
    TruncatedIncompleteTail {
        segment_no: u64,
        record_offset: u64,
        truncated_bytes: u64,
    },
    ReplayedIntoDb {
        wal_position: WalPosition,
        ingest_id: String,
        observations_applied: usize,
    },
    SkippedAlreadyApplied {
        wal_position: WalPosition,
        ingest_id: String,
    },
    CheckpointAdvanced {
        from: WalPosition,
        to: WalPosition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryAlert {
    MiddleRecordCorrupted {
        segment_no: u64,
        record_offset: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineHealthState {
    Ready,
    SoftWatermarkWarning,
    HardWatermarkBackpressure,
    HaltedOnWalCorruption,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub initial_checkpoint: WalPosition,
    pub final_checkpoint: WalPosition,
    pub scanned_records: usize,
    pub replayed_records: usize,
    pub skipped_already_applied: usize,
    pub actions: Vec<RecoveryAction>,
    pub alerts: Vec<RecoveryAlert>,
    pub health_state: EngineHealthState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalDedupEntry {
    pub ingest_id: String,
    pub payload_hash: String,
    pub wal_position: WalPosition,
}

/// Encode a `WalRecordPayload` into a binary frame with 48-byte header (magic, version, flags, payload_len, crc32, sha256).
pub fn encode_wal_frame(record: &WalRecordPayload) -> Result<Vec<u8>, String> {
    let payload_bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
    let payload_len = u32::try_from(payload_bytes.len())
        .map_err(|_| "payload exceeds u32::MAX bytes".to_string())?;
    let version = WAL_FORMAT_VERSION;
    let flags: u16 = 0;

    let mut sha_hasher = Sha256::new();
    sha_hasher.update(&payload_bytes);
    let sha256_bytes: [u8; 32] = sha_hasher.finalize().into();

    let mut crc_hasher = Crc32Hasher::new();
    crc_hasher.update(&version.to_le_bytes());
    crc_hasher.update(&flags.to_le_bytes());
    crc_hasher.update(&payload_len.to_le_bytes());
    crc_hasher.update(&sha256_bytes);
    crc_hasher.update(&payload_bytes);
    let crc32 = crc_hasher.finalize();

    let mut frame = Vec::with_capacity(WAL_FRAME_HEADER_LEN + payload_bytes.len());
    frame.extend_from_slice(&WAL_MAGIC);
    frame.extend_from_slice(&version.to_le_bytes());
    frame.extend_from_slice(&flags.to_le_bytes());
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.extend_from_slice(&crc32.to_le_bytes());
    frame.extend_from_slice(&sha256_bytes);
    frame.extend_from_slice(&payload_bytes);
    Ok(frame)
}

#[derive(Debug)]
enum FrameScanOutcome {
    Valid {
        record: WalRecordPayload,
        next_offset: u64,
    },
    IncompleteTail {
        record_offset: u64,
        remaining_bytes: u64,
    },
    Corrupted {
        record_offset: u64,
        reason: String,
    },
}

fn scan_single_frame(
    file: &mut File,
    file_len: u64,
    offset: u64,
    is_last_segment: bool,
) -> Result<FrameScanOutcome, String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let remaining = file_len.saturating_sub(offset);
    if remaining < WAL_FRAME_HEADER_LEN as u64 {
        if is_last_segment {
            return Ok(FrameScanOutcome::IncompleteTail {
                record_offset: offset,
                remaining_bytes: remaining,
            });
        } else {
            return Ok(FrameScanOutcome::Corrupted {
                record_offset: offset,
                reason: format!(
                    "non-final segment has trailing partial header ({} bytes < {})",
                    remaining, WAL_FRAME_HEADER_LEN
                ),
            });
        }
    }

    let mut header = [0u8; WAL_FRAME_HEADER_LEN];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;

    if header[0..4] != WAL_MAGIC {
        return Ok(FrameScanOutcome::Corrupted {
            record_offset: offset,
            reason: format!("invalid frame magic at offset {}: {:?}", offset, &header[0..4]),
        });
    }

    let version = u16::from_le_bytes([header[4], header[5]]);
    let flags = u16::from_le_bytes([header[6], header[7]]);
    if version != WAL_FORMAT_VERSION {
        return Ok(FrameScanOutcome::Corrupted {
            record_offset: offset,
            reason: format!("unsupported frame version {} at offset {}", version, offset),
        });
    }

    let payload_len = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as u64;
    let expected_crc32 = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let mut expected_sha256 = [0u8; 32];
    expected_sha256.copy_from_slice(&header[16..48]);

    let frame_total_len = (WAL_FRAME_HEADER_LEN as u64) + payload_len;
    if offset + frame_total_len > file_len {
        if is_last_segment {
            return Ok(FrameScanOutcome::IncompleteTail {
                record_offset: offset,
                remaining_bytes: remaining,
            });
        } else {
            return Ok(FrameScanOutcome::Corrupted {
                record_offset: offset,
                reason: format!(
                    "non-final segment truncated inside payload (expected {} bytes, available {})",
                    frame_total_len, remaining
                ),
            });
        }
    }

    let mut payload_bytes = vec![0u8; payload_len as usize];
    file.read_exact(&mut payload_bytes)
        .map_err(|e| e.to_string())?;

    let mut crc_hasher = Crc32Hasher::new();
    crc_hasher.update(&version.to_le_bytes());
    crc_hasher.update(&flags.to_le_bytes());
    crc_hasher.update(&(payload_len as u32).to_le_bytes());
    crc_hasher.update(&expected_sha256);
    crc_hasher.update(&payload_bytes);
    let actual_crc32 = crc_hasher.finalize();

    let mut sha_hasher = Sha256::new();
    sha_hasher.update(&payload_bytes);
    let actual_sha256: [u8; 32] = sha_hasher.finalize().into();

    if actual_crc32 != expected_crc32 || actual_sha256 != expected_sha256 {
        return Ok(FrameScanOutcome::Corrupted {
            record_offset: offset,
            reason: format!(
                "checksum mismatch at offset {} (crc32 expected={:#010x} actual={:#010x}, sha256_match={})",
                offset,
                expected_crc32,
                actual_crc32,
                actual_sha256 == expected_sha256
            ),
        });
    }

    let record: WalRecordPayload = serde_json::from_slice(&payload_bytes).map_err(|e| {
        format!(
            "corrupted JSON payload despite valid checksum at offset {}: {}",
            offset, e
        )
    })?;

    Ok(FrameScanOutcome::Valid {
        record,
        next_offset: offset + frame_total_len,
    })
}

pub struct ApplicationWalEngine {
    pub config: WalConfig,
    pub current_segment_no: u64,
    pub next_sequence_no: u64,
    pub wal_dedup_index: BTreeMap<String, WalDedupEntry>,
    pub db_state: DurableDbState,
    pub checkpoint_state: CheckpointState,
    pub health_state: EngineHealthState,
}

impl ApplicationWalEngine {
    pub fn open_and_recover(config: WalConfig) -> Result<(Self, RecoveryReport), String> {
        fs::create_dir_all(&config.wal_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&config.state_dir).map_err(|e| e.to_string())?;

        let db_state: DurableDbState = Self::read_json_or_default(&config.state_dir.join("db_state.json"))?;
        let checkpoint_state: CheckpointState =
            Self::read_json_or_default(&config.state_dir.join("checkpoint_state.json"))?;

        let mut engine = Self {
            config,
            current_segment_no: 1,
            next_sequence_no: 1,
            wal_dedup_index: BTreeMap::new(),
            db_state,
            checkpoint_state,
            health_state: EngineHealthState::Ready,
        };

        // Populate dedup index from already committed DB applied_messages first
        for (k, row) in &engine.db_state.applied_messages {
            engine.wal_dedup_index.insert(
                k.clone(),
                WalDedupEntry {
                    ingest_id: row.ingest_id.clone(),
                    payload_hash: row.payload_hash.clone(),
                    wal_position: row.wal_position,
                },
            );
        }

        let report = engine.recover_and_replay()?;
        Ok((engine, report))
    }

    fn read_json_or_default<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T, String> {
        if !path.exists() {
            return Ok(T::default());
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
        let tmp_path = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }

    pub fn segment_path(&self, segment_no: u64) -> PathBuf {
        self.config
            .wal_dir
            .join(format!("wal-{:016x}.log", segment_no))
    }

    pub fn list_segments(&self) -> Result<Vec<(u64, PathBuf)>, String> {
        let mut segments = Vec::new();
        if !self.config.wal_dir.exists() {
            return Ok(segments);
        }
        for entry in fs::read_dir(&self.config.wal_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(hex_part) = name
                .strip_prefix("wal-")
                .and_then(|s| s.strip_suffix(".log"))
            {
                if hex_part.len() == 16 {
                    if let Ok(seg_no) = u64::from_str_radix(hex_part, 16) {
                        segments.push((seg_no, entry.path()));
                    }
                }
            }
        }
        segments.sort_by_key(|(seg_no, _)| *seg_no);
        Ok(segments)
    }

    pub fn total_wal_bytes(&self) -> Result<u64, String> {
        let mut total = 0u64;
        for (_, path) in self.list_segments()? {
            let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
            total += meta.len();
        }
        Ok(total)
    }

    pub fn recover_and_replay(&mut self) -> Result<RecoveryReport, String> {
        let initial_checkpoint = self.checkpoint_state.contiguous_wal_position;
        let mut latest_contiguous = initial_checkpoint;
        let mut scanned_records = 0usize;
        let mut replayed_records = 0usize;
        let mut skipped_already_applied = 0usize;
        let mut actions = Vec::new();
        let mut alerts = Vec::new();

        let segments = self.list_segments()?;
        if let Some((last_seg_no, _)) = segments.last() {
            self.current_segment_no = *last_seg_no;
        }

        let num_segments = segments.len();
        'seg_loop: for (idx, (segment_no, seg_path)) in segments.into_iter().enumerate() {
            let is_last_segment = idx + 1 == num_segments;
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&seg_path)
                .map_err(|e| e.to_string())?;
            let file_len = file.metadata().map_err(|e| e.to_string())?.len();
            let mut offset = 0u64;

            while offset < file_len {
                match scan_single_frame(&mut file, file_len, offset, is_last_segment)? {
                    FrameScanOutcome::Valid {
                        record,
                        next_offset,
                    } => {
                        scanned_records += 1;
                        self.next_sequence_no = self.next_sequence_no.max(record.sequence_no + 1);
                        let key_str = record.message_key.canonical_string();
                        self.wal_dedup_index.insert(
                            key_str.clone(),
                            WalDedupEntry {
                                ingest_id: record.ingest_id.clone(),
                                payload_hash: record.payload_hash.clone(),
                                wal_position: record.wal_position,
                            },
                        );

                        // Replay if at or after checkpoint or not yet in applied_messages
                        if self.db_state.applied_messages.contains_key(&key_str) {
                            if record.wal_position > initial_checkpoint {
                                skipped_already_applied += 1;
                                actions.push(RecoveryAction::SkippedAlreadyApplied {
                                    wal_position: record.wal_position,
                                    ingest_id: record.ingest_id.clone(),
                                });
                            }
                        } else {
                            let applied_obs = self.apply_record_to_db_in_memory(&record);
                            Self::atomic_write_json(
                                &self.config.state_dir.join("db_state.json"),
                                &self.db_state,
                            )?;
                            replayed_records += 1;
                            actions.push(RecoveryAction::ReplayedIntoDb {
                                wal_position: record.wal_position,
                                ingest_id: record.ingest_id.clone(),
                                observations_applied: applied_obs,
                            });
                        }

                        if record.wal_position > latest_contiguous {
                            latest_contiguous = record.wal_position;
                        }
                        offset = next_offset;
                    }
                    FrameScanOutcome::IncompleteTail {
                        record_offset,
                        remaining_bytes,
                    } => {
                        // F02: Tail incomplete record (never fsynced / never durable ACKed).
                        // Safely truncate to record_offset and sync segment file.
                        file.set_len(record_offset).map_err(|e| e.to_string())?;
                        file.sync_all().map_err(|e| e.to_string())?;
                        actions.push(RecoveryAction::TruncatedIncompleteTail {
                            segment_no,
                            record_offset,
                            truncated_bytes: remaining_bytes,
                        });
                        break;
                    }
                    FrameScanOutcome::Corrupted {
                        record_offset,
                        reason,
                    } => {
                        // F05: Corrupted record (either checksum mismatch or middle-of-stream damage).
                        // NEVER silently skip over corrupted records or claim data completeness!
                        alerts.push(RecoveryAlert::MiddleRecordCorrupted {
                            segment_no,
                            record_offset,
                            reason,
                        });
                        self.health_state = EngineHealthState::HaltedOnWalCorruption;
                        break 'seg_loop;
                    }
                }
            }
        }

        if latest_contiguous > initial_checkpoint {
            self.checkpoint_state.contiguous_wal_position = latest_contiguous;
            self.checkpoint_state.revision += 1;
            Self::atomic_write_json(
                &self.config.state_dir.join("checkpoint_state.json"),
                &self.checkpoint_state,
            )?;
            actions.push(RecoveryAction::CheckpointAdvanced {
                from: initial_checkpoint,
                to: latest_contiguous,
            });
        }

        Ok(RecoveryReport {
            initial_checkpoint,
            final_checkpoint: self.checkpoint_state.contiguous_wal_position,
            scanned_records,
            replayed_records,
            skipped_already_applied,
            actions,
            alerts,
            health_state: self.health_state,
        })
    }

    fn apply_record_to_db_in_memory(&mut self, record: &WalRecordPayload) -> usize {
        let mut applied_count = 0usize;
        for obs in &record.observations {
            let should_persist_history = match self.db_state.last_persisted_values.get(&obs.resource_id)
            {
                None => true,
                Some(baseline) => {
                    let quality_changed = baseline.quality_state != obs.quality_state;
                    let delta = (obs.value_f64 - baseline.last_value_f64).abs();
                    quality_changed || delta >= self.config.deadband_abs
                }
            };

            if should_persist_history {
                self.db_state.history_observations.push(obs.clone());
                self.db_state.last_persisted_values.insert(
                    obs.resource_id.clone(),
                    LastPersistedBaseline {
                        resource_id: obs.resource_id.clone(),
                        last_observation_id: obs.observation_id.clone(),
                        last_value_f64: obs.value_f64,
                        last_persisted_at_ms: obs.source_at_ms,
                        quality_state: obs.quality_state.clone(),
                    },
                );
                applied_count += 1;
            }

            let parsed_seq = obs.source_seq.parse::<u64>().unwrap_or(0);
            let update_latest = match self.db_state.latest_values.get(&obs.resource_id) {
                None => true,
                Some(cur) => {
                    (obs.source_at_ms, parsed_seq) >= (cur.value_sampled_at_ms, cur.source_seq)
                }
            };
            if update_latest {
                let next_rev = self
                    .db_state
                    .latest_values
                    .get(&obs.resource_id)
                    .map(|r| r.revision + 1)
                    .unwrap_or(1);
                self.db_state.latest_values.insert(
                    obs.resource_id.clone(),
                    LatestValueRow {
                        resource_id: obs.resource_id.clone(),
                        observation_id: obs.observation_id.clone(),
                        value_f64: obs.value_f64,
                        value_sampled_at_ms: obs.source_at_ms,
                        received_at_ms: obs.received_at_ms,
                        source_epoch: obs.source_epoch.clone(),
                        source_seq: parsed_seq,
                        quality_state: obs.quality_state.clone(),
                        model_revision_id: obs.model_revision_id.clone(),
                        binding_revision_id: obs.binding_revision_id.clone(),
                        revision: next_rev,
                    },
                );
            }
        }

        let key_str = record.message_key.canonical_string();
        self.db_state.applied_messages.insert(
            key_str,
            AppliedMessageRow {
                message_key: record.message_key.clone(),
                payload_hash: record.payload_hash.clone(),
                ingest_id: record.ingest_id.clone(),
                wal_position: record.wal_position,
                applied_at_ms: record.received_at_ms,
                outcome: "committed".to_string(),
            },
        );
        applied_count
    }

    /// Ingest a telemetry request with optional fault injection.
    pub fn ingest(
        &mut self,
        req: &TelemetryIngestRequest,
        fault: FaultInjectionPoint,
    ) -> Result<IngestSuccessResponse, IngestErrorResponse> {
        if self.health_state == EngineHealthState::HaltedOnWalCorruption {
            return Err(IngestErrorResponse {
                http_status: 503,
                code: "WAL_CORRUPTION_HALTED".to_string(),
                message: "Engine halted due to middle WAL record corruption; operator intervention required.".to_string(),
                retryable: false,
                retry_after_ms: None,
            });
        }

        let msg_key = req.message_key();
        let key_str = msg_key.canonical_string();
        let payload_hash = req.compute_payload_hash();

        // 1. Check idempotency across WAL dedup index and committed DB applied_messages
        if let Some(existing) = self.wal_dedup_index.get(&key_str) {
            if existing.payload_hash == payload_hash {
                let history_status = if self.db_state.applied_messages.contains_key(&key_str) {
                    "committed"
                } else {
                    "pending"
                };
                return Ok(IngestSuccessResponse {
                    http_status: 200,
                    acceptance: "duplicate".to_string(),
                    ingest_id: existing.ingest_id.clone(),
                    wal_position: Some(existing.wal_position),
                    history_status: history_status.to_string(),
                    warning: None,
                });
            } else {
                return Err(IngestErrorResponse {
                    http_status: 409,
                    code: "IDEMPOTENCY_CONFLICT".to_string(),
                    message: format!(
                        "Same message_key {} submitted with conflicting payload_hash",
                        key_str
                    ),
                    retryable: false,
                    retry_after_ms: None,
                });
            }
        }

        let seg_path = self.segment_path(self.current_segment_no);
        let current_seg_len = if seg_path.exists() {
            fs::metadata(&seg_path)
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };
        let wal_pos = WalPosition::new(self.current_segment_no, current_seg_len);
        let ingest_id = req.derive_ingest_id();

        let observations: Vec<NormalizedObservation> = req
            .points
            .iter()
            .map(|p| NormalizedObservation {
                observation_id: req.derive_observation_id(&p.property),
                tenant_id: req.tenant_id.clone(),
                resource_id: p.resource_id.clone(),
                device_id: req.device_id.clone(),
                property_identifier: p.property.clone(),
                model_revision_id: req.model_revision_id.clone(),
                binding_revision_id: req.binding_revision_id.clone(),
                source_id: req.source_id.clone(),
                source_epoch: req.source_epoch.clone(),
                source_seq: p.source_seq.clone(),
                source_at_ms: p.source_at_ms,
                received_at_ms: req.received_at_ms,
                source_time_origin: p.source_time_origin.clone(),
                value_f64: p.value_f64,
                unit_code: p.unit_code.clone(),
                quality_state: p.quality_state.clone(),
                source_quality_code: p.source_quality_code.clone(),
                replayed: p.replayed,
                ingested_wal_position: wal_pos,
            })
            .collect();

        let record = WalRecordPayload {
            wal_position: wal_pos,
            sequence_no: self.next_sequence_no,
            ingest_id: ingest_id.clone(),
            message_key: msg_key.clone(),
            payload_hash: payload_hash.clone(),
            device_id: req.device_id.clone(),
            model_revision_id: req.model_revision_id.clone(),
            binding_revision_id: req.binding_revision_id.clone(),
            received_at_ms: req.received_at_ms,
            observations,
        };

        let frame_bytes = encode_wal_frame(&record).map_err(|e| IngestErrorResponse {
            http_status: 500,
            code: "WAL_ENCODE_ERROR".to_string(),
            message: e,
            retryable: false,
            retry_after_ms: None,
        })?;

        if frame_bytes.len() as u64 > self.config.max_record_bytes {
            return Err(IngestErrorResponse {
                http_status: 413,
                code: "RECORD_TOO_LARGE".to_string(),
                message: "Encoded WAL frame exceeds max_record_bytes".to_string(),
                retryable: false,
                retry_after_ms: None,
            });
        }

        // 2. Check WAL disk watermark quota (T07)
        let current_wal_bytes = self.total_wal_bytes().unwrap_or(0);
        let projected_bytes = current_wal_bytes + (frame_bytes.len() as u64);

        if projected_bytes > self.config.hard_watermark_bytes {
            self.health_state = EngineHealthState::HardWatermarkBackpressure;
            // T07: When client requests durable (or default durable), NEVER silently downgrade to accepted_volatile!
            if req.durability == DurabilityPreference::Durable {
                return Err(IngestErrorResponse {
                    http_status: 503,
                    code: "DURABILITY_BACKPRESSURE".to_string(),
                    message: format!(
                        "WAL usage ({} bytes + {} bytes) exceeds hard watermark ({} bytes); refusing durable ingest without silent downgrade.",
                        current_wal_bytes,
                        frame_bytes.len(),
                        self.config.hard_watermark_bytes
                    ),
                    retryable: true,
                    retry_after_ms: Some(1000),
                });
            }
        }

        let mut warning = None;
        if projected_bytes > self.config.soft_watermark_bytes {
            self.health_state = EngineHealthState::SoftWatermarkWarning;
            warning = Some("WAL_SOFT_WATERMARK_EXCEEDED".to_string());
        }

        // Fault F01: Crash BEFORE writing any bytes to WAL
        if fault == FaultInjectionPoint::F01BeforeWalWrite {
            return Err(IngestErrorResponse {
                http_status: 500,
                code: "SIMULATED_CRASH_F01_BEFORE_WAL_WRITE".to_string(),
                message: "Process crashed before writing WAL frame".to_string(),
                retryable: true,
                retry_after_ms: Some(100),
            });
        }

        let is_new_segment = !seg_path.exists();
        let mut seg_file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&seg_path)
            .map_err(|e| IngestErrorResponse {
                http_status: 500,
                code: "WAL_IO_ERROR".to_string(),
                message: e.to_string(),
                retryable: true,
                retry_after_ms: Some(500),
            })?;

        // Fault F02: Write partial frame (header + partial payload) without fsync, then crash
        if let FaultInjectionPoint::F02PartialFrameBeforeSync {
            truncate_payload_bytes,
        } = fault
        {
            let write_len = (WAL_FRAME_HEADER_LEN + truncate_payload_bytes)
                .min(frame_bytes.len().saturating_sub(1));
            seg_file
                .write_all(&frame_bytes[..write_len])
                .map_err(|e| IngestErrorResponse {
                    http_status: 500,
                    code: "WAL_IO_ERROR".to_string(),
                    message: e.to_string(),
                    retryable: true,
                    retry_after_ms: Some(100),
                })?;
            return Err(IngestErrorResponse {
                http_status: 500,
                code: "SIMULATED_CRASH_F02_PARTIAL_FRAME_BEFORE_SYNC".to_string(),
                message: "Process crashed after writing partial WAL frame before fsync".to_string(),
                retryable: true,
                retry_after_ms: Some(100),
            });
        }

        // Write full frame
        seg_file
            .write_all(&frame_bytes)
            .map_err(|e| IngestErrorResponse {
                http_status: 500,
                code: "WAL_IO_ERROR".to_string(),
                message: e.to_string(),
                retryable: true,
                retry_after_ms: Some(500),
            })?;

        // Enforce durability sync boundary before returning accepted_durable
        match self.config.sync_policy {
            SyncPolicy::SyncData => seg_file.sync_data(),
            SyncPolicy::SyncAll => seg_file.sync_all(),
        }
        .map_err(|e| IngestErrorResponse {
            http_status: 500,
            code: "WAL_FSYNC_ERROR".to_string(),
            message: e.to_string(),
            retryable: true,
            retry_after_ms: Some(500),
        })?;

        if is_new_segment {
            if let Some(parent) = seg_path.parent() {
                if let Ok(dir) = File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
        }

        self.next_sequence_no += 1;
        self.wal_dedup_index.insert(
            key_str,
            WalDedupEntry {
                ingest_id: ingest_id.clone(),
                payload_hash,
                wal_position: wal_pos,
            },
        );

        // Fault F03: WAL frame is durably synced on disk, crash BEFORE sending ACK to client
        if fault == FaultInjectionPoint::F03AfterFsyncBeforeAck {
            return Err(IngestErrorResponse {
                http_status: 500,
                code: "SIMULATED_CRASH_F03_AFTER_FSYNC_BEFORE_ACK".to_string(),
                message: "Process crashed after WAL fsync but before sending HTTP 202 ACK"
                    .to_string(),
                retryable: true,
                retry_after_ms: Some(100),
            });
        }

        // Fault F04a: Crash during DB transaction before DB commit (client already or concurrently gets durable WAL boundary, DB rolls back)
        if fault == FaultInjectionPoint::F04aDbRollbackBeforeCommit {
            return Ok(IngestSuccessResponse {
                http_status: 202,
                acceptance: "accepted_durable".to_string(),
                ingest_id,
                wal_position: Some(wal_pos),
                history_status: "pending".to_string(),
                warning,
            });
        }

        // Apply to DB atomically (history_observations + last_persisted_values + applied_messages)
        self.apply_record_to_db_in_memory(&record);
        Self::atomic_write_json(
            &self.config.state_dir.join("db_state.json"),
            &self.db_state,
        )
        .map_err(|e| IngestErrorResponse {
            http_status: 500,
            code: "DB_COMMIT_ERROR".to_string(),
            message: e,
            retryable: true,
            retry_after_ms: Some(500),
        })?;

        // Fault F04b: DB transaction committed to disk, crash BEFORE advancing contiguous_wal_position checkpoint
        if fault == FaultInjectionPoint::F04bAfterDbCommitBeforeCheckpoint {
            return Ok(IngestSuccessResponse {
                http_status: 202,
                acceptance: "accepted_durable".to_string(),
                ingest_id,
                wal_position: Some(wal_pos),
                history_status: "committed".to_string(),
                warning,
            });
        }

        // Advance contiguous checkpoint
        self.checkpoint_state.contiguous_wal_position = wal_pos;
        self.checkpoint_state.revision += 1;
        self.checkpoint_state.updated_at_ms = req.received_at_ms;
        Self::atomic_write_json(
            &self.config.state_dir.join("checkpoint_state.json"),
            &self.checkpoint_state,
        )
        .map_err(|e| IngestErrorResponse {
            http_status: 500,
            code: "CHECKPOINT_WRITE_ERROR".to_string(),
            message: e,
            retryable: true,
            retry_after_ms: Some(500),
        })?;

        Ok(IngestSuccessResponse {
            http_status: 202,
            acceptance: "accepted_durable".to_string(),
            ingest_id,
            wal_position: Some(wal_pos),
            history_status: "committed".to_string(),
            warning,
        })
    }

    /// Helper for F05 fault injection: corrupt a byte inside the frame at `wal_position`.
    pub fn corrupt_byte_in_record(&self, pos: WalPosition, byte_offset_within_frame: u64) -> Result<(), String> {
        let seg_path = self.segment_path(pos.segment_no);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&seg_path)
            .map_err(|e| e.to_string())?;
        let target_offset = pos.record_offset + byte_offset_within_frame;
        file.seek(SeekFrom::Start(target_offset))
            .map_err(|e| e.to_string())?;
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte).map_err(|e| e.to_string())?;
        byte[0] ^= 0xFF;
        file.seek(SeekFrom::Start(target_offset))
            .map_err(|e| e.to_string())?;
        file.write_all(&byte).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    }
}
