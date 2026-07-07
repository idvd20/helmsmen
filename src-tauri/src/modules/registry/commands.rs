//! Tauri command glue for the Helmsmen registry — deliberately thin.
//!
//! Boundary validation happens here (paths re-resolved against the real
//! filesystem) and in the pure core (`apply` re-validates every field as
//! data), so a hostile frontend payload can neither register a phantom
//! clone nor smuggle an invalid entity into the registry.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tauri::State;

use crate::modules::core::profile::Profile;
use crate::modules::core::project::{repo_name, Project};
use crate::modules::core::settings::ProjectSettings;
use crate::modules::core::state::Event;
use crate::modules::core::workspace::Workspace;
use crate::modules::workspace::WorkspaceRegistry;

use super::detect::{detect_project, resolve_repo_root, ProjectDetection};
use super::pipeline;
use super::worktree::{self, CutWorkspace};
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
        // Settings start empty and are edited afterwards; Profiles are
        // seeded by the pure core when this event is applied.
        settings: Default::default(),
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

/// Replace a Project's settings: setup script, carry-over globs, Process
/// definitions. Definitions only at this slice — nothing is executed
/// here (the cut pipeline runs them, task #8). The pure core validates
/// every field; storage is Helmsmen's registry, never a file in the repo.
#[tauri::command]
pub fn helm_update_project_settings(
    registry: State<'_, RegistryState>,
    project_id: String,
    settings: ProjectSettings,
) -> Result<Project, String> {
    update_project_settings(&registry, project_id, settings)
}

pub(crate) fn update_project_settings(
    registry: &RegistryState,
    project_id: String,
    settings: ProjectSettings,
) -> Result<Project, String> {
    let next = registry.commit(Event::ProjectSettingsUpdated {
        project_id: project_id.clone(),
        settings,
    })?;
    next.projects
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| format!("no project with id {project_id:?}"))
}

/// List Profiles — all of them, or one Project's seeded copies.
#[tauri::command]
pub fn helm_list_profiles(
    registry: State<'_, RegistryState>,
    project_id: Option<String>,
) -> Result<Vec<Profile>, String> {
    let profiles = registry.snapshot()?.profiles;
    Ok(match project_id {
        Some(id) => profiles.into_iter().filter(|p| p.project_id == id).collect(),
        None => profiles,
    })
}

/// Edit a Project-owned Profile (full replacement by id). Divergence is
/// per Project by construction: the event can only touch one Profile row
/// and the core pins its `project_id`.
#[tauri::command]
pub fn helm_update_profile(
    registry: State<'_, RegistryState>,
    profile: Profile,
) -> Result<Profile, String> {
    update_profile(&registry, profile)
}

pub(crate) fn update_profile(
    registry: &RegistryState,
    profile: Profile,
) -> Result<Profile, String> {
    // Boundary check at the seam: the referenced Harness must exist in
    // code. The pure core validates only the shape (exactly one
    // non-empty id) so it stays independent of the harness layer.
    if crate::modules::harness::by_id(&profile.harness_id).is_none() {
        return Err(format!("unknown harness {:?}", profile.harness_id));
    }
    registry.commit(Event::ProfileUpdated {
        profile: profile.clone(),
    })?;
    Ok(profile)
}

/// The cut form payload: which Project, and the slug that fills the
/// branch template and names the worktree directory.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutWorkspaceInput {
    pub project_id: String,
    pub slug: String,
}

/// Cut a Workspace (task #5): worktree + branch off base with the branch
/// template, Slot = lowest free integer in the Project, worktree path
/// authorized as a Terax workspace root, `HELMSMEN_*` env assembled.
#[tauri::command]
pub fn helm_cut_workspace(
    registry: State<'_, RegistryState>,
    roots: State<'_, WorkspaceRegistry>,
    input: CutWorkspaceInput,
) -> Result<CutWorkspace, String> {
    worktree::cut(&registry, &roots, &input.project_id, &input.slug)
}

/// The cut-pipeline form payload (task #8): Project, slug, the Profile
/// the first Session launches under, the Brief, and the optional fetch.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutPipelineInput {
    pub project_id: String,
    pub slug: String,
    pub profile_id: String,
    #[serde(default)]
    pub brief: String,
    #[serde(default)]
    pub fetch: bool,
}

/// Cut a Workspace through the full ambient pipeline (task #8). The
/// command only enqueues — boundary validation, Slot allocation,
/// `HELMSMEN_*` env, one registry commit — and returns the Cutting
/// Workspace immediately; every slow step (fetch, worktree add,
/// authorize, carry-overs, setup script, harness wiring, first Agent
/// Session) runs on a background thread and any failure parks the
/// Workspace in Needs you with that step's log. The UI never blocks.
#[tauri::command]
pub fn helm_cut_pipeline(
    app: tauri::AppHandle,
    registry: State<'_, RegistryState>,
    input: CutPipelineInput,
) -> Result<Workspace, String> {
    let enqueued = pipeline::enqueue(
        &registry,
        &pipeline::CutRequest {
            project_id: input.project_id,
            slug: input.slug,
            profile_id: input.profile_id,
            brief: input.brief,
            fetch: input.fetch,
        },
    )?;
    let workspace = enqueued.workspace.clone();

    // Fully ambient from here: the pipeline owns its own thread and
    // reports only through registry events (parked or completed).
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager;
        let registry = app.state::<RegistryState>();
        let roots = app.state::<WorkspaceRegistry>();
        let runtimes = app.state::<crate::modules::runtime::RuntimeState>();
        // The per-Workspace control-plane endpoints (task #16): the pipeline
        // starts one here for a control-plane-hooks Harness and keeps it live
        // for the Workspace's lifetime.
        let endpoints = app.state::<crate::modules::hooks::EndpointRegistry>();
        // Both resolutions were verified at enqueue / are compiled-in;
        // failing here means the app is misassembled — still park rather
        // than lose the cut silently.
        let runtime = match runtimes.get(crate::modules::runtime::LOCAL_PTY) {
            Ok(runtime) => runtime,
            Err(e) => {
                return pipeline::park_unlaunchable(&registry, &enqueued, e);
            }
        };
        let Some(harness) = crate::modules::harness::by_id(&enqueued.profile.harness_id) else {
            return pipeline::park_unlaunchable(
                &registry,
                &enqueued,
                format!("unknown harness {:?}", enqueued.profile.harness_id),
            );
        };
        pipeline::run(&registry, &roots, runtime.as_ref(), harness, &endpoints, &enqueued);
    });
    Ok(workspace)
}

/// Remove a Workspace: delete worktree and branch, free the Slot, update
/// the registry, and drop its control-plane endpoint (task #16) so the
/// loopback listener does not outlive the Workspace.
#[tauri::command]
pub fn helm_remove_workspace(
    registry: State<'_, RegistryState>,
    endpoints: State<'_, crate::modules::hooks::EndpointRegistry>,
    workspace_id: String,
) -> Result<(), String> {
    worktree::remove(&registry, &workspace_id)?;
    // Only after the git + registry teardown succeeds: a failed remove leaves
    // the Workspace live, so its endpoint must stay too.
    endpoints.remove(&workspace_id);
    Ok(())
}

/// List all live Workspaces.
#[tauri::command]
pub fn helm_list_workspaces(registry: State<'_, RegistryState>) -> Result<Vec<Workspace>, String> {
    Ok(registry.snapshot()?.workspaces)
}

/// The `HELMSMEN_*` env for one Workspace — the set everything spawned in
/// it (setup script, Processes, Agent Sessions) receives.
#[tauri::command]
pub fn helm_workspace_env(
    registry: State<'_, RegistryState>,
    workspace_id: String,
) -> Result<BTreeMap<String, String>, String> {
    worktree::workspace_env(&registry, &workspace_id)
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
    use super::*;

    #[test]
    fn project_ids_are_unique_within_a_process() {
        let a = next_project_id();
        let b = next_project_id();
        assert_ne!(a, b);
        assert!(a.starts_with("prj-"));
    }

    // --- task #7: the settings/Profile seam over a real registry ---

    fn seeded_registry() -> (tempfile::TempDir, RegistryState) {
        let dir = tempfile::tempdir().unwrap();
        let reg = RegistryState::load(dir.path().join("helmsmen"));
        reg.commit(Event::ProjectAdded {
            project: Project {
                id: "prj-1".to_string(),
                name: "demo".to_string(),
                repo_root: "/home/dev/src/demo".to_string(),
                base_branch: "main".to_string(),
                worktree_home: "/home/dev/.helmsmen/worktrees/demo".to_string(),
                branch_template: "helm/{slug}".to_string(),
                settings: Default::default(),
            },
        })
        .unwrap();
        (dir, reg)
    }

    #[test]
    fn update_project_settings_returns_the_updated_project() {
        let (_dir, reg) = seeded_registry();
        let settings = ProjectSettings {
            setup_script: "make setup".to_string(),
            carry_over_globs: vec![".env*".to_string()],
            processes: vec![],
        };
        let project =
            update_project_settings(&reg, "prj-1".to_string(), settings.clone()).unwrap();
        assert_eq!(project.settings, settings);
        assert_eq!(reg.snapshot().unwrap().projects[0].settings, settings);
    }

    #[test]
    fn update_profile_rejects_an_unknown_harness_at_the_seam() {
        let (_dir, reg) = seeded_registry();
        let mut profile = reg.snapshot().unwrap().profiles[0].clone();
        // Shape-valid for the pure core, but no such Harness exists in
        // code — the seam must refuse before anything is committed.
        profile.harness_id = "ghost-harness".to_string();
        let err = update_profile(&reg, profile).unwrap_err();
        assert!(err.contains("ghost-harness"), "unexpected error: {err}");
        assert!(reg
            .snapshot()
            .unwrap()
            .profiles
            .iter()
            .all(|p| p.harness_id == "claude-code"));
    }

    #[test]
    fn update_profile_commits_a_registered_harness_edit() {
        let (_dir, reg) = seeded_registry();
        let mut profile = reg.snapshot().unwrap().profiles[0].clone();
        profile.verify_command = "pnpm test".to_string();
        let updated = update_profile(&reg, profile.clone()).unwrap();
        assert_eq!(updated, profile);
        let stored = reg.snapshot().unwrap();
        let got = stored.profiles.iter().find(|p| p.id == profile.id).unwrap();
        assert_eq!(got.verify_command, "pnpm test");
    }
}
