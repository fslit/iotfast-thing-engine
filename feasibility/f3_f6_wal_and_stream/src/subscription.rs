use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitSpec {
    pub code: String,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceLiveState {
    pub resource_id: String,
    pub device_id: String,
    pub property: String,
    pub canonical_uri: String,
    pub model_revision_id: String,
    pub binding_revision_id: Option<String>,
    pub resource_revision: u64,
    pub last_stream_seq: u64,
    pub value: Option<f64>,
    pub unit: Option<UnitSpec>,
    pub quality: String,
    pub quality_reason: Option<String>,
    pub freshness: String,
    pub value_sampled_at_ms: Option<i64>,
    pub quality_observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedScopeContext {
    pub tenant_id: String,
    pub caller_service_id: String,
    pub authorized_device_ids: BTreeSet<String>,
    pub authorized_resource_ids: BTreeSet<String>,
}

impl VerifiedScopeContext {
    pub fn allows_resource(&self, res: &ResourceLiveState) -> bool {
        self.authorized_device_ids.contains(&res.device_id)
            || self.authorized_resource_ids.contains(&res.resource_id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionFilter {
    pub device_ids: Vec<String>,
    pub resource_ids: Vec<String>,
    pub uri_prefixes: Vec<String>,
}

impl SubscriptionFilter {
    pub fn matches(&self, res: &ResourceLiveState) -> bool {
        let filter_empty = self.device_ids.is_empty()
            && self.resource_ids.is_empty()
            && self.uri_prefixes.is_empty();
        if filter_empty {
            return true;
        }
        if self.device_ids.iter().any(|d| d == &res.device_id) {
            return true;
        }
        if self.resource_ids.iter().any(|r| r == &res.resource_id) {
            return true;
        }
        if self
            .uri_prefixes
            .iter()
            .any(|p| res.canonical_uri.starts_with(p))
        {
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionOptions {
    pub coalesce_slow_consumer: bool,
    pub max_pending_deltas: usize,
    pub snapshot_chunk_size: usize,
}

impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            coalesce_slow_consumer: false,
            max_pending_deltas: 256,
            snapshot_chunk_size: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub client_sub_id: String,
    pub filter: SubscriptionFilter,
    pub options: SubscriptionOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaFrame {
    pub subscription_id: String,
    pub stream_epoch: String,
    pub stream_seq: String,
    pub stream_seq_num: u64,
    pub change_kind: String, // "measurement_updated" | "quality_changed"
    pub coalesced: bool,
    pub resource_id: String,
    pub device_id: String,
    pub property: String,
    pub model_revision_id: String,
    pub binding_revision_id: Option<String>,
    pub resource_revision: u64,
    pub value: Option<f64>,
    pub unit: Option<UnitSpec>,
    pub quality: String,
    pub quality_reason: Option<String>,
    pub freshness: String,
    pub value_sampled_at_ms: Option<i64>,
    pub quality_observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerWsFrame {
    SnapshotBegin {
        subscription_id: String,
        client_sub_id: String,
        stream_epoch: String,
        total_resources: usize,
    },
    SnapshotChunk {
        subscription_id: String,
        client_sub_id: String,
        stream_epoch: String,
        chunk_index: usize,
        items: Vec<ResourceLiveState>,
    },
    SnapshotEnd {
        subscription_id: String,
        client_sub_id: String,
        stream_epoch: String,
        boundary_seq: String,
        boundary_seq_num: u64,
    },
    Delta(DeltaFrame),
    ResyncRequired {
        subscription_id: String,
        client_sub_id: String,
        stream_epoch: String,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubscriptionPhase {
    SendingSnapshot,
    StreamingDeltas,
    OverflowedPendingNotice,
    Terminated,
}

struct SubscriptionSession {
    subscription_id: String,
    client_sub_id: String,
    scope: VerifiedScopeContext,
    filter: SubscriptionFilter,
    options: SubscriptionOptions,
    boundary_seq: u64,
    phase: SubscriptionPhase,
    pending_snapshot_frames: VecDeque<ServerWsFrame>,
    pending_deltas: VecDeque<DeltaFrame>,
}

struct HubInner {
    stream_epoch: String,
    current_stream_seq: u64,
    next_sub_seq: u64,
    resources: BTreeMap<String, ResourceLiveState>,
    subscriptions: BTreeMap<String, SubscriptionSession>,
}

#[derive(Clone)]
pub struct LiveSubscriptionHub {
    inner: Arc<Mutex<HubInner>>,
}

impl LiveSubscriptionHub {
    pub fn new(stream_epoch: &str) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                stream_epoch: stream_epoch.to_string(),
                current_stream_seq: 0,
                next_sub_seq: 9001,
                resources: BTreeMap::new(),
                subscriptions: BTreeMap::new(),
            })),
        }
    }

    pub fn register_resource(&self, initial_state: ResourceLiveState) {
        let mut guard = self.inner.lock().expect("hub lock");
        guard
            .resources
            .insert(initial_state.resource_id.clone(), initial_state);
    }

    /// Atomically updates a resource measurement in the ordered state domain (`stream_seq` + 1)
    /// and enqueues a `measurement_updated` delta to all active subscriptions with `boundary_seq < stream_seq`.
    pub fn publish_measurement_update(
        &self,
        resource_id: &str,
        value: f64,
        sampled_at_ms: i64,
    ) -> Result<u64, String> {
        let mut guard = self.inner.lock().expect("hub lock");
        guard.current_stream_seq += 1;
        let seq = guard.current_stream_seq;
        let epoch = guard.stream_epoch.clone();

        let updated_res = {
            let res = guard
                .resources
                .get_mut(resource_id)
                .ok_or_else(|| format!("unknown resource_id: {}", resource_id))?;
            res.resource_revision += 1;
            res.last_stream_seq = seq;
            res.value = Some(value);
            res.quality = "good".to_string();
            res.quality_reason = None;
            res.freshness = "fresh".to_string();
            res.value_sampled_at_ms = Some(sampled_at_ms);
            res.quality_observed_at_ms = Some(sampled_at_ms);
            res.clone()
        };

        Self::fanout_delta(&mut guard, &epoch, seq, "measurement_updated", &updated_res);
        Ok(seq)
    }

    /// Atomically updates quality/freshness without fabricating a new measurement timestamp (`T06`).
    pub fn publish_quality_change(
        &self,
        resource_id: &str,
        quality: &str,
        quality_reason: Option<String>,
        freshness: &str,
        observed_at_ms: i64,
    ) -> Result<u64, String> {
        let mut guard = self.inner.lock().expect("hub lock");
        guard.current_stream_seq += 1;
        let seq = guard.current_stream_seq;
        let epoch = guard.stream_epoch.clone();

        let updated_res = {
            let res = guard
                .resources
                .get_mut(resource_id)
                .ok_or_else(|| format!("unknown resource_id: {}", resource_id))?;
            res.resource_revision += 1;
            res.last_stream_seq = seq;
            res.quality = quality.to_string();
            res.quality_reason = quality_reason;
            res.freshness = freshness.to_string();
            res.quality_observed_at_ms = Some(observed_at_ms);
            res.clone()
        };

        Self::fanout_delta(&mut guard, &epoch, seq, "quality_changed", &updated_res);
        Ok(seq)
    }

    fn fanout_delta(
        guard: &mut HubInner,
        epoch: &str,
        seq: u64,
        change_kind: &str,
        res: &ResourceLiveState,
    ) {
        for sub in guard.subscriptions.values_mut() {
            if matches!(
                sub.phase,
                SubscriptionPhase::OverflowedPendingNotice | SubscriptionPhase::Terminated
            ) {
                continue;
            }
            if seq <= sub.boundary_seq {
                continue;
            }
            if !sub.scope.allows_resource(res) || !sub.filter.matches(res) {
                continue;
            }

            let delta = DeltaFrame {
                subscription_id: sub.subscription_id.clone(),
                stream_epoch: epoch.to_string(),
                stream_seq: seq.to_string(),
                stream_seq_num: seq,
                change_kind: change_kind.to_string(),
                coalesced: false,
                resource_id: res.resource_id.clone(),
                device_id: res.device_id.clone(),
                property: res.property.clone(),
                model_revision_id: res.model_revision_id.clone(),
                binding_revision_id: res.binding_revision_id.clone(),
                resource_revision: res.resource_revision,
                value: res.value,
                unit: res.unit.clone(),
                quality: res.quality.clone(),
                quality_reason: res.quality_reason.clone(),
                freshness: res.freshness.clone(),
                value_sampled_at_ms: res.value_sampled_at_ms,
                quality_observed_at_ms: res.quality_observed_at_ms,
            };

            if sub.options.coalesce_slow_consumer {
                if let Some(existing) = sub
                    .pending_deltas
                    .iter_mut()
                    .find(|d| d.resource_id == res.resource_id)
                {
                    let mut coalesced_delta = delta;
                    coalesced_delta.coalesced = true;
                    *existing = coalesced_delta;
                    continue;
                }
            }

            if sub.pending_deltas.len() >= sub.options.max_pending_deltas {
                // T09 / NB06: Buffer overflow during snapshot or slow consumption!
                // Deterministically transition to OverflowedPendingNotice and clear pending frames.
                sub.pending_deltas.clear();
                sub.pending_snapshot_frames.clear();
                sub.phase = SubscriptionPhase::OverflowedPendingNotice;
            } else {
                sub.pending_deltas.push_back(delta);
            }
        }
    }

    /// Step 1 & 2 of F6: Register subscription and atomically lock boundary `S` in the ordered state domain.
    pub fn subscribe(
        &self,
        scope: VerifiedScopeContext,
        req: SubscribeRequest,
    ) -> (String, u64) {
        let mut guard = self.inner.lock().expect("hub lock");
        let sub_id = format!("sub-srv-{}", guard.next_sub_seq);
        guard.next_sub_seq += 1;

        let boundary_seq = guard.current_stream_seq;
        let epoch = guard.stream_epoch.clone();

        let matching_resources: Vec<ResourceLiveState> = guard
            .resources
            .values()
            .filter(|r| scope.allows_resource(r) && req.filter.matches(r))
            .cloned()
            .collect();

        let mut snapshot_frames = VecDeque::new();
        snapshot_frames.push_back(ServerWsFrame::SnapshotBegin {
            subscription_id: sub_id.clone(),
            client_sub_id: req.client_sub_id.clone(),
            stream_epoch: epoch.clone(),
            total_resources: matching_resources.len(),
        });

        let chunk_size = req.options.snapshot_chunk_size.max(1);
        for (chunk_idx, chunk) in matching_resources.chunks(chunk_size).enumerate() {
            snapshot_frames.push_back(ServerWsFrame::SnapshotChunk {
                subscription_id: sub_id.clone(),
                client_sub_id: req.client_sub_id.clone(),
                stream_epoch: epoch.clone(),
                chunk_index: chunk_idx,
                items: chunk.to_vec(),
            });
        }

        snapshot_frames.push_back(ServerWsFrame::SnapshotEnd {
            subscription_id: sub_id.clone(),
            client_sub_id: req.client_sub_id.clone(),
            stream_epoch: epoch,
            boundary_seq: boundary_seq.to_string(),
            boundary_seq_num: boundary_seq,
        });

        let session = SubscriptionSession {
            subscription_id: sub_id.clone(),
            client_sub_id: req.client_sub_id,
            scope,
            filter: req.filter,
            options: req.options,
            boundary_seq,
            phase: SubscriptionPhase::SendingSnapshot,
            pending_snapshot_frames: snapshot_frames,
            pending_deltas: VecDeque::new(),
        };

        guard.subscriptions.insert(sub_id.clone(), session);
        (sub_id, boundary_seq)
    }

    /// Polls the next frame for `subscription_id`.
    /// Delivers `snapshot_begin` -> `snapshot_chunk`* -> `snapshot_end(boundary_seq=S)` -> `delta(stream_seq > S)`*,
    /// or `resync_required(code="BUFFER_OVERFLOW")` followed by `None` (stream closed) if overflow occurred.
    pub fn poll_next_frame(&self, subscription_id: &str) -> Option<ServerWsFrame> {
        let mut guard = self.inner.lock().expect("hub lock");
        let epoch = guard.stream_epoch.clone();
        let sub = guard.subscriptions.get_mut(subscription_id)?;

        match sub.phase {
            SubscriptionPhase::Terminated => None,
            SubscriptionPhase::OverflowedPendingNotice => {
                sub.phase = SubscriptionPhase::Terminated;
                Some(ServerWsFrame::ResyncRequired {
                    subscription_id: sub.subscription_id.clone(),
                    client_sub_id: sub.client_sub_id.clone(),
                    stream_epoch: epoch,
                    code: "BUFFER_OVERFLOW".to_string(),
                    message: "Pending delta buffer exceeded capacity during snapshot or slow consumption; reconnect and fetch a new snapshot.".to_string(),
                })
            }
            SubscriptionPhase::SendingSnapshot => {
                if let Some(frame) = sub.pending_snapshot_frames.pop_front() {
                    if matches!(frame, ServerWsFrame::SnapshotEnd { .. }) {
                        sub.phase = SubscriptionPhase::StreamingDeltas;
                    }
                    Some(frame)
                } else {
                    sub.phase = SubscriptionPhase::StreamingDeltas;
                    sub.pending_deltas.pop_front().map(ServerWsFrame::Delta)
                }
            }
            SubscriptionPhase::StreamingDeltas => {
                sub.pending_deltas.pop_front().map(ServerWsFrame::Delta)
            }
        }
    }

    pub fn is_terminated(&self, subscription_id: &str) -> bool {
        let guard = self.inner.lock().expect("hub lock");
        guard
            .subscriptions
            .get(subscription_id)
            .map(|s| s.phase == SubscriptionPhase::Terminated)
            .unwrap_or(true)
    }

    pub fn snapshot_server_state(&self, scope: &VerifiedScopeContext) -> BTreeMap<String, ResourceLiveState> {
        let guard = self.inner.lock().expect("hub lock");
        guard
            .resources
            .iter()
            .filter(|(_, r)| scope.allows_resource(r))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Client-side state machine that consumes `ServerWsFrame`s and verifies F6 snapshot-to-delta invariants.
#[derive(Debug, Default)]
pub struct ClientSubscriptionReplica {
    pub subscription_id: Option<String>,
    pub stream_epoch: Option<String>,
    pub snapshot_started: bool,
    pub snapshot_completed: bool,
    pub boundary_seq: Option<u64>,
    pub last_delta_seq: Option<u64>,
    pub delta_frames_applied: usize,
    pub resync_required_error: Option<(String, String)>,
    pub resources: BTreeMap<String, ResourceLiveState>,
}

impl ClientSubscriptionReplica {
    pub fn apply_frame(&mut self, frame: ServerWsFrame) -> Result<(), String> {
        match frame {
            ServerWsFrame::SnapshotBegin {
                subscription_id,
                stream_epoch,
                ..
            } => {
                if self.snapshot_started {
                    return Err("Duplicate snapshot_begin received".to_string());
                }
                self.subscription_id = Some(subscription_id);
                self.stream_epoch = Some(stream_epoch);
                self.snapshot_started = true;
                self.resources.clear();
                Ok(())
            }
            ServerWsFrame::SnapshotChunk { items, .. } => {
                if !self.snapshot_started || self.snapshot_completed {
                    return Err("snapshot_chunk received outside active snapshot phase".to_string());
                }
                for item in items {
                    self.resources.insert(item.resource_id.clone(), item);
                }
                Ok(())
            }
            ServerWsFrame::SnapshotEnd {
                boundary_seq_num, ..
            } => {
                if !self.snapshot_started || self.snapshot_completed {
                    return Err("Invalid snapshot_end transition".to_string());
                }
                self.snapshot_completed = true;
                self.boundary_seq = Some(boundary_seq_num);
                Ok(())
            }
            ServerWsFrame::Delta(delta) => {
                if !self.snapshot_completed {
                    return Err("Delta received before snapshot_end!".to_string());
                }
                let boundary = self.boundary_seq.ok_or("Missing boundary_seq")?;
                if delta.stream_seq_num <= boundary {
                    return Err(format!(
                        "Invariant violation: delta stream_seq {} <= boundary_seq {}",
                        delta.stream_seq_num, boundary
                    ));
                }
                if let Some(prev_seq) = self.last_delta_seq {
                    if delta.stream_seq_num <= prev_seq {
                        return Err(format!(
                            "Non-monotonic delta stream_seq: {} <= {}",
                            delta.stream_seq_num, prev_seq
                        ));
                    }
                }
                self.last_delta_seq = Some(delta.stream_seq_num);

                let entry = self
                    .resources
                    .get_mut(&delta.resource_id)
                    .ok_or_else(|| format!("Delta for unknown resource {}", delta.resource_id))?;

                if delta.resource_revision <= entry.resource_revision {
                    return Err(format!(
                        "Resource revision regression on {}: delta rev {} <= current rev {}",
                        delta.resource_id, delta.resource_revision, entry.resource_revision
                    ));
                }

                entry.resource_revision = delta.resource_revision;
                entry.last_stream_seq = delta.stream_seq_num;
                entry.value = delta.value;
                entry.unit = delta.unit;
                entry.quality = delta.quality;
                entry.quality_reason = delta.quality_reason;
                entry.freshness = delta.freshness;
                entry.value_sampled_at_ms = delta.value_sampled_at_ms;
                entry.quality_observed_at_ms = delta.quality_observed_at_ms;
                self.delta_frames_applied += 1;
                Ok(())
            }
            ServerWsFrame::ResyncRequired { code, message, .. } => {
                self.resync_required_error = Some((code, message));
                Ok(())
            }
        }
    }
}
