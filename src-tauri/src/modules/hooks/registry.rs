//! Per-Workspace control-plane endpoint registry (task #16) — the imperative
//! shell that keeps each Workspace's loopback endpoint alive.
//!
//! The cut pipeline starts one [`ControlPlaneEndpoint`] per Workspace (when
//! its Harness declares the `control_plane_hooks` Cap) and stashes it here,
//! keyed by Workspace id. The endpoint's accept loop runs on its own thread
//! and stops when the endpoint is dropped, so ownership lives exactly as long
//! as the entry does: it is created at cut, and removed when the Workspace is
//! removed (or when a cut is scuttled mid-flight). Managed as Tauri state so
//! every Session in a Workspace — the cut's first one and any later spawn —
//! shares the same loopback port and token.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::modules::core::control_plane::ControlPlaneState;

use super::server::ControlPlaneEndpoint;

/// Live control-plane endpoints, one per Workspace. Thread-safe; cloned
/// `Arc`s hand a caller a stable handle to the endpoint even across a
/// concurrent [`remove`](Self::remove).
#[derive(Default)]
pub struct EndpointRegistry {
    endpoints: Mutex<HashMap<String, Arc<ControlPlaneEndpoint>>>,
}

impl EndpointRegistry {
    /// Start-or-get the Workspace's endpoint, bound to its TRUSTED worktree
    /// root (the anchor for the policy's destructive-fs rule). Idempotent: a
    /// second call for a Workspace that already has a live endpoint returns
    /// the same one (same port, token, and policy root), so re-entering the
    /// wiring step never orphans a listener or rebinds the policy. Only a
    /// first, failing `bind` surfaces an error.
    pub fn start_for(
        &self,
        workspace_id: &str,
        workspace_root: &str,
    ) -> io::Result<Arc<ControlPlaneEndpoint>> {
        let mut map = self.lock();
        if let Some(existing) = map.get(workspace_id) {
            return Ok(Arc::clone(existing));
        }
        let endpoint = Arc::new(ControlPlaneEndpoint::start_in(workspace_root)?);
        map.insert(workspace_id.to_string(), Arc::clone(&endpoint));
        Ok(endpoint)
    }

    /// The Workspace's live endpoint, if one is running.
    pub fn get(&self, workspace_id: &str) -> Option<Arc<ControlPlaneEndpoint>> {
        self.lock().get(workspace_id).map(Arc::clone)
    }

    /// Drop the Workspace's endpoint, stopping its accept loop once the last
    /// handle is released. Idempotent — removing an absent id is a no-op — so
    /// it is safe to call on every Workspace removal.
    pub fn remove(&self, workspace_id: &str) {
        self.lock().remove(workspace_id);
    }

    /// A snapshot of the Workspace's derived control-plane state (approval
    /// cards + warnings), or `None` if it has no endpoint.
    pub fn snapshot(&self, workspace_id: &str) -> Option<ControlPlaneState> {
        self.get(workspace_id).map(|e| e.snapshot())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<ControlPlaneEndpoint>>> {
        self.endpoints
            .lock()
            .expect("control-plane endpoint registry mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_for_is_idempotent_per_workspace() {
        let reg = EndpointRegistry::default();
        let a = reg.start_for("ws-1", "/tmp/ws-1").unwrap();
        let b = reg.start_for("ws-1", "/tmp/ws-1").unwrap();
        // Same endpoint: same port and token, no second listener bound.
        assert_eq!(a.port(), b.port());
        assert_eq!(a.token(), b.token());
    }

    #[test]
    fn distinct_workspaces_get_distinct_endpoints() {
        let reg = EndpointRegistry::default();
        let a = reg.start_for("ws-1", "/tmp/ws-1").unwrap();
        let b = reg.start_for("ws-2", "/tmp/ws-2").unwrap();
        assert_ne!(a.port(), b.port(), "each Workspace binds its own port");
        assert_ne!(a.token(), b.token(), "per-session tokens must differ");
    }

    #[test]
    fn get_returns_the_started_endpoint_and_none_otherwise() {
        let reg = EndpointRegistry::default();
        assert!(reg.get("ws-1").is_none());
        let started = reg.start_for("ws-1", "/tmp/ws-1").unwrap();
        assert_eq!(reg.get("ws-1").unwrap().port(), started.port());
    }

    #[test]
    fn remove_stops_serving_and_is_idempotent() {
        let reg = EndpointRegistry::default();
        reg.start_for("ws-1", "/tmp/ws-1").unwrap();
        reg.remove("ws-1");
        assert!(reg.get("ws-1").is_none());
        // Removing an absent id is a no-op, never a panic.
        reg.remove("ws-1");
        reg.remove("never-existed");
    }

    #[test]
    fn snapshot_reads_the_endpoint_state() {
        let reg = EndpointRegistry::default();
        assert!(reg.snapshot("ws-1").is_none());
        reg.start_for("ws-1", "/tmp/ws-1").unwrap();
        let snap = reg.snapshot("ws-1").unwrap();
        assert!(snap.cards.is_empty(), "a fresh endpoint has no cards");
    }
}
