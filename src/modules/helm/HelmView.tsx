// Helmsmen — the Helm host surface (task #10).
//
// The thin imperative shell around the pure wall: it loads registry data
// through the typed `invoke` seam (never spawning processes, running git,
// or touching repo files itself), polls Workspaces while cuts run
// ambient, and feeds `buildWall` a `nowMs` clock so elapsed minutes tick.
// All wall logic (rollup, rank sort, counts) is the tested pure code in
// viewModel.ts; this file only gathers inputs and mounts.
//
// Mounting into the app shell is an upstream integration point kept
// deliberately minimal (see docs/fork-posture.md): `HelmView` is the
// surface #9 (New Workspace) and #12 (Zoom) build on, and
// `mountHelmOverlay` / the dev console give an interim way to open it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  mergeSessionFacts,
  sessionStore,
} from "@/modules/workspaces/sessionStore";
import {
  applyLiveStatuses,
  type HelmAgentSignal,
  type LiveSessionStatuses,
  reduceAgentSignal,
} from "./agentSignal";
import {
  createHelmApi,
  type HelmProfile,
  type HelmProject,
  type HelmWorkspace,
} from "./api";
import { Helm } from "./Helm";
import { buildWall, type WallView, type WorkspaceFacts } from "./viewModel";

const WORKSPACE_POLL_MS = 1500;
const CLOCK_TICK_MS = 30_000;

export interface HelmViewProps {
  /** Zoom to a Session. Defaults to a logging placeholder; #12 wires the
   * real zoom view through this prop. */
  onZoomSession?: (sessionId: string) => void;
  /** False while an overlay (Zoom / New Workspace) owns the keyboard, so
   * the wall's `f`/`g`/`r` keys yield. The container (#12/#9) drives it;
   * the standalone mount leaves it true. */
  keyboardActive?: boolean;
}

export function HelmView({ onZoomSession, keyboardActive = true }: HelmViewProps) {
  const api = useMemo(() => createHelmApi(invoke), []);
  const [projects, setProjects] = useState<HelmProject[]>([]);
  const [profiles, setProfiles] = useState<HelmProfile[]>([]);
  const [workspaces, setWorkspaces] = useState<HelmWorkspace[]>([]);
  const [facts, setFacts] = useState<Record<string, WorkspaceFacts>>({});
  const [liveStatuses, setLiveStatuses] = useState<LiveSessionStatuses>({});
  const [sessions, setSessions] = useState(() => sessionStore.list());
  const [nowMs, setNowMs] = useState(() => Date.now());

  // Projects and Profiles change rarely — load them once.
  useEffect(() => {
    let live = true;
    void api
      .listProjects()
      .then((p) => live && setProjects(p))
      .catch(() => {});
    void api
      .listProfiles()
      .then((p) => live && setProfiles(p))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [api]);

  // Cuts run ambient, so poll Workspaces. First-seen time stands in for a
  // cut start until the core entity carries one (documented limitation);
  // it lives in state, not a render-time mutation, so the clock is pure.
  useEffect(() => {
    let live = true;
    const tick = async () => {
      try {
        const ws = await api.listWorkspaces();
        if (!live) return;
        setWorkspaces(ws);
        setFacts((prev) => {
          const seen = Date.now();
          let changed = false;
          const next = { ...prev };
          for (const w of ws) {
            if (!next[w.id]) {
              next[w.id] = { startedAtMs: seen };
              changed = true;
            }
          }
          return changed ? next : prev;
        });
      } catch {
        // A transient invoke failure just skips this poll.
      }
    };
    void tick();
    const id = setInterval(tick, WORKSPACE_POLL_MS);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [api]);

  // Advance the elapsed-minutes clock without re-fetching.
  useEffect(() => {
    const id = setInterval(() => setNowMs(Date.now()), CLOCK_TICK_MS);
    return () => clearInterval(id);
  }, []);

  // The interim live-Session registry (spawns from the zoom's add-session
  // controls and the dev console). Folded into the Session facts below so a
  // Session added or killed shows up / disappears as a card chip at once.
  useEffect(
    () => sessionStore.subscribe(() => setSessions(sessionStore.list())),
    [],
  );

  // Fold the live Sessions onto the polled facts, keyed by Workspace, before
  // the status overlay — this is the wire that makes added Sessions appear as
  // the wall's Session chips.
  const factsWithSessions = useMemo(
    () => mergeSessionFacts(facts, sessions),
    [facts, sessions],
  );

  // Live status (task #11): ride Terax's OSC agent-signal. The signal is
  // data, never a command — the pure reducer folds it into a per-Session
  // status map, overlaid onto Session facts below. This is the M2 interim
  // SOURCE; M3's per-Workspace control-plane hooks replace it without
  // touching the reducer (see agentSignal.ts).
  useEffect(() => {
    if (typeof window === "undefined") return;
    let live = true;
    let unlisten: (() => void) | undefined;
    void listen<HelmAgentSignal>("terax:agent-signal", (event) => {
      if (live) setLiveStatuses((prev) => reduceAgentSignal(prev, event.payload));
    }).then((fn) => {
      if (live) unlisten = fn;
      else fn();
    });
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  // Overlay live Session statuses onto the Session facts the rollup reads.
  // The Session list itself is populated by the zoom/attach slice (#12); #11
  // only fills in each Session's live status, so the existing rollup lights
  // the dot with no card or rollup change.
  const liveFacts = useMemo<Record<string, WorkspaceFacts>>(() => {
    if (Object.keys(liveStatuses).length === 0) return factsWithSessions;
    let changed = false;
    const next: Record<string, WorkspaceFacts> = {};
    for (const [id, f] of Object.entries(factsWithSessions)) {
      if (f.sessions && f.sessions.length > 0) {
        const withStatus = applyLiveStatuses(f.sessions, liveStatuses);
        if (withStatus !== f.sessions) {
          next[id] = { ...f, sessions: withStatus };
          changed = true;
          continue;
        }
      }
      next[id] = f;
    }
    return changed ? next : factsWithSessions;
  }, [factsWithSessions, liveStatuses]);

  const wall = useMemo<WallView>(
    () => buildWall({ projects, workspaces, profiles, facts: liveFacts, nowMs }),
    [projects, workspaces, profiles, liveFacts, nowMs],
  );

  return (
    <Helm
      wall={wall}
      projects={projects}
      keyboardActive={keyboardActive}
      onZoomSession={onZoomSession}
    />
  );
}

// --- interim overlay mount (dev console entry) ---

let overlayRoot: Root | null = null;
let overlayHost: HTMLDivElement | null = null;

/** Mount the Helm as a full-window overlay. Returns an unmount handle.
 * Idempotent. */
export function mountHelmOverlay(
  onZoomSession?: (sessionId: string) => void,
): () => void {
  if (!overlayHost) {
    overlayHost = document.createElement("div");
    overlayHost.id = "helmsmen-helm-overlay";
    overlayHost.style.cssText =
      "position:fixed;inset:0;z-index:2147483000;background:#070a0f";
    document.body.append(overlayHost);
    overlayRoot = createRoot(overlayHost);
  }
  const zoom =
    onZoomSession ??
    ((sessionId: string) => {
      // Placeholder zoom target until #12 lands the zoom view.
      console.info("[helm] zoom → session", sessionId);
    });
  overlayRoot?.render(<HelmView onZoomSession={zoom} />);
  return unmountHelmOverlay;
}

export function unmountHelmOverlay(): void {
  overlayRoot?.unmount();
  overlayRoot = null;
  overlayHost?.remove();
  overlayHost = null;
}
