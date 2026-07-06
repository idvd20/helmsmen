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
  backend modules (`core`, `registry`, `runtime`, `harness`; later `hooks`).
- `src-tauri/src/lib.rs` — registration only: manage
  `registry::RegistryState` in `.setup()`, manage `runtime::RuntimeState`,
  and list the `helm_*` commands in `invoke_handler` (tasks #4, #5, #6;
  #7 adds the Project-settings/Profile commands, #8 adds
  `helm_cut_pipeline` to the same list).
- `src-tauri/Cargo.toml` — dependency additions only (task #8 adds `glob`
  for carry-over matching; it was already in the lock as a transitive
  dependency, and `cargo-deny check licenses` gates every addition).
- `src/main.tsx` — install the Helm dev console (`window.helmsmen`,
  task #4); later the Helm surface registration.
- _Still expected during M1+ (tracked in issue #2):_ settings schema (Terax
  AI side-panel toggle).

Workspace-root authorization for the cut pipeline (task #5) needed **no**
shared-file edit: `helm_cut_workspace` authorizes each cut worktree path via
the existing public API `modules::workspace::WorkspaceRegistry::authorize`
(`workspace.rs` itself stays untouched).

The runtime/harness layer (task #6) likewise stays out of upstream modules:
`modules::runtime::local_pty` builds on the `portable-pty` crate directly
(Terax's `modules::pty` is untouched), and Agent Session spawns re-use
`WorkspaceRegistry::authorize` for the cut worktree only.

The ambient cut pipeline (task #8) lives entirely in Helmsmen modules
(`modules::registry::pipeline` orchestrating, lifecycle events in
`modules::core::cut`). It touches upstream only through existing public
seams — `WorkspaceRegistry::authorize` and `modules::proc::hide_console` —
plus the enumerated `lib.rs` command registration and the `glob`
dependency in `Cargo.toml`.

The Helm wall (task #10) added **no** shared-file edit. It lives entirely
in `src/modules/helm`: the tested pure view-model (`viewModel.ts` — status
rollup, rank sort, header counts, elapsed minutes, all deterministic over
data), the presentational React surface (`Helm.tsx`, `WorkspaceCard.tsx`),
and the host container `HelmView` (`HelmView.tsx`). The status rollup is
also mirrored in the pure core (`core::cut::roll_up_status` + `rank`) so
the "derived, never stored" rule lives on both sides of the seam.
`HelmView` is the surface #9 (New Workspace) and #12 (Zoom) build on;
Session chips call an injected `onZoomSession` — a logging placeholder now,
wired so #12 takes it over. **Mounting into the app shell is kept interim
and minimal**: the existing `window.helmsmen` dev console (installed in
`main.tsx` at task #4) grew `openHelm()` / `closeHelm()`, which mount the
wall as a full-window overlay. Promoting the Helm to a real route / the
default home view is a later upstream integration point, deliberately left
for when the view switch (`esc`/`t`) and repo picker (#14) land.

The Zoom / "take the wheel" view (task #12) added **no** upstream Terax
edit. It lives entirely in the new `src/modules/workspaces` module: pure,
tested navigation logic (`keymap.ts` — key→action map, tab-index and
`[`/`]` hop math; `zoomModel.ts` — Session→tab projection, zoom-target
resolution, message-line) and the React shell over it (`Zoom.tsx`,
`PtyPane.tsx`, `Quarterdeck.tsx`). The PTY pane reuses the helm module's
safe rendering path (hostile bytes → `createStreamBuffer` → `textContent`
only) and the backend runtime **unchanged**: attach-on-zoom is the existing
`helm_attach_agent` (scrollback replays, then live), and `m`'s line-to-PTY
is the existing `helm_write_agent` — both already pinned by the runtime
conformance suite (`case_attach_replays_scrollback_then_streams`,
`case_write_reaches_stdin`). The **only** helm-module edit was the
container-level wiring the #10 seam left for it: `devConsole.ts`'s
`openHelm()`/`closeHelm()` now mount the quarterdeck (wall + zoom) instead
of the bare wall, and `spawnAgentView` registers each spawn in the interim
`sessionStore` so a Workspace can be zoomed into before Session facts land
on the wall. No status-dot / Session-facts render code (#11's) was touched.
Promoting the zoom to a real app-shell route — and giving the wall a card
cursor so `↵` zooms a *selected* card (it currently zooms the first
zoomable Workspace) — is a later upstream integration point, left for when
the view switch and wall keyboard-nav land.

## Local, non-committed state

`.git/info/exclude` carries the per-clone excludes (`.pipeline/`,
`CLAUDE.local.md`); `.pipeline/state.json` is the cross-skill pipeline state and
never gets committed. When cloning fresh, re-add the excludes by hand.
