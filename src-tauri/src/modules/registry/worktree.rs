//! Cut / remove a Workspace — thin git glue around the pure core (task #5).
//!
//! The pipeline start, in the PRD's order: `git worktree add` off the
//! Project's base branch with the branch template → authorize the worktree
//! path as a Terax workspace root → allocate the Slot and assemble the
//! `HELMSMEN_*` env. All decisions (slot rule, branch expansion, path
//! shape, uniqueness) are pure functions in `modules::core::workspace`;
//! this module only runs git and commits events.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::modules::core::project::{validate_abs_path, validate_ref_name};
use crate::modules::core::state::Event;
use crate::modules::core::workspace::{
    expand_branch_template, helmsmen_env, lowest_free_slot, validate_slug, worktree_path,
    Workspace,
};
use crate::modules::workspace::WorkspaceRegistry;

use super::RegistryState;

/// Serializes cut/remove so two concurrent cuts cannot compute the same
/// Slot from the same snapshot (the pure core would reject the loser, but
/// only after its worktree was created and had to be torn down again).
static CUT_LOCK: Mutex<()> = Mutex::new(());

/// What a cut hands back: the live Workspace plus the assembled
/// `HELMSMEN_*` env every later pipeline step spawns with.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CutWorkspace {
    pub workspace: Workspace,
    pub env: BTreeMap<String, String>,
}

/// Cut a Workspace: worktree + branch off base, workspace-root
/// authorization, Slot, env, registry commit. Any failure after the
/// worktree exists tears it down again — no silently broken worktree.
pub fn cut(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    project_id: &str,
    slug: &str,
) -> Result<CutWorkspace, String> {
    let _guard = CUT_LOCK.lock().expect("cut lock poisoned");

    // Boundary validation before any side effect: the slug is hostile
    // frontend input until proven otherwise.
    validate_slug("slug", slug).map_err(|e| e.to_string())?;

    let state = registry.snapshot()?;
    let project = state
        .projects
        .iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| format!("no project with id {project_id:?}"))?
        .clone();

    let slot = lowest_free_slot(&state.workspaces, &project.id);
    let branch = expand_branch_template(&project.branch_template, slug, slot);
    // The template was validated at Project-add; re-validate the concrete
    // expansion at this seam before it reaches a git command line.
    validate_ref_name("branch", &branch).map_err(|e| e.to_string())?;
    let path = worktree_path(&project.worktree_home, slug, slot);
    // Registry data is not trusted blindly either: a tampered registry
    // file must not steer the worktree outside its home (reject `..`).
    validate_abs_path("worktreePath", &path).map_err(|e| e.to_string())?;
    if Path::new(&path).exists() {
        return Err(format!("worktree path already exists: {path}"));
    }

    std::fs::create_dir_all(&project.worktree_home)
        .map_err(|e| format!("cannot create {}: {e}", project.worktree_home))?;

    // 1. `git worktree add` off base with the branch template applied.
    run_git(
        Path::new(&project.repo_root),
        &["worktree", "add", "-b", &branch, &path, &project.base_branch],
    )?;

    let cleanup = |reason: String| {
        remove_worktree_best_effort(&project.repo_root, &path, &branch);
        reason
    };

    // Store what the directory really is, not what we asked for (macOS
    // tempdirs, symlinked homes).
    let canonical = std::fs::canonicalize(&path)
        .map(|p| crate::modules::fs::to_canon(&p))
        .map_err(|e| cleanup(format!("cannot resolve created worktree {path:?}: {e}")))?;

    // 2. Authorize exactly the worktree path as a Terax workspace root —
    // no other path gains permissions. (If a later step fails, the
    // in-memory root goes stale but points at a deleted directory, which
    // every workspace-root check re-canonicalizes and therefore rejects.)
    roots
        .authorize(&canonical)
        .map_err(|e| cleanup(format!("cannot authorize workspace root {canonical:?}: {e}")))?;

    // 3. Slot + entity through the pure core; persist atomically.
    let workspace = Workspace {
        id: next_workspace_id(),
        project_id: project.id.clone(),
        slug: slug.to_string(),
        branch: branch.clone(),
        worktree_path: canonical,
        slot,
    };
    registry
        .commit(Event::WorkspaceCut {
            workspace: workspace.clone(),
        })
        .map_err(cleanup)?;

    let env = helmsmen_env(&project, &workspace);
    Ok(CutWorkspace { workspace, env })
}

/// Remove a Workspace: delete worktree and branch, free the Slot, update
/// the registry. Tolerates a worktree or branch that is already gone
/// (manual cleanup) so removal stays retryable; the registry entry only
/// disappears once the git state is verifiably clean.
pub fn remove(registry: &RegistryState, workspace_id: &str) -> Result<(), String> {
    let _guard = CUT_LOCK.lock().expect("cut lock poisoned");

    let state = registry.snapshot()?;
    let workspace = state
        .workspaces
        .iter()
        .find(|w| w.id == workspace_id)
        .ok_or_else(|| format!("no workspace with id {workspace_id:?}"))?
        .clone();
    let project = state
        .projects
        .iter()
        .find(|p| p.id == workspace.project_id)
        .ok_or_else(|| format!("no project with id {:?}", workspace.project_id))?
        .clone();

    // Boundary re-check before the stored path reaches a git command line.
    validate_abs_path("worktreePath", &workspace.worktree_path).map_err(|e| e.to_string())?;

    let repo_root = Path::new(&project.repo_root);
    if Path::new(&workspace.worktree_path).exists() {
        run_git(
            repo_root,
            &["worktree", "remove", "--force", &workspace.worktree_path],
        )?;
    } else {
        // Directory already gone: let git drop the stale bookkeeping.
        let _ = run_git(repo_root, &["worktree", "prune"]);
    }

    if branch_exists(repo_root, &workspace.branch) {
        run_git(repo_root, &["branch", "-D", &workspace.branch])?;
    }

    registry.commit(Event::WorkspaceRemoved {
        workspace_id: workspace.id,
    })?;
    Ok(())
}

/// Look a Workspace up and assemble its `HELMSMEN_*` env (pure; exposed so
/// later spawn glue and the dev console read one seam).
pub fn workspace_env(
    registry: &RegistryState,
    workspace_id: &str,
) -> Result<BTreeMap<String, String>, String> {
    let state = registry.snapshot()?;
    let workspace = state
        .workspaces
        .iter()
        .find(|w| w.id == workspace_id)
        .ok_or_else(|| format!("no workspace with id {workspace_id:?}"))?;
    let project = state
        .projects
        .iter()
        .find(|p| p.id == workspace.project_id)
        .ok_or_else(|| format!("no project with id {:?}", workspace.project_id))?;
    Ok(helmsmen_env(project, workspace))
}

/// Tear-down for a failed cut. Best effort: the cut error stays the
/// user-visible one, cleanup failures are only logged.
fn remove_worktree_best_effort(repo_root: &str, path: &str, branch: &str) {
    let root = Path::new(repo_root);
    if let Err(e) = run_git(root, &["worktree", "remove", "--force", path]) {
        log::warn!("cut rollback: cannot remove worktree {path:?}: {e}");
    }
    if branch_exists(root, branch) {
        if let Err(e) = run_git(root, &["branch", "-D", branch]) {
            log::warn!("cut rollback: cannot delete branch {branch:?}: {e}");
        }
    }
}

fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    run_git(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")],
    )
    .is_ok()
}

/// Run git with `-C dir`; a non-zero exit is an error carrying stderr (the
/// cut pipeline surfaces it as the failing step's log).
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    crate::modules::proc::hide_console(&mut cmd);
    let output = cmd.output().map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Opaque, unique-per-process registry id (same scheme as project ids);
/// the pure core still rejects duplicates as a final guard.
fn next_workspace_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("ws-{millis:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::project::Project;
    use crate::modules::core::state::Event;

    /// A real git repo with one commit on `main`, a RegistryState over a
    /// tempdir, and a Project registered for the repo.
    struct Fixture {
        _tmp: tempfile::TempDir,
        registry: RegistryState,
        roots: WorkspaceRegistry,
        repo_root: String,
        worktree_home: String,
        project_id: String,
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git must be runnable in tests");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    fn fixture_with_template(branch_template: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        let repo_root = crate::modules::fs::to_canon(std::fs::canonicalize(&repo).unwrap());
        let worktree_home = crate::modules::fs::to_canon(tmp.path().join("wt"));

        let registry = RegistryState::load(tmp.path().join("appdata"));
        registry
            .commit(Event::ProjectAdded {
                project: Project {
                    id: "prj-1".to_string(),
                    name: "demo".to_string(),
                    repo_root: repo_root.clone(),
                    base_branch: "main".to_string(),
                    worktree_home: worktree_home.clone(),
                    branch_template: branch_template.to_string(),
                    settings: Default::default(),
                },
            })
            .unwrap();

        Fixture {
            _tmp: tmp,
            registry,
            roots: WorkspaceRegistry::default(),
            repo_root,
            worktree_home,
            project_id: "prj-1".to_string(),
        }
    }

    fn fixture() -> Fixture {
        fixture_with_template("helm/{slug}")
    }

    fn worktree_list(repo_root: &str) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    // --- AC: cut creates a worktree + branch off base with the template ---

    #[test]
    fn cut_creates_worktree_and_branch_off_base() {
        let f = fixture();
        let cutws = cut(&f.registry, &f.roots, &f.project_id, "fix-login").unwrap();

        assert_eq!(cutws.workspace.branch, "helm/fix-login");
        assert_eq!(cutws.workspace.slot, 1);
        assert!(
            Path::new(&cutws.workspace.worktree_path).is_dir(),
            "worktree directory must exist"
        );
        assert!(
            worktree_list(&f.repo_root).contains("refs/heads/helm/fix-login"),
            "git must list the new worktree on its branch"
        );
        // Cut off base: the new branch points at main's commit.
        let base = Command::new("git")
            .arg("-C")
            .arg(&f.repo_root)
            .args(["rev-parse", "main"])
            .output()
            .unwrap();
        let cut_tip = Command::new("git")
            .arg("-C")
            .arg(&f.repo_root)
            .args(["rev-parse", "helm/fix-login"])
            .output()
            .unwrap();
        assert_eq!(base.stdout, cut_tip.stdout);
        // Registry updated.
        let state = f.registry.snapshot().unwrap();
        assert_eq!(state.workspaces, vec![cutws.workspace]);
    }

    #[test]
    fn branch_template_with_slot_placeholder_expands() {
        let f = fixture_with_template("helm/{slug}-{slot}");
        let cutws = cut(&f.registry, &f.roots, &f.project_id, "fix").unwrap();
        assert_eq!(cutws.workspace.branch, "helm/fix-1");
    }

    // --- AC: Slot rule + HELMSMEN_* env assembly ---

    #[test]
    fn slots_count_up_and_env_is_assembled() {
        let f = fixture();
        let a = cut(&f.registry, &f.roots, &f.project_id, "a").unwrap();
        let b = cut(&f.registry, &f.roots, &f.project_id, "b").unwrap();
        assert_eq!((a.workspace.slot, b.workspace.slot), (1, 2));

        assert_eq!(b.env["HELMSMEN_SLOT"], "2");
        assert_eq!(b.env["HELMSMEN_WORKSPACE"], b.workspace.worktree_path);
        assert_eq!(b.env["HELMSMEN_PROJECT"], "demo");
        assert_eq!(b.env["HELMSMEN_MAIN_CHECKOUT"], f.repo_root);
        assert_eq!(b.env.len(), 4, "exactly the specced HELMSMEN_* set");

        // The same env is readable later for everything spawned in the
        // Workspace.
        assert_eq!(
            workspace_env(&f.registry, &b.workspace.id).unwrap(),
            b.env
        );
    }

    // --- AC: authorization scoped to exactly the worktree path ---

    #[test]
    fn cut_authorizes_only_the_worktree_path() {
        let f = fixture();
        let cutws = cut(&f.registry, &f.roots, &f.project_id, "fix").unwrap();

        let wt = Path::new(&cutws.workspace.worktree_path);
        assert!(f.roots.is_authorized(wt), "worktree root must be authorized");
        assert!(
            !f.roots.is_authorized(Path::new(&f.repo_root)),
            "the main checkout must not gain permissions from a cut"
        );
        assert!(
            !f.roots.is_authorized(Path::new(&f.worktree_home)),
            "the worktree home (parent) must not gain permissions"
        );
        assert!(
            !f.roots.is_authorized(wt.parent().unwrap().join("other-1").as_path()),
            "sibling paths must not gain permissions"
        );
    }

    // --- AC: removal deletes worktree + branch, frees Slot, updates registry ---

    #[test]
    fn removal_cleans_git_and_registry_and_frees_the_slot() {
        let f = fixture();
        let a = cut(&f.registry, &f.roots, &f.project_id, "a").unwrap();
        let b = cut(&f.registry, &f.roots, &f.project_id, "b").unwrap();

        remove(&f.registry, &a.workspace.id).unwrap();

        assert!(
            !Path::new(&a.workspace.worktree_path).exists(),
            "worktree directory must be deleted"
        );
        assert!(
            !worktree_list(&f.repo_root).contains("refs/heads/helm/a"),
            "git must no longer list the removed worktree"
        );
        assert!(
            !branch_exists(Path::new(&f.repo_root), "helm/a"),
            "branch must be deleted"
        );
        let state = f.registry.snapshot().unwrap();
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].id, b.workspace.id);

        // Slot 1 is free again and reused by the next cut.
        let c = cut(&f.registry, &f.roots, &f.project_id, "c").unwrap();
        assert_eq!(c.workspace.slot, 1);
    }

    #[test]
    fn removal_tolerates_a_manually_deleted_worktree() {
        let f = fixture();
        let a = cut(&f.registry, &f.roots, &f.project_id, "a").unwrap();
        std::fs::remove_dir_all(&a.workspace.worktree_path).unwrap();

        remove(&f.registry, &a.workspace.id).unwrap();

        assert!(!branch_exists(Path::new(&f.repo_root), "helm/a"));
        assert!(f.registry.snapshot().unwrap().workspaces.is_empty());
    }

    #[test]
    fn removing_an_unknown_workspace_errors() {
        let f = fixture();
        let err = remove(&f.registry, "ws-ghost").unwrap_err();
        assert!(err.contains("ws-ghost"), "got: {err}");
    }

    // --- AC: paths / input validated at the boundary ---

    #[test]
    fn hostile_slugs_are_rejected_before_any_side_effect() {
        let f = fixture();
        for bad in ["", "../escape", "a/b", "-flag", "has space", "a..b"] {
            let err = cut(&f.registry, &f.roots, &f.project_id, bad)
                .expect_err(&format!("slug {bad:?} must be rejected"));
            assert!(err.contains("slug"), "got: {err}");
        }
        // No worktree, branch, or registry entry came into being.
        assert_eq!(worktree_list(&f.repo_root).matches("worktree ").count(), 1);
        assert!(f.registry.snapshot().unwrap().workspaces.is_empty());
        assert!(!Path::new(&f.worktree_home).exists());
    }

    #[test]
    fn unknown_project_is_rejected() {
        let f = fixture();
        let err = cut(&f.registry, &f.roots, "prj-ghost", "fix").unwrap_err();
        assert!(err.contains("prj-ghost"), "got: {err}");
    }

    // --- failure handling: no silently broken worktree ---

    #[test]
    fn failed_cut_leaves_no_worktree_branch_or_registry_entry() {
        let f = fixture();
        // Point the Project at a base branch that does not exist.
        {
            let missing_base = Project {
                id: "prj-2".to_string(),
                name: "demo2".to_string(),
                repo_root: f.repo_root.clone(),
                base_branch: "no-such-branch".to_string(),
                worktree_home: f.worktree_home.clone(),
                branch_template: "helm/{slug}".to_string(),
                settings: Default::default(),
            };
            // Same repo root is rejected by the core, so use a second repo.
            let tmp2 = tempfile::tempdir().unwrap();
            let repo2 = tmp2.path().join("repo2");
            std::fs::create_dir_all(&repo2).unwrap();
            git(&repo2, &["init", "-b", "main", "."]);
            let repo2_root =
                crate::modules::fs::to_canon(std::fs::canonicalize(&repo2).unwrap());
            f.registry
                .commit(Event::ProjectAdded {
                    project: Project {
                        repo_root: repo2_root.clone(),
                        ..missing_base
                    },
                })
                .unwrap();

            let err = cut(&f.registry, &f.roots, "prj-2", "fix").unwrap_err();
            assert!(err.contains("git"), "must surface the git error, got: {err}");
            assert!(f.registry.snapshot().unwrap().workspaces.is_empty());
            assert!(
                !worktree_list(&repo2_root).contains("helm/fix"),
                "no worktree may survive a failed cut"
            );
        }
    }

    #[test]
    fn cut_refuses_an_existing_worktree_path() {
        let f = fixture();
        let path = worktree_path(&f.worktree_home, "fix", 1);
        std::fs::create_dir_all(&path).unwrap();
        let err = cut(&f.registry, &f.roots, &f.project_id, "fix").unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");
    }

    #[test]
    fn duplicate_live_slug_is_rejected_cleanly() {
        // Default template has no {slot}: the same slug would collide on
        // the branch; the second cut must fail without side effects.
        let f = fixture();
        cut(&f.registry, &f.roots, &f.project_id, "fix").unwrap();
        let err = cut(&f.registry, &f.roots, &f.project_id, "fix").unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(f.registry.snapshot().unwrap().workspaces.len(), 1);
    }

    #[test]
    fn workspace_ids_are_unique_within_a_process() {
        let a = next_workspace_id();
        let b = next_workspace_id();
        assert_ne!(a, b);
        assert!(a.starts_with("ws-"));
    }
}
