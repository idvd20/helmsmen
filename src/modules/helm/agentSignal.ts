// Helmsmen — agent-signal ingestion for live status dots (task #11).
//
// The M2 interim status SOURCE: Terax's in-tree OSC agent-signal, parsed
// backend-side in `modules::pty::agent_detect` and emitted on the Tauri
// event `terax:agent-signal`. This module is the frontend mirror of the
// pure-core seam (`core::cut::SessionSignal` + `session_status_from_signal`):
// it maps a hostile signal `kind` to a per-Session status and folds a stream
// of signals into a live status map. HelmView overlays that map onto the
// Session facts and hands them to the existing view-model rollup
// (`rollUpStatus` in viewModel.ts), so the existing dot goes live — no card
// or rollup changes.
//
// Pure and DOM-free — every function here transforms data only, so it is
// unit-tested without a webview (this repo has no jsdom). The Tauri `listen`
// subscription that drives it lives in HelmView.
//
// === signal -> event -> status seam (the M3 swap point) ===
//   SOURCE  (M2): `terax:agent-signal`, best-effort, whole-terminal.
//   REDUCER: sessionStatusFromSignal + rollUpStatus (viewModel) — unchanged.
// At M3 the control plane's per-Workspace hooks replace the SOURCE, emitting
// the same signal kinds keyed per Session; this reducer stays put. The
// correlation key (which Session id a signal carries) is exactly what the M3
// source pins down per Workspace — see `applyLiveStatuses`.

import type { HelmWorkspaceStatus } from "./api";
import type { SessionFacts } from "./viewModel";

/** The signal kinds Terax's agent-signal emits (mirrors the backend
 * `pty::agent_detect::Transition` variants). */
export type HelmAgentSignalKind =
  | "started"
  | "working"
  | "attention"
  | "finished"
  | "exited";

/** The `terax:agent-signal` event payload. `kind` is typed loose on purpose:
 * it is signal content and therefore hostile — it is validated in
 * `sessionStatusFromSignal`, never trusted as-is. */
export interface HelmAgentSignal {
  id: number;
  kind: string;
  agent?: string | null;
}

/** Longest kind we classify; anything longer is hostile/garbage and yields
 * null. Mirrors the backend `harness::agent_signal::MAX_SIGNAL_KIND_LEN`. */
export const MAX_SIGNAL_KIND_LEN = 32;

/** Map an agent-signal `kind` to the status it implies for that one Session,
 * mirroring `core::cut::session_status_from_signal`. `exited` and every
 * unknown or oversized kind return null: the Session then contributes no
 * status to the rollup (so a dead process never pins a stale dot). */
export function sessionStatusFromSignal(
  kind: string,
): HelmWorkspaceStatus | null {
  if (kind.length > MAX_SIGNAL_KIND_LEN) return null;
  switch (kind) {
    case "started":
    case "working":
      return "working";
    case "attention":
      return "blocked";
    case "finished":
      return "done";
    default:
      // "exited" and every unknown kind: no status contribution.
      return null;
  }
}

/** Live per-Session status, keyed by the signal's session id (stringified).
 * Ephemeral — derived, never stored — mirroring the core's "status is never
 * persisted" rule for WorkspaceStatus. */
export type LiveSessionStatuses = Readonly<Record<string, HelmWorkspaceStatus>>;

/** Fold one agent-signal into the live status map (pure; returns the same
 * reference when nothing changed, so React state stays stable). A signal
 * that maps to no status ("exited" or any unknown kind) drops the Session
 * from the map so a stale dot never lingers; otherwise it sets the Session's
 * current status. */
export function reduceAgentSignal(
  prev: LiveSessionStatuses,
  signal: HelmAgentSignal,
): LiveSessionStatuses {
  const key = String(signal.id);
  const status = sessionStatusFromSignal(signal.kind);
  if (status === null) {
    if (!(key in prev)) return prev;
    const next: Record<string, HelmWorkspaceStatus> = {};
    for (const [k, v] of Object.entries(prev)) {
      if (k !== key) next[k] = v;
    }
    return next;
  }
  if (prev[key] === status) return prev;
  return { ...prev, [key]: status };
}

/** Overlay live agent-signal statuses onto a Workspace's Session facts,
 * setting each Session's `status` from the live map (keyed by `sessionId`).
 * A Session with no live signal keeps whatever status it already had, and a
 * status of the same value returns the same array reference.
 *
 * This is task #11's precise job: the Session *list* comes from the
 * zoom/attach slice (#12) as `WorkspaceFacts.sessions`; #11 fills in each
 * Session's *live status* so the existing `buildWall` rollup lights the dot.
 * The `sessionId <-> signal id` correlation is the M3 source-swap concern —
 * M3's per-Workspace hooks key the same live map by Session. */
export function applyLiveStatuses(
  sessions: readonly SessionFacts[],
  live: LiveSessionStatuses,
): SessionFacts[] {
  let changed = false;
  const next = sessions.map((s) => {
    const status = live[s.sessionId];
    if (status && status !== s.status) {
      changed = true;
      return { ...s, status };
    }
    return s;
  });
  return changed ? next : (sessions as SessionFacts[]);
}
