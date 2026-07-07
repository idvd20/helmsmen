import { describe, expect, it } from "vitest";
import type { HelmProject, HelmWorkspace } from "./api";
import {
  applyLiveStatuses,
  type HelmAgentSignal,
  type LiveSessionStatuses,
  MAX_SIGNAL_KIND_LEN,
  reduceAgentSignal,
  sessionStatusFromSignal,
} from "./agentSignal";
import { buildWall, type SessionFacts } from "./viewModel";

const signal = (id: number, kind: string): HelmAgentSignal => ({ id, kind });

describe("sessionStatusFromSignal", () => {
  it("maps the Terax kinds to the PRD dot vocabulary", () => {
    expect(sessionStatusFromSignal("started")).toBe("working");
    expect(sessionStatusFromSignal("working")).toBe("working");
    expect(sessionStatusFromSignal("attention")).toBe("blocked");
    expect(sessionStatusFromSignal("finished")).toBe("done");
  });

  it("returns null for exited (no status contribution)", () => {
    expect(sessionStatusFromSignal("exited")).toBeNull();
  });

  it("ignores unknown and wrong-case kinds — signal content is hostile", () => {
    for (const hostile of [
      "",
      "STARTED",
      "work",
      "notify;Terax;working",
      "133;C;claude",
    ]) {
      expect(sessionStatusFromSignal(hostile)).toBeNull();
    }
  });

  it("drops oversized kinds before matching", () => {
    expect(sessionStatusFromSignal("working".repeat(1000))).toBeNull();
    expect(sessionStatusFromSignal("x".repeat(MAX_SIGNAL_KIND_LEN))).toBeNull();
    // A known kind well under the cap still maps.
    expect(sessionStatusFromSignal("finished")).toBe("done");
  });
});

describe("reduceAgentSignal", () => {
  it("sets a Session's status keyed by signal id", () => {
    const next = reduceAgentSignal({}, signal(7, "working"));
    expect(next).toEqual({ "7": "working" });
  });

  it("updates an existing Session's status", () => {
    const next = reduceAgentSignal({ "7": "working" }, signal(7, "attention"));
    expect(next).toEqual({ "7": "blocked" });
  });

  it("drops the Session on exit so no stale dot lingers", () => {
    const next = reduceAgentSignal(
      { "7": "working", "9": "done" },
      signal(7, "exited"),
    );
    expect(next).toEqual({ "9": "done" });
  });

  it("drops the Session on an unknown kind too", () => {
    const next = reduceAgentSignal({ "7": "working" }, signal(7, "garbage"));
    expect(next).toEqual({});
  });

  it("returns the same reference when nothing changed (stable state)", () => {
    const prev: LiveSessionStatuses = { "7": "working" };
    // Same status re-asserted.
    expect(reduceAgentSignal(prev, signal(7, "working"))).toBe(prev);
    // Exit for an id that was never tracked.
    expect(reduceAgentSignal(prev, signal(42, "exited"))).toBe(prev);
  });
});

describe("applyLiveStatuses", () => {
  const session = (sessionId: string): SessionFacts => ({
    sessionId,
    kind: "agent",
    runtime: "pty",
  });

  it("fills each Session's live status from the map", () => {
    const out = applyLiveStatuses(
      [session("s-1"), session("s-2")],
      { "s-1": "working" },
    );
    expect(out[0].status).toBe("working");
    // A Session with no live signal keeps its (absent) status.
    expect(out[1].status).toBeUndefined();
  });

  it("returns the same array reference when nothing changed", () => {
    const sessions = [session("s-1")];
    expect(applyLiveStatuses(sessions, {})).toBe(sessions);
    expect(applyLiveStatuses(sessions, { "other": "done" })).toBe(sessions);
  });
});

// The seam end-to-end at the pure level: a live agent-signal lights a card's
// dot through the EXISTING buildWall rollup, with no card or rollup change.
describe("live dot through the existing rollup", () => {
  const project: HelmProject = {
    id: "prj-1",
    name: "helmsmen",
    repoRoot: "/home/dev/src/helmsmen",
    baseBranch: "main",
    worktreeHome: "/home/dev/.helmsmen/worktrees/helmsmen",
    branchTemplate: "helm/{slug}",
    settings: { setupScript: "", carryOverGlobs: [], processes: [] },
  };
  // A completed cut derives to Idle on its own (the M2 baseline dot).
  const workspace: HelmWorkspace = {
    id: "ws-1",
    projectId: "prj-1",
    slug: "fix",
    branch: "helm/fix",
    worktreePath: "/home/dev/.helmsmen/worktrees/helmsmen/fix-1",
    slot: 1,
    cut: { phase: "complete", firstSessionId: "s-1" },
  };

  const wallStatus = (live: LiveSessionStatuses) => {
    // #12 provides the Session list; #11 overlays live statuses onto it.
    const sessions = applyLiveStatuses(
      [{ sessionId: "s-1", kind: "agent", runtime: "pty" }],
      live,
    );
    const wall = buildWall({
      projects: [project],
      workspaces: [workspace],
      profiles: [],
      facts: { "ws-1": { sessions } },
      nowMs: 0,
    });
    return wall.cards[0].status;
  };

  it("a completed cut with no live signal stays idle", () => {
    expect(wallStatus({})).toBe("idle");
  });

  it("a working signal lights the dot to working", () => {
    const live = reduceAgentSignal({}, signal(0, "working"));
    // signal id 0 -> key "0"; correlate the Session id to that key.
    expect(wallStatus({ "s-1": live["0"] })).toBe("working");
  });

  it("an attention signal parks the card in Needs you", () => {
    expect(wallStatus({ "s-1": "blocked" })).toBe("blocked");
    const wall = buildWall({
      projects: [project],
      workspaces: [workspace],
      profiles: [],
      facts: {
        "ws-1": {
          sessions: applyLiveStatuses(
            [{ sessionId: "s-1", kind: "agent", runtime: "pty" }],
            { "s-1": "blocked" },
          ),
        },
      },
      nowMs: 0,
    });
    expect(wall.counts.needsYou).toBe(1);
    expect(wall.counts.needsAttention).toBe(true);
  });

  it("after exit the dot falls back to the cut-derived status", () => {
    let live = reduceAgentSignal({}, signal(1, "working"));
    live = reduceAgentSignal(live, signal(1, "exited"));
    // The Session dropped out of the live map, so s-1 has no live status.
    expect(wallStatus(live)).toBe("idle");
  });
});
