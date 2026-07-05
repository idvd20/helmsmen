# PROTOTYPE — approval-loop spike (throwaway)

**Run:** `node spike-approval-loop/spike.js` (from repo root, in a real terminal)

## The question

Can Helmsmen's v1 approval mechanism — PreToolUse hook returns `ask` + inbox answers
via `tmux send-keys` — actually close the loop, end to end, against a live interactive
`claude`? (design-notes.md → Decisions → Approvals; this spike gates M0.)

## Pass/fail criteria (from design-notes.md)

1. **`ask` surfaces the prompt** — hook returns `permissionDecision: ask` for a Bash
   call and the built-in permission dialog reliably appears in the pane.
2. **send-keys answers it** — `answer-prompt.sh` (the seam) can Allow and the tool
   runs (PostToolUse observed). Which keys work, and does the layout look stable?
3. **Notification(permission_prompt) payload** — what exactly is in it? (Press `[v]`
   in the TUI, or read `events.jsonl`.) This is the Blocked-status signal.
4. **PreToolUse-POST ↔ prompt correlation is unambiguous** — enough to render and
   route an inbox card. Evidence: does the payload carry `tool_use_id`? Does
   `correlate.js` produce warnings when multiple calls are in flight?
5. **Deny-with-reason works** — reject the prompt and inject an instruction message;
   claude course-corrects.

## How it's wired

- `spike.js` — one process: dummy control-plane HTTP server (`:4519`, override with
  `HELMSMEN_SPIKE_PORT`) + a keyboard-driven TUI that shows events, derived inbox
  cards, and a live snapshot of the claude pane.
- `templates/` — `.claude/settings.json` + hook script. `[c]` copies them into
  `workdir/`, git-inits it (so claude's project root is unambiguously the sandbox),
  and launches `claude` in tmux session `helmsmen-spike`.
- The hook POSTs every PreToolUse / PostToolUse / Notification / Stop payload to the
  server and returns `ask` for **every Bash call** (risk-list simulation). Payloads
  are also appended to `events.jsonl` (server-side) and
  `workdir/.claude/hooks/events.jsonl` (hook-side, survives server downtime).
- `answer-prompt.sh` — **the one seam**. All keystroke injection goes through it;
  discovering the right keys is criterion 2, so it also has a `raw` mode for
  experiments. Current guesses: Allow = `1`; Deny+reason = `Esc`, type, `Enter`.

## Driving it

Typical run: `[c]` launch → answer claude's first-run trust prompt if it appears
(`[k]` raw `Enter`, or `tmux attach -t helmsmen-spike` in another tab) → `[p]` send
the canned test prompt (asks claude to run one harmless git command) → watch the
card go pending → surfaced → `[a]` allow or `[d]` deny with a reason.

To test criterion 4 under load, `[p]` a prompt like "run git status, git log, and
git diff, each as a separate bash call" and watch for correlation warnings.

Caveat: user-level `~/.claude/settings.json` hooks (e.g. the RTK rewrite hook) merge
with the sandbox hooks — `events.jsonl` shows what actually fired; that's realistic
for Helmsmen anyway.

## When done

Fill in the verdict in [NOTES.md](NOTES.md), then delete this directory. The only
things worth lifting are the verdict, `correlate.js` (inbox-card reducer shape), and
whatever key sequences `answer-prompt.sh` ends up encoding.
