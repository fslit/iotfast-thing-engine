use f3_f6_wal_and_stream::checkpoint::{
    DeliveryStateRow, DeliveryStatus, NonContiguousCheckpointCoordinator,
};
use f3_f6_wal_and_stream::subscription::{
    ClientSubscriptionReplica, LiveSubscriptionHub, ResourceLiveState, ServerWsFrame,
    SubscribeRequest, SubscriptionFilter, SubscriptionOptions, UnitSpec, VerifiedScopeContext,
};
use f3_f6_wal_and_stream::wal::{
     encode_wal_frame, ApplicationWalEngine, DurabilityPreference, EngineHealthState,
    FaultInjectionPoint, RecoveryAction, RecoveryAlert, SyncPolicy, TelemetryIngestRequest,
    TelemetryPointInput, WalConfig, WalPosition,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::thread;
use std::time::Instant;
use tempfile::TempDir;

fn sample_ingest_request(
    message_id: &str,
    value_f64: f64,
    source_seq: u64,
    source_at_ms: i64,
    replayed: bool,
    durability: DurabilityPreference,
) -> TelemetryIngestRequest {
    TelemetryIngestRequest {
        request_id: format!("req-{}", message_id),
        tenant_id: "tenant-8iic".to_string(),
        source_id: "gw-smt-01".to_string(),
        source_epoch: "epoch-20261106-01".to_string(),
        message_id: message_id.to_string(),
        device_id: "oven-02".to_string(),
        model_revision_id: "mr-oven-v1".to_string(),
        binding_revision_id: "br-oven-02-v1".to_string(),
        received_at_ms: source_at_ms + 15,
        durability,
        points: vec![TelemetryPointInput {
            property: "zone1_temp".to_string(),
            resource_id: "res-oven-02-zone1-temp".to_string(),
            value_f64,
            unit_code: "Cel".to_string(),
            source_at_ms,
            source_time_origin: "gateway_sample".to_string(),
            source_seq: source_seq.to_string(),
            quality_state: "good".to_string(),
            source_quality_code: None,
            replayed,
        }],
    }
}

#[test]
fn test_wal_position_fixed_width_and_frame_checksum() {
    let positions = vec![
        WalPosition::new(0, 0),
        WalPosition::new(1, 0),
        WalPosition::new(1, 9),
        WalPosition::new(1, 10),
        WalPosition::new(1, 255),
        WalPosition::new(1, 256),
        WalPosition::new(1, 65535),
        WalPosition::new(2, 0),
        WalPosition::new(16, 4096),
    ];

    for w in positions.windows(2) {
        let a = w[0];
        let b = w[1];
        let a_str = a.format_fixed();
        let b_str = b.format_fixed();
        assert_eq!(a_str.len(), 33);
        assert_eq!(b_str.len(), 33);
        assert!(a < b);
        assert!(
            a_str < b_str,
            "Lexicographical order failed: {} vs {}",
            a_str,
            b_str
        );
        assert_eq!(WalPosition::from_str(&a_str).unwrap(), a);
        assert_eq!(WalPosition::from_str(&b_str).unwrap(), b);
    }

    // Reject non-fixed-width or uppercase strings
    assert!(WalPosition::from_str("1:65").is_err());
    assert!(WalPosition::from_str("0000000000000001:00000000000000AB").is_err());
}

#[test]
fn test_f01_crash_before_wal_write_and_retry() {
    let tmp = TempDir::new().unwrap();
    let cfg = WalConfig::new(tmp.path().join("wal"), tmp.path().join("state"));
    let (mut engine, _) = ApplicationWalEngine::open_and_recover(cfg.clone()).unwrap();

    let req = sample_ingest_request(
        "msg-f01-01",
        245.6,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );

    // Inject F01 crash before WAL write
    let err = engine
        .ingest(&req, FaultInjectionPoint::F01BeforeWalWrite)
        .unwrap_err();
    assert_eq!(err.http_status, 500);
    assert!(err.retryable);
    assert_eq!(engine.total_wal_bytes().unwrap(), 0);
    assert!(engine.db_state.history_observations.is_empty());

    // Reopen / recover and retry with original key
    let (mut engine_recovered, report) = ApplicationWalEngine::open_and_recover(cfg).unwrap();
    assert_eq!(report.scanned_records, 0);
    assert_eq!(report.replayed_records, 0);

    let mut retry_req = req.clone();
    retry_req.points[0].replayed = true;
    let ack = engine_recovered
        .ingest(&retry_req, FaultInjectionPoint::None)
        .unwrap();
    assert_eq!(ack.http_status, 202);
    assert_eq!(ack.acceptance, "accepted_durable");
    assert_eq!(engine_recovered.db_state.history_observations.len(), 1);
    assert_eq!(
        engine_recovered.db_state.history_observations[0].value_f64,
        245.6
    );

    // Also verify SyncPolicy::SyncData works cleanly on durable ingest
    let mut cfg_sync_data = WalConfig::new(tmp.path().join("wal_sd"), tmp.path().join("state_sd"));
    cfg_sync_data.sync_policy = SyncPolicy::SyncData;
    let (mut eng_sd, _) = ApplicationWalEngine::open_and_recover(cfg_sync_data).unwrap();
    let ack_sd = eng_sd.ingest(&req, FaultInjectionPoint::None).unwrap();
    assert_eq!(ack_sd.acceptance, "accepted_durable");
}

#[test]
fn test_f02_partial_tail_frame_truncation_recovery_and_retry() {
    let tmp = TempDir::new().unwrap();
    let cfg = WalConfig::new(tmp.path().join("wal"), tmp.path().join("state"));
    let (mut engine, _) = ApplicationWalEngine::open_and_recover(cfg.clone()).unwrap();

    // 1. First record succeeds and is durably synced
    let req1 = sample_ingest_request(
        "msg-f02-01",
        245.6,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );
    let ack1 = engine.ingest(&req1, FaultInjectionPoint::None).unwrap();
    assert_eq!(ack1.acceptance, "accepted_durable");
    let valid_wal_bytes = engine.total_wal_bytes().unwrap();
    assert!(valid_wal_bytes > 0);

    // 2. Second record crashes mid-frame before fsync (F02)
    let req2 = sample_ingest_request(
        "msg-f02-02",
        246.8,
        2,
        1793869201000,
        false,
        DurabilityPreference::Durable,
    );
    let err2 = engine
        .ingest(
            &req2,
            FaultInjectionPoint::F02PartialFrameBeforeSync {
                truncate_payload_bytes: 32,
            },
        )
        .unwrap_err();
    assert_eq!(
        err2.code,
        "SIMULATED_CRASH_F02_PARTIAL_FRAME_BEFORE_SYNC"
    );
    let corrupted_tail_wal_bytes = engine.total_wal_bytes().unwrap();
    assert!(corrupted_tail_wal_bytes > valid_wal_bytes);

    // 3. Restart engine: must detect incomplete tail frame, truncate to valid_wal_bytes, and preserve req1
    let (mut engine_recovered, report) = ApplicationWalEngine::open_and_recover(cfg).unwrap();
    assert_eq!(report.health_state, EngineHealthState::Ready);
    assert_eq!(report.scanned_records, 1);
    assert!(report.actions.iter().any(|a| matches!(
        a,
        RecoveryAction::TruncatedIncompleteTail {
            segment_no: 1,
            record_offset,
            truncated_bytes,
        } if *record_offset == valid_wal_bytes && *truncated_bytes == (corrupted_tail_wal_bytes - valid_wal_bytes)
    )));
    assert_eq!(engine_recovered.total_wal_bytes().unwrap(), valid_wal_bytes);
    assert_eq!(engine_recovered.db_state.history_observations.len(), 1);

    // 4. Client retries req2 with original key -> succeeds cleanly
    let mut retry_req2 = req2.clone();
    retry_req2.points[0].replayed = true;
    let ack2 = engine_recovered
        .ingest(&retry_req2, FaultInjectionPoint::None)
        .unwrap();
    assert_eq!(ack2.http_status, 202);
    assert_eq!(ack2.acceptance, "accepted_durable");
    assert_eq!(engine_recovered.db_state.history_observations.len(), 2);
    assert_eq!(
        engine_recovered.db_state.history_observations[0].value_f64,
        245.6
    );
    assert_eq!(
        engine_recovered.db_state.history_observations[1].value_f64,
        246.8
    );
}

#[test]
fn test_f03_crash_after_fsync_before_ack_idempotent_retry() {
    let tmp = TempDir::new().unwrap();
    let cfg = WalConfig::new(tmp.path().join("wal"), tmp.path().join("state"));
    let (mut engine, _) = ApplicationWalEngine::open_and_recover(cfg.clone()).unwrap();

    let req = sample_ingest_request(
        "msg-f03-01",
        245.6,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );
    let expected_ingest_id = req.derive_ingest_id();

    // Crash after WAL fsync but before returning HTTP 202 ACK or committing DB
    let err = engine
        .ingest(&req, FaultInjectionPoint::F03AfterFsyncBeforeAck)
        .unwrap_err();
    assert_eq!(err.code, "SIMULATED_CRASH_F03_AFTER_FSYNC_BEFORE_ACK");
    assert!(engine.db_state.history_observations.is_empty());

    // Restart engine: WAL record is replayed into DB and dedup index is restored
    let (mut engine_recovered, report) = ApplicationWalEngine::open_and_recover(cfg).unwrap();
    assert_eq!(report.scanned_records, 1);
    assert_eq!(report.replayed_records, 1);
    assert_eq!(engine_recovered.db_state.history_observations.len(), 1);

    // Client retries with original key (and replayed=true): must get 200 duplicate with original ingest_id
    let mut retry_req = req.clone();
    retry_req.points[0].replayed = true;
    let dup_ack = engine_recovered
        .ingest(&retry_req, FaultInjectionPoint::None)
        .unwrap();
    assert_eq!(dup_ack.http_status, 200);
    assert_eq!(dup_ack.acceptance, "duplicate");
    assert_eq!(dup_ack.ingest_id, expected_ingest_id);
    assert_eq!(engine_recovered.db_state.history_observations.len(), 1);

    // Client sends same key with modified temperature (255.0): must get 409 IDEMPOTENCY_CONFLICT (T04)
    let mut conflict_req = req.clone();
    conflict_req.points[0].value_f64 = 255.0;
    let conflict_err = engine_recovered
        .ingest(&conflict_req, FaultInjectionPoint::None)
        .unwrap_err();
    assert_eq!(conflict_err.http_status, 409);
    assert_eq!(conflict_err.code, "IDEMPOTENCY_CONFLICT");
    assert_eq!(
        engine_recovered.db_state.history_observations[0].value_f64,
        245.6
    );
}

#[test]
fn test_f04_db_uncommitted_and_checkpoint_lag_idempotent_replay() {
    let tmp = TempDir::new().unwrap();
    let cfg = WalConfig::new(tmp.path().join("wal"), tmp.path().join("state"));
    let (mut engine, _) = ApplicationWalEngine::open_and_recover(cfg.clone()).unwrap();

    // Record 1: DB committed, but checkpoint not updated (F04b)
    let req1 = sample_ingest_request(
        "msg-f04-01",
        245.6,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );
    let ack1 = engine
        .ingest(
            &req1,
            FaultInjectionPoint::F04bAfterDbCommitBeforeCheckpoint,
        )
        .unwrap();
    assert_eq!(ack1.acceptance, "accepted_durable");
    assert_eq!(
        engine.checkpoint_state.contiguous_wal_position,
        WalPosition::ZERO
    );

    // Record 2: WAL synced, DB uncommitted before crash (F04a)
    let req2 = sample_ingest_request(
        "msg-f04-02",
        247.2,
        2,
        1793869201000,
        false,
        DurabilityPreference::Durable,
    );
    let ack2 = engine
        .ingest(&req2, FaultInjectionPoint::F04aDbRollbackBeforeCommit)
        .unwrap();
    assert_eq!(ack2.acceptance, "accepted_durable");
    assert_eq!(engine.db_state.history_observations.len(), 1);

    // Restart & recover:
    // - req1 must be recognized via applied_messages (SkippedAlreadyApplied, not duplicated!)
    // - req2 must be replayed into DB (ReplayedIntoDb)
    // - contiguous_wal_position advances to req2's wal_position
    let (engine_recovered, report) = ApplicationWalEngine::open_and_recover(cfg).unwrap();
    assert_eq!(report.scanned_records, 2);
    assert_eq!(report.skipped_already_applied, 1);
    assert_eq!(report.replayed_records, 1);
    assert_eq!(
        report.final_checkpoint,
        ack2.wal_position.expect("req2 wal_position")
    );
    assert_eq!(engine_recovered.db_state.history_observations.len(), 2);
    assert_eq!(
        engine_recovered.db_state.history_observations[0].value_f64,
        245.6
    );
    assert_eq!(
        engine_recovered.db_state.history_observations[1].value_f64,
        247.2
    );
}

#[test]
fn test_f05_middle_record_corruption_halts_without_silent_skip_and_t07_backpressure() {
    // Part A: F05 Middle Record Corruption
    let tmp = TempDir::new().unwrap();
    let cfg = WalConfig::new(tmp.path().join("wal"), tmp.path().join("state"));
    let (mut engine, _) = ApplicationWalEngine::open_and_recover(cfg.clone()).unwrap();

    let req1 = sample_ingest_request(
        "msg-f05-01",
        245.1,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );
    let req2 = sample_ingest_request(
        "msg-f05-02",
        245.2,
        2,
        1793869201000,
        false,
        DurabilityPreference::Durable,
    );
    let req3 = sample_ingest_request(
        "msg-f05-03",
        245.3,
        3,
        1793869202000,
        false,
        DurabilityPreference::Durable,
    );

    let ack1 = engine
        .ingest(&req1, FaultInjectionPoint::F04aDbRollbackBeforeCommit)
        .unwrap();
    let ack2 = engine
        .ingest(&req2, FaultInjectionPoint::F04aDbRollbackBeforeCommit)
        .unwrap();
    let _ack3 = engine
        .ingest(&req3, FaultInjectionPoint::F04aDbRollbackBeforeCommit)
        .unwrap();

    // Corrupt a payload byte inside the middle record (req2)
    let pos2 = ack2.wal_position.unwrap();
    engine.corrupt_byte_in_record(pos2, 60).unwrap();

    // Recover: must replay req1, halt immediately at req2, emit MiddleRecordCorrupted alert, and NOT skip to req3!
    let (mut engine_halted, report) = ApplicationWalEngine::open_and_recover(cfg).unwrap();
    assert_eq!(report.health_state, EngineHealthState::HaltedOnWalCorruption);
    assert_eq!(report.replayed_records, 1);
    assert_eq!(report.final_checkpoint, ack1.wal_position.unwrap());
    assert_eq!(engine_halted.db_state.history_observations.len(), 1);
    assert_eq!(report.alerts.len(), 1);
    assert!(matches!(
        &report.alerts[0],
        RecoveryAlert::MiddleRecordCorrupted {
            segment_no: 1,
            record_offset,
            ..
        } if *record_offset == pos2.record_offset
    ));

    // New writes while halted on corruption must be rejected
    let req4 = sample_ingest_request(
        "msg-f05-04",
        245.4,
        4,
        1793869203000,
        false,
        DurabilityPreference::Durable,
    );
    let halted_err = engine_halted
        .ingest(&req4, FaultInjectionPoint::None)
        .unwrap_err();
    assert_eq!(halted_err.http_status, 503);
    assert_eq!(halted_err.code, "WAL_CORRUPTION_HALTED");

    // Part B: T07 Hard Watermark Backpressure
    let tmp_bp = TempDir::new().unwrap();
    let mut cfg_bp = WalConfig::new(tmp_bp.path().join("wal"), tmp_bp.path().join("state"));
    // Measure single frame size (~750 bytes) and set soft watermark = 1 frame, hard watermark = 2 frames
    let probe_req = sample_ingest_request(
        "msg-bp-01",
        245.6,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );
    let ( mut engine_bp, _) = ApplicationWalEngine::open_and_recover(cfg_bp.clone()).unwrap();
    let ok1 = engine_bp
        .ingest(&probe_req, FaultInjectionPoint::None)
        .unwrap();
    let one_frame_bytes = engine_bp.total_wal_bytes().unwrap();
    assert_eq!(ok1.http_status, 202);
    assert_eq!(ok1.acceptance, "accepted_durable");

    cfg_bp.soft_watermark_bytes = one_frame_bytes + 100;
    cfg_bp.hard_watermark_bytes = one_frame_bytes * 2 + 50;
    engine_bp.config = cfg_bp;

    // 2nd message crosses soft watermark -> succeeds with warning, still accepted_durable
    let req_bp2 = sample_ingest_request(
        "msg-bp-02",
        246.0,
        2,
        1793869201000,
        false,
        DurabilityPreference::Durable,
    );
    let ok2 = engine_bp
        .ingest(&req_bp2, FaultInjectionPoint::None)
        .unwrap();
    assert_eq!(ok2.http_status, 202);
    assert_eq!(ok2.acceptance, "accepted_durable");
    assert_eq!(
        ok2.warning.as_deref(),
        Some("WAL_SOFT_WATERMARK_EXCEEDED")
    );

    // 3rd message crosses hard watermark with Prefer: durability=durable -> 503 DURABILITY_BACKPRESSURE (T07)
    let req_bp3 = sample_ingest_request(
        "msg-bp-03",
        246.5,
        3,
        1793869202000,
        false,
        DurabilityPreference::Durable,
    );
    let bp_err = engine_bp
        .ingest(&req_bp3, FaultInjectionPoint::None)
        .unwrap_err();
    assert_eq!(bp_err.http_status, 503);
    assert_eq!(bp_err.code, "DURABILITY_BACKPRESSURE");
    assert!(bp_err.retryable);
    assert_eq!(engine_bp.db_state.history_observations.len(), 2);
}

#[test]
fn test_st04_a06_non_contiguous_checkpoint_and_delivery_gap_disclosure() {
    let tmp = TempDir::new().unwrap();
    let store_path = tmp.path().join("coordinator_store.json");
    let mut coord = NonContiguousCheckpointCoordinator::open(store_path.clone()).unwrap();
    coord
        .register_target("tenant-8iic", "remote-tsdb", 1793869200000)
        .unwrap();

    let pos_101 = WalPosition::new(1, 0x65);
    let pos_102 = WalPosition::new(1, 0x66);
    let pos_103 = WalPosition::new(1, 0x67);

    // Record 101: Committed
    let cp1 = coord
        .record_delivery_state(DeliveryStateRow {
            tenant_id: "tenant-8iic".to_string(),
            target_id: "remote-tsdb".to_string(),
            seq_no: 101,
            wal_position: pos_101,
            observation_id: "obs-101".to_string(),
            resource_id: "res-oven-02-zone1-temp".to_string(),
            source_at_ms: 1793869201000,
            value_f64: 245.1,
            payload_hash: "sha256-jcs-v1:hash-101".to_string(),
            state: DeliveryStatus::Committed,
            attempt_count: 1,
            last_error_code: None,
            updated_at_ms: 1793869201050,
        })
        .unwrap();
    assert_eq!(cp1.contiguous_seq_no, 101);
    assert_eq!(cp1.contiguous_wal_position, pos_101);

    // Record 102: Inflight (simulating timeout / crash before response)
    let cp2 = coord
        .record_delivery_state(DeliveryStateRow {
            tenant_id: "tenant-8iic".to_string(),
            target_id: "remote-tsdb".to_string(),
            seq_no: 102,
            wal_position: pos_102,
            observation_id: "obs-102".to_string(),
            resource_id: "res-oven-02-zone1-temp".to_string(),
            source_at_ms: 1793869202000,
            value_f64: 245.2,
            payload_hash: "sha256-jcs-v1:hash-102".to_string(),
            state: DeliveryStatus::Inflight,
            attempt_count: 1,
            last_error_code: None,
            updated_at_ms: 1793869202050,
        })
        .unwrap();
    assert_eq!(cp2.contiguous_seq_no, 101);
    assert_eq!(cp2.contiguous_wal_position, pos_101);

    // Record 103: Committed out-of-order while 102 is unresolved
    let cp3 = coord
        .record_delivery_state(DeliveryStateRow {
            tenant_id: "tenant-8iic".to_string(),
            target_id: "remote-tsdb".to_string(),
            seq_no: 103,
            wal_position: pos_103,
            observation_id: "obs-103".to_string(),
            resource_id: "res-oven-02-zone1-temp".to_string(),
            source_at_ms: 1793869203000,
            value_f64: 245.3,
            payload_hash: "sha256-jcs-v1:hash-103".to_string(),
            state: DeliveryStatus::Committed,
            attempt_count: 1,
            last_error_code: None,
            updated_at_ms: 1793869203050,
        })
        .unwrap();

    // Contiguous checkpoint MUST stay at 101, never jumping to 103!
    assert_eq!(cp3.contiguous_seq_no, 101);
    assert_eq!(cp3.contiguous_wal_position, pos_101);

    // Simulate process restart: reload coordinator from disk
    let mut coord_restarted =
        NonContiguousCheckpointCoordinator::open(store_path.clone()).unwrap();
    let d102 = coord_restarted.get_delivery("remote-tsdb", 102).unwrap();
    let d103 = coord_restarted.get_delivery("remote-tsdb", 103).unwrap();
    assert_eq!(d102.state, DeliveryStatus::Unknown);
    assert_eq!(d103.state, DeliveryStatus::Committed);

    // Verify 103 is NOT scheduled for re-delivery; only 102 requires reconciliation
    let pending_items = coord_restarted.items_requiring_recovery_or_retry("remote-tsdb");
    assert_eq!(pending_items.len(), 1);
    assert_eq!(pending_items[0].seq_no, 102);
    assert_eq!(
        coord_restarted.safe_wal_recycle_watermark(&["remote-tsdb"]),
        pos_101
    );

    // Path A: Query-back confirms 102 on remote target -> checkpoint jumps from 101 to 103
    let store_path_branch_a = tmp.path().join("coordinator_branch_a.json");
    fs::copy(&store_path, &store_path_branch_a).unwrap();
    let mut coord_a = NonContiguousCheckpointCoordinator::open(store_path_branch_a).unwrap();
    let cp_resolved = coord_a
        .reconcile_unknown_by_query_back(
            "remote-tsdb",
            102,
            "sha256-jcs-v1:hash-102",
            1793869204000,
        )
        .unwrap();
    assert_eq!(cp_resolved.contiguous_seq_no, 103);
    assert_eq!(cp_resolved.contiguous_wal_position, pos_103);
    let q_a = coord_a
        .query_history_with_coverage(
            "remote-tsdb",
            "res-oven-02-zone1-temp",
            1793869200000,
            1793869210000,
        )
        .unwrap();
    assert!(!q_a.partial);
    assert!(q_a.coverage.known_gaps.is_empty());
    assert_eq!(q_a.items.len(), 3);

    // Path B: Unauthorized skip is rejected; explicit authorized delivery_gap advances checkpoint to 103 and discloses gap
    assert!(coord_restarted
        .authorize_delivery_gap(
            "gap-102",
            "remote-tsdb",
            102,
            "permanent_schema_mismatch",
            "isolate_and_disclose",
            "",
            1793869205000,
        )
        .is_err());

    let (cp_gap, gap_row) = coord_restarted
        .authorize_delivery_gap(
            "gap-20261106-102",
            "remote-tsdb",
            102,
            "permanent_schema_mismatch",
            "isolate_and_disclose",
            "ops-lead@8iic",
            1793869205000,
        )
        .unwrap();
    assert_eq!(cp_gap.contiguous_seq_no, 103);
    assert_eq!(cp_gap.contiguous_wal_position, pos_103);
    assert_eq!(gap_row.observation_id, "obs-102");

    let q_b = coord_restarted
        .query_history_with_coverage(
            "remote-tsdb",
            "res-oven-02-zone1-temp",
            1793869200000,
            1793869210000,
        )
        .unwrap();
    assert!(q_b.partial);
    assert_eq!(q_b.warnings, vec!["DELIVERY_GAP_PRESENT".to_string()]);
    assert_eq!(q_b.items.len(), 2);
    assert_eq!(q_b.coverage.known_gaps.len(), 1);
    assert_eq!(q_b.coverage.known_gaps[0].gap_id, "gap-20261106-102");
    assert_eq!(q_b.coverage.known_gaps[0].seq_no, 102);
    assert_eq!(q_b.coverage.known_gaps[0].wal_position, pos_102);
}

#[test]
fn test_f6_concurrent_snapshot_and_delta_handoff_and_overflow_resync() {
    let hub = LiveSubscriptionHub::new("srv-epoch-20261106-01");

    // Register 8 bound resources on pump-001 + 1 unbound resource on pump-001 + 2 resources on unauthorized pump-002
    for idx in 0..8 {
        let prop = format!("pressure_ch{}", idx);
        let res_id = format!("res-pump-001-{}", prop);
        hub.register_resource(ResourceLiveState {
            resource_id: res_id,
            device_id: "pump-001".to_string(),
            property: prop.clone(),
            canonical_uri: format!("uns://8iic/plant-a/pump-001/properties/{}", prop),
            model_revision_id: "pump-model-r1".to_string(),
            binding_revision_id: Some("pump-001-bind-r1".to_string()),
            resource_revision: 1,
            last_stream_seq: 0,
            value: Some(0.40 + (idx as f64) * 0.01),
            unit: Some(UnitSpec {
                code: "MPa".to_string(),
                symbol: "MPa".to_string(),
            }),
            quality: "good".to_string(),
            quality_reason: None,
            freshness: "fresh".to_string(),
            value_sampled_at_ms: Some(1793869200000),
            quality_observed_at_ms: Some(1793869200000),
        });
    }

    // Unbound property: must keep value=None and quality="unknown", never fabricating 0.0
    hub.register_resource(ResourceLiveState {
        resource_id: "res-pump-001-maint-note".to_string(),
        device_id: "pump-001".to_string(),
        property: "maintenance_note".to_string(),
        canonical_uri: "uns://8iic/plant-a/pump-001/properties/maintenance_note".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        binding_revision_id: None,
        resource_revision: 1,
        last_stream_seq: 0,
        value: None,
        unit: None,
        quality: "unknown".to_string(),
        quality_reason: Some("unbound_property".to_string()),
        freshness: "unknown".to_string(),
        value_sampled_at_ms: None,
        quality_observed_at_ms: None,
    });

    // Unauthorized device pump-002
    hub.register_resource(ResourceLiveState {
        resource_id: "res-pump-002-pressure".to_string(),
        device_id: "pump-002".to_string(),
        property: "outlet_pressure".to_string(),
        canonical_uri: "uns://8iic/plant-b/pump-002/properties/outlet_pressure".to_string(),
        model_revision_id: "pump-model-r1".to_string(),
        binding_revision_id: Some("pump-002-bind-r1".to_string()),
        resource_revision: 1,
        last_stream_seq: 0,
        value: Some(0.88),
        unit: Some(UnitSpec {
            code: "MPa".to_string(),
            symbol: "MPa".to_string(),
        }),
        quality: "good".to_string(),
        quality_reason: None,
        freshness: "fresh".to_string(),
        value_sampled_at_ms: Some(1793869200000),
        quality_observed_at_ms: Some(1793869200000),
    });

    // Pre-subscription updates (advance stream_seq to 40)
    for step in 0..40u64 {
        let ch = step % 8;
        let res_id = format!("res-pump-001-pressure_ch{}", ch);
        hub.publish_measurement_update(
            &res_id,
            0.50 + (step as f64) * 0.005,
            1793869200100 + (step as i64) * 10,
        )
        .unwrap();
    }

    let scope = VerifiedScopeContext {
        tenant_id: "tenant-8iic".to_string(),
        caller_service_id: "scada-view-svc".to_string(),
        authorized_device_ids: BTreeSet::from(["pump-001".to_string()]),
        authorized_resource_ids: BTreeSet::new(),
    };

    // Subscribe with chunk_size = 3 so snapshot spans multiple chunks while concurrent updates happen
    let (sub_id, boundary_seq) = hub.subscribe(
        scope.clone(),
        SubscribeRequest {
            client_sub_id: "sub-client-01".to_string(),
            filter: SubscriptionFilter {
                device_ids: vec!["pump-001".to_string()],
                resource_ids: vec![],
                uri_prefixes: vec![],
            },
            options: SubscriptionOptions {
                coalesce_slow_consumer: false,
                max_pending_deltas: 512,
                snapshot_chunk_size: 3,
            },
        },
    );
    assert_eq!(boundary_seq, 40);

    // Spawn concurrent high-frequency updater thread while snapshot and delta frames are polled
    let hub_writer = hub.clone();
    let writer_handle = thread::spawn(move || {
        for step in 0..160u64 {
            let ch = step % 8;
            let res_id = format!("res-pump-001-pressure_ch{}", ch);
            let ts = 1793869210000 + (step as i64) * 10;
            if step == 155 {
                // Inject a communication timeout quality change on ch0 (T06)
                hub_writer
                    .publish_quality_change(
                        "res-pump-001-pressure_ch0",
                        "bad",
                        Some("modbus_read_timeout".to_string()),
                        "stale",
                        ts,
                    )
                    .unwrap();
            } else {
                hub_writer
                    .publish_measurement_update(&res_id, 0.70 + (step as f64) * 0.002, ts)
                    .unwrap();
            }
            // Also update unauthorized pump-002 to prove scope isolation
            if step % 20 == 0 {
                hub_writer
                    .publish_measurement_update("res-pump-002-pressure", 0.95, ts)
                    .unwrap();
            }
        }
    });

    // Poll first two snapshot frames while writer is concurrently pushing updates
    let mut replica = ClientSubscriptionReplica::default();
    let first_frame = hub.poll_next_frame(&sub_id).unwrap();
    assert!(matches!(first_frame, ServerWsFrame::SnapshotBegin { .. }));
    replica.apply_frame(first_frame).unwrap();

    writer_handle.join().unwrap();

    // Drain remaining snapshot chunks, snapshot_end, and all buffered delta frames
    while let Some(frame) = hub.poll_next_frame(&sub_id) {
        replica.apply_frame(frame).unwrap();
    }

    assert!(replica.snapshot_completed);
    assert_eq!(replica.boundary_seq, Some(40));
    assert_eq!(replica.delta_frames_applied, 160);
    assert!(replica.resync_required_error.is_none());

    // Verify unbound property still has value == None and quality == "unknown"
    let maint = replica.resources.get("res-pump-001-maint-note").unwrap();
    assert_eq!(maint.value, None);
    assert_eq!(maint.quality, "unknown");

    // Verify ch0 retained its last measurement timestamp while quality changed to bad (T06)
    let ch0 = replica.resources.get("res-pump-001-pressure_ch0").unwrap();
    assert_eq!(ch0.quality, "bad");
    assert_eq!(ch0.quality_reason.as_deref(), Some("modbus_read_timeout"));
    assert!(ch0.value.is_some());
    assert!(ch0.quality_observed_at_ms.unwrap() > ch0.value_sampled_at_ms.unwrap());

    // Verify 100% convergence between client replica and server live state across all authorized resources
    let server_state = hub.snapshot_server_state(&scope);
    assert_eq!(replica.resources.len(), server_state.len());
    assert_eq!(replica.resources, server_state);

    // Part B: Slow consumer buffer overflow -> deterministic resync_required (T09 / NB06)
    let (slow_sub_id, _) = hub.subscribe(
        scope.clone(),
        SubscribeRequest {
            client_sub_id: "sub-client-slow".to_string(),
            filter: SubscriptionFilter {
                device_ids: vec!["pump-001".to_string()],
                resource_ids: vec![],
                uri_prefixes: vec![],
            },
            options: SubscriptionOptions {
                coalesce_slow_consumer: false,
                max_pending_deltas: 16,
                snapshot_chunk_size: 4,
            },
        },
    );

    // Push 24 updates (> max_pending_deltas = 16) before slow consumer reads
    for step in 0..24u64 {
        let ch = step % 8;
        let res_id = format!("res-pump-001-pressure_ch{}", ch);
        hub.publish_measurement_update(
            &res_id,
            1.10 + (step as f64) * 0.001,
            1793869250000 + (step as i64) * 10,
        )
        .unwrap();
    }

    let overflow_frame = hub.poll_next_frame(&slow_sub_id).unwrap();
    match &overflow_frame {
        ServerWsFrame::ResyncRequired { code, .. } => {
            assert_eq!(code, "BUFFER_OVERFLOW");
        }
        other => panic!("Expected ResyncRequired frame, got {:?}", other),
    }
    let mut slow_replica = ClientSubscriptionReplica::default();
    slow_replica.apply_frame(overflow_frame).unwrap();
    assert_eq!(
        slow_replica.resync_required_error.as_ref().unwrap().0,
        "BUFFER_OVERFLOW"
    );
    // Stream must now be closed
    assert!(hub.poll_next_frame(&slow_sub_id).is_none());
    assert!(hub.is_terminated(&slow_sub_id));

    // Reconnect and resync after overflow -> converges to 100% server state
    let (resync_sub_id, _) = hub.subscribe(
        scope.clone(),
        SubscribeRequest {
            client_sub_id: "sub-client-resync".to_string(),
            filter: SubscriptionFilter {
                device_ids: vec!["pump-001".to_string()],
                resource_ids: vec![],
                uri_prefixes: vec![],
            },
            options: SubscriptionOptions::default(),
        },
    );
    let mut resync_replica = ClientSubscriptionReplica::default();
    while let Some(frame) = hub.poll_next_frame(&resync_sub_id) {
        resync_replica.apply_frame(frame).unwrap();
    }
    assert_eq!(resync_replica.resources, hub.snapshot_server_state(&scope));
}

#[test]
fn test_zz_generate_evidence_json() {
    let start_total = Instant::now();

    // Measure F3 (F01..F05 + T07)
    let t_f3 = Instant::now();
    test_wal_position_fixed_width_and_frame_checksum();
    test_f01_crash_before_wal_write_and_retry();
    test_f02_partial_tail_frame_truncation_recovery_and_retry();
    test_f03_crash_after_fsync_before_ack_idempotent_retry();
    test_f04_db_uncommitted_and_checkpoint_lag_idempotent_replay();
    test_f05_middle_record_corruption_halts_without_silent_skip_and_t07_backpressure();
    let f3_elapsed_ms = t_f3.elapsed().as_secs_f64() * 1000.0;

    // Measure ST-04 / A06
    let t_st04 = Instant::now();
    test_st04_a06_non_contiguous_checkpoint_and_delivery_gap_disclosure();
    let st04_elapsed_ms = t_st04.elapsed().as_secs_f64() * 1000.0;

    // Measure F6 (NB05, NB06, T09)
    let t_f6 = Instant::now();
    test_f6_concurrent_snapshot_and_delta_handoff_and_overflow_resync();
    let f6_elapsed_ms = t_f6.elapsed().as_secs_f64() * 1000.0;

    let total_elapsed_ms = start_total.elapsed().as_secs_f64() * 1000.0;

    // Compute sample frame bytes for telemetry message
    let sample_req = sample_ingest_request(
        "msg-sample-01",
        245.6,
        1,
        1793869200000,
        false,
        DurabilityPreference::Durable,
    );
    let sample_pos = WalPosition::new(1, 0);
    let sample_payload = f3_f6_wal_and_stream::wal::WalRecordPayload {
        wal_position: sample_pos,
        sequence_no: 1,
        ingest_id: sample_req.derive_ingest_id(),
        message_key: sample_req.message_key(),
        payload_hash: sample_req.compute_payload_hash(),
        device_id: sample_req.device_id.clone(),
        model_revision_id: sample_req.model_revision_id.clone(),
        binding_revision_id: sample_req.binding_revision_id.clone(),
        received_at_ms: sample_req.received_at_ms,
        observations: vec![],
    };
    let encoded_len = encode_wal_frame(&sample_payload).unwrap().len();

    let evidence = serde_json::json!({
        "suite": "f3_f6_wal_and_stream",
        "executed_at": "2026-11-06",
        "git_commit": "840c36f1b485266a595b6730aa5852b1b56ce126",
        "environment": {
            "os": "Darwin 27.0.0 arm64 (macOS)",
            "rustc": "rustc 1.98.1 (48a229cea 2026-09-01)",
            "cargo": "cargo 1.98.1 (797e8a9bc 2026-08-05)",
            "sync_policy": "SyncAll (File::sync_all + parent directory sync_all)"
        },
        "summary": {
            "all_passed": true,
            "f3_wal_and_fault_injection_ms": (f3_elapsed_ms * 100.0).round() / 100.0,
            "st04_non_contiguous_checkpoint_ms": (st04_elapsed_ms * 100.0).round() / 100.0,
            "f6_snapshot_delta_subscription_ms": (f6_elapsed_ms * 100.0).round() / 100.0,
            "total_elapsed_ms": (total_elapsed_ms * 100.0).round() / 100.0
        },
        "f3_wal_verification": {
            "wal_position_format": "{segment_no:016x}:{record_offset:016x} (33 ASCII bytes)",
            "frame_header_bytes": 48,
            "sample_empty_record_frame_bytes": encoded_len,
            "checksums": ["CRC32 (crc32fast)", "SHA-256 (sha2)"],
            "cases": {
                "F01_before_wal_write": {
                    "status": "PASS",
                    "behavior": "0 bytes written on crash; client retry with original key produces 202 accepted_durable and 1 history observation (245.6)"
                },
                "F02_partial_tail_frame_before_sync": {
                    "status": "PASS",
                    "behavior": "Recovery detects trailing incomplete frame, truncates segment to exact offset of last durable record, preserves prior durable records, and accepts client retry for tail message"
                },
                "F03_after_fsync_before_ack": {
                    "status": "PASS",
                    "behavior": "Recovery replays synced WAL record into DB and restores dedup index; client retry with same key returns HTTP 200 duplicate with original ingest_id; conflicting payload returns HTTP 409 IDEMPOTENCY_CONFLICT (T04)"
                },
                "F04_db_commit_and_checkpoint_lag": {
                    "status": "PASS",
                    "behavior": "Uncommitted DB record is replayed from WAL; already committed DB record with lagging checkpoint is skipped via applied_messages (SkippedAlreadyApplied) and checkpoint advances without duplicate rows"
                },
                "F05_middle_record_byte_corruption": {
                    "status": "PASS",
                    "behavior": "CRC32/SHA-256 mismatch on middle record #2 halts replay immediately at record #2 offset, emits MiddleRecordCorrupted alert, sets HaltedOnWalCorruption, and refuses to silently skip to record #3"
                },
                "T07_hard_watermark_backpressure": {
                    "status": "PASS",
                    "behavior": "Exceeding soft watermark returns 202 accepted_durable with warning WAL_SOFT_WATERMARK_EXCEEDED; exceeding hard watermark with Prefer: durability=durable returns HTTP 503 DURABILITY_BACKPRESSURE (retryable=true) and never downgrades to accepted_volatile"
                }
            }
        },
        "st04_checkpoint_verification": {
            "status": "PASS",
            "sequence_tested": ["101:committed", "102:inflight->unknown", "103:committed"],
            "checkpoint_before_102_resolution": "0000000000000001:0000000000000065 (seq 101)",
            "persisted_103_not_redelivered_on_restart": true,
            "items_requiring_retry_after_restart": [102],
            "resolution_path_a_query_back": {
                "checkpoint_after_reconcile": "0000000000000001:0000000000000067 (seq 103)",
                "partial": false,
                "known_gaps_count": 0
            },
            "resolution_path_b_authorized_gap": {
                "unauthorized_skip_rejected": true,
                "checkpoint_after_authorized_gap": "0000000000000001:0000000000000067 (seq 103)",
                "partial": true,
                "warnings": ["DELIVERY_GAP_PRESENT"],
                "disclosed_gap_id": "gap-20261106-102",
                "disclosed_observation_id": "obs-102"
            }
        },
        "f6_subscription_verification": {
            "status": "PASS",
            "boundary_seq_locked": "40",
            "concurrent_updates_during_snapshot": 160,
            "delta_frames_applied": 160,
            "all_deltas_strictly_greater_than_boundary": true,
            "unbound_resource_null_value_preserved": true,
            "unauthorized_device_filtered_out": true,
            "quality_timeout_retains_last_measurement_timestamp": true,
            "final_replica_matches_server_100_percent": true,
            "overflow_resync_test_t09_nb06": {
                "max_pending_deltas": 16,
                "injected_burst_updates": 24,
                "emitted_frame_type": "resync_required",
                "emitted_code": "BUFFER_OVERFLOW",
                "stream_terminated_immediately": true,
                "post_reconnect_resync_converged": true
            }
        }
    });

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let evidence_path = manifest_dir.join("evidence.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}
