# Intended Workflow — the Helmsman Loop

**Thesis: agents row, you steer.** Helmsmen reduces every running task to its
decision points and routes them to one surface. Any touch that is not a decision —
cutting branches, watching scrollback, remembering which tab held which task,
re-explaining context after a restart — is friction the app must absorb.

The reason this works without building an agent platform: **a Session is a real
interactive `claude`, in a real worktree, in real tmux.** Everything already built
keeps working unchanged inside it — personal skills (`/tdd`, `/diagnose`,
`/handoff`, …), RTK, MCP servers, `gh`, plain git. Helmsmen adds routing, derived
status, and one approval queue on top; it never wraps or replaces the tools
underneath. (Spike-verified: user-level hooks like RTK coexist with the
control-plane hooks; skills are just prompts, so they need zero integration.)

## The loop at a glance

```
        ┌───────────────────────────────────────────────────┐
        │  HELM — one wall of cards, rank-sorted:
        │  Needs you → To review → Working → Idle
        └───────────────────────────────────────────────────┘
 Brief ──▶ Workspace cards ──▶ interrupts route to you ──▶ Review ──▶ Land / re-instruct / scuttle
 (30s)     (ambient)           (ask block on the card)     (zoom)     (then next Brief)
```

Your day is four verbs: **glance** at Helm, **clear** the Inbox, **review** what's
Done, **brief** what's next. Everything between those touches is ambient.

## Before the loop — the once-per setup

### Who picks the provider: the selection chain

A worktree never knows its agent. The choice binds per Session launch — the
brief screen simply picks the first Agent Session's Profile:

```
Session (each launch; a Reviewer carries its own)
  └── Profile        ← Project-owned copy, seeded from built-in templates;
        │               each Project names a default
        ├── prompt snippet · model · MCP set · verify command · color
        └── Harness   ← code-backed, shipped: claude-code | byoa (codex… later)
              ├── launch command / env (the only user-editable fields)
              └── Caps: Hooks · Keys · Signal · Resume · McpBlock — code, not config
```

So "which provider" is a per-Session decision made by picking a Profile —
defaulted by the Project, overridden with one keystroke on the brief screen.
Exploring a new harness later = it ships in a release (or runs under the `byoa`
floor today); clone a Profile pointing at it, brief a Workspace under it.
Nothing else changes.

### Once per harness (rare) — harnesses ship in the app

A Cap is an implementation (tested answer-prompt grammar, that CLI's hook
template), never a settings checkbox. v1 ships `claude-code` (full Caps) and a
generic `byoa` floor: any CLI command — LocalPty/tmux, Signal status if it emits
the OSC, nothing else. Each Cap a harness lacks switches off a surface — it
never breaks the loop:

| Cap missing | What degrades                                                          |
|-------------|------------------------------------------------------------------------|
| `Hooks`     | No inbox cards or structured events; status falls back to `Signal`     |
| `Keys`      | Inbox card shows Blocked but Allow/Deny require drilling in            |
| `Signal`    | Status degrades to process-alive/idle heuristics only                  |
| `Resume`    | Interrupted Sessions restart fresh with the brief instead of resuming  |

The floor is already real: Terax's in-tree OSC `terax:agent-signal` covers
Claude/Codex/Gemini status today (design-notes → M2), so a hookless harness still
gets Working/Done/Idle on Helm — it just doesn't get the Inbox.

### Once per repo (~2 minutes) — Add Project

The Supacode steal: per-repo settings plus setup/run scripts, stored in
Helmsmen's registry (app-data) — **never read from a file in the repo**, same
posture as the approval policy (a repo must not configure its own trust).

1. **Add Project**: pick the local clone. Base branch detected; worktree home
   (default `~/.helmsmen/worktrees/<project>/`) and branch template
   (`helm/<slug>`) prefilled.
2. **Setup script**: runs in every fresh worktree before the agent launches —
   `pnpm install`, `direnv allow`, db seed, whatever the repo needs to row.
3. **Carry-over globs**: untracked files copied from the main checkout into each
   worktree — `.env*`, `.claude/settings.local.json`.
4. **Process definitions**: named long-lived commands (dev server, log tail) that
   become startable Process Sessions inside any Workspace (Supacode's run-kind
   scripts, distilled onto the existing Session noun). They interpolate
   `HELMSMEN_SLOT` for collision-free ports and db names.
5. **Profiles**: seeded as Project-owned copies from the built-in templates
   (Feature/Bugfix/Research/Spike/Reviewer); pick the Project's default.
   Copies are independent — repo-specific verify commands and prompt tweaks are
   the point. Done — first brief doubles as the smoke test.

### What Enter actually does — the cut pipeline

Behind touch 1's Enter, in order; any failure parks the Workspace in **Needs
you** with the step's log attached — never a silently broken worktree:

1. Optional fetch, then `git worktree add` off the base branch, branch template applied.
2. Worktree authorized as a workspace root (Terax gating).
3. Slot allocated (lowest free integer in the Project); the `HELMSMEN_*` env —
   slot, workspace, project, main checkout — is assembled, and every later step
   spawns with it.
4. Carry-over globs copied in.
5. Setup script runs: one multiline shell command, your shell, cwd = worktree.
6. Harness wiring written — for Claude Code, the per-worktree
   `.claude/settings.json` control-plane hooks (M3); for `Signal`-only
   harnesses, nothing (OSC is already global).
7. Agent Session launched: Harness launch command + model + MCP set + opening
   prompt with the brief.
8. Process definitions start on demand (or auto-start if marked).

## The five touches

### 1. Brief — start a Workspace (~30 seconds, one screen)

New Workspace (`n`): pick Project, type the brief ("this becomes the opening
prompt"), pick a Profile (defaulted; the Profile's Harness decides the
provider). Branch comes prefilled from the Project's template — editable, never
required. Enter runs the cut pipeline above and returns you to Helm. You do not
`cd`, arrange panes, or install anything.

The Profile is where existing skills get distilled instead of rebuilt: its prompt
snippet *is* a skill invocation. Seed Profiles (proposal — pure config, no engine
work):

| Profile  | Opening prompt        | Verify command | Typical exit        |
|----------|-----------------------|----------------|---------------------|
| Feature  | `/tdd <brief>`        | test suite     | Land (PR)           |
| Bugfix   | `/diagnose <brief>`   | test suite     | Land                |
| Research | `/research <brief>`   | —              | Read report, scuttle|
| Spike    | `/prototype <brief>`  | —              | Verdict, scuttle    |

Brief sources, in order of arrival: your head (v1), a GitHub issue via `gh`
(post-verdict — `/to-issues` output becomes a pick-list), a PRD milestone.

### 2. Glance — the Helm wall (seconds, many times a day)

Helm is one rank-sorted wall of Workspace cards across every Project:
**Needs you** (Blocked) → **To review** (Done-unreviewed) → **Working** → **Idle**,
with status filter tabs, flat/Project grouping (Ship grouping at M8), and a repo
picker. The header counts ("3 need you · 4 working · 2 to review") answer "does
anything need me?" without reading a card. Working is ambient by design — its
dots don't pulse by default; watching an agent work is the habit this app exists
to break. Only Blocked and Done-unreviewed may demand attention (CONTEXT.md rule).

This is `/triage` distilled into a permanent surface, fed by hook-derived status
instead of labels.

### 3. Decide — the Approval Inbox, inline (the core interaction)

The Approval Inbox is logical, not a drawer: each paused risky call renders as
an ask block on its own Workspace card — tool, rule, exact command (correlated
by `tool_use_id`, spike-proven) — plus the Needs-you count/filter, and bulk
**Allow all** (two-press confirm) / **Deny all** when several queue up. Answers:

- **Allow** (`a`) — send-keys `1`; the agent continues.
- **Deny** (`x`) — one click (`Esc` under the hood); the tool verifiably never
  runs and the agent reroutes. Steering is decoupled: type at the card or in
  the command bar and it lands straight in that agent's PTY.
- **Amend** (investigate at M3.5) — approve an edited command; `Tab to amend`
  exists on hook-forced dialogs, so this may be achievable natively.

The mental-load claim, concretely: you answer "may it force-push?" from the card,
without reconstructing what the task was or finding its tab. What asks is the
day-1 risk list (design-notes → Decisions); everything inside the worktree is
free — worktree isolation is what keeps this queue short.

### 4. Steer — take the wheel (only when needed)

Messaging never requires leaving the wall: bare text in the command bar (or `m`
on a card) goes straight to that agent's PTY ("⏺ noted — queued after current
step"). When you do take the wheel, Enter zooms into the Workspace's Sessions —
agent PTY, your Shell, Processes; `1–9` to switch, `[` `]` to hop between
Workspaces without surfacing — with diff / preview / editor / verify panes
alongside. It is a real terminal: type to `claude` directly, run `/handoff` if
the context is long — then `esc` back to the wall.

Design intent: zooming is the escape hatch, not the loop. If the verdict week
shows constant zoom-ins, the cards are failing at context-carrying.

### 5. Review & land — Done → gone

Done-unreviewed Workspaces surface near the top of the wall, badged with their
verify result ("✓ verify passed — ready to review" / "no verify — look closer";
verify runs on demand or automatically on Stop). Zoom in: diff with file tree,
sandboxed preview, editor, verify panes, Open PR. You review the way
design-notes prescribes: architecture, seams, behavior — not line-by-line;
clippy/tests already gated.

Three exits, all one action:
- **Re-instruct** — back to Working with your note.
- **Land** — commit/push/PR (Terax git surface + `gh`); the Workspace retires
  to a history record, worktree cleaned.
- **Scuttle** — worktree and branch deleted; the main repo never knew (a
  history record keeps the brief + transcript path).

Until M5's diff/PR wiring, this touch happens in the drill-in Shell with plain
git/`gh` — the loop is intact, just not chromed.

## A day across two Projects (the loop, concretely)

Morning glance: Helm shows `rentvine` and `helmsmen` interleaved — two Workspaces
Done overnight, one Blocked. Clear the Blocked card first (it's a `db:migrate`
against the dev database — Allow). Open the first Done: diff is right, verify
passed — Land; PR opens via `gh`, worktree cleaned. Second Done went sideways —
Re-instruct with two lines, it drops back to Working. Brief three new Workspaces:
two on `rentvine` (Bugfix profile → `/diagnose`), one on `helmsmen` (Feature →
`/tdd`); each cut takes the 30 seconds of typing the brief — setup scripts and
env copies happen behind the spinner. Through the day, inbox cards from all five
Workspaces arrive in one queue; you answer from the card, cross-project, without
finding tabs. Once, you take the wheel on a `rentvine` Workspace to eyeball its
dev server — a Process Session started from the Project's run script — then back
out. Evening: everything still Working keeps rowing under tmux; close the lid.

## Between sessions — nothing held in the head

- **Tmux Runtime (M4):** quit Helmsmen, sessions keep rowing; relaunch reconciles
  the registry against live tmux, never replaces it. Overnight runs are normal.
- The Workspace *is* the handoff artifact — transcript, worktree, branch, status
  all persist. `/handoff` stays useful *inside* a long Agent Session; Helmsmen
  needs no feature for cross-day continuity beyond the registry.
- **Recap (post-verdict, M6 pairing):** a daily-recap view — what landed, what
  stalled, what it cost (cost source: transcript JSONL, per the spike breadcrumb).

## What stays outside the app

The design pipeline — `/caveman` → `/grill-me` → `/prototype` → `/to-prd` →
`/to-issues` — runs in a plain deep `claude` session per project (as this repo is
doing via `.pipeline/`). That work is one long conversation, not parallel tasks;
there is no routing problem for Helmsmen to solve there. Helmsmen picks up at the
backlog boundary: issues in, Workspaces out. `/afk` is the pattern Helmsmen
supersedes *within* the app (fresh-context agent per worktree, but with steering);
it remains the right tool on machines without Helmsmen.

## Distillation map

| Already built                  | What Helmsmen takes                                          |
|--------------------------------|--------------------------------------------------------------|
| `/afk`                         | The whole Workspace model: fresh context, one worktree per task — plus live steering |
| `/triage`                      | Helm's fixed attention sections                              |
| `/handoff`                     | Persistence posture: the Workspace is the handoff            |
| `/tdd` `/diagnose` `/research` `/prototype` | Seed Profiles (prompt snippets, zero integration) |
| `/daily-recap`                 | Recap view, post-verdict                                     |
| Claude Code hooks              | Status derivation + inbox cards (spike-proven)               |
| tmux                           | Runtime survival + the `answer_prompt` seam                  |
| git worktree                   | Isolation that keeps the risk list small and in-tree ops free|
| Terax fork                     | Terminal rendering, git surface, fs/proc/pty plumbing        |
| `gh`                           | Land step + brief source                                     |
| Supacode                       | Per-repo settings, setup/run scripts, worktree lifecycle → Project settings + the cut pipeline + Process Sessions |
| Quarterdeck v3 design          | Authoritative UI: the wall, zoom panes, inline approvals, command bar (docs/design/quarterdeck-v3-spec.md) |
| RTK / MCP / user hooks         | Untouched — they run inside Sessions as they do today        |

## Friction budget (what "polished" means, testably)

1. A brief costs *you* under 60 seconds and one screen — Enter returns you to
   Helm while the cut (cold installs included) runs ambient. Zero terminal
   commands. The budget measures your touch, never the machine's.
2. A glance answers "does anything need me?" in under 5 seconds, from counts.
3. An inbox card is answerable without drilling in — target ≥ 80% of cards; every
   drill-in-to-answer is logged as a card-fidelity failure to fix.
4. Only Blocked and Done-unreviewed ever notify, badge, or reorder.
5. App quit/crash loses zero task state (M4).
6. Every layer has a raw escape hatch (tmux attach, plain git in the worktree) —
   trust requires that the app is a view over real things, not a cage.

## Milestone availability of the loop

| Milestone | Loop available                                                        |
|-----------|-----------------------------------------------------------------------|
| M2        | Brief + Glance (agent-signal status dots) + Steer — daily-usable skeleton |
| M3        | Structured events (control plane) behind the same Glance              |
| M3.5      | Decide (inline ask blocks + bulk) — **full loop; verdict week runs here** |
| M4        | Between-sessions continuity (tmux)                                    |
| M5        | Review & land in-app (diff/preview/editor/verify, Open PR), command bar, themes, card polish |
| M6        | Recap + cost                                                          |

## Resolved by grilling (2026-07-05)

All of this doc's opens are closed — decisions with rationale live in
design-notes.md; new canonical terms (Brief, Cut, Land, Scuttle, Carry-over,
Cap, Slot, History record) live in CONTEXT.md. In short: Profile binds per
Session launch with exactly one code-backed Harness; Profiles are Project-owned
copies of built-in templates; Slot injection handles parallel collisions; setup
cost is accepted (the budget measures your touch); no repo-committed settings
file in v1; retirement collapses to a history record — no archive.

## Design round (2026-07-05)

Quarterdeck v3 is the authoritative UI — distilled in
docs/design/quarterdeck-v3-spec.md, decisions in design-notes.md. Headline
absorptions: the Approval Inbox dissolved into inline ask blocks on the wall;
deny decoupled from instruct (message-to-PTY steers instead); verify badges +
on-Stop runs; per-card token/cost. Canon vocabulary wins over design copy
(Working not "Running", Brief not "Task", Ship not "Machine", ask not "defer";
Working-dot pulse defaults Off). Machines/Terminals/Crew stay future-marked
(M8 / post-verdict / post-v1).

## The verdict, restated in workflow terms

One work week, ≥ 3 parallel Workspaces a day, all four verbs happening in
Helmsmen, zero fallbacks to raw terminal tabs for triage. If the loop above isn't
clearly lighter than tmux + tabs, stop (design-notes → kill criterion).
