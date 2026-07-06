//! Pure request handling for the control-plane endpoint (task #15).
//!
//! This is the boundary logic — token check, size cap, typed parse — with
//! zero I/O so every rejection is unit-testable. The [`server`] module reads
//! bytes off a loopback socket and hands them here; nothing in this file
//! touches the network, filesystem, or clock.
//!
//! [`server`]: super::server
//!
//! A hook payload is DATA, never an instruction: [`parse_hook_event`] turns
//! hostile JSON into one of the fixed, typed
//! [`HookEventKind`](crate::modules::core::control_plane::HookEventKind)
//! variants and nothing else. An unrecognized event name, extra fields, or
//! a malformed body all reduce to a rejection — never to code that runs.

use serde::Deserialize;

use crate::modules::core::control_plane::{HookEventKind, NotificationKind};
use crate::modules::core::policy::{Decision, ToolInput};

/// The one path the endpoint answers. Everything else is a 404 (after auth).
pub const HOOK_PATH: &str = "/hook";

/// Upper bound on a hook request body. Hook payloads are small JSON objects;
/// anything larger is hostile or malformed and is rejected before parsing.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// The pieces of an HTTP request the handler needs. Built by the server from
/// the raw socket bytes; the body is already capped at [`MAX_BODY_BYTES`]
/// (+1, so an overflow is still detectable here).
#[derive(Debug, Clone)]
pub struct RequestParts {
    pub method: String,
    pub path: String,
    /// Raw `Authorization` header value, if present.
    pub auth: Option<String>,
    /// Declared `Content-Length`, if the client sent one.
    pub content_length: Option<usize>,
    pub body: Vec<u8>,
}

/// Why a request was refused. Each maps to a fixed HTTP status; bodies are
/// deliberately terse so a prober learns nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Missing or wrong bearer token — the sole gate.
    Unauthorized,
    MethodNotAllowed,
    NotFound,
    PayloadTooLarge,
    /// Body was not a JSON object we recognize as a hook event.
    BadRequest,
}

impl Rejection {
    /// HTTP status code + reason phrase.
    pub fn status(self) -> (u16, &'static str) {
        match self {
            Rejection::Unauthorized => (401, "Unauthorized"),
            Rejection::MethodNotAllowed => (405, "Method Not Allowed"),
            Rejection::NotFound => (404, "Not Found"),
            Rejection::PayloadTooLarge => (413, "Payload Too Large"),
            Rejection::BadRequest => (400, "Bad Request"),
        }
    }
}

/// The result of handling a request: an accepted, typed event (which the
/// server folds into the control-plane state) or a rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Accepted {
        session_id: String,
        kind: HookEventKind,
    },
    Rejected(Rejection),
}

/// Decide what to do with one request, purely. Order is security-first:
///
/// 1. **Bearer token** — the only gate. A request without the exact
///    per-session token is rejected before its method, path, or body is
///    interpreted at all, so a process that merely discovers the port
///    (but not the token) learns nothing and can inject no event.
/// 2. Method must be `POST`, path must be [`HOOK_PATH`].
/// 3. Size cap — declared or actual body over [`MAX_BODY_BYTES`] is refused.
/// 4. Typed parse — hostile JSON in, a fixed typed event out.
pub fn handle_request(parts: &RequestParts, expected_token: &str) -> Outcome {
    if !authorized(parts.auth.as_deref(), expected_token) {
        return Outcome::Rejected(Rejection::Unauthorized);
    }
    if parts.method != "POST" {
        return Outcome::Rejected(Rejection::MethodNotAllowed);
    }
    if parts.path != HOOK_PATH {
        return Outcome::Rejected(Rejection::NotFound);
    }
    let declared_over = parts.content_length.is_some_and(|n| n > MAX_BODY_BYTES);
    if declared_over || parts.body.len() > MAX_BODY_BYTES {
        return Outcome::Rejected(Rejection::PayloadTooLarge);
    }
    match parse_hook_event(&parts.body) {
        Ok((session_id, kind)) => Outcome::Accepted { session_id, kind },
        Err(_) => Outcome::Rejected(Rejection::BadRequest),
    }
}

/// Constant-time-ish bearer check. Returns true only for `Bearer <token>`
/// exactly equal to the expected per-session token.
fn authorized(header: Option<&str>, expected: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    let Some(token) = header.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// Length-checked, branch-flat byte comparison. The token length is fixed
/// and non-secret, so the early length return leaks nothing useful.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Lenient DTO for the hostile hook body. Every field is optional; the
/// mapping below decides what is required per event, so a missing or
/// mistyped field yields a clean rejection instead of a panic. Unknown
/// extra fields are ignored — they are data, and carry no meaning here.
#[derive(Debug, Deserialize)]
struct HookPayload {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    notification_type: Option<String>,
    /// The tool call's input. Only the fields the pure policy reasons over
    /// (`command`, `file_path`) are lifted out; every other key is ignored as
    /// data. A missing or mistyped `tool_input` yields an empty [`ToolInput`],
    /// never a rejection.
    #[serde(default)]
    tool_input: Option<RawToolInput>,
}

/// The lenient DTO for a PreToolUse `tool_input`. Both fields optional so a
/// hostile or partial object degrades to an empty policy input rather than a
/// parse error.
#[derive(Debug, Deserialize)]
struct RawToolInput {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
}

fn to_tool_input(raw: Option<RawToolInput>) -> ToolInput {
    match raw {
        Some(raw) => ToolInput {
            command: raw.command,
            file_path: raw.file_path,
        },
        None => ToolInput::default(),
    }
}

/// Serialize a policy [`Decision`] into the Claude Code PreToolUse hook
/// response — the JSON the synchronous relay prints to stdout so Claude Code
/// enforces the decision (`ask` pauses, `deny` blocks, `allow` proceeds).
/// This is the wire shape of the decision seam; it is pure, so the mapping is
/// unit-tested here.
pub fn pretooluse_permission_json(decision: &Decision) -> String {
    let (permission, reason) = match decision {
        Decision::Allow => (
            "allow",
            "Helmsmen: allowed (permissive in-worktree)".to_string(),
        ),
        Decision::Ask(rule) => (
            "ask",
            format!("Helmsmen: {} — approval required", rule.label()),
        ),
        Decision::Deny(rule) => ("deny", format!("Helmsmen: {} — blocked", rule.label())),
    };
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": permission,
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// Parse error kinds. Both currently collapse to a 400 for the client; kept
/// distinct for readable internal tests and future observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// Body was not valid JSON of the expected shape.
    Malformed,
    /// A recognizable JSON object but not a hook event we map.
    UnknownEvent,
}

/// Turn a raw hook body into the typed `(session_id, kind)` the pure core
/// consumes. The Claude Code hook payload discriminates on
/// `hook_event_name`; notifications further discriminate on
/// `notification_type`.
pub fn parse_hook_event(body: &[u8]) -> Result<(String, HookEventKind), ParseError> {
    let payload: HookPayload = serde_json::from_slice(body).map_err(|_| ParseError::Malformed)?;
    let name = payload
        .hook_event_name
        .as_deref()
        .ok_or(ParseError::UnknownEvent)?;
    let session_id = payload.session_id.unwrap_or_default();
    let kind = match name {
        "PreToolUse" => HookEventKind::PreToolUse {
            tool_use_id: payload.tool_use_id,
            tool_name: payload.tool_name.unwrap_or_default(),
            input: to_tool_input(payload.tool_input),
        },
        "PostToolUse" => HookEventKind::PostToolUse {
            tool_use_id: payload.tool_use_id,
            tool_name: payload.tool_name.unwrap_or_default(),
        },
        "Notification" => HookEventKind::Notification {
            notification: classify_notification(payload.notification_type.as_deref()),
        },
        "Stop" => HookEventKind::Stop,
        _ => return Err(ParseError::UnknownEvent),
    };
    Ok((session_id, kind))
}

/// Map the hook's `notification_type` to the typed kind. Only the permission
/// and idle prompts carry status meaning; anything else is `Other`.
fn classify_notification(notification_type: Option<&str>) -> NotificationKind {
    match notification_type {
        Some("permission_prompt") => NotificationKind::Permission,
        Some("idle_prompt") => NotificationKind::Idle,
        _ => NotificationKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn parts(auth: Option<&str>, body: &[u8]) -> RequestParts {
        RequestParts {
            method: "POST".to_string(),
            path: HOOK_PATH.to_string(),
            auth: auth.map(|s| s.to_string()),
            content_length: Some(body.len()),
            body: body.to_vec(),
        }
    }

    fn bearer() -> String {
        format!("Bearer {TOKEN}")
    }

    // --- parse: the corpus event shapes ---

    #[test]
    fn parses_pretooluse_with_tool_use_id_and_lifts_the_command() {
        let body = br#"{"session_id":"s1","hook_event_name":"PreToolUse",
            "tool_name":"Bash","tool_use_id":"toolu_x","tool_input":{"command":"ls"}}"#;
        let (sid, kind) = parse_hook_event(body).unwrap();
        assert_eq!(sid, "s1");
        assert_eq!(
            kind,
            HookEventKind::PreToolUse {
                tool_use_id: Some("toolu_x".to_string()),
                tool_name: "Bash".to_string(),
                input: ToolInput::command("ls"),
            }
        );
    }

    #[test]
    fn lifts_file_path_and_tolerates_a_missing_tool_input() {
        let body = br#"{"session_id":"s1","hook_event_name":"PreToolUse",
            "tool_name":"Read","tool_use_id":"toolu_y","tool_input":{"file_path":"/x/.env"}}"#;
        let (_sid, kind) = parse_hook_event(body).unwrap();
        assert_eq!(
            kind,
            HookEventKind::PreToolUse {
                tool_use_id: Some("toolu_y".to_string()),
                tool_name: "Read".to_string(),
                input: ToolInput::file_path("/x/.env"),
            }
        );
        // A PreToolUse without any tool_input degrades to an empty input.
        let body = br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"z"}"#;
        let (_sid, kind) = parse_hook_event(body).unwrap();
        assert_eq!(
            kind,
            HookEventKind::PreToolUse {
                tool_use_id: Some("z".to_string()),
                tool_name: "Bash".to_string(),
                input: ToolInput::default(),
            }
        );
    }

    #[test]
    fn permission_json_maps_each_decision_to_the_claude_code_shape() {
        use crate::modules::core::policy::{DenyRule, RiskRule};
        use serde_json::Value;

        let parse = |d: &Decision| -> Value {
            serde_json::from_str(&pretooluse_permission_json(d)).unwrap()
        };

        let allow = parse(&Decision::Allow);
        assert_eq!(allow["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(allow["hookSpecificOutput"]["permissionDecision"], "allow");

        let ask = parse(&Decision::Ask(RiskRule::GitHistoryRewrite));
        assert_eq!(ask["hookSpecificOutput"]["permissionDecision"], "ask");
        assert!(ask["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("git history rewrite"));

        let deny = parse(&Decision::Deny(DenyRule::Sudo));
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn parses_permission_and_idle_notifications() {
        let perm = br#"{"session_id":"s1","hook_event_name":"Notification",
            "notification_type":"permission_prompt","message":"Claude needs your permission"}"#;
        assert_eq!(
            parse_hook_event(perm).unwrap().1,
            HookEventKind::Notification {
                notification: NotificationKind::Permission
            }
        );
        let idle = br#"{"hook_event_name":"Notification","notification_type":"idle_prompt"}"#;
        assert_eq!(
            parse_hook_event(idle).unwrap().1,
            HookEventKind::Notification {
                notification: NotificationKind::Idle
            }
        );
    }

    #[test]
    fn parses_posttooluse_and_stop() {
        let post = br#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"toolu_y"}"#;
        assert_eq!(
            parse_hook_event(post).unwrap().1,
            HookEventKind::PostToolUse {
                tool_use_id: Some("toolu_y".to_string()),
                tool_name: "Bash".to_string(),
            }
        );
        let stop = br#"{"hook_event_name":"Stop","stop_hook_active":false}"#;
        assert_eq!(parse_hook_event(stop).unwrap().1, HookEventKind::Stop);
    }

    #[test]
    fn unknown_event_and_malformed_body_are_errors() {
        assert_eq!(
            parse_hook_event(br#"{"hook_event_name":"SessionEnd"}"#),
            Err(ParseError::UnknownEvent)
        );
        assert_eq!(
            parse_hook_event(br#"{"no_event_name":true}"#),
            Err(ParseError::UnknownEvent)
        );
        assert_eq!(parse_hook_event(b"not json at all"), Err(ParseError::Malformed));
        assert_eq!(parse_hook_event(b""), Err(ParseError::Malformed));
    }

    // --- handle_request: the AC rejections, all covered ---

    #[test]
    fn valid_request_is_accepted() {
        let body = br#"{"session_id":"s1","hook_event_name":"Stop"}"#;
        let out = handle_request(&parts(Some(&bearer()), body), TOKEN);
        assert_eq!(
            out,
            Outcome::Accepted {
                session_id: "s1".to_string(),
                kind: HookEventKind::Stop,
            }
        );
    }

    #[test]
    fn missing_token_is_rejected() {
        let body = br#"{"hook_event_name":"Stop"}"#;
        assert_eq!(
            handle_request(&parts(None, body), TOKEN),
            Outcome::Rejected(Rejection::Unauthorized)
        );
    }

    #[test]
    fn bad_token_is_rejected_even_on_the_right_path() {
        let body = br#"{"hook_event_name":"Stop"}"#;
        for wrong in ["Bearer wrong", "Bearer ", "Basic xyz", "0123", &bearer()[..20]] {
            assert_eq!(
                handle_request(&parts(Some(wrong), body), TOKEN),
                Outcome::Rejected(Rejection::Unauthorized),
                "token {wrong:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_prober_without_the_token_learns_nothing() {
        // Wrong method AND wrong path AND no token still yields 401 — the
        // request is never interpreted past the gate.
        let probe = RequestParts {
            method: "GET".to_string(),
            path: "/".to_string(),
            auth: None,
            content_length: None,
            body: Vec::new(),
        };
        assert_eq!(
            handle_request(&probe, TOKEN),
            Outcome::Rejected(Rejection::Unauthorized)
        );
    }

    #[test]
    fn oversized_payload_is_rejected() {
        // Actual body over the cap.
        let big = vec![b'x'; MAX_BODY_BYTES + 1];
        assert_eq!(
            handle_request(&parts(Some(&bearer()), &big), TOKEN),
            Outcome::Rejected(Rejection::PayloadTooLarge)
        );
        // Declared Content-Length over the cap, even with a small body.
        let lying = RequestParts {
            method: "POST".to_string(),
            path: HOOK_PATH.to_string(),
            auth: Some(bearer()),
            content_length: Some(MAX_BODY_BYTES + 1),
            body: b"{}".to_vec(),
        };
        assert_eq!(
            handle_request(&lying, TOKEN),
            Outcome::Rejected(Rejection::PayloadTooLarge)
        );
    }

    #[test]
    fn wrong_method_and_path_are_rejected_once_authorized() {
        let body = br#"{"hook_event_name":"Stop"}"#;
        let mut get = parts(Some(&bearer()), body);
        get.method = "GET".to_string();
        assert_eq!(
            handle_request(&get, TOKEN),
            Outcome::Rejected(Rejection::MethodNotAllowed)
        );
        let mut wrong_path = parts(Some(&bearer()), body);
        wrong_path.path = "/events".to_string();
        assert_eq!(
            handle_request(&wrong_path, TOKEN),
            Outcome::Rejected(Rejection::NotFound)
        );
    }

    #[test]
    fn malformed_body_with_valid_token_is_bad_request() {
        assert_eq!(
            handle_request(&parts(Some(&bearer()), b"{ not json"), TOKEN),
            Outcome::Rejected(Rejection::BadRequest)
        );
        assert_eq!(
            handle_request(&parts(Some(&bearer()), br#"{"hook_event_name":"Nope"}"#), TOKEN),
            Outcome::Rejected(Rejection::BadRequest)
        );
    }

    #[test]
    fn rejection_status_codes_are_stable() {
        assert_eq!(Rejection::Unauthorized.status().0, 401);
        assert_eq!(Rejection::MethodNotAllowed.status().0, 405);
        assert_eq!(Rejection::NotFound.status().0, 404);
        assert_eq!(Rejection::PayloadTooLarge.status().0, 413);
        assert_eq!(Rejection::BadRequest.status().0, 400);
    }
}
