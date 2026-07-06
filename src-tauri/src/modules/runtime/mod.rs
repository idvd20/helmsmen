//! Helmsmen runtime layer (new module per docs/fork-posture.md, task #6).
//!
//! A Runtime is *where a process lives and what it survives*: LocalPty at
//! M1 (dies with the app), Tmux at M4 (survives quit). Orthogonal to
//! `modules::harness` (*what agent runs*). Frontend, status derivation,
//! and the approval queue are written against this trait, never a
//! concrete implementation.
//!
//! Security invariant (PRD): all output flowing through a Runtime is
//! hostile on every implementation. A Runtime moves bytes; it never
//! parses or acts on them. The conformance suite pins this.

pub mod answer;
pub mod commands;
pub mod local_pty;
pub mod spawn;

#[cfg(all(test, unix))]
mod conformance;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

/// What to start: argv (never a shell string), where, and with which env
/// on top of the app's own. Everything spawned in a Workspace carries the
/// cut's `HELMSMEN_*` set in `env` (assembled by `spawn::prepare_spawn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Absolute path the process starts in (the cut worktree).
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "code")]
pub enum SessionStatus {
    Running,
    Exited(i32),
}

pub type OnOutput = Box<dyn Fn(&[u8]) + Send + Sync>;
pub type OnExit = Box<dyn Fn(i32) + Send + Sync>;

/// Where a session's bytes go. `on_output` may be called from any thread;
/// `on_exit` is called exactly once per sink. Callbacks receive hostile
/// data and must treat it as such (ship it, never interpret it).
pub struct OutputSink {
    pub on_output: OnOutput,
    pub on_exit: OnExit,
}

/// Where a process lives. Any future implementation (Tmux at M4) must
/// pass the conformance suite in `conformance.rs`.
pub trait Runtime: Send + Sync {
    /// Start a session; returns its opaque id. Output streams to `sink`
    /// until exit; the session stays queryable (`status`) after exit.
    fn spawn(&self, spec: SpawnSpec, sink: OutputSink) -> Result<String, String>;

    /// Re-point a session's output at a new sink (webview reload). The
    /// new sink first receives the retained scrollback, then live output;
    /// if the session already exited, `on_exit` fires right after the
    /// scrollback.
    fn attach(&self, session: &str, sink: OutputSink) -> Result<(), String>;

    /// Type into the session. Bytes go to the process verbatim.
    fn write(&self, session: &str, bytes: &[u8]) -> Result<(), String>;

    /// A read-only snapshot of the session's CURRENT VISIBLE SCREEN — the M3.5
    /// `capture-pane` analog, generic over every Runtime. This is the rendered
    /// screen (a dialog drawn then cleared is gone; a queued dialog on top
    /// hides the one beneath), NOT session history, so the answering seam
    /// verifies the live dialog before injecting a key (verify-before-inject,
    /// user story 30). LocalPty reconstructs it from retained scrollback via a
    /// terminal-grid model; Tmux at M4 returns `tmux capture-pane` directly.
    /// Unlike [`attach`](Self::attach) it does NOT re-point the live sink, so
    /// it never disturbs the UI's stream. The conformance suite pins the
    /// behavior. (The bytes are still hostile — the caller strips them; the
    /// runtime does not interpret output for display, only renders the grid.)
    fn snapshot(&self, session: &str) -> Result<Vec<u8>, String>;

    fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), String>;

    fn status(&self, session: &str) -> Result<SessionStatus, String>;

    /// Terminate the session's process. Idempotent on an already-exited
    /// session; unknown ids error.
    fn kill(&self, session: &str) -> Result<(), String>;
}

/// The Runtimes this app instance offers, keyed by kind. Managed as Tauri
/// state; commands resolve a kind to `Arc<dyn Runtime>` so the glue stays
/// written against the trait.
pub struct RuntimeState {
    local_pty: Arc<local_pty::LocalPty>,
}

pub const LOCAL_PTY: &str = "local-pty";

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            local_pty: Arc::new(local_pty::LocalPty::default()),
        }
    }
}

impl RuntimeState {
    /// Resolve a runtime kind named across the IPC boundary; unknown
    /// kinds are rejected here, at the seam.
    pub fn get(&self, kind: &str) -> Result<Arc<dyn Runtime>, String> {
        match kind {
            LOCAL_PTY => Ok(self.local_pty.clone()),
            other => Err(format!("unknown runtime {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_state_resolves_local_pty_and_rejects_unknown_kinds() {
        let state = RuntimeState::default();
        assert!(state.get(LOCAL_PTY).is_ok());
        assert!(state.get("tmux").is_err(), "tmux arrives at M4");
        assert!(state.get("").is_err());
    }

    #[test]
    fn session_status_serializes_a_tagged_shape_for_the_frontend() {
        assert_eq!(
            serde_json::to_value(SessionStatus::Running).unwrap(),
            serde_json::json!({ "state": "running" })
        );
        assert_eq!(
            serde_json::to_value(SessionStatus::Exited(7)).unwrap(),
            serde_json::json!({ "state": "exited", "code": 7 })
        );
    }
}
