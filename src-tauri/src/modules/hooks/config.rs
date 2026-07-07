//! Pure composer for the Claude Code control-plane hook settings (task #16).
//!
//! The cut writes this JSON into the worktree so a launched `claude` POSTs
//! every relevant hook event to its per-Workspace loopback control-plane
//! endpoint. Pure: url + token in, a settings-JSON string out. No I/O — the
//! cut pipeline's imperative shell (`runtime::spawn::apply_config_injection`)
//! writes the bytes, and the endpoint itself lives in [`super::server`].
//!
//! # Coexistence with the user's and Terax's hooks (AC)
//!
//! The wiring targets the worktree-LOCAL settings file
//! ([`CLAUDE_HOOK_SETTINGS_REL`]), never the user's `~/.claude/settings.json`
//! (RTK, Terax's global agent-signal hooks) and never the repo's committed
//! `.claude/settings.json`. Claude Code MERGES hooks across all of those
//! sources, so the user's and Terax's global hooks keep firing untouched —
//! we only add a source, we never clobber one.
//!
//! # A hook payload is DATA
//!
//! The composed command only forwards the hook's stdin (the event JSON Claude
//! Code emits) to the loopback endpoint with the bearer token. It never runs
//! anything the payload contains, and the endpoint ([`super::wire`]) typed-
//! parses the body — a payload can change status/inbox state but never
//! executes.
//!
//! # PreToolUse is the SYNCHRONOUS decision relay (task #17)
//!
//! `PreToolUse` is special: instead of firing and forgetting, its command
//! forwards the event and prints the endpoint's response — the Claude Code
//! permission decision (`ask` / `deny` / `allow`, computed by the pure
//! [`policy`](crate::modules::core::policy)) — to stdout, so Claude Code
//! ENFORCES it. Hard-deny is thereby robust regardless of the agent's prompt
//! layout. If the endpoint is unreachable, the relay prints nothing and exits
//! 0, so Claude Code falls through to its own permission flow (never
//! auto-allowing). The other three hooks stay fire-and-forget as #16 wrote
//! them (their output is status/inbox data only).

use serde_json::json;

/// Worktree-relative settings file the cut writes the control-plane hook
/// wiring into. The LOCAL variant on purpose (see the module docs): it is
/// merged with, never a replacement for, the user's and repo's hook sources.
pub const CLAUDE_HOOK_SETTINGS_REL: &str = ".claude/settings.local.json";

/// The fire-and-forget events (status / inbox signals only). `PreToolUse` is
/// wired separately as the synchronous decision relay.
const FIRE_AND_FORGET_EVENTS: [&str; 3] = ["PostToolUse", "Notification", "Stop"];

/// The fire-and-forget POST a status/inbox hook runs. Claude Code pipes the
/// hook event JSON to the command's stdin; `--data-binary @-` forwards it
/// verbatim to the loopback endpoint under the bearer token. Everything is
/// bounded and silent so a hook never stalls or blocks the agent:
///
/// - `-m 5` caps the whole request (a dead endpoint can't hang a tool call);
/// - `>/dev/null 2>&1` keeps hook output from ever feeding back into Claude;
/// - `|| true` forces exit 0 so a failed POST never blocks the agent.
///
/// `url` is `http://127.0.0.1:<port>/hook` and `token` is 64 hex chars, so
/// neither carries a shell metacharacter; single-quoting keeps them inert
/// regardless, and `serde_json` escapes the whole string for the JSON file.
fn fire_and_forget_command(url: &str, token: &str) -> String {
    format!(
        "curl -sS -m 5 -X POST \
         -H 'Authorization: Bearer {token}' \
         -H 'Content-Type: application/json' \
         --data-binary @- '{url}' >/dev/null 2>&1 || true"
    )
}

/// The SYNCHRONOUS `PreToolUse` relay. Same authenticated POST, but its
/// stdout (the endpoint's permission-decision JSON) is NOT suppressed — Claude
/// Code reads it and enforces the decision. Only stderr is dropped, and a
/// failed/timed-out POST falls back to empty stdout + exit 0, so Claude Code
/// applies its own permission flow rather than the tool auto-running.
fn pre_tool_use_command(url: &str, token: &str) -> String {
    format!(
        "curl -sS -m 5 -X POST \
         -H 'Authorization: Bearer {token}' \
         -H 'Content-Type: application/json' \
         --data-binary @- '{url}' 2>/dev/null || true"
    )
}

fn command_group(command: String) -> serde_json::Value {
    json!([ { "hooks": [ { "type": "command", "command": command } ] } ])
}

/// Compose the settings JSON wiring the control-plane hooks to the
/// per-Workspace loopback endpoint at `url`, authenticated with `token`:
/// `PreToolUse` as the synchronous decision relay, the other three as
/// fire-and-forget status/inbox POSTs.
///
/// The shape is Claude Code's `hooks` settings object: each event maps to a
/// list of matcher groups, each group a list of `command` hooks. No matcher
/// is set, so the hook fires for every tool / notification.
pub fn claude_code_hook_settings(url: &str, token: &str) -> String {
    let mut hooks = serde_json::Map::new();
    hooks.insert(
        "PreToolUse".to_string(),
        command_group(pre_tool_use_command(url, token)),
    );
    let fire = command_group(fire_and_forget_command(url, token));
    for event in FIRE_AND_FORGET_EVENTS {
        hooks.insert(event.to_string(), fire.clone());
    }
    let root = json!({ "hooks": hooks });
    // Pretty + trailing newline: this is a config file a human may open.
    let mut out = serde_json::to_string_pretty(&root).expect("hook settings must serialize");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn settings(url: &str, token: &str) -> Value {
        serde_json::from_str(&claude_code_hook_settings(url, token)).unwrap()
    }

    const URL: &str = "http://127.0.0.1:54321/hook";
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn wires_every_control_plane_event() {
        let s = settings(URL, TOKEN);
        let hooks = s["hooks"].as_object().unwrap();
        // Exactly the events the reducer understands — no more, no less.
        let mut keys: Vec<&str> = hooks.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["Notification", "PostToolUse", "PreToolUse", "Stop"]);
    }

    fn command_for<'a>(s: &'a Value, event: &str) -> &'a str {
        s["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("no command for {event}"))
    }

    #[test]
    fn each_hook_posts_to_the_endpoint_with_the_bearer_token() {
        let s = settings(URL, TOKEN);
        for event in ["PreToolUse", "PostToolUse", "Notification", "Stop"] {
            let command = command_for(&s, event);
            assert!(command.contains(URL), "{event} must POST to the endpoint url");
            assert!(
                command.contains(&format!("Authorization: Bearer {TOKEN}")),
                "{event} must carry the session bearer token"
            );
            // Forwards the hook's stdin verbatim — the event JSON is DATA.
            assert!(command.contains("--data-binary @-"), "{event} forwards stdin");
            assert!(command.contains("-m 5"), "{event} POST is time-bounded");
            assert!(command.contains("|| true"), "{event} exits 0 on failure");
            assert_eq!(s["hooks"][event][0]["hooks"][0]["type"], "command");
        }
    }

    #[test]
    fn pretooluse_is_a_synchronous_relay_that_returns_the_decision_to_claude() {
        let s = settings(URL, TOKEN);
        let pre = command_for(&s, "PreToolUse");
        // Its stdout (the endpoint's decision JSON) must NOT be suppressed —
        // Claude Code reads it to enforce ask/deny/allow. The fire-and-forget
        // stdout-suppressing form (`>/dev/null 2>&1`) must be absent.
        assert!(
            !pre.contains(">/dev/null 2>&1"),
            "PreToolUse must NOT swallow stdout: {pre}"
        );
        assert!(!pre.contains(" >/dev/null"), "no stdout redirect: {pre}");
        // …only stderr is dropped, and it still fails open to exit 0.
        assert!(pre.contains("2>/dev/null"), "PreToolUse drops stderr");
        assert!(pre.ends_with("|| true"));
    }

    #[test]
    fn the_status_hooks_stay_fire_and_forget() {
        let s = settings(URL, TOKEN);
        for event in ["PostToolUse", "Notification", "Stop"] {
            let command = command_for(&s, event);
            assert!(
                command.contains(">/dev/null 2>&1"),
                "{event} output is suppressed"
            );
        }
    }

    #[test]
    fn distinct_tokens_produce_distinct_settings() {
        // A per-session token means each Workspace's file is unique.
        assert_ne!(
            claude_code_hook_settings(URL, "aaaa"),
            claude_code_hook_settings(URL, "bbbb")
        );
    }

    #[test]
    fn the_target_is_the_local_settings_file_never_the_committed_one() {
        // Guards the coexistence AC: we write the LOCAL file, so the user's
        // and Terax's global hooks (a different file) are never clobbered.
        assert_eq!(CLAUDE_HOOK_SETTINGS_REL, ".claude/settings.local.json");
        assert!(!CLAUDE_HOOK_SETTINGS_REL.ends_with("/settings.json"));
    }
}
