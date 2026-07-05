//! Pure state transitions: `apply(state, event) -> state`.
//!
//! Zero I/O. Errors are data (`CoreError`); persistence and side effects
//! live in `modules::registry`.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::project::Project;
use super::workspace::{lowest_free_slot, validate_workspace, Workspace};

/// Whole-registry pure state. Serialized inside a versioned envelope by
/// `modules::registry`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreState {
    #[serde(default)]
    pub projects: Vec<Project>,
    /// Live Workspaces across all Projects (removed on land/scuttle).
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
}

/// Every mutation of Helmsmen state is an Event fed through [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ProjectAdded { project: Project },
    /// A Workspace was cut (worktree + branch already created by the
    /// shell). Carries the full entity; `apply` enforces the Slot rule.
    WorkspaceCut { workspace: Workspace },
    /// A Workspace was removed (worktree + branch already cleaned up by
    /// the shell). Frees its Slot implicitly.
    WorkspaceRemoved { workspace_id: String },
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
    UnknownProject(String),
    UnknownWorkspace(String),
    DuplicateWorkspaceId(String),
    /// The branch is already used by a live Workspace of the same Project.
    DuplicateBranch(String),
    DuplicateWorktreePath(String),
    /// The Slot rule: a cut must carry the lowest free integer among the
    /// Project's live Workspaces.
    SlotNotLowestFree { expected: u32, got: u32 },
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
            CoreError::UnknownProject(id) => write!(f, "no project with id {id:?}"),
            CoreError::UnknownWorkspace(id) => write!(f, "no workspace with id {id:?}"),
            CoreError::DuplicateWorkspaceId(id) => {
                write!(f, "a workspace with id {id:?} already exists")
            }
            CoreError::DuplicateBranch(branch) => {
                write!(f, "branch {branch:?} is already used by a live workspace")
            }
            CoreError::DuplicateWorktreePath(path) => {
                write!(f, "worktree path {path:?} is already used by a live workspace")
            }
            CoreError::SlotNotLowestFree { expected, got } => write!(
                f,
                "slot {got} is not the lowest free slot (expected {expected})"
            ),
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
        Event::WorkspaceCut { workspace } => {
            validate_workspace(&workspace)?;
            if !state.projects.iter().any(|p| p.id == workspace.project_id) {
                return Err(CoreError::UnknownProject(workspace.project_id));
            }
            if state.workspaces.iter().any(|w| w.id == workspace.id) {
                return Err(CoreError::DuplicateWorkspaceId(workspace.id));
            }
            if state
                .workspaces
                .iter()
                .any(|w| w.project_id == workspace.project_id && w.branch == workspace.branch)
            {
                return Err(CoreError::DuplicateBranch(workspace.branch));
            }
            if state
                .workspaces
                .iter()
                .any(|w| w.worktree_path == workspace.worktree_path)
            {
                return Err(CoreError::DuplicateWorktreePath(workspace.worktree_path));
            }
            let expected = lowest_free_slot(&state.workspaces, &workspace.project_id);
            if workspace.slot != expected {
                return Err(CoreError::SlotNotLowestFree {
                    expected,
                    got: workspace.slot,
                });
            }
            let mut next = state;
            next.workspaces.push(workspace);
            Ok(next)
        }
        Event::WorkspaceRemoved { workspace_id } => {
            if !state.workspaces.iter().any(|w| w.id == workspace_id) {
                return Err(CoreError::UnknownWorkspace(workspace_id));
            }
            let mut next = state;
            next.workspaces.retain(|w| w.id != workspace_id);
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

    // --- apply: WorkspaceCut / WorkspaceRemoved (task #5) ---

    fn workspace(id: &str, project_id: &str, slug: &str, slot: u32) -> Workspace {
        Workspace {
            id: id.to_string(),
            project_id: project_id.to_string(),
            slug: slug.to_string(),
            branch: format!("helm/{slug}"),
            worktree_path: format!("/home/dev/.helmsmen/worktrees/helmsmen/{slug}-{slot}"),
            slot,
        }
    }

    fn state_with_project(id: &str) -> CoreState {
        apply(
            CoreState::default(),
            Event::ProjectAdded {
                project: project(id, &format!("/home/dev/src/{id}")),
            },
        )
        .unwrap()
    }

    #[test]
    fn workspace_cut_appends_and_first_slot_is_one() {
        let w = workspace("ws-1", "prj-1", "fix-login", 1);
        let next = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut {
                workspace: w.clone(),
            },
        )
        .expect("valid cut must be accepted");
        assert_eq!(next.workspaces, vec![w]);
    }

    #[test]
    fn workspace_cut_requires_an_existing_project() {
        let w = workspace("ws-1", "prj-ghost", "fix-login", 1);
        let err = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut { workspace: w },
        )
        .unwrap_err();
        assert_eq!(err, CoreError::UnknownProject("prj-ghost".to_string()));
    }

    #[test]
    fn workspace_cut_enforces_the_lowest_free_slot() {
        let s = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut {
                workspace: workspace("ws-1", "prj-1", "a", 1),
            },
        )
        .unwrap();
        // Slot 2 is the lowest free; 3 must be rejected.
        let err = apply(
            s.clone(),
            Event::WorkspaceCut {
                workspace: workspace("ws-2", "prj-1", "b", 3),
            },
        )
        .unwrap_err();
        assert_eq!(err, CoreError::SlotNotLowestFree { expected: 2, got: 3 });
        // A stale slot 1 (already taken) must be rejected too.
        let err = apply(
            s,
            Event::WorkspaceCut {
                workspace: workspace("ws-2", "prj-1", "b", 1),
            },
        )
        .unwrap_err();
        assert_eq!(err, CoreError::SlotNotLowestFree { expected: 2, got: 1 });
    }

    #[test]
    fn removal_frees_the_slot_for_the_next_cut() {
        let s = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut {
                workspace: workspace("ws-1", "prj-1", "a", 1),
            },
        )
        .unwrap();
        let s = apply(
            s,
            Event::WorkspaceCut {
                workspace: workspace("ws-2", "prj-1", "b", 2),
            },
        )
        .unwrap();
        let s = apply(
            s,
            Event::WorkspaceRemoved {
                workspace_id: "ws-1".to_string(),
            },
        )
        .unwrap();
        assert_eq!(s.workspaces.len(), 1);
        // Slot 1 is free again: a cut carrying slot 1 is accepted.
        let s = apply(
            s,
            Event::WorkspaceCut {
                workspace: workspace("ws-3", "prj-1", "c", 1),
            },
        )
        .unwrap();
        assert_eq!(
            s.workspaces.iter().map(|w| w.slot).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn slots_are_allocated_per_project() {
        let s = state_with_project("prj-1");
        let s = apply(
            s,
            Event::ProjectAdded {
                project: project("prj-2", "/home/dev/src/prj-2"),
            },
        )
        .unwrap();
        let s = apply(
            s,
            Event::WorkspaceCut {
                workspace: workspace("ws-1", "prj-1", "a", 1),
            },
        )
        .unwrap();
        // prj-2's first cut also gets slot 1.
        let mut w = workspace("ws-2", "prj-2", "a", 1);
        w.worktree_path = "/home/dev/.helmsmen/worktrees/other/a-1".to_string();
        apply(s, Event::WorkspaceCut { workspace: w }).expect("slots are per project");
    }

    #[test]
    fn duplicate_workspace_id_branch_and_path_are_rejected() {
        let s = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut {
                workspace: workspace("ws-1", "prj-1", "a", 1),
            },
        )
        .unwrap();

        let mut dup_id = workspace("ws-1", "prj-1", "b", 2);
        dup_id.worktree_path = "/home/dev/wt/b-2".to_string();
        let err = apply(
            s.clone(),
            Event::WorkspaceCut { workspace: dup_id },
        )
        .unwrap_err();
        assert_eq!(err, CoreError::DuplicateWorkspaceId("ws-1".to_string()));

        let mut dup_branch = workspace("ws-2", "prj-1", "b", 2);
        dup_branch.branch = "helm/a".to_string();
        let err = apply(
            s.clone(),
            Event::WorkspaceCut {
                workspace: dup_branch,
            },
        )
        .unwrap_err();
        assert_eq!(err, CoreError::DuplicateBranch("helm/a".to_string()));

        let mut dup_path = workspace("ws-2", "prj-1", "b", 2);
        dup_path.worktree_path =
            "/home/dev/.helmsmen/worktrees/helmsmen/a-1".to_string();
        let err = apply(
            s,
            Event::WorkspaceCut {
                workspace: dup_path,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            CoreError::DuplicateWorktreePath(
                "/home/dev/.helmsmen/worktrees/helmsmen/a-1".to_string()
            )
        );
    }

    #[test]
    fn workspace_cut_rejects_traversal_in_worktree_path() {
        let mut w = workspace("ws-1", "prj-1", "a", 1);
        w.worktree_path = "/home/dev/wt/../../etc".to_string();
        let err = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut { workspace: w },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CoreError::Invalid {
                field: "worktreePath",
                ..
            }
        ));
    }

    #[test]
    fn removing_an_unknown_workspace_is_rejected() {
        let err = apply(
            state_with_project("prj-1"),
            Event::WorkspaceRemoved {
                workspace_id: "ws-ghost".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err, CoreError::UnknownWorkspace("ws-ghost".to_string()));
    }

    #[test]
    fn workspaces_serialize_with_camel_case_fields_and_round_trip() {
        let s = apply(
            state_with_project("prj-1"),
            Event::WorkspaceCut {
                workspace: workspace("ws-1", "prj-1", "a", 1),
            },
        )
        .unwrap();
        let json = serde_json::to_value(&s).unwrap();
        let w = &json["workspaces"][0];
        for key in ["id", "projectId", "slug", "branch", "worktreePath", "slot"] {
            assert!(w.get(key).is_some(), "missing camelCase key {key}");
        }
        let back: CoreState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn registry_files_without_workspaces_still_deserialize() {
        // Files written by the task-#4 build have no `workspaces` key.
        let s: CoreState = serde_json::from_str(r#"{ "projects": [] }"#).unwrap();
        assert_eq!(s, CoreState::default());
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
