#!/bin/bash
# PROTOTYPE (spike-approval-loop) — the `answer_prompt` seam.
#
# design-notes.md → Test seams: ALL keystroke injection into a claude pane goes
# through this ONE function, because the permission-prompt layout is not a stable
# API. Whatever key sequences this spike proves out get encoded here and nowhere
# else; in Helmsmen this becomes the single integration-tested seam re-run per
# Claude Code release.
#
# Usage:
#   answer-prompt.sh <tmux-target> allow
#   answer-prompt.sh <tmux-target> deny [message...]
#   answer-prompt.sh <tmux-target> raw <tmux-send-keys args...>   # key discovery
set -euo pipefail

TARGET="${1:?usage: answer-prompt.sh <tmux-target> allow|deny|raw ...}"
ACTION="${2:?usage: answer-prompt.sh <tmux-target> allow|deny|raw ...}"
shift 2

case "$ACTION" in
  allow)
    # Guess (Claude Code 2.1.x): numbered dialog, "1. Yes" — the digit alone
    # selects and submits. If layout drifted, try: Enter (accept highlighted),
    # or Down/Up then Enter, via `raw`.
    tmux send-keys -t "$TARGET" "1"
    ;;
  deny)
    # Guess: Esc = "No, and tell Claude what to do differently" — rejects the
    # call and focuses the input box; then type the instruction and submit.
    tmux send-keys -t "$TARGET" Escape
    sleep 0.4
    if [ $# -gt 0 ]; then
      tmux send-keys -t "$TARGET" -l "$*"
      sleep 0.2
      tmux send-keys -t "$TARGET" Enter
    fi
    ;;
  raw)
    tmux send-keys -t "$TARGET" "$@"
    ;;
  *)
    echo "unknown action: $ACTION (want allow|deny|raw)" >&2
    exit 2
    ;;
esac
