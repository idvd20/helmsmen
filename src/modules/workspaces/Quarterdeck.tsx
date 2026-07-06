// Helmsmen — the quarterdeck container (task #12): the Helm wall plus the
// Zoom overlay it opens onto.
//
// This is the container-level wiring the Helm left open at #10: it renders
// the existing `HelmView` and supplies the real `onZoomSession` (chips zoom
// here now, not to a log placeholder), keeps "which Workspace/Session am I
// zoomed into" state, and services the cross-Workspace `[`/`]` hop and the
// `↵`-from-the-wall entry. Zoom navigation math all lives in the pure
// zoomModel/keymap modules; this shell only holds state and mounts.
//
// It touches no helm render code (the status dot, card body, and Session
// facts stay #10/#11's) and spawns/gits/touches nothing — Session I/O flows
// through the injected HelmApi over the invoke seam.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  type ChannelFactory,
  createHelmApi,
  type HelmApi,
} from "@/modules/helm/api";
import { HelmView } from "@/modules/helm/HelmView";
import { sessionStore } from "./sessionStore";
import { Zoom } from "./Zoom";
import {
  firstZoomTarget,
  groupSessions,
  hopZoomTarget,
  resolveZoomTarget,
  type ZoomTarget,
  type ZoomWorkspaceRef,
} from "./zoomModel";

const WORKSPACE_POLL_MS = 2000;

export interface QuarterdeckProps {
  api: HelmApi;
}

function isEditable(el: Element | null): boolean {
  if (!el) return false;
  const tag = el.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    (el as HTMLElement).isContentEditable
  );
}

export function Quarterdeck({ api }: QuarterdeckProps) {
  const [sessions, setSessions] = useState(() => sessionStore.list());
  const [workspaces, setWorkspaces] = useState<ZoomWorkspaceRef[]>([]);
  const [zoom, setZoom] = useState<ZoomTarget | null>(null);

  // Live spawned-Session registry (interim, see sessionStore.ts).
  useEffect(
    () => sessionStore.subscribe(() => setSessions(sessionStore.list())),
    [],
  );

  // Poll Workspaces for id + branch — the hop ring and the zoom header.
  useEffect(() => {
    let live = true;
    const tick = async () => {
      try {
        const ws = await api.listWorkspaces();
        if (live) setWorkspaces(ws.map((w) => ({ id: w.id, branch: w.branch })));
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

  const grouped = useMemo(() => groupSessions(sessions), [sessions]);

  const openZoom = useCallback(
    (sessionId: string) => {
      const target = resolveZoomTarget(sessionId, workspaces, grouped);
      if (target) setZoom(target);
    },
    [workspaces, grouped],
  );

  const hop = useCallback(
    (delta: -1 | 1) => {
      setZoom((current) =>
        current
          ? (hopZoomTarget(current.workspaceId, delta, workspaces, grouped) ??
            current)
          : current,
      );
    },
    [workspaces, grouped],
  );

  // `↵` on the wall zooms the first zoomable Workspace (an interim entry
  // until the wall gains a card cursor — see the module journal). Yields to
  // any focused field (e.g. the New Workspace modal).
  useEffect(() => {
    if (zoom) return;
    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key !== "Enter" || isEditable(document.activeElement)) return;
      const target = firstZoomTarget(workspaces, grouped);
      if (target) {
        ev.preventDefault();
        setZoom(target);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [zoom, workspaces, grouped]);

  return (
    <>
      <HelmView onZoomSession={openZoom} />
      {zoom ? (
        <Zoom
          api={api}
          target={zoom}
          onReturn={() => setZoom(null)}
          onHopWorkspace={hop}
        />
      ) : null}
    </>
  );
}

// --- interim overlay mount (dev console entry) ---

const makeChannel: ChannelFactory = <T,>(onMessage: (message: T) => void) => {
  const channel = new Channel<T>();
  channel.onmessage = onMessage;
  return channel;
};

let overlayRoot: Root | null = null;
let overlayHost: HTMLDivElement | null = null;

/** Mount the quarterdeck (wall + zoom) as a full-window overlay. Idempotent;
 * returns an unmount handle. The real app-shell route is a later upstream
 * integration point (see docs/fork-posture.md). */
export function mountQuarterdeck(): () => void {
  if (!overlayHost) {
    overlayHost = document.createElement("div");
    overlayHost.id = "helmsmen-quarterdeck-overlay";
    overlayHost.style.cssText =
      "position:fixed;inset:0;z-index:2147483000;background:#070a0f";
    document.body.append(overlayHost);
    overlayRoot = createRoot(overlayHost);
  }
  const api = createHelmApi(invoke, makeChannel);
  overlayRoot?.render(<Quarterdeck api={api} />);
  return unmountQuarterdeck;
}

export function unmountQuarterdeck(): void {
  overlayRoot?.unmount();
  overlayRoot = null;
  overlayHost?.remove();
  overlayHost = null;
}
