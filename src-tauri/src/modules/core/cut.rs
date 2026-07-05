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
}
