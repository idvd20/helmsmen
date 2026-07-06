//! Control-plane event reducer (M3, task #15) — the approval-inbox card
//! lifecycle and Session status, as pure data.
//!
//! This is the PURE CORE half of the M3 control plane. The imperative shell
//! (`modules::hooks`) owns the loopback endpoint: it authenticates a hook
//! POST, caps its size, and parses the hostile JSON body into one of the
//! typed [`HookEvent`]s defined here. Everything below is `data in -> data
//! out`: no I/O, no async, nothing here ever executes anything. A hook
//! payload is DATA, never an instruction — the strongest form of that rule
//! is that this module cannot perform a side effect even if it wanted to.
//!
//! # Lifted from the approval-loop spike
//!
//! The `spike-approval-loop/correlate.js` reducer (verdict PASS) is the
//! authoritative shape: the same card lifecycle
//! `Pending -> Surfaced -> Allowed -> ClosedNoRun`, the same strict
//! `tool_use_id` correlation, and the same discipline that a
//! `Notification(permission)` is a status-only signal that never *sources* a
//! card. The spike's one residual ambiguity — it surfaced the *oldest*
//! pending card on a permission notification and warned when more than one
//! was open (parallel tool calls) — is removed here: a permission
//! notification carries no tool identity, so it surfaces *every* pending
//! card in the session (no per-card guess), and resolution is by
//! `tool_use_id` alone. Replaying the spike's captured multi-call session
//! through this reducer therefore produces zero warnings — the criterion-4
//! pass signal. Any genuine ambiguity (a result with no `tool_use_id`, or a
//! result that matches no open approval) still pushes a warning: the
//! [`ControlPlaneState::warnings`] channel hides nothing.
//!
//! # Event -> transition mapping (PRD M3 table)
//!
//! | Event                            | Session status         | Card effect                     |
//! | -------------------------------- | ---------------------- | ------------------------------- |
//! | `PreToolUse` (activity)          | Working                | enqueue a Pending approval      |
//! | `Notification(permission)`       | Blocked                | surface the session's Pending   |
//! | `Notification(idle)`             | Blocked (input-wait)   | none                            |
//! | `PostToolUse`                    | Working                | matching approval -> Allowed    |
//! | `Stop`                           | Done                   | unresolved approvals -> Closed  |
//!
//! The status column maps to the existing [`SessionSignal`] seam
//! ([`hook_event_signal`]) so the control plane feeds
//! `core::cut::session_status_from_signal` + `roll_up_status` exactly as the
//! M2 agent-signal source did — both sources coexist during the M2->M3
//! swap. `input-wait` has no dot of its own in the current status
//! vocabulary, so it folds into `Attention` -> "Needs you" alongside a
//! permission prompt; the two stay distinct at the *card* level (permission
//! surfaces approvals, idle does not).

use serde::Serialize;

use super::cut::SessionSignal;

/// Session id used when a hook payload omits one. Correlation still works —
/// unkeyed events simply group together — but a real per-Workspace endpoint
/// always carries its Session's id.
pub const UNKNOWN_SESSION: &str = "unknown-session";

/// Which kind of `Notification` a hook payload carried. A notification is a
/// status-only signal: it never sources an approval card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationKind {
    /// "Claude needs your permission" — the Session is blocked awaiting an
    /// approval decision. Carries no tool identity and is not 1:1 with
    /// asks, so it surfaces the whole session's pending approvals rather
    /// than guessing one.
    Permission,
    /// "Claude is waiting for your input" — an idle input-wait.
    Idle,
    /// Any other notification: status-neutral, and a no-op for the reducer.
    Other,
}

/// One parsed control-plane event. Constructed only by the hooks shell from
/// an authenticated, size-capped, typed-parsed payload — never deserialized
/// directly from data (no `Deserialize`), mirroring the Serialize-only
/// discipline of [`SessionSignal`]. This is the boundary the PRD calls
/// "core receives already-parsed typed events".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum HookEventKind {
    /// A tool call is about to run. Activity (-> Working) that also enqueues
    /// a pending approval keyed by `tool_use_id`.
    PreToolUse {
        tool_use_id: Option<String>,
        tool_name: String,
    },
    /// A status-only notification (see [`NotificationKind`]).
    Notification { notification: NotificationKind },
    /// A tool call finished — the Allow path completed for its
    /// `tool_use_id`.
    PostToolUse {
        tool_use_id: Option<String>,
        tool_name: String,
    },
    /// The agent's turn ended.
    Stop,
}

/// A control-plane event with its session and a monotonic sequence number
/// (assigned by the shell at receipt; used for stable card ids and warning
/// provenance). See [`HookEventKind`] for the trust-boundary note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEvent {
    pub seq: u64,
    pub session_id: String,
    pub kind: HookEventKind,
}

impl HookEvent {
    /// Convenience constructor for the shell and tests.
    pub fn new(seq: u64, session_id: impl Into<String>, kind: HookEventKind) -> Self {
        Self {
            seq,
            session_id: session_id.into(),
            kind,
        }
    }
}

/// Where an approval card is in its lifecycle. Serialize-only for the
/// frontend mirror (the Approval Inbox UI lands at M3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CardStatus {
    /// `PreToolUse` enqueued the approval; no permission prompt seen yet.
    Pending,
    /// A permission `Notification` confirmed the Session is blocked on this
    /// (and every other still-pending) approval.
    Surfaced,
    /// `PostToolUse` observed for the same `tool_use_id` — the Allow path
    /// completed, the call ran.
    Allowed,
    /// The Session hit `Stop` with the approval unresolved — denied or
    /// dismissed, the call never ran.
    ClosedNoRun,
}

impl CardStatus {
    /// An approval still awaiting a decision (the set a permission prompt
    /// surfaces and a `Stop` closes).
    fn is_open(self) -> bool {
        matches!(self, CardStatus::Pending | CardStatus::Surfaced)
    }
}

/// One Approval Inbox card: a single tool call that needed a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCard {
    /// Stable id, derived from the sourcing event's sequence number.
    pub id: String,
    pub seq: u64,
    pub session_id: String,
    pub tool_name: String,
    /// The correlation key. `None` means the sourcing `PreToolUse` carried
    /// no `tool_use_id` (flagged in `warnings`); such a card can never be
    /// resolved by the strict rule and closes at `Stop`.
    pub tool_use_id: Option<String>,
    pub status: CardStatus,
}

/// The whole control-plane reduction: derived approval cards plus a warnings
/// channel that flags every correlation ambiguity. Serialize-only; this is
/// derived state, never persisted in the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneState {
    pub cards: Vec<ApprovalCard>,
    pub warnings: Vec<String>,
    pub event_count: u64,
}

/// The initial (empty) control-plane state.
pub fn empty_state() -> ControlPlaneState {
    ControlPlaneState::default()
}

/// First 8 chars of a session id, for readable warnings.
fn short(sid: &str) -> &str {
    let end = sid
        .char_indices()
        .nth(8)
        .map(|(i, _)| i)
        .unwrap_or(sid.len());
    &sid[..end]
}

/// The only way control-plane state changes: fold one [`HookEvent`] into the
/// state. Pure and total. Replay-tolerant: a duplicated event (same
/// `tool_use_id` re-sourced, a repeated result, a second `Stop`) never
/// corrupts state — it is deduplicated or is a no-op.
pub fn apply_hook_event(prev: ControlPlaneState, event: HookEvent) -> ControlPlaneState {
    let mut state = prev;
    state.event_count += 1;

    let sid = if event.session_id.is_empty() {
        UNKNOWN_SESSION.to_string()
    } else {
        event.session_id
    };

    match event.kind {
        HookEventKind::PreToolUse {
            tool_use_id,
            tool_name,
        } => {
            // Idempotent enqueue: a replayed PreToolUse for a tool_use_id we
            // already carry (in any state) must not source a second card.
            if let Some(tuid) = tool_use_id.as_deref() {
                let already = state
                    .cards
                    .iter()
                    .any(|c| c.session_id == sid && c.tool_use_id.as_deref() == Some(tuid));
                if already {
                    return state; // replay tolerated: no change
                }
            } else {
                state.warnings.push(format!(
                    "seq {}: PreToolUse for {:?} in session {} carries no tool_use_id — \
                     the approval cannot be correlated by the strict tool_use_id rule",
                    event.seq,
                    tool_name,
                    short(&sid)
                ));
            }
            state.cards.push(ApprovalCard {
                id: format!("card-{}", event.seq),
                seq: event.seq,
                session_id: sid,
                tool_name,
                tool_use_id,
                status: CardStatus::Pending,
            });
        }

        HookEventKind::Notification { notification } => {
            // Status-only. A permission prompt never sources a card; it
            // surfaces EVERY pending approval in the session (no per-card
            // guess -> no ambiguity warning even with parallel tool calls).
            // Idle / Other touch no card at all.
            if notification == NotificationKind::Permission {
                for card in state.cards.iter_mut() {
                    if card.session_id == sid && card.status == CardStatus::Pending {
                        card.status = CardStatus::Surfaced;
                    }
                }
            }
        }

        HookEventKind::PostToolUse {
            tool_use_id,
            tool_name,
        } => {
            let Some(tuid) = tool_use_id else {
                // Strict rule: no tool_use_id means no correlation. Do not
                // guess; flag it.
                state.warnings.push(format!(
                    "seq {}: PostToolUse for {:?} in session {} carries no tool_use_id — \
                     cannot correlate (strict tool_use_id rule); ignored",
                    event.seq,
                    tool_name,
                    short(&sid)
                ));
                return state;
            };
            let open = state.cards.iter().position(|c| {
                c.session_id == sid
                    && c.tool_use_id.as_deref() == Some(tuid.as_str())
                    && c.status.is_open()
            });
            match open {
                Some(i) => state.cards[i].status = CardStatus::Allowed,
                None => {
                    // A repeated result for an already-Allowed call is a
                    // tolerated replay; a result matching nothing at all is
                    // a genuine orphan and gets flagged.
                    let replay = state.cards.iter().any(|c| {
                        c.session_id == sid
                            && c.tool_use_id.as_deref() == Some(tuid.as_str())
                            && c.status == CardStatus::Allowed
                    });
                    if !replay {
                        state.warnings.push(format!(
                            "seq {}: PostToolUse tool_use_id {:?} in session {} matched no \
                             open approval — ignored (strict tool_use_id rule)",
                            event.seq,
                            tuid,
                            short(&sid)
                        ));
                    }
                }
            }
        }

        HookEventKind::Stop => {
            for card in state.cards.iter_mut() {
                if card.session_id == sid && card.status.is_open() {
                    card.status = CardStatus::ClosedNoRun;
                }
            }
        }
    }

    state
}

/// Map a control-plane event to the [`SessionSignal`] it implies for the
/// Session's wall dot — the M3 replacement source for the M2 agent-signal.
/// Returns `None` when an event carries no status meaning (a non-permission,
/// non-idle notification). The result feeds
/// `core::cut::session_status_from_signal` unchanged.
pub fn hook_event_signal(kind: &HookEventKind) -> Option<SessionSignal> {
    match kind {
        // Activity — the agent is running a tool or processing its result.
        HookEventKind::PreToolUse { .. } | HookEventKind::PostToolUse { .. } => {
            Some(SessionSignal::Working)
        }
        HookEventKind::Notification { notification } => match notification {
            // Both a permission prompt and an idle input-wait need the user
            // ("Needs you"); they fold into Attention until the status
            // vocabulary grows an input-wait dot.
            NotificationKind::Permission | NotificationKind::Idle => Some(SessionSignal::Attention),
            NotificationKind::Other => None,
        },
        HookEventKind::Stop => Some(SessionSignal::Finished),
    }
}

#[cfg(test)]
mod tests {
    use super::super::cut::{roll_up_status, session_status_from_signal, WorkspaceStatus};
    use super::*;

    const SESSION: &str = "bb6de6a5-789a-4bcf-97cf-2eca27d74234";

    fn pre(seq: u64, tuid: &str, tool: &str) -> HookEvent {
        HookEvent::new(
            seq,
            SESSION,
            HookEventKind::PreToolUse {
                tool_use_id: Some(tuid.to_string()),
                tool_name: tool.to_string(),
            },
        )
    }

    fn post(seq: u64, tuid: &str, tool: &str) -> HookEvent {
        HookEvent::new(
            seq,
            SESSION,
            HookEventKind::PostToolUse {
                tool_use_id: Some(tuid.to_string()),
                tool_name: tool.to_string(),
            },
        )
    }

    fn note(seq: u64, kind: NotificationKind) -> HookEvent {
        HookEvent::new(
            seq,
            SESSION,
            HookEventKind::Notification { notification: kind },
        )
    }

    fn stop(seq: u64) -> HookEvent {
        HookEvent::new(seq, SESSION, HookEventKind::Stop)
    }

    fn replay(events: &[HookEvent]) -> ControlPlaneState {
        events
            .iter()
            .cloned()
            .fold(empty_state(), apply_hook_event)
    }

    fn card<'a>(state: &'a ControlPlaneState, tuid: &str) -> &'a ApprovalCard {
        state
            .cards
            .iter()
            .find(|c| c.tool_use_id.as_deref() == Some(tuid))
            .unwrap_or_else(|| panic!("no card for {tuid}"))
    }

    // --- per-event-type: card lifecycle ---

    #[test]
    fn pretooluse_enqueues_a_pending_approval() {
        let s = replay(&[pre(1, "toolu_a", "Bash")]);
        assert_eq!(s.cards.len(), 1);
        assert_eq!(s.cards[0].status, CardStatus::Pending);
        assert_eq!(s.cards[0].tool_name, "Bash");
        assert_eq!(s.cards[0].id, "card-1");
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn permission_notification_surfaces_every_pending_card_without_warning() {
        // Two parallel calls, then one permission prompt: the spike's
        // ambiguity case. Surfacing ALL pending cards means no guess and no
        // warning.
        let s = replay(&[
            pre(1, "toolu_a", "Bash"),
            pre(2, "toolu_b", "Bash"),
            note(3, NotificationKind::Permission),
        ]);
        assert_eq!(card(&s, "toolu_a").status, CardStatus::Surfaced);
        assert_eq!(card(&s, "toolu_b").status, CardStatus::Surfaced);
        assert!(s.warnings.is_empty(), "no per-card guess -> no warning");
    }

    #[test]
    fn permission_notification_never_sources_a_card() {
        // Status-only: with nothing pending, a permission prompt fabricates
        // no card.
        let s = replay(&[note(1, NotificationKind::Permission)]);
        assert!(s.cards.is_empty());
        assert!(s.warnings.is_empty());
        // Idle likewise touches no card.
        let s = replay(&[pre(1, "toolu_a", "Bash"), note(2, NotificationKind::Idle)]);
        assert_eq!(s.cards.len(), 1);
        assert_eq!(card(&s, "toolu_a").status, CardStatus::Pending);
    }

    #[test]
    fn posttooluse_allows_the_matching_approval_by_tool_use_id() {
        let s = replay(&[
            pre(1, "toolu_a", "Bash"),
            note(2, NotificationKind::Permission),
            post(3, "toolu_a", "Bash"),
        ]);
        assert_eq!(card(&s, "toolu_a").status, CardStatus::Allowed);
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn stop_closes_unresolved_approvals() {
        let s = replay(&[
            pre(1, "toolu_a", "Bash"),
            note(2, NotificationKind::Permission),
            stop(3),
        ]);
        assert_eq!(card(&s, "toolu_a").status, CardStatus::ClosedNoRun);
        assert!(s.warnings.is_empty());
    }

    // --- warning discipline (genuine ambiguities are never hidden) ---

    #[test]
    fn posttooluse_without_tool_use_id_is_flagged_not_guessed() {
        let s = replay(&[
            pre(1, "toolu_a", "Bash"),
            HookEvent::new(
                2,
                SESSION,
                HookEventKind::PostToolUse {
                    tool_use_id: None,
                    tool_name: "Bash".to_string(),
                },
            ),
        ]);
        // The pending card is untouched (no guess); the ambiguity is flagged.
        assert_eq!(card(&s, "toolu_a").status, CardStatus::Pending);
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].contains("no tool_use_id"));
    }

    #[test]
    fn orphan_posttooluse_is_flagged() {
        let s = replay(&[post(1, "toolu_ghost", "Bash")]);
        assert!(s.cards.is_empty());
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].contains("matched no"));
    }

    #[test]
    fn pretooluse_without_tool_use_id_is_flagged() {
        let s = replay(&[HookEvent::new(
            1,
            SESSION,
            HookEventKind::PreToolUse {
                tool_use_id: None,
                tool_name: "Bash".to_string(),
            },
        )]);
        assert_eq!(s.cards.len(), 1);
        assert_eq!(s.cards[0].tool_use_id, None);
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].contains("PreToolUse"));
    }

    // --- replay / idempotency (duplicate events don't corrupt state) ---

    #[test]
    fn duplicate_pretooluse_does_not_enqueue_twice() {
        let s = replay(&[pre(1, "toolu_a", "Bash"), pre(2, "toolu_a", "Bash")]);
        assert_eq!(s.cards.len(), 1, "same tool_use_id must not duplicate");
        assert_eq!(s.cards[0].seq, 1, "the first sighting keeps its id");
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn duplicate_posttooluse_is_a_silent_noop() {
        let s = replay(&[
            pre(1, "toolu_a", "Bash"),
            post(2, "toolu_a", "Bash"),
            post(3, "toolu_a", "Bash"), // replay of the same result
        ]);
        assert_eq!(card(&s, "toolu_a").status, CardStatus::Allowed);
        assert!(s.warnings.is_empty(), "an Allowed-replay is not an orphan");
    }

    #[test]
    fn repeated_stop_and_notification_are_noops() {
        let s = replay(&[
            pre(1, "toolu_a", "Bash"),
            note(2, NotificationKind::Permission),
            note(3, NotificationKind::Permission),
            stop(4),
            stop(5),
        ]);
        assert_eq!(card(&s, "toolu_a").status, CardStatus::ClosedNoRun);
        assert!(s.warnings.is_empty());
    }

    // --- per-event-type: status mapping (event -> SessionSignal) ---

    #[test]
    fn status_mapping_covers_every_event_type() {
        assert_eq!(
            hook_event_signal(&HookEventKind::PreToolUse {
                tool_use_id: Some("t".into()),
                tool_name: "Bash".into()
            }),
            Some(SessionSignal::Working)
        );
        assert_eq!(
            hook_event_signal(&HookEventKind::PostToolUse {
                tool_use_id: Some("t".into()),
                tool_name: "Bash".into()
            }),
            Some(SessionSignal::Working)
        );
        assert_eq!(
            hook_event_signal(&HookEventKind::Notification {
                notification: NotificationKind::Permission
            }),
            Some(SessionSignal::Attention)
        );
        assert_eq!(
            hook_event_signal(&HookEventKind::Notification {
                notification: NotificationKind::Idle
            }),
            Some(SessionSignal::Attention)
        );
        assert_eq!(
            hook_event_signal(&HookEventKind::Notification {
                notification: NotificationKind::Other
            }),
            None
        );
        assert_eq!(
            hook_event_signal(&HookEventKind::Stop),
            Some(SessionSignal::Finished)
        );
    }

    #[test]
    fn status_mapping_feeds_the_existing_rollup() {
        // The whole M3 status seam at the pure level: a PreToolUse rolls a
        // completed (idle) cut up to Working; a permission prompt parks it
        // in "Needs you"; Stop surfaces it as "To review".
        let working = hook_event_signal(&HookEventKind::PreToolUse {
            tool_use_id: Some("t".into()),
            tool_name: "Bash".into(),
        })
        .and_then(session_status_from_signal)
        .unwrap();
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[working]),
            WorkspaceStatus::Working
        );

        let blocked = hook_event_signal(&HookEventKind::Notification {
            notification: NotificationKind::Permission,
        })
        .and_then(session_status_from_signal)
        .unwrap();
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[blocked]),
            WorkspaceStatus::Blocked
        );

        let done = hook_event_signal(&HookEventKind::Stop)
            .and_then(session_status_from_signal)
            .unwrap();
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[done]),
            WorkspaceStatus::Done
        );
    }

    // --- synthetic-event suite, seeded from the spike corpus ---
    //
    // The exact 14 captured hook events from spike-approval-loop/events.jsonl
    // (a single multi-call session: a resolved call, a denied call, then two
    // parallel calls both allowed). Zero warnings across it is the
    // criterion-4 pass signal.

    fn spike_corpus() -> Vec<HookEvent> {
        vec![
            pre(1, "toolu_0147xSUu5zjYeq1oRrbYL8Bo", "Bash"),
            note(2, NotificationKind::Permission),
            post(3, "toolu_0147xSUu5zjYeq1oRrbYL8Bo", "Bash"),
            stop(4),
            pre(5, "toolu_01TMs4yARv9nEizBk5cac5RE", "Bash"),
            note(6, NotificationKind::Permission),
            stop(7),
            note(8, NotificationKind::Idle),
            pre(9, "toolu_01Dvw7DqDGE3pjV6KsWPFNiU", "Bash"),
            pre(10, "toolu_01GBXYZ17dmzAZ8pw66RhauK", "Bash"),
            note(11, NotificationKind::Permission),
            post(12, "toolu_01GBXYZ17dmzAZ8pw66RhauK", "Bash"),
            post(13, "toolu_01Dvw7DqDGE3pjV6KsWPFNiU", "Bash"),
            stop(14),
        ]
    }

    #[test]
    fn spike_corpus_replays_to_the_expected_cards_with_zero_warnings() {
        let s = replay(&spike_corpus());

        assert_eq!(s.event_count, 14);
        assert_eq!(s.cards.len(), 4);
        // Resolved call -> Allowed.
        assert_eq!(
            card(&s, "toolu_0147xSUu5zjYeq1oRrbYL8Bo").status,
            CardStatus::Allowed
        );
        // Denied call (Stop while pending/surfaced) -> ClosedNoRun.
        assert_eq!(
            card(&s, "toolu_01TMs4yARv9nEizBk5cac5RE").status,
            CardStatus::ClosedNoRun
        );
        // Both parallel calls -> Allowed, correlated by tool_use_id.
        assert_eq!(
            card(&s, "toolu_01Dvw7DqDGE3pjV6KsWPFNiU").status,
            CardStatus::Allowed
        );
        assert_eq!(
            card(&s, "toolu_01GBXYZ17dmzAZ8pw66RhauK").status,
            CardStatus::Allowed
        );

        assert!(
            s.warnings.is_empty(),
            "zero warnings is the criterion-4 pass signal, got: {:?}",
            s.warnings
        );
    }

    #[test]
    fn replaying_the_whole_corpus_twice_is_idempotent() {
        let once = replay(&spike_corpus());
        // Feed the identical stream again (fresh seqs would differ in a live
        // system, but dedup is by tool_use_id, so the cards are unchanged).
        let twice = spike_corpus()
            .into_iter()
            .fold(once.clone(), apply_hook_event);
        assert_eq!(twice.cards, once.cards, "duplicate events must not corrupt");
        assert!(twice.warnings.is_empty());
        assert_eq!(twice.event_count, 28);
    }

    // --- serialization shape (locks the M3.5 frontend contract) ---

    #[test]
    fn cards_serialize_camel_case_for_the_frontend_mirror() {
        let s = replay(&[pre(1, "toolu_a", "Bash")]);
        let json = serde_json::to_value(&s).unwrap();
        let c = &json["cards"][0];
        for key in ["id", "seq", "sessionId", "toolName", "toolUseId", "status"] {
            assert!(c.get(key).is_some(), "missing camelCase key {key}");
        }
        assert_eq!(c["status"], "pending");
        assert!(json.get("warnings").is_some());
        assert!(json.get("eventCount").is_some());
    }

    #[test]
    fn closed_no_run_status_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(CardStatus::ClosedNoRun).unwrap(),
            serde_json::json!("closedNoRun")
        );
    }
}
