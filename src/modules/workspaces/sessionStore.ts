// Helmsmen — interim spawned-Session registry for the zoom (task #12/#13).
//
// The wall's Session chips and the zoom's tabs both read a Workspace's live
// Sessions from here: a small subscribable list of the `HelmSession` handles
// the spawn commands return (Agent, Shell, Process). The dev console and the
// zoom's add-session controls register each spawn; killing a Session
// unregisters it. Pure state + pub/sub — it never spawns or touches the OS,
// and it holds only the opaque handle plus that Session's chip facts.
//
// Interim (see docs/fork-posture.md): the durable per-Workspace Session
// registry lands with the control plane; until then this bridges spawns to
// both surfaces. `sessionFactsByWorkspace` / `mergeSessionFacts` are the pure
// projection the Helm wall folds into its Session facts.

import type { HelmSession } from "@/modules/helm/api";
import type { SessionFacts, WorkspaceFacts } from "@/modules/helm/viewModel";
import { harnessToken } from "./zoomModel";

export interface SessionStore {
  /** Add (or replace, by session id) a spawned Session, keeping order. */
  register(session: HelmSession): void;
  /** Drop a Session by id (e.g. after killing it); no-op if absent. */
  unregister(sessionId: string): void;
  list(): HelmSession[];
  /** Subscribe to changes; returns an unsubscribe handle. */
  subscribe(listener: () => void): () => void;
}

export function createSessionStore(): SessionStore {
  let sessions: HelmSession[] = [];
  const listeners = new Set<() => void>();

  const emit = () => {
    for (const listener of listeners) listener();
  };

  return {
    register(session) {
      const at = sessions.findIndex((s) => s.sessionId === session.sessionId);
      if (at >= 0) {
        sessions = sessions.map((s) => (s === sessions[at] ? session : s));
      } else {
        sessions = [...sessions, session];
      }
      emit();
    },
    unregister(sessionId) {
      const next = sessions.filter((s) => s.sessionId !== sessionId);
      if (next.length === sessions.length) return;
      sessions = next;
      emit();
    },
    list: () => sessions,
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** Project one spawned Session onto the wall's `SessionFacts` (the chip
 * shape). Live per-Session status is layered on separately by the
 * agent-signal overlay, so it is left unset here. */
export function toSessionFacts(session: HelmSession): SessionFacts {
  return {
    sessionId: session.sessionId,
    kind: session.kind,
    runtime: session.runtime,
    harness:
      session.kind === "agent" && session.harnessId
        ? harnessToken(session.harnessId)
        : undefined,
    processName: session.processName,
    port: session.port,
  };
}

/** Group live Sessions into per-Workspace `SessionFacts`, preserving
 * registration (spawn) order — the wall's chip order. */
export function sessionFactsByWorkspace(
  sessions: HelmSession[],
): Record<string, SessionFacts[]> {
  const grouped: Record<string, SessionFacts[]> = {};
  for (const session of sessions) {
    if (!grouped[session.workspaceId]) grouped[session.workspaceId] = [];
    grouped[session.workspaceId].push(toSessionFacts(session));
  }
  return grouped;
}

/** Fold the live Sessions into a Workspace-facts map for the wall: each
 * Workspace's `sessions` becomes its live Session list so the existing chip
 * rendering and status rollup pick them up. Returns the input unchanged when
 * there are no Sessions (referential stability for the wall's memo). */
export function mergeSessionFacts(
  facts: Record<string, WorkspaceFacts>,
  sessions: HelmSession[],
): Record<string, WorkspaceFacts> {
  if (sessions.length === 0) return facts;
  const grouped = sessionFactsByWorkspace(sessions);
  const ids = new Set([...Object.keys(facts), ...Object.keys(grouped)]);
  const next: Record<string, WorkspaceFacts> = {};
  for (const id of ids) {
    const base = facts[id] ?? {};
    const s = grouped[id];
    next[id] = s ? { ...base, sessions: s } : base;
  }
  return next;
}

/** The app-wide store the dev console and the zoom's add-session controls
 * feed, and the wall + zoom container read. Interim (see docs/fork-posture.md)
 * until the durable Session registry lands. */
export const sessionStore: SessionStore = createSessionStore();
