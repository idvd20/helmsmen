//! Local-clone detection — thin git glue for the add-Project flow.
//!
//! Reads only git *metadata* (ref names) via the `git` binary to detect the
//! base branch. It never reads a file inside the repo for configuration:
//! prefills come from the pure core (`core::project::prefill`), derived
//! from the canonical repo root and the user's home directory only.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::modules::core::project::prefill;

/// What the add-Project form is prefilled with; every field is editable
/// before the Project is committed to the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetection {
    pub repo_root: String,
    pub name: String,
    pub base_branch: String,
    pub worktree_home: String,
    pub branch_template: String,
}

/// Validate a user-picked path at the boundary and detect its Project
/// prefill. Rejects anything that does not canonicalize to a directory
/// inside a git work tree.
pub fn detect_project(raw_path: &str) -> Result<ProjectDetection, String> {
    let root = resolve_repo_root(raw_path)?;
    let repo_root = crate::modules::fs::to_canon(&root);
    let base_branch = detect_base_branch(&root);
    let home = dirs::home_dir().ok_or("cannot resolve the home directory")?;
    let pf = prefill(&repo_root, &home.to_string_lossy()).map_err(|e| e.to_string())?;
    Ok(ProjectDetection {
        repo_root,
        name: pf.name,
        base_branch,
        worktree_home: pf.worktree_home,
        branch_template: pf.branch_template,
    })
}

/// Boundary validation for a picked path: must be non-hostile as a string,
/// canonicalize on the real filesystem (resolving `..` and symlinks), be a
/// directory, and sit inside a git work tree. Returns the canonical git
/// toplevel so the registry never stores a subdirectory.
pub fn resolve_repo_root(raw_path: &str) -> Result<PathBuf, String> {
    if raw_path.trim().is_empty() {
        return Err("path is empty".to_string());
    }
    if raw_path.contains('\0') {
        return Err("path contains a NUL byte".to_string());
    }
    let canon = std::fs::canonicalize(raw_path)
        .map_err(|e| format!("cannot resolve path {raw_path:?}: {e}"))?;
    if !canon.is_dir() {
        return Err(format!("not a directory: {}", canon.display()));
    }
    let inside = run_git(&canon, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.as_deref() != Some("true") {
        return Err(format!("not a git work tree: {}", canon.display()));
    }
    let toplevel = run_git(&canon, &["rev-parse", "--show-toplevel"])?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("git reported no toplevel for {}", canon.display()))?;
    std::fs::canonicalize(&toplevel)
        .map_err(|e| format!("cannot resolve git toplevel {toplevel:?}: {e}"))
}

/// Base-branch detection, most-authoritative first:
/// 1. `origin/HEAD` (what the remote calls its default branch),
/// 2. a local `main` or `master`,
/// 3. the currently checked-out branch,
/// 4. `"main"` as the editable last resort.
fn detect_base_branch(root: &Path) -> String {
    if let Ok(Some(short)) = run_git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        if let Some(branch) = strip_remote_prefix(&short) {
            return branch;
        }
    }
    for candidate in ["main", "master"] {
        if let Ok(Some(_)) = run_git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{candidate}"),
            ],
        ) {
            return candidate.to_string();
        }
    }
    if let Ok(Some(current)) = run_git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        if !current.is_empty() {
            return current;
        }
    }
    "main".to_string()
}

/// `"origin/main"` -> `"main"`; keeps nested branch names intact
/// (`"origin/feat/x"` -> `"feat/x"`).
pub(crate) fn strip_remote_prefix(short_ref: &str) -> Option<String> {
    short_ref
        .split_once('/')
        .map(|(_, branch)| branch.to_string())
        .filter(|branch| !branch.is_empty())
}

/// Run git with `-C dir`. `Ok(None)` on a non-zero exit (the caller treats
/// it as "no answer"), `Err` only when git itself cannot be spawned.
fn run_git(dir: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    crate::modules::proc::hide_console(&mut cmd);
    let output = cmd.output().map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git must be runnable in tests");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    fn init_repo(dir: &Path, initial_branch: &str) {
        git(dir, &["init", "-b", initial_branch, "."]);
    }

    #[test]
    fn strip_remote_prefix_handles_nested_and_empty() {
        assert_eq!(strip_remote_prefix("origin/main"), Some("main".into()));
        assert_eq!(strip_remote_prefix("origin/feat/x"), Some("feat/x".into()));
        assert_eq!(strip_remote_prefix("origin/"), None);
        assert_eq!(strip_remote_prefix("nomatch"), None);
    }

    #[test]
    fn detects_toplevel_name_and_prefills_from_a_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("demo-repo");
        let sub = repo.join("src/deep");
        std::fs::create_dir_all(&sub).unwrap();
        init_repo(&repo, "trunk");

        let detection = detect_project(sub.to_str().unwrap()).unwrap();
        assert_eq!(
            detection.repo_root,
            crate::modules::fs::to_canon(std::fs::canonicalize(&repo).unwrap())
        );
        assert_eq!(detection.name, "demo-repo");
        // No origin/HEAD, no main/master -> current branch.
        assert_eq!(detection.base_branch, "trunk");
        assert!(
            detection.worktree_home.ends_with("demo-repo"),
            "worktree home {:?} should end with the repo name",
            detection.worktree_home
        );
        assert_eq!(detection.branch_template, "helm/{slug}");
    }

    #[test]
    fn origin_head_wins_over_local_branches() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo, "main");
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        );

        let detection = detect_project(repo.to_str().unwrap()).unwrap();
        assert_eq!(detection.base_branch, "develop");
    }

    #[test]
    fn falls_back_to_local_master_when_no_origin_head() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo, "work");
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
                "x",
            ],
        );
        git(&repo, &["branch", "master"]);

        let detection = detect_project(repo.to_str().unwrap()).unwrap();
        assert_eq!(detection.base_branch, "master");
    }

    /// AC (task #4): no file inside the repo is ever read for
    /// configuration — a repo must never configure its own trust. A
    /// hostile "config" file planted in the clone must not influence the
    /// detected prefill.
    #[test]
    fn repo_files_never_configure_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("demo-repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo, "main");
        std::fs::write(
            repo.join(".helmsmen.json"),
            br#"{ "baseBranch": "pwned", "worktreeHome": "/", "branchTemplate": "pwned/{slug}" }"#,
        )
        .unwrap();

        let detection = detect_project(repo.to_str().unwrap()).unwrap();
        assert_eq!(detection.base_branch, "main");
        assert_eq!(detection.branch_template, "helm/{slug}");
        assert!(detection.worktree_home.ends_with("demo-repo"));
    }

    #[test]
    fn rejects_a_directory_that_is_not_a_git_work_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let err = detect_project(tmp.path().to_str().unwrap()).unwrap_err();
        assert!(err.contains("not a git work tree"), "got: {err}");
    }

    #[test]
    fn rejects_missing_and_hostile_paths() {
        assert!(detect_project("").is_err());
        assert!(detect_project("   ").is_err());
        assert!(detect_project("/definitely/not/a/real/path/xyz").is_err());
        assert!(detect_project("with\0nul").is_err());
    }
}
