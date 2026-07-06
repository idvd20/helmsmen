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
import { useCallback, useEffect, useMemo, useState } from "react";
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
  type HelmApproval,
  createHelmApi,
  type HelmProfile,
  type HelmProject,
  type HelmWorkspace,
} from "./api";
import { Helm } from "./Helm";
import {
  buildWall,
  deriveBulkApprovals,
  deriveBulkAnswerPlan,
  deriveCardAnswerItem,
  describeAnswerOutcome,
  type WallAnswerTarget,
  type WallView,
  type WorkspaceFacts,
} from "./viewModel";

const WORKSPACE_POLL_MS = 1500;
const CLOCK_TICK_MS = 30_000;

/** Cheap change check for a Workspace's approval cards, so an unchanged poll
 * doesn't churn `facts` (and re-derive the whole wall). Compares the fields the
 * wall reads: id, status, and decision. */
function approvalsEqual(
  a: HelmApproval[] | undefined,
  b: HelmApproval[],
): boolean {
  if (a === undefined) return false;
  if (a.length !== b.length) return false;
  return a.every(
    (card, i) =>
      card.id === b[i].id &&
      card.status === b[i].status &&
      card.decision === b[i].decision,
  );
}

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

  // Live approvals → facts (task #18): poll each Workspace's control-plane
  // snapshot so the running reducer's pending asks surface as ask cards on the
  // wall (an open `ask` forces Blocked + the ask block; answering clears it).
  // A null snapshot (no endpoint) or a transient failure leaves the last set.
  useEffect(() => {
    let live = true;
    const tick = async () => {
      try {
        const ws = await api.listWorkspaces();
        const snaps = await Promise.all(
          ws.map(async (w) => {
            try {
              const state = await api.approvalsSnapshot(w.id);
              return [w.id, state?.cards ?? []] as const;
            } catch {
              return [w.id, null] as const;
            }
          }),
        );
        if (!live) return;
        setFacts((prev) => {
          let changed = false;
          const next = { ...prev };
          for (const [id, cards] of snaps) {
            if (cards === null) continue; // keep the last snapshot
            const existing = next[id] ?? {};
            if (approvalsEqual(existing.approvals, cards)) continue;
            next[id] = { ...existing, approvals: cards };
            changed = true;
          }
          return changed ? next : prev;
        });
      } catch {
        // a transient listWorkspaces failure just skips this poll
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

  // The bulk-approvals banner (task #19): count + one-line preview derived from
  // the same pending queue the cards read. Rendered only when >1 is pending.
  const bulk = useMemo(() => deriveBulkApprovals(wall.cards), [wall.cards]);

  // Bulk Allow-all / Deny-all: reuse #18's verify-before-inject `answer_prompt`
  // per pending ask, iterating the whole queue. The bulk decision is logged
  // DISTINCTLY (a bulk-flagged approval record) before the injections, so the
  // audit trail captures the queue the user acted on even if a call resolves
  // mid-loop. Correlation stays strictly by tool_use_id, so each paused call
  // resumes exactly where it paused.
  const runBulk = useCallback(
    async (action: "allowAll" | "denyAll") => {
      const plan = deriveBulkAnswerPlan(liveFacts);
      if (plan.length === 0) return;
      const workspaceIds = [...new Set(plan.map((item) => item.workspaceId))];
      for (const workspaceId of workspaceIds) {
        try {
          await api.recordBulkDecision(workspaceId, action);
        } catch {
          // A transient log failure never blocks the decisions themselves.
        }
      }
      const answer = action === "allowAll" ? "allow" : "deny";
      for (const item of plan) {
        if (!item.agentSession) continue; // no agent Session to receive keys
        try {
          await api.answerPrompt({
            session: item.agentSession.sessionId,
            runtime: item.agentSession.runtime,
            toolUseId: item.toolUseId,
            expectedCommand: item.expectedCommand,
            action: answer,
          });
        } catch {
          // A single unreachable agent skips its card; the rest proceed.
        }
      }
    },
    [api, liveFacts],
  );

  const onBulkAllow = useCallback(() => void runBulk("allowAll"), [runBulk]);
  const onBulkDeny = useCallback(() => void runBulk("denyAll"), [runBulk]);

  // Per-card Allow/Deny from the wall (`a`/`x`, task #34): the wall resolved
  // WHICH card; resolve that ask to its live agent Session + correlation
  // anchor and answer through #18's verify-before-inject seam — the zoom's
  // answer path, from the wall. Every non-answer (ask already resolved, no
  // agent Session, a mismatch — the backend injected NOTHING because the
  // visible dialog was not this card's, or an unreachable agent) surfaces as
  // a note instead of a silent no-op. The approvals poll reconciles the card.
  const [answerNote, setAnswerNote] = useState<string | null>(null);
  const onAnswerCard = useCallback(
    (target: WallAnswerTarget, action: "allow" | "deny") => {
      void (async () => {
        setAnswerNote(null);
        const item = deriveCardAnswerItem(liveFacts, target);
        if (!item) {
          setAnswerNote("ask already resolved — nothing to answer");
          return;
        }
        if (!item.agentSession) {
          setAnswerNote("no agent session in this workspace to answer");
          return;
        }
        try {
          const outcome = await api.answerPrompt({
            session: item.agentSession.sessionId,
            runtime: item.agentSession.runtime,
            toolUseId: item.toolUseId,
            expectedCommand: item.expectedCommand,
            action,
          });
          setAnswerNote(describeAnswerOutcome(outcome));
        } catch {
          setAnswerNote("could not reach the agent");
        }
      })();
    },
    [api, liveFacts],
  );

  return (
    <Helm
      wall={wall}
      projects={projects}
      keyboardActive={keyboardActive}
      onZoomSession={onZoomSession}
      bulk={bulk}
      onBulkAllow={onBulkAllow}
      onBulkDeny={onBulkDeny}
      onAnswerCard={onAnswerCard}
      answerNote={answerNote}
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
