//! Tauri command glue for the control plane, deliberately thin (task #18).
//!
//! The frontend polls a Workspace's derived approval state so the reducer's
//! pending asks surface as ask cards on the wall and inline in the zoom, and so
//! a card can be reconciled after answering (post-hoc, by `tool_use_id`). This
//! is the read side; the write side (a hook POST changing state) is the
//! loopback endpoint, and answering is the runtime send-keys seam. This command
//! only reads already-derived state.

use tauri::State;

use crate::modules::core::control_plane::ControlPlaneState;

use super::EndpointRegistry;

/// A snapshot of the Workspace's derived control-plane state — approval cards,
/// warnings, and the per-decision audit records — or `None` when the Workspace
/// has no running endpoint (no control-plane Harness, or not cut yet).
#[tauri::command]
pub fn helm_approvals_snapshot(
    endpoints: State<'_, EndpointRegistry>,
    workspace_id: String,
) -> Option<ControlPlaneState> {
    endpoints.snapshot(&workspace_id)
}
