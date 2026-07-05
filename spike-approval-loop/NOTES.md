# Spike verdict — approval loop (ask + send-keys)

**Question:** does PreToolUse `ask` + tmux send-keys close the approval loop against
a live interactive `claude`? (Gates M0 — see design-notes.md → Next step.)

**Verdict: PASS** — all five criteria verified 2026-07-04 in an automated probe
against Claude Code v2.1.200 / tmux 3.7b (raw evidence: `events.jsonl`, 14 events
across three turns). Re-drive it yourself with `node spike-approval-loop/spike.js`
if you want the feel before deleting this directory.

## Criteria

- [x] **1. `ask` surfaces the prompt** — every hook-flagged Bash call produced the
  dialog, within ~1–3s of the PreToolUse POST, across three turns including a
  two-calls-in-one-turn case. The hook's `permissionDecisionReason` is rendered in
  the dialog ("Hook PreToolUse:Bash requires confirmation…").
- [x] **2. send-keys answers it.** Working key sequences (now encoded in
  `answer-prompt.sh`):
  - Allow: `1` (digit alone selects and submits)
  - Deny+reason: `Esc` (cancels the tool call), then type the instruction, `Enter`
    — it lands as a new user message and claude complied verbatim (`DENIED-ACK`).
  - Layout: hook-forced asks get a **2-option** dialog (`1. Yes / 2. No`, footer
    `Esc to cancel · Tab to amend · ctrl+e to explain`) — not the 3-option
    "don't ask again" layout. Stable across the probe; still re-test per release.
- [x] **3. Notification(permission_prompt) payload** (full, verbatim):
  ```json
  {
   "session_id": "bb6de6a5-789a-4bcf-97cf-2eca27d74234",
   "transcript_path": "~/.claude/projects/…/bb6de6a5….jsonl",
   "cwd": ".../spike-approval-loop/workdir",
   "prompt_id": "fff7249e-130c-44c2-982e-79bc23f5b2c0",
   "hook_event_name": "Notification",
   "message": "Claude needs your permission",
   "notification_type": "permission_prompt"
  }
  ```
  No tool name, no tool_use_id — it is a Blocked-status signal only, never a card
  source. Also observed: `notification_type` for idle is `"Claude is waiting for
  your input"` message — usable for Done/Idle status.
- [x] **4. Correlation unambiguous — via `tool_use_id`.** PreToolUse *and*
  PostToolUse both carry `tool_use_id`; it round-tripped exactly, even when
  (a) the RTK user-hook rewrote the command between ask and execution, and
  (b) two parallel calls completed out of order. **Correlate by tool_use_id,
  never by command string.** Notifications are NOT 1:1 with asks (two queued
  dialogs → one notification) — correlate.js correctly flagged that as its one
  warning of the probe.
- [x] **5. Deny-with-reason** — `Esc` canceled the tool (event log shows no
  PostToolUse for the denied call; the transcript's "Ran 1 shell command" line is
  a rendering artifact of the interrupted turn), the typed instruction reached
  claude as a user message, and it course-corrected.

## Surprises / design-notes deltas (recorded in design-notes.md → Spike results)

1. **Visible-dialog targeting**: with two asks queued, both PreToolUse events fire
   in parallel but the UI shows dialogs one at a time — and the *second* call's
   dialog was on top. Blind send-keys answers whatever is visible, which may not be
   the card the user clicked. Helmsmen's real `answer_prompt` seam must
   capture-pane and verify the visible dialog matches the intended card before
   injecting (post-hoc reconciliation by tool_use_id catches, but doesn't prevent,
   a mis-answer).
2. **`Tab to amend` exists on hook-forced dialogs** — "Edit input" may be
   achievable natively rather than degrading to Deny+instruct. Investigate at M3.5.
3. **User-level hooks rewrite after our hook sees the input** (RTK: `git log` →
   `rtk git log`): inbox cards may show pre-rewrite commands. tool_use_id makes
   correlation immune; card fidelity caveat stands.
4. Notification payload carries `transcript_path` — breadcrumb for the M6
   cost-source question (transcript JSONL, as suspected).
5. Claude Code 2.1.200 renamed "default" permission mode to "Manual"; probe ran
   under it. Trust prompt did not appear (parent dir already trusted) — untrusted
   Workspace roots will still hit it; keep the raw-keys escape hatch.

## Next

PASS → revise HANDOFF to v7 (approval mechanism, M0 fork posture, reshaped
milestones, risk list) and start M0. Pipeline: `/to-prd` reads this file +
design-notes.md. Worth lifting before deletion: this file, `correlate.js` (reducer
shape + warning discipline), and the key sequences in `answer-prompt.sh`.
