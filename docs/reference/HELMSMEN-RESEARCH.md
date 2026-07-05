# Helmsmen — Research & References

> Companion to `HANDOFF.md` (build spec) and `DOCS.md` (behavior spec). This document catalogues every tool Helmsmen references: what we borrowed (concept vs. code vs. stack), each project's license, and the boundary we must not cross. **Rule of thumb throughout: ideas and UX patterns are free to reimplement; source code, assets, and distinctive visual identity are governed by license.** Verify every license at the upstream repo before first publish — licenses change (cmux's did).

## 1. What Helmsmen is built FROM (code in the tree)

**Terax** — Apache-2.0. The fork base: Tauri 2 + React 19 shell, xterm.js terminal rendering, file tree, diff view, git graph, CodeMirror editing. This is the only project whose *code* lives in our tree. Obligations: retain its LICENSE and copyright notices, preserve any NOTICE file, state significant changes (Apache-2.0 §4). Also inherited: the CVE lesson that terminal output is hostile data — no escape sequence may trigger privileged action — which became our invariant §8.

## 2. What Helmsmen is built ON (protocols & platform surfaces)

**Claude Code (Anthropic)** — the first-class harness, integrated through documented surfaces, not copied code: the hooks system (`PreToolUse`, `PostToolUse`, `Notification` with `permission_prompt`/`idle_prompt`, `Stop`, `SubagentStop`) as our state/control plane; the `defer` permission decision as the mechanism behind the Approval Inbox (pause-don't-kill, context preserved); `--resume` for crash recovery; `.claude/settings.json` + `.mcp.json` for per-worktree config injection; headless/stream-json as a future structured channel; subagents as the answer to intra-task parallelism (which is why pair mode is gated). These APIs evolve quickly — HANDOFF §0 rule 4 requires re-verifying against current docs at each milestone that touches them. Claude Code's native Agent Teams remains a research preview; Helmsmen's post-v1 orchestration plan deliberately stays on stable hook events instead.

**MCP (Model Context Protocol)** — consumed as configuration only (composing profile `mcp_set`s into `.mcp.json`); we implement no protocol code.

**Codex, opencode** — BYOA harness targets at M7 via the `Harness` trait's capability flags. Integration is launch-command + config; no code from either.

## 3. What Helmsmen borrowed IDEAS from (no code, ever)

**Supacode** — the starting point of this whole project: worktree-per-task and project/workspace management proved the core loop, and its limits ("not enough to live in") defined our goals. Borrowed: the Project → Workspace mental model. 

**AgentsRoom** — the roles concept, rescoped for a solo dev into Profiles (`{prompt, model, color, mcp_set, verify_cmd}`): a few personal profiles instead of a template marketplace.

**cmux** — GPL-3.0-or-later (relicensed from AGPL; commercial option exists). Borrowed ideas: *contextual* notifications — show **why** an agent is waiting (the actual question/tool call), not just that it waits — which became the blocked card's headline; pane glow / notification rings → our attention-budget saturation rule; sidebar tabs carrying branch/cwd/port metadata → our session chips; sessions-within-a-container shape → part of the Workspace 1→N Session model. Swift/AppKit on libghostty, macOS-only, so its code would be useless to us even if it were permissible — it is not.

**Warp** — mixed licensing: the `warpui_core`/`warpui` UI-framework crates are MIT; **everything else in the repo is AGPL v3 — treat as untouchable**. Borrowed ideas: the Agent Management Panel as a centralized cross-context triage surface (agents notify you; you work elsewhere trusting the panel) → our Helm + summary strip; unread/red-dot semantics and jump-to-conversation → our needs-you section and drill-in; panes-within-a-tab ≈ sessions-within-a-workspace.

**Conductor, Herdr** — landscape references that shaped scoping. Herdr is AGPL: ideas only, never source. Conductor informed what a Mac-native parallel-agent app feels like; no code relationship.

**Theo — cmux-over-SSH demo (YouTube, `9tGrhrVKCrE`)** — the concept proof for the Fleet (M8): one Mac as the orchestrating device, cmux SSH workspaces per remote machine (Linux box, Windows laptop), tasks executing remotely while status flows back to the local sidebar. Borrowed: the *workflow shape* — main device holds the conn, fleet does the work. cmux's implementation details worth knowing (from its docs, ideas only — GPL): a relay daemon on the remote host forwards agent notifications to the local sidebar; dropped connections reconnect with exponential backoff while remote sessions persist and reattach. Helmsmen's equivalents are deliberately boring standard tools, not cmux's code: tmux-over-SSH as the runtime, an `ssh -R` reverse tunnel as the hook relay, on-disk event spooling + idempotent replay for disconnects. Naming: remote machines are **Ships**; the main device is the **Flagship**.

**Vim / tmux (conventions)** — `j/k`, `[`/`]`, modal esc-back navigation, and the statusline are decades-old TUI conventions, not any project's IP. tmux itself is a runtime dependency (ISC license, invoked as a CLI — no linking concerns).

## 4. Stack (dependencies, licenses to keep green in CI)

| Layer | Choice | License | Notes |
|---|---|---|---|
| App shell | Tauri 2 | Apache-2.0/MIT | inherited from Terax |
| Frontend | React 19, TypeScript | MIT / Apache-2.0 | |
| Terminal | xterm.js | MIT | via Terax |
| PTY | portable-pty (WezTerm) | MIT | |
| Persistence runtime | tmux (CLI) | ISC | subprocess, not linked |
| Control plane server | axum or hyper (loopback) | MIT | |
| Git/PR | git worktree, `gh` CLI | GPL v2 / MIT | invoked as subprocesses — no linking, no license propagation |
| Remote seam (M8) | Cloudflare Managed Agents / Claude Code on the web | service | re-evaluate at M8 |
| Enforcement | cargo-deny + JS license checker in CI | — | Apache-2.0-compatible tree only |

## 5. The contamination vector, restated

Agents write most of this codebase. The realistic license failure is not a human deliberately copying GPL code — it's an agent helpfully fetching a snippet from cmux or Warp mid-task. Hence the absolute rule in the repo CLAUDE.md and HANDOFF §0: **never fetch or reproduce code from cmux, Herdr, Warp (non-warpui), or AgentsRoom; ideas only.** CI license checks are the backstop, not the fence.

## 6. Name

`Helmsmen` — the crew member who physically steers under the direction of command; plural because every workspace has one at its wheel, while you hold the conn on the Helm. Pre-publish sweep still required: crates.io / npm / GitHub / trademark skim, plus a deliberate check for confusion-distance from Helm (the Kubernetes package manager) — different domain, but adjacent enough in dev-tool space to verify.

*(Not legal advice. Verify every upstream license text at fork/publish time.)*
