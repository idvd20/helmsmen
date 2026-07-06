//! The `claude-code` Harness: an interactive `claude` in a PTY.

use super::{Caps, ConfigFile, Harness, LaunchContext, LaunchPlan};
use crate::modules::hooks::{claude_code_hook_settings, CLAUDE_HOOK_SETTINGS_REL};

/// Full capability set. Declared as a const so the compiler, not a config
/// file, is the source of truth; adding a `Caps` field forces an explicit
/// decision here.
pub const CLAUDE_CODE_CAPS: Caps = Caps {
    resume: true,
    control_plane_hooks: true,
    agent_signal: true,
    cost_telemetry: true,
    mcp_config: true,
    model_select: true,
};

pub struct ClaudeCode;

impl Harness for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn caps(&self) -> Caps {
        CLAUDE_CODE_CAPS
    }

    /// Interactive `claude`. The Profile's model and the opening prompt
    /// (snippet + Brief) compose onto the plan as argv (task #8); the MCP
    /// set arrives at M6. Bare `claude` when both are empty (M1 behavior,
    /// and every Session after the first).
    fn launch_plan(&self, ctx: &LaunchContext) -> LaunchPlan {
        let mut args = Vec::new();
        if !ctx.model.is_empty() {
            // `--model=x` as ONE argv element: a hostile model string can
            // never be re-parsed as a separate flag.
            args.push(format!("--model={}", ctx.model));
        }
        if !ctx.opening_prompt.is_empty() {
            // Positional prompt starts the interactive REPL with it. A
            // leading `-` would parse as a flag; a leading space keeps it
            // data without changing the prompt's meaning.
            if ctx.opening_prompt.starts_with('-') {
                args.push(format!(" {}", ctx.opening_prompt));
            } else {
                args.push(ctx.opening_prompt.to_string());
            }
        }
        LaunchPlan {
            program: "claude".to_string(),
            args,
        }
    }

    /// M3 control-plane hook wiring (task #16): when the cut has a running
    /// per-Workspace endpoint, write the Claude Code hook settings so this
    /// `claude` POSTs its hook events there under the session bearer token.
    /// The worktree-LOCAL settings file is used, so the user's and Terax's
    /// global hooks (a different file Claude Code also merges) keep firing.
    /// Without an endpoint — the M1/M2 stub — there is nothing to inject.
    fn config_injection(&self, ctx: &LaunchContext) -> Vec<ConfigFile> {
        match ctx.control_plane {
            Some(cp) => vec![ConfigFile {
                rel_path: CLAUDE_HOOK_SETTINGS_REL.to_string(),
                contents: claude_code_hook_settings(cp.url, cp.token),
            }],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ctx_env() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HELMSMEN_SLOT".to_string(), "1".to_string()),
            ("HELMSMEN_WORKSPACE".to_string(), "/tmp/wt/fix-1".to_string()),
            ("HELMSMEN_PROJECT".to_string(), "demo".to_string()),
            ("HELMSMEN_MAIN_CHECKOUT".to_string(), "/tmp/repo".to_string()),
        ])
    }

    /// AC: `claude-code` declares its full Cap set, in code. Destructured
    /// exhaustively so a new Caps field breaks this test until claude-code
    /// takes a position on it.
    #[test]
    fn claude_code_declares_the_full_cap_set() {
        let Caps {
            resume,
            control_plane_hooks,
            agent_signal,
            cost_telemetry,
            mcp_config,
            model_select,
        } = ClaudeCode.caps();
        assert!(resume);
        assert!(control_plane_hooks);
        assert!(agent_signal);
        assert!(cost_telemetry);
        assert!(mcp_config);
        assert!(model_select);
    }

    fn ctx<'a>(
        env: &'a BTreeMap<String, String>,
        model: &'a str,
        opening_prompt: &'a str,
    ) -> LaunchContext<'a> {
        LaunchContext {
            workspace_root: "/tmp/wt/fix-1",
            env,
            model,
            opening_prompt,
            control_plane: None,
        }
    }

    fn wired_ctx<'a>(
        env: &'a BTreeMap<String, String>,
        wiring: super::super::ControlPlaneWiring<'a>,
    ) -> LaunchContext<'a> {
        LaunchContext {
            workspace_root: "/tmp/wt/fix-1",
            env,
            model: "",
            opening_prompt: "",
            control_plane: Some(wiring),
        }
    }

    #[test]
    fn launch_plan_is_interactive_claude_as_argv() {
        let env = ctx_env();
        let plan = ClaudeCode.launch_plan(&ctx(&env, "", ""));
        assert_eq!(plan.program, "claude");
        assert!(plan.args.is_empty(), "bare interactive claude: no args");
    }

    // --- task #8: model + opening prompt compose onto the plan ---

    #[test]
    fn launch_plan_composes_model_and_opening_prompt() {
        let env = ctx_env();
        let plan = ClaudeCode.launch_plan(&ctx(&env, "claude-sonnet-4-5", "/tdd fix login"));
        assert_eq!(plan.program, "claude");
        assert_eq!(
            plan.args,
            vec!["--model=claude-sonnet-4-5".to_string(), "/tdd fix login".to_string()]
        );
    }

    #[test]
    fn model_is_a_single_argv_element_never_a_separate_flag() {
        let env = ctx_env();
        // A hostile model string must stay the value of --model=.
        let plan = ClaudeCode.launch_plan(&ctx(&env, "--dangerously-skip-permissions", ""));
        assert_eq!(plan.args, vec!["--model=--dangerously-skip-permissions"]);
    }

    #[test]
    fn a_prompt_starting_with_a_dash_is_kept_as_data() {
        let env = ctx_env();
        let plan = ClaudeCode.launch_plan(&ctx(&env, "", "--help me fix this"));
        assert_eq!(plan.args, vec![" --help me fix this"]);
        assert!(
            !plan.args.iter().any(|a| a.starts_with('-')),
            "no workspace-derived arg may parse as a flag"
        );
    }

    #[test]
    fn config_injection_is_empty_without_a_control_plane() {
        // No endpoint (the M1/M2 stub, or a Signal-only path): nothing is
        // written, so the agent-signal source stays the only one.
        let env = ctx_env();
        assert!(ClaudeCode
            .config_injection(&ctx(&env, "", "/tdd x"))
            .is_empty());
    }

    // --- task #16: with a control plane, the hook wiring is injected ---

    #[test]
    fn config_injection_writes_the_local_hook_settings_with_the_token() {
        let env = ctx_env();
        let wiring = super::super::ControlPlaneWiring {
            url: "http://127.0.0.1:54321/hook",
            token: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        };
        let files = ClaudeCode.config_injection(&wired_ctx(&env, wiring));
        assert_eq!(files.len(), 1, "exactly the hook settings file");
        // The LOCAL settings file — never the committed one — so the user's
        // and Terax's global hooks (a different file) are never clobbered.
        assert_eq!(files[0].rel_path, ".claude/settings.local.json");
        assert!(!files[0].rel_path.ends_with("/settings.json"));
        // The contents POST to the endpoint under the session bearer token.
        let settings: serde_json::Value = serde_json::from_str(&files[0].contents).unwrap();
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.contains(wiring.url));
        assert!(command.contains(&format!("Authorization: Bearer {}", wiring.token)));
    }

    #[test]
    fn caps_serialize_camel_case_for_the_frontend() {
        let json = serde_json::to_value(CLAUDE_CODE_CAPS).unwrap();
        let obj = json.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "agentSignal",
                "controlPlaneHooks",
                "costTelemetry",
                "mcpConfig",
                "modelSelect",
                "resume",
            ]
        );
        assert!(obj.values().all(|v| v == &serde_json::Value::Bool(true)));
    }
}
