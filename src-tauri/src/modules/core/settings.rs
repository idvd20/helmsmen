//! Per-Project settings (task #7): setup script, carry-over globs, and
//! Process definitions.
//!
//! Pure data + validation only. These are *definitions*: nothing here (or
//! anywhere in the core) runs a script, copies a file, or spawns a
//! process — execution belongs to the cut pipeline in the imperative
//! shell (task #8). Settings live user-level in Helmsmen's registry and
//! are never read from a file inside the repo, so a repo can never
//! configure its own trust.

use serde::{Deserialize, Serialize};

use super::state::CoreError;

/// Editable per-Project settings, embedded in
/// [`crate::modules::core::project::Project`]. Every field is additive
/// with a default so registry files written before task #7 keep loading.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    /// One multiline shell command run in every fresh worktree (user's
    /// shell, cwd = worktree). Empty means no setup script.
    #[serde(default)]
    pub setup_script: String,
    /// Globs of untracked files (`.env*` etc.) copied from the main
    /// checkout into each fresh worktree. Relative to the repo root.
    #[serde(default)]
    pub carry_over_globs: Vec<String>,
    /// Named long-lived commands (dev server etc.) startable as Process
    /// Sessions inside any Workspace of this Project.
    #[serde(default)]
    pub processes: Vec<ProcessDef>,
}

/// One named long-lived command, run in the Workspace's worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDef {
    /// Display name and per-Project key (`dev`, `db`). Unique within the
    /// Project.
    pub name: String,
    /// Shell command line (user's shell, cwd = worktree).
    pub command: String,
}

/// Validate settings as pure data. Called by `apply` so no invalid
/// settings can ever enter the state.
pub fn validate_settings(settings: &ProjectSettings) -> Result<(), CoreError> {
    if settings.setup_script.contains('\0') {
        return Err(CoreError::Invalid {
            field: "setupScript",
            reason: "must not contain a NUL byte".to_string(),
        });
    }
    for glob in &settings.carry_over_globs {
        validate_carry_over_glob(glob)?;
    }
    let mut seen = Vec::with_capacity(settings.processes.len());
    for process in &settings.processes {
        validate_process(process)?;
        if seen.contains(&&process.name) {
            return Err(CoreError::Invalid {
                field: "processes",
                reason: format!("duplicate process name {:?}", process.name),
            });
        }
        seen.push(&process.name);
    }
    Ok(())
}

/// A carry-over glob names untracked files *inside* the main checkout:
/// relative, no `..`, no NUL. A hostile glob must not be able to reach
/// outside the repo root (boundary validation as data — the copy step in
/// the shell re-checks against the real filesystem).
fn validate_carry_over_glob(glob: &str) -> Result<(), CoreError> {
    let invalid = |reason: String| CoreError::Invalid {
        field: "carryOverGlobs",
        reason,
    };
    if glob.trim().is_empty() {
        return Err(invalid("glob must not be empty".to_string()));
    }
    if glob.contains('\0') {
        return Err(invalid(format!("glob {glob:?} contains a NUL byte")));
    }
    if let Some(bad) = glob.chars().find(|c| c.is_control()) {
        return Err(invalid(format!(
            "glob {glob:?} contains control character {bad:?}"
        )));
    }
    if glob.starts_with('/') || glob.starts_with('\\') {
        return Err(invalid(format!(
            "glob {glob:?} must be relative to the repo root"
        )));
    }
    if glob
        .split(['/', '\\'])
        .any(|component| component == "..")
    {
        return Err(invalid(format!(
            "glob {glob:?} must not contain '..' components"
        )));
    }
    Ok(())
}

fn validate_process(process: &ProcessDef) -> Result<(), CoreError> {
    let invalid = |reason: String| CoreError::Invalid {
        field: "processes",
        reason,
    };
    if process.name.trim().is_empty() {
        return Err(invalid("process name must not be empty".to_string()));
    }
    if let Some(bad) = process.name.chars().find(|c| c.is_control()) {
        return Err(invalid(format!(
            "process name {:?} contains control character {bad:?}",
            process.name
        )));
    }
    if process.command.trim().is_empty() {
        return Err(invalid(format!(
            "process {:?} has an empty command",
            process.name
        )));
    }
    if process.command.contains('\0') {
        return Err(invalid(format!(
            "process {:?} command contains a NUL byte",
            process.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str, command: &str) -> ProcessDef {
        ProcessDef {
            name: name.to_string(),
            command: command.to_string(),
        }
    }

    #[test]
    fn default_settings_are_empty_and_valid() {
        let s = ProjectSettings::default();
        assert_eq!(s.setup_script, "");
        assert!(s.carry_over_globs.is_empty());
        assert!(s.processes.is_empty());
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn reasonable_settings_are_accepted() {
        let s = ProjectSettings {
            setup_script: "pnpm install --frozen-lockfile\npnpm build".to_string(),
            carry_over_globs: vec![".env*".to_string(), "config/*.local.json".to_string()],
            processes: vec![process("dev", "pnpm dev"), process("db", "docker compose up db")],
        };
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn hostile_carry_over_globs_are_rejected() {
        for bad in [
            "",
            "   ",
            "/etc/passwd",
            "\\windows\\system32",
            "../secrets/.env",
            "ok/../../.ssh/*",
            "..",
            "nul\0glob",
            "ctrl\u{7}glob",
        ] {
            let s = ProjectSettings {
                carry_over_globs: vec![bad.to_string()],
                ..ProjectSettings::default()
            };
            let err = validate_settings(&s).unwrap_err();
            assert!(
                matches!(
                    err,
                    CoreError::Invalid {
                        field: "carryOverGlobs",
                        ..
                    }
                ),
                "expected glob {bad:?} to be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn dotfile_globs_are_accepted() {
        // `..` traversal is banned but plain dotfiles are the whole point.
        let s = ProjectSettings {
            carry_over_globs: vec![".env*".to_string(), ".claude/settings.local.json".to_string()],
            ..ProjectSettings::default()
        };
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn hostile_processes_are_rejected() {
        for (bad, why) in [
            (process("", "pnpm dev"), "empty name"),
            (process("   ", "pnpm dev"), "blank name"),
            (process("dev\u{7}", "pnpm dev"), "control char in name"),
            (process("dev", ""), "empty command"),
            (process("dev", "   "), "blank command"),
            (process("dev", "pnpm\0dev"), "NUL in command"),
        ] {
            let s = ProjectSettings {
                processes: vec![bad],
                ..ProjectSettings::default()
            };
            let err = validate_settings(&s).unwrap_err();
            assert!(
                matches!(
                    err,
                    CoreError::Invalid {
                        field: "processes",
                        ..
                    }
                ),
                "expected rejection for {why}, got {err:?}"
            );
        }
    }

    #[test]
    fn duplicate_process_names_are_rejected() {
        let s = ProjectSettings {
            processes: vec![process("dev", "pnpm dev"), process("dev", "pnpm dev:api")],
            ..ProjectSettings::default()
        };
        let err = validate_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "processes",
                ..
            }
        ));
    }

    #[test]
    fn setup_script_allows_multiline_but_not_nul() {
        let ok = ProjectSettings {
            setup_script: "export FOO=1\nmake setup".to_string(),
            ..ProjectSettings::default()
        };
        assert!(validate_settings(&ok).is_ok());
        let bad = ProjectSettings {
            setup_script: "echo\0boom".to_string(),
            ..ProjectSettings::default()
        };
        assert!(matches!(
            validate_settings(&bad).unwrap_err(),
            CoreError::Invalid {
                field: "setupScript",
                ..
            }
        ));
    }

    #[test]
    fn settings_serialize_with_camel_case_and_round_trip() {
        let s = ProjectSettings {
            setup_script: "make setup".to_string(),
            carry_over_globs: vec![".env*".to_string()],
            processes: vec![process("dev", "pnpm dev")],
        };
        let json = serde_json::to_value(&s).unwrap();
        for key in ["setupScript", "carryOverGlobs", "processes"] {
            assert!(json.get(key).is_some(), "missing camelCase key {key}");
        }
        assert!(json["processes"][0].get("name").is_some());
        assert!(json["processes"][0].get("command").is_some());
        let back: ProjectSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn settings_json_without_any_field_deserializes_to_default() {
        let s: ProjectSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, ProjectSettings::default());
    }
}
