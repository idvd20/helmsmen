// Helmsmen — interim spawned-Session registry for the zoom (task #12).
//
// The wall's Session chips / facts are populated by later slices; until
// then the zoom needs some way to know a Workspace's live Agent Sessions
// (to build tabs and resolve a clicked chip). This is a small subscribable
// list of the `HelmAgentSession` handles the spawn command already returns
// — the dev-console demo registers each spawn here. Pure state + pub/sub;
// it never spawns or touches the OS.

import type { HelmAgentSession } from "@/modules/helm/api";

export interface SessionStore {
  /** Add (or replace, by session id) a spawned Session, keeping order. */
  register(session: HelmAgentSession): void;
  list(): HelmAgentSession[];
  /** Subscribe to changes; returns an unsubscribe handle. */
  subscribe(listener: () => void): () => void;
}

export function createSessionStore(): SessionStore {
  let sessions: HelmAgentSession[] = [];
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
    list: () => sessions,
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** The app-wide store the dev console feeds and the zoom container reads.
 * Interim (see docs/fork-posture.md) until Session facts land on the wall. */
export const sessionStore: SessionStore = createSessionStore();
