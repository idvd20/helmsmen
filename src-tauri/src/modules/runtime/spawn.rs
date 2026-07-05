//! Assemble a SpawnSpec for an Agent Session in a cut Workspace: the seam
//! where Harness (what to run) meets Runtime (where it runs).
//!
//! Boundary validation happens here: the Workspace must exist in the
//! Helmsmen registry, its worktree must still canonicalize on disk, and
//! injected config paths must stay inside the worktree. Everything
//! spawned in a Workspace carries the cut's `HELMSMEN_*` env.

use std::path::{Component, Path};

use crate::modules::core::project::validate_abs_path;
use crate::modules::core::workspace::helmsmen_env;
use crate::modules::harness::{ConfigFile, Harness, LaunchContext};
use crate::modules::registry::RegistryState;
use crate::modules::workspace::WorkspaceRegistry;

use super::SpawnSpec;

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
    prepare_spawn_with(registry, roots, workspace_id, harness, cols, rows)
}

/// Trait-typed variant so tests (and later Profiles) can hand in any
/// Harness.
pub fn prepare_spawn_with(
    registry: &RegistryState,
    roots: &WorkspaceRegistry,
    workspace_id: &str,
    harness: &dyn Harness,
    cols: u16,
    rows: u16,
) -> Result<SpawnSpec, String> {
    if cols == 0 || rows == 0 {
        return Err("spawn: cols and rows must be non-zero".to_string());
    }

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

    // The stored path is data, not truth: re-validate and re-resolve it
    // against the real filesystem before anything spawns in it.
    validate_abs_path("worktreePath", &workspace.worktree_path).map_err(|e| e.to_string())?;
    let worktree = std::fs::canonicalize(&workspace.worktree_path)
        .map(|p| crate::modules::fs::to_canon(&p))
        .map_err(|e| format!("worktree {:?} is gone: {e}", workspace.worktree_path))?;

    // Re-authorize the worktree as a Terax workspace root: the registry
    // entry is durable, the in-memory authorization is not (app restart).
    // Idempotent, and scoped to exactly this path like the cut itself.
    roots
        .authorize(&worktree)
        .map_err(|e| format!("cannot authorize workspace root {worktree:?}: {e}"))?;

    let mut env = helmsmen_env(project, workspace);
    let ctx = LaunchContext {
        workspace_root: &worktree,
        env: &env,
    };
    apply_config_injection(&worktree, &harness.config_injection(&ctx))?;
    let plan = harness.launch_plan(&ctx);

    // A GUI-launched app has no useful TERM; interactive agents need one.
    // HELMSMEN_* stays exactly the specced set, TERM is a spawn default.
    env.entry("TERM".to_string())
        .or_insert_with(|| "xterm-256color".to_string());

    Ok(SpawnSpec {
        program: plan.program,
        args: plan.args,
        cwd: worktree,
        env,
        cols,
        rows,
    })
}

/// Write the Harness's config files into the worktree (M3: hook wiring).
/// Paths are hostile until proven worktree-relative: absolute paths and
/// any `..` component are rejected before a byte is written.
fn apply_config_injection(worktree: &str, files: &[ConfigFile]) -> Result<(), String> {
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
        let dest = Path::new(worktree).join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&dest, &file.contents)
            .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    use crate::modules::core::project::Project;
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

        let spec = prepare_spawn_with(&f.registry, &f.roots, &cut.workspace.id, &agent, 120, 32)
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
        prepare_spawn_with(&f.registry, &fresh_roots, &cut.workspace.id, &agent, 80, 24).unwrap();
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
}
