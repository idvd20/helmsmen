# Tab-amend investigation (report only) — task #20, M3.5

**Status:** report only. No product code, tests, or config changed by this task.
**Recommendation (summary):** **Do NOT adopt native `Tab to amend` for v1.** Keep it
as a dialog-presence marker only; revisit post-M6 behind the live canary if — and only
if — a live spike first captures its actual key sequence *and* its effect on
`tool_use_id` reconciliation. **The decision stays with the maintainer.**

---

## 0. Method and evidence base — read this first

The task allows basing this report on captured evidence rather than driving a fresh
live prompt, and requires me to say so plainly if I do not drive one. **I did not drive
a fresh live `claude` prompt.** Doing so from inside this sandbox risks spawning a
nested interactive agent, which the task explicitly forbids. This report is therefore
grounded in:

1. **The approval-loop spike** (main checkout, read-only): `spike-approval-loop/NOTES.md`
   (PASS verdict, Claude Code **v2.1.200** / tmux **3.7b**, verified 2026-07-04, 14
   events across three turns), `answer-prompt.sh` (the proven key sequences),
   `events.jsonl` (the real queued/parallel-dialog corpus), `correlate.js` (the reducer
   shape), `README.md`.
2. **The now-merged #18 seam** (this worktree): `src-tauri/src/modules/harness/answer.rs`
   (pure verify-before-inject planner), `src-tauri/src/modules/runtime/answer.rs`
   (imperative send-keys shell + the ignored live canary), `src-tauri/src/modules/harness/mod.rs`
   (the `Harness::answer_plan` seam), and `src-tauri/src/modules/core/control_plane.rs`
   (the `tool_use_id` reconciliation reducer).

**The single most important fact for this whole investigation:** the spike *observed the
`Tab to amend` affordance exists* but **never pressed Tab**. It is recorded as a surprise
to investigate later, not as a tested path:

> "**`Tab to amend` exists on hook-forced dialogs** — 'Edit input' may be achievable
> natively rather than degrading to Deny+instruct. Investigate at M3.5."
> — `spike-approval-loop/NOTES.md:61-63` (Surprises / design-notes deltas #2)

Everything below that concerns *what Tab does after you press it* is therefore
**unverified**. I flag every such claim as `[UNVERIFIED]`. This is exactly the gap a
future live spike would have to close before adoption.

---

## 1. Exact key sequences

### 1a. What IS proven (the surrounding dialog and the answers we ship)

Hook-forced permission dialogs (the ones a PreToolUse `ask` produces) render as a
**2-option** dialog, not the 3-option "don't ask again" layout
(`NOTES.md:22-24`). The footer, captured verbatim, is:

```
Esc to cancel · Tab to amend · ctrl+e to explain
```

The proven answer key sequences (from `answer-prompt.sh`, encoded into the #18 seam at
`src-tauri/src/modules/harness/answer.rs:40-51,200-213`):

| Action                | Key sequence                                        | Evidence |
|-----------------------|-----------------------------------------------------|----------|
| **Allow**             | `1` (the digit alone selects **and** submits `1. Yes`) | `NOTES.md:20`, `answer-prompt.sh:25`, `answer.rs:44,202` |
| **Deny, no reason**   | `Esc` (cancels the tool call; it verifiably never runs) | `answer.rs:203-205`, `NOTES.md:47-50` |
| **Deny + reason**     | `Esc` → wait 400 ms → type instruction → wait 200 ms → `Enter` | `NOTES.md:21`, `answer-prompt.sh:28-37`, `answer.rs:206-212` |

The `Esc`-then-type path lands the instruction as a new user message and the agent
complied verbatim in the spike (`last_assistant_message: "DENIED-ACK"`,
`events.jsonl:7`; `NOTES.md:21,47-50`).

### 1b. What Tab-amend's key sequence actually is — `[UNVERIFIED]`

The **only** captured evidence is the footer string `Tab to amend`. The literal trigger
key is therefore **`Tab`** (tmux: `send-keys -t <target> Tab`). Beyond that keystroke,
**nothing is captured**:

- `[UNVERIFIED]` What screen state Tab produces. By analogy to Claude Code's normal
  edit affordance it is presumably an **editable text field pre-filled with the tool
  input** (the command), but the spike never rendered it, so its exact layout — prompt
  text, cursor position, whether the command is pre-selected, multi-line handling — is
  unknown.
- `[UNVERIFIED]` The submit key for the amended value (`Enter`? `Ctrl+Enter`? a second
  `Tab` to a confirm button?).
- `[UNVERIFIED]` The escape/abort key from within edit mode, and whether it returns to
  the dialog or cancels the whole call.
- `[UNVERIFIED]` Whether an amended command re-enters the hook (fires a fresh
  PreToolUse), which governs everything in §3.

**Bottom line for criterion 1:** a *complete* Tab-amend key sequence cannot be stated
from the evidence — only its entry key (`Tab`) is known. Producing the full sequence is
itself a prerequisite live-spike task, not something this report can supply honestly.

---

## 2. Queued / parallel-dialog behavior

This is well-captured, and it is the hazard the #18 seam was built around.

**Observed (spike):** with two `ask`s queued, both PreToolUse events fire in parallel,
but the UI shows dialogs **one at a time, newest on top** (`NOTES.md:54-60`, surprise
#1). The real corpus is `events.jsonl:9-13`:

- `seq 9` PreToolUse `git status` — `tool_use_id: toolu_01Dvw7DqDGE3pjV6KsWPFNiU`
- `seq 10` PreToolUse `git diff --stat` — `tool_use_id: toolu_01GBXYZ17dmzAZ8pw66RhauK`
- `seq 11` **one** Notification(permission) for the pair — not one per dialog
- `seq 12` PostToolUse for `git diff --stat` (the **second** call) — completes **first**
- `seq 13` PostToolUse for `git status` (the **first** call) — completes second

Two lessons, both already baked into the merged code:

1. **Correlate by `tool_use_id`, never by command string** (`NOTES.md:40-46`). Both
   Pre/PostToolUse carry it and it round-trips exactly, even when calls complete out of
   order and even when a user-level hook rewrote the command in between (`git log` →
   `rtk git log`, `events.jsonl:1` vs `:3`). The Rust reducer enforces this strictly:
   PostToolUse with no `tool_use_id` is refused, not guessed
   (`control_plane.rs:361-372`); a match is by `tool_use_id` only
   (`control_plane.rs:373-377`).
2. **Notifications are NOT 1:1 with asks** (two queued → one notification). The reducer
   sidesteps the ambiguity by surfacing **every** pending card in the session on a
   permission notification rather than guessing one (`control_plane.rs:343-354`).

**How Tab-amend interacts with the queue — `[UNVERIFIED]`, and this is a real concern:**
the visible dialog is the **newest** call, which is not necessarily the card the user
clicked. If the operator Tab-amends "whatever is on top," they may be editing the wrong
call's command — the exact mis-targeting the verify-before-inject guard exists to
prevent for Allow/Deny (`answer.rs:13-21`). Worse, it is unknown whether amending the
top dialog:

- pops it and reveals the queued one underneath (queue preserved), or
- resubmits the edited command as a **new** tool call that jumps the queue, or
- collapses/abandons the queued dialog.

None of these was observed. Any adoption must first characterize queue behavior under
amend, because the queue is exactly where the seam's security guarantee lives.

---

## 3. Interaction with `answer_prompt`'s capture-pane verification

This is the crux, and where #18's implementation lets us reason precisely.

### 3a. How verify-before-inject works today

`answer_prompt` (`src-tauri/src/modules/runtime/answer.rs:45-71`) does exactly four
things, in order:

1. **Snapshot once** — `runtime.snapshot(session)` reads the current visible screen
   (the `capture-pane` analog) — `answer.rs:54`.
2. **Plan (pure)** — `harness.answer_plan(&snapshot, dialog, answer)`
   (`harness/answer.rs:180-214`) strips ANSI, confirms a dialog is present via
   `DIALOG_MARKERS`, and confirms the intended card's **exact command is visible**
   (case-sensitive, whitespace-collapsed). On any mismatch it returns `Mismatch` and
   **no** steps.
3. **Inject only on match** — a `Mismatch` writes nothing (`runtime/answer.rs:58-61`).
4. **Reconcile later** — the card flips to Allowed/ClosedNoRun via the control plane's
   PostToolUse/Stop correlation, not from here (`runtime/answer.rs:29-36`).

Note the `DIALOG_MARKERS` list already contains `"tab to amend"`
(`harness/answer.rs:58-65`) — but purely as one of several **presence** signals that
*a* dialog is on screen. It has nothing to do with driving amend; it just helps detect
the dialog. Adopting amend would not require touching that.

### 3b. "Does amend change what's on-screen mid-verify?" — yes, and it opens a window

The verification snapshot is taken **once** (`runtime/answer.rs:54`); the plan is built
from that frozen snapshot and then executed. Amend introduces a **new screen state** (an
editable field) that replaces the static dialog. Two failure windows follow:

- **Allow (single keystroke, no waits):** the window between snapshot and the `1` write
  is tiny but nonzero. If a Tab-amend fires in that window (a human at the pane, or a
  future in-app amend button racing the answer), the `1` lands **in an editable text
  field** instead of selecting `1. Yes`, silently corrupting the command being composed.
- **Deny + reason (multi-step with 400 ms + 200 ms waits):** far worse. The plan is
  `Esc → wait 400 → type reason → wait 200 → Enter` (`harness/answer.rs:206-212`).
  **The screen is never re-verified during those waits.** If the dialog mutates into
  edit-mode mid-sequence, the typed reason and `Enter` land in the wrong UI state.

The verify-before-inject guarantee is precisely scoped: it prevents answering the
**wrong card**; it does **not** re-check that the dialog is still in the same *mode* it
was in at snapshot time. Adding amend — an operation whose entire purpose is to *change
the screen state* — increases the value of a mode that this guard does not defend
against. (This is a property of adding amend, not a defect in #18: the current seam only
ever transitions the screen from "dialog present" to "dialog gone," so there is no
mid-sequence mode change to guard against today.)

### 3c. "Does it disturb the `tool_use_id` reconciliation?" — the strongest reason for caution — `[UNVERIFIED]`

The reducer resolves a card **only** by matching a PostToolUse `tool_use_id` to an open
card's `tool_use_id` (`control_plane.rs:357-400`). Everything hinges on one unanswered
question: **when you amend a command and submit, does Claude Code reuse the original
`tool_use_id` or mint a new one?** Neither was observed. Both possible answers are bad:

- **If it mints a NEW `tool_use_id`:** the original card (Pending/Surfaced, keyed to the
  *old* id) will never get a matching PostToolUse. It stays open until `Stop`, then flips
  to **`ClosedNoRun`** (`control_plane.rs:402-408`) — i.e. the inbox says "denied / never
  ran" **even though an (edited) command did run.** Meanwhile the amended command's
  PostToolUse matches no open card → either an **orphan warning**
  (`control_plane.rs:389-397`) or, if the amend re-enters the hook, a **brand-new card**
  appears mid-flight. The inbox misrepresents reality.
- **If it REUSES the `tool_use_id`:** reconciliation stays clean, but the card's stored
  `input.command` is now **stale** — the inbox shows the pre-amend command while a
  *different* command actually executed. The audit record lies about what ran.

Either way, amend threatens the exact correctness property M3 just shipped: a truthful
approval inbox. And which branch occurs is unknown. This alone is disqualifying for v1
without a live spike.

---

## 4. Per-release stability risk

**Claude Code's prompt layout is not a stable API** — this is a first-principles
assumption the entire seam is built on:

- The seam exists *because* the layout is unstable: "the permission-prompt layout is not
  a stable API, so ALL keystroke injection goes through a single function"
  (`harness/answer.rs:1-8`; `Harness::answer_plan` doc `harness/mod.rs:115-133`,
  "re-verified against a live agent per release").
- `DIALOG_MARKERS` is explicitly called "the FRAGILE, layout-coupled part of the seam …
  re-verified live per Claude Code release" (`harness/answer.rs:53-57`).
- There is a dedicated ignored live canary to catch drift, run per release
  (`runtime/answer.rs:260-269,307-309`; "run with `cargo test -- --ignored`").
- The spike already saw layout churn between releases: 2.1.200 renamed the "default"
  permission mode to "Manual" (`NOTES.md:69-70`, surprise #5), and even the passing
  probe hedged "Stable across the probe; **still re-test per release**"
  (`NOTES.md:24`).

Amend **multiplies** this fragile surface. Today the seam reasons about essentially one
post-key transition (dialog present → gone) and two proven key sequences. Adopting amend
adds:

1. a **third** key sequence (`Tab` … amend … submit), whose steps are `[UNVERIFIED]`;
2. an **entirely new screen state** (the editable field) that must be detected and
   verified on its own — its own markers, cursor semantics, and submit key, each an
   independent per-release breakage point;
3. a new correlation contract (§3c) whose stability across releases is also
   `[UNVERIFIED]`.

Each addition is a place a silent Claude Code layout change can break — and unlike a
broken Allow/Deny (which fails *safe*, into `Mismatch` → inject nothing,
`harness/answer.rs:56-57`), a broken amend that types into a shifted field could fail
*unsafe* (mutating a command in place). The per-release testing burden roughly doubles,
and the new path is the one least able to fail safe.

---

## 5. Recommendation — adopt / don't-adopt

**Recommendation: DON'T adopt native `Tab to amend` for v1 (M3.5). The maintainer
decides.**

Rationale, in priority order:

1. **Zero behavioral evidence.** The spike saw the affordance and never drove it
   (`NOTES.md:61-63`). Shipping a keystroke path we have never observed end-to-end
   violates the seam's own live-verification discipline.
2. **Unverified correctness impact on the inbox (§3c).** Amend can desynchronize the
   `tool_use_id` reconciliation — producing either false `ClosedNoRun` cards or stale
   command records. That directly undermines the truthful approval inbox M3 exists to
   provide.
3. **It enlarges the one surface we deliberately keep minimal (§4).** A new screen state
   + a third fragile key sequence + a new correlation contract, each a per-release
   breakage point, and the new path is the one that fails *unsafe*.
4. **The functional gain is small and already covered.** "Edit input" already degrades
   cleanly to **Deny + instruct** (`harness/answer.rs:32-34`), which is proven, tested,
   reconciles cleanly (the denied call verifiably never runs, then the instruction
   reroutes — `NOTES.md:47-50`, `events.jsonl:7`), and — unlike amend — fails safe. The
   marginal UX upside of editing-in-place does not justify concentrating new risk
   exactly on the guarantee #18 was built to protect.

**What to keep as-is:** leave `"tab to amend"` in `DIALOG_MARKERS`
(`harness/answer.rs:61`) — it is a useful *presence* signal and costs nothing.

**Conditions to revisit (post-M6, behind the canary):** adopt only after a dedicated
live spike, driven in a real terminal (never a nested sandbox agent), captures **all**
of the `[UNVERIFIED]` items above:

1. the full Tab-amend key sequence (enter, edit, submit, abort);
2. the resulting screen layout, added as its own verify-before-inject markers/state;
3. definitive `tool_use_id` behavior on an amended submit (reuse vs new), with the
   control-plane reconciliation adjusted to match; and
4. queue behavior when amending the top of a parallel-dialog stack.

Only once those are captured and encoded into the ignored per-release canary
(`runtime/answer.rs:260-269`) should native amend be reconsidered — and even then, the
adopt/don't-adopt call remains the maintainer's.

---

### Appendix — evidence index

| Claim | Source |
|-------|--------|
| PASS verdict; version/tmux; 14 events | `spike-approval-loop/NOTES.md:6-9` |
| 2-option dialog + footer copy | `NOTES.md:22-24` |
| Allow=`1`, Deny+reason=`Esc`/type/`Enter` | `NOTES.md:20-21`, `answer-prompt.sh:20-45` |
| Tab-amend affordance seen, not driven | `NOTES.md:61-63` |
| Parallel dialogs: newest on top | `NOTES.md:54-60` |
| Real parallel corpus (out-of-order Post) | `events.jsonl:9-13` |
| One notification for two queued asks | `NOTES.md:44-46`, `events.jsonl:11` |
| Correlate by `tool_use_id` only | `NOTES.md:40-46`, `control_plane.rs:357-400` |
| Verify-before-inject: snapshot→plan→inject | `runtime/answer.rs:45-71` |
| Pure planner + `DIALOG_MARKERS` + case-sensitive match | `harness/answer.rs:53-65,180-214` |
| Deny+reason multi-step waits (no re-verify) | `harness/answer.rs:206-212` |
| Layout is not a stable API; per-release canary | `harness/answer.rs:1-8,53-57`, `harness/mod.rs:115-133`, `runtime/answer.rs:260-269` |
| Layout churn precedent (mode rename) | `NOTES.md:69-70` |
| "Edit input" already degrades to Deny+instruct | `harness/answer.rs:32-34` |
