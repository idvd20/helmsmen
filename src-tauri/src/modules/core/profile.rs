//! Profile entity and the five built-in templates (task #7).
//!
//! A Profile is a reusable launch config for one Session: prompt snippet,
//! model, MCP set, verify command, color — and exactly one Harness it
//! launches under (referenced by the Harness's stable id, e.g.
//! `"claude-code"`; existence is checked at the command seam, shape
//! here). Profiles are *Project-owned copies*, seeded from the built-in
//! templates at Project-add; there is no global Profile entity, so a
//! seeded copy diverges freely without affecting other Projects or the
//! templates (design-notes 2026-07-04).
//!
//! Templates are app code, like Harnesses — never configuration, never
//! read from a repo.

use serde::{Deserialize, Serialize};

use super::state::CoreError;

/// A Project-owned launch config for one Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// Stable registry id (seeded ids are `{projectId}:{templateId}`).
    pub id: String,
    /// Owning Project; immutable after seeding.
    pub project_id: String,
    /// Display name (`Feature`, `Bugfix`, …); unique within the Project.
    pub name: String,
    /// Prepended/wrapped around the Brief as the first Session's opening
    /// prompt; `{brief}` marks where the Brief goes (substitution is the
    /// cut pipeline's job, task #8).
    pub prompt_snippet: String,
    /// Harness-specific model name. Empty means the Harness default.
    #[serde(default)]
    pub model: String,
    /// Named MCP servers composed into the worktree's MCP config at
    /// spawn (M6). A set: no duplicates.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    /// The Profile's check command, run in the worktree on demand or on
    /// Stop. Empty means no verify. Repo-specific by nature — the reason
    /// Profiles are per-Project copies.
    #[serde(default)]
    pub verify_command: String,
    /// `#rrggbb` hex; follows the Workspace everywhere in the UI.
    pub color: String,
    /// Exactly one Harness, by its stable id (`"claude-code"`).
    pub harness_id: String,
}

/// One built-in template. App code: `&'static` data only, nothing here
/// can be granted or altered by configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileTemplate {
    /// Stable template id, used in seeded Profile ids.
    pub id: &'static str,
    pub name: &'static str,
    pub prompt_snippet: &'static str,
    pub model: &'static str,
    pub mcp_servers: &'static [&'static str],
    pub verify_command: &'static str,
    pub color: &'static str,
    pub harness_id: &'static str,
}

/// Stable id of the built-in claude-code Harness
/// (`crate::modules::harness::claude_code`). Duplicated as a literal so
/// the pure core never imports the harness layer; a guard test in
/// `modules::harness` asserts every template id resolves there.
const CLAUDE_CODE: &str = "claude-code";

/// The five built-in templates seeded at Project-add. Verify commands
/// are empty on purpose: they are inherently repo-specific, which is why
/// Profiles are per-Project copies in the first place.
pub const BUILTIN_TEMPLATES: [ProfileTemplate; 5] = [
    ProfileTemplate {
        id: "feature",
        name: "Feature",
        prompt_snippet: "/tdd {brief}",
        model: "",
        mcp_servers: &[],
        verify_command: "",
        color: "#3b82f6",
        harness_id: CLAUDE_CODE,
    },
    ProfileTemplate {
        id: "bugfix",
        name: "Bugfix",
        prompt_snippet: "/diagnose {brief}",
        model: "",
        mcp_servers: &[],
        verify_command: "",
        color: "#ef4444",
        harness_id: CLAUDE_CODE,
    },
    ProfileTemplate {
        id: "research",
        name: "Research",
        prompt_snippet: "Research only — explore and report, change no code: {brief}",
        model: "",
        mcp_servers: &[],
        verify_command: "",
        color: "#8b5cf6",
        harness_id: CLAUDE_CODE,
    },
    ProfileTemplate {
        id: "spike",
        name: "Spike",
        prompt_snippet: "Spike — cheapest throwaway proof, answer the question then stop: {brief}",
        model: "",
        mcp_servers: &[],
        verify_command: "",
        color: "#f59e0b",
        harness_id: CLAUDE_CODE,
    },
    ProfileTemplate {
        id: "reviewer",
        name: "Reviewer",
        prompt_snippet: "Review the changes in this worktree — report findings, modify nothing: {brief}",
        model: "",
        mcp_servers: &[],
        verify_command: "",
        color: "#10b981",
        harness_id: CLAUDE_CODE,
    },
];

/// The Project-owned copies seeded when `project_id` is added.
/// Deterministic and pure: ids are `{project_id}:{template_id}`, unique
/// because Project ids are unique and template ids contain no `:`.
pub fn seed_profiles(project_id: &str) -> Vec<Profile> {
    BUILTIN_TEMPLATES
        .iter()
        .map(|t| Profile {
            id: format!("{project_id}:{}", t.id),
            project_id: project_id.to_string(),
            name: t.name.to_string(),
            prompt_snippet: t.prompt_snippet.to_string(),
            model: t.model.to_string(),
            mcp_servers: t.mcp_servers.iter().map(|s| s.to_string()).collect(),
            verify_command: t.verify_command.to_string(),
            color: t.color.to_string(),
            harness_id: t.harness_id.to_string(),
        })
        .collect()
}

/// Validate a Profile's fields as pure data. Called by `apply` so no
/// invalid entity can ever enter the state. Harness *existence* is the
/// command seam's job (the core stays independent of the harness layer);
/// the shape — exactly one non-empty harness id — is enforced here.
pub fn validate_profile(profile: &Profile) -> Result<(), CoreError> {
    non_empty("id", &profile.id)?;
    non_empty("projectId", &profile.project_id)?;
    non_empty("name", &profile.name)?;
    no_control_chars("name", &profile.name)?;
    text_field("promptSnippet", &profile.prompt_snippet)?;
    text_field("verifyCommand", &profile.verify_command)?;
    no_control_chars("model", &profile.model)?;
    non_empty("harnessId", &profile.harness_id)?;
    no_control_chars("harnessId", &profile.harness_id)?;
    validate_color(&profile.color)?;
    let mut seen: Vec<&String> = Vec::with_capacity(profile.mcp_servers.len());
    for server in &profile.mcp_servers {
        if server.trim().is_empty() {
            return Err(CoreError::Invalid {
                field: "mcpServers",
                reason: "MCP server name must not be empty".to_string(),
            });
        }
        no_control_chars("mcpServers", server)?;
        if seen.contains(&server) {
            return Err(CoreError::Invalid {
                field: "mcpServers",
                reason: format!("duplicate MCP server {server:?}"),
            });
        }
        seen.push(server);
    }
    Ok(())
}

fn non_empty(field: &'static str, value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() {
        return Err(CoreError::Invalid {
            field,
            reason: "must not be empty".to_string(),
        });
    }
    Ok(())
}

fn no_control_chars(field: &'static str, value: &str) -> Result<(), CoreError> {
    if let Some(bad) = value.chars().find(|c| c.is_control()) {
        return Err(CoreError::Invalid {
            field,
            reason: format!("contains control character {bad:?}"),
        });
    }
    Ok(())
}

/// Multiline free text (prompt snippets, verify commands): newlines are
/// fine, NUL is not.
fn text_field(field: &'static str, value: &str) -> Result<(), CoreError> {
    if value.contains('\0') {
        return Err(CoreError::Invalid {
            field,
            reason: "must not contain a NUL byte".to_string(),
        });
    }
    Ok(())
}

fn validate_color(color: &str) -> Result<(), CoreError> {
    let ok = color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return Err(CoreError::Invalid {
            field: "color",
            reason: format!("{color:?} is not a #rrggbb hex color"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_five_templates_with_unique_ids_names_and_colors() {
        assert_eq!(BUILTIN_TEMPLATES.len(), 5);
        let expect = ["Feature", "Bugfix", "Research", "Spike", "Reviewer"];
        let names: Vec<&str> = BUILTIN_TEMPLATES.iter().map(|t| t.name).collect();
        assert_eq!(names, expect);
        let mut ids: Vec<&str> = BUILTIN_TEMPLATES.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 5, "template ids must be unique");
        let mut colors: Vec<&str> = BUILTIN_TEMPLATES.iter().map(|t| t.color).collect();
        colors.sort_unstable();
        colors.dedup();
        assert_eq!(colors.len(), 5, "template colors must be distinct");
        // Seeded ids embed the template id after a ':'; template ids must
        // therefore never contain one.
        assert!(BUILTIN_TEMPLATES.iter().all(|t| !t.id.contains(':')));
    }

    #[test]
    fn every_template_declares_exactly_one_harness() {
        for t in BUILTIN_TEMPLATES {
            assert!(!t.harness_id.is_empty(), "template {} has no harness", t.id);
        }
    }

    #[test]
    fn seeding_is_deterministic_and_valid() {
        let a = seed_profiles("prj-1");
        let b = seed_profiles("prj-1");
        assert_eq!(a, b);
        assert_eq!(a.len(), 5);
        for p in &a {
            assert_eq!(p.project_id, "prj-1");
            assert!(p.id.starts_with("prj-1:"), "unexpected id {}", p.id);
            validate_profile(p).unwrap_or_else(|e| panic!("seed {} invalid: {e}", p.id));
        }
        let mut ids: Vec<&String> = a.iter().map(|p| &p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 5, "seeded ids must be unique");
    }

    #[test]
    fn seeds_for_different_projects_never_collide() {
        let a = seed_profiles("prj-a");
        let b = seed_profiles("prj-b");
        for p in &a {
            assert!(b.iter().all(|q| q.id != p.id));
        }
    }

    fn profile() -> Profile {
        seed_profiles("prj-1").remove(0)
    }

    type Mutation = Box<dyn Fn(&mut Profile)>;

    #[test]
    fn hostile_profile_fields_are_rejected() {
        let cases: Vec<(&str, Mutation)> = vec![
            ("id", Box::new(|p| p.id = "  ".to_string())),
            ("projectId", Box::new(|p| p.project_id = "".to_string())),
            ("name", Box::new(|p| p.name = "".to_string())),
            ("name", Box::new(|p| p.name = "bad\u{7}name".to_string())),
            (
                "promptSnippet",
                Box::new(|p| p.prompt_snippet = "a\0b".to_string()),
            ),
            (
                "verifyCommand",
                Box::new(|p| p.verify_command = "a\0b".to_string()),
            ),
            ("model", Box::new(|p| p.model = "son\u{1b}net".to_string())),
            ("harnessId", Box::new(|p| p.harness_id = "".to_string())),
            ("color", Box::new(|p| p.color = "red".to_string())),
            ("color", Box::new(|p| p.color = "#12345".to_string())),
            ("color", Box::new(|p| p.color = "#12345g".to_string())),
            (
                "mcpServers",
                Box::new(|p| p.mcp_servers = vec!["".to_string()]),
            ),
            (
                "mcpServers",
                Box::new(|p| {
                    p.mcp_servers = vec!["playwright".to_string(), "playwright".to_string()]
                }),
            ),
        ];
        for (field, mutate) in cases {
            let mut p = profile();
            mutate(&mut p);
            let err = validate_profile(&p).unwrap_err();
            match err {
                CoreError::Invalid { field: got, .. } => {
                    assert_eq!(got, field, "wrong field for mutation of {field}")
                }
                other => panic!("expected Invalid {field}, got {other:?}"),
            }
        }
    }

    #[test]
    fn multiline_prompt_snippet_and_verify_command_are_accepted() {
        let mut p = profile();
        p.prompt_snippet = "/tdd {brief}\n\nKeep commits small.".to_string();
        p.verify_command = "pnpm lint\npnpm test".to_string();
        p.model = "claude-sonnet-4-5".to_string();
        p.mcp_servers = vec!["playwright".to_string(), "context7".to_string()];
        assert!(validate_profile(&p).is_ok());
    }

    #[test]
    fn profile_serializes_with_camel_case_and_round_trips() {
        let p = profile();
        let json = serde_json::to_value(&p).unwrap();
        for key in [
            "id",
            "projectId",
            "name",
            "promptSnippet",
            "model",
            "mcpServers",
            "verifyCommand",
            "color",
            "harnessId",
        ] {
            assert!(json.get(key).is_some(), "missing camelCase key {key}");
        }
        let back: Profile = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }
}
