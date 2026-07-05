# Fork posture — tracking upstream Terax

Helmsmen is a fork of [Terax](https://github.com/crynta/terax-ai) that **tracks
upstream, never strips**: Helmsmen lives in new modules only (frontend: helm /
inbox / workspaces; backend: core / runtime / harness / hooks); Terax's AI/agents
modules stay in-tree, hidden behind a setting; product-facing rename only, no
file/crate renames; shared-file edits limited to the enumerated integration
points below. Re-evaluate the posture at M4; freeze knowingly if merge cost
exceeds value.

## Remotes

| Remote     | URL                                        | Role                          |
| ---------- | ------------------------------------------ | ----------------------------- |
| `origin`   | git@github.com:idvd20/helmsmen.git         | This fork; issues live here   |
| `upstream` | https://github.com/crynta/terax-ai.git     | Terax; fetch + merge only     |

## Fork base

- Merged **Terax v0.8.2** (tag) into the planning repo's `main` with
  `--allow-unrelated-histories` — merge commit `da8b569`. Planning history
  stays in `main`'s ancestry and is also pinned by the `planning-complete` tag.
- The only file conflict was `README.md` (resolved: upstream README with a
  Helmsmen fork banner prepended). No `docs/` path overlapped.

## Security-fix confirmation (M0 acceptance)

The "OSC path-traversal CVE fix" required by the PRD is upstream commit
**`32a5ec9` — security(terminal): gate OSC 7 cwd updates by OSC 133 in-command
state** (part of PR #319 "security-hardening", merge `0e9296d`): untrusted
command output could emit `\e]7;file://x/etc\e\` and silently relocate the AI
tool layer's relative-path root. Verified present in `v0.8.2` via
`git merge-base --is-ancestor 32a5ec9 v0.8.2`.

Also confirmed in the base: `b7ba0ee` (CR/LF + C0 controls blocked in the shell
command guard), `2bfffae` (path guard canonical re-check vs symlink traversal),
`b5f6534` (workspace auth on spawn, sentinel nonce, pathspec guard).

## Merge cadence

- **Per milestone**: `git fetch upstream --tags && git merge <next release tag>`
  (fall back to `upstream/main` if a needed fix is unreleased). Resolve, run the
  CI gates, commit the merge.
- **Security fixes**: cherry-pick immediately (`git cherry-pick <sha>`), don't
  wait for the milestone merge.
- Base at merge time: `v0.8.2`; upstream `main` was 33 commits ahead (editor/LSP
  features only) — they arrive with the next milestone merge.

## Enumerated integration points (shared-file edits)

The only Terax files Helmsmen may edit. Grow this list deliberately; anything
not listed is upstream's territory.

- `src-tauri/src/modules/mod.rs` — module declarations for the new Helmsmen
  backend modules (`core`, `registry`; later `runtime`, `harness`, `hooks`).
- `src-tauri/src/lib.rs` — registration only: manage
  `registry::RegistryState` in `.setup()` and list the `helm_*` commands in
  `invoke_handler` (tasks #4, #5).
- `src/main.tsx` — install the Helm dev console (`window.helmsmen`,
  task #4); later the Helm surface registration.
- _Still expected during M1+ (tracked in issue #2):_ settings schema (Terax
  AI side-panel toggle).

Workspace-root authorization for the cut pipeline (task #5) needed **no**
shared-file edit: `helm_cut_workspace` authorizes each cut worktree path via
the existing public API `modules::workspace::WorkspaceRegistry::authorize`
(`workspace.rs` itself stays untouched).

## Local, non-committed state

`.git/info/exclude` carries the per-clone excludes (`.pipeline/`,
`CLAUDE.local.md`); `.pipeline/state.json` is the cross-skill pipeline state and
never gets committed. When cloning fresh, re-add the excludes by hand.
