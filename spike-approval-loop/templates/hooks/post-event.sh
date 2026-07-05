#!/bin/bash
# PROTOTYPE (spike-approval-loop) — hook relay.
# Reads the hook payload from stdin, POSTs it to the spike server, and for
# PreToolUse returns permissionDecision=ask so the built-in prompt surfaces.
# Copied into workdir/.claude/hooks/ by spike.js — edit the template, relaunch [c].
EVENT="${1:?event name required}"
PORT="${HELMSMEN_SPIKE_PORT:-4519}"
PAYLOAD="$(cat)"

# Hook-side evidence trail — survives the server being down.
LOG_DIR="${CLAUDE_PROJECT_DIR:-.}/.claude/hooks"
[ -d "$LOG_DIR" ] && printf '%s\n' "$PAYLOAD" >> "$LOG_DIR/events.jsonl" || true

curl -s --max-time 2 -X POST "http://127.0.0.1:${PORT}/event/${EVENT}" \
  -H 'Content-Type: application/json' --data-binary "$PAYLOAD" >/dev/null 2>&1 || true

if [ "$EVENT" = "pretooluse" ]; then
  cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"Helmsmen spike: risk-list simulation — every Bash call routes to the inbox"}}
JSON
fi
exit 0
