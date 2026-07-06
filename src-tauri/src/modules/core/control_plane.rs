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
use super::policy::{decide, Decision, PolicyContext, ToolInput};

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
    /// a pending approval keyed by `tool_use_id`, carrying the parsed tool
    /// input so the pure [`policy`](super::policy) can decide it and the ask
    /// block can show the exact command.
    PreToolUse {
        tool_use_id: Option<String>,
        tool_name: String,
        input: ToolInput,
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

/// What the user-level [`policy`](super::policy) decided for a card's tool
/// call. Orthogonal to [`CardStatus`] (which tracks the correlation
/// lifecycle): `decision` is *why* the call did or did not pause, `status` is
/// *where it got to*. The frontend inbox renders only `Ask` cards as ask
/// blocks; `Allow`/`Deny` cards are the audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CardDecision {
    /// Permitted — ran freely (permissive in-worktree).
    Allow,
    /// A risk-list rule fired — the call paused for an approval.
    Ask,
    /// A hard-deny rule fired — the call was blocked and never ran.
    Deny,
}

/// The rule that fired, as the ask block / record shows it: a stable [`id`]
/// (kebab-case, for logs) plus a human [`label`].
///
/// [`id`]: super::policy::RiskRule::id
/// [`label`]: super::policy::RiskRule::label
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRule {
    pub id: String,
    pub label: String,
}

/// One Approval Inbox card: a single tool call that the policy decided on.
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
    /// What the policy decided (why it did or did not pause).
    pub decision: CardDecision,
    /// The rule that fired, if any (present on `Ask`/`Deny`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<CardRule>,
    /// The exact tool input the decision was made on — the ask block's
    /// "exact command". This is the PRE-hook input; a user-level hook (RTK)
    /// may rewrite the command afterwards, so a card may show pre-rewrite
    /// text — an accepted fidelity caveat that never affects correlation.
    pub input: ToolInput,
}

/// Who authored an approval record. The reducer stamps every per-call policy
/// decision [`Policy`](Self::Policy); a bulk banner action (task #19) stamps
/// its records [`Bulk`](Self::Bulk). This marker is exactly what keeps bulk
/// decisions "logged distinctly" from the per-call trail — same record shape,
/// different, always-serialized source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DecisionSource {
    /// The reducer decided a `PreToolUse` against the user-level policy.
    #[default]
    Policy,
    /// A bulk banner action (Allow all / Deny all) decided the whole pending
    /// queue at once.
    Bulk,
}

/// One approval record: every decision writes one, so the whole decision
/// history is auditable. Workspace scope is implicit — a [`ControlPlaneState`]
/// belongs to exactly one Workspace's endpoint. Per-call policy decisions and
/// bulk-banner decisions (task #19) share this shape but carry a distinct
/// [`source`](Self::source).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRecord {
    pub seq: u64,
    pub session_id: String,
    pub tool_name: String,
    pub input: ToolInput,
    pub decision: CardDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<CardRule>,
    /// Per-call `Policy` or a `Bulk` banner action — the distinct-logging
    /// marker.
    pub source: DecisionSource,
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
    /// Append-only audit trail: one [`ApprovalRecord`] per policy decision.
    pub records: Vec<ApprovalRecord>,
}

/// Map a pure [`Decision`] to the card/record shape (decision tag + the rule
/// that fired).
fn classify(decision: Decision) -> (CardDecision, Option<CardRule>) {
    match decision {
        Decision::Allow => (CardDecision::Allow, None),
        Decision::Ask(rule) => (
            CardDecision::Ask,
            Some(CardRule {
                id: rule.id().to_string(),
                label: rule.label().to_string(),
            }),
        ),
        Decision::Deny(rule) => (
            CardDecision::Deny,
            Some(CardRule {
                id: rule.id().to_string(),
                label: rule.label().to_string(),
            }),
        ),
    }
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
/// state, evaluating the user-level [`policy`](super::policy) for each new
/// `PreToolUse` against the trusted [`PolicyContext`]. Pure and total.
/// Replay-tolerant: a duplicated event (same `tool_use_id` re-sourced, a
/// repeated result, a second `Stop`) never corrupts state — it is
/// deduplicated or is a no-op, and a deduplicated `PreToolUse` writes no
/// second record.
pub fn apply_hook_event(
    prev: ControlPlaneState,
    event: HookEvent,
    policy: &PolicyContext,
) -> ControlPlaneState {
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
            input,
        } => {
            // Idempotent enqueue: a replayed PreToolUse for a tool_use_id we
            // already carry (in any state) must not source a second card or a
            // second record.
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
            // Every decision writes an approval record.
            let (decision, rule) = classify(decide(&tool_name, &input, policy));
            state.records.push(ApprovalRecord {
                seq: event.seq,
                session_id: sid.clone(),
                tool_name: tool_name.clone(),
                input: input.clone(),
                decision,
                rule: rule.clone(),
                source: DecisionSource::Policy,
            });
            state.cards.push(ApprovalCard {
                id: format!("card-{}", event.seq),
                seq: event.seq,
                session_id: sid,
                tool_name,
                tool_use_id,
                status: CardStatus::Pending,
                decision,
                rule,
                input,
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

/// The still-open `Ask` cards — the pending-approvals queue a bulk banner
/// action decides and a permission prompt surfaces. Only policy `Ask` cards
/// count (`Allow`/`Deny` are the audit trail, never asks); a resolved card
/// (`Allowed`/`ClosedNoRun`) has left the queue. Kept in incoming (seq) order.
/// Pure; this is the backend mirror of the frontend `deriveApprovalAsks` /
/// `derivePausedCalls` pending predicate, so a Workspace's on-card asks and its
/// slice of the bulk banner can never disagree on what is pending.
pub fn pending_ask_cards(state: &ControlPlaneState) -> Vec<&ApprovalCard> {
    state
        .cards
        .iter()
        .filter(|c| c.decision == CardDecision::Ask && c.status.is_open())
        .collect()
}

/// Build the DISTINCT bulk-decision log for a bulk banner action: one
/// [`ApprovalRecord`] per still-open ask, stamped [`DecisionSource::Bulk`] and
/// carrying the user's bulk [`decision`](CardDecision) (`Allow` for Allow-all,
/// `Deny` for Deny-all) over each card's tool + exact input + rule. Pure and
/// total — the imperative shell appends these on top of the per-call policy
/// trail, so "who decided this, and in bulk?" is always answerable from the
/// audit records alone. The keys themselves are injected via the runtime
/// `answer_prompt` seam; this only produces the distinct log entries.
pub fn bulk_decision_records(
    state: &ControlPlaneState,
    decision: CardDecision,
) -> Vec<ApprovalRecord> {
    pending_ask_cards(state)
        .into_iter()
        .map(|card| ApprovalRecord {
            seq: card.seq,
            session_id: card.session_id.clone(),
            tool_name: card.tool_name.clone(),
            input: card.input.clone(),
            decision,
            rule: card.rule.clone(),
            source: DecisionSource::Bulk,
        })
        .collect()
}

/// Apply a bulk banner decision to the state in place: append the distinct
/// bulk-decision records (see [`bulk_decision_records`]) for every still-open
/// ask, and return how many were logged. Pure over `state` (the only mutation
/// is the append); the card lifecycle itself still advances only through hook
/// events (`PostToolUse` → `Allowed`, `Stop` → `ClosedNoRun`) as the answered
/// calls resolve, so the queue drains through the same correlated path a
/// per-card answer would.
pub fn record_bulk_decision(state: &mut ControlPlaneState, decision: CardDecision) -> usize {
    let records = bulk_decision_records(state, decision);
    let n = records.len();
    state.records.extend(records);
    n
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

    /// A trusted context whose worktree root and home make the destructive-fs
    /// rule decidable in tests.
    fn ctx() -> PolicyContext {
        PolicyContext::new("/Users/dev/wt/feature", "/Users/dev")
    }

    /// A `PreToolUse` for an ordinary (policy-allowed) `ls` — the default the
    /// lifecycle tests use, so status/correlation are exercised without a
    /// risk rule firing.
    fn pre(seq: u64, tuid: &str, tool: &str) -> HookEvent {
        pre_cmd(seq, tuid, tool, "ls")
    }

    /// A `PreToolUse` for a specific shell command, so a test can drive a
    /// risk-list or hard-deny decision.
    fn pre_cmd(seq: u64, tuid: &str, tool: &str, command: &str) -> HookEvent {
        HookEvent::new(
            seq,
            SESSION,
            HookEventKind::PreToolUse {
                tool_use_id: Some(tuid.to_string()),
                tool_name: tool.to_string(),
                input: ToolInput::command(command),
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
        let ctx = ctx();
        events
            .iter()
            .cloned()
            .fold(empty_state(), |state, event| {
                apply_hook_event(state, event, &ctx)
            })
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
                input: ToolInput::command("ls"),
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
                tool_name: "Bash".into(),
                input: ToolInput::command("ls"),
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
            input: ToolInput::command("ls"),
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
        let ctx = ctx();
        let twice = spike_corpus()
            .into_iter()
            .fold(once.clone(), |state, event| {
                apply_hook_event(state, event, &ctx)
            });
        assert_eq!(twice.cards, once.cards, "duplicate events must not corrupt");
        assert_eq!(
            twice.records, once.records,
            "replayed PreToolUse writes no second record"
        );
        assert!(twice.warnings.is_empty());
        assert_eq!(twice.event_count, 28);
    }

    // --- M3.5 policy: decisions on cards + records, correlation of asks ---

    #[test]
    fn a_risk_list_call_becomes_an_ask_card_with_the_rule_and_exact_command() {
        let s = replay(&[pre_cmd(1, "toolu_a", "Bash", "git push --force origin main")]);
        let c = card(&s, "toolu_a");
        assert_eq!(c.decision, CardDecision::Ask);
        assert_eq!(c.tool_name, "Bash");
        assert_eq!(
            c.rule.as_ref().map(|r| r.id.as_str()),
            Some("git-history-rewrite")
        );
        // The ask block shows the exact (pre-rewrite) command.
        assert_eq!(c.input.command.as_deref(), Some("git push --force origin main"));
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn a_hard_deny_call_is_recorded_as_deny_and_never_ran() {
        // Hard-deny returns deny at the hook; the tool never runs, so no
        // PostToolUse arrives and Stop closes the card unrun.
        let s = replay(&[
            pre_cmd(1, "toolu_a", "Bash", "sudo rm -rf /var"),
            note(2, NotificationKind::Permission),
            stop(3),
        ]);
        let c = card(&s, "toolu_a");
        assert_eq!(c.decision, CardDecision::Deny);
        assert_eq!(c.rule.as_ref().map(|r| r.id.as_str()), Some("hard-deny-sudo"));
        assert_eq!(c.status, CardStatus::ClosedNoRun, "a hard-denied call never runs");
    }

    #[test]
    fn every_decision_writes_exactly_one_record() {
        let s = replay(&[
            pre_cmd(1, "toolu_a", "Bash", "ls"),                 // allow
            pre_cmd(2, "toolu_b", "Bash", "git reset --hard"),  // ask
            pre_cmd(3, "toolu_c", "Bash", "sudo id"),           // deny
        ]);
        assert_eq!(s.records.len(), 3, "one record per PreToolUse decision");
        let decisions: Vec<CardDecision> = s.records.iter().map(|r| r.decision).collect();
        assert_eq!(
            decisions,
            vec![CardDecision::Allow, CardDecision::Ask, CardDecision::Deny]
        );
        // The record carries the exact input + rule fired.
        assert_eq!(s.records[1].input.command.as_deref(), Some("git reset --hard"));
        assert_eq!(
            s.records[2].rule.as_ref().map(|r| r.id.as_str()),
            Some("hard-deny-sudo")
        );
    }

    #[test]
    fn parallel_ask_cards_correlate_by_tool_use_id_not_command_string() {
        // Two parallel risk calls, then their results arrive OUT OF ORDER and
        // with RTK-REWRITTEN commands (post-hook the command string differs);
        // correlation is strictly by tool_use_id, so both cards resolve to
        // Allowed and nothing is misrouted. This is the spike's criterion-4
        // (parallel + rewrite) applied to ask cards.
        let s = replay(&[
            pre_cmd(1, "toolu_p1", "Bash", "git push --force origin main"),
            pre_cmd(2, "toolu_p2", "Bash", "git rebase -i HEAD~2"),
            note(3, NotificationKind::Permission),
            // Results out of order; the command a PostToolUse carries is
            // irrelevant to correlation (and, per PostToolUse, unused).
            post(4, "toolu_p2", "Bash"),
            post(5, "toolu_p1", "Bash"),
        ]);
        assert_eq!(card(&s, "toolu_p1").decision, CardDecision::Ask);
        assert_eq!(card(&s, "toolu_p2").decision, CardDecision::Ask);
        assert_eq!(card(&s, "toolu_p1").status, CardStatus::Allowed);
        assert_eq!(card(&s, "toolu_p2").status, CardStatus::Allowed);
        assert!(
            s.warnings.is_empty(),
            "strict tool_use_id correlation leaves no ambiguity: {:?}",
            s.warnings
        );
    }

    #[test]
    fn allow_cards_carry_an_allow_decision_and_no_rule() {
        let s = replay(&[pre(1, "toolu_a", "Bash")]);
        let c = card(&s, "toolu_a");
        assert_eq!(c.decision, CardDecision::Allow);
        assert!(c.rule.is_none());
    }

    // --- serialization shape (locks the M3.5 frontend contract) ---

    #[test]
    fn cards_serialize_camel_case_for_the_frontend_mirror() {
        let s = replay(&[pre_cmd(1, "toolu_a", "Bash", "git push --force")]);
        let json = serde_json::to_value(&s).unwrap();
        let c = &json["cards"][0];
        for key in [
            "id", "seq", "sessionId", "toolName", "toolUseId", "status", "decision", "input",
        ] {
            assert!(c.get(key).is_some(), "missing camelCase key {key}");
        }
        assert_eq!(c["status"], "pending");
        assert_eq!(c["decision"], "ask");
        // The ask block's exact-command + rule (tool, rule, command) all ride
        // the serialized card the frontend mirrors.
        assert_eq!(c["input"]["command"], "git push --force");
        assert_eq!(c["rule"]["id"], "git-history-rewrite");
        assert_eq!(c["rule"]["label"], "git history rewrite");
        assert!(json.get("warnings").is_some());
        assert!(json.get("eventCount").is_some());
        // The audit trail serializes too.
        assert_eq!(json["records"][0]["decision"], "ask");
        assert_eq!(json["records"][0]["input"]["command"], "git push --force");
    }

    #[test]
    fn closed_no_run_status_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(CardStatus::ClosedNoRun).unwrap(),
            serde_json::json!("closedNoRun")
        );
    }

    // --- M3.5 bulk banner: the pending queue + distinct bulk logging ---

    #[test]
    fn pending_ask_cards_are_only_the_open_asks() {
        // An open ask, an allowed call (audit, not an ask), an ask that already
        // resolved, and a hard-deny (never an ask). Only the open ask queues.
        let s = replay(&[
            pre_cmd(1, "toolu_open", "Bash", "git push --force origin main"),
            pre_cmd(2, "toolu_allow", "Bash", "ls"), // allow decision — audit only
            pre_cmd(3, "toolu_done", "Bash", "git rebase -i HEAD~2"),
            post(4, "toolu_done", "Bash"), // the ask ran → left the queue
            pre_cmd(5, "toolu_deny", "Bash", "sudo rm -rf /var"), // hard-deny
        ]);
        let queue = pending_ask_cards(&s);
        assert_eq!(queue.len(), 1, "only the one still-open ask is pending");
        assert_eq!(queue[0].tool_use_id.as_deref(), Some("toolu_open"));
    }

    #[test]
    fn bulk_decision_records_are_stamped_bulk_and_distinct_from_policy() {
        // Two open asks in one session; Allow-all logs one BULK record per ask,
        // carrying the user's decision + each card's tool/input/rule, while the
        // original per-call records stay POLICY. That marker is the whole
        // "logged distinctly" contract.
        let s = replay(&[
            pre_cmd(1, "toolu_a", "Bash", "git push --force origin main"),
            pre_cmd(2, "toolu_b", "Bash", "git rebase -i HEAD~3"),
        ]);
        // Per-call trail: two policy Ask records.
        assert_eq!(s.records.len(), 2);
        assert!(s.records.iter().all(|r| r.source == DecisionSource::Policy));

        let allow = bulk_decision_records(&s, CardDecision::Allow);
        assert_eq!(allow.len(), 2, "one bulk record per pending ask");
        assert!(allow.iter().all(|r| r.source == DecisionSource::Bulk));
        assert!(allow.iter().all(|r| r.decision == CardDecision::Allow));
        // The bulk record carries the exact command + rule of the card it
        // decided (so the audit trail names what was bulk-allowed).
        assert_eq!(allow[0].input.command.as_deref(), Some("git push --force origin main"));
        assert_eq!(
            allow[0].rule.as_ref().map(|r| r.id.as_str()),
            Some("git-history-rewrite")
        );

        // Deny-all logs the same cards with a Deny decision, still bulk-sourced.
        let deny = bulk_decision_records(&s, CardDecision::Deny);
        assert!(deny.iter().all(|r| r.source == DecisionSource::Bulk
            && r.decision == CardDecision::Deny));
    }

    #[test]
    fn record_bulk_decision_appends_distinctly_and_leaves_policy_records() {
        let mut s = replay(&[
            pre_cmd(1, "toolu_a", "Bash", "git push --force origin main"),
            pre_cmd(2, "toolu_b", "Bash", "git rebase -i HEAD~3"),
        ]);
        let before = s.records.len();
        let n = record_bulk_decision(&mut s, CardDecision::Allow);
        assert_eq!(n, 2);
        assert_eq!(s.records.len(), before + 2);
        // The trail now holds both kinds, distinguishable by source.
        let policy = s.records.iter().filter(|r| r.source == DecisionSource::Policy).count();
        let bulk = s.records.iter().filter(|r| r.source == DecisionSource::Bulk).count();
        assert_eq!((policy, bulk), (2, 2));
    }

    #[test]
    fn bulk_records_serialize_their_source_for_the_frontend_mirror() {
        let mut s = replay(&[pre_cmd(1, "toolu_a", "Bash", "git push --force")]);
        record_bulk_decision(&mut s, CardDecision::Deny);
        let json = serde_json::to_value(&s).unwrap();
        // The per-call record is `policy`; the appended bulk one is `bulk`.
        assert_eq!(json["records"][0]["source"], "policy");
        assert_eq!(json["records"][1]["source"], "bulk");
        assert_eq!(json["records"][1]["decision"], "deny");
    }

    // ═══════════════════════════════════════════════════════════════════
    // M3.5 scripted demo — the milestone "Done when", backend seam.
    //
    //   Two agents Blocked in DIFFERENT Projects, both visible in ONE queue;
    //   one ALLOWED and resumed exactly where it paused, one DENIED and
    //   rerouted.
    //
    // This is the deterministic backend half of the M3.5 demo. The frontend
    // half — the unified banner queue (`deriveBulkApprovals`), the banner
    // >1-pending rule, the bulk answer plan, and the per-card answer seam
    // calls — lives in `src/modules/helm/m3_5Demo.test.ts`. The one fragile
    // live link, `answer_prompt` against a real `claude` PTY, is the
    // `#[ignore]`d `live_claude_answer_prompt_seam` in `runtime::answer` (never
    // spawned inside `cargo test`); everything here is synthetic hook events
    // through the pure reducer + the pure bulk-log derivations.
    //
    // COVERED here (automated): two Projects' endpoints each reduce a risk
    // `ask` into their own queue; the union is ONE queue of two; one call is
    // allowed and resolves to `Allowed` correlated strictly by `tool_use_id`
    // (resumed exactly where it paused, not by command string); the other is
    // denied and its tool never runs (`ClosedNoRun`); and each decision is
    // logged distinctly (per-call `Policy` + bulk `Bulk`).
    // NOT covered (human/verify at the running Tauri app): the literal `A`/`X`
    // key presses, the two-press confirm animation, and the banner render.
    #[test]
    fn m3_5_demo_two_projects_one_queue_allow_one_deny_one() {
        let ctx = ctx();
        // Session ids stand in for two agents in two DIFFERENT Projects; each
        // Project runs its own control-plane endpoint (its own state).
        let sess_a = "agent-alpha";
        let sess_b = "agent-beta";
        let fold = |events: &[HookEvent]| -> ControlPlaneState {
            events
                .iter()
                .cloned()
                .fold(empty_state(), |st, ev| apply_hook_event(st, ev, &ctx))
        };
        let ev = |seq: u64, sid: &str, kind: HookEventKind| HookEvent::new(seq, sid, kind);
        let ask = |seq: u64, sid: &str, tuid: &str, cmd: &str| {
            ev(
                seq,
                sid,
                HookEventKind::PreToolUse {
                    tool_use_id: Some(tuid.to_string()),
                    tool_name: "Bash".to_string(),
                    input: ToolInput::command(cmd),
                },
            )
        };
        let permission =
            |seq: u64, sid: &str| ev(seq, sid, HookEventKind::Notification {
                notification: NotificationKind::Permission,
            });

        // Both agents pause on a risk-list call and raise a permission prompt:
        // agent A wants a force-push, agent B an interactive rebase.
        let state_a = fold(&[
            ask(1, sess_a, "toolu_a", "git push --force origin main"),
            permission(2, sess_a),
        ]);
        let state_b = fold(&[
            ask(1, sess_b, "toolu_b", "git rebase -i HEAD~3"),
            permission(2, sess_b),
        ]);

        // ONE queue across the two Projects: the banner unions each endpoint's
        // pending asks. Two agents, two Projects, one list.
        let queue: Vec<&ApprovalCard> = pending_ask_cards(&state_a)
            .into_iter()
            .chain(pending_ask_cards(&state_b))
            .collect();
        assert_eq!(queue.len(), 2, "two agents in two Projects, one queue");
        assert_eq!(queue[0].session_id, sess_a);
        assert_eq!(queue[1].session_id, sess_b);
        // Both are surfaced (blocked on a permission prompt).
        assert!(queue.iter().all(|c| c.status == CardStatus::Surfaced));

        // --- one ALLOWED and resumed exactly where it paused ---
        // The Allow keys inject via the runtime seam (live-tested, #[ignore]d);
        // the tool then runs and its PostToolUse resolves the card by
        // tool_use_id — never by the (possibly RTK-rewritten) command string.
        let state_a = apply_hook_event(
            state_a,
            ev(
                3,
                sess_a,
                HookEventKind::PostToolUse {
                    tool_use_id: Some("toolu_a".to_string()),
                    tool_name: "Bash".to_string(),
                },
            ),
            &ctx,
        );
        assert_eq!(
            card(&state_a, "toolu_a").status,
            CardStatus::Allowed,
            "agent A's paused call resumed and ran, correlated by tool_use_id"
        );

        // --- one DENIED and rerouted ---
        // Deny + reroute lands the instruction as a user message and the tool
        // verifiably never runs; the turn ends with the ask still open, so Stop
        // closes it ClosedNoRun (denied, never ran).
        let state_b = apply_hook_event(state_b, ev(3, sess_b, HookEventKind::Stop), &ctx);
        assert_eq!(
            card(&state_b, "toolu_b").status,
            CardStatus::ClosedNoRun,
            "agent B's call was denied and never ran"
        );

        // The queue is now drained: neither agent is still pending.
        assert!(pending_ask_cards(&state_a).is_empty());
        assert!(pending_ask_cards(&state_b).is_empty());

        // Both endpoints kept a clean per-call policy trail throughout.
        assert!(state_a.records.iter().all(|r| r.source == DecisionSource::Policy));
        assert!(state_b.records.iter().all(|r| r.source == DecisionSource::Policy));
        assert!(state_a.warnings.is_empty() && state_b.warnings.is_empty());
    }
}
