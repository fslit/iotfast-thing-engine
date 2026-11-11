use crate::error::{EngineError, ErrorCode};
use crate::scope::VerifiedScopeContext;
use crate::storage_duckdb::EngineRuntime;
use duckdb::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub resource_ids: Vec<String>,
    pub include_initial_snapshot: bool,
    pub max_pending_events: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotResourceItem {
    pub resource_id: String,
    pub resource_revision: u64,
    pub value: Value,
    pub quality: String,
    pub freshness: String,
    pub value_sampled_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubscriptionEvent {
    SnapshotBegin {
        stream_epoch: String,
        boundary_seq: u64,
    },
    SnapshotChunk {
        items: Vec<SnapshotResourceItem>,
    },
    SnapshotEnd {
        boundary_seq: u64,
    },
    Delta {
        stream_seq: u64,
        resource_id: String,
        value: Value,
        quality: String,
        observed_at_ms: i64,
    },
    ResyncRequired {
        code: String,
        reason: String,
    },
}

struct SubscriberSession {
    tenant_id: String,
    resource_ids: BTreeSet<String>,
    boundary_seq: u64,
    max_pending_events: usize,
    expires_at_ms: i64,
    terminated: bool,
    queue: VecDeque<SubscriptionEvent>,
}

pub struct SubscriptionHub {
    next_sub_seq: u64,
    sessions: BTreeMap<String, SubscriberSession>,
}

impl Default for SubscriptionHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionHub {
    pub fn new() -> Self {
        Self {
            next_sub_seq: 1,
            sessions: BTreeMap::new(),
        }
    }

    /// Strict snapshot at boundary `S` (`R04`, `NB05`):
    /// Captures all authorized resources at `boundary_seq = S` (verifying `resource_revision <= S`),
    /// emits `SnapshotBegin(S)` -> `SnapshotChunk(<= S)` -> `SnapshotEnd(S)`, and registers subscriber
    /// for subsequent `Delta(stream_seq > S)`.
    pub fn subscribe(
        &mut self,
        ctx: &VerifiedScopeContext,
        req: SubscribeRequest,
        rt: &EngineRuntime,
        now_ms: i64,
    ) -> Result<String, EngineError> {
        ctx.validate_active(now_ms)?;
        ctx.require_action("subscription:connect")?;

        if req.resource_ids.is_empty() || req.max_pending_events == 0 {
            return Err(EngineError::new(
                "req-sub",
                ErrorCode::ValidationFailed,
                "Invalid subscription resource_ids or max_pending_events",
                vec![],
            ));
        }

        // Verify all requested resource_ids are within Scope (R05: uniform 404 on any unauthorized ID)
        for res_id in &req.resource_ids {
            if !rt.is_resource_in_scope(ctx, res_id)? {
                return Err(EngineError::new(
                    "req-sub",
                    ErrorCode::ResourceNotFound,
                    "One or more subscribed resource_ids were not found",
                    vec![],
                ));
            }
        }

        let boundary_s = rt.stream_seq;
        let mut queue = VecDeque::new();

        if req.include_initial_snapshot {
            queue.push_back(SubscriptionEvent::SnapshotBegin {
                stream_epoch: rt.stream_epoch.clone(),
                boundary_seq: boundary_s,
            });

            let mut snap_items = Vec::with_capacity(req.resource_ids.len());
            for res_id in &req.resource_ids {
                let latest_row: Option<(String, i64, i64)> = rt
                    .conn()
                    .query_row(
                        "SELECT value_json, value_sampled_at_ms, resource_revision FROM latest_value WHERE resource_id = ?",
                        params![res_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();
                let q_state: String = rt
                    .conn()
                    .query_row(
                        "SELECT quality_state FROM point_quality_state WHERE resource_id = ?",
                        params![res_id],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|_| "unknown".to_string());

                match latest_row {
                    None => {
                        snap_items.push(SnapshotResourceItem {
                            resource_id: res_id.clone(),
                            resource_revision: 0,
                            value: Value::Null,
                            quality: q_state,
                            freshness: "unknown".to_string(),
                            value_sampled_at_ms: None,
                        });
                    }
                    Some((val_json, sampled_at, res_rev)) => {
                        let rev_u64 = res_rev as u64;
                        // R04: Strict snapshot requires every resource in snapshot to satisfy `seq_r <= S`
                        if rev_u64 > boundary_s {
                            let sub_id = format!("sub-{:06}", self.next_sub_seq);
                            self.next_sub_seq += 1;
                            let mut fail_q = VecDeque::new();
                            fail_q.push_back(SubscriptionEvent::ResyncRequired {
                                code: "SNAPSHOT_BOUNDARY_VIOLATED".to_string(),
                                reason: "Resource revision exceeded snapshot boundary S (R04)"
                                    .to_string(),
                            });
                            self.sessions.insert(
                                sub_id.clone(),
                                SubscriberSession {
                                    tenant_id: ctx.tenant_id.clone(),
                                    resource_ids: BTreeSet::new(),
                                    boundary_seq: boundary_s,
                                    max_pending_events: req.max_pending_events,
                                    expires_at_ms: ctx.expires_at_ms,
                                    terminated: true,
                                    queue: fail_q,
                                },
                            );
                            return Ok(sub_id);
                        }
                        // I6: Check source_gateway connection_state for freshness
                        let source_disconnected = {
                            // Look up device's active binding source
                            let dev_source: Option<(String, String)> = rt.conn()
                                .query_row(
                                    "SELECT br.source_id, sg.connection_state
                                     FROM resource r
                                     JOIN device d ON r.device_id = d.device_id
                                     JOIN binding_revision br ON d.active_binding_revision_id = br.binding_revision_id
                                     JOIN source_gateway sg ON sg.tenant_id = d.tenant_id AND sg.source_id = br.source_id
                                     WHERE r.resource_id = ?",
                                    params![res_id],
                                    |r| Ok((r.get(0)?, r.get(1)?)),
                                )
                                .ok();
                            dev_source.map_or(false, |(_, cs)| cs == "disconnected")
                        };
                        let freshness = if q_state == "bad"
                            || source_disconnected
                            || (now_ms - sampled_at) > rt.config.stale_threshold_ms
                        {
                            "stale"
                        } else {
                            "fresh"
                        };
                        snap_items.push(SnapshotResourceItem {
                            resource_id: res_id.clone(),
                            resource_revision: rev_u64,
                            value: serde_json::from_str(&val_json).unwrap_or(Value::Null),
                            quality: q_state,
                            freshness: freshness.to_string(),
                            value_sampled_at_ms: Some(sampled_at),
                        });
                    }
                }
            }

            queue.push_back(SubscriptionEvent::SnapshotChunk { items: snap_items });
            queue.push_back(SubscriptionEvent::SnapshotEnd {
                boundary_seq: boundary_s,
            });
        }

        let sub_id = format!("sub-{:06}", self.next_sub_seq);
        self.next_sub_seq += 1;
        self.sessions.insert(
            sub_id.clone(),
            SubscriberSession {
                tenant_id: ctx.tenant_id.clone(),
                resource_ids: req.resource_ids.into_iter().collect(),
                boundary_seq: boundary_s,
                max_pending_events: req.max_pending_events,
                expires_at_ms: ctx.expires_at_ms,
                terminated: false,
                queue,
            },
        );
        Ok(sub_id)
    }

    pub fn publish_point_delta(
        &mut self,
        tenant_id: &str,
        resource_id: &str,
        stream_seq: u64,
        value: Value,
        quality: &str,
        observed_at_ms: i64,
    ) {
        for session in self.sessions.values_mut() {
            if session.terminated
                || session.tenant_id != tenant_id
                || !session.resource_ids.contains(resource_id)
                || stream_seq <= session.boundary_seq
            {
                continue;
            }

            // I6/SC10: Check credential expiry
            if observed_at_ms > session.expires_at_ms {
                session.queue.clear();
                session.queue.push_back(SubscriptionEvent::ResyncRequired {
                    code: "CREDENTIAL_EXPIRED".to_string(),
                    reason: "Subscriber scope credential expired (SC10)".to_string(),
                });
                session.terminated = true;
                continue;
            }

            let pending_deltas = session
                .queue
                .iter()
                .filter(|e| matches!(e, SubscriptionEvent::Delta { .. }))
                .count();

            if pending_deltas >= session.max_pending_events {
                // NB06 / T09: Bounded buffer overflow emits ResyncRequired and terminates stream
                session.queue.clear();
                session.queue.push_back(SubscriptionEvent::ResyncRequired {
                    code: "BUFFER_OVERFLOW".to_string(),
                    reason: format!(
                        "Pending delta queue exceeded max_pending_events={}",
                        session.max_pending_events
                    ),
                });
                session.terminated = true;
                continue;
            }

            session.queue.push_back(SubscriptionEvent::Delta {
                stream_seq,
                resource_id: resource_id.to_string(),
                value: value.clone(),
                quality: quality.to_string(),
                observed_at_ms,
            });
        }
    }

    pub fn drain_events(&mut self, sub_id: &str) -> Vec<SubscriptionEvent> {
        match self.sessions.get_mut(sub_id) {
            Some(s) => s.queue.drain(..).collect(),
            None => Vec::new(),
        }
    }

    pub fn is_terminated(&self, sub_id: &str) -> bool {
        self.sessions.get(sub_id).map_or(true, |s| s.terminated)
    }
}

impl SubscriptionEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, SubscriptionEvent::ResyncRequired { .. })
    }

    pub fn to_wire_json(
        &self,
        sub_id: &str,
        client_sub_id: Option<&str>,
        stream_epoch: &str,
    ) -> Value {
        let csub = client_sub_id.unwrap_or(sub_id);
        match self {
            SubscriptionEvent::SnapshotBegin {
                stream_epoch,
                boundary_seq,
            } => serde_json::json!({
                "type": "snapshot_begin",
                "sub_id": csub,
                "server_sub_id": sub_id,
                "stream_epoch": stream_epoch,
                "snapshot_boundary_seq": boundary_seq,
            }),
            SubscriptionEvent::SnapshotChunk { items } => serde_json::json!({
                "type": "snapshot_chunk",
                "sub_id": csub,
                "server_sub_id": sub_id,
                "items": items,
            }),
            SubscriptionEvent::SnapshotEnd { boundary_seq } => serde_json::json!({
                "type": "snapshot_end",
                "sub_id": csub,
                "server_sub_id": sub_id,
                "snapshot_boundary_seq": boundary_seq,
            }),
            SubscriptionEvent::Delta {
                stream_seq,
                resource_id,
                value,
                quality,
                observed_at_ms,
            } => serde_json::json!({
                "type": "delta",
                "sub_id": csub,
                "server_sub_id": sub_id,
                "stream_epoch": stream_epoch,
                "stream_seq": stream_seq,
                "resource_id": resource_id,
                "value": value,
                "quality": quality,
                "observed_at_ms": observed_at_ms,
            }),
            SubscriptionEvent::ResyncRequired { code, reason } => {
                // Q05 / SC10: CREDENTIAL_EXPIRED and SCOPE_REVOKED map to subscription_terminated on the wire
                if code == "CREDENTIAL_EXPIRED" || code == "SCOPE_REVOKED" {
                    serde_json::json!({
                        "type": "subscription_terminated",
                        "sub_id": csub,
                        "server_sub_id": sub_id,
                        "reason": code,
                        "message": reason,
                        "action": "reauthenticate_and_resubscribe",
                    })
                } else {
                    serde_json::json!({
                        "type": "resync_required",
                        "sub_id": csub,
                        "server_sub_id": sub_id,
                        "reason": code,
                        "message": reason,
                        "action": "reconnect_and_resubscribe_with_snapshot",
                    })
                }
            }
        }
    }
}
