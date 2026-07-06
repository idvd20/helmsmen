//! Tauri command glue for Agent Sessions, deliberately thin.
//!
//! The frontend names a Workspace and a Harness; everything else
//! (worktree path, env, launch command) is resolved backend-side, so the
//! webview never touches the OS, a process, or git. Session output flows
//! back over raw-byte channels and is hostile: the backend ships it
//! verbatim and never interprets it.

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, Response};
use tauri::State;

use crate::modules::core::session::SessionKind;
use crate::modules::harness::{self, IntendedDialog, PromptAnswer};
use crate::modules::registry::RegistryState;
use crate::modules::workspace::WorkspaceRegistry;

use super::answer::{answer_prompt, AnswerOutcome};
use super::spawn::{prepare_process_spawn, prepare_shell_spawn, prepare_spawn};
use super::{OutputSink, RuntimeState, SessionStatus, LOCAL_PTY};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnAgentInput {
    pub workspace_id: String,
    #[serde(default)]
    pub harness_id: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

/// The handle the frontend holds: opaque ids only, echoed back on every
/// session operation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionInfo {
    pub session_id: String,
    pub runtime: String,
    pub harness_id: String,
    pub workspace_id: String,
}

fn channel_sink(on_data: Channel<Response>, on_exit: Channel<i32>) -> OutputSink {
    OutputSink {
        on_output: Box::new(move |bytes| {
            let _ = on_data.send(Response::new(bytes.to_vec()));
        }),
        on_exit: Box::new(move |code| {
            let _ = on_exit.send(code);
        }),
    }
}

/// Spawn an Agent Session in a cut Workspace: Harness launch plan +
/// `HELMSMEN_*` env, running on the named Runtime (LocalPty by default).
#[tauri::command]
pub async fn helm_spawn_agent(
    registry: State<'_, RegistryState>,
    roots: State<'_, WorkspaceRegistry>,
    runtimes: State<'_, RuntimeState>,
    input: SpawnAgentInput,
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
) -> Result<AgentSessionInfo, String> {
    let harness_id = input.harness_id.as_deref().unwrap_or("claude-code").to_string();
    let runtime_kind = input.runtime.as_deref().unwrap_or(LOCAL_PTY).to_string();
    let runtime = runtimes.get(&runtime_kind)?;
    let spec = prepare_spawn(
        &registry,
        &roots,
        &input.workspace_id,
        &harness_id,
        input.cols.unwrap_or(120),
        input.rows.unwrap_or(32),
    )?;

    // openpty + fork block; keep them off the IPC thread like pty_open.
    let session_id = tauri::async_runtime::spawn_blocking(move || {
        runtime.spawn(spec, channel_sink(on_data, on_exit))
    })
    .await
    .map_err(|e| e.to_string())??;

    log::info!(
        "agent session {session_id} spawned (workspace={}, harness={harness_id}, runtime={runtime_kind})",
        input.workspace_id
    );
    Ok(AgentSessionInfo {
        session_id,
        runtime: runtime_kind,
        harness_id,
        workspace_id: input.workspace_id,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnShellInput {
    pub workspace_id: String,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

/// The handle for a spawned Shell Session — the user's own terminal in the
/// worktree. `kind` is always [`SessionKind::Shell`]; the frontend keys its
/// chip/tab off it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSessionInfo {
    pub session_id: String,
    pub runtime: String,
    pub workspace_id: String,
    pub kind: SessionKind,
}

/// Spawn a **Shell Session** in a cut Workspace: the user's own shell,
/// running in the worktree with the cut's `HELMSMEN_*` env, on the named
/// Runtime (LocalPty by default).
#[tauri::command]
pub async fn helm_spawn_shell(
    registry: State<'_, RegistryState>,
    roots: State<'_, WorkspaceRegistry>,
    runtimes: State<'_, RuntimeState>,
    input: SpawnShellInput,
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
) -> Result<ShellSessionInfo, String> {
    let runtime_kind = input.runtime.as_deref().unwrap_or(LOCAL_PTY).to_string();
    let runtime = runtimes.get(&runtime_kind)?;
    let spec = prepare_shell_spawn(
        &registry,
        &roots,
        &input.workspace_id,
        input.cols.unwrap_or(120),
        input.rows.unwrap_or(32),
    )?;

    let session_id = tauri::async_runtime::spawn_blocking(move || {
        runtime.spawn(spec, channel_sink(on_data, on_exit))
    })
    .await
    .map_err(|e| e.to_string())??;

    log::info!(
        "shell session {session_id} spawned (workspace={}, runtime={runtime_kind})",
        input.workspace_id
    );
    Ok(ShellSessionInfo {
        session_id,
        runtime: runtime_kind,
        workspace_id: input.workspace_id,
        kind: SessionKind::Shell,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnProcessInput {
    pub workspace_id: String,
    /// Name of one of the Project's Process definitions (`dev`, `db`).
    pub process_name: String,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

/// The handle for a spawned Process Session. Carries the Process's name and
/// declared port so the frontend can render its chip (`dev:5173`) without
/// parsing the (hostile) process output.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSessionInfo {
    pub session_id: String,
    pub runtime: String,
    pub workspace_id: String,
    pub kind: SessionKind,
    pub process_name: String,
    pub port: Option<u16>,
}

/// Spawn a **Process Session** in a cut Workspace: one of the Project's
/// Process definitions, resolved by name backend-side (the frontend names a
/// definition, never a command), run in the worktree with the `HELMSMEN_*`
/// env.
#[tauri::command]
pub async fn helm_spawn_process(
    registry: State<'_, RegistryState>,
    roots: State<'_, WorkspaceRegistry>,
    runtimes: State<'_, RuntimeState>,
    input: SpawnProcessInput,
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
) -> Result<ProcessSessionInfo, String> {
    let runtime_kind = input.runtime.as_deref().unwrap_or(LOCAL_PTY).to_string();
    let runtime = runtimes.get(&runtime_kind)?;
    let (spec, def) = prepare_process_spawn(
        &registry,
        &roots,
        &input.workspace_id,
        &input.process_name,
        input.cols.unwrap_or(120),
        input.rows.unwrap_or(32),
    )?;

    let session_id = tauri::async_runtime::spawn_blocking(move || {
        runtime.spawn(spec, channel_sink(on_data, on_exit))
    })
    .await
    .map_err(|e| e.to_string())??;

    log::info!(
        "process session {session_id} spawned (workspace={}, process={}, runtime={runtime_kind})",
        input.workspace_id,
        def.name
    );
    Ok(ProcessSessionInfo {
        session_id,
        runtime: runtime_kind,
        workspace_id: input.workspace_id,
        kind: SessionKind::Process,
        process_name: def.name,
        port: def.port,
    })
}

/// Re-point a session's output at fresh channels (webview reload). The
/// new channel first receives the retained scrollback, then live output.
#[tauri::command]
pub fn helm_attach_agent(
    runtimes: State<'_, RuntimeState>,
    runtime: Option<String>,
    session: String,
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
) -> Result<(), String> {
    runtimes
        .get(runtime.as_deref().unwrap_or(LOCAL_PTY))?
        .attach(&session, channel_sink(on_data, on_exit))
}

/// Type into a session. Dev-console volume; the latency-critical raw-body
/// path (see pty_write) can arrive with the zoom view if it's ever felt.
#[tauri::command]
pub fn helm_write_agent(
    runtimes: State<'_, RuntimeState>,
    runtime: Option<String>,
    session: String,
    data: String,
) -> Result<(), String> {
    runtimes
        .get(runtime.as_deref().unwrap_or(LOCAL_PTY))?
        .write(&session, data.as_bytes())
}

#[tauri::command]
pub fn helm_resize_agent(
    runtimes: State<'_, RuntimeState>,
    runtime: Option<String>,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    runtimes
        .get(runtime.as_deref().unwrap_or(LOCAL_PTY))?
        .resize(&session, cols, rows)
}

#[tauri::command]
pub fn helm_agent_status(
    runtimes: State<'_, RuntimeState>,
    runtime: Option<String>,
    session: String,
) -> Result<SessionStatus, String> {
    runtimes
        .get(runtime.as_deref().unwrap_or(LOCAL_PTY))?
        .status(&session)
}

#[tauri::command]
pub fn helm_kill_agent(
    runtimes: State<'_, RuntimeState>,
    runtime: Option<String>,
    session: String,
) -> Result<(), String> {
    runtimes
        .get(runtime.as_deref().unwrap_or(LOCAL_PTY))?
        .kill(&session)
}

/// Whether the user Allowed or Denied a paused approval.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerAction {
    Allow,
    Deny,
}

/// Input to the ONE send-keys seam. The frontend names the agent session and
/// the card being answered — `expectedCommand` (the exact command the card
/// showed) is what the backend must see on screen before injecting any key,
/// and `toolUseId` is the correlation anchor for post-hoc reconciliation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerPromptInput {
    /// The agent Session's opaque runtime id (keys inject here).
    pub session: String,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub harness_id: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The exact command/file the card showed; must be visible on screen.
    pub expected_command: String,
    pub action: AnswerAction,
    /// Deny reroute instruction (lands as a user message). Ignored for Allow.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Answer a paused approval by injecting keys into the agent's PTY — the ONE
/// fragile seam. Snapshots the session, verifies the visible dialog is the
/// intended card's, and injects the accept/deny key sequence only on a match
/// (a mismatch injects nothing). Runs off the IPC thread because the deny
/// sequence carries settle delays.
#[tauri::command]
pub async fn helm_answer_prompt(
    runtimes: State<'_, RuntimeState>,
    input: AnswerPromptInput,
) -> Result<AnswerOutcome, String> {
    let runtime = runtimes.get(input.runtime.as_deref().unwrap_or(LOCAL_PTY))?;
    let harness_id = input.harness_id.unwrap_or_else(|| "claude-code".to_string());
    let harness =
        harness::by_id(&harness_id).ok_or_else(|| format!("unknown harness {harness_id:?}"))?;
    let answer = match input.action {
        AnswerAction::Allow => PromptAnswer::Allow,
        AnswerAction::Deny => PromptAnswer::Deny {
            reason: input.reason.unwrap_or_default(),
        },
    };
    let session = input.session;
    let expected = input.expected_command;
    let tool_use_id = input.tool_use_id;

    tauri::async_runtime::spawn_blocking(move || {
        let dialog = IntendedDialog {
            tool_use_id: tool_use_id.as_deref(),
            expected_command: &expected,
        };
        answer_prompt(runtime.as_ref(), harness, &session, &dialog, &answer, |ms| {
            std::thread::sleep(std::time::Duration::from_millis(ms))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
