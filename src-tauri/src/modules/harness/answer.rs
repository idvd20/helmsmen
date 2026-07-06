//! Answering a paused approval — the PURE half of the ONE fragile seam
//! (M3.5, task #18).
//!
//! The permission-prompt layout is not a stable API, so ALL keystroke
//! injection into an agent pane goes through a single function. This module is
//! its pure, data-in/data-out core: given a snapshot of the agent's CURRENT
//! visible screen, the intended dialog (the card the user answered), and the
//! answer, it returns either the exact [`KeyStep`]s to inject or a
//! [`Mismatch`] that means **inject nothing**. The imperative half
//! ([`crate::modules::runtime::answer`]) takes the screen snapshot over the
//! Runtime trait and executes the plan with `Runtime::write`.
//!
//! # Verify-before-inject is a hard security property (user story 30)
//!
//! Parallel tool calls queue permission dialogs and the *newest* renders on
//! top (spike-identified hazard). Blindly answering whatever is visible would
//! resume the WRONG tool. So the plan is built ONLY when the intended card's
//! exact command is actually visible on screen right now; otherwise the result
//! is a [`Mismatch`] and the shell injects nothing. Post-hoc reconciliation by
//! `tool_use_id` (the control-plane reducer) catches a mis-answer, but this
//! discipline *prevents* one.
//!
//! # Key sequences (from `spike-approval-loop/answer-prompt.sh`, PASS)
//!
//! - **Allow** — inject `1`. On a hook-forced 2-option dialog the digit alone
//!   selects and submits (`1. Yes`).
//! - **Deny with reason** — `Esc` (cancels the tool call — it verifiably never
//!   runs), then type the instruction, then `Enter`; the instruction lands as
//!   a new user message and the agent reroutes.
//! - **Deny without a reason** — `Esc` only (a clean block, no reroute).
//!
//! "Edit input" degrades to Deny + instruct in v1 (native `Tab to amend` is
//! task #20's report-only investigation). These sequences are re-verified
//! against a live `claude` per Claude Code release (test seam 2).

use serde::Serialize;

/// The Escape key — "No, and tell Claude what to do differently": cancels the
/// pending tool call and focuses the input box.
const ESC: &[u8] = b"\x1b";
/// Carriage return — submits the typed instruction as a user message.
const ENTER: &[u8] = b"\r";
/// The Allow keystroke: the digit that selects+submits "1. Yes".
const ALLOW: &[u8] = b"1";

/// Settle delay after `Esc` before typing the instruction (matches the spike's
/// `sleep 0.4`; the dialog dismiss + input focus needs a beat).
const AFTER_ESC_MS: u64 = 400;
/// Settle delay after typing the instruction before submitting (spike's
/// `sleep 0.2`).
const BEFORE_ENTER_MS: u64 = 200;

/// Markers that a Claude Code permission dialog is currently on screen. Any one
/// present is enough. These are the FRAGILE, layout-coupled part of the seam —
/// they are matched case-insensitively against the stripped screen and
/// re-verified live per Claude Code release. If they drift, matching fails
/// *safe*: an answer becomes [`Mismatch::NoDialog`] (never a mis-injection).
const DIALOG_MARKERS: &[&str] = &[
    "esc to cancel",
    "tab to amend",
    "do you want to proceed",
    "1. yes",
    "2. no",
    "no, and tell claude",
];

/// The answer a user gave a paused approval. Constructed by the shell from the
/// (deserialized) IPC input — never deserialized here (harness stays code, not
/// config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAnswer {
    /// Allow the call to proceed (resume exactly where it paused).
    Allow,
    /// Deny the call. A non-empty `reason` is typed back as a user message so
    /// the agent reroutes; an empty one is a clean block (Esc only).
    Deny { reason: String },
}

/// The dialog the user MEANT to answer — the intended card's identity as the
/// verify-before-inject check matches it against the live screen. `tool_use_id`
/// is the correlation key for post-hoc reconciliation; `expected_command` is
/// the exact command/file the card showed and the string that must be visible
/// on screen before any key is injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntendedDialog<'a> {
    pub tool_use_id: Option<&'a str>,
    pub expected_command: &'a str,
}

/// One step the imperative shell executes against the Runtime, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStep {
    /// Write these bytes to the session (`Runtime::write`), verbatim.
    Inject(Vec<u8>),
    /// Sleep this many milliseconds (let the dialog/input settle).
    Wait(u64),
}

/// Why a verify-before-inject refused to act. Serialize-only so the shell can
/// surface it to the frontend ("dialog changed — not answered"); never
/// deserialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mismatch {
    /// No permission dialog is on screen at all — the agent is not paused
    /// where we expected. Inject nothing.
    NoDialog,
    /// A dialog is up, but it is NOT this card's: the intended command is not
    /// the one currently visible (a queued dialog for another call is on top).
    /// Inject nothing.
    DialogNotVisible,
}

/// Strip the escape/control sequences a raw PTY snapshot carries so the visible
/// text can be matched like tmux `capture-pane -p` output. Removes CSI
/// (`ESC [ … final`), OSC (`ESC ] … BEL/ST`), and other two-byte `ESC x`
/// sequences, drops remaining control bytes, and collapses runs of whitespace
/// to single spaces. This is NOT the Runtime interpreting output to act on it
/// (forbidden) — it is the answering seam reading the screen to verify a
/// dialog, exactly what a real `capture-pane` hands back rendered.
fn visible_text(snapshot: &[u8]) -> String {
    let text = String::from_utf8_lossy(snapshot);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    // CSI: consume until a final byte in 0x40..=0x7e.
                    for f in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&f) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: consume until BEL or ST (ESC \).
                    while let Some(&f) = chars.peek() {
                        if f == '\u{07}' {
                            chars.next();
                            break;
                        }
                        if f == '\u{1b}' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        chars.next();
                    }
                }
                // Any other ESC x (e.g. `ESC c` reset): drop both bytes.
                _ => {}
            }
            continue;
        }
        // Keep printable/whitespace; drop other control bytes.
        if c.is_control() {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    collapse_ws(&out)
}

/// Collapse every run of ASCII whitespace to a single space and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The pure plan for answering a Claude Code permission dialog: verify the
/// intended card is the one on screen, then return its key sequence. On any
/// mismatch return the reason and NO steps — the shell injects nothing.
///
/// Command matching is case-SENSITIVE (tightest form of the security check —
/// never resume a differently-cased command); dialog-presence markers are
/// matched case-insensitively (layout copy, not identity).
pub fn claude_code_answer_plan(
    snapshot: &[u8],
    dialog: &IntendedDialog,
    answer: &PromptAnswer,
) -> Result<Vec<KeyStep>, Mismatch> {
    let screen = visible_text(snapshot);

    // 1. Is a permission dialog even up? If not, do not inject a stray key.
    let lower = screen.to_lowercase();
    if !DIALOG_MARKERS.iter().any(|m| lower.contains(m)) {
        return Err(Mismatch::NoDialog);
    }

    // 2. Is THIS card's exact command the one visible? An empty expected
    //    command can never be verified, so it fails safe.
    let want = collapse_ws(dialog.expected_command);
    if want.is_empty() || !screen.contains(&want) {
        return Err(Mismatch::DialogNotVisible);
    }

    // 3. Verified — build the key sequence.
    Ok(match answer {
        PromptAnswer::Allow => vec![KeyStep::Inject(ALLOW.to_vec())],
        PromptAnswer::Deny { reason } if reason.trim().is_empty() => {
            vec![KeyStep::Inject(ESC.to_vec())]
        }
        PromptAnswer::Deny { reason } => vec![
            KeyStep::Inject(ESC.to_vec()),
            KeyStep::Wait(AFTER_ESC_MS),
            KeyStep::Inject(reason.as_bytes().to_vec()),
            KeyStep::Wait(BEFORE_ENTER_MS),
            KeyStep::Inject(ENTER.to_vec()),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic hook-forced permission dialog, with ANSI color codes and a
    /// cursor move interspersed, as a raw PTY snapshot would carry it.
    fn dialog_screen(command: &str) -> Vec<u8> {
        format!(
            "\x1b[2J\x1b[H\x1b[1mBash\x1b[0m command\r\n  \x1b[36m{command}\x1b[0m\r\n\r\n\
             Hook PreToolUse:Bash requires confirmation\r\n\
             \x1b[7m 1. Yes \x1b[0m\r\n 2. No, and tell Claude what to do differently\r\n\
             \x1b[2mEsc to cancel \u{b7} Tab to amend \u{b7} ctrl+e to explain\x1b[0m",
            command = command
        )
        .into_bytes()
    }

    fn allow(snapshot: &[u8], cmd: &str) -> Result<Vec<KeyStep>, Mismatch> {
        claude_code_answer_plan(
            snapshot,
            &IntendedDialog {
                tool_use_id: Some("toolu_a"),
                expected_command: cmd,
            },
            &PromptAnswer::Allow,
        )
    }

    #[test]
    fn allow_injects_the_single_accept_keystroke() {
        let screen = dialog_screen("git push --force origin main");
        let steps = allow(&screen, "git push --force origin main").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
    }

    #[test]
    fn deny_with_reason_is_esc_then_instruction_then_enter() {
        let screen = dialog_screen("git push --force origin main");
        let steps = claude_code_answer_plan(
            &screen,
            &IntendedDialog {
                tool_use_id: Some("toolu_a"),
                expected_command: "git push --force origin main",
            },
            &PromptAnswer::Deny {
                reason: "open a PR instead".to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            steps,
            vec![
                KeyStep::Inject(b"\x1b".to_vec()),
                KeyStep::Wait(AFTER_ESC_MS),
                KeyStep::Inject(b"open a PR instead".to_vec()),
                KeyStep::Wait(BEFORE_ENTER_MS),
                KeyStep::Inject(b"\r".to_vec()),
            ]
        );
    }

    #[test]
    fn deny_without_a_reason_is_a_bare_esc_block() {
        let screen = dialog_screen("git reset --hard HEAD~3");
        let steps = claude_code_answer_plan(
            &screen,
            &IntendedDialog {
                tool_use_id: Some("toolu_a"),
                expected_command: "git reset --hard HEAD~3",
            },
            &PromptAnswer::Deny {
                reason: "   ".to_string(),
            },
        )
        .unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"\x1b".to_vec())]);
    }

    // --- verify-before-inject: the hard security property (story 30) ---

    #[test]
    fn queued_dialog_for_another_call_is_a_mismatch_and_injects_nothing() {
        // Two calls queued; the NEWEST (call B) renders on top. We intend to
        // answer card A, but B's command is what's visible → mismatch.
        let visible_b = dialog_screen("git rebase -i HEAD~5");
        let err = allow(&visible_b, "git push --force origin main").unwrap_err();
        assert_eq!(err, Mismatch::DialogNotVisible, "must not answer the wrong dialog");
    }

    #[test]
    fn no_dialog_on_screen_is_a_mismatch() {
        // Ordinary agent output, no permission prompt: injecting `1` would be a
        // stray keystroke.
        let screen = b"\x1b[32mrunning tests...\x1b[0m\r\n123 passed\r\n";
        let err = allow(screen, "git push --force origin main").unwrap_err();
        assert_eq!(err, Mismatch::NoDialog);
    }

    #[test]
    fn an_empty_expected_command_can_never_verify() {
        let screen = dialog_screen("git push --force origin main");
        let err = allow(&screen, "").unwrap_err();
        assert_eq!(err, Mismatch::DialogNotVisible);
    }

    #[test]
    fn matching_tolerates_ansi_and_reflowed_whitespace() {
        // The command is split across a soft-wrap and carries color codes; the
        // stripped+whitespace-collapsed screen still contains it.
        let screen = b"\x1b[1mBash\x1b[0m\r\n  \x1b[36mnpm    publish\r\n  --access public\x1b[0m\r\n\
            Do you want to proceed?\r\n 1. Yes\r\n 2. No"
            .to_vec();
        let steps = allow(&screen, "npm publish --access public").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
    }

    #[test]
    fn command_match_is_case_sensitive() {
        // A differently-cased command is NOT the same call — never resume it.
        let screen = dialog_screen("RM -RF BUILD");
        let err = allow(&screen, "rm -rf build").unwrap_err();
        assert_eq!(err, Mismatch::DialogNotVisible);
    }

    #[test]
    fn mismatch_serializes_camel_case_for_the_frontend() {
        assert_eq!(
            serde_json::to_value(Mismatch::NoDialog).unwrap(),
            serde_json::json!("noDialog")
        );
        assert_eq!(
            serde_json::to_value(Mismatch::DialogNotVisible).unwrap(),
            serde_json::json!("dialogNotVisible")
        );
    }
}
