//! Assemble a SpawnSpec for an Agent Session in a cut Workspace: the seam
//! where Harness (what to run) meets Runtime (where it runs).
//!
//! Boundary validation happens here: the Workspace must exist in the
//! Helmsmen registry, its worktree must still canonicalize on disk, and
//! injected config paths must stay inside the worktree. Everything
//! spawned in a Workspace carries the cut's `HELMSMEN_*` env.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::modules::core::project::{validate_abs_path, Project};
use crate::modules::core::settings::ProcessDef;
use crate::modules::core::workspace::helmsmen_env;
use crate::modules::harness::{ConfigFile, Harness, LaunchContext, LaunchPlan};
use crate::modules::registry::RegistryState;
use crate::modules::workspace::WorkspaceRegistry;

use super::SpawnSpec;

/// `TERM` a GUI-launched app lacks; interactive Sessions (agent, shell,
/// process) all need one. `HELMSMEN_*` stays exactly the specced set.
const DEFAULT_TERM: &str = "xterm-256color";

/// Per-launch values from the Profile (task #8): model and opening
/// prompt. `Default` = launch bare (M1 behavior, and every Session after
/// the first).
#[derive(Debug, Clone, Copy, Default)]
pub struct LaunchOverrides<'a> {
    /// Harness-specific model; empty = the Harness default.
    pub model: &'a str,
    /// Opening prompt (Profile snippet with the Brief composed in);
    /// empty = start without a prompt.
    pub opening_prompt: &'a str,
}

pub fn prepare_spawn(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    harness_id: &str,
    cols: u16,
    rows: u16,
) -> Result<SpawnSpec, String> {
    let harness = crate::modules::harness::by_id(harness_id)
        .ok_or_else(|| format!("unknown harness {harness_id:?}"))?;
    prepare_spawn_with(
        registry,
        roots,
        workspace_id,
        harness,
        LaunchOverrides::default(),
        cols,
        rows,
    )
}

/// Trait-typed variant so tests, the cut pipeline, and later Profiles can
/// hand in any Harness (plus that launch's overrides).
pub fn prepare_spawn_with(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    harness: &dyn Harness,
    overrides: LaunchOverrides<'_>,
    cols: u16,
    rows: u16,
) -> Result<SpawnSpec, String> {
    let base = resolve_workspace_root(registry, roots, workspace_id, cols, rows)?;
    // The control-plane endpoint is wired by the cut pipeline (task #16): a
    // Harness with the `control_plane_hooks` Cap already has its hook settings
    // in the worktree from the cut, and the endpoint stays live in the
    // `EndpointRegistry`. A later spawn therefore leaves that wiring alone —
    // `None` here means `claude-code::config_injection` writes nothing and so
    // never clobbers the cut-written settings file.
    let ctx = LaunchContext {
        workspace_root: &base.worktree,
        env: &base.env,
        model: overrides.model,
        opening_prompt: overrides.opening_prompt,
        control_plane: None,
    };
    apply_config_injection(&base.worktree, &harness.config_injection(&ctx))?;
    let plan = harness.launch_plan(&ctx);
    Ok(base.into_spec(plan, cols, rows))
}

/// The worktree + `HELMSMEN_*` env every Session in a Workspace shares,
/// resolved once at the Harness/Shell/Process ↔ Runtime seam. Carries the
/// resolved Project and Workspace so a Process spawn can look up its
/// definition and a Shell spawn can read the Slot.
struct WorkspaceRoot {
    project: Project,
    worktree: String,
    env: BTreeMap<String, String>,
}

impl WorkspaceRoot {
    /// Finish a spec from a launch plan: the plan's argv, the resolved
    /// worktree as cwd, the `HELMSMEN_*` env plus a `TERM` default.
    fn into_spec(mut self, plan: LaunchPlan, cols: u16, rows: u16) -> SpawnSpec {
        self.env
            .entry("TERM".to_string())
            .or_insert_with(|| DEFAULT_TERM.to_string());
        SpawnSpec {
            program: plan.program,
            args: plan.args,
            cwd: self.worktree,
            env: self.env,
            cols,
            rows,
        }
    }
}

/// Boundary validation shared by every spawn into a Workspace: the
/// Workspace must exist in the Helmsmen registry, its worktree must still
/// canonicalize on disk, the size must be non-zero, and the worktree is
/// re-authorized as a Terax workspace root (idempotent, scoped to exactly
/// that path). The stored path is data, not truth — it is re-validated and
/// re-resolved against the real filesystem before anything spawns in it.
fn resolve_workspace_root(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    cols: u16,
    rows: u16,
) -> Result<WorkspaceRoot, String> {
    if cols == 0 || rows == 0 {
        return Err("spawn: cols and rows must be non-zero".to_string());
    }

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

    validate_abs_path("worktreePath", &workspace.worktree_path).map_err(|e| e.to_string())?;
    let worktree = std::fs::canonicalize(&workspace.worktree_path)
        .map(|p| crate::modules::fs::to_canon(&p))
        .map_err(|e| format!("worktree {:?} is gone: {e}", workspace.worktree_path))?;

    roots
        .authorize(&worktree)
        .map_err(|e| format!("cannot authorize workspace root {worktree:?}: {e}"))?;

    let env = helmsmen_env(&project, &workspace);
    Ok(WorkspaceRoot {
        project,
        worktree,
        env,
    })
}

/// The user's interactive shell for a Shell Session: `$SHELL` when set,
/// else `/bin/sh`. Read from the environment (the imperative shell owns the
/// OS); the pure launch-plan builders take the resolved program as data.
fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// Launch plan for a Shell Session: just the user's shell, PTY-attached so
/// it runs interactively. Argv only — nothing is re-parsed by a shell.
pub fn shell_launch_plan(shell: &str) -> LaunchPlan {
    LaunchPlan {
        program: shell.to_string(),
        args: Vec::new(),
    }
}

/// Launch plan for a Process Session: the Project's declared command line
/// handed to the user's shell as a single `-c` argument, exactly like the
/// setup script. The command is the user's own settings data (never a
/// repo-supplied value), run in the worktree; it is one argv element, so
/// nothing Helmsmen adds is re-interpreted by the shell.
pub fn process_launch_plan(shell: &str, command: &str) -> LaunchPlan {
    LaunchPlan {
        program: shell.to_string(),
        args: vec!["-c".to_string(), command.to_string()],
    }
}

/// Assemble a SpawnSpec for a **Shell Session** — the user's own terminal
/// in the Workspace's worktree, carrying the cut's `HELMSMEN_*` env.
pub fn prepare_shell_spawn(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    cols: u16,
    rows: u16,
) -> Result<SpawnSpec, String> {
    prepare_shell_spawn_with(registry, roots, workspace_id, &user_shell(), cols, rows)
}

/// Shell spawn with the shell program handed in — the test seam (a fake
/// shell script stands in for `$SHELL` so `HELMSMEN_*` reaching a real
/// process is provable in CI without depending on the runner's shell).
pub fn prepare_shell_spawn_with(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    shell: &str,
    cols: u16,
    rows: u16,
) -> Result<SpawnSpec, String> {
    let base = resolve_workspace_root(registry, roots, workspace_id, cols, rows)?;
    Ok(base.into_spec(shell_launch_plan(shell), cols, rows))
}

/// Assemble a SpawnSpec for a **Process Session** — one of the Project's
/// Process definitions run on demand in the Workspace's worktree. Returns
/// the matched [`ProcessDef`] alongside the spec so the command can echo
/// the Session's name and port back for its chip (`dev:5173`).
pub fn prepare_process_spawn(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    process_name: &str,
    cols: u16,
    rows: u16,
) -> Result<(SpawnSpec, ProcessDef), String> {
    prepare_process_spawn_with(
        registry,
        roots,
        workspace_id,
        process_name,
        &user_shell(),
        cols,
        rows,
    )
}

/// Process spawn with the shell program handed in — the test seam.
pub fn prepare_process_spawn_with(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    process_name: &str,
    shell: &str,
    cols: u16,
    rows: u16,
) -> Result<(SpawnSpec, ProcessDef), String> {
    let base = resolve_workspace_root(registry, roots, workspace_id, cols, rows)?;
    // The Process must be one the Project actually defines: the name is a
    // key into user-level settings, never a caller-supplied command.
    let def = base
        .project
        .settings
        .processes
        .iter()
        .find(|p| p.name == process_name)
        .cloned()
        .ok_or_else(|| {
            format!("no process {process_name:?} defined for project {:?}", base.project.name)
        })?;
    let plan = process_launch_plan(shell, &def.command);
    Ok((base.into_spec(plan, cols, rows), def))
}

/// Write the Harness's config files into the worktree (M3: hook wiring;
/// also the cut pipeline's harness-wiring step, task #8). Paths are
/// hostile until proven worktree-relative: absolute paths and any `..`
/// component are rejected lexically, and because a checked-out branch can
/// plant symlinks the destination is also resolved against the live
/// filesystem and must stay inside the canonical worktree root. Fail safe:
/// anything unresolvable or out-of-root refuses before a byte is written.
pub(crate) fn apply_config_injection(worktree: &str, files: &[ConfigFile]) -> Result<(), String> {
    let root = std::fs::canonicalize(worktree)
        .map_err(|e| format!("cannot resolve worktree {worktree:?}: {e}"))?;
    for file in files {
        let rel = Path::new(&file.rel_path);
        let escapes = rel.is_absolute()
            || rel
                .components()
                .any(|c| !matches!(c, Component::Normal(_)));
        if escapes || file.rel_path.trim().is_empty() {
            return Err(format!(
                "config injection path {:?} must be worktree-relative",
                file.rel_path
            ));
        }
        let dest = root.join(rel);
        let parent = dest
            .parent()
            .ok_or_else(|| format!("config injection path {:?} has no parent", file.rel_path))?;
        // Resolve before creating anything so a symlinked component cannot
        // carry even an intermediate directory outside the root.
        ensure_within_root(&root, deepest_existing_ancestor(parent), &file.rel_path)?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        // Re-check the created parent: the containment must hold on the
        // path actually being written through, not just the pre-image.
        ensure_within_root(&root, parent, &file.rel_path)?;
        // `fs::write` follows a symlinked leaf to some other file; a config
        // destination that is a symlink is never legitimate, so refuse.
        if std::fs::symlink_metadata(&dest).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "config injection refuses to write through symlink {}",
                dest.display()
            ));
        }
        std::fs::write(&dest, &file.contents)
            .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    }
    Ok(())
}

/// Containment check for one injected destination: `candidate` must
/// resolve (symlinks and all) to a path under the canonical `root`.
fn ensure_within_root(root: &Path, candidate: &Path, rel_path: &str) -> Result<(), String> {
    let resolved = std::fs::canonicalize(candidate).map_err(|e| {
        format!(
            "config injection path {rel_path:?}: cannot resolve {}: {e}",
            candidate.display()
        )
    })?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "config injection path {rel_path:?} escapes the worktree (resolves to {})",
            resolved.display()
        ));
    }
    Ok(())
}

/// The closest ancestor of `path` that exists on disk (symlinks count as
/// existing, dangling or not, so they are resolved rather than skipped).
fn deepest_existing_ancestor(path: &Path) -> &Path {
    let mut cur = path;
    while std::fs::symlink_metadata(cur).is_err() {
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    cur
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    use crate::modules::core::project::Project;
    use crate::modules::core::settings::ProjectSettings;
    use crate::modules::core::state::Event;
    use crate::modules::harness::{Caps, LaunchPlan};
    use crate::modules::registry::worktree;
    use crate::modules::runtime::local_pty::LocalPty;
    use crate::modules::runtime::{OutputSink, Runtime, SessionStatus};

    struct Fixture {
        _tmp: tempfile::TempDir,
        registry: RegistryState,
        roots: WorkspaceRegistry,
        repo_root: String,
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

    fn fixture() -> Fixture {
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
        let registry = RegistryState::load(tmp.path().join("appdata"));
        registry
            .commit(Event::ProjectAdded {
                project: Project {
                    id: "prj-1".to_string(),
                    name: "demo".to_string(),
                    repo_root: repo_root.clone(),
                    base_branch: "main".to_string(),
                    worktree_home: crate::modules::fs::to_canon(tmp.path().join("wt")),
                    branch_template: "helm/{slug}".to_string(),
                    settings: Default::default(),
                },
            })
            .unwrap();
        Fixture {
            _tmp: tmp,
            registry,
            roots: WorkspaceRegistry::default(),
            repo_root,
        }
    }

    /// Stands in for `claude` so the demo runs headless and in CI: prints
    /// the env it received, echoes one typed line back, then waits like an
    /// interactive agent would.
    struct FakeAgent {
        script: String,
    }

    impl crate::modules::harness::Harness for FakeAgent {
        fn id(&self) -> &'static str {
            "fake-agent"
        }
        fn display_name(&self) -> &'static str {
            "Fake Agent"
        }
        fn caps(&self) -> Caps {
            Caps {
                resume: false,
                control_plane_hooks: false,
                agent_signal: false,
                cost_telemetry: false,
                mcp_config: false,
                model_select: false,
            }
        }
        fn launch_plan(&self, _ctx: &LaunchContext) -> LaunchPlan {
            LaunchPlan {
                program: "/bin/sh".to_string(),
                args: vec![self.script.clone()],
            }
        }
        fn config_injection(&self, _ctx: &LaunchContext) -> Vec<ConfigFile> {
            vec![ConfigFile {
                rel_path: ".helmsmen/agent.cfg".to_string(),
                contents: "injected".to_string(),
            }]
        }
    }

    fn write_script(dir: &Path) -> String {
        let script = dir.join("fake-agent.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             printf 'AGENT_UP slot=%s ws=%s cwd=%s\\n' \"$HELMSMEN_SLOT\" \"$HELMSMEN_WORKSPACE\" \"$(pwd)\"\n\
             read line\n\
             printf 'typed:%s\\n' \"$line\"\n\
             sleep 30\n",
        )
        .unwrap();
        script.to_string_lossy().into_owned()
    }

    fn sink() -> (OutputSink, Receiver<Vec<u8>>, Receiver<i32>) {
        let (out_tx, out_rx) = channel::<Vec<u8>>();
        let (exit_tx, exit_rx) = channel::<i32>();
        (
            OutputSink {
                on_output: Box::new(move |b| {
                    let _ = out_tx.send(b.to_vec());
                }),
                on_exit: Box::new(move |c| {
                    let _ = exit_tx.send(c);
                }),
            },
            out_rx,
            exit_rx,
        )
    }

    fn wait_for(rx: &Receiver<Vec<u8>>, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains(needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(chunk) => seen.extend_from_slice(&chunk),
                Err(_) => panic!(
                    "never saw {needle:?}; transcript: {:?}",
                    String::from_utf8_lossy(&seen)
                ),
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    // --- AC: the M1 scripted demo, add Project -> cut -> spawn -> stream
    // + type, minus only the webview (the dev console calls exactly these
    // seams through the helm_* commands). ---

    #[test]
    fn m1_demo_cut_spawn_stream_type() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "demo").unwrap();
        let agent = FakeAgent {
            script: write_script(f._tmp.path()),
        };

        let spec = prepare_spawn_with(
            &f.registry,
            &f.roots,
            &cut.workspace.id,
            &agent,
            LaunchOverrides::default(),
            120,
            32,
        )
        .unwrap();
        assert_eq!(spec.cwd, cut.workspace.worktree_path);
        assert_eq!(spec.env["HELMSMEN_SLOT"], "1");
        assert_eq!(spec.env["HELMSMEN_WORKSPACE"], cut.workspace.worktree_path);
        assert_eq!(spec.env["HELMSMEN_PROJECT"], "demo");
        assert_eq!(spec.env["HELMSMEN_MAIN_CHECKOUT"], f.repo_root);
        assert!(spec.env.contains_key("TERM"));

        // Config-injection seam ran, scoped inside the worktree.
        let injected = Path::new(&cut.workspace.worktree_path).join(".helmsmen/agent.cfg");
        assert_eq!(std::fs::read_to_string(injected).unwrap(), "injected");

        // Spawn in the cut worktree, stream, type, kill: the full loop.
        let rt = LocalPty::default();
        let (sink, out, exit) = sink();
        let id = rt.spawn(spec, sink).unwrap();
        let up = wait_for(&out, "AGENT_UP");
        assert!(up.contains("slot=1"), "env must reach the agent: {up}");
        assert!(
            up.contains(&format!("ws={}", cut.workspace.worktree_path)),
            "agent must see its workspace: {up}"
        );
        rt.write(&id, b"hello helm\r").unwrap();
        wait_for(&out, "typed:hello helm");
        rt.kill(&id).unwrap();
        exit.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            rt.status(&id).unwrap(),
            SessionStatus::Exited(_)
        ));
    }

    /// Liveness against the real thing, per the claude-code Harness's own
    /// launch plan. `--version` only: a real interactive claude must
    /// never run unattended. Skips when claude is not installed (CI).
    #[test]
    fn real_claude_launch_plan_is_alive() {
        if Command::new("claude").arg("--version").output().is_err() {
            eprintln!("skipping: no `claude` on PATH");
            return;
        }
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "live").unwrap();
        let mut spec = prepare_spawn(&f.registry, &f.roots, &cut.workspace.id, "claude-code", 120, 32)
            .unwrap();
        assert_eq!(spec.program, "claude");
        assert!(spec.args.is_empty());
        // Liveness only: version, not an unattended interactive session.
        spec.args = vec!["--version".to_string()];

        let rt = LocalPty::default();
        let (sink, out, exit) = sink();
        rt.spawn(spec, sink).unwrap();
        let code = exit.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(code, 0, "claude --version must exit cleanly");
        let transcript = wait_for(&out, ".");
        assert!(!transcript.trim().is_empty());
    }

    // --- boundary validation ---

    #[test]
    fn unknown_workspace_and_harness_are_rejected() {
        let f = fixture();
        let err =
            prepare_spawn(&f.registry, &f.roots, "ws-ghost", "claude-code", 80, 24).unwrap_err();
        assert!(err.contains("ws-ghost"), "got: {err}");

        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "x").unwrap();
        let err =
            prepare_spawn(&f.registry, &f.roots, &cut.workspace.id, "ghost", 80, 24).unwrap_err();
        assert!(err.contains("ghost"), "got: {err}");
    }

    #[test]
    fn a_deleted_worktree_cannot_be_spawned_into() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "gone").unwrap();
        std::fs::remove_dir_all(&cut.workspace.worktree_path).unwrap();
        let err = prepare_spawn(&f.registry, &f.roots, &cut.workspace.id, "claude-code", 80, 24)
            .unwrap_err();
        assert!(err.contains("gone"), "got: {err}");
    }

    #[test]
    fn zero_size_is_rejected_at_the_seam() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "z").unwrap();
        assert!(
            prepare_spawn(&f.registry, &f.roots, &cut.workspace.id, "claude-code", 0, 24).is_err()
        );
        assert!(
            prepare_spawn(&f.registry, &f.roots, &cut.workspace.id, "claude-code", 80, 0).is_err()
        );
    }

    #[test]
    fn spawn_reauthorizes_exactly_the_worktree_root() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "auth").unwrap();
        // Simulate an app restart: authorization state is fresh.
        let fresh_roots = WorkspaceRegistry::default();
        let agent = FakeAgent {
            script: write_script(f._tmp.path()),
        };
        prepare_spawn_with(
            &f.registry,
            &fresh_roots,
            &cut.workspace.id,
            &agent,
            LaunchOverrides::default(),
            80,
            24,
        )
        .unwrap();
        assert!(fresh_roots.is_authorized(Path::new(&cut.workspace.worktree_path)));
        assert!(
            !fresh_roots.is_authorized(Path::new(&f.repo_root)),
            "the main checkout must not gain permissions from a spawn"
        );
    }

    // --- config injection stays inside the worktree ---

    #[test]
    fn hostile_config_injection_paths_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::modules::fs::to_canon(tmp.path());
        for bad in ["/etc/evil", "../outside", "a/../../b", ""] {
            let files = vec![ConfigFile {
                rel_path: bad.to_string(),
                contents: "x".to_string(),
            }];
            let err = apply_config_injection(&root, &files)
                .expect_err(&format!("path {bad:?} must be rejected"));
            assert!(err.contains("worktree-relative"), "got: {err}");
        }
        assert!(!Path::new("/etc/evil").exists());
        assert!(!tmp.path().parent().unwrap().join("outside").exists());
    }

    #[test]
    fn config_injection_writes_nested_relative_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::modules::fs::to_canon(tmp.path());
        apply_config_injection(
            &root,
            &[ConfigFile {
                rel_path: ".claude/hooks/wiring.json".to_string(),
                contents: "{}".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".claude/hooks/wiring.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn config_injection_refuses_a_symlinked_directory_component() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("worktree");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // A repo-supplied symlink checked out into the worktree: lexically
        // the path is worktree-relative, on disk it points elsewhere.
        std::os::unix::fs::symlink(&outside, worktree.join(".claude")).unwrap();
        let root = crate::modules::fs::to_canon(&worktree);
        let err = apply_config_injection(
            &root,
            &[ConfigFile {
                rel_path: ".claude/hooks/wiring.json".to_string(),
                contents: "pwned".to_string(),
            }],
        )
        .expect_err("a symlinked directory component must be refused");
        assert!(err.contains("escapes the worktree"), "got: {err}");
        // Nothing landed outside the root, not even the intermediate dir.
        assert!(!outside.join("hooks").exists());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[test]
    fn config_injection_refuses_a_dangling_symlinked_directory_component() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), worktree.join(".claude")).unwrap();
        let root = crate::modules::fs::to_canon(&worktree);
        let err = apply_config_injection(
            &root,
            &[ConfigFile {
                rel_path: ".claude/settings.json".to_string(),
                contents: "x".to_string(),
            }],
        )
        .expect_err("an unresolvable component must be refused");
        assert!(err.contains("cannot resolve"), "got: {err}");
        assert!(!tmp.path().join("nowhere").exists());
    }

    #[test]
    fn config_injection_refuses_a_symlinked_leaf_file() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(worktree.join(".claude")).unwrap();
        let target = tmp.path().join("victim.json");
        std::fs::write(&target, "original").unwrap();
        std::os::unix::fs::symlink(&target, worktree.join(".claude/settings.json")).unwrap();
        let root = crate::modules::fs::to_canon(&worktree);
        let err = apply_config_injection(
            &root,
            &[ConfigFile {
                rel_path: ".claude/settings.json".to_string(),
                contents: "pwned".to_string(),
            }],
        )
        .expect_err("writing through a symlinked leaf must be refused");
        assert!(err.contains("symlink"), "got: {err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn config_injection_allows_a_symlink_that_stays_inside_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(worktree.join("shared")).unwrap();
        std::os::unix::fs::symlink(worktree.join("shared"), worktree.join(".claude")).unwrap();
        let root = crate::modules::fs::to_canon(&worktree);
        apply_config_injection(
            &root,
            &[ConfigFile {
                rel_path: ".claude/settings.json".to_string(),
                contents: "ok".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(worktree.join("shared/settings.json")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn config_injection_overwrites_an_existing_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::fs::write(tmp.path().join(".claude/settings.json"), "stale").unwrap();
        let root = crate::modules::fs::to_canon(tmp.path());
        apply_config_injection(
            &root,
            &[ConfigFile {
                rel_path: ".claude/settings.json".to_string(),
                contents: "fresh".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap(),
            "fresh"
        );
    }

    // --- Shell + Process Sessions (task #13) ---

    /// Give a Project a single Process definition (a long-lived command that
    /// announces its env then idles like a dev server).
    fn define_process(f: &Fixture, name: &str, command: &str, port: Option<u16>) {
        f.registry
            .commit(Event::ProjectSettingsUpdated {
                project_id: "prj-1".to_string(),
                settings: ProjectSettings {
                    processes: vec![ProcessDef {
                        name: name.to_string(),
                        command: command.to_string(),
                        port,
                    }],
                    ..ProjectSettings::default()
                },
            })
            .unwrap();
    }

    // --- launch-plan builders (pure) ---

    #[test]
    fn shell_plan_is_the_bare_shell_and_process_plan_is_a_single_c_argument() {
        assert_eq!(
            shell_launch_plan("/bin/zsh"),
            LaunchPlan {
                program: "/bin/zsh".to_string(),
                args: vec![],
            }
        );
        // The command is one argv element, never split by us — a hostile
        // command string cannot inject extra argv.
        assert_eq!(
            process_launch_plan("/bin/sh", "pnpm dev && echo $(whoami)"),
            LaunchPlan {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), "pnpm dev && echo $(whoami)".to_string()],
            }
        );
    }

    // --- Shell Session payload + a real shell in the worktree ---

    #[test]
    fn shell_spawn_carries_helmsmen_env_and_the_worktree_as_cwd() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "sh").unwrap();
        let spec =
            prepare_shell_spawn_with(&f.registry, &f.roots, &cut.workspace.id, "/bin/sh", 100, 30)
                .unwrap();
        assert_eq!(spec.program, "/bin/sh");
        assert!(spec.args.is_empty(), "a shell launches bare, PTY-interactive");
        assert_eq!(spec.cwd, cut.workspace.worktree_path);
        assert_eq!(spec.env["HELMSMEN_SLOT"], "1");
        assert_eq!(spec.env["HELMSMEN_WORKSPACE"], cut.workspace.worktree_path);
        assert_eq!(spec.env["HELMSMEN_PROJECT"], "demo");
        assert_eq!(spec.env["HELMSMEN_MAIN_CHECKOUT"], f.repo_root);
        assert!(spec.env.contains_key("TERM"));
    }

    #[test]
    fn a_real_shell_runs_in_the_worktree_with_the_helmsmen_env() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "live-sh").unwrap();
        let spec =
            prepare_shell_spawn_with(&f.registry, &f.roots, &cut.workspace.id, "/bin/sh", 80, 24)
                .unwrap();

        let rt = LocalPty::default();
        let (sink, out, exit) = sink();
        let id = rt.spawn(spec, sink).unwrap();
        // Drive the interactive shell exactly as the user would from the
        // zoom message box: type a command; its stdout proves the shell is
        // real, in the worktree, and carrying HELMSMEN_*.
        rt.write(
            &id,
            b"printf 'SHELL_UP slot=%s cwd=%s\\n' \"$HELMSMEN_SLOT\" \"$(pwd)\"\n",
        )
        .unwrap();
        // Wait on the resolved worktree path: it appears only in the shell's
        // *executed* output, never in the PTY echo of the typed command
        // (which still holds the literal `$(pwd)`), so this can't match the
        // echo before the command actually runs.
        let up = wait_for(&out, &format!("cwd={}", cut.workspace.worktree_path));
        assert!(
            up.contains("slot=1"),
            "HELMSMEN_* must reach the shell (expanded, not echoed): {up}"
        );
        rt.kill(&id).unwrap();
        exit.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    // --- Process Session payload + a real process in the worktree ---

    #[test]
    fn process_spawn_runs_the_definition_via_the_shell_and_returns_its_chip() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "proc").unwrap();
        define_process(&f, "dev", "pnpm dev", Some(5173));

        let (spec, def) = prepare_process_spawn_with(
            &f.registry,
            &f.roots,
            &cut.workspace.id,
            "dev",
            "/bin/sh",
            120,
            32,
        )
        .unwrap();
        assert_eq!(spec.program, "/bin/sh");
        assert_eq!(spec.args, vec!["-c".to_string(), "pnpm dev".to_string()]);
        assert_eq!(spec.cwd, cut.workspace.worktree_path);
        assert_eq!(spec.env["HELMSMEN_SLOT"], "1");
        assert!(spec.env.contains_key("TERM"));
        // The chip data the command echoes back: name + declared port.
        assert_eq!(def.name, "dev");
        assert_eq!(def.port, Some(5173));
    }

    #[test]
    fn a_real_process_runs_in_the_worktree_with_the_helmsmen_env() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "live-proc").unwrap();
        // A dev-server stand-in: announce the env, then stay alive.
        define_process(
            &f,
            "dev",
            "printf 'PROC_UP slot=%s cwd=%s\\n' \"$HELMSMEN_SLOT\" \"$(pwd)\"; sleep 30",
            Some(5173),
        );

        let (spec, _def) = prepare_process_spawn_with(
            &f.registry,
            &f.roots,
            &cut.workspace.id,
            "dev",
            "/bin/sh",
            80,
            24,
        )
        .unwrap();

        let rt = LocalPty::default();
        let (sink, out, exit) = sink();
        let id = rt.spawn(spec, sink).unwrap();
        let up = wait_for(&out, "PROC_UP");
        assert!(up.contains("slot=1"), "HELMSMEN_* must reach the process: {up}");
        assert!(
            up.contains(&format!("cwd={}", cut.workspace.worktree_path)),
            "the process must run in the worktree: {up}"
        );
        // Killing the Process Session ends only this process; the Runtime
        // reports it exited (the frontend then drops it from the rollup).
        rt.kill(&id).unwrap();
        exit.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(rt.status(&id).unwrap(), SessionStatus::Exited(_)));
    }

    // --- boundary validation (the same seam as agent spawn) ---

    #[test]
    fn shell_and_process_spawn_reject_an_unknown_workspace() {
        let f = fixture();
        let err = prepare_shell_spawn_with(&f.registry, &f.roots, "ws-ghost", "/bin/sh", 80, 24)
            .unwrap_err();
        assert!(err.contains("ws-ghost"), "got: {err}");
        let err = prepare_process_spawn_with(
            &f.registry, &f.roots, "ws-ghost", "dev", "/bin/sh", 80, 24,
        )
        .unwrap_err();
        assert!(err.contains("ws-ghost"), "got: {err}");
    }

    #[test]
    fn process_spawn_rejects_a_name_the_project_never_defined() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "x").unwrap();
        define_process(&f, "dev", "pnpm dev", None);
        let err = prepare_process_spawn_with(
            &f.registry,
            &f.roots,
            &cut.workspace.id,
            "ghost-proc",
            "/bin/sh",
            80,
            24,
        )
        .unwrap_err();
        assert!(err.contains("ghost-proc"), "got: {err}");
    }

    #[test]
    fn shell_and_process_spawn_reject_zero_size() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "z").unwrap();
        define_process(&f, "dev", "pnpm dev", None);
        assert!(
            prepare_shell_spawn_with(&f.registry, &f.roots, &cut.workspace.id, "/bin/sh", 0, 24)
                .is_err()
        );
        assert!(prepare_process_spawn_with(
            &f.registry, &f.roots, &cut.workspace.id, "dev", "/bin/sh", 80, 0,
        )
        .is_err());
    }

    #[test]
    fn shell_spawn_rejects_a_deleted_worktree() {
        let f = fixture();
        let cut = worktree::cut(&f.registry, &f.roots, "prj-1", "gone").unwrap();
        std::fs::remove_dir_all(&cut.workspace.worktree_path).unwrap();
        let err =
            prepare_shell_spawn_with(&f.registry, &f.roots, &cut.workspace.id, "/bin/sh", 80, 24)
                .unwrap_err();
        assert!(err.contains("gone"), "got: {err}");
    }
}
