//! Cut-pipeline lifecycle as pure data (task #8).
//!
//! The ambient cut pipeline lives in the imperative shell
//! (`modules::registry::pipeline`); everything it *decides* is here: the
//! ordered step names, the Workspace's cut lifecycle, log truncation, the
//! opening-prompt composition, and the derived Workspace status. Events
//! may change this state; nothing here executes anything.

use serde::{Deserialize, Serialize};

use super::state::CoreError;
use super::workspace::Workspace;

/// One effectful step of the cut pipeline, in PRD order. Slot allocation
/// and `HELMSMEN_*` env assembly are pure data settled at enqueue time
/// (their failures reject the cut synchronously, before a Workspace
/// exists); every variant here runs ambient and parks the Workspace in
/// Needs you when it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CutStep {
    /// `git fetch` in the main checkout (optional, per cut).
    Fetch,
    /// `git worktree add` off base with the branch template applied.
    WorktreeAdd,
    /// Canonicalize + authorize the worktree as a Terax workspace root.
    AuthorizeRoot,
    /// Copy the Project's carry-over globs from the main checkout.
    CopyCarryOvers,
    /// Run the Project's setup script (user's shell, cwd = worktree).
    SetupScript,
    /// Write the Harness's config injection into the worktree. Stub at
    /// M2 — M3 writes control-plane hook wiring through this same step.
    HarnessWiring,
    /// Launch the first Agent Session (Harness launch command, Profile
    /// model, opening prompt composed from the snippet + Brief).
    LaunchSession,
}

impl CutStep {
    /// Human-readable step name for logs and the parked card.
    pub fn label(self) -> &'static str {
        match self {
            CutStep::Fetch => "fetch",
            CutStep::WorktreeAdd => "worktree add",
            CutStep::AuthorizeRoot => "authorize workspace root",
            CutStep::CopyCarryOvers => "copy carry-overs",
            CutStep::SetupScript => "setup script",
            CutStep::HarnessWiring => "harness wiring",
            CutStep::LaunchSession => "launch first session",
        }
    }
}

/// Where a Workspace is in its cut lifecycle. Stored on the Workspace
/// (the *facts*: which step failed, its log); the user-facing status is
/// derived from it by [`derive_status`], never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum CutState {
    /// The ambient pipeline is still running its steps.
    Cutting,
    /// Every step finished. `first_session_id` records the first Agent
    /// Session's runtime id; it does not survive what the Runtime does
    /// not survive (empty = unknown: pre-#8 registry files, app restart
    /// on LocalPty, or the synchronous M1 cut that launches nothing).
    #[serde(rename_all = "camelCase")]
    Complete {
        #[serde(default)]
        first_session_id: String,
    },
    /// A step failed: the Workspace is parked as Blocked ("Needs you")
    /// with the failing step and its log attached.
    #[serde(rename_all = "camelCase")]
    Failed { step: CutStep, log: String },
}

impl Default for CutState {
    /// Pre-#8 registry files have no `cut` key; every Workspace they
    /// hold was cut synchronously and completely (task #5).
    fn default() -> Self {
        CutState::Complete {
            first_session_id: String::new(),
        }
    }
}

/// Upper bound for a stored step log. Failure logs are hostile process
/// output; the registry keeps only the tail (the end of a failing log is
/// the part that explains the failure).
pub const MAX_CUT_LOG_BYTES: usize = 64 * 1024;

/// Keep at most [`MAX_CUT_LOG_BYTES`] of the *end* of a log,
/// char-boundary safe. Applied inside `apply` so no event can bloat the
/// registry.
pub fn truncate_log(log: &str) -> String {
    if log.len() <= MAX_CUT_LOG_BYTES {
        return log.to_string();
    }
    let mut start = log.len() - MAX_CUT_LOG_BYTES;
    while !log.is_char_boundary(start) {
        start += 1;
    }
    format!("[log truncated]\n{}", &log[start..])
}

/// Compose the first Session's opening prompt from the Profile's prompt
/// snippet and the Brief: every `{brief}` placeholder is substituted; a
/// snippet without the placeholder is joined with the Brief by a space
/// (PRD: opening prompt = Profile snippet + Brief, e.g. `/tdd <brief>`).
pub fn compose_opening_prompt(snippet: &str, brief: &str) -> String {
    let composed = if snippet.contains("{brief}") {
        snippet.replace("{brief}", brief)
    } else {
        [snippet, brief]
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    composed.trim().to_string()
}

/// Validate a Brief as pure data: free multiline text, but never a NUL
/// byte (it becomes an argv element of the launch command).
pub fn validate_brief(brief: &str) -> Result<(), CoreError> {
    if brief.contains('\0') {
        return Err(CoreError::Invalid {
            field: "brief",
            reason: "must not contain a NUL byte".to_string(),
        });
    }
    Ok(())
}

/// Derived Workspace status — the wall's rank order. Serialize-only on
/// purpose: a status is derived by [`derive_status`], never stored and
/// never accepted from data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceStatus {
    /// Waiting on the user ("Needs you"): a failed cut, later also
    /// blocked Sessions (M3).
    Blocked,
    Working,
    /// Finished, unreviewed ("To review"). Session-driven — arrives with
    /// the control plane (M3); no cut state maps here.
    Done,
    Idle,
}

impl WorkspaceStatus {
    /// Display alias per the PRD: Blocked = "Needs you", Done = "To
    /// review".
    pub fn display_alias(self) -> &'static str {
        match self {
            WorkspaceStatus::Blocked => "Needs you",
            WorkspaceStatus::Working => "Working",
            WorkspaceStatus::Done => "To review",
            WorkspaceStatus::Idle => "Idle",
        }
    }

    /// The Helm wall's rank order: Blocked 0 -> Done 1 -> Working 2 ->
    /// Idle 3 — the canonical attention order, applied as a sort across
    /// all Projects (not sections). Lower ranks float to the top.
    pub fn rank(self) -> u8 {
        match self {
            WorkspaceStatus::Blocked => 0,
            WorkspaceStatus::Done => 1,
            WorkspaceStatus::Working => 2,
            WorkspaceStatus::Idle => 3,
        }
    }
}

/// Roll a Workspace's status up from its Sessions, per the PRD rule: any
/// Session blocked -> Blocked; else any working -> Working; else all done
/// -> Done; else Idle. The status is derived, never stored.
///
/// `cut_status` is the [`derive_status`] result: a failed cut parks the
/// Workspace as Blocked ("Needs you") regardless of its Sessions, and
/// with no Sessions the cut-derived status stands (M2: the cut is the
/// only status source until the control plane feeds Session status at
/// M3). This is the pure-core seam the frontend view-model mirrors.
pub fn roll_up_status(
    cut_status: WorkspaceStatus,
    sessions: &[WorkspaceStatus],
) -> WorkspaceStatus {
    if cut_status == WorkspaceStatus::Blocked {
        return WorkspaceStatus::Blocked;
    }
    if sessions.is_empty() {
        return cut_status;
    }
    if sessions.contains(&WorkspaceStatus::Blocked) {
        WorkspaceStatus::Blocked
    } else if sessions.contains(&WorkspaceStatus::Working) {
        WorkspaceStatus::Working
    } else if sessions.iter().all(|s| *s == WorkspaceStatus::Done) {
        WorkspaceStatus::Done
    } else {
        WorkspaceStatus::Idle
    }
}

/// Derive a Workspace's status. At M2 only the cut lifecycle feeds it: a
/// failed cut parks the Workspace as Blocked, a running pipeline shows
/// Working, a completed cut is Idle until Session-driven status (M3)
/// layers on top.
pub fn derive_status(workspace: &Workspace) -> WorkspaceStatus {
    match &workspace.cut {
        CutState::Failed { .. } => WorkspaceStatus::Blocked,
        CutState::Cutting => WorkspaceStatus::Working,
        CutState::Complete { .. } => WorkspaceStatus::Idle,
    }
}

/// One Agent Session lifecycle signal, as pure-core data. Serialize-only,
/// like [`WorkspaceStatus`]: a signal is *observed*, never accepted from
/// stored data.
///
/// The M2 interim SOURCE is Terax's in-tree OSC agent-signal —
/// `modules::pty::agent_detect` parses hostile PTY bytes into a small set
/// of signal kinds, and `modules::harness::agent_signal` maps one kind to
/// this event. Nothing about a `SessionSignal` executes anything: it is
/// data that a reducer folds into a derived status.
///
/// # signal -> event -> status seam (the M3 swap point)
///
/// - **SOURCE** (`harness::agent_signal::ingest_agent_signal`, M2): OSC
///   agent-signal, best-effort, whole-terminal.
/// - **EVENT** (this `SessionSignal`): pure data; no side effect ever
///   follows from its content.
/// - **REDUCER** ([`session_status_from_signal`] then [`roll_up_status`]):
///   folds a Session's signal into its status, then into the Workspace's.
///
/// At M3 the control plane's per-Workspace hooks replace the SOURCE — they
/// emit this SAME `SessionSignal` per Session — and the reducer here is
/// untouched. `agent_signal` then stays the Signal-only fallback for
/// Harnesses without the control-plane-hooks Cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionSignal {
    /// The Session's agent command was recognized and started running.
    Started,
    /// The agent is actively working.
    Working,
    /// The agent is waiting on the user ("Needs you").
    Attention,
    /// The agent finished its turn — output is ready to review.
    Finished,
    /// The Session's process exited; it no longer contributes a status.
    Exited,
}

/// Map one [`SessionSignal`] to the [`WorkspaceStatus`] it implies for that
/// single Session, per the PRD dot vocabulary. `Exited` yields `None`: an
/// exited Session drops out of the rollup rather than pinning a stale dot,
/// so with no other live Sessions the cut-derived status stands again.
///
/// Pure and total. This is the reducer M3 keeps; only the SOURCE feeding it
/// changes (agent-signal now, per-Workspace control-plane hooks at M3).
pub fn session_status_from_signal(signal: SessionSignal) -> Option<WorkspaceStatus> {
    match signal {
        SessionSignal::Started | SessionSignal::Working => Some(WorkspaceStatus::Working),
        SessionSignal::Attention => Some(WorkspaceStatus::Blocked),
        SessionSignal::Finished => Some(WorkspaceStatus::Done),
        SessionSignal::Exited => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- opening prompt composition ---

    #[test]
    fn brief_substitutes_into_the_placeholder() {
        assert_eq!(
            compose_opening_prompt("/tdd {brief}", "add dark mode"),
            "/tdd add dark mode"
        );
    }

    #[test]
    fn every_placeholder_occurrence_is_substituted() {
        assert_eq!(
            compose_opening_prompt("{brief} — plan first, then {brief}", "fix login"),
            "fix login — plan first, then fix login"
        );
    }

    #[test]
    fn snippet_without_placeholder_is_joined_with_the_brief() {
        assert_eq!(
            compose_opening_prompt("Research only:", "why is CI flaky"),
            "Research only: why is CI flaky"
        );
    }

    #[test]
    fn empty_parts_do_not_leave_stray_whitespace() {
        assert_eq!(compose_opening_prompt("/tdd {brief}", ""), "/tdd");
        assert_eq!(compose_opening_prompt("", "just the brief"), "just the brief");
        assert_eq!(compose_opening_prompt("", ""), "");
        assert_eq!(compose_opening_prompt("{brief}", "  "), "");
    }

    #[test]
    fn multiline_briefs_survive_composition() {
        let brief = "fix login\n\nsteps:\n1. reproduce";
        assert_eq!(
            compose_opening_prompt("/tdd {brief}", brief),
            format!("/tdd {brief}")
        );
    }

    // --- brief validation ---

    #[test]
    fn briefs_reject_nul_and_accept_multiline_text() {
        assert!(validate_brief("fix the login page\nwith tests").is_ok());
        assert!(validate_brief("").is_ok());
        assert!(matches!(
            validate_brief("a\0b"),
            Err(CoreError::Invalid { field: "brief", .. })
        ));
    }

    // --- log truncation ---

    #[test]
    fn short_logs_pass_through_unchanged() {
        assert_eq!(truncate_log("pnpm ERR! boom"), "pnpm ERR! boom");
    }

    #[test]
    fn long_logs_keep_the_tail_and_are_marked_truncated() {
        let log = format!("{}THE-END", "x".repeat(MAX_CUT_LOG_BYTES * 2));
        let out = truncate_log(&log);
        assert!(out.starts_with("[log truncated]\n"));
        assert!(out.ends_with("THE-END"), "the tail must survive");
        assert!(out.len() <= MAX_CUT_LOG_BYTES + "[log truncated]\n".len());
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let log = "é".repeat(MAX_CUT_LOG_BYTES); // 2 bytes per char
        let out = truncate_log(&log);
        assert!(out.chars().skip(1).all(|c| c == 'é' || c.is_ascii()));
    }

    // --- cut state serialization (locks the registry JSON contract) ---

    #[test]
    fn cut_states_serialize_tagged_camel_case() {
        assert_eq!(
            serde_json::to_value(CutState::Cutting).unwrap(),
            serde_json::json!({ "phase": "cutting" })
        );
        assert_eq!(
            serde_json::to_value(CutState::Complete {
                first_session_id: "rt-1".to_string()
            })
            .unwrap(),
            serde_json::json!({ "phase": "complete", "firstSessionId": "rt-1" })
        );
        assert_eq!(
            serde_json::to_value(CutState::Failed {
                step: CutStep::SetupScript,
                log: "exit 7".to_string()
            })
            .unwrap(),
            serde_json::json!({ "phase": "failed", "step": "setupScript", "log": "exit 7" })
        );
    }

    #[test]
    fn cut_states_round_trip_and_default_is_complete() {
        for state in [
            CutState::Cutting,
            CutState::Complete {
                first_session_id: "rt-9".to_string(),
            },
            CutState::Failed {
                step: CutStep::Fetch,
                log: "no remote".to_string(),
            },
        ] {
            let back: CutState =
                serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            assert_eq!(back, state);
        }
        assert_eq!(
            CutState::default(),
            CutState::Complete {
                first_session_id: String::new()
            }
        );
    }

    #[test]
    fn every_step_has_a_label_and_a_stable_wire_name() {
        let steps = [
            (CutStep::Fetch, "fetch"),
            (CutStep::WorktreeAdd, "worktreeAdd"),
            (CutStep::AuthorizeRoot, "authorizeRoot"),
            (CutStep::CopyCarryOvers, "copyCarryOvers"),
            (CutStep::SetupScript, "setupScript"),
            (CutStep::HarnessWiring, "harnessWiring"),
            (CutStep::LaunchSession, "launchSession"),
        ];
        for (step, wire) in steps {
            assert!(!step.label().is_empty());
            assert_eq!(
                serde_json::to_value(step).unwrap(),
                serde_json::Value::String(wire.to_string())
            );
        }
    }

    // --- status derivation (never stored) ---

    fn workspace(cut: CutState) -> Workspace {
        Workspace {
            id: "ws-1".to_string(),
            project_id: "prj-1".to_string(),
            slug: "fix".to_string(),
            branch: "helm/fix".to_string(),
            worktree_path: "/home/dev/wt/fix-1".to_string(),
            slot: 1,
            cut,
        }
    }

    #[test]
    fn failed_cut_derives_blocked_alias_needs_you() {
        let status = derive_status(&workspace(CutState::Failed {
            step: CutStep::SetupScript,
            log: "boom".to_string(),
        }));
        assert_eq!(status, WorkspaceStatus::Blocked);
        assert_eq!(status.display_alias(), "Needs you");
    }

    #[test]
    fn cutting_derives_working_and_complete_derives_idle() {
        assert_eq!(
            derive_status(&workspace(CutState::Cutting)),
            WorkspaceStatus::Working
        );
        assert_eq!(
            derive_status(&workspace(CutState::default())),
            WorkspaceStatus::Idle
        );
    }

    #[test]
    fn statuses_serialize_camel_case_and_done_aliases_to_review() {
        assert_eq!(
            serde_json::to_value(WorkspaceStatus::Blocked).unwrap(),
            serde_json::json!("blocked")
        );
        assert_eq!(WorkspaceStatus::Done.display_alias(), "To review");
    }

    // --- wall rank order (never stored; a sort, not sections) ---

    #[test]
    fn rank_orders_needs_you_then_to_review_then_working_then_idle() {
        assert_eq!(WorkspaceStatus::Blocked.rank(), 0);
        assert_eq!(WorkspaceStatus::Done.rank(), 1);
        assert_eq!(WorkspaceStatus::Working.rank(), 2);
        assert_eq!(WorkspaceStatus::Idle.rank(), 3);

        let mut statuses = [
            WorkspaceStatus::Idle,
            WorkspaceStatus::Working,
            WorkspaceStatus::Blocked,
            WorkspaceStatus::Done,
        ];
        statuses.sort_by_key(|s| s.rank());
        assert_eq!(
            statuses,
            [
                WorkspaceStatus::Blocked,
                WorkspaceStatus::Done,
                WorkspaceStatus::Working,
                WorkspaceStatus::Idle,
            ]
        );
    }

    // --- Session rollup (the PRD rule, derived not stored) ---

    #[test]
    fn a_failed_cut_parks_blocked_regardless_of_sessions() {
        assert_eq!(
            roll_up_status(
                WorkspaceStatus::Blocked,
                &[WorkspaceStatus::Working, WorkspaceStatus::Done]
            ),
            WorkspaceStatus::Blocked
        );
    }

    #[test]
    fn with_no_sessions_the_cut_status_stands() {
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[]),
            WorkspaceStatus::Idle
        );
        assert_eq!(
            roll_up_status(WorkspaceStatus::Working, &[]),
            WorkspaceStatus::Working
        );
    }

    #[test]
    fn rollup_follows_blocked_then_working_then_done_then_idle() {
        // any blocked wins
        assert_eq!(
            roll_up_status(
                WorkspaceStatus::Idle,
                &[
                    WorkspaceStatus::Working,
                    WorkspaceStatus::Blocked,
                    WorkspaceStatus::Done
                ]
            ),
            WorkspaceStatus::Blocked
        );
        // else any working wins
        assert_eq!(
            roll_up_status(
                WorkspaceStatus::Idle,
                &[WorkspaceStatus::Idle, WorkspaceStatus::Working]
            ),
            WorkspaceStatus::Working
        );
        // else all done -> done
        assert_eq!(
            roll_up_status(
                WorkspaceStatus::Idle,
                &[WorkspaceStatus::Done, WorkspaceStatus::Done]
            ),
            WorkspaceStatus::Done
        );
        // else (some idle) -> idle
        assert_eq!(
            roll_up_status(
                WorkspaceStatus::Done,
                &[WorkspaceStatus::Done, WorkspaceStatus::Idle]
            ),
            WorkspaceStatus::Idle
        );
    }

    // --- session-signal -> status mapping (M2 interim source, task #11) ---

    #[test]
    fn session_signals_map_to_the_prd_dot_vocabulary() {
        assert_eq!(
            session_status_from_signal(SessionSignal::Started),
            Some(WorkspaceStatus::Working)
        );
        assert_eq!(
            session_status_from_signal(SessionSignal::Working),
            Some(WorkspaceStatus::Working)
        );
        assert_eq!(
            session_status_from_signal(SessionSignal::Attention),
            Some(WorkspaceStatus::Blocked)
        );
        assert_eq!(
            session_status_from_signal(SessionSignal::Finished),
            Some(WorkspaceStatus::Done)
        );
        assert_eq!(session_status_from_signal(SessionSignal::Exited), None);
    }

    #[test]
    fn a_live_session_signal_lights_the_dot_through_the_existing_rollup() {
        // The whole M2 seam at the pure level: a "working" signal on a
        // completed (idle) cut rolls the Workspace up to Working.
        let working = session_status_from_signal(SessionSignal::Working).unwrap();
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[working]),
            WorkspaceStatus::Working
        );
        // "attention" parks it in Needs you regardless of the cut status.
        let attention = session_status_from_signal(SessionSignal::Attention).unwrap();
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[attention]),
            WorkspaceStatus::Blocked
        );
        // "finished" surfaces it as To review.
        let finished = session_status_from_signal(SessionSignal::Finished).unwrap();
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[finished]),
            WorkspaceStatus::Done
        );
    }

    #[test]
    fn an_exited_session_contributes_no_status_and_falls_back_to_the_cut() {
        // Exited maps to None, so it never enters the rollup slice; with no
        // other live Sessions the cut-derived status stands (never a stale
        // Working/Done left pinned by a dead process).
        assert_eq!(session_status_from_signal(SessionSignal::Exited), None);
        assert_eq!(
            roll_up_status(WorkspaceStatus::Idle, &[]),
            WorkspaceStatus::Idle
        );
    }

    #[test]
    fn session_signals_serialize_camel_case_for_the_frontend_mirror() {
        assert_eq!(
            serde_json::to_value(SessionSignal::Started).unwrap(),
            serde_json::json!("started")
        );
        assert_eq!(
            serde_json::to_value(SessionSignal::Attention).unwrap(),
            serde_json::json!("attention")
        );
        assert_eq!(
            serde_json::to_value(SessionSignal::Finished).unwrap(),
            serde_json::json!("finished")
        );
    }
}
