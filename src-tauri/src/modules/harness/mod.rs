//! Helmsmen harness layer (new module per docs/fork-posture.md, task #6).
//!
//! A Harness is *what agent runs and how Helmsmen talks to it*: launch
//! command, capability declaration, config-injection seam. Orthogonal to
//! `modules::runtime` (*where the process lives*). Frontend, status
//! derivation, and the approval queue are written against this trait; a
//! missing Cap switches off its UI surface, never the architecture.
//!
//! Everything in this module is pure data-in/data-out: composing a launch
//! plan or a config file set touches no filesystem, process, or settings
//! store. Applying the plan is the imperative shell's job
//! (`modules::runtime::spawn`). A guard test below enforces this.

pub mod claude_code;
pub mod commands;

use std::collections::BTreeMap;

use serde::Serialize;

/// What a Harness can do, declared in code at compile time.
///
/// Caps are code, never configuration: this type is Serialize-only (no
/// data can ever deserialize into a Caps), and nothing in this module
/// reads settings, so no file or IPC payload can ever grant a
/// capability. A missing Cap degrades the UI surface it powers, nothing
/// else. Guard-tested below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Caps {
    /// Sessions can be resumed after a restart (`--resume`, M6).
    pub resume: bool,
    /// Per-worktree hook wiring into the control plane (M3/M3.5).
    pub control_plane_hooks: bool,
    /// Emits the in-tree OSC agent-signal used for early status (M2).
    pub agent_signal: bool,
    /// Token/cost telemetry per session (transcript JSONL, M6).
    pub cost_telemetry: bool,
    /// MCP server set composable at launch (M6).
    pub mcp_config: bool,
    /// Model selectable on the launch command.
    pub model_select: bool,
}

/// Everything a Harness may consult when composing its launch plan or
/// config injection. Borrowed views only; the Harness cannot mutate the
/// Workspace through this.
pub struct LaunchContext<'a> {
    /// Canonical worktree path of the cut Workspace.
    pub workspace_root: &'a str,
    /// The Workspace's `HELMSMEN_*` env set.
    pub env: &'a BTreeMap<String, String>,
}

/// The command a Runtime should spawn: argv only, never a shell string, so
/// no Workspace-derived value can be reinterpreted by a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
}

/// One file the Harness wants inside the worktree before launch (M3 writes
/// control-plane hook wiring through this seam). `rel_path` is
/// worktree-relative; the imperative shell rejects absolute paths and
/// `..` before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    pub rel_path: String,
    pub contents: String,
}

/// What agent runs and how Helmsmen talks to it.
pub trait Harness: Send + Sync {
    /// Stable identifier (`"claude-code"`); registry and frontend key.
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// Capability declaration, in code. See [`Caps`].
    fn caps(&self) -> Caps;
    /// The command that starts an interactive Agent Session.
    fn launch_plan(&self, ctx: &LaunchContext) -> LaunchPlan;
    /// Config-injection seam: files to place in the worktree before
    /// launch. Empty means nothing to write (claude-code at M1; hook
    /// wiring arrives at M3).
    fn config_injection(&self, ctx: &LaunchContext) -> Vec<ConfigFile>;
}

/// Every built-in Harness. `byoa` (bring-your-own-agent) joins post-M6.
pub fn all() -> &'static [&'static dyn Harness] {
    static ALL: [&dyn Harness; 1] = [&claude_code::ClaudeCode];
    &ALL
}

pub fn by_id(id: &str) -> Option<&'static dyn Harness> {
    all().iter().copied().find(|h| h.id() == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn by_id_resolves_claude_code_and_rejects_unknown() {
        assert_eq!(by_id("claude-code").map(|h| h.id()), Some("claude-code"));
        assert!(by_id("ghost").is_none());
        assert!(by_id("").is_none());
    }

    #[test]
    fn every_registered_harness_has_a_unique_nonempty_id() {
        let mut ids: Vec<&str> = all().iter().map(|h| h.id()).collect();
        assert!(ids.iter().all(|id| !id.is_empty()));
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all().len(), "harness ids must be unique");
    }

    /// AC guard: no Cap is settable from settings. Production code in the
    /// harness module must never deserialize anything, read a settings
    /// store, or touch the filesystem or environment; Caps therefore can
    /// only originate in code. Runs against the source tree (the part
    /// before each file's `#[cfg(test)]` block) so a violating import
    /// fails CI even if it compiles.
    #[test]
    fn caps_are_code_never_settings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/modules/harness");
        let forbidden = [
            "Deserialize",
            "tauri_plugin_store",
            "std::fs",
            "std::env",
            "serde_json::from",
        ];
        for entry in std::fs::read_dir(&dir).expect("harness module must exist") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let production = source.split("#[cfg(test)]").next().unwrap();
            for token in forbidden {
                assert!(
                    !production.contains(token),
                    "harness file {} contains forbidden token {token:?}",
                    path.display()
                );
            }
        }
    }
}
