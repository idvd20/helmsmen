// Helmsmen — pure model for the Zoom view (task #12).
//
// Given the spawned Agent Sessions and the Workspaces they belong to, this
// derives the zoom target: which Workspace owns a clicked Session, its
// sibling Sessions as tabs, the active tab, and the `[`/`]` cross-Workspace
// hop. Deterministic over data — no clock, no DOM, no process — so the
// zoom's navigation is CI-checked even though its React shell is not.

import type { HelmAgentSession } from "@/modules/helm/api";
import { hopWorkspaceIndex } from "./keymap";

export type ZoomSessionKind = "agent" | "shell" | "process" | "reviewer";

/** One Session tab in the zoom's left pane. `runtime` + `sessionId` are the
 * opaque handle the write/attach commands echo back. */
export interface ZoomSession {
  sessionId: string;
  runtime: string;
  kind: ZoomSessionKind;
  label: string;
}

/** Minimal Workspace identity the zoom needs (id for grouping, branch for
 * the header). A subset of `HelmWorkspace`, so callers can pass the real
 * entity. */
export interface ZoomWorkspaceRef {
  id: string;
  branch: string;
}

/** What the zoom renders: the active Workspace, its Session tabs, and which
 * tab is live. */
export interface ZoomTarget {
  workspaceId: string;
  branch: string;
  tabs: ZoomSession[];
  activeIndex: number;
}

const HARNESS_TOKEN: Record<string, string> = { "claude-code": "claude" };

/** Session tab label, e.g. `claude·local-pty` — mirrors the wall's chip
 * copy (`{harness}·{runtime}`). */
export function sessionTabLabel(
  session: Pick<HelmAgentSession, "harnessId" | "runtime">,
): string {
  const token = HARNESS_TOKEN[session.harnessId] ?? session.harnessId;
  return `${token}·${session.runtime}`;
}

/** Project a spawned Agent Session onto a zoom tab. */
export function toZoomSession(session: HelmAgentSession): ZoomSession {
  return {
    sessionId: session.sessionId,
    runtime: session.runtime,
    kind: "agent",
    label: sessionTabLabel(session),
  };
}

/** Group Sessions by Workspace, preserving registration (spawn) order so
 * tab numbering `1…9` stays stable. */
export function groupSessions(
  sessions: HelmAgentSession[],
): Record<string, ZoomSession[]> {
  const grouped: Record<string, ZoomSession[]> = {};
  for (const session of sessions) {
    if (!grouped[session.workspaceId]) grouped[session.workspaceId] = [];
    grouped[session.workspaceId].push(toZoomSession(session));
  }
  return grouped;
}

/** The Workspaces that can be zoomed (have at least one Session), in the
 * given Workspace order — the `[`/`]` hop ring. */
export function zoomableWorkspaceIds(
  workspaces: ZoomWorkspaceRef[],
  grouped: Record<string, ZoomSession[]>,
): string[] {
  return workspaces
    .filter((w) => (grouped[w.id]?.length ?? 0) > 0)
    .map((w) => w.id);
}

function targetFor(
  workspaceId: string,
  workspaces: ZoomWorkspaceRef[],
  grouped: Record<string, ZoomSession[]>,
  activeIndex: number,
): ZoomTarget | null {
  const ws = workspaces.find((w) => w.id === workspaceId);
  const tabs = grouped[workspaceId] ?? [];
  if (!ws || tabs.length === 0) return null;
  return {
    workspaceId,
    branch: ws.branch,
    tabs,
    activeIndex: Math.min(Math.max(activeIndex, 0), tabs.length - 1),
  };
}

/** Resolve the zoom target for a clicked/entered Session id: its owning
 * Workspace, the sibling Sessions as tabs, and that Session active. Null if
 * no Workspace owns the id. */
export function resolveZoomTarget(
  sessionId: string,
  workspaces: ZoomWorkspaceRef[],
  grouped: Record<string, ZoomSession[]>,
): ZoomTarget | null {
  for (const ws of workspaces) {
    const tabs = grouped[ws.id] ?? [];
    const activeIndex = tabs.findIndex((t) => t.sessionId === sessionId);
    if (activeIndex >= 0) {
      return { workspaceId: ws.id, branch: ws.branch, tabs, activeIndex };
    }
  }
  return null;
}

/** `[`/`]` hop: the next zoomable Workspace (wrapping, skipping
 * session-less ones), with its first tab active. Null if the current
 * Workspace is not in the hop ring. */
export function hopZoomTarget(
  currentWorkspaceId: string,
  delta: -1 | 1,
  workspaces: ZoomWorkspaceRef[],
  grouped: Record<string, ZoomSession[]>,
): ZoomTarget | null {
  const ring = zoomableWorkspaceIds(workspaces, grouped);
  const current = ring.indexOf(currentWorkspaceId);
  if (current < 0) return null;
  const next = hopWorkspaceIndex(current, delta, ring.length);
  return targetFor(ring[next], workspaces, grouped, 0);
}

/** The default `↵`-zoom entry: the first zoomable Workspace's first
 * Session. Null if nothing has a Session yet. */
export function firstZoomTarget(
  workspaces: ZoomWorkspaceRef[],
  grouped: Record<string, ZoomSession[]>,
): ZoomTarget | null {
  const [first] = zoomableWorkspaceIds(workspaces, grouped);
  return first ? targetFor(first, workspaces, grouped, 0) : null;
}

/** The bytes a message-box submission delivers to the PTY mid-run: the
 * typed text plus a carriage return (Enter). The text is only ever data —
 * it is written to the process verbatim, never interpreted. */
export function messageToPtyLine(text: string): string {
  return `${text}\r`;
}
