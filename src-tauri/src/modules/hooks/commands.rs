//! Tauri command glue for the control plane, deliberately thin (task #18).
//!
//! The frontend polls a Workspace's derived approval state so the reducer's
//! pending asks surface as ask cards on the wall and inline in the zoom, and so
//! a card can be reconciled after answering (post-hoc, by `tool_use_id`). This
//! is the read side; the write side (a hook POST changing state) is the
//! loopback endpoint, and answering is the runtime send-keys seam. This command
//! only reads already-derived state.

use serde::Deserialize;
use tauri::State;

use crate::modules::core::control_plane::{CardDecision, ControlPlaneState};

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

/// A bulk banner action (task #19) over the whole pending queue. Deserialized
/// from the frontend's `"allowAll"` / `"denyAll"`; maps to the [`CardDecision`]
/// each still-open ask is logged with.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BulkAction {
    AllowAll,
    DenyAll,
}

impl BulkAction {
    fn decision(self) -> CardDecision {
        match self {
            BulkAction::AllowAll => CardDecision::Allow,
            BulkAction::DenyAll => CardDecision::Deny,
        }
    }
}

/// Log a bulk banner decision (Allow all / Deny all) DISTINCTLY on a
/// Workspace's audit trail — one bulk-sourced approval record per still-open
/// ask. Returns how many records were appended (`0` when the Workspace has a
/// live endpoint but nothing is pending; `0` too when it has no endpoint). The
/// keys themselves are injected per card through the runtime `answer_prompt`
/// seam; this command only writes the distinct log.
#[tauri::command]
pub fn helm_record_bulk_decision(
    endpoints: State<'_, EndpointRegistry>,
    workspace_id: String,
    action: BulkAction,
) -> usize {
    endpoints
        .record_bulk_decision(&workspace_id, action.decision())
        .unwrap_or(0)
}
