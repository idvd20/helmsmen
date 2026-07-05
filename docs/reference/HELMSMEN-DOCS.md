# Helmsmen — Documentation

> Companion to `HANDOFF.md` (the build spec) and `RESEARCH.md` (reference & license catalogue). This document explains what Helmsmen is, how it works, and how you use it day to day. The prototype UI (`helmsmen-ui-prototype.jsx`) implements the screens described in §5.

---

## 1. What Helmsmen is

Helmsmen is a personal desktop app for running CLI coding agents — primarily Claude Code — in parallel across your projects. Every task gets its own git worktree, its own agent, and its own lifecycle, and the GUI is where you live: watching status, answering approval requests, reviewing diffs, running verification, and opening PRs, without hopping between terminal tabs.

It is a fork of Terax (Tauri 2 + React 19), which supplies the terminal renderer, file tree, diff view, git graph, and editor. Helmsmen adds the orchestration layer on top: projects, workspaces, profiles, harnesses, runtimes, a hook-based control plane, and an approval inbox.

The design bet is simple: with one agent, you watch it. With five agents, watching doesn't scale — what scales is *structured signals* (state, cost, verification) plus *one queue* for everything that needs your judgment. Helmsmen is built around that queue.

## 2. Core concepts

Six nouns define the whole system. Everything in the UI and the backend maps onto them.

| Concept | What it is | Example |
|---|---|---|
| **Project** | A repo you've added, with a default base branch. Owns its workspaces, profiles, and history. | `rentvine-app` (base `main`) |
| **Workspace** | One task = one git worktree on its own branch. Owns its sessions; its status is a rollup of theirs. | `feat/tenant-portal-a11y` |
| **Session** | One live process inside a workspace's worktree: `{ kind, harness?, runtime, status }`. Kinds: `Agent`, `Shell` (yours), `Process` (dev server), `Reviewer`. | claude·tmux + your shell + `dev:5173` |
| **Profile** | A reusable launch config: `{ prompt/CLAUDE.md snippet, model, color, mcp_set, verify_cmd }`. The color follows the workspace everywhere in the UI. | "Frontend" — sonnet, cyan, Playwright + GitHub MCPs, `pnpm vitest run` |
| **Harness** | *What* agent runs and *how Helmsmen talks to it*, expressed as capability flags (hooks, session resume, cost reporting, MCP config, stream-json). Claude Code is first-class; others degrade gracefully. | `claude-code`, `codex`, `opencode` |
| **Runtime** | *Where* a session's process lives, and therefore what it survives. Chosen per session. | `LocalPty`, `Tmux`, `SshTmux(ship)` |
| **Ship** *(future, M8)* | A registered machine in your fleet — another Linux box, a Windows laptop, a rented cloud runner — that runs sessions over SSH + tmux. The device running Helmsmen is the **Flagship**: it holds the conn; Ships do the rowing. | `rig` (Linux desktop), `probook` (Windows) |

Workspace status is derived, never stored: any session blocked → **blocked**; else any working → **working**; all done → **done**; otherwise **idle**. The Helm card is workspace-level with per-session chips; drilling in gives each session its own tab over the same worktree.

The separation between Harness and Runtime matters: "Claude Code in a tmux session" and "Codex in a local PTY" are just two combinations of the same two traits, and every other subsystem (state detection, cost tracking, the sidebar) is written against the traits, never against a concrete choice.

### Agent status

Every workspace is always in exactly one of four states, shown as a colored dot on its sidebar row and grid card. **Working** (amber, pulsing) means the agent is actively executing. **Blocked** (red) means it's waiting on you — usually an item in the Approval Inbox, sometimes a question typed at the prompt. **Done** (green) means the run finished; pair with the verify badge before reviewing. **Idle** (grey) means a live session with no active task, typically a resumed tmux session waiting for a prompt.

### Persistence tiers

The runtime choice is really a persistence choice, and the UI is honest about what each tier survives.

| Runtime | Survives | Doesn't survive | When to use |
|---|---|---|---|
| `LocalPty` | UI reload | App quit, crash | Quick interactive tasks |
| `Tmux` | App quit, crash, relaunch (reattaches) | Machine sleep, reboot | Long runs — the default for anything over a few minutes |
| `SshTmux` (M8) | The Flagship sleeping, closing, or rebooting — the session lives on the Ship | The Ship itself going down | True walk-away runs on hardware you own |
| `CloudRunner` (post-M8) | Everything above, on a rented Ship | — | Walk-away without owning hardware |

## 3. How it works — the control plane

This is the architectural heart, and the biggest departure from tools that scrape terminal output.

Helmsmen talks to Claude Code over **two channels**:

**The PTY channel** is the terminal itself — what you see in the Agent pane and type into. It's for interaction and rendering, and per the Terax CVE lesson, its output is treated as hostile: nothing in the byte stream may ever trigger a privileged action.

**The hook channel** is the control plane. When Helmsmen creates a workspace, it injects a `.claude/settings.json` into the worktree wiring Claude Code's hooks to a small local HTTP endpoint served by the Tauri backend (`127.0.0.1:<port>`, loopback only, per-session token). From then on, Claude Code *tells* Helmsmen what's happening as typed, structured events:

```
Claude Code (in worktree)
   │  PreToolUse / PostToolUse / Notification / Stop / SubagentStop
   ▼
POST http://127.0.0.1:<port>/hook   ──►  Tauri backend
                                          │ pure core: apply(state, event)
                                          ▼
                                    emit → sidebar dots, grid cards,
                                           approval inbox, cost meters
```

Each Claude Code capability maps onto a hook event, so nothing important is inferred from pixels:

| Signal | Source |
|---|---|
| Working | `PreToolUse` / activity events |
| Blocked (needs approval) | `PreToolUse` hook returns `defer` |
| Blocked (asking you a question) | `Notification: permission_prompt` / `idle_prompt` |
| Done | `Stop` event |
| Token/cost meters | usage payloads in result/stop events |
| Subagent activity | `SubagentStart` / `SubagentStop` |

Terminal-buffer classification still exists, but only as the *fallback* for BYOA harnesses that don't support hooks — their status dot is heuristic and the UI labels it as such.

### The Approval Inbox and `defer`

Claude Code's permission hooks support three answers to "may I run this tool?": allow, deny, and **defer**. Defer pauses the tool call without killing the run — the agent's full context, tool history, and plan survive the wait.

Helmsmen's policy hooks return `defer` for anything on your risk list (force pushes, env-file writes, destructive shell, production-ish anything). The deferred call lands in the **Approval Inbox** — one queue across every workspace and project — showing the exact tool and arguments in monospace, which workspace asked, and why the rule fired. **Allow** resumes the run exactly where it paused; **Deny** sends the agent looking for another approach; **Edit** lets you amend the tool input before allowing.

This is the feature that makes parallel agents actually parallel: the human bottleneck (answering agents) is consolidated into a single pane you can clear in seconds, instead of five terminal tabs each silently waiting on a y/n.

### MCP composition

MCP servers are configured per profile as a named `mcp_set`. At spawn, Helmsmen composes the set into the worktree's `.mcp.json` — so a "Frontend" workspace comes up with Playwright and GitHub MCPs attached, a "Backend" workspace with Postgres, with no manual setup per worktree. Toggling a server on a profile affects future spawns; existing workspaces keep the config they were born with.

### Verification

A profile can carry a `verify_cmd` (`pnpm vitest run`, `cargo test`, `playwright test`). Helmsmen runs it in the worktree — on demand, or automatically on `Stop` — and surfaces pass/fail as a badge on the workspace card. The Helm then reads as a real review dashboard: **done + ✓ verify** means the diff is worth your attention; **done** without the badge means look closer.

## 4. Workflows

### The daily loop

1. **Spawn.** Pick a project → New workspace → name the branch, pick a profile, pick a runtime (default long runs to Tmux) → type the task. Helmsmen creates the worktree off the project's base branch, injects hook config and the profile's MCP set, and launches the harness.
2. **Fan out.** Repeat for parallel tasks — across projects if you like. Stay on the **Helm** and let them run.
3. **Answer the inbox.** The Inbox badge is the only thing you must react to. Blocked cards glow; open the inbox, read the exact tool calls, Allow/Deny/Edit. Each decision un-blocks its agent instantly.
4. **Review.** When a card flips to done (ideally with ✓ verify), drill in (`enter`) → Diff tab (`d`). Terax's diff view and git graph are wired to that workspace's worktree. Nudge the agent in the same pane if the diff needs another pass.
5. **Ship.** Open PR (`gh pr create` from the worktree). Once merged, mark the workspace merged — Helmsmen removes the worktree, the branch, and the tmux session in one cleanup.

### Surviving a quit (Tmux runtime)

Start a long run in a Tmux workspace and quit Helmsmen entirely. The agent keeps running in its named tmux session (`helmsmen-<workspace-id>`). On relaunch, Helmsmen discovers `helmsmen-*` sessions and rehydrates the workspaces — reattaching with history, not respawning. With Claude Code, `--resume` additionally restores the *conversation* after a crash, on top of the process surviving. Honest caveat, surfaced in the UI: tmux sleeps with the machine. Quit-safe ≠ lid-safe; true walk-away is the Remote runtime (M8).

### Working with a second harness

Spawn a workspace with `harness: codex`. The launch command comes from the Codex harness adapter; capabilities it lacks simply don't render — no cost meter if it doesn't report usage, a "heuristic" status dot if it has no hooks. Everything else (worktree isolation, diff review, PR, cleanup, tmux persistence) is identical, because those subsystems never knew which harness was running.

### Multi-session workspaces

A workspace usually starts with one agent session, but the worktree is yours too. **＋ session** adds: a **Shell** (your own terminal in the same worktree — inspect, run git, poke the build without interrupting the agent), a **Process** (a pinned dev server whose port feeds the Preview tab), or a **Reviewer** (a second, read-only-by-policy agent that critiques the finished diff — the lightest useful orchestration, no graph required). Each session picks its own runtime; tmux names are `helmsmen-<workspace>-<session>`.

One deliberate friction: adding a second *writing* agent to the same worktree is allowed but labeled **pair mode** and requires confirmation — two agents editing one tree defeats the isolation that justifies worktrees. Parallelism between tasks belongs in separate workspaces; parallelism *within* a task is what Claude Code's subagents are for.

## 5. UI guide

The interface is designed around an **attention budget**: only two states may demand attention — *blocked* and *done-but-unreviewed*. Everything else is ambient. Each visual channel carries one meaning: saturated color = "needs you," profile color = a 2px edge tick, and the only motion is the working-dot pulse. (Borrowed deliberately: cmux's contextual notifications — show *why* an agent waits, not just that it waits — and Warp's Agent Management Panel as a centralized triage surface.)

**Navigation is a stack, not a mode toggle.** The **Helm** is home; pressing `enter` on a workspace **takes the wheel** (drills in); `esc` pops back, and the ⎈ Helm button (or `h`) returns home from anywhere. Every movement has both a vim key and an arrow key — `j/↓`, `k/↑`, `[/←`, `]/→` — so the muscle memory you have is the one that works. `⌘K` opens a command palette (Go to Helm, jump to any workspace, new workspace, sidebar, inbox), and a persistent **statusline** shows the exact keymap for the current context (helm / workspace / inbox), so the keyboard UX is discoverable rather than memorized.

**Reply-in-place** — any card expands into a message box with `m` (or the ✉ button): type, `enter` sends straight to that agent's PTY, `esc` collapses. Steering an agent no longer requires leaving the Helm — the most common intervention ("also add a test for X") becomes a five-keystroke drive-by.

**Helm** — workspaces grouped into fixed triage sections: **Needs you → To review → Working → Idle**. Geometry stays stable; priority stays obvious. Card content varies by state instead of showing a uniform feed: a blocked card's headline is the actual deferred tool call with Allow (`a`) / Deny (`x`) inline; a working card is one quiet last-action line + elapsed; a done card shows outcome — diffstat + verify badge; idle collapses to one-line rows. Session chips on each card show every live process. The header's **summary strip** (`N need you · N working · N to review · $cost`) carries the contract: if "need you" is zero, the screen can be ignored.

**Workspace (drill-in)** — session tabs across the top (`1`–`9` to switch: agent, your shell, dev server, reviewer — one worktree, N processes), the PTY below with input going straight to the selected session, and a right panel with **Diff / Preview / Verify** tabs (`d`/`p`/`v`) plus Open PR. Approvals remain answerable here with the same `a`/`x`.

**Sidebar** — collapsible with `s`: full mode shows projects → workspaces (status dot, branch, `×N` session count); collapsed mode is a thin rail of per-project rollup dots so cross-project status survives even at minimum width.

**Approval Inbox** (`i`) — a right-side drawer aggregating deferred calls across all projects: workspace, exact tool + arguments, the rule that fired, Allow / Deny / Edit. Fully keyboard-operable: `j/↓` `k/↑` select an item, `a` allow, `x` deny, and **bulk actions** — `X` denies everything immediately, while `A` (Allow all) requires a second press to confirm, because bulk-approving risky operations sight-unseen is exactly the failure mode the inbox exists to prevent. Helm cards offer the same per-item decision inline; the inbox is the exhaustive view.

**Keymap summary** — Helm: `j/↓` `k/↑` select, `enter` open, `m` message, `a` allow, `x` deny. Workspace: `esc` back, `[/←` `]/→` prev/next workspace, `1–9` session, `d/p/v` right panel. Inbox: `j/↓` `k/↑` select, `a`/`x` decide, `A`/`X` bulk (allow-all confirms). Anywhere: `⌘K` palette, `h` helm, `i` inbox, `s` sidebar.

### The Fleet (future, M8)

The endgame workflow — credit to Theo's cmux-over-SSH demo for proving it feels right — is one **Flagship** (your Mac) orchestrating tasks executing on several **Ships**: a Linux desktop chewing through a heavy build, a Windows laptop running platform-specific verification, a rented cloud box for overnight runs. Each Ship runs sessions in tmux over SSH; the Flagship keeps the registry, the Helm, the Approval Inbox, and the hook endpoint. Hook events travel back through an SSH reverse tunnel, spool on the Ship during disconnects, and replay on reconnect — so closing the MacBook loses nothing. A Ship going offline greys its sessions as unreachable; tmux keeps them alive until you reattach. On the Helm, fleet sessions read `claude·tmux@rig`; everything else — triage, approvals, diffs, reply-in-place — is identical, because Ships are just a Runtime, and every subsystem was written against the trait.

## 6. Feature reference

| Feature | Milestone | Notes |
|---|---|---|
| Projects, worktree isolation, **Session model** | M1 | Workspace 1→N sessions; rollup status; `..`-free path validation |
| Runtime trait: LocalPty | M1 | Reload-safe default |
| Harness trait: Claude Code adapter | M1 | Capability flags drive the UI |
| Triage Helm + drill-in navigation, palette, statusline, collapsible sidebar | M2 | Attention-budget design; per-state card content |
| Hook control plane & state detection | M3 | Deterministic; buffer heuristics only for BYOA |
| Approval Inbox (`defer`) + inline card decisions | M3.5 | Cross-project queue; Allow/Deny/Edit |
| Tmux runtime & rehydration | M4 | `helmsmen-<ws>-<session>` naming; survives quit/crash |
| Diff review, git graph, Open PR, cleanup | M5 | Reuses Terax; `gh` for PRs |
| `--resume`, notifications, token/cost, MCP sets, verify | M6 | Cost from hook payloads, not output parsing |
| BYOA adapters, Shell/Process/Reviewer sessions UI, preview, Playwright | M7 | Pair mode gated behind confirmation |
| The Fleet: Ships over SSH | M8 | Flagship orchestrates; tmux-on-Ship survives the Flagship sleeping; cloud runners later as rented Ships |

## 7. Security posture (summary)

All agent output is hostile — no escape sequence triggers privileged action (the Terax CVE lesson). The hook endpoint binds to loopback only and authenticates per-session tokens; hook payloads are data, never instructions. The approval policy is deny-by-default for the risk list, with `defer` routing judgment to you rather than auto-allowing. Pane content and tmux scrollback can contain secrets, so persistence of pane content is opt-in and off by default. Remote runs (M8) scope credentials to the runner and ship nothing beyond the task's needs.

## 8. Licensing & open-source references

Helmsmen is intended to be open source, licensed **Apache-2.0** — matching its Terax base, which keeps the fork compliant with a single license and carries Apache's patent grant. The obligations inherited from the fork: retain Terax's LICENSE and copyright notices, preserve any NOTICE file, and state significant changes.

Tools Helmsmen references are *idea sources only* — UI patterns, workflows, and concepts aren't copyrightable; source code, assets, and distinctive visual identity are. The boundary per tool: **cmux** is GPL-3.0-or-later (never copy source — GPL code entering an Apache-2.0 project forces the whole project copyleft); **Herdr** is AGPL (same rule, stricter); **Warp**'s open repo is mixed — its `warpui` crates are MIT, everything else is AGPL v3, so treat all non-warpui code as untouchable; **AgentsRoom** — reimplement ideas, never lift source.

Two enforcement mechanisms, because agents write most of this codebase: Helmsmen's own CLAUDE.md carries the rule *"Never fetch or reproduce code from cmux, Herdr, Warp (non-warpui), or AgentsRoom repositories; ideas only"*, and CI runs `cargo-deny` (Rust) plus a JS license checker so the dependency tree stays Apache-compatible. (Not legal advice — read Apache-2.0 §4 before first publish.)

## 9. What Helmsmen deliberately isn't (v1)

No multi-step orchestration graphs (dev→QA handoffs) — parallel execution only, though the hook control plane is exactly the seam that enables orchestration later as a pure-core state machine consuming events. No kanban/backlog, no cron, no mobile app, no accounts or telemetry, no Windows, no custom agent. One user, one machine, N agents.
