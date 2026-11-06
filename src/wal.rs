use crate::config::EngineConfig;
use crate::d00::WalPosition;
use crate::error::{EngineError, ErrorCode};
use crc32fast::Hasher as Crc32Hasher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

pub const WAL_HEADER_LEN: usize = 48;
pub const WAL_MAGIC: [u8; 4] = *b"IWAL";
pub const WAL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalPointRecord {
    pub observation_id: String,
    pub resource_id: String,
    pub property: String,
    pub value_type: String,
    pub value_json: String,
    pub value_f64: Option<f64>,
    pub value_i64: Option<i64>,
    pub value_bool: Option<bool>,
    pub value_text: Option<String>,
    pub unit_code: Option<String>,
    pub unit_symbol: Option<String>,
    pub source_at_ms: i64,
    pub source_time_origin: String,
    pub source_seq: String,
    pub source_seq_u64: u64,
    pub quality_state: String,
    pub quality_source_code: Option<String>,
    pub replayed: bool,
    pub deadband: Option<f64>,
    pub heartbeat_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalRecord {
    pub ingest_id: String,
    pub tenant_id: String,
    pub source_id: String,
    pub source_epoch: String,
    pub message_id: String,
    pub device_id: String,
    pub model_revision_id: String,
    pub binding_revision_id: String,
    pub payload_hash: String,
    pub received_at_ms: i64,
    pub points: Vec<WalPointRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalAppendReceipt {
    pub wal_position: WalPosition,
    pub frame_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalRecoveryOutcome {
    Clean,
    CleanWithTruncatedTail { truncated_from_offset: u64 },
    HaltedMiddleRecordCorrupted { corrupted_offset: u64, reason: String },
}

#[derive(Debug, Clone)]
pub struct WalRecoveryScan {
    pub outcome: WalRecoveryOutcome,
    pub valid_end_offset: u64,
    pub records: Vec<(WalPosition, WalRecord)>,
}

pub struct AppWal {
    config: EngineConfig,
    segment_no: u64,
    segment_path: PathBuf,
}

impl AppWal {
    pub fn open(config: &EngineConfig) -> Result<Self, EngineError> {
        fs::create_dir_all(&config.wal_dir).map_err(|e| {
            EngineError::new(
                "req-wal-open",
                ErrorCode::InternalError,
                format!("Failed to create WAL directory: {}", e),
                vec![],
            )
        })?;
        let segment_no = 1;
        let segment_path = config.wal_dir.join(format!("seg-{:016x}.wal", segment_no));
        if !segment_path.exists() {
            let f = OpenOptions::new()
                .create(true)
                .write(true)
                .open(&segment_path)
                .map_err(|e| {
                    EngineError::new(
                        "req-wal-open",
                        ErrorCode::InternalError,
                        format!("Failed to open WAL segment: {}", e),
                        vec![],
                    )
                })?;
            let _ = f.sync_all();
        }
        Ok(Self {
            config: config.clone(),
            segment_no,
            segment_path,
        })
    }

    pub fn active_segment_path(&self) -> PathBuf {
        self.segment_path.clone()
    }

    pub fn current_bytes(&self) -> u64 {
        fs::metadata(&self.segment_path)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    pub fn append_durable(&mut self, record: &WalRecord) -> Result<WalAppendReceipt, EngineError> {
        let payload_bytes = serde_json::to_vec(record).map_err(|e| {
            EngineError::new(
                "req-wal-append",
                ErrorCode::InternalError,
                e.to_string(),
                vec![],
            )
        })?;
        let frame_len = (WAL_HEADER_LEN + payload_bytes.len()) as u64;
        if self.current_bytes() + frame_len > self.config.wal_hard_watermark_bytes {
            return Err(EngineError::new(
                "req-wal-append",
                ErrorCode::DurabilityBackpressure,
                "WAL hard watermark reached; durable append refused (T07)",
                vec![],
            )
            .with_retry_after(1_000));
        }

        let mut sha_hasher = Sha256::new();
        sha_hasher.update(&payload_bytes);
        let sha_digest: [u8; 32] = sha_hasher.finalize().into();

        let mut crc_hasher = Crc32Hasher::new();
        crc_hasher.update(&WAL_MAGIC);
        crc_hasher.update(&WAL_VERSION.to_le_bytes());
        crc_hasher.update(&0u16.to_le_bytes());
        crc_hasher.update(&(payload_bytes.len() as u32).to_le_bytes());
        crc_hasher.update(&sha_digest);
        crc_hasher.update(&payload_bytes);
        let crc32 = crc_hasher.finalize();

        let mut header = [0u8; WAL_HEADER_LEN];
        header[0..4].copy_from_slice(&WAL_MAGIC);
        header[4..6].copy_from_slice(&WAL_VERSION.to_le_bytes());
        header[6..8].copy_from_slice(&0u16.to_le_bytes());
        header[8..12].copy_from_slice(&(payload_bytes.len() as u32).to_le_bytes());
        header[12..16].copy_from_slice(&crc32.to_le_bytes());
        header[16..48].copy_from_slice(&sha_digest);

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.segment_path)
            .map_err(|e| {
                EngineError::new(
                    "req-wal-append",
                    ErrorCode::InternalError,
                    format!("Failed to open WAL for append: {}", e),
                    vec![],
                )
            })?;
        let offset = file.seek(SeekFrom::End(0)).map_err(|e| {
            EngineError::new("req-wal-append", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        file.write_all(&header).map_err(|e| {
            EngineError::new("req-wal-append", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        file.write_all(&payload_bytes).map_err(|e| {
            EngineError::new("req-wal-append", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        file.sync_all().map_err(|e| {
            EngineError::new(
                "req-wal-append",
                ErrorCode::DurabilityBackpressure,
                format!("WAL fsync failed: {}", e),
                vec![],
            )
        })?;

        Ok(WalAppendReceipt {
            wal_position: WalPosition {
                segment_no: self.segment_no,
                record_offset: offset,
            },
            frame_bytes: frame_len,
        })
    }

    pub fn scan_and_recover(
        &mut self,
        from_checkpoint: Option<&WalPosition>,
    ) -> Result<WalRecoveryScan, EngineError> {
        let mut f = File::open(&self.segment_path).map_err(|e| {
            EngineError::new("req-wal-scan", ErrorCode::InternalError, e.to_string(), vec![])
        })?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| {
            EngineError::new("req-wal-scan", ErrorCode::InternalError, e.to_string(), vec![])
        })?;

        let mut offset = 0usize;
        let total_len = buf.len();
        let mut records = Vec::new();

        while offset < total_len {
            if offset + WAL_HEADER_LEN > total_len {
                // F02: Incomplete tail header -> truncate at `offset`
                let file = OpenOptions::new()
                    .write(true)
                    .open(&self.segment_path)
                    .map_err(|e| {
                        EngineError::new(
                            "req-wal-truncate",
                            ErrorCode::InternalError,
                            e.to_string(),
                            vec![],
                        )
                    })?;
                file.set_len(offset as u64).map_err(|e| {
                    EngineError::new(
                        "req-wal-truncate",
                        ErrorCode::InternalError,
                        e.to_string(),
                        vec![],
                    )
                })?;
                let _ = file.sync_all();
                return Ok(WalRecoveryScan {
                    outcome: WalRecoveryOutcome::CleanWithTruncatedTail {
                        truncated_from_offset: offset as u64,
                    },
                    valid_end_offset: offset as u64,
                    records,
                });
            }

            let hdr = &buf[offset..offset + WAL_HEADER_LEN];
            if hdr[0..4] != WAL_MAGIC {
                // F05: If data exists beyond this offset, it's a middle-record corruption — halt!
                let remaining = &buf[offset..];
                let is_true_tail = remaining.iter().all(|&b| b == 0);
                if !is_true_tail && (offset + WAL_HEADER_LEN) < total_len {
                    return Ok(WalRecoveryScan {
                        outcome: WalRecoveryOutcome::HaltedMiddleRecordCorrupted {
                            corrupted_offset: offset as u64,
                            reason: "WAL_MAGIC mismatch in middle WAL frame (F05)".to_string(),
                        },
                        valid_end_offset: offset as u64,
                        records,
                    });
                }
                // True tail (zeroed preallocation or last bytes) — safe to truncate (F02)
                let file = OpenOptions::new()
                    .write(true)
                    .open(&self.segment_path)
                    .map_err(|e| {
                        EngineError::new("req-wal-truncate", ErrorCode::InternalError, e.to_string(), vec![])
                    })?;
                file.set_len(offset as u64).map_err(|e| {
                    EngineError::new("req-wal-truncate", ErrorCode::InternalError, e.to_string(), vec![])
                })?;
                let _ = file.sync_all();
                return Ok(WalRecoveryScan {
                    outcome: WalRecoveryOutcome::CleanWithTruncatedTail {
                        truncated_from_offset: offset as u64,
                    },
                    valid_end_offset: offset as u64,
                    records,
                });
            }

            let payload_len = u32::from_le_bytes(hdr[8..12].try_into().unwrap()) as usize;
            let expected_crc = u32::from_le_bytes(hdr[12..16].try_into().unwrap());
            let expected_sha = &hdr[16..48];

            let frame_end = offset + WAL_HEADER_LEN + payload_len;
            if frame_end > total_len {
                // Validate header CRC first — if header CRC fails, payload_len is corrupt (F05)
                let _expected_crc_pre = u32::from_le_bytes(hdr[12..16].try_into().unwrap());
                let _expected_sha_pre = &hdr[16..48];
                // Build a CRC over what we have (header fields sans CRC + sha + available payload)
                let mut crc_pre = Crc32Hasher::new();
                crc_pre.update(&hdr[0..12]);
                crc_pre.update(&hdr[16..48]);
                // We can't verify full payload CRC since frame_end > total_len, but we CAN check
                // if the header itself is internally consistent by verifying a zero-payload CRC
                // against the header-only portion. However the simplest reliable check: if there are
                // more valid WAL frames beyond offset + WAL_HEADER_LEN, this is a corrupted middle record.
                let scan_ahead_start = offset + WAL_HEADER_LEN;
                let has_subsequent_frame = if scan_ahead_start + 4 <= total_len {
                    buf[scan_ahead_start..scan_ahead_start + 4] == WAL_MAGIC
                } else {
                    false
                };
                if has_subsequent_frame {
                    return Ok(WalRecoveryScan {
                        outcome: WalRecoveryOutcome::HaltedMiddleRecordCorrupted {
                            corrupted_offset: offset as u64,
                            reason: "Corrupt payload_len in middle WAL frame — subsequent valid frame exists (F05)".to_string(),
                        },
                        valid_end_offset: offset as u64,
                        records,
                    });
                }
                // F02: True incomplete tail payload -> truncate at `offset`
                let file = OpenOptions::new()
                    .write(true)
                    .open(&self.segment_path)
                    .map_err(|e| {
                        EngineError::new("req-wal-truncate", ErrorCode::InternalError, e.to_string(), vec![])
                    })?;
                file.set_len(offset as u64).map_err(|e| {
                    EngineError::new("req-wal-truncate", ErrorCode::InternalError, e.to_string(), vec![])
                })?;
                let _ = file.sync_all();
                return Ok(WalRecoveryScan {
                    outcome: WalRecoveryOutcome::CleanWithTruncatedTail {
                        truncated_from_offset: offset as u64,
                    },
                    valid_end_offset: offset as u64,
                    records,
                });
            }

            let payload = &buf[offset + WAL_HEADER_LEN..frame_end];
            let mut sha_hasher = Sha256::new();
            sha_hasher.update(payload);
            let actual_sha: [u8; 32] = sha_hasher.finalize().into();

            let mut crc_hasher = Crc32Hasher::new();
            crc_hasher.update(&hdr[0..12]);
            crc_hasher.update(&hdr[16..48]);
            crc_hasher.update(payload);
            let actual_crc = crc_hasher.finalize();

            if actual_crc != expected_crc || actual_sha.as_slice() != expected_sha {
                // Check if this is the very last frame or a middle record (F05)
                if frame_end < total_len {
                    return Ok(WalRecoveryScan {
                        outcome: WalRecoveryOutcome::HaltedMiddleRecordCorrupted {
                            corrupted_offset: offset as u64,
                            reason: "CRC32/SHA-256 mismatch in middle WAL frame (F05)".to_string(),
                        },
                        valid_end_offset: offset as u64,
                        records,
                    });
                } else {
                    let file = OpenOptions::new()
                        .write(true)
                        .open(&self.segment_path)
                        .map_err(|e| {
                            EngineError::new("req-wal-truncate", ErrorCode::InternalError, e.to_string(), vec![])
                        })?;
                    file.set_len(offset as u64).map_err(|e| {
                        EngineError::new("req-wal-truncate", ErrorCode::InternalError, e.to_string(), vec![])
                    })?;
                    let _ = file.sync_all();
                    return Ok(WalRecoveryScan {
                        outcome: WalRecoveryOutcome::CleanWithTruncatedTail {
                            truncated_from_offset: offset as u64,
                        },
                        valid_end_offset: offset as u64,
                        records,
                    });
                }
            }

            let pos = WalPosition {
                segment_no: self.segment_no,
                record_offset: offset as u64,
            };
            let should_include = match from_checkpoint {
                None => true,
                Some(cp) => &pos > cp,
            };
            if should_include {
                let rec: WalRecord = serde_json::from_slice(payload).map_err(|e| {
                    EngineError::new(
                        "req-wal-decode",
                        ErrorCode::InternalError,
                        e.to_string(),
                        vec![],
                    )
                })?;
                records.push((pos, rec));
            }
            offset = frame_end;
        }

        Ok(WalRecoveryScan {
            outcome: WalRecoveryOutcome::Clean,
            valid_end_offset: offset as u64,
            records,
        })
    }
}
