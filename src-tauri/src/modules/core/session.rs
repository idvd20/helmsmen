//! Session kinds as pure data (task #13).
//!
//! A Workspace hosts Sessions of a few kinds: an **Agent** (a Harness such
//! as claude-code), a **Shell** (the user's own terminal in the worktree),
//! and a **Process** (a Project Process definition — a dev server and the
//! like). Reviewer Sessions arrive with the approvals slice (later); they
//! are deliberately not modelled here yet.
//!
//! This is `data in -> data out` only: a `SessionKind` is stamped by the
//! backend when it *spawns* a Session (`modules::runtime`), so it is
//! `Serialize`-only — nothing deserializes a kind from stored data or an
//! IPC payload, exactly like [`crate::modules::core::cut::WorkspaceStatus`].
//! The Workspace status rollup that a Session feeds is itself derived and
//! never stored (see `core::cut::roll_up_status`); killing a Session simply
//! removes it from the live set the shell rolls up.

use serde::Serialize;

/// What kind of Session runs inside a Workspace. Serialize-only: produced
/// by the spawn seam, never accepted as input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    /// An Agent Session — a Harness launch (claude-code at M1). Its chip
    /// reads `{harness}·{runtime}`.
    Agent,
    /// A Shell Session — the user's own terminal in the worktree, carrying
    /// the cut's `HELMSMEN_*` env. Its chip reads `shell`.
    Shell,
    /// A Process Session — one of the Project's Process definitions run on
    /// demand in the worktree. Its chip reads `{name}:{port}` (or just
    /// `{name}` when the definition declares no port).
    Process,
}

impl SessionKind {
    /// Stable wire token, matching the frontend `HelmSessionKind` union.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Agent => "agent",
            SessionKind::Shell => "shell",
            SessionKind::Process => "process",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_serialize_to_the_frontend_wire_tokens() {
        assert_eq!(
            serde_json::to_value(SessionKind::Agent).unwrap(),
            serde_json::json!("agent")
        );
        assert_eq!(
            serde_json::to_value(SessionKind::Shell).unwrap(),
            serde_json::json!("shell")
        );
        assert_eq!(
            serde_json::to_value(SessionKind::Process).unwrap(),
            serde_json::json!("process")
        );
    }

    #[test]
    fn as_str_matches_the_serialized_token() {
        for kind in [SessionKind::Agent, SessionKind::Shell, SessionKind::Process] {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().to_string())
            );
        }
    }
}
