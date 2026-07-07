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

/// Strip the escape/control sequences a snapshot carries and return the visible
/// text as LINES, so the command can be anchored to the dialog's own command
/// row(s) rather than matched anywhere in the blob. Removes CSI
/// (`ESC [ … final`), OSC (`ESC ] … BEL/ST`), and other two-byte `ESC x`
/// sequences, preserves line breaks (`\n`), maps remaining control bytes to
/// spaces, and collapses intra-line whitespace runs to single spaces (blank
/// lines are kept as empty strings). This is NOT the Runtime interpreting
/// output to act on it (forbidden) — it is the answering seam reading the
/// screen to verify a dialog. In production the snapshot is already the
/// rendered visible screen (the Runtime's `capture-pane`); stripping here is
/// defense in depth and keeps the planner testable with raw fixtures.
fn visible_lines(snapshot: &[u8]) -> Vec<String> {
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
        if c == '\n' {
            out.push('\n');
        } else if c.is_control() {
            // \r and friends become a space and collapse away within a line.
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out.lines().map(collapse_ws).collect()
}

/// Collapse every run of ASCII whitespace to a single space and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rank of the hook-confirmation header (`Hook … requires confirmation …`) —
/// the TOP chrome line of a hook-forced dialog, directly below the command box
/// (and the description line) on a live CC 2.1.x screen.
const RANK_CONFIRMATION: u8 = 0;
/// Rank of the `Do you want to proceed?` question. The free-text hook REASON
/// renders between [`RANK_CONFIRMATION`] and this line (verified live), which
/// is the ONE place non-chrome text legally sits inside the chrome stack.
const RANK_PROCEED: u8 = 1;

/// Classify a (lowercased, whitespace-collapsed) visible line as dialog chrome
/// and return WHERE it sits within ONE dialog's vertical layout — top (0) to
/// bottom (4), calibrated against a live CC 2.1.x dialog: the hook-confirmation
/// header, the proceed question, option 1, option 2, the key-hint footer.
///
/// The rank order is what anchors the match to the ACTIVE dialog on a
/// partially repainted screen: walking UP from the bottom-most chrome line,
/// ranks strictly decrease within a single dialog, so a repeated or increasing
/// rank can only be a STALE dialog's chrome left above by a repaint.
///
/// Classification is by line PREFIX (after an optional selection caret), not
/// substring, so a command whose text merely *contains* marker copy (e.g.
/// `echo '1. Yes'`) is a command line, not chrome — a substring test would
/// truncate the command box and make that command unapprovable. The
/// confirmation header is the exception: CC renders it as
/// `Hook <event>:<Tool> requires confirmation …`, so it is keyed on the `hook`
/// prefix plus the phrase. These are the FRAGILE, layout-coupled markers of the
/// seam, re-verified live per Claude Code release; drift fails *safe*
/// ([`Mismatch::DialogNotVisible`], never a mis-injection).
fn chrome_rank(line_lower: &str) -> Option<u8> {
    // The highlighted option may carry a selection caret — strip it before the
    // prefix tests.
    let line = line_lower
        .strip_prefix('\u{276f}') // ❯
        .map(str::trim_start)
        .unwrap_or(line_lower);
    if line.starts_with("hook") && line.contains("requires confirmation") {
        return Some(RANK_CONFIRMATION);
    }
    if line.starts_with("do you want to proceed") {
        return Some(RANK_PROCEED);
    }
    if line.starts_with("1. yes") {
        return Some(2);
    }
    if line.starts_with("2. no") || line.starts_with("no, and tell claude") {
        return Some(3);
    }
    // Footer key hints; `tab to amend` also catches a soft-wrapped footer tail.
    if line.starts_with("esc to cancel") || line.starts_with("tab to amend") {
        return Some(4);
    }
    None
}

/// Command wrappers that are transparent proxies: they run the wrapped command
/// unchanged, so a permission dialog showing `<wrapper> <cmd>` is the SAME call
/// a card recorded as `<cmd>`. The card holds the PRE-hook command (see
/// [`ApprovalCard::input`](crate::modules::core::control_plane::ApprovalCard)),
/// but a user-level `PreToolUse` hook such as RTK prepends the wrapper before
/// the dialog renders — Claude Code runs all PreToolUse hooks in PARALLEL on the
/// ORIGINAL input, so the recording hook can never observe the rewrite, and the
/// live command diverges from the card by exactly this prefix. Only transparent,
/// non-privilege-changing, single-program wrappers belong here: the REMAINDER
/// after stripping must still equal `want` EXACTLY, so this never widens into
/// the substring hazard (a longer or different live command never matches a
/// shorter card). `sudo`/`env`/`time`/`xargs` are deliberately excluded — they
/// change semantics or can hide a second command.
const TRANSPARENT_WRAPPERS: &[&str] = &["rtk"];

/// Does the on-screen `candidate` command denote the same call as the card's
/// `want`? Exact equality, OR exact equality after stripping a single leading
/// transparent-wrapper token (`rtk git …` ≡ `git …`). Both inputs are already
/// whitespace-collapsed, so `rest` carries no leading space.
fn command_matches(candidate: &str, want: &str) -> bool {
    if candidate == want {
        return true;
    }
    match candidate.split_once(' ') {
        Some((wrapper, rest)) if TRANSPARENT_WRAPPERS.contains(&wrapper) => rest == want,
        _ => false,
    }
}

/// Locate the ACTIVE dialog's command box: the range of the contiguous
/// non-blank block of visible lines sitting directly above the active
/// (bottom-most) dialog's TOP chrome line. `None` when no chrome — or nothing
/// above it — is recognizable (fail safe).
///
/// The injected keystroke always lands on the ACTIVE dialog, so the command
/// match must be bounded to THAT dialog's own command box. A repaint without a
/// screen clear can leave an OLDER dialog, or plain output echoing a command,
/// visible ABOVE the active one on the same snapshot — matching anywhere up
/// there would approve a command the user never saw (story 30).
///
/// The walk starts at the bottom-most chrome line and climbs while
/// [`chrome_rank`]s strictly DECREASE — the signature of one dialog read
/// bottom-up. Blank rows are furniture. The one non-chrome text that legally
/// sits inside the chrome stack is the free-text hook REASON between the
/// proceed question and the confirmation header (verified live), so a
/// non-chrome gap is crossed ONLY from the question ([`RANK_PROCEED`]) and
/// ONLY to adopt a confirmation header ([`RANK_CONFIRMATION`]) above it. Any
/// other shape ends the dialog: a repeated/increasing rank is a stale dialog's
/// chrome, and any other non-chrome row is where the command box begins.
///
/// Residual assumption, documented: helmsmen only answers cards its own hook
/// paused, and that hook always forces a dialog WITH the confirmation header.
/// A screen where the active dialog lacks that header while a stale header —
/// plus its command box — survives directly above the active question line is
/// therefore not a reachable answer target; the reason-gap crossing prefers
/// the hook-forced reading of that shape.
fn active_command_box(lines: &[String]) -> Option<std::ops::Range<usize>> {
    let lower: Vec<String> = lines.iter().map(|line| line.to_lowercase()).collect();
    let bottom = lower.iter().rposition(|line| chrome_rank(line).is_some())?;
    let mut top = bottom;
    let mut top_rank = chrome_rank(&lower[bottom]).expect("bottom is a chrome line");
    let mut i = bottom;
    'walk: while i > 0 {
        i -= 1;
        if lower[i].is_empty() {
            continue; // blank rows are dialog furniture
        }
        match chrome_rank(&lower[i]) {
            Some(rank) if rank < top_rank => {
                top = i;
                top_rank = rank;
            }
            // A repeated or increasing rank is another (stale) dialog's chrome.
            Some(_) => break,
            // A non-chrome row below the proceed question may be the hook
            // reason: cross it iff a confirmation header sits directly above.
            None if top_rank == RANK_PROCEED => {
                let mut j = i;
                loop {
                    if j == 0 {
                        break 'walk; // screen top: the gap was the command box
                    }
                    j -= 1;
                    if lower[j].is_empty() {
                        continue;
                    }
                    match chrome_rank(&lower[j]) {
                        Some(RANK_CONFIRMATION) => {
                            top = j;
                            top_rank = RANK_CONFIRMATION;
                            i = j;
                            break;
                        }
                        // Any other chrome above the gap: the gap was the
                        // command box, not a reason.
                        Some(_) => break 'walk,
                        None => {} // still inside the (multi-line) gap
                    }
                }
            }
            // Above the dialog's top chrome line: the command box starts here.
            None => break,
        }
    }
    // The command box is the contiguous non-blank block directly above the
    // top chrome line (blank rows between box and chrome are tolerated; the
    // box may end with the description line CC renders under the command).
    let mut end = top;
    while end > 0 && lines[end - 1].is_empty() {
        end -= 1;
    }
    let mut begin = end;
    while begin > 0 && !lines[begin - 1].is_empty() {
        begin -= 1;
    }
    (begin < end).then_some(begin..end)
}

/// Is `want` the command of the ACTIVE dialog currently on screen? True iff
/// some run of consecutive visible lines WITHIN the active dialog's own
/// command box ([`active_command_box`]) collapses to EXACTLY `want`
/// (case-sensitive), or to a transparent wrapper applied to `want`
/// ([`command_matches`]). Exact equality defeats the substring/prefix hazard —
/// a card command that is only a prefix of a longer, more dangerous live
/// command ("git push" vs the on-screen "git push --force origin main") never
/// matches. The multi-line run tolerates a soft-wrapped command; bounding the
/// search to the active dialog's command box keeps a stale dialog or echoed
/// output higher on a partially repainted screen from verifying a card the
/// keystroke would not answer. Any layout that yields no exact-line match →
/// DialogNotVisible (fail safe, never a mis-injection).
fn command_is_on_screen(lines: &[String], want: &str) -> bool {
    let Some(command_box) = active_command_box(lines) else {
        return false;
    };
    for start in command_box.clone() {
        let mut run: Vec<&str> = Vec::new();
        for line in &lines[start..command_box.end] {
            run.push(line);
            if command_matches(&collapse_ws(&run.join(" ")), want) {
                return true;
            }
        }
    }
    false
}

/// Test-only calibration aid: render a snapshot EXACTLY as the matcher sees it
/// — the stripped visible lines, numbered, with the chrome boundary marked and
/// the `command_is_on_screen` verdict. The live canary prints this so a real
/// mismatch reveals the true Claude Code layout to calibrate against, instead
/// of guessing. Not compiled into release builds.
#[cfg(test)]
pub(crate) fn debug_dump_screen(snapshot: &[u8], want: &str) -> String {
    use std::fmt::Write as _;
    let lines = visible_lines(snapshot);
    let command_box = active_command_box(&lines);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n=== visible screen: {} lines, active command box = {:?} ===",
        lines.len(),
        command_box
    );
    for (i, line) in lines.iter().enumerate() {
        let rank = chrome_rank(&line.to_lowercase());
        let flag = match (&command_box, rank) {
            (_, Some(rank)) => format!(" <== chrome (rank {rank})"),
            (Some(range), None) if range.contains(&i) => " <== command box".to_string(),
            _ => String::new(),
        };
        let _ = writeln!(out, "{i:>3} | {line:?}{flag}");
    }
    let _ = writeln!(
        out,
        "=== command_is_on_screen({want:?}) = {} ===",
        command_is_on_screen(&lines, want)
    );
    out
}

/// The pure plan for answering a Claude Code permission dialog: verify the
/// intended card is the one on screen, then return its key sequence. On any
/// mismatch return the reason and NO steps — the shell injects nothing.
///
/// Command matching is case-SENSITIVE and EXACT (tightest form of the security
/// check — never resume a differently-cased command, and never resume a
/// command that only *contains* the card's command as a substring/prefix); it
/// is anchored to the ACTIVE (bottom-most) dialog's own command row(s) — the
/// dialog the injected keystroke actually answers — not matched anywhere in
/// the screen, so a stale dialog or echoed output left above by a partial
/// repaint never verifies. The one tolerated divergence is a leading transparent-wrapper token
/// ([`TRANSPARENT_WRAPPERS`], e.g. `rtk`) a user-level hook prepended after the
/// card was recorded — the remainder must still equal the card command exactly. Dialog-presence markers are matched case-insensitively (layout copy,
/// not identity). The snapshot passed here is the CURRENT visible screen (the
/// Runtime's `capture-pane`), so a dismissed or a queued-underneath dialog is
/// not on it — the seam verifies against what is truly live (user story 30).
pub fn claude_code_answer_plan(
    snapshot: &[u8],
    dialog: &IntendedDialog,
    answer: &PromptAnswer,
) -> Result<Vec<KeyStep>, Mismatch> {
    let lines = visible_lines(snapshot);

    // 1. Is a permission dialog even up? If not, do not inject a stray key.
    let has_dialog = lines.iter().any(|line| {
        let lower = line.to_lowercase();
        DIALOG_MARKERS.iter().any(|m| lower.contains(m))
    });
    if !has_dialog {
        return Err(Mismatch::NoDialog);
    }

    // 2. Is THIS card's exact command the one on the live dialog? Matched
    //    exactly against the command row(s) directly above the prompt, so a
    //    substring/prefix of a different, longer command never verifies. An
    //    empty expected command can never be verified, so it fails safe.
    let want = collapse_ws(dialog.expected_command);
    if want.is_empty() || !command_is_on_screen(&lines, &want) {
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
    fn a_prefix_of_a_longer_visible_command_is_a_mismatch() {
        // The user's card is "git push" (a benign call they approved), but the
        // dialog actually up is for the more dangerous "git push --force origin
        // main". The card's command is a PREFIX of the visible one — verifying
        // by substring would Allow the force-push. It must be a mismatch.
        let screen = dialog_screen("git push --force origin main");
        let err = allow(&screen, "git push").unwrap_err();
        assert_eq!(
            err,
            Mismatch::DialogNotVisible,
            "a prefix must not resume a longer, different command"
        );
    }

    #[test]
    fn a_substring_of_a_longer_visible_command_is_a_mismatch() {
        // Card "rm -rf build"; the live dialog is "rm -rf build/../../etc". The
        // card command is a substring of the visible one — must not Allow the
        // path-escaping variant.
        let screen = dialog_screen("rm -rf build/../../etc");
        let err = allow(&screen, "rm -rf build").unwrap_err();
        assert_eq!(err, Mismatch::DialogNotVisible, "a substring is not the same command");
    }

    /// One real CC 2.1.x hook-forced dialog (captured live), WITHOUT a leading
    /// screen clear: the command is followed by a human DESCRIPTION line and
    /// the hook reason before the prompt — the command is not immediately
    /// adjacent to the chrome.
    fn live_dialog_frame(command: &str) -> String {
        format!(
            " Bash command\r\n\r\n  \x1b[36m{command}\x1b[0m\r\n  \
             Amend the last commit keeping the same message\r\n\r\n \
             Hook PreToolUse:Bash requires confirmation for this command:\r\n \
             Helmsmen: git history rewrite \u{2014} approval required\r\n\r\n \
             Do you want to proceed?\r\n \x1b[7m 1. Yes \x1b[0m\r\n   2. No\r\n\r\n \
             Esc to cancel \u{b7} Tab to amend \u{b7} ctrl+e to explain",
            command = command
        )
    }

    /// The real CC 2.1.x hook-forced dialog (captured live) on a cleanly
    /// repainted screen. The match must still find the command (anchored to
    /// the dialog's own command box), and must still reject a mere prefix.
    fn live_layout_screen(command: &str) -> Vec<u8> {
        format!("\x1b[2J\x1b[H{}", live_dialog_frame(command)).into_bytes()
    }

    #[test]
    fn live_dialog_with_a_description_line_still_matches_the_command() {
        let screen = live_layout_screen("git commit --amend --no-edit");
        let steps = allow(&screen, "git commit --amend --no-edit").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
    }

    #[test]
    fn live_dialog_still_rejects_a_prefix_of_the_command() {
        let screen = live_layout_screen("git commit --amend --no-edit");
        let err = allow(&screen, "git commit").unwrap_err();
        assert_eq!(err, Mismatch::DialogNotVisible);
    }

    // --- transparent command wrappers (RTK): a user-level PreToolUse hook
    // prepends `rtk `, so the LIVE command diverges from the card by exactly
    // that token. The card holds the pre-rewrite command; the wrapper strip
    // reconciles them WITHOUT reopening the substring hazard (task #18 live). ---

    #[test]
    fn rtk_wrapped_live_command_matches_the_bare_card_command() {
        // Exactly the captured live layout: RTK rewrote the command to
        // "rtk git commit --amend --no-edit" on screen, but the card recorded
        // the pre-rewrite "git commit --amend --no-edit". Stripping the
        // transparent wrapper reconciles them → Allow injects.
        let screen = live_layout_screen("rtk git commit --amend --no-edit");
        let steps = allow(&screen, "git commit --amend --no-edit").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
    }

    #[test]
    fn rtk_wrapped_still_rejects_a_shorter_prefix_card() {
        // The wrapper strip must not widen into the substring hazard: card
        // "git commit" vs live "rtk git commit --amend --no-edit" — the
        // REMAINDER after stripping rtk ("git commit --amend --no-edit") is not
        // exactly "git commit", so it must mismatch.
        let screen = live_layout_screen("rtk git commit --amend --no-edit");
        let err = allow(&screen, "git commit").unwrap_err();
        assert_eq!(
            err,
            Mismatch::DialogNotVisible,
            "stripping a wrapper must still require the remainder to match exactly"
        );
    }

    #[test]
    fn a_non_transparent_wrapper_is_not_stripped() {
        // `sudo` is NOT a transparent wrapper (privilege change): a card for
        // "git commit --amend --no-edit" must NOT resume a live
        // "sudo git commit --amend --no-edit" dialog.
        let screen = live_layout_screen("sudo git commit --amend --no-edit");
        let err = allow(&screen, "git commit --amend --no-edit").unwrap_err();
        assert_eq!(
            err,
            Mismatch::DialogNotVisible,
            "only allowlisted transparent wrappers are stripped"
        );
    }

    #[test]
    fn command_match_is_case_sensitive() {
        // A differently-cased command is NOT the same call — never resume it.
        let screen = dialog_screen("RM -RF BUILD");
        let err = allow(&screen, "rm -rf build").unwrap_err();
        assert_eq!(err, Mismatch::DialogNotVisible);
    }

    // --- partial repaint: the match must be bounded to the ACTIVE dialog
    // (story 30). A repaint without a screen clear (no ESC[2J) can leave an
    // OLDER dialog — or plain output echoing a command — visible ABOVE the
    // active one on the same screen. The injected `1` always lands on the
    // ACTIVE (bottom-most) dialog, so a match anywhere higher would approve a
    // command the user never saw. ---

    /// A PARTIALLY repainted screen: dialog A (stale, already superseded) is
    /// still fully visible near the top — no ESC[2J was issued — and the
    /// ACTIVE dialog B was painted below it. Both use the live-captured
    /// layout, so two commands coexist above one shared bottom edge.
    fn partially_repainted_screen(stale: &str, active: &str) -> Vec<u8> {
        format!("{}\r\n{}", live_dialog_frame(stale), live_dialog_frame(active)).into_bytes()
    }

    #[test]
    fn a_stale_dialog_above_the_active_one_cannot_verify_its_card() {
        // The user answers the EARLIER card (A). Its exact command is still on
        // screen — but only in the stale top dialog; the keystroke would land
        // on the ACTIVE bottom dialog (B). Must refuse, never inject.
        let screen =
            partially_repainted_screen("git push --force origin main", "git rebase -i HEAD~5");
        let err = allow(&screen, "git push --force origin main").unwrap_err();
        assert_eq!(
            err,
            Mismatch::DialogNotVisible,
            "a stale dialog's command above the active one must not verify"
        );
    }

    #[test]
    fn the_active_bottom_dialog_still_verifies_on_a_partially_repainted_screen() {
        // Same screen: the card for the ACTIVE (bottom-most) dialog is the one
        // the keystroke actually answers — it must verify.
        let screen =
            partially_repainted_screen("git push --force origin main", "git rebase -i HEAD~5");
        let steps = allow(&screen, "git rebase -i HEAD~5").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
    }

    #[test]
    fn a_command_echoed_in_output_above_the_active_dialog_cannot_verify() {
        // Agent output above the dialog echoes card A's exact command as a
        // plain line (partial repaint, no clear). The active dialog below is
        // for a DIFFERENT command. Answering card A must refuse.
        let screen = "$ tail of ordinary agent output\r\n\
             git push --force origin main\r\n\r\n\
             Bash command\r\n  \x1b[36mgit rebase -i HEAD~5\x1b[0m\r\n\r\n\
             Hook PreToolUse:Bash requires confirmation\r\n\
             \x1b[7m 1. Yes \x1b[0m\r\n 2. No, and tell Claude what to do differently\r\n\
             Esc to cancel \u{b7} Tab to amend"
            .as_bytes()
            .to_vec();
        let err = allow(&screen, "git push --force origin main").unwrap_err();
        assert_eq!(
            err,
            Mismatch::DialogNotVisible,
            "an echoed command outside the active dialog's command box must not verify"
        );
        // The active dialog's own card still verifies on the same screen.
        let steps = allow(&screen, "git rebase -i HEAD~5").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
    }

    // --- chrome classification: a command CONTAINING marker text is a
    // command, not chrome. Misclassifying it truncates the command box so the
    // command can never be approved from the wall (fails safe, but wrongly). ---

    #[test]
    fn a_command_containing_chrome_marker_text_is_still_approvable() {
        let screen = dialog_screen("echo '1. Yes'");
        let steps = allow(&screen, "echo '1. Yes'").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);

        let screen = dialog_screen("printf 'Esc to cancel \u{b7} Tab to amend'");
        let steps = allow(&screen, "printf 'Esc to cancel \u{b7} Tab to amend'").unwrap();
        assert_eq!(steps, vec![KeyStep::Inject(b"1".to_vec())]);
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
