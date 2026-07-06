//! Agent-signal ingestion — the M2 interim status SOURCE, mapped at the
//! Harness seam into a pure-core event (task #11).
//!
//! Terax's in-tree OSC agent-signal parser (`modules::pty::agent_detect`)
//! turns hostile PTY bytes into a small, bounded set of signal *kinds*
//! (`started` / `working` / `attention` / `finished` / `exited`). This
//! function is the one place that maps such a kind to a pure-core
//! [`SessionSignal`]; from there `core::cut::session_status_from_signal` and
//! `core::cut::roll_up_status` derive the Workspace's live dot.
//!
//! An agent-signal is *data*: nothing here executes anything. The `kind` is
//! signal content and therefore hostile — an unknown or oversized kind is
//! ignored (`None`), never trusted, never acted on.
//!
//! # Signal -> event seam (the M3 swap point)
//!
//! This is the SOURCE side of the seam documented on
//! [`crate::modules::core::cut::SessionSignal`]. At M3 the control plane's
//! per-Workspace hooks replace THIS ingestion — they emit the same
//! `SessionSignal` per Session — while the pure-core reducer
//! (`session_status_from_signal` + `roll_up_status`) is untouched.
//! `agent_signal` then stays the Signal-only fallback for Harnesses without
//! the `control_plane_hooks` Cap.

use crate::modules::core::cut::SessionSignal;

/// Longest agent-signal `kind` this seam will even classify. Terax's
/// detector emits short fixed tokens; a longer kind is hostile or garbage
/// and maps to `None` so it can never drive status or cost work matching it.
pub const MAX_SIGNAL_KIND_LEN: usize = 32;

/// Map a Terax agent-signal `kind` to a pure-core [`SessionSignal`].
///
/// The accepted kinds are exactly the strings
/// `agent_detect::Transition::into_signal` emits. Every other kind — empty,
/// wrong case, oversized, or a raw OSC fragment — yields `None` and is
/// dropped. Pure and total: no side effect ever follows from the signal's
/// content.
pub fn ingest_agent_signal(kind: &str) -> Option<SessionSignal> {
    // Cap first: classify only bounded input, never a hostile blob.
    if kind.len() > MAX_SIGNAL_KIND_LEN {
        return None;
    }
    match kind {
        "started" => Some(SessionSignal::Started),
        "working" => Some(SessionSignal::Working),
        "attention" => Some(SessionSignal::Attention),
        "finished" => Some(SessionSignal::Finished),
        "exited" => Some(SessionSignal::Exited),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::cut::{session_status_from_signal, WorkspaceStatus};

    /// AC: the signal -> event mapping is unit-tested. These are exactly the
    /// `kind` strings `pty::agent_detect::Transition::into_signal` emits.
    #[test]
    fn maps_every_terax_transition_kind() {
        assert_eq!(ingest_agent_signal("started"), Some(SessionSignal::Started));
        assert_eq!(ingest_agent_signal("working"), Some(SessionSignal::Working));
        assert_eq!(
            ingest_agent_signal("attention"),
            Some(SessionSignal::Attention)
        );
        assert_eq!(
            ingest_agent_signal("finished"),
            Some(SessionSignal::Finished)
        );
        assert_eq!(ingest_agent_signal("exited"), Some(SessionSignal::Exited));
    }

    #[test]
    fn unknown_kinds_are_ignored_never_trusted() {
        for hostile in [
            "",
            "STARTED",
            "work",
            "notify;Terax;working",
            "133;C;claude",
            "\u{1b}]777;attention",
        ] {
            assert_eq!(
                ingest_agent_signal(hostile),
                None,
                "hostile kind {hostile:?} must be ignored"
            );
        }
    }

    #[test]
    fn oversized_kinds_are_dropped_before_matching() {
        let huge = "working".repeat(1000);
        assert_eq!(ingest_agent_signal(&huge), None);
        // An unknown kind exactly at the cap is still None; a known kind
        // well under the cap still maps.
        assert!(ingest_agent_signal(&"x".repeat(MAX_SIGNAL_KIND_LEN)).is_none());
        assert_eq!(
            ingest_agent_signal("finished"),
            Some(SessionSignal::Finished)
        );
    }

    /// The ingestion feeds the pure core directly, with no imperative shell
    /// in between: an ingested signal flows straight into the reducer.
    #[test]
    fn ingested_signal_feeds_the_pure_core_reducer() {
        let signal = ingest_agent_signal("attention").expect("known kind");
        assert_eq!(
            session_status_from_signal(signal),
            Some(WorkspaceStatus::Blocked)
        );
        // "exited" ingests to a signal that contributes no status.
        let exited = ingest_agent_signal("exited").expect("known kind");
        assert_eq!(session_status_from_signal(exited), None);
    }
}
