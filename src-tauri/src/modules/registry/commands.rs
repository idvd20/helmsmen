//! Tauri command glue for the Helmsmen registry — deliberately thin.
//!
//! Boundary validation happens here (paths re-resolved against the real
//! filesystem) and in the pure core (`apply` re-validates every field as
//! data), so a hostile frontend payload can neither register a phantom
//! clone nor smuggle an invalid entity into the registry.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tauri::State;

use crate::modules::core::project::{repo_name, Project};
use crate::modules::core::state::Event;

use super::detect::{detect_project, resolve_repo_root, ProjectDetection};
use super::RegistryState;

/// Detect prefill values for a picked local clone. Reads git metadata
/// only — never a config file inside the repo.
#[tauri::command]
pub fn helm_detect_project(path: String) -> Result<ProjectDetection, String> {
    detect_project(&path)
}

/// The add-Project form payload: detection prefill, possibly edited.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddProjectInput {
    pub repo_root: String,
    #[serde(default)]
    pub name: Option<String>,
    pub base_branch: String,
    pub worktree_home: String,
    pub branch_template: String,
}

/// Add a Project to the registry. Re-validates the repo path at this seam
/// (it must still canonicalize into a git work tree) and normalizes it to
/// the git toplevel; the pure core validates every field and rejects
/// duplicates before anything is persisted.
#[tauri::command]
pub fn helm_add_project(
    registry: State<'_, RegistryState>,
    input: AddProjectInput,
) -> Result<Project, String> {
    let root = resolve_repo_root(&input.repo_root)?;
    let repo_root = crate::modules::fs::to_canon(&root);
    let name = input
        .name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| repo_name(&repo_root))
        .ok_or("cannot derive a project name from the repo path")?;
    let project = Project {
        id: next_project_id(),
        name,
        repo_root,
        base_branch: input.base_branch,
        worktree_home: input.worktree_home,
        branch_template: input.branch_template,
    };
    let added = project.clone();
    registry.commit(Event::ProjectAdded { project })?;
    Ok(added)
}

/// List all registered Projects.
#[tauri::command]
pub fn helm_list_projects(registry: State<'_, RegistryState>) -> Result<Vec<Project>, String> {
    Ok(registry.snapshot()?.projects)
}

/// Opaque, unique-per-process registry id. Uniqueness across restarts
/// comes from the millisecond timestamp; the pure core still rejects
/// duplicate ids as a final guard.
fn next_project_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "prj-{millis:x}-{:x}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::next_project_id;

    #[test]
    fn project_ids_are_unique_within_a_process() {
        let a = next_project_id();
        let b = next_project_id();
        assert_ne!(a, b);
        assert!(a.starts_with("prj-"));
    }
}
