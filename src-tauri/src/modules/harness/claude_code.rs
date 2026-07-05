//! The `claude-code` Harness: an interactive `claude` in a PTY.

use super::{Caps, ConfigFile, Harness, LaunchContext, LaunchPlan};

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

    /// Interactive `claude`, no args at M1. Model, MCP set, and the
    /// opening prompt from the Brief compose onto this plan at M2 (#8).
    fn launch_plan(&self, _ctx: &LaunchContext) -> LaunchPlan {
        LaunchPlan {
            program: "claude".to_string(),
            args: Vec::new(),
        }
    }

    /// M3 writes control-plane hook wiring through this seam; at M1 there
    /// is deliberately nothing to inject.
    fn config_injection(&self, _ctx: &LaunchContext) -> Vec<ConfigFile> {
        Vec::new()
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

    #[test]
    fn launch_plan_is_interactive_claude_as_argv() {
        let env = ctx_env();
        let ctx = LaunchContext {
            workspace_root: "/tmp/wt/fix-1",
            env: &env,
        };
        let plan = ClaudeCode.launch_plan(&ctx);
        assert_eq!(plan.program, "claude");
        assert!(plan.args.is_empty(), "interactive at M1: no args");
    }

    #[test]
    fn config_injection_seam_exists_and_is_empty_at_m1() {
        let env = ctx_env();
        let ctx = LaunchContext {
            workspace_root: "/tmp/wt/fix-1",
            env: &env,
        };
        assert!(ClaudeCode.config_injection(&ctx).is_empty());
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
