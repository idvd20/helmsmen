//! Answering a paused approval — the IMPERATIVE half of the ONE fragile seam
//! (M3.5, task #18).
//!
//! [`answer_prompt`] is THE single function through which every keystroke is
//! injected into an agent pane. It is deliberately fenced: it (1) snapshots the
//! target session's current visible screen over the Runtime trait (the
//! `capture-pane` analog — never re-pointing the live sink), (2) asks the pure
//! Harness planner ([`crate::modules::harness::answer`]) whether that screen is
//! the intended card's dialog, (3) injects the returned key sequence with
//! `Runtime::write` ONLY on a match — a mismatch injects nothing — and (4)
//! leaves post-hoc reconciliation by `tool_use_id` to the control-plane
//! reducer.
//!
//! Runtime-generic: it takes `&dyn Runtime`, so the #6 conformance machinery
//! (snapshot + write) is all it needs and Tmux at M4 reuses it verbatim.
//! Harness-generic: it takes `&dyn Harness`, so each agent owns its own prompt
//! key sequences.

use serde::Serialize;

use crate::modules::harness::{Harness, IntendedDialog, KeyStep, Mismatch, PromptAnswer};

use super::Runtime;

/// The result of an answer attempt, for the frontend inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum AnswerOutcome {
    /// The visible dialog matched the intended card and the key sequence was
    /// injected. The card flips to Allowed/ClosedNoRun via the control plane's
    /// PostToolUse/Stop correlation, not from here.
    Injected,
    /// The visible dialog did NOT match the intended card; nothing was
    /// injected (verify-before-inject; user story 30).
    Mismatch { reason: Mismatch },
}

/// Answer a paused approval by injecting keys into the agent's PTY — the ONE
/// send-keys seam. Snapshots the screen, verifies it is the intended card's
/// dialog (pure Harness planner), and injects the key sequence only on a match.
/// `sleep` runs the plan's inter-key settle delays (production passes
/// `thread::sleep`; tests pass a recorder). A write that fails mid-sequence
/// surfaces as `Err`; the verify-before-inject guarantee still holds because
/// the match is decided before any byte is written.
pub fn answer_prompt(
    runtime: &dyn Runtime,
    harness: &dyn Harness,
    session: &str,
    dialog: &IntendedDialog,
    answer: &PromptAnswer,
    mut sleep: impl FnMut(u64),
) -> Result<AnswerOutcome, String> {
    // (1) Snapshot the target session's CURRENT visible screen.
    let snapshot = runtime.snapshot(session)?;

    // (2) Match the on-screen dialog against the intended card (pure, per
    //     Harness). (3) A mismatch injects NOTHING.
    let steps = match harness.answer_plan(&snapshot, dialog, answer) {
        Ok(steps) => steps,
        Err(reason) => return Ok(AnswerOutcome::Mismatch { reason }),
    };

    // (3) Verified — inject the exact key sequence, in order.
    for step in steps {
        match step {
            KeyStep::Inject(bytes) => runtime.write(session, &bytes)?,
            KeyStep::Wait(ms) => sleep(ms),
        }
    }
    Ok(AnswerOutcome::Injected)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::modules::harness::claude_code::ClaudeCode;
    use crate::modules::runtime::{OutputSink, SessionStatus, SpawnSpec};

    /// A fake Runtime (the deterministic "fake PTY"): it returns a preset
    /// screen from `snapshot` and records every `write` in order, so the
    /// answering logic is provable with no live agent. Only the two methods the
    /// seam touches do anything.
    struct FakePty {
        screen: Vec<u8>,
        writes: Mutex<Vec<Vec<u8>>>,
    }

    impl FakePty {
        fn showing(screen: impl Into<Vec<u8>>) -> Self {
            Self {
                screen: screen.into(),
                writes: Mutex::new(Vec::new()),
            }
        }
        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes.lock().unwrap().clone()
        }
    }

    impl Runtime for FakePty {
        fn spawn(&self, _spec: SpawnSpec, _sink: OutputSink) -> Result<String, String> {
            unreachable!("the answer seam never spawns")
        }
        fn attach(&self, _session: &str, _sink: OutputSink) -> Result<(), String> {
            unreachable!("the answer seam never attaches")
        }
        fn write(&self, _session: &str, bytes: &[u8]) -> Result<(), String> {
            self.writes.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }
        fn snapshot(&self, _session: &str) -> Result<Vec<u8>, String> {
            Ok(self.screen.clone())
        }
        fn resize(&self, _session: &str, _cols: u16, _rows: u16) -> Result<(), String> {
            Ok(())
        }
        fn status(&self, _session: &str) -> Result<SessionStatus, String> {
            Ok(SessionStatus::Running)
        }
        fn kill(&self, _session: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn dialog_screen(command: &str) -> Vec<u8> {
        format!(
            "\x1b[1mBash\x1b[0m\r\n  \x1b[36m{command}\x1b[0m\r\n\
             Hook PreToolUse:Bash requires confirmation\r\n 1. Yes\r\n \
             2. No, and tell Claude what to do differently\r\n\
             Esc to cancel \u{b7} Tab to amend"
        )
        .into_bytes()
    }

    fn dialog(cmd: &str) -> IntendedDialog<'_> {
        IntendedDialog {
            tool_use_id: Some("toolu_a"),
            expected_command: cmd,
        }
    }

    /// A sleep recorder + its no-op closure.
    fn recorder() -> (std::rc::Rc<std::cell::RefCell<Vec<u64>>>, impl FnMut(u64)) {
        let waits = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = waits.clone();
        (waits, move |ms| sink.borrow_mut().push(ms))
    }

    #[test]
    fn allow_on_a_matching_dialog_injects_exactly_the_accept_key() {
        let pty = FakePty::showing(dialog_screen("git push --force origin main"));
        let (_waits, sleep) = recorder();
        let outcome = answer_prompt(
            &pty,
            &ClaudeCode,
            "lpty-1",
            &dialog("git push --force origin main"),
            &PromptAnswer::Allow,
            sleep,
        )
        .unwrap();
        assert_eq!(outcome, AnswerOutcome::Injected);
        assert_eq!(pty.writes(), vec![b"1".to_vec()], "one accept keystroke, nothing else");
    }

    #[test]
    fn deny_with_reason_injects_esc_reason_enter_with_the_settle_waits() {
        let pty = FakePty::showing(dialog_screen("git push --force origin main"));
        let (waits, sleep) = recorder();
        let outcome = answer_prompt(
            &pty,
            &ClaudeCode,
            "lpty-1",
            &dialog("git push --force origin main"),
            &PromptAnswer::Deny {
                reason: "open a PR instead".to_string(),
            },
            sleep,
        )
        .unwrap();
        assert_eq!(outcome, AnswerOutcome::Injected);
        assert_eq!(
            pty.writes(),
            vec![
                b"\x1b".to_vec(),
                b"open a PR instead".to_vec(),
                b"\r".to_vec()
            ]
        );
        // The two inter-key settle delays ran, in order.
        assert_eq!(*waits.borrow(), vec![400, 200]);
    }

    #[test]
    fn a_queued_dialog_for_another_call_injects_nothing() {
        // The visible dialog is call B's; we intend to answer card A. The seam
        // must inject NOTHING — the hard security property (story 30).
        let pty = FakePty::showing(dialog_screen("git rebase -i HEAD~5"));
        let (_waits, sleep) = recorder();
        let outcome = answer_prompt(
            &pty,
            &ClaudeCode,
            "lpty-1",
            &dialog("git push --force origin main"),
            &PromptAnswer::Allow,
            sleep,
        )
        .unwrap();
        assert_eq!(
            outcome,
            AnswerOutcome::Mismatch {
                reason: Mismatch::DialogNotVisible
            }
        );
        assert!(pty.writes().is_empty(), "mismatch must inject nothing");
    }

    #[test]
    fn no_dialog_on_screen_injects_nothing() {
        let pty = FakePty::showing(b"running tests...\r\n123 passed\r\n".to_vec());
        let (_waits, sleep) = recorder();
        let outcome = answer_prompt(
            &pty,
            &ClaudeCode,
            "lpty-1",
            &dialog("git push --force origin main"),
            &PromptAnswer::Deny {
                reason: "no".to_string(),
            },
            sleep,
        )
        .unwrap();
        assert_eq!(
            outcome,
            AnswerOutcome::Mismatch {
                reason: Mismatch::NoDialog
            }
        );
        assert!(pty.writes().is_empty());
    }

    #[test]
    fn outcome_serializes_the_frontend_shape() {
        assert_eq!(
            serde_json::to_value(AnswerOutcome::Injected).unwrap(),
            serde_json::json!({ "status": "injected" })
        );
        assert_eq!(
            serde_json::to_value(AnswerOutcome::Mismatch {
                reason: Mismatch::DialogNotVisible
            })
            .unwrap(),
            serde_json::json!({ "status": "mismatch", "reason": "dialogNotVisible" })
        );
    }
}

// --- End-to-end seam over a REAL PTY (no claude): the verify-before-inject
//     safety property against genuinely-rendered output. Drives a plain `sh`
//     that paints permission-dialog-shaped frames and clears them, so the
//     snapshot → capture-pane → match → inject path is exercised end to end,
//     deterministically, with no interactive agent. Proves the story-30 fix
//     (HIGH finding): a dialog that is dismissed, or a different dialog
//     rendered on top, is NOT answered. Unix-only (real PTY).
#[cfg(all(test, unix))]
mod real_pty_seam {
    use std::time::{Duration, Instant};

    use super::{answer_prompt, AnswerOutcome};
    use crate::modules::harness::claude_code::ClaudeCode;
    use crate::modules::harness::{IntendedDialog, Mismatch, PromptAnswer};
    use crate::modules::runtime::local_pty::LocalPty;
    use crate::modules::runtime::{OutputSink, Runtime, SpawnSpec};

    fn sink() -> OutputSink {
        OutputSink {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(|_| {}),
        }
    }

    /// Spawn `/bin/sh -c script` on a real PTY, then poll the rendered
    /// snapshot until it satisfies `ready` (bounded), so the assertion runs
    /// against a settled visible screen.
    fn spawn_until(script: &str, ready: impl Fn(&str) -> bool) -> (LocalPty, String) {
        let rt = LocalPty::default();
        let spec = SpawnSpec {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            env: Default::default(),
            cols: 120,
            rows: 40,
        };
        let id = rt.spawn(spec, sink()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let screen = rt.snapshot(&id).unwrap_or_default();
            if ready(&String::from_utf8_lossy(&screen)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        (rt, id)
    }

    fn dialog_frame(command: &str) -> String {
        format!(
            "printf 'Bash command\\r\\n  {command}\\r\\nHook PreToolUse:Bash requires confirmation\\r\\n \
             1. Yes\\r\\n 2. No, and tell Claude what to do differently\\r\\nEsc to cancel\\r\\n'; "
        )
    }

    fn intend(command: &str) -> IntendedDialog<'_> {
        IntendedDialog {
            tool_use_id: Some("toolu_a"),
            expected_command: command,
        }
    }

    #[test]
    fn a_dialog_rendered_on_top_hides_the_one_beneath_so_it_is_not_answered() {
        // Dialog A is painted, the screen is cleared, then dialog B is painted
        // on top (the newest-on-top hazard, user story 30). The live screen is
        // B's; answering the A card must inject NOTHING, and answering the B
        // card that is actually visible must inject.
        let script = format!(
            "{a}printf '\\033[2J\\033[H'; {b}sleep 5",
            a = dialog_frame("git push --force origin main"),
            b = dialog_frame("git rebase -i HEAD~5"),
        );
        let (rt, id) = spawn_until(&script, |s| s.contains("git rebase -i HEAD~5"));

        // The A card is queued underneath — its command is not on the live
        // screen (it would be, in raw scrollback: the bug this guards).
        let miss = answer_prompt(
            &rt,
            &ClaudeCode,
            &id,
            &intend("git push --force origin main"),
            &PromptAnswer::Allow,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            miss,
            AnswerOutcome::Mismatch { reason: Mismatch::DialogNotVisible },
            "the queued-underneath card must never be answered"
        );

        // The B dialog is the live one, so its card verifies and injects.
        let hit = answer_prompt(
            &rt,
            &ClaudeCode,
            &id,
            &intend("git rebase -i HEAD~5"),
            &PromptAnswer::Allow,
            |_| {},
        )
        .unwrap();
        assert_eq!(hit, AnswerOutcome::Injected, "the visible dialog's card verifies");
        let _ = rt.kill(&id);
    }

    #[test]
    fn a_dismissed_dialog_leaves_no_live_dialog_to_answer() {
        // A dialog is painted then the screen cleared and a plain line drawn —
        // the dialog is gone. Answering its card must report NoDialog (against
        // raw scrollback the stale markers would still match → stray key).
        let script = format!(
            "{a}printf '\\033[2J\\033[HALL_CLEAR_no_dialog\\r\\n'; sleep 5",
            a = dialog_frame("git push --force origin main"),
        );
        let (rt, id) = spawn_until(&script, |s| s.contains("ALL_CLEAR_no_dialog"));

        let miss = answer_prompt(
            &rt,
            &ClaudeCode,
            &id,
            &intend("git push --force origin main"),
            &PromptAnswer::Deny { reason: String::new() },
            |_| {},
        )
        .unwrap();
        assert_eq!(
            miss,
            AnswerOutcome::Mismatch { reason: Mismatch::NoDialog },
            "a dismissed dialog is not a live dialog"
        );
        let _ = rt.kill(&id);
    }
}

// --- Test seam 2: live integration vs a real `claude` prompt ---
//
// The ONE fragile seam, exercised against a live agent: capture-pane match
// before injecting, allow, and deny-with-reason. Fenced and re-run per Claude
// Code release (`cargo test -- --ignored`), so it never spawns an interactive
// agent during the normal suite. It also skips gracefully where `claude` is
// absent (mirrors #6's `--version` liveness / #8's real-PTY test). The
// deterministic match/mismatch + exact key sequences are proven above with the
// fake PTY; this canary catches a prompt-layout drift the deterministic tests
// cannot.
#[cfg(all(test, unix))]
mod live {
    use std::io::Write;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use crate::modules::harness::{IntendedDialog, PromptAnswer};
    use crate::modules::hooks::{claude_code_hook_settings, ControlPlaneEndpoint, CLAUDE_HOOK_SETTINGS_REL};
    use crate::modules::runtime::answer::{answer_prompt, AnswerOutcome};
    use crate::modules::runtime::local_pty::LocalPty;
    use crate::modules::runtime::{OutputSink, Runtime, SpawnSpec};

    fn claude_present() -> bool {
        Command::new("claude").arg("--version").output().is_ok()
    }

    /// A throwaway git worktree with Claude Code hook settings wired to a live
    /// control-plane endpoint, so a risk call pauses with a real dialog.
    fn wired_worktree(url: &str, token: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("helmsmen-live-answer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let _ = Command::new("git").arg("init").arg("-q").current_dir(&root).output();
        let settings = root.join(CLAUDE_HOOK_SETTINGS_REL);
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&settings).unwrap();
        f.write_all(claude_code_hook_settings(url, token).as_bytes()).unwrap();
        root
    }

    fn sink() -> OutputSink {
        OutputSink {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(|_| {}),
        }
    }

    #[test]
    #[ignore = "live interactive claude; per-release canary — run with `cargo test -- --ignored`"]
    fn live_claude_answer_prompt_seam() {
        if !claude_present() {
            eprintln!("skipping: no `claude` on PATH");
            return;
        }

        let endpoint = ControlPlaneEndpoint::start_in(
            std::env::temp_dir().to_string_lossy().into_owned(),
        )
        .expect("endpoint binds");
        let root = wired_worktree(&endpoint.url(), endpoint.token());

        // A risk-list call (git history rewrite) so the hook returns `ask` and
        // the tool pauses; deny below means it verifiably never runs.
        let rt = LocalPty::default();
        let spec = SpawnSpec {
            program: "claude".to_string(),
            args: vec!["run this exact shell command: git commit --amend --no-edit".to_string()],
            cwd: root.to_string_lossy().into_owned(),
            env: Default::default(),
            cols: 120,
            rows: 40,
        };
        let session = match rt.spawn(spec, sink()) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("inconclusive: could not spawn claude: {e}");
                return;
            }
        };

        // Wait (bounded) for a real permission dialog to render.
        let expected = "git commit --amend --no-edit";
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut saw_dialog = false;
        while Instant::now() < deadline {
            let screen = rt.snapshot(&session).unwrap_or_default();
            let text = String::from_utf8_lossy(&screen).to_lowercase();
            if text.contains("esc to cancel") || text.contains("do you want to proceed") {
                saw_dialog = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !saw_dialog {
            // Trust prompt, auth, or an agent-behavior change: inconclusive, not
            // a seam failure. Investigate manually on a release bump.
            eprintln!("inconclusive: no permission dialog observed within deadline");
            let _ = rt.kill(&session);
            return;
        }

        // Verify-before-inject: a card whose command is NOT the visible one
        // must inject nothing (the queued-dialog hazard, against real output).
        let bogus = IntendedDialog {
            tool_use_id: Some("toolu_bogus"),
            expected_command: "npm publish --access public",
        };
        let mismatch = answer_prompt(&rt, &crate::modules::harness::claude_code::ClaudeCode, &session, &bogus, &PromptAnswer::Allow, |ms| {
            std::thread::sleep(Duration::from_millis(ms))
        })
        .unwrap();
        assert!(
            matches!(mismatch, AnswerOutcome::Mismatch { .. }),
            "a non-visible command must never be answered, got {mismatch:?}"
        );

        // Deny-with-reason on the real dialog: the tool verifiably never runs.
        let intended = IntendedDialog {
            tool_use_id: Some("toolu_live"),
            expected_command: expected,
        };
        let denied = answer_prompt(
            &rt,
            &crate::modules::harness::claude_code::ClaudeCode,
            &session,
            &intended,
            &PromptAnswer::Deny {
                reason: "do not amend; leave history alone".to_string(),
            },
            |ms| std::thread::sleep(Duration::from_millis(ms)),
        )
        .unwrap();
        assert_eq!(denied, AnswerOutcome::Injected, "the visible dialog matched, so keys were injected");

        // The endpoint must never record a PostToolUse (the amend never ran).
        std::thread::sleep(Duration::from_secs(3));
        let state = endpoint.snapshot();
        let amend_ran = state.cards.iter().any(|c| {
            c.input.command.as_deref() == Some(expected)
                && matches!(c.status, crate::modules::core::control_plane::CardStatus::Allowed)
        });
        assert!(!amend_ran, "a denied call must verifiably never run");

        let _ = rt.kill(&session);
        let _ = std::fs::remove_dir_all(&root);
    }
}
