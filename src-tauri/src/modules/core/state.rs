//! Pure state transitions: `apply(state, event) -> state`.
//!
//! Zero I/O. Errors are data (`CoreError`); persistence and side effects
//! live in `modules::registry`.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::project::Project;

/// Whole-registry pure state. Serialized inside a versioned envelope by
/// `modules::registry`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreState {
    #[serde(default)]
    pub projects: Vec<Project>,
}

/// Every mutation of Helmsmen state is an Event fed through [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ProjectAdded { project: Project },
}

/// Pure, serializable transition errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// A field failed data validation.
    Invalid {
        field: &'static str,
        reason: String,
    },
    DuplicateProjectId(String),
    DuplicateRepoRoot(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::Invalid { field, reason } => write!(f, "invalid {field}: {reason}"),
            CoreError::DuplicateProjectId(id) => {
                write!(f, "a project with id {id:?} already exists")
            }
            CoreError::DuplicateRepoRoot(root) => {
                write!(f, "a project for {root:?} already exists")
            }
        }
    }
}

impl std::error::Error for CoreError {}

/// The only way state changes. Pure: consumes a state and an event,
/// returns the next state or an error (in which case the caller's state is
/// unchanged — the shell clones before applying).
pub fn apply(state: CoreState, event: Event) -> Result<CoreState, CoreError> {
    match event {
        Event::ProjectAdded { project } => {
            super::project::validate_project(&project)?;
            if state.projects.iter().any(|p| p.id == project.id) {
                return Err(CoreError::DuplicateProjectId(project.id));
            }
            if state
                .projects
                .iter()
                .any(|p| p.repo_root == project.repo_root)
            {
                return Err(CoreError::DuplicateRepoRoot(project.repo_root));
            }
            let mut next = state;
            next.projects.push(project);
            Ok(next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::project::{
        prefill, repo_name, validate_branch_template, validate_ref_name, DEFAULT_BRANCH_TEMPLATE,
    };
    use super::*;

    fn project(id: &str, repo_root: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "helmsmen".to_string(),
            repo_root: repo_root.to_string(),
            base_branch: "main".to_string(),
            worktree_home: "/home/dev/.helmsmen/worktrees/helmsmen".to_string(),
            branch_template: DEFAULT_BRANCH_TEMPLATE.to_string(),
        }
    }

    // --- apply: ProjectAdded ---

    #[test]
    fn project_added_appends_to_empty_state() {
        let p = project("prj-1", "/home/dev/src/helmsmen");
        let next = apply(
            CoreState::default(),
            Event::ProjectAdded { project: p.clone() },
        )
        .expect("valid project must be accepted");
        assert_eq!(next.projects, vec![p]);
    }

    #[test]
    fn project_added_preserves_existing_projects() {
        let a = project("prj-a", "/home/dev/src/a");
        let b = project("prj-b", "/home/dev/src/b");
        let s1 = apply(
            CoreState::default(),
            Event::ProjectAdded { project: a.clone() },
        )
        .unwrap();
        let s2 = apply(s1, Event::ProjectAdded { project: b.clone() }).unwrap();
        assert_eq!(s2.projects, vec![a, b]);
    }

    #[test]
    fn duplicate_project_id_is_rejected() {
        let a = project("prj-a", "/home/dev/src/a");
        let dup = project("prj-a", "/home/dev/src/other");
        let s1 = apply(CoreState::default(), Event::ProjectAdded { project: a }).unwrap();
        let err = apply(s1, Event::ProjectAdded { project: dup }).unwrap_err();
        assert_eq!(err, CoreError::DuplicateProjectId("prj-a".to_string()));
    }

    #[test]
    fn duplicate_repo_root_is_rejected() {
        let a = project("prj-a", "/home/dev/src/a");
        let dup = project("prj-b", "/home/dev/src/a");
        let s1 = apply(CoreState::default(), Event::ProjectAdded { project: a }).unwrap();
        let err = apply(s1, Event::ProjectAdded { project: dup }).unwrap_err();
        assert_eq!(
            err,
            CoreError::DuplicateRepoRoot("/home/dev/src/a".to_string())
        );
    }

    // --- apply: validation guards the state ---

    #[test]
    fn empty_id_is_rejected() {
        let mut p = project("", "/home/dev/src/a");
        p.id = "".to_string();
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(err, CoreError::Invalid { field: "id", .. }));
    }

    #[test]
    fn blank_name_is_rejected() {
        let mut p = project("prj-a", "/home/dev/src/a");
        p.name = "   ".to_string();
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(err, CoreError::Invalid { field: "name", .. }));
    }

    #[test]
    fn relative_repo_root_is_rejected() {
        let p = project("prj-a", "src/helmsmen");
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "repoRoot",
                ..
            }
        ));
    }

    #[test]
    fn parent_traversal_in_repo_root_is_rejected() {
        let p = project("prj-a", "/home/dev/../../etc");
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "repoRoot",
                ..
            }
        ));
    }

    #[test]
    fn relative_worktree_home_is_rejected() {
        let mut p = project("prj-a", "/home/dev/src/a");
        p.worktree_home = "worktrees/a".to_string();
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "worktreeHome",
                ..
            }
        ));
    }

    #[test]
    fn hostile_base_branch_is_rejected() {
        for bad in [
            "",
            "-option-injection",
            "has space",
            "a..b",
            "ends.lock",
            "trailing/",
            "/leading",
            "double//slash",
            ".dotstart",
            "seg/.dotstart",
            "@",
            "at@{brace",
            "ctrl\u{7}char",
            "back\\slash",
            "star*",
            "colon:name",
            "ends.",
        ] {
            let mut p = project("prj-a", "/home/dev/src/a");
            p.base_branch = bad.to_string();
            let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
            assert!(
                matches!(
                    err,
                    CoreError::Invalid {
                        field: "baseBranch",
                        ..
                    }
                ),
                "expected {bad:?} to be rejected as a base branch, got {err:?}"
            );
        }
    }

    #[test]
    fn reasonable_base_branches_are_accepted() {
        for good in ["main", "master", "develop", "release/1.2", "trunk-2026"] {
            let mut p = project("prj-a", "/home/dev/src/a");
            p.base_branch = good.to_string();
            apply(CoreState::default(), Event::ProjectAdded { project: p })
                .unwrap_or_else(|e| panic!("expected {good:?} to be accepted, got {e}"));
        }
    }

    #[test]
    fn branch_template_with_unknown_placeholder_is_rejected() {
        let mut p = project("prj-a", "/home/dev/src/a");
        p.branch_template = "helm/{typo}".to_string();
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "branchTemplate",
                ..
            }
        ));
    }

    #[test]
    fn branch_template_expanding_to_invalid_ref_is_rejected() {
        let mut p = project("prj-a", "/home/dev/src/a");
        p.branch_template = "helm/{slug}/".to_string();
        let err = apply(CoreState::default(), Event::ProjectAdded { project: p }).unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "branchTemplate",
                ..
            }
        ));
    }

    // --- validators directly ---

    #[test]
    fn validate_ref_name_accepts_nested_branches() {
        assert!(validate_ref_name("baseBranch", "feat/deep/branch-1").is_ok());
    }

    #[test]
    fn validate_branch_template_accepts_slug_and_slot() {
        assert!(validate_branch_template("branchTemplate", "helm/{slug}-{slot}").is_ok());
    }

    // --- prefill (pure) ---

    #[test]
    fn prefill_derives_name_home_and_template() {
        let pf = prefill("/home/dev/src/helmsmen", "/home/dev").unwrap();
        assert_eq!(pf.name, "helmsmen");
        assert_eq!(
            std::path::Path::new(&pf.worktree_home),
            std::path::Path::new("/home/dev/.helmsmen/worktrees/helmsmen")
        );
        assert_eq!(pf.branch_template, DEFAULT_BRANCH_TEMPLATE);
    }

    #[test]
    fn prefill_of_unusable_root_errors() {
        assert!(prefill("/", "/home/dev").is_err());
    }

    #[test]
    fn repo_name_takes_last_component() {
        assert_eq!(repo_name("/a/b/repo"), Some("repo".to_string()));
        assert_eq!(repo_name("/"), None);
    }

    // --- serialization shape (locks the registry JSON contract) ---

    #[test]
    fn state_serializes_with_camel_case_fields() {
        let s = apply(
            CoreState::default(),
            Event::ProjectAdded {
                project: project("prj-a", "/home/dev/src/a"),
            },
        )
        .unwrap();
        let json = serde_json::to_value(&s).unwrap();
        let p = &json["projects"][0];
        for key in [
            "id",
            "name",
            "repoRoot",
            "baseBranch",
            "worktreeHome",
            "branchTemplate",
        ] {
            assert!(p.get(key).is_some(), "missing camelCase key {key}");
        }
    }

    #[test]
    fn state_round_trips_through_json() {
        let s = apply(
            CoreState::default(),
            Event::ProjectAdded {
                project: project("prj-a", "/home/dev/src/a"),
            },
        )
        .unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let back: CoreState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
