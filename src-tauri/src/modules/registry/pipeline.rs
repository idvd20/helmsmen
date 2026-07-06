//! The ambient cut pipeline (task #8) — imperative shell sequencing the
//! existing seams, in the PRD's order: fetch (optional) → `git worktree
//! add` off base with the branch template → authorize the workspace root
//! → copy carry-over globs → run the setup script (user's shell, cwd =
//! worktree) → write harness wiring (task #16: a Harness with the
//! `control_plane_hooks` Cap gets a per-Workspace loopback endpoint started
//! and its hook config written through the `Harness::config_injection`
//! seam; a Signal-only Harness keeps agent-signal) → launch the first Agent
//! Session (Harness launch command, Profile model, opening prompt =
//! snippet + Brief).
//!
//! Split into [`enqueue`] (fast: boundary validation, Slot allocation,
//! `HELMSMEN_*` env assembly, one registry commit — the only part a
//! command waits for) and [`run`] (slow: every effectful step, on a
//! background thread). Any step failure parks the Workspace as Blocked
//! ("Needs you") with that step's log via a pure-core event; the cut
//! never holds the user's attention and never leaves a silently broken
//! worktree — every effect is either recorded on the Workspace or torn
//! down.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::modules::core::cut::{compose_opening_prompt, validate_brief, CutState, CutStep};
use crate::modules::core::profile::Profile;
use crate::modules::core::project::{validate_abs_path, validate_ref_name, Project};
use crate::modules::core::state::Event;
use crate::modules::core::workspace::{
    expand_branch_template, helmsmen_env, lowest_free_slot, validate_slug, worktree_path,
    Workspace,
};
use crate::modules::harness::{ControlPlaneWiring, Harness, LaunchContext};
use crate::modules::hooks::EndpointRegistry;
use crate::modules::runtime::spawn::{apply_config_injection, prepare_spawn_with, LaunchOverrides};
use crate::modules::runtime::{OutputSink, Runtime};
use crate::modules::workspace::WorkspaceRegistry;

use super::worktree::{next_workspace_id, remove_worktree_best_effort, run_git, CUT_LOCK};
use super::RegistryState;

/// Terminal size for the ambient first Session (same defaults as
/// `helm_spawn_agent`); the frontend resizes on attach.
const AMBIENT_COLS: u16 = 120;
const AMBIENT_ROWS: u16 = 32;

/// What the user asked for: which Project, the slug, the Profile the
/// first Session launches under, the Brief, and whether to fetch first.
#[derive(Debug, Clone)]
pub struct CutRequest {
    pub project_id: String,
    pub slug: String,
    pub profile_id: String,
    pub brief: String,
    /// The PRD's optional first step: `git fetch` the main checkout so
    /// the cut comes off a fresh base.
    pub fetch: bool,
}

/// Everything [`run`] needs, resolved and validated by [`enqueue`]. When
/// this exists the Workspace is already committed (phase Cutting) with
/// its Slot reserved and its `HELMSMEN_*` env assembled.
#[derive(Debug)]
pub struct EnqueuedCut {
    pub workspace: Workspace,
    /// The `HELMSMEN_*` set every spawned step carries.
    pub env: BTreeMap<String, String>,
    pub project: Project,
    pub profile: Profile,
    /// The Profile's prompt snippet with the Brief composed in.
    pub opening_prompt: String,
    pub fetch: bool,
}

/// The fast half of a cut: validate at the boundary, allocate the Slot,
/// assemble the env, commit the Workspace as Cutting. This is all a
/// command blocks on; failures here reject the cut synchronously, before
/// any Workspace exists (nothing to park, nothing on disk but the
/// worktree home directory).
pub fn enqueue(registry: &RegistryState, req: &CutRequest) -> Result<EnqueuedCut, String> {
    // Boundary validation before any side effect: slug and Brief are
    // hostile frontend input until proven otherwise.
    validate_slug("slug", &req.slug).map_err(|e| e.to_string())?;
    validate_brief(&req.brief).map_err(|e| e.to_string())?;

    // Serialized with cut/remove so two concurrent enqueues cannot
    // compute the same Slot from the same snapshot.
    let _guard = CUT_LOCK.lock().expect("cut lock poisoned");

    let state = registry.snapshot()?;
    let project = state
        .projects
        .iter()
        .find(|p| p.id == req.project_id)
        .ok_or_else(|| format!("no project with id {:?}", req.project_id))?
        .clone();
    let profile = state
        .profiles
        .iter()
        .find(|p| p.id == req.profile_id)
        .ok_or_else(|| format!("no profile with id {:?}", req.profile_id))?
        .clone();
    if profile.project_id != project.id {
        return Err(format!(
            "profile {:?} belongs to another project",
            req.profile_id
        ));
    }
    // The Harness must exist in code before anything is committed.
    if crate::modules::harness::by_id(&profile.harness_id).is_none() {
        return Err(format!("unknown harness {:?}", profile.harness_id));
    }

    let slot = lowest_free_slot(&state.workspaces, &project.id);
    let branch = expand_branch_template(&project.branch_template, &req.slug, slot);
    validate_ref_name("branch", &branch).map_err(|e| e.to_string())?;

    // Create and canonicalize the worktree home now, so the committed
    // worktree path is already canonical: `git worktree add` later
    // creates the leaf directory itself (fresh, never a symlink), and
    // canonical(home)/leaf stays canonical.
    std::fs::create_dir_all(&project.worktree_home)
        .map_err(|e| format!("cannot create {}: {e}", project.worktree_home))?;
    let home = std::fs::canonicalize(&project.worktree_home)
        .map(|p| crate::modules::fs::to_canon(&p))
        .map_err(|e| format!("cannot resolve {}: {e}", project.worktree_home))?;
    let path = worktree_path(&home, &req.slug, slot);
    // Registry data is not trusted blindly: a tampered worktree home must
    // not steer the path outside itself.
    validate_abs_path("worktreePath", &path).map_err(|e| e.to_string())?;
    if Path::new(&path).exists() {
        return Err(format!("worktree path already exists: {path}"));
    }

    let workspace = Workspace {
        id: next_workspace_id(),
        project_id: project.id.clone(),
        slug: req.slug.clone(),
        branch,
        worktree_path: path,
        slot,
        cut: CutState::Cutting,
    };
    registry.commit(Event::WorkspaceCut {
        workspace: workspace.clone(),
    })?;

    let env = helmsmen_env(&project, &workspace);
    let opening_prompt = compose_opening_prompt(&profile.prompt_snippet, &req.brief);
    Ok(EnqueuedCut {
        workspace,
        env,
        project,
        profile,
        opening_prompt,
        fetch: req.fetch,
    })
}

/// The slow half of a cut: every effectful step, in order. Runs on a
/// background thread — it never returns an error, because the error path
/// IS the product: any step failure parks the Workspace as Blocked
/// ("Needs you") with that step's log and stops.
///
/// A hung setup script leaves the Workspace visibly Cutting (no timeout
/// at M2); the user can scuttle it, which [`settle`] detects and cleans
/// up after.
pub fn run(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    runtime: &dyn Runtime,
    harness: &dyn Harness,
    endpoints: &EndpointRegistry,
    cut: &EnqueuedCut,
) {
    let ws = &cut.workspace;
    let repo_root = Path::new(&cut.project.repo_root);

    // Step 1: fetch (optional) — main-checkout git, before anything is
    // created, so a failure parks with nothing to tear down.
    if cut.fetch {
        if let Err(log) = run_git(repo_root, &["fetch"]) {
            return park(registry, cut, CutStep::Fetch, log);
        }
    }

    // Step 2: `git worktree add` off base with the branch template
    // applied (the branch and path were validated at enqueue).
    if let Err(log) = run_git(
        repo_root,
        &[
            "worktree",
            "add",
            "-b",
            &ws.branch,
            &ws.worktree_path,
            &cut.project.base_branch,
        ],
    ) {
        // A failed add can leave partial bookkeeping: sweep it, then
        // park — nothing of this step survives.
        remove_worktree_best_effort(&cut.project.repo_root, &ws.worktree_path, &ws.branch);
        return park(registry, cut, CutStep::WorktreeAdd, log);
    }
    // From here on the worktree is *recorded*: it is the Workspace's own
    // registry path, and removal stays retryable — a parked cut is never
    // a silently broken worktree.

    // Step 3: authorize exactly the worktree as a Terax workspace root.
    // Store-what-it-really-is check first: the enqueue-time path was
    // canonical by construction; a directory that resolves elsewhere is
    // never authorized.
    match std::fs::canonicalize(&ws.worktree_path).map(|p| crate::modules::fs::to_canon(&p)) {
        Ok(canonical) if canonical == ws.worktree_path => {
            if let Err(e) = roots.authorize(&canonical) {
                return park(
                    registry,
                    cut,
                    CutStep::AuthorizeRoot,
                    format!("cannot authorize workspace root {canonical:?}: {e}"),
                );
            }
        }
        Ok(canonical) => {
            return park(
                registry,
                cut,
                CutStep::AuthorizeRoot,
                format!(
                    "worktree resolved to {canonical:?} but the registry recorded {:?}; \
                     refusing to authorize a diverged path",
                    ws.worktree_path
                ),
            );
        }
        Err(e) => {
            return park(
                registry,
                cut,
                CutStep::AuthorizeRoot,
                format!("cannot resolve created worktree {:?}: {e}", ws.worktree_path),
            );
        }
    }

    // Step 4: copy carry-over globs from the main checkout.
    if let Err(log) = copy_carry_overs(
        &cut.project.repo_root,
        &ws.worktree_path,
        &cut.project.settings.carry_over_globs,
    ) {
        return park(registry, cut, CutStep::CopyCarryOvers, log);
    }

    // Step 5: setup script — user's shell, cwd = worktree, HELMSMEN_* env.
    if let Err(log) = run_setup_script(&cut.project.settings.setup_script, ws, &cut.env) {
        return park(registry, cut, CutStep::SetupScript, log);
    }

    // Step 6: harness wiring (task #16). A Harness with the
    // `control_plane_hooks` Cap gets a per-Workspace loopback endpoint started
    // here and its hook settings written into the worktree, so the first
    // Session POSTs its hook events to the control plane under the session
    // bearer token. A Signal-only Harness (no Cap) starts no endpoint and
    // writes no hook config — its agent-signal path stays the status source
    // (Cap degradation). Hostile config paths are still rejected inside the
    // worktree boundary; a failure to bind parks the cut at this step.
    let endpoint = if harness.caps().control_plane_hooks {
        match endpoints.start_for(&ws.id) {
            Ok(endpoint) => Some(endpoint),
            Err(e) => {
                return park(
                    registry,
                    cut,
                    CutStep::HarnessWiring,
                    format!("cannot start control-plane endpoint: {e}"),
                );
            }
        }
    } else {
        None
    };
    // Own the url for the borrow the LaunchContext takes; the Arc endpoint
    // (hence `token()`) stays alive through the injection below, and the
    // registry keeps its own Arc so the endpoint outlives this cut.
    let endpoint_url = endpoint.as_ref().map(|e| e.url());
    let control_plane = match (endpoint.as_ref(), endpoint_url.as_deref()) {
        (Some(endpoint), Some(url)) => Some(ControlPlaneWiring {
            url,
            token: endpoint.token(),
        }),
        _ => None,
    };
    let ctx = LaunchContext {
        workspace_root: &ws.worktree_path,
        env: &cut.env,
        model: &cut.profile.model,
        opening_prompt: &cut.opening_prompt,
        control_plane,
    };
    if let Err(log) = apply_config_injection(&ws.worktree_path, &harness.config_injection(&ctx)) {
        return park(registry, cut, CutStep::HarnessWiring, log);
    }

    // Step 7: launch the first Agent Session — Harness launch command,
    // Profile model, opening prompt composed from the snippet + Brief.
    // The sink is empty on purpose: the Runtime retains scrollback until
    // the frontend attaches.
    let launched = prepare_spawn_with(
        registry,
        roots,
        &ws.id,
        harness,
        LaunchOverrides {
            model: &cut.profile.model,
            opening_prompt: &cut.opening_prompt,
        },
        AMBIENT_COLS,
        AMBIENT_ROWS,
    )
    .and_then(|spec| runtime.spawn(spec, ambient_sink()));
    let first_session_id = match launched {
        Ok(id) => id,
        Err(log) => return park(registry, cut, CutStep::LaunchSession, log),
    };

    if !settle(
        registry,
        cut,
        Event::CutCompleted {
            workspace_id: ws.id.clone(),
            first_session_id: first_session_id.clone(),
        },
    ) {
        // The Workspace vanished mid-cut (scuttled): the session just
        // launched belongs to nothing — kill it, and drop the control-plane
        // endpoint so its listener thread does not outlive the Workspace.
        let _ = runtime.kill(&first_session_id);
        endpoints.remove(&ws.id);
    }
}

/// Park an enqueued cut whose Runtime or Harness could not even be
/// resolved (app misassembly — should be unreachable, but a lost cut must
/// never be silent). Attributed to the launch step: that is the step
/// those pieces serve.
pub(crate) fn park_unlaunchable(registry: &RegistryState, cut: &EnqueuedCut, log: String) {
    park(registry, cut, CutStep::LaunchSession, log);
}

/// Park the Workspace as Blocked ("Needs you") with the failing step's
/// log. The log is data (hostile process output) and is bounded by the
/// pure core before it is stored.
fn park(registry: &RegistryState, cut: &EnqueuedCut, step: CutStep, log: String) {
    log::warn!(
        "cut of workspace {}: step {:?} failed: {log}",
        cut.workspace.id,
        step.label()
    );
    settle(
        registry,
        cut,
        Event::CutStepFailed {
            workspace_id: cut.workspace.id.clone(),
            step,
            log,
        },
    );
}

/// Commit a cut-lifecycle event. If it cannot be committed because the
/// Workspace is gone — the user scuttled it mid-cut — tear the pipeline's
/// git effects down so nothing broken survives without a registry record.
/// Returns whether the event was recorded.
fn settle(registry: &RegistryState, cut: &EnqueuedCut, event: Event) -> bool {
    let Err(commit_err) = registry.commit(event) else {
        return true;
    };
    let workspace_gone = matches!(
        registry.snapshot(),
        Ok(state) if !state.workspaces.iter().any(|w| w.id == cut.workspace.id)
    );
    if workspace_gone {
        log::warn!(
            "workspace {} was removed mid-cut; tearing the worktree down",
            cut.workspace.id
        );
        remove_worktree_best_effort(
            &cut.project.repo_root,
            &cut.workspace.worktree_path,
            &cut.workspace.branch,
        );
    } else {
        log::warn!(
            "cannot record cut lifecycle for workspace {}: {commit_err}",
            cut.workspace.id
        );
    }
    false
}

/// The ambient first Session has no viewer yet: bytes are retained by the
/// Runtime as scrollback until the frontend attaches. Output stays
/// hostile and uninterpreted either way.
fn ambient_sink() -> OutputSink {
    OutputSink {
        on_output: Box::new(|_| {}),
        on_exit: Box::new(|_| {}),
    }
}

/// Copy the Project's carry-over globs (untracked `.env*`-style files)
/// from the main checkout into the fresh worktree. The globs are
/// validated data (relative, no `..`); every match is re-checked against
/// the real filesystem to stay inside the main checkout, and the
/// destination is always the matching relative path inside the worktree.
fn copy_carry_overs(repo_root: &str, worktree: &str, globs: &[String]) -> Result<(), String> {
    for pattern in globs {
        // Escape the repo root so its own characters are never glob
        // syntax; only the user's pattern globs.
        let full = Path::new(&glob::Pattern::escape(repo_root))
            .join(pattern)
            .to_string_lossy()
            .into_owned();
        let matches = glob::glob(&full)
            .map_err(|e| format!("carry-over glob {pattern:?} is invalid: {e}"))?;
        for entry in matches {
            let source =
                entry.map_err(|e| format!("carry-over glob {pattern:?} cannot be read: {e}"))?;
            copy_carry_over_path(repo_root, worktree, &source)?;
        }
    }
    Ok(())
}

fn copy_carry_over_path(repo_root: &str, worktree: &str, source: &Path) -> Result<(), String> {
    // Boundary re-check against the real filesystem: a match that leads
    // outside the main checkout is refused, not copied.
    let rel = source
        .strip_prefix(repo_root)
        .map_err(|_| format!("carry-over match {source:?} is outside the main checkout"))?;
    let meta = source
        .symlink_metadata()
        .map_err(|e| format!("cannot stat carry-over match {source:?}: {e}"))?;
    if meta.file_type().is_symlink() {
        // Carry-overs are untracked config *files*. Symlinks are neither
        // followed nor recreated: repo contents are hostile, and a
        // planted link could otherwise pull secrets from outside the
        // checkout into the agent's worktree.
        log::warn!("carry-over: skipping symlink {source:?}");
        return Ok(());
    }
    if meta.is_dir() {
        let children = std::fs::read_dir(source)
            .map_err(|e| format!("cannot read carry-over directory {source:?}: {e}"))?;
        for child in children {
            let child = child.map_err(|e| format!("cannot read {source:?}: {e}"))?;
            copy_carry_over_path(repo_root, worktree, &child.path())?;
        }
        return Ok(());
    }
    let dest = Path::new(worktree).join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::copy(source, &dest)
        .map_err(|e| format!("cannot copy {source:?} to {}: {e}", dest.display()))?;
    Ok(())
}

/// Run the Project's setup script: one multiline command in the user's
/// shell, cwd = worktree, `HELMSMEN_*` on the environment. Output is
/// captured (hostile bytes, shipped as text); a non-zero exit fails the
/// step with the captured log.
fn run_setup_script(
    script: &str,
    workspace: &Workspace,
    env: &BTreeMap<String, String>,
) -> Result<(), String> {
    if script.trim().is_empty() {
        return Ok(());
    }
    let (shell, flag) = user_shell();
    let mut cmd = Command::new(&shell);
    cmd.arg(flag)
        .arg(script)
        .current_dir(&workspace.worktree_path)
        .envs(env);
    crate::modules::proc::hide_console(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| format!("cannot run setup script with {shell:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "setup script failed ({}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(())
}

/// The user's shell, like the PRD asks ("my shell, cwd = worktree"):
/// `$SHELL` on unix with `/bin/sh` as fallback, `%COMSPEC%` on Windows.
#[cfg(unix)]
fn user_shell() -> (String, &'static str) {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    (shell, "-c")
}

#[cfg(windows)]
fn user_shell() -> (String, &'static str) {
    let shell = std::env::var("COMSPEC")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string());
    (shell, "/C")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::modules::core::cut::{derive_status, WorkspaceStatus};
    use crate::modules::core::project::Project;
    use crate::modules::core::settings::ProjectSettings;
    use crate::modules::harness::claude_code::ClaudeCode;
    use crate::modules::harness::{Caps, ConfigFile, LaunchPlan};
    use crate::modules::registry::worktree;
    use crate::modules::runtime::local_pty::LocalPty;
    use crate::modules::runtime::{SessionStatus, SpawnSpec};

    struct Fixture {
        _tmp: tempfile::TempDir,
        registry: RegistryState,
        roots: WorkspaceRegistry,
        endpoints: EndpointRegistry,
        repo_root: String,
        worktree_home: String,
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

    fn fixture_with(base_branch: &str, settings: ProjectSettings) -> Fixture {
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
        // Untracked carry-over material in the main checkout.
        std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(repo.join(".env.local"), "LOCAL=2\n").unwrap();

        let repo_root = crate::modules::fs::to_canon(std::fs::canonicalize(&repo).unwrap());
        let worktree_home = crate::modules::fs::to_canon(tmp.path().join("wt"));
        let registry = RegistryState::load(tmp.path().join("appdata"));
        registry
            .commit(Event::ProjectAdded {
                project: Project {
                    id: "prj-1".to_string(),
                    name: "demo".to_string(),
                    repo_root: repo_root.clone(),
                    base_branch: base_branch.to_string(),
                    worktree_home: worktree_home.clone(),
                    branch_template: "helm/{slug}".to_string(),
                    settings: Default::default(),
                },
            })
            .unwrap();
        if settings != ProjectSettings::default() {
            registry
                .commit(Event::ProjectSettingsUpdated {
                    project_id: "prj-1".to_string(),
                    settings,
                })
                .unwrap();
        }
        Fixture {
            _tmp: tmp,
            registry,
            roots: WorkspaceRegistry::default(),
            endpoints: EndpointRegistry::default(),
            repo_root,
            worktree_home,
        }
    }

    fn fixture() -> Fixture {
        fixture_with("main", ProjectSettings::default())
    }

    /// The seeded Feature Profile: prompt snippet `/tdd {brief}`.
    const FEATURE: &str = "prj-1:feature";

    fn request(slug: &str, brief: &str) -> CutRequest {
        CutRequest {
            project_id: "prj-1".to_string(),
            slug: slug.to_string(),
            profile_id: FEATURE.to_string(),
            brief: brief.to_string(),
            fetch: false,
        }
    }

    /// The one Workspace's cut state, straight from the registry (what
    /// `helm_list_workspaces` serializes for the dev console / wall).
    fn cut_state(f: &Fixture) -> CutState {
        let state = f.registry.snapshot().unwrap();
        assert_eq!(state.workspaces.len(), 1, "expected exactly one workspace");
        state.workspaces[0].cut.clone()
    }

    fn parked_at(f: &Fixture) -> (CutStep, String) {
        match cut_state(f) {
            CutState::Failed { step, log } => (step, log),
            other => panic!("expected a parked cut, got {other:?}"),
        }
    }

    /// Captures every SpawnSpec instead of running anything; `kill` is
    /// recorded too. Stands in for LocalPty where the launch *plan* is
    /// the assertion.
    #[derive(Default)]
    struct RecordingRuntime {
        spawned: Mutex<Vec<SpawnSpec>>,
        fail_spawn: bool,
        killed: Mutex<Vec<String>>,
    }

    impl Runtime for RecordingRuntime {
        fn spawn(&self, spec: SpawnSpec, _sink: OutputSink) -> Result<String, String> {
            if self.fail_spawn {
                return Err("runtime refused to spawn".to_string());
            }
            let mut spawned = self.spawned.lock().unwrap();
            spawned.push(spec);
            Ok(format!("rt-{}", spawned.len()))
        }
        fn attach(&self, _s: &str, _sink: OutputSink) -> Result<(), String> {
            Err("unused".to_string())
        }
        fn write(&self, _s: &str, _b: &[u8]) -> Result<(), String> {
            Err("unused".to_string())
        }
        fn resize(&self, _s: &str, _c: u16, _r: u16) -> Result<(), String> {
            Err("unused".to_string())
        }
        fn status(&self, _s: &str) -> Result<SessionStatus, String> {
            Err("unused".to_string())
        }
        fn kill(&self, s: &str) -> Result<(), String> {
            self.killed.lock().unwrap().push(s.to_string());
            Ok(())
        }
    }

    /// A Harness whose wiring and launch plan the test controls.
    struct FakeHarness {
        wiring: Vec<ConfigFile>,
        plan: fn(&LaunchContext) -> LaunchPlan,
    }

    impl Default for FakeHarness {
        fn default() -> Self {
            FakeHarness {
                wiring: Vec::new(),
                plan: |_| LaunchPlan {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), "true".to_string()],
                },
            }
        }
    }

    impl Harness for FakeHarness {
        fn id(&self) -> &'static str {
            "fake-agent"
        }
        fn display_name(&self) -> &'static str {
            "Fake Agent"
        }
        fn caps(&self) -> Caps {
            ClaudeCode.caps()
        }
        fn launch_plan(&self, ctx: &LaunchContext) -> LaunchPlan {
            (self.plan)(ctx)
        }
        fn config_injection(&self, _ctx: &LaunchContext) -> Vec<ConfigFile> {
            self.wiring.clone()
        }
    }

    // --- AC: enqueue is the only blocking part; steps run ambient ---

    #[test]
    fn enqueue_commits_a_cutting_workspace_and_does_no_git_work() {
        let f = fixture();
        let enq = enqueue(&f.registry, &request("fix-login", "fix the login page")).unwrap();

        assert_eq!(enq.workspace.slot, 1);
        assert_eq!(enq.workspace.branch, "helm/fix-login");
        assert_eq!(enq.workspace.cut, CutState::Cutting);
        assert_eq!(cut_state(&f), CutState::Cutting);
        assert_eq!(
            derive_status(&enq.workspace),
            WorkspaceStatus::Working,
            "a cutting workspace shows as Working on the wall"
        );
        // Slot + env are settled at enqueue; every later step spawns
        // with this set.
        assert_eq!(enq.env["HELMSMEN_SLOT"], "1");
        assert_eq!(enq.env["HELMSMEN_WORKSPACE"], enq.workspace.worktree_path);
        assert_eq!(enq.env["HELMSMEN_PROJECT"], "demo");
        assert_eq!(enq.env["HELMSMEN_MAIN_CHECKOUT"], f.repo_root);
        assert_eq!(enq.env.len(), 4);
        // The Brief is already composed into the Profile snippet.
        assert_eq!(enq.opening_prompt, "/tdd fix the login page");
        // Nothing slow happened: no worktree, no branch.
        assert!(!Path::new(&enq.workspace.worktree_path).exists());
        assert!(!worktree::branch_exists(
            Path::new(&f.repo_root),
            "helm/fix-login"
        ));
    }

    #[test]
    fn the_registry_stays_responsive_while_a_cut_runs() {
        // A gated setup script holds the pipeline mid-step; meanwhile the
        // registry answers snapshots and even a second enqueue — the
        // backend equivalent of "the UI never blocks".
        let gate_script = "i=0\n\
             while [ $i -lt 200 ] && [ ! -f \"$HELMSMEN_MAIN_CHECKOUT/go\" ]; do\n\
               sleep 0.05; i=$((i+1))\n\
             done\n";
        let f = fixture_with(
            "main",
            ProjectSettings {
                setup_script: gate_script.to_string(),
                ..Default::default()
            },
        );
        let enq = enqueue(&f.registry, &request("slow", "brief")).unwrap();
        let ws_path = enq.workspace.worktree_path.clone();

        std::thread::scope(|scope| {
            let registry = &f.registry;
            let roots = &f.roots;
            let endpoints = &f.endpoints;
            let runtime = RecordingRuntime::default();
            let handle = scope.spawn(move || {
                run(registry, roots, &runtime, &ClaudeCode, endpoints, &enq);
            });

            // Wait until the pipeline is verifiably mid-flight.
            let deadline = Instant::now() + Duration::from_secs(10);
            while !Path::new(&ws_path).exists() {
                assert!(Instant::now() < deadline, "worktree never appeared");
                std::thread::sleep(Duration::from_millis(10));
            }
            // Registry reads and a second cut both go through while the
            // first cut is still Cutting.
            let state = f.registry.snapshot().unwrap();
            assert_eq!(state.workspaces[0].cut, CutState::Cutting);
            let second = enqueue(&f.registry, &request("second", "b")).unwrap();
            assert_eq!(second.workspace.slot, 2);

            // Open the gate; the first cut completes.
            std::fs::write(Path::new(&f.repo_root).join("go"), "").unwrap();
            handle.join().unwrap();
        });

        let state = f.registry.snapshot().unwrap();
        let first = state.workspaces.iter().find(|w| w.slug == "slow").unwrap();
        assert!(matches!(first.cut, CutState::Complete { .. }));
    }

    // --- AC: steps in order, each spawned step carrying HELMSMEN_* ---

    #[test]
    fn the_full_pipeline_completes_with_every_step_observable() {
        // The setup script *proves the order*: it fails unless the
        // carry-overs are already in place, and it records the env and
        // cwd it ran with.
        let f = fixture_with(
            "main",
            ProjectSettings {
                setup_script: "test -f .env || exit 9\n\
                     test -f .env.local || exit 9\n\
                     printf 'slot=%s ws=%s cwd=%s' \"$HELMSMEN_SLOT\" \"$HELMSMEN_WORKSPACE\" \"$(pwd)\" > setup-ran.txt\n"
                    .to_string(),
                carry_over_globs: vec![".env*".to_string()],
                processes: vec![],
            },
        );
        // The Profile's model must reach the launch command.
        let mut profile = f
            .registry
            .snapshot()
            .unwrap()
            .profiles
            .iter()
            .find(|p| p.id == FEATURE)
            .unwrap()
            .clone();
        profile.model = "claude-sonnet-4-5".to_string();
        f.registry
            .commit(Event::ProfileUpdated { profile })
            .unwrap();

        let enq = enqueue(&f.registry, &request("fix-login", "fix the login page")).unwrap();
        let runtime = RecordingRuntime::default();
        run(&f.registry, &f.roots, &runtime, &ClaudeCode, &f.endpoints, &enq);

        let ws = &enq.workspace;
        let wt = Path::new(&ws.worktree_path);
        assert!(wt.is_dir(), "worktree must exist");
        assert!(
            worktree::branch_exists(Path::new(&f.repo_root), "helm/fix-login"),
            "branch off base must exist"
        );
        // Authorization scoped to exactly the worktree.
        assert!(f.roots.is_authorized(wt));
        assert!(!f.roots.is_authorized(Path::new(&f.repo_root)));
        assert!(!f.roots.is_authorized(Path::new(&f.worktree_home)));
        // Carry-overs copied from the main checkout.
        assert_eq!(
            std::fs::read_to_string(wt.join(".env")).unwrap(),
            "SECRET=1\n"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "LOCAL=2\n"
        );
        // Setup script ran in the user's shell, cwd = worktree, with the
        // HELMSMEN_* env — after the carry-overs (or it would have failed).
        assert_eq!(
            std::fs::read_to_string(wt.join("setup-ran.txt")).unwrap(),
            format!("slot=1 ws={} cwd={}", ws.worktree_path, ws.worktree_path)
        );
        // First Agent Session launched: harness launch command, Profile
        // model, opening prompt = snippet + Brief; spawned in the
        // worktree with the HELMSMEN_* env.
        let spawned = runtime.spawned.lock().unwrap();
        assert_eq!(spawned.len(), 1, "exactly one first session");
        let spec = &spawned[0];
        assert_eq!(spec.program, "claude");
        assert_eq!(
            spec.args,
            vec![
                "--model=claude-sonnet-4-5".to_string(),
                "/tdd fix the login page".to_string()
            ]
        );
        assert_eq!(spec.cwd, ws.worktree_path);
        for (key, value) in &enq.env {
            assert_eq!(spec.env.get(key), Some(value), "spawn env must carry {key}");
        }
        // The registry records completion + the first session id; the
        // derived status goes back to Idle.
        assert_eq!(
            cut_state(&f),
            CutState::Complete {
                first_session_id: "rt-1".to_string()
            }
        );
        let state = f.registry.snapshot().unwrap();
        assert_eq!(derive_status(&state.workspaces[0]), WorkspaceStatus::Idle);
    }

    #[test]
    fn the_first_session_really_opens_with_the_composed_brief() {
        // End to end on a real PTY: a fake agent prints the opening
        // prompt it was launched with; the scrollback proves the first
        // Session opened with snippet + Brief.
        let f = fixture();
        let script = f._tmp.path().join("echo-agent.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf 'OPENED[%s]\\n' \"$1\"\nsleep 30\n",
        )
        .unwrap();
        let script = script.to_string_lossy().into_owned();

        struct EchoHarness {
            script: String,
        }
        impl Harness for EchoHarness {
            fn id(&self) -> &'static str {
                "fake-agent"
            }
            fn display_name(&self) -> &'static str {
                "Echo"
            }
            fn caps(&self) -> Caps {
                ClaudeCode.caps()
            }
            fn launch_plan(&self, ctx: &LaunchContext) -> LaunchPlan {
                LaunchPlan {
                    program: "/bin/sh".to_string(),
                    args: vec![self.script.clone(), ctx.opening_prompt.to_string()],
                }
            }
            fn config_injection(&self, _ctx: &LaunchContext) -> Vec<ConfigFile> {
                Vec::new()
            }
        }

        let enq = enqueue(&f.registry, &request("live", "fix the login page")).unwrap();
        let runtime = LocalPty::default();
        run(&f.registry, &f.roots, &runtime, &EchoHarness { script }, &f.endpoints, &enq);

        let CutState::Complete { first_session_id } = cut_state(&f) else {
            panic!("cut must complete, got {:?}", cut_state(&f));
        };
        // Attach to the ambient session: scrollback replays first.
        let (tx, rx) = channel::<Vec<u8>>();
        runtime
            .attach(
                &first_session_id,
                OutputSink {
                    on_output: Box::new(move |b| {
                        let _ = tx.send(b.to_vec());
                    }),
                    on_exit: Box::new(|_| {}),
                },
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("OPENED[/tdd fix the login page]") {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(chunk) => seen.extend_from_slice(&chunk),
                Err(_) => panic!(
                    "first session never showed the composed prompt; transcript: {:?}",
                    String::from_utf8_lossy(&seen)
                ),
            }
        }
        runtime.kill(&first_session_id).unwrap();
    }

    // --- AC: an induced failure at each step parks with that step's log ---

    #[test]
    fn fetch_failure_parks_before_anything_exists() {
        let f = fixture();
        // A remote that is not a repository: `git fetch` fails hard.
        git(
            Path::new(&f.repo_root),
            &["remote", "add", "origin", "/nonexistent-remote-path"],
        );
        let mut req = request("fix", "b");
        req.fetch = true;
        let enq = enqueue(&f.registry, &req).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::Fetch);
        assert!(log.contains("fetch"), "log must carry the git error: {log}");
        assert!(!Path::new(&enq.workspace.worktree_path).exists());
        assert!(!worktree::branch_exists(
            Path::new(&f.repo_root),
            "helm/fix"
        ));
        // Parked means Blocked, alias "Needs you".
        let state = f.registry.snapshot().unwrap();
        let status = derive_status(&state.workspaces[0]);
        assert_eq!(status, WorkspaceStatus::Blocked);
        assert_eq!(status.display_alias(), "Needs you");
    }

    #[test]
    fn worktree_add_failure_parks_and_leaves_no_git_debris() {
        let f = fixture_with("no-such-branch", ProjectSettings::default());
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::WorktreeAdd);
        assert!(log.contains("git"), "log must carry the git error: {log}");
        assert!(!Path::new(&enq.workspace.worktree_path).exists());
        assert!(!worktree::branch_exists(
            Path::new(&f.repo_root),
            "helm/fix"
        ));
        // The parked Workspace is removable (retry path stays open).
        worktree::remove(&f.registry, &enq.workspace.id).unwrap();
        assert!(f.registry.snapshot().unwrap().workspaces.is_empty());
    }

    #[test]
    fn a_worktree_that_resolves_away_from_its_recorded_path_parks_at_authorize() {
        let f = fixture();
        let mut enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        // Route the pipeline's own path through a symlink alias of the
        // worktree home: `git worktree add` succeeds, but the directory
        // canonicalizes to somewhere else than the pipeline recorded.
        let alias_home = f._tmp.path().join("alias-home");
        std::os::unix::fs::symlink(&f.worktree_home, &alias_home).unwrap();
        let leaf = Path::new(&enq.workspace.worktree_path)
            .file_name()
            .unwrap()
            .to_owned();
        enq.workspace.worktree_path = alias_home.join(leaf).to_string_lossy().into_owned();

        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::AuthorizeRoot);
        assert!(log.contains("diverged"), "got: {log}");
        assert!(
            !f.roots.is_authorized(Path::new(&enq.workspace.worktree_path)),
            "a diverged path must never be authorized"
        );
    }

    #[test]
    fn a_carry_over_glob_the_shell_cannot_parse_parks_the_cut() {
        // `[` passes the core's data validation (it bans traversal, not
        // glob syntax) but fails to parse at copy time.
        let f = fixture_with(
            "main",
            ProjectSettings {
                carry_over_globs: vec!["[".to_string()],
                ..Default::default()
            },
        );
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::CopyCarryOvers);
        assert!(log.contains("invalid"), "got: {log}");
        // The worktree survives, *recorded* on the parked Workspace: the
        // user inspects or scuttles it — never a silently broken tree.
        assert!(Path::new(&enq.workspace.worktree_path).is_dir());
        assert_eq!(
            f.registry.snapshot().unwrap().workspaces[0].worktree_path,
            enq.workspace.worktree_path
        );
    }

    #[test]
    fn carry_overs_never_follow_symlinks_out_of_the_checkout() {
        let f = fixture();
        // A hostile repo plants a symlink matching the user's glob.
        let outside = f._tmp.path().join("outside-secret");
        std::fs::write(&outside, "id_rsa contents").unwrap();
        std::os::unix::fs::symlink(&outside, Path::new(&f.repo_root).join(".env.link")).unwrap();
        let f2 = f; // rebind to configure settings after planting
        f2.registry
            .commit(Event::ProjectSettingsUpdated {
                project_id: "prj-1".to_string(),
                settings: ProjectSettings {
                    carry_over_globs: vec![".env*".to_string()],
                    ..Default::default()
                },
            })
            .unwrap();

        let enq = enqueue(&f2.registry, &request("fix", "b")).unwrap();
        run(
            &f2.registry,
            &f2.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f2.endpoints,
            &enq,
        );

        assert!(matches!(cut_state(&f2), CutState::Complete { .. }));
        let wt = Path::new(&enq.workspace.worktree_path);
        assert!(wt.join(".env").is_file(), "real files are carried over");
        assert!(
            !wt.join(".env.link").exists(),
            "symlinked carry-overs must be skipped, not followed"
        );
    }

    #[test]
    fn setup_script_failure_parks_with_the_captured_output() {
        let f = fixture_with(
            "main",
            ProjectSettings {
                setup_script: "echo doomed-stdout\necho doomed-stderr 1>&2\nexit 7\n".to_string(),
                ..Default::default()
            },
        );
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::SetupScript);
        assert!(log.contains("doomed-stdout"), "stdout must be attached: {log}");
        assert!(log.contains("doomed-stderr"), "stderr must be attached: {log}");
        assert!(log.contains('7'), "the exit code must be visible: {log}");
        assert!(Path::new(&enq.workspace.worktree_path).is_dir());
    }

    #[test]
    fn hostile_harness_wiring_parks_and_writes_nothing_outside() {
        let f = fixture();
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        let harness = FakeHarness {
            wiring: vec![ConfigFile {
                rel_path: "../outside-wiring".to_string(),
                contents: "evil".to_string(),
            }],
            ..Default::default()
        };
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &harness,
            &f.endpoints,
            &enq,
        );

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::HarnessWiring);
        assert!(log.contains("worktree-relative"), "got: {log}");
        assert!(!f.worktree_home.is_empty());
        assert!(
            !Path::new(&f.worktree_home).join("outside-wiring").exists(),
            "nothing may be written outside the worktree"
        );
    }

    #[test]
    fn the_wiring_seam_writes_inside_the_worktree_for_m3() {
        // The stub step is a real seam: a Harness that does inject (as
        // claude-code will at M3) gets its file placed inside the
        // worktree before launch.
        let f = fixture();
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        let harness = FakeHarness {
            wiring: vec![ConfigFile {
                rel_path: ".claude/settings.local.json".to_string(),
                contents: "{ \"hooks\": \"m3 seam\" }".to_string(),
            }],
            ..Default::default()
        };
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &harness,
            &f.endpoints,
            &enq,
        );

        assert!(matches!(cut_state(&f), CutState::Complete { .. }));
        assert_eq!(
            std::fs::read_to_string(
                Path::new(&enq.workspace.worktree_path).join(".claude/settings.local.json")
            )
            .unwrap(),
            "{ \"hooks\": \"m3 seam\" }"
        );
    }

    #[test]
    fn launch_failure_parks_with_the_runtime_error() {
        let f = fixture();
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        let runtime = RecordingRuntime {
            fail_spawn: true,
            ..Default::default()
        };
        run(&f.registry, &f.roots, &runtime, &ClaudeCode, &f.endpoints, &enq);

        let (step, log) = parked_at(&f);
        assert_eq!(step, CutStep::LaunchSession);
        assert!(log.contains("refused to spawn"), "got: {log}");
    }

    // ═══════════════════════════════════════════════════════════════════
    // task #16 — M3 hook wiring at cut: the control plane replaces
    // agent-signal for Helmsmen Workspaces; Signal-only Harnesses keep it.
    // ═══════════════════════════════════════════════════════════════════

    /// Minimal raw-HTTP client: POST `body` to the cut's loopback endpoint and
    /// return the response status code. Proves the wire path a real `claude`
    /// hook would take, with no live agent.
    fn http_post(port: u16, auth: Option<&str>, body: &str) -> u16 {
        use std::io::{Read, Write};
        use std::net::{Ipv4Addr, TcpStream};
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let auth_line = auth
            .map(|a| format!("Authorization: {a}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth_line}\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response)
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0)
    }

    /// The wall status a hook payload implies for the Workspace, hook-driven
    /// only: parse the payload the way the endpoint does, map it through the
    /// exact reducer chain the frontend mirrors, and roll it up over the cut.
    fn status_after(ws: &Workspace, payload: &str) -> WorkspaceStatus {
        use crate::modules::core::cut::{roll_up_status, session_status_from_signal};
        use crate::modules::core::control_plane::hook_event_signal;
        use crate::modules::hooks::parse_hook_event;
        let (_sid, kind) = parse_hook_event(payload.as_bytes()).expect("payload must parse");
        let session = hook_event_signal(&kind)
            .and_then(session_status_from_signal)
            .into_iter()
            .collect::<Vec<_>>();
        roll_up_status(derive_status(ws), &session)
    }

    const PRE_TOOL_USE: &str = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_x"}"#;
    const PERMISSION: &str = r#"{"session_id":"s1","hook_event_name":"Notification","notification_type":"permission_prompt"}"#;
    const STOP: &str = r#"{"session_id":"s1","hook_event_name":"Stop"}"#;

    #[test]
    fn cut_wires_claude_code_hooks_to_a_live_endpoint_with_the_right_token() {
        // AC: the cut writes the hook config, and events arrive with the right
        // token. A claude-code cut starts a per-Workspace endpoint and writes
        // hook settings pointing at it under the session bearer token.
        let f = fixture();
        let enq = enqueue(&f.registry, &request("hooked", "wire me")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );
        assert!(matches!(cut_state(&f), CutState::Complete { .. }));

        let ws = &enq.workspace;
        let endpoint = f
            .endpoints
            .get(&ws.id)
            .expect("a control-plane-hooks cut must start an endpoint");

        // The hook config landed in the worktree LOCAL settings file (never
        // the committed one), pointing at THIS endpoint with THIS token.
        let settings = std::fs::read_to_string(
            Path::new(&ws.worktree_path).join(".claude/settings.local.json"),
        )
        .expect("cut must write the hook settings");
        let command = serde_json::from_str::<serde_json::Value>(&settings).unwrap()["hooks"]
            ["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(command.contains(&endpoint.url()), "hook POSTs to the endpoint");
        assert!(
            command.contains(&format!("Bearer {}", endpoint.token())),
            "hook carries the session bearer token"
        );

        // A POST bearing that exact token is accepted and renders a card; a
        // wrong or missing token injects nothing (the #15 gate, live at cut).
        assert_eq!(
            http_post(endpoint.port(), Some(&format!("Bearer {}", endpoint.token())), PRE_TOOL_USE),
            200
        );
        assert_eq!(http_post(endpoint.port(), Some("Bearer wrong"), PRE_TOOL_USE), 401);
        assert_eq!(http_post(endpoint.port(), None, PRE_TOOL_USE), 401);
        let snap = endpoint.snapshot();
        assert_eq!(snap.cards.len(), 1, "only the authorized POST injected");
        assert!(snap.warnings.is_empty());
    }

    #[test]
    fn hook_events_flip_the_workspace_working_blocked_done_end_to_end() {
        // AC/demo: hook-driven only, the agent flips Working → Blocked on a
        // question → Done on finish. Drive the cut's live endpoint with the
        // payloads a real claude sends and assert both the derived wall status
        // and the approval-card lifecycle — no agent-signal in sight.
        let f = fixture();
        let enq = enqueue(&f.registry, &request("flip", "drive me")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );
        // The live Workspace after the cut completed (Idle: no signal yet).
        let ws = f
            .registry
            .snapshot()
            .unwrap()
            .workspaces
            .into_iter()
            .find(|w| w.id == enq.workspace.id)
            .unwrap();
        let endpoint = f.endpoints.get(&ws.id).unwrap();
        let auth = format!("Bearer {}", endpoint.token());

        // A completed cut with no live signal derives as Idle.
        assert_eq!(derive_status(&ws), WorkspaceStatus::Idle);

        // PreToolUse → Working, and a pending approval card appears.
        assert_eq!(http_post(endpoint.port(), Some(&auth), PRE_TOOL_USE), 200);
        assert_eq!(status_after(&ws, PRE_TOOL_USE), WorkspaceStatus::Working);
        use crate::modules::core::control_plane::CardStatus;
        assert_eq!(endpoint.snapshot().cards[0].status, CardStatus::Pending);

        // Permission prompt → Blocked ("Needs you"), the card is surfaced.
        assert_eq!(http_post(endpoint.port(), Some(&auth), PERMISSION), 200);
        assert_eq!(status_after(&ws, PERMISSION), WorkspaceStatus::Blocked);
        assert_eq!(status_after(&ws, PERMISSION).display_alias(), "Needs you");
        assert_eq!(endpoint.snapshot().cards[0].status, CardStatus::Surfaced);

        // Stop → Done ("To review"); the unresolved approval closes unrun.
        assert_eq!(http_post(endpoint.port(), Some(&auth), STOP), 200);
        assert_eq!(status_after(&ws, STOP), WorkspaceStatus::Done);
        assert_eq!(endpoint.snapshot().cards[0].status, CardStatus::ClosedNoRun);
        assert!(endpoint.snapshot().warnings.is_empty());
    }

    #[test]
    fn removing_a_workspace_would_drop_its_endpoint() {
        // The lifecycle seam the Tauri command uses: an endpoint is live after
        // a control-plane cut and gone once the Workspace is removed.
        let f = fixture();
        let enq = enqueue(&f.registry, &request("temp", "b")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &ClaudeCode,
            &f.endpoints,
            &enq,
        );
        assert!(f.endpoints.get(&enq.workspace.id).is_some());
        f.endpoints.remove(&enq.workspace.id);
        assert!(f.endpoints.get(&enq.workspace.id).is_none());
    }

    /// A Harness with no `control_plane_hooks` Cap (Signal-only): it keeps the
    /// M2 agent-signal path and never touches the control plane.
    struct SignalOnlyHarness;

    impl Harness for SignalOnlyHarness {
        fn id(&self) -> &'static str {
            "fake-agent"
        }
        fn display_name(&self) -> &'static str {
            "Signal Only"
        }
        fn caps(&self) -> Caps {
            Caps {
                control_plane_hooks: false,
                agent_signal: true,
                ..ClaudeCode.caps()
            }
        }
        fn launch_plan(&self, _ctx: &LaunchContext) -> LaunchPlan {
            LaunchPlan {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), "true".to_string()],
            }
        }
        fn config_injection(&self, _ctx: &LaunchContext) -> Vec<ConfigFile> {
            Vec::new()
        }
    }

    #[test]
    fn a_signal_only_harness_starts_no_endpoint_and_keeps_agent_signal() {
        // AC: a Signal-only Harness still shows status — Cap degradation must
        // not break. No endpoint is bound, no hook config is written, and the
        // agent-signal → status reducer still lights the wall.
        use crate::modules::core::cut::{roll_up_status, session_status_from_signal, SessionSignal};

        let f = fixture();
        let enq = enqueue(&f.registry, &request("signal", "no hooks")).unwrap();
        run(
            &f.registry,
            &f.roots,
            &RecordingRuntime::default(),
            &SignalOnlyHarness,
            &f.endpoints,
            &enq,
        );
        assert!(matches!(cut_state(&f), CutState::Complete { .. }));

        let ws = f
            .registry
            .snapshot()
            .unwrap()
            .workspaces
            .into_iter()
            .find(|w| w.id == enq.workspace.id)
            .unwrap();
        // No control plane for this Workspace at all.
        assert!(
            f.endpoints.get(&ws.id).is_none(),
            "a Signal-only Harness must not start an endpoint"
        );
        assert!(
            !Path::new(&ws.worktree_path)
                .join(".claude/settings.local.json")
                .exists(),
            "no hook config is written without the control_plane_hooks Cap"
        );

        // The M2 agent-signal path still derives status (unchanged reducer).
        assert_eq!(
            roll_up_status(
                derive_status(&ws),
                &[session_status_from_signal(SessionSignal::Working).unwrap()]
            ),
            WorkspaceStatus::Working
        );
        assert_eq!(
            roll_up_status(
                derive_status(&ws),
                &[session_status_from_signal(SessionSignal::Attention).unwrap()]
            ),
            WorkspaceStatus::Blocked
        );
    }

    // --- enqueue failures reject synchronously, before a Workspace exists
    // (the Slot/env "step" of the PRD list is settled here) ---

    #[test]
    fn enqueue_rejects_hostile_input_and_unknown_references() {
        let f = fixture();
        for bad in ["", "../escape", "a/b", "-flag", "has space"] {
            let mut req = request(bad, "b");
            req.slug = bad.to_string();
            assert!(enqueue(&f.registry, &req).is_err(), "slug {bad:?}");
        }
        let mut req = request("ok", "b");
        req.brief = "a\0b".to_string();
        assert!(enqueue(&f.registry, &req).unwrap_err().contains("brief"));

        let mut req = request("ok", "b");
        req.project_id = "prj-ghost".to_string();
        assert!(enqueue(&f.registry, &req).unwrap_err().contains("prj-ghost"));

        let mut req = request("ok", "b");
        req.profile_id = "prj-1:ghost".to_string();
        assert!(enqueue(&f.registry, &req).unwrap_err().contains("ghost"));

        // Nothing was committed and nothing slow ran.
        assert!(f.registry.snapshot().unwrap().workspaces.is_empty());
    }

    #[test]
    fn a_profile_of_another_project_is_rejected_at_the_seam() {
        let f = fixture();
        let repo2 = f._tmp.path().join("repo2");
        std::fs::create_dir_all(&repo2).unwrap();
        git(&repo2, &["init", "-b", "main", "."]);
        f.registry
            .commit(Event::ProjectAdded {
                project: Project {
                    id: "prj-2".to_string(),
                    name: "other".to_string(),
                    repo_root: crate::modules::fs::to_canon(
                        std::fs::canonicalize(&repo2).unwrap(),
                    ),
                    base_branch: "main".to_string(),
                    worktree_home: crate::modules::fs::to_canon(f._tmp.path().join("wt2")),
                    branch_template: "helm/{slug}".to_string(),
                    settings: Default::default(),
                },
            })
            .unwrap();
        let mut req = request("ok", "b");
        req.profile_id = "prj-2:feature".to_string();
        let err = enqueue(&f.registry, &req).unwrap_err();
        assert!(err.contains("another project"), "got: {err}");
    }

    #[test]
    fn a_second_cut_of_a_live_slug_is_rejected_at_enqueue() {
        // Same slug, template without {slot}: the branch would collide.
        // The Slot/branch rules reject the cut *synchronously* — no
        // half-cut Workspace ever exists.
        let f = fixture();
        enqueue(&f.registry, &request("fix", "b")).unwrap();
        let err = enqueue(&f.registry, &request("fix", "b")).unwrap_err();
        assert!(err.contains("helm/fix"), "got: {err}");
        assert_eq!(f.registry.snapshot().unwrap().workspaces.len(), 1);
    }

    // --- scuttle mid-cut: recorded or torn down, never orphaned ---

    #[test]
    fn a_workspace_scuttled_mid_cut_leaves_no_orphan_worktree() {
        let f = fixture();
        let enq = enqueue(&f.registry, &request("fix", "b")).unwrap();
        // The user scuttles while the pipeline is still queued.
        worktree::remove(&f.registry, &enq.workspace.id).unwrap();
        assert!(f.registry.snapshot().unwrap().workspaces.is_empty());

        let runtime = RecordingRuntime::default();
        run(&f.registry, &f.roots, &runtime, &ClaudeCode, &f.endpoints, &enq);

        // The pipeline could not park (nothing to park) — and nothing of
        // its work survives without a registry record.
        assert!(f.registry.snapshot().unwrap().workspaces.is_empty());
        assert!(!Path::new(&enq.workspace.worktree_path).exists());
        assert!(!worktree::branch_exists(
            Path::new(&f.repo_root),
            "helm/fix"
        ));
    }

    // ═══════════════════════════════════════════════════════════════════
    // M2 scripted demo — the milestone "Done when", backend seam.
    //
    //   Two Projects × two agents, triaged and driven end-to-end,
    //   keyboard-only — at the automatable backend seam.
    //
    // This is the backend half of the M2 demo. The frontend half — the
    // agent-signal → rollup → wall filter/group/repo-picker view-model —
    // lives in `src/modules/helm/m2Demo.test.ts`. Together they exercise the
    // whole end-to-end path; run this with `cargo test m2_demo`.
    //
    // COVERED here (automated): the real project → cut-pipeline → runtime →
    // rollup path —
    //   • two real git Projects added to the registry;
    //   • two Workspaces cut through the full ambient pipeline, each
    //     launching a *fake* agent on a real LocalPty (never a real
    //     unattended `claude` — the fake prints its prompt then blocks);
    //   • a zoom attach + a steer (write-to-PTY) through the runtime;
    //   • status observed via the agent-signal → event → rollup path
    //     (`session_status_from_signal` → `roll_up_status`).
    //
    // NOT covered here (human/verify at the running Tauri app, per the
    // no-DOM-test constraint): the literal `f`/`g`/`r` key presses and the
    // wall re-render. See the TS demo for the view-model assertions and the
    // task journal for the human checklist.
    #[test]
    fn m2_demo_two_projects_two_agents_driven_end_to_end() {
        use crate::modules::core::cut::{
            roll_up_status, session_status_from_signal, SessionSignal,
        };

        // Drain output until `needle` shows up or the deadline passes;
        // panics with the transcript on timeout (mirrors the conformance
        // suite's `wait_for`).
        fn wait_until(rx: &std::sync::mpsc::Receiver<Vec<u8>>, needle: &str) {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut seen = Vec::new();
            while !String::from_utf8_lossy(&seen).contains(needle) {
                let left = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(left) {
                    Ok(chunk) => seen.extend_from_slice(&chunk),
                    Err(_) => panic!(
                        "never saw {needle:?}; transcript so far: {:?}",
                        String::from_utf8_lossy(&seen)
                    ),
                }
            }
        }

        // --- two Projects, both real git repos ---
        // Project A comes from the standard fixture (repo on `main`).
        let f = fixture();

        // Project B: a second real repo on `trunk`, with its own worktree
        // home, so the wall genuinely spans two Projects with two base
        // branches.
        let repo2 = f._tmp.path().join("repo2");
        std::fs::create_dir_all(&repo2).unwrap();
        git(&repo2, &["init", "-b", "trunk", "."]);
        git(
            &repo2,
            &[
                "-c", "user.name=t", "-c", "user.email=t@t", "commit",
                "--allow-empty", "-m", "base",
            ],
        );
        let repo2_root =
            crate::modules::fs::to_canon(std::fs::canonicalize(&repo2).unwrap());
        let wt2 = crate::modules::fs::to_canon(f._tmp.path().join("wt2"));
        f.registry
            .commit(Event::ProjectAdded {
                project: Project {
                    id: "prj-2".to_string(),
                    name: "beta".to_string(),
                    repo_root: repo2_root,
                    base_branch: "trunk".to_string(),
                    worktree_home: wt2,
                    branch_template: "helm/{slug}".to_string(),
                    settings: Default::default(),
                },
            })
            .unwrap();

        // --- a fake agent: prints its opening prompt, then echoes one line
        //     steered in from the PTY (so attach + write are observable) ---
        let script = f._tmp.path().join("steer-agent.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf 'OPENED[%s]\\n' \"$1\"\nread line\nprintf 'STEER[%s]\\n' \"$line\"\nsleep 30\n",
        )
        .unwrap();
        let script = script.to_string_lossy().into_owned();

        struct SteerHarness {
            script: String,
        }
        impl Harness for SteerHarness {
            fn id(&self) -> &'static str {
                "fake-agent"
            }
            fn display_name(&self) -> &'static str {
                "Steer"
            }
            fn caps(&self) -> Caps {
                ClaudeCode.caps()
            }
            fn launch_plan(&self, ctx: &LaunchContext) -> LaunchPlan {
                LaunchPlan {
                    program: "/bin/sh".to_string(),
                    args: vec![self.script.clone(), ctx.opening_prompt.to_string()],
                }
            }
            fn config_injection(&self, _ctx: &LaunchContext) -> Vec<ConfigFile> {
                Vec::new()
            }
        }
        let harness = SteerHarness { script };
        let runtime = LocalPty::default();

        // --- cut one Workspace in each Project (the two agents) ---
        let enq_a =
            enqueue(&f.registry, &request("triage-login", "fix login")).unwrap();
        run(&f.registry, &f.roots, &runtime, &harness, &f.endpoints, &enq_a);

        let req_b = CutRequest {
            project_id: "prj-2".to_string(),
            slug: "triage-signup".to_string(),
            profile_id: "prj-2:feature".to_string(),
            brief: "add signup".to_string(),
            fetch: false,
        };
        let enq_b = enqueue(&f.registry, &req_b).unwrap();
        run(&f.registry, &f.roots, &runtime, &harness, &f.endpoints, &enq_b);

        // Both cuts completed: the wall would show two Projects × two agents.
        let state = f.registry.snapshot().unwrap();
        assert_eq!(state.projects.len(), 2, "two Projects");
        assert_eq!(state.workspaces.len(), 2, "two agents (Workspaces)");
        let ws_a = state
            .workspaces
            .iter()
            .find(|w| w.slug == "triage-login")
            .unwrap();
        let ws_b = state
            .workspaces
            .iter()
            .find(|w| w.slug == "triage-signup")
            .unwrap();
        assert_ne!(ws_a.project_id, ws_b.project_id, "one agent per Project");
        // A completed cut with no live Session yet derives as Idle.
        assert_eq!(derive_status(ws_a), WorkspaceStatus::Idle);
        assert_eq!(derive_status(ws_b), WorkspaceStatus::Idle);
        let CutState::Complete {
            first_session_id: sid_a,
        } = ws_a.cut.clone()
        else {
            panic!("workspace A cut must complete, got {:?}", ws_a.cut);
        };
        let CutState::Complete {
            first_session_id: sid_b,
        } = ws_b.cut.clone()
        else {
            panic!("workspace B cut must complete, got {:?}", ws_b.cut);
        };

        // --- zoom attach + steer on agent A's real PTY session ---
        let (tx, rx) = channel::<Vec<u8>>();
        runtime
            .attach(
                &sid_a,
                OutputSink {
                    on_output: Box::new(move |b| {
                        let _ = tx.send(b.to_vec());
                    }),
                    on_exit: Box::new(|_| {}),
                },
            )
            .unwrap();
        // Scrollback replays the opening prompt (Profile snippet + Brief).
        wait_until(&rx, "OPENED[/tdd fix login]");
        // Take the wheel: write a steer line straight to the PTY; the fake
        // agent echoes it back, proving the steer reached the process.
        runtime.write(&sid_a, b"take a different approach\r").unwrap();
        wait_until(&rx, "STEER[take a different approach]");

        // --- status via the agent-signal → event → rollup path ---
        // Triage: agent A asks for approval (Attention → Blocked = "Needs
        // you"); agent B is actively Working. This is the exact reducer the
        // frontend mirrors (`viewModel.rollUpStatus`).
        let a_status = roll_up_status(
            derive_status(ws_a),
            &[session_status_from_signal(SessionSignal::Attention).unwrap()],
        );
        assert_eq!(a_status, WorkspaceStatus::Blocked, "A asks → Needs you");
        let b_status = roll_up_status(
            derive_status(ws_b),
            &[session_status_from_signal(SessionSignal::Working).unwrap()],
        );
        assert_eq!(b_status, WorkspaceStatus::Working, "B works");
        // Driven to done: approve A, it finishes → To review.
        assert_eq!(
            roll_up_status(
                derive_status(ws_a),
                &[session_status_from_signal(SessionSignal::Finished).unwrap()],
            ),
            WorkspaceStatus::Done,
        );
        // An exited Session contributes nothing → the cut-derived Idle stands
        // (no stale dot pinned by a dead process).
        assert_eq!(session_status_from_signal(SessionSignal::Exited), None);

        // --- never leave a live unattended agent behind ---
        runtime.kill(&sid_a).ok();
        runtime.kill(&sid_b).ok();
    }
}
