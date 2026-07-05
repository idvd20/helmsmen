'use strict';

// PROTOTYPE (spike-approval-loop) — but this module is the portable piece.
//
// Pure reducer answering criterion 4: given the raw stream of hook events the
// control plane receives, can we derive unambiguous Approval Inbox cards?
// This is the shape Helmsmen's pure core would lift (as Rust) if the spike passes.
//
// Card lifecycle:
//   pending       PreToolUse fired and we returned `ask`
//   surfaced      Notification(permission) arrived for the session → Blocked
//   allowed       PostToolUse observed for the same call → the Allow path completed
//   closed-no-run session hit Stop with the card unresolved → denied or dismissed
//
// Ambiguities are not hidden: any time matching had to guess, a warning is pushed.
// Zero warnings across a multi-call session is the criterion-4 pass signal.

const emptyState = () => ({ cards: [], warnings: [], eventCount: 0 });

const short = (sid) => (sid || '?').slice(0, 8);

// event: { seq, receivedAt, type: 'pretooluse'|'notification'|'posttooluse'|'stop', payload }
function applyEvent(prev, event) {
  const state = {
    cards: prev.cards.map((c) => ({ ...c })),
    warnings: prev.warnings.slice(),
    eventCount: prev.eventCount + 1,
  };
  const p = event.payload || {};
  const sid = p.session_id || 'unknown-session';

  switch (event.type) {
    case 'pretooluse': {
      state.cards.push({
        id: `card-${event.seq}`,
        seq: event.seq,
        receivedAt: event.receivedAt,
        sessionId: sid,
        toolName: p.tool_name || '?',
        toolInput: p.tool_input || {},
        toolUseId: p.tool_use_id || null, // criterion 4: is this even present?
        status: 'pending',
        notification: null,
        resolvedAt: null,
      });
      break;
    }

    case 'notification': {
      const open = state.cards.filter((c) => c.sessionId === sid && c.status === 'pending');
      if (open.length === 0) break; // idle notice etc. — not a permission prompt for us
      if (open.length > 1) {
        state.warnings.push(
          `seq ${event.seq}: ${open.length} pending cards in session ${short(sid)} when a ` +
            `notification arrived — matched the oldest; correlation ambiguous (criterion 4)`
        );
      }
      open[0].status = 'surfaced';
      open[0].notification = p.message || JSON.stringify(p);
      break;
    }

    case 'posttooluse': {
      const candidates = state.cards.filter(
        (c) =>
          c.sessionId === sid &&
          (c.status === 'surfaced' || c.status === 'pending') &&
          c.toolName === (p.tool_name || '?')
      );
      if (candidates.length === 0) break;
      let match = null;
      if (p.tool_use_id) {
        match = candidates.find((c) => c.toolUseId === p.tool_use_id) || null;
        if (!match)
          state.warnings.push(
            `seq ${event.seq}: PostToolUse tool_use_id ${p.tool_use_id} matched no card — ` +
              `fell back to oldest open ${p.tool_name} card in ${short(sid)}`
          );
      } else if (candidates.length > 1) {
        state.warnings.push(
          `seq ${event.seq}: no tool_use_id and ${candidates.length} open ${p.tool_name} ` +
            `cards in ${short(sid)} — matched the oldest (criterion 4)`
        );
      }
      match = match || candidates[0];
      match.status = 'allowed';
      match.resolvedAt = event.receivedAt;
      break;
    }

    case 'stop': {
      for (const c of state.cards) {
        if (c.sessionId === sid && (c.status === 'pending' || c.status === 'surfaced')) {
          c.status = 'closed-no-run';
          c.resolvedAt = event.receivedAt;
        }
      }
      break;
    }
  }
  return state;
}

module.exports = { emptyState, applyEvent };
