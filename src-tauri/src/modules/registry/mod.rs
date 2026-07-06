//! Helmsmen registry — the imperative shell around the pure core.
//!
//! One versioned JSON file in app-data, written atomically (temp file +
//! fsync + rename in the same directory). Every mutation goes through the
//! pure core's `apply(state, event) -> state`; this module only loads,
//! persists, and hands snapshots out. A repo never configures its own
//! trust: nothing here reads configuration from inside a repo.

pub mod commands;
pub mod detect;
pub mod pipeline;
pub mod worktree;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::modules::core::state::{apply, CoreState, Event};

/// Current registry file schema version. Bump + migrate on shape changes.
pub const REGISTRY_VERSION: u32 = 1;
/// File name inside the Helmsmen app-data directory.
pub const REGISTRY_FILE: &str = "registry.json";

#[derive(Serialize)]
struct RegistryFileRef<'a> {
    version: u32,
    state: &'a CoreState,
}

#[derive(Deserialize)]
struct RegistryFileOwned {
    version: u32,
    #[serde(default)]
    state: CoreState,
}

enum Inner {
    Ready {
        path: PathBuf,
        state: CoreState,
    },
    /// The registry file exists but cannot be used (corrupt or written by
    /// a newer app). We refuse to operate rather than clobber user data.
    Poisoned {
        error: String,
    },
}

/// Tauri-managed registry state. Thread-safe; commands are thin wrappers
/// over [`RegistryState::snapshot`] and [`RegistryState::commit`].
pub struct RegistryState {
    inner: Mutex<Inner>,
}

impl RegistryState {
    /// Load the registry from `dir` (created lazily on first write). A
    /// missing file is an empty registry; an unreadable file poisons the
    /// registry so it is never overwritten.
    pub fn load(dir: PathBuf) -> Self {
        let path = dir.join(REGISTRY_FILE);
        let inner = match read_registry(&path) {
            Ok(state) => Inner::Ready { path, state },
            Err(error) => {
                log::error!("helmsmen registry unusable: {error}");
                Inner::Poisoned { error }
            }
        };
        RegistryState {
            inner: Mutex::new(inner),
        }
    }

    /// Current state, cloned (the pure core owns the shape).
    pub fn snapshot(&self) -> Result<CoreState, String> {
        match &*self.lock() {
            Inner::Ready { state, .. } => Ok(state.clone()),
            Inner::Poisoned { error } => Err(error.clone()),
        }
    }

    /// Apply an event through the pure core and persist atomically. The
    /// in-memory state only advances if the write succeeded, so memory and
    /// disk never diverge.
    pub fn commit(&self, event: Event) -> Result<CoreState, String> {
        match &mut *self.lock() {
            Inner::Ready { path, state } => {
                let next = apply(state.clone(), event).map_err(|e| e.to_string())?;
                write_registry_atomic(path, &next)?;
                *state = next.clone();
                Ok(next)
            }
            Inner::Poisoned { error } => Err(error.clone()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("helmsmen registry mutex poisoned")
    }
}

fn read_registry(path: &Path) -> Result<CoreState, String> {
    let bytes = match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CoreState::default());
        }
        Err(e) => return Err(format!("cannot read registry {}: {e}", path.display())),
        Ok(bytes) => bytes,
    };
    let file: RegistryFileOwned = serde_json::from_slice(&bytes)
        .map_err(|e| format!("registry {} is not valid JSON: {e}", path.display()))?;
    if file.version != REGISTRY_VERSION {
        return Err(format!(
            "registry {} has version {} but this build supports {}; refusing to touch it",
            path.display(),
            file.version,
            REGISTRY_VERSION
        ));
    }
    Ok(file.state)
}

fn write_registry_atomic(path: &Path, state: &CoreState) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("registry path {} has no parent", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let json = serde_json::to_vec_pretty(&RegistryFileRef {
        version: REGISTRY_VERSION,
        state,
    })
    .map_err(|e| format!("cannot serialize registry: {e}"))?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("cannot create temp file in {}: {e}", parent.display()))?;
    tmp.write_all(&json)
        .and_then(|()| tmp.as_file().sync_all())
        .map_err(|e| format!("cannot write registry: {e}"))?;
    tmp.persist(path)
        .map_err(|e| format!("cannot replace registry {}: {e}", path.display()))?;

    // Best-effort directory sync so the rename itself is durable.
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::project::{Project, DEFAULT_BRANCH_TEMPLATE};
    use crate::modules::core::settings::{ProcessDef, ProjectSettings};

    fn project(id: &str, repo_root: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "demo".to_string(),
            repo_root: repo_root.to_string(),
            base_branch: "main".to_string(),
            worktree_home: "/home/dev/.helmsmen/worktrees/demo".to_string(),
            branch_template: DEFAULT_BRANCH_TEMPLATE.to_string(),
            settings: Default::default(),
        }
    }

    fn added(id: &str, repo_root: &str) -> Event {
        Event::ProjectAdded {
            project: project(id, repo_root),
        }
    }

    #[test]
    fn fresh_registry_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let reg = RegistryState::load(dir.path().join("helmsmen"));
        assert_eq!(reg.snapshot().unwrap(), CoreState::default());
    }

    #[test]
    fn committed_project_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");

        let reg = RegistryState::load(home.clone());
        reg.commit(added("prj-1", "/home/dev/src/demo")).unwrap();
        drop(reg);

        // "Restart": a brand-new RegistryState over the same directory.
        let reg2 = RegistryState::load(home);
        let state = reg2.snapshot().unwrap();
        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.projects[0].id, "prj-1");
        assert_eq!(state.projects[0].repo_root, "/home/dev/src/demo");
    }

    #[test]
    fn registry_file_is_versioned_json() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        let reg = RegistryState::load(home.clone());
        reg.commit(added("prj-1", "/home/dev/src/demo")).unwrap();

        let raw = std::fs::read_to_string(home.join(REGISTRY_FILE)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["version"], REGISTRY_VERSION);
        assert_eq!(json["state"]["projects"][0]["id"], "prj-1");
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        let reg = RegistryState::load(home.clone());
        reg.commit(added("prj-1", "/home/dev/src/demo")).unwrap();

        let entries: Vec<_> = std::fs::read_dir(&home)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec![REGISTRY_FILE.to_string()]);
    }

    #[test]
    fn corrupt_registry_is_poisoned_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join(REGISTRY_FILE);
        std::fs::write(&path, b"{ not json").unwrap();

        let reg = RegistryState::load(home);
        assert!(reg.snapshot().is_err());
        assert!(reg.commit(added("prj-1", "/home/dev/src/demo")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[test]
    fn newer_registry_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join(REGISTRY_FILE);
        std::fs::write(&path, br#"{ "version": 99, "state": { "projects": [] } }"#).unwrap();

        let reg = RegistryState::load(home);
        let err = reg.snapshot().unwrap_err();
        assert!(err.contains("version 99"), "unexpected error: {err}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            br#"{ "version": 99, "state": { "projects": [] } }"#
        );
    }

    #[test]
    fn rejected_event_changes_neither_memory_nor_disk() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        let reg = RegistryState::load(home.clone());
        reg.commit(added("prj-1", "/home/dev/src/demo")).unwrap();
        let before = std::fs::read(home.join(REGISTRY_FILE)).unwrap();

        // Same repo root -> pure core rejects the event.
        assert!(reg.commit(added("prj-2", "/home/dev/src/demo")).is_err());
        assert_eq!(reg.snapshot().unwrap().projects.len(), 1);
        assert_eq!(std::fs::read(home.join(REGISTRY_FILE)).unwrap(), before);
    }

    // --- task #7: Project settings + Profiles are registry-only state ---

    fn demo_settings() -> ProjectSettings {
        ProjectSettings {
            setup_script: "pnpm install --frozen-lockfile".to_string(),
            carry_over_globs: vec![".env*".to_string()],
            processes: vec![ProcessDef {
                name: "dev".to_string(),
                command: "pnpm dev".to_string(),
                port: Some(5173),
            }],
        }
    }

    #[test]
    fn settings_and_profiles_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");

        let reg = RegistryState::load(home.clone());
        reg.commit(added("prj-1", "/home/dev/src/demo")).unwrap();
        reg.commit(Event::ProjectSettingsUpdated {
            project_id: "prj-1".to_string(),
            settings: demo_settings(),
        })
        .unwrap();
        let mut profile = reg.snapshot().unwrap().profiles[0].clone();
        profile.verify_command = "cargo test".to_string();
        reg.commit(Event::ProfileUpdated {
            profile: profile.clone(),
        })
        .unwrap();
        drop(reg);

        // "Restart": a brand-new RegistryState over the same directory.
        let state = RegistryState::load(home).snapshot().unwrap();
        assert_eq!(state.projects[0].settings, demo_settings());
        assert_eq!(state.profiles.len(), 5, "seeds must persist");
        assert_eq!(state.profiles[0], profile, "the edit must persist");
    }

    #[test]
    fn settings_and_profiles_live_only_in_the_registry_file() {
        // The single place these settings are stored is registry.json in
        // app-data — never a file inside the repo (a repo must not be
        // able to configure its own trust).
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        let reg = RegistryState::load(home.clone());
        reg.commit(added("prj-1", "/home/dev/src/demo")).unwrap();
        reg.commit(Event::ProjectSettingsUpdated {
            project_id: "prj-1".to_string(),
            settings: demo_settings(),
        })
        .unwrap();

        let raw = std::fs::read_to_string(home.join(REGISTRY_FILE)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let project = &json["state"]["projects"][0];
        assert_eq!(
            project["settings"]["setupScript"],
            "pnpm install --frozen-lockfile"
        );
        assert_eq!(project["settings"]["carryOverGlobs"][0], ".env*");
        assert_eq!(project["settings"]["processes"][0]["name"], "dev");
        assert_eq!(json["state"]["profiles"].as_array().unwrap().len(), 5);
        // And the registry file is still the only file written.
        let entries: Vec<_> = std::fs::read_dir(&home)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec![REGISTRY_FILE.to_string()]);
    }

    #[test]
    fn pre_task7_registry_file_loads_and_stays_editable() {
        // A v1 file written before task #7: no `settings` on projects, no
        // `profiles` key. It must keep loading (additive fields only) and
        // accept new events.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("helmsmen");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(REGISTRY_FILE),
            br#"{ "version": 1, "state": { "projects": [{
                "id": "prj-old",
                "name": "demo",
                "repoRoot": "/home/dev/src/demo",
                "baseBranch": "main",
                "worktreeHome": "/home/dev/.helmsmen/worktrees/demo",
                "branchTemplate": "helm/{slug}"
            }], "workspaces": [] } }"#,
        )
        .unwrap();

        let reg = RegistryState::load(home);
        let state = reg.snapshot().unwrap();
        assert_eq!(state.projects[0].settings, ProjectSettings::default());
        assert!(state.profiles.is_empty());

        let next = reg
            .commit(Event::ProjectSettingsUpdated {
                project_id: "prj-old".to_string(),
                settings: demo_settings(),
            })
            .unwrap();
        assert_eq!(next.projects[0].settings, demo_settings());
    }

    /// Architecture guard for the PRD's functional-core rule: the pure core
    /// must stay free of PTY, async, HTTP, filesystem, process, and Tauri
    /// imports. Runs against the source tree, so a violating import fails
    /// CI even if it compiles.
    #[test]
    fn pure_core_has_no_io_imports() {
        let core_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/modules/core");
        let forbidden = [
            "std::fs",
            "std::process",
            "std::net",
            "std::thread",
            "tokio",
            "reqwest",
            "portable_pty",
            "portable-pty",
            "async fn",
            "async move",
            "tauri",
            "tempfile",
        ];
        for entry in std::fs::read_dir(&core_dir).expect("core module must exist") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for token in forbidden {
                assert!(
                    !source.contains(token),
                    "pure core file {} contains forbidden token {token:?}",
                    path.display()
                );
            }
        }
    }
}
