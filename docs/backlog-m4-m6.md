# Backlog — M4–M6 slices (held until the verdict week passes)

Drafted by `/to-issues` from `PRD.md` alongside the published M0–M3.5 issues (#2–#21).
Held deliberately: milestones M4–M6 proceed only if the M3.5 verdict week (#21)
passes — "miss → stop or rescope". On a pass, publish these as GitHub issues using
the same template as #2–#21 (Parent: #1; labels `ready-for-agent` / `ready-for-human`;
blockers first so real issue numbers can be referenced).

Slice numbers continue the approved 35-slice breakdown (1–20 published as #2–#21;
slice N → issue #N+1). "Blocked by" uses slice numbers for held slices and issue
numbers for published ones. Strict milestone gating: each milestone's first slices
block on the previous milestone's "Done when" carrier (◆).

## M4 — Tmux Runtime

### 21. Tmux pane rendering decision — HITL (`ready-for-human`)

Resolve the open question parked at M4: attach vs control mode vs pipe-pane for
rendering tmux panes in the app. Prototype as needed; record the decision as an ADR.
- Areas: docs/adr
- Blocked by: #21 (verdict pass)
- Stories: parked open question
- Accept: ADR committed with the chosen approach and rejected alternatives; the
  choice demonstrated against a live tmux session.

### 22. ◆ Tmux Runtime — survive quit, discovery reconciles registry, tier-honest UI — AFK

Tmux Runtime implementation (named sessions) passing the same Runtime conformance
suite as LocalPty, plus the survive-app-quit test: spawn, drop the parent, reattach,
assert history + liveness. Launch-time tmux discovery reconciles against the
registry — never replaces it. UI honest about persistence tiers (local pty dies with
the app; tmux "survives quit, not sleep"). Never persist pane content by default.
Carries the M4 "Done when": quit fully mid-run, relaunch, still running, reattaches
with history.
- Areas: backend/runtime, registry, frontend (tier copy)
- Blocked by: slice 21
- Stories: 52, 53, 54

## M5 — Review & land in-app

### 23. Diff / editor pane with file tree + resizable persisted split — AFK

Right pane in zoom: diff with file tree and M/A badges, editor; keys `d e`, `h l`
cycle tabs, `j k` walk the tree, `↵` toggles diff⇄editor. Terminal/pane split
resizable (drag, clamped, double-click reset), persisted per machine in the same
local settings store as theme — not web localStorage.
- Areas: frontend/workspaces
- Blocked by: slice 22 (M4 ◆)
- Stories: 39 (d/e half), 41

### 24. Preview + verify panes — AFK

Preview tab as a sandboxed iframe to localhost with no IPC (a dev-server page can
never touch the app); verify tab runs the Profile's check command in the worktree on
demand and shows output. Keys `p v`.
- Areas: frontend/workspaces
- Blocked by: slice 23
- Stories: 39 (p/v half), 40

### 25. ◆ Land, Scuttle, Open PR & History records — AFK

Done-card actions end-to-end: Land (commit/push/Open PR from the review view; worktree
cleaned, branch handled), Scuttle (worktree and branch deleted, nothing reaches the
repo). Either way the Workspace collapses into a History record owned by its Project:
brief, branch, outcome + PR link, transcript path, timestamps, cost (slot filled at
M6). Never resurrectable — a new Brief instead. Terax's git surface wired per
workspace root. Carries the M5 "Done when": finish → review diff → open PR → clean
up, all in-app.
- Areas: backend/core (retire transitions), git glue, frontend helm+workspaces, registry
- Blocked by: slice 23
- Stories: 47, 49, 50, 51

### 26. Re-instruct — AFK

On a Done card: send a note to the agent and drop the card back to Working — iteration
costs one message.
- Areas: frontend/helm, backend/runtime
- Blocked by: slice 22 (M4 ◆)
- Stories: 48

### 27. Command bar — AFK

Wall command bar: `/new`, `/allow`, `/deny`, `/focus <substring>`; bare text messages
the selected Workspace's agent. Whole loop drivable without the mouse.
- Areas: frontend/helm
- Blocked by: slice 22 (M4 ◆)
- Stories: 43, 65 (part)

### 28. Themes, settings, card polish — AFK

Themes (Phosphor / Ember / Slate / Paper); Working-dot pulse toggle (default Off);
default Runtime setting; Terax AI side-panel hidden behind a setting (fork posture —
never stripped); card polish per the Quarterdeck v3 spec. Saved on this machine, no
config files.
- Areas: frontend (themes/settings)
- Blocked by: slice 22 (M4 ◆)
- Stories: 64, 65, 66

## M6 — Claude Code first-class

### 29. Resume Cap — `--resume` on relaunch — AFK

Resume Claude Code Sessions with `--resume` where possible; a Harness without the
Resume Cap restarts fresh with the Brief. Done when: quit mid-run → relaunch →
conversation resumes.
- Areas: backend/harness
- Blocked by: slice 25 (M5 ◆); interacts with slice 22 (tmux reattach)
- Stories: 56

### 30. Desktop notifications — Blocked and Done only — AFK

Notifications obey the attention rule: only Blocked and Done-unreviewed ever notify.
- Areas: backend (notifications), frontend settings
- Blocked by: slice 25 (M5 ◆)
- Stories: 57

### 31. Token/cost meters from transcript JSONL — AFK

Per-Workspace token/cost sourced from the session transcript (verify the source
first — open question parked at M6), summed per Project and in the header strip;
History records get their cost field.
- Areas: backend/harness (transcript parse), frontend/helm (meta, $total)
- Blocked by: slice 25 (M5 ◆)
- Stories: 21 (cost half), 62

### 32. MCP-set composition at spawn — AFK

The Profile's MCP set composed into the worktree's MCP config at spawn; frozen at
spawn (Profile edits affect future cuts only).
- Areas: backend/harness (config injection seam from M1)
- Blocked by: slice 25 (M5 ◆)
- Stories: 61

### 33. Verify-on-Stop badges — AFK

Verify runs automatically on Stop (and stays on demand); Done cards badge
"✓ verify passed — ready to review" / "no verify — look closer". Gates nothing.
- Areas: backend/core (Stop→Verify trigger), frontend/helm (badges)
- Blocked by: slices 24, 25
- Stories: 45, 46

### 34. Reviewer Session + pair-mode gate — AFK

Reviewer Session kind: read-only-by-policy agent with its own Profile (Profile binds
per Session launch — mixed-model/mixed-harness review needs no new concept). A second
*writing* Agent Session in one worktree is gated behind an explicit pair-mode
confirmation.
- Areas: backend/core (Session kinds, pair gate), backend/hooks (read-only policy), frontend/workspaces
- Blocked by: #13 (add Sessions), #17 (policy machinery), slice 25 (M6 gate)
- Stories: 35 (Reviewer), 60, 63

### 35. `byoa` Harness floor + Cap-degradation demo — AFK

The `byoa` Harness: launch any CLI agent with user-edited launch command/env, no Caps
granted from settings. Demonstrate degradation end-to-end: no Hooks → agent-signal /
heuristics; no Keys → answer by drilling in; no Signal → alive/idle heuristics; no
Resume → fresh start with the Brief. A weaker CLI still rides the same wall.
- Areas: backend/harness
- Blocked by: #16 (signal fallback), slice 25 (M6 gate)
- Stories: 58, 59

## M2/M6 demo criteria already encoded in published issues

M0 ◆ #3 · M1 ◆ #6 · M2 ◆ #14 · M3 ◆ #16 · M3.5 ◆ #19 · verdict #21.
