//! The per-Workspace control-plane endpoint (task #15): a loopback-only
//! HTTP listener with a per-session bearer token.
//!
//! This is the imperative shell. It binds `127.0.0.1:0` (loopback only —
//! never a routable interface), mints a 256-bit bearer token, reads a hook
//! POST off the socket under a size cap, and hands the raw bytes to the pure
//! [`wire::handle_request`] boundary. An accepted event is folded into the
//! pure-core reducer ([`apply_hook_event`]); a rejected one becomes a terse
//! HTTP error. An event may change state — it never executes anything.
//!
//! # Wiring seam (task #16)
//!
//! The endpoint exposes [`ControlPlaneEndpoint::port`],
//! [`ControlPlaneEndpoint::token`], and [`ControlPlaneEndpoint::url`]. Task
//! #16 reads these to write the Claude Code hook config into the cut
//! worktree (the config points every hook at `url` with the bearer token).
//! This slice deliberately does **not** write any hook config itself.
//!
//! # No new dependency
//!
//! The server is built on `std::net` blocking sockets, so it adds no crate
//! and stays clear of `cargo-deny`'s license gate and the upstream HTTP
//! stack. See `docs/fork-posture.md`.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::modules::core::control_plane::{
    apply_hook_event, ControlPlaneState, HookEvent, HookEventKind,
};
use crate::modules::core::policy::{decide, PolicyContext};

use super::wire::{
    handle_request, pretooluse_permission_json, Outcome, RequestParts, HOOK_PATH, MAX_BODY_BYTES,
};

/// Cap on the request head (request line + headers). Hook requests have a
/// handful of small headers; anything larger is refused before the body.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Per-connection socket timeout — a client that stalls mid-request is
/// dropped rather than tying up a handler.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the accept loop wakes to check the shutdown flag.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// A running control-plane endpoint bound to loopback. Dropping it (or
/// calling [`shutdown`](Self::shutdown)) stops the accept loop.
pub struct ControlPlaneEndpoint {
    port: u16,
    token: String,
    state: Arc<Mutex<ControlPlaneState>>,
    running: Arc<AtomicBool>,
}

impl ControlPlaneEndpoint {
    /// Bind a fresh loopback endpoint whose policy has no known worktree root
    /// (destructive-fs decisions then fail safe to an ask). Used by tests and
    /// callers that do not scope to a Workspace; prefer [`start_in`] at cut.
    ///
    /// [`start_in`]: Self::start_in
    pub fn start() -> io::Result<Self> {
        Self::start_in("")
    }

    /// Bind a fresh loopback endpoint bound to a Workspace's TRUSTED worktree
    /// root, and start serving in the background. The root anchors the pure
    /// policy's "destructive fs outside the worktree" rule; the home directory
    /// is read from the process environment (the shell), never from a payload.
    pub fn start_in(workspace_root: impl Into<String>) -> io::Result<Self> {
        // Loopback only: bind 127.0.0.1, never 0.0.0.0. Port 0 = an
        // OS-assigned ephemeral port.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = generate_token();
        let state = Arc::new(Mutex::new(ControlPlaneState::default()));
        let running = Arc::new(AtomicBool::new(true));
        let policy = Arc::new(PolicyContext::new(workspace_root, home_dir()));

        let seq = Arc::new(AtomicU64::new(0));
        let loop_token = token.clone();
        let loop_state = Arc::clone(&state);
        let loop_running = Arc::clone(&running);
        let loop_policy = Arc::clone(&policy);
        std::thread::Builder::new()
            .name("helmsmen-control-plane".to_string())
            .spawn(move || {
                accept_loop(listener, loop_token, loop_state, seq, loop_running, loop_policy)
            })?;

        Ok(Self {
            port,
            token,
            state,
            running,
        })
    }

    /// The loopback port the hook config should POST to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The per-session bearer token the hook must present.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Full loopback URL for the hook config (task #16 consumes this).
    pub fn url(&self) -> String {
        format!("http://{}:{}{}", Ipv4Addr::LOCALHOST, self.port, HOOK_PATH)
    }

    /// A snapshot of the derived control-plane state (approval cards +
    /// warnings). The frontend mirror and tests read this.
    pub fn snapshot(&self) -> ControlPlaneState {
        self.state
            .lock()
            .expect("control-plane state mutex poisoned")
            .clone()
    }

    /// Stop serving. Idempotent.
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for ControlPlaneEndpoint {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(
    listener: TcpListener,
    token: String,
    state: Arc<Mutex<ControlPlaneState>>,
    seq: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    policy: Arc<PolicyContext>,
) {
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _addr)) => {
                let token = token.clone();
                let state = Arc::clone(&state);
                let seq = Arc::clone(&seq);
                let policy = Arc::clone(&policy);
                // A stalled or hostile connection is isolated on its own
                // handler; it cannot block the accept loop.
                let spawned = std::thread::Builder::new()
                    .name("helmsmen-control-plane-conn".to_string())
                    .spawn(move || {
                        let _ = handle_connection(stream, &token, &state, &seq, &policy);
                    });
                let _ = spawned;
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(_) => std::thread::sleep(ACCEPT_POLL),
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    token: &str,
    state: &Mutex<ControlPlaneState>,
    seq: &AtomicU64,
    policy: &PolicyContext,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;

    let parts = read_request(&mut stream)?;
    let (code, reason, body) = match handle_request(&parts, token) {
        Outcome::Accepted { session_id, kind } => {
            // Server-assigned monotonic sequence — stable card ids and
            // warning provenance without trusting any client-sent counter.
            let n = seq.fetch_add(1, Ordering::SeqCst) + 1;
            // The PreToolUse relay is SYNCHRONOUS: run the user-level policy
            // and return the decision as the hook's response so Claude Code
            // enforces it (ask pauses, deny blocks, allow proceeds). Every
            // other event stays fire-and-forget.
            let body = match &kind {
                HookEventKind::PreToolUse {
                    tool_name, input, ..
                } => pretooluse_permission_json(&decide(tool_name, input, policy)),
                _ => String::from("{\"ok\":true}"),
            };
            let event = HookEvent::new(n, session_id, kind);
            let mut guard = state.lock().expect("control-plane state mutex poisoned");
            let current = std::mem::take(&mut *guard);
            *guard = apply_hook_event(current, event, policy);
            (200u16, "OK", body)
        }
        Outcome::Rejected(rejection) => {
            let (code, reason) = rejection.status();
            (code, reason, format!("{{\"error\":\"{reason}\"}}"))
        }
    };
    write_response(&mut stream, code, reason, &body)
}

/// Read one HTTP request off the socket into typed parts, capping both the
/// header block and the body so a hostile client cannot exhaust memory.
fn read_request(stream: &mut TcpStream) -> io::Result<RequestParts> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    // Read until the end-of-headers marker (or the header cap).
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers too large",
            ));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            // Peer closed before a full header block; parse what we have
            // (it will fall through to a 400/404/401).
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut body = buf.get(header_end + 4..).map(<[u8]>::to_vec).unwrap_or_default();

    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("").to_string();
    let path = request_parts.next().unwrap_or("").to_string();

    let mut auth = None;
    let mut content_length = None;
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            match key.trim().to_ascii_lowercase().as_str() {
                "authorization" => auth = Some(value.trim().to_string()),
                "content-length" => content_length = value.trim().parse::<usize>().ok(),
                _ => {}
            }
        }
    }

    // Read the remaining body up to the declared length, hard-capped one
    // byte past the limit so `handle_request` can still detect an overflow.
    let cap = MAX_BODY_BYTES + 1;
    let target = content_length.unwrap_or(0).min(cap);
    while body.len() < target {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > cap {
            break;
        }
    }

    Ok(RequestParts {
        method,
        path,
        auth,
        content_length,
        body,
    })
}

fn write_response(stream: &mut TcpStream, code: u16, reason: &str, body: &str) -> io::Result<()> {
    let auth_challenge = if code == 401 {
        "WWW-Authenticate: Bearer\r\n"
    } else {
        ""
    };
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         {auth_challenge}\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The user's home directory, read from the process environment (the trusted
/// shell) — never from a hook payload. Anchors the policy's `rm $HOME` and
/// `~/.ssh` hard-deny checks. Empty if unset (the checks then match only the
/// textual `~` / `$HOME` forms).
fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default()
}

/// Mint a 256-bit bearer token, hex-encoded. Prefers OS randomness
/// (`/dev/urandom`); a degraded time/pid mix is the fallback if that is
/// unavailable (documented, and not reached on the macOS/Linux targets).
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    if fill_os_random(&mut bytes).is_err() {
        fill_fallback_random(&mut bytes);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    token
}

#[cfg(unix)]
fn fill_os_random(bytes: &mut [u8]) -> io::Result<()> {
    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(bytes)
}

#[cfg(not(unix))]
fn fill_os_random(_bytes: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no OS random source",
    ))
}

fn fill_fallback_random(bytes: &mut [u8]) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    seed ^= u64::from(std::process::id());
    seed ^= bytes.as_ptr() as usize as u64;
    for byte in bytes.iter_mut() {
        // xorshift64 — spreads the seed; only a fallback for a missing OS
        // random source.
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = (seed & 0xff) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::control_plane::CardStatus;

    /// Minimal raw-HTTP client: POST `body` to the loopback endpoint and
    /// return the response status code. Proves the full wire path with no
    /// live agent.
    fn http_post(port: u16, auth: Option<&str>, body: &str) -> u16 {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        let auth_line = auth
            .map(|a| format!("Authorization: {a}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /hook HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             {auth_line}\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let text = String::from_utf8_lossy(&response);
        let status_line = text.lines().next().unwrap_or("");
        status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(0)
    }

    /// Like [`http_post`] but returns the response body (the JSON the
    /// PreToolUse relay hands back to Claude Code).
    fn http_post_body(port: u16, auth: Option<&str>, body: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(SOCKET_TIMEOUT)).unwrap();
        let auth_line = auth
            .map(|a| format!("Authorization: {a}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth_line}\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let text = String::from_utf8_lossy(&response);
        // Body is everything after the blank line ending the headers.
        text.split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default()
    }

    fn bearer(endpoint: &ControlPlaneEndpoint) -> String {
        format!("Bearer {}", endpoint.token())
    }

    #[test]
    fn binds_loopback_and_mints_a_nonempty_token() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        assert_ne!(endpoint.port(), 0);
        // 32 bytes hex-encoded.
        assert_eq!(endpoint.token().len(), 64);
        assert!(endpoint.token().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(endpoint.url().starts_with("http://127.0.0.1:"));
        assert!(endpoint.url().ends_with("/hook"));
    }

    #[test]
    fn each_endpoint_gets_a_distinct_token() {
        let a = ControlPlaneEndpoint::start().unwrap();
        let b = ControlPlaneEndpoint::start().unwrap();
        assert_ne!(a.token(), b.token(), "per-session tokens must differ");
    }

    #[test]
    fn a_valid_pretooluse_post_renders_a_pending_card() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let body = r#"{"session_id":"s1","hook_event_name":"PreToolUse",
            "tool_name":"Bash","tool_use_id":"toolu_x","tool_input":{"command":"ls"}}"#;
        assert_eq!(http_post(endpoint.port(), Some(&bearer(&endpoint)), body), 200);

        let state = endpoint.snapshot();
        assert_eq!(state.cards.len(), 1);
        assert_eq!(state.cards[0].status, CardStatus::Pending);
        assert_eq!(state.cards[0].tool_name, "Bash");
        assert!(state.warnings.is_empty());
    }

    #[test]
    fn missing_and_bad_tokens_are_rejected_and_change_no_state() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let body = r#"{"session_id":"s1","hook_event_name":"PreToolUse",
            "tool_name":"Bash","tool_use_id":"toolu_x"}"#;
        assert_eq!(http_post(endpoint.port(), None, body), 401);
        assert_eq!(
            http_post(endpoint.port(), Some("Bearer deadbeef"), body),
            401
        );
        // A process that finds the port but not the token injects nothing.
        assert!(endpoint.snapshot().cards.is_empty());
    }

    #[test]
    fn an_oversized_payload_is_rejected_and_changes_no_state() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let mut body = String::from(r#"{"session_id":"s1","hook_event_name":"Stop","pad":""#);
        body.push_str(&"A".repeat(MAX_BODY_BYTES + 100));
        body.push_str("\"}");
        assert_eq!(
            http_post(endpoint.port(), Some(&bearer(&endpoint)), &body),
            413
        );
        assert!(endpoint.snapshot().cards.is_empty());
        assert!(endpoint.snapshot().warnings.is_empty());
    }

    // --- Test seam 1: the spike corpus, POSTed at the real endpoint ---

    /// The exact 14 hook payloads captured in
    /// `spike-approval-loop/events.jsonl` (inner `payload` objects). Driving
    /// them through the live loopback endpoint proves everything from hook
    /// POST to rendered card — with no live agent.
    const SPIKE_CORPUS: &[&str] = &[
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_0147xSUu5zjYeq1oRrbYL8Bo","tool_input":{"command":"git log --oneline -3"}}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"toolu_0147xSUu5zjYeq1oRrbYL8Bo"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Stop","stop_hook_active":false}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_01TMs4yARv9nEizBk5cac5RE","tool_input":{"command":"git status"}}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Stop","stop_hook_active":false}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Notification","notification_type":"idle_prompt","message":"Claude is waiting for your input"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_01Dvw7DqDGE3pjV6KsWPFNiU","tool_input":{"command":"git status"}}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_01GBXYZ17dmzAZ8pw66RhauK","tool_input":{"command":"git diff --stat"}}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"toolu_01GBXYZ17dmzAZ8pw66RhauK"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"toolu_01Dvw7DqDGE3pjV6KsWPFNiU"}"#,
        r#"{"session_id":"bb6de6a5-789a-4bcf-97cf-2eca27d74234","hook_event_name":"Stop","stop_hook_active":false}"#,
    ];

    #[test]
    fn spike_corpus_posts_to_the_expected_cards_with_zero_warnings() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let auth = bearer(&endpoint);
        for payload in SPIKE_CORPUS {
            assert_eq!(
                http_post(endpoint.port(), Some(&auth), payload),
                200,
                "every well-formed hook POST is accepted"
            );
        }

        let state = endpoint.snapshot();
        assert_eq!(state.event_count, 14);
        assert_eq!(state.cards.len(), 4);

        let status_of = |tuid: &str| {
            state
                .cards
                .iter()
                .find(|c| c.tool_use_id.as_deref() == Some(tuid))
                .unwrap_or_else(|| panic!("no card for {tuid}"))
                .status
        };
        assert_eq!(
            status_of("toolu_0147xSUu5zjYeq1oRrbYL8Bo"),
            CardStatus::Allowed
        );
        assert_eq!(
            status_of("toolu_01TMs4yARv9nEizBk5cac5RE"),
            CardStatus::ClosedNoRun
        );
        assert_eq!(
            status_of("toolu_01Dvw7DqDGE3pjV6KsWPFNiU"),
            CardStatus::Allowed
        );
        assert_eq!(
            status_of("toolu_01GBXYZ17dmzAZ8pw66RhauK"),
            CardStatus::Allowed
        );
        assert!(
            state.warnings.is_empty(),
            "zero warnings across the corpus is the pass signal, got: {:?}",
            state.warnings
        );
    }

    #[test]
    fn duplicate_posts_do_not_corrupt_state() {
        // Replay the whole corpus twice at the endpoint: dedup is by
        // tool_use_id, so the cards are identical and no warning appears.
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let auth = bearer(&endpoint);
        for payload in SPIKE_CORPUS.iter().chain(SPIKE_CORPUS.iter()) {
            assert_eq!(http_post(endpoint.port(), Some(&auth), payload), 200);
        }
        let state = endpoint.snapshot();
        assert_eq!(state.cards.len(), 4, "replay must not duplicate cards");
        assert_eq!(state.event_count, 28);
        assert!(state.warnings.is_empty());
    }

    // --- Test seam: the decision relay (policy -> Claude Code) at the wire ---

    /// A PreToolUse POST for a risk-list call comes back with a Claude Code
    /// `ask` decision AND lands a paused ask card carrying the rule + exact
    /// command — the whole "risk call pauses with an ask block" AC, at the
    /// primary seam, with no live agent.
    #[test]
    fn a_risk_pretooluse_returns_ask_and_lands_a_paused_ask_card() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let auth = bearer(&endpoint);
        let body = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_use_id":"toolu_fp","tool_input":{"command":"git push --force origin main"}}"#;

        let resp = http_post_body(endpoint.port(), Some(&auth), body);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "ask");

        let state = endpoint.snapshot();
        let card = &state.cards[0];
        assert_eq!(card.status, CardStatus::Pending);
        assert_eq!(
            card.rule.as_ref().map(|r| r.id.as_str()),
            Some("git-history-rewrite")
        );
        assert_eq!(
            card.input.command.as_deref(),
            Some("git push --force origin main")
        );
        // Every decision writes a record.
        assert_eq!(state.records.len(), 1);
        assert_eq!(state.records[0].input.command.as_deref(), Some("git push --force origin main"));
    }

    /// A hard-deny call comes back with a Claude Code `deny` decision (so the
    /// tool never runs) and is recorded as denied — robust via the hook
    /// return, regardless of prompt layout.
    #[test]
    fn a_hard_deny_pretooluse_returns_deny_and_is_recorded() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let auth = bearer(&endpoint);
        let body = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_use_id":"toolu_sudo","tool_input":{"command":"sudo rm -rf /var"}}"#;

        let resp = http_post_body(endpoint.port(), Some(&auth), body);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");

        let state = endpoint.snapshot();
        assert_eq!(
            state.records[0].rule.as_ref().map(|r| r.id.as_str()),
            Some("hard-deny-sudo")
        );
    }

    /// An ordinary in-worktree call comes back `allow` (permissive
    /// in-worktree) and is recorded.
    #[test]
    fn an_ordinary_pretooluse_returns_allow() {
        let endpoint = ControlPlaneEndpoint::start().unwrap();
        let auth = bearer(&endpoint);
        let body = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_use_id":"toolu_ok","tool_input":{"command":"git status"}}"#;
        let resp = http_post_body(endpoint.port(), Some(&auth), body);
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(endpoint.snapshot().records[0].decision, crate::modules::core::control_plane::CardDecision::Allow);
    }

    /// The destructive-fs rule is decidable because the endpoint is bound to a
    /// trusted worktree root: an in-tree `rm` is allowed (free), the same op
    /// escaping the root asks.
    #[test]
    fn destructive_fs_is_scoped_to_the_bound_worktree_root() {
        let root = std::env::temp_dir().join("helmsmen-wt-test");
        let endpoint = ControlPlaneEndpoint::start_in(root.to_string_lossy()).unwrap();
        let auth = bearer(&endpoint);

        let in_tree = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_use_id":"toolu_in","tool_input":{"command":"rm -rf build/output"}}"#;
        let escape = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_use_id":"toolu_out","tool_input":{"command":"rm -rf ../../etc/hosts"}}"#;

        let a: serde_json::Value =
            serde_json::from_str(&http_post_body(endpoint.port(), Some(&auth), in_tree)).unwrap();
        assert_eq!(a["hookSpecificOutput"]["permissionDecision"], "allow");
        let b: serde_json::Value =
            serde_json::from_str(&http_post_body(endpoint.port(), Some(&auth), escape)).unwrap();
        assert_eq!(b["hookSpecificOutput"]["permissionDecision"], "ask");
    }
}
