import { describe, expect, it } from "vitest";
import {
  applyLiveStatuses,
  type HelmAgentSignal,
  type LiveSessionStatuses,
  reduceAgentSignal,
} from "./agentSignal";
import type { HelmProfile, HelmProject, HelmWorkspace } from "./api";
import {
  applyScope,
  buildWall,
  cycleFilter,
  deriveFilterTabs,
  deriveRepoPicker,
  filterCards,
  groupCards,
  type SessionFacts,
  type WallFilter,
  type WorkspaceFacts,
} from "./viewModel";

// ═══════════════════════════════════════════════════════════════════════
// M2 scripted demo — the milestone "Done when" at the FRONTEND seam.
//
//   Two Projects × two agents, triaged and driven — keyboard-only.
//
// This is the re-runnable, deterministic half of the M2 demo (the backend
// half — real git Projects, a real cut pipeline, a real PTY attach + steer,
// and the pure-core rollup — lives in `pipeline.rs`'s
// `m2_demo_two_projects_two_agents_driven_end_to_end`). Here we drive the
// wall exactly as `HelmView` does at runtime:
//
//   agent-signal  →  reduceAgentSignal  →  applyLiveStatuses  →  buildWall
//                 →  applyScope / filterCards / groupCards / deriveRepoPicker
//
// i.e. hostile OSC signals fold into a live status map, overlay onto Session
// facts, roll up into card statuses, and the `f`/`g`/`r` lenses project the
// resulting wall. Every assertion is on real production functions.
//
// COVERED here (automated): the whole status→view-model path and the
// filter/group/repo-picker derivations against the resulting two-Project,
// two-agent state — the "watch status / triage / scope" narrative.
// NOT covered (human/verify at the running Tauri app, per the no-DOM-test
// constraint): the literal `f`/`g`/`r` key presses re-rendering the wall,
// the dot colors / 30%-dimming pixels, and the repo-picker dropdown's
// keyboard navigation. Those are journaled as verify items.
// ═══════════════════════════════════════════════════════════════════════

const alpha: HelmProject = {
  id: "prj-a",
  name: "alpha",
  repoRoot: "/src/alpha",
  baseBranch: "main",
  worktreeHome: "/wt/alpha",
  branchTemplate: "helm/{slug}",
  settings: { setupScript: "", carryOverGlobs: [], processes: [] },
};
const beta: HelmProject = {
  ...alpha,
  id: "prj-b",
  name: "beta",
  repoRoot: "/src/beta",
  baseBranch: "develop",
  worktreeHome: "/wt/beta",
};

const featureA: HelmProfile = {
  id: "prj-a:feature",
  projectId: "prj-a",
  name: "Feature",
  promptSnippet: "/tdd {brief}",
  model: "",
  mcpServers: [],
  verifyCommand: "",
  color: "#7c3aed",
  harnessId: "claude-code",
};
const featureB: HelmProfile = { ...featureA, id: "prj-b:feature", projectId: "prj-b" };

// Two cut-complete Workspaces, one per Project (the two agents).
const complete = (id: string): HelmWorkspace["cut"] => ({
  phase: "complete",
  firstSessionId: `sess-${id}`,
});
const wsA: HelmWorkspace = {
  id: "wsA",
  projectId: "prj-a",
  slug: "fix-login",
  branch: "helm/fix-login",
  worktreePath: "/wt/alpha/fix-login",
  slot: 1,
  cut: complete("wsA"),
};
const wsB: HelmWorkspace = {
  id: "wsB",
  projectId: "prj-b",
  slug: "add-signup",
  branch: "helm/add-signup",
  worktreePath: "/wt/beta/add-signup",
  slot: 1,
  cut: complete("wsB"),
};

// Each agent's first Session, as the wall's Session facts (populated at
// runtime by the zoom/attach slice; here we seed them directly). The
// Session ids are the stringified signal ids: `reduceAgentSignal` keys the
// live map by `String(signal.id)` and `applyLiveStatuses` reads it by
// `sessionId`, so this is exactly the runtime correlation (M2:
// whole-terminal — the signal's id is the Session's).
const AGENT_A_SIGNAL_ID = 1;
const AGENT_B_SIGNAL_ID = 2;
const sessionA: SessionFacts = {
  sessionId: String(AGENT_A_SIGNAL_ID),
  kind: "agent",
  runtime: "pty",
  harness: "claude",
};
const sessionB: SessionFacts = {
  sessionId: String(AGENT_B_SIGNAL_ID),
  kind: "agent",
  runtime: "pty",
  harness: "claude",
};

/** Fold a stream of agent-signals into the live per-Session status map,
 * exactly as HelmView's `terax:agent-signal` listener does. */
function foldSignals(signals: HelmAgentSignal[]): LiveSessionStatuses {
  return signals.reduce(
    (acc, sig) => reduceAgentSignal(acc, sig),
    {} as LiveSessionStatuses,
  );
}

/** Build the wall from the two Workspaces after overlaying the live status
 * map onto each agent's Session — the runtime path end to end. The signal
 * ids key by Session id (M2: whole-terminal; the frontend keys the map by
 * the signal's id, which the demo aligns to each Session's id). */
function wallFrom(live: LiveSessionStatuses) {
  const facts: Record<string, WorkspaceFacts> = {
    wsA: { profileId: "prj-a:feature", sessions: applyLiveStatuses([sessionA], live) },
    wsB: { profileId: "prj-b:feature", sessions: applyLiveStatuses([sessionB], live) },
  };
  return buildWall({
    projects: [alpha, beta],
    workspaces: [wsA, wsB],
    profiles: [featureA, featureB],
    facts,
    nowMs: 0,
  });
}

const sig = (id: number, kind: string): HelmAgentSignal => ({ id, kind });
const AGENT_A = AGENT_A_SIGNAL_ID;
const AGENT_B = AGENT_B_SIGNAL_ID;

describe("M2 demo — two Projects × two agents, triaged and driven", () => {
  it("both agents start Idle after their cuts complete (no live signal yet)", () => {
    const wall = wallFrom(foldSignals([]));
    expect(wall.cards).toHaveLength(2);
    expect(wall.cards.every((c) => c.status === "idle")).toBe(true);
    // Two Projects present, each with its own agent.
    expect(new Set(wall.cards.map((c) => c.projectId))).toEqual(
      new Set(["prj-a", "prj-b"]),
    );
  });

  it("live signals drive status: agent A asks (Needs you), agent B works", () => {
    // agent A: started → working → attention (asks for approval).
    // agent B: started → working (still going).
    const live = foldSignals([
      sig(AGENT_A, "started"),
      sig(AGENT_B, "started"),
      sig(AGENT_A, "working"),
      sig(AGENT_B, "working"),
      sig(AGENT_A, "attention"),
    ]);
    const wall = wallFrom(live);
    const byId = Object.fromEntries(wall.cards.map((c) => [c.workspaceId, c]));
    expect(byId.wsA.status).toBe("blocked"); // "Needs you"
    expect(byId.wsB.status).toBe("working");

    // The header strip turns red (something needs the user) and counts are live.
    expect(wall.counts).toMatchObject({ needsYou: 1, working: 1, needsAttention: true });

    // Blocked floats to the top of the rank sort (attention order).
    expect(wall.cards[0].workspaceId).toBe("wsA");
  });

  it("`f` cycles status filters with tab counts + dots staying correct live", () => {
    const wall = wallFrom(
      foldSignals([sig(AGENT_A, "attention"), sig(AGENT_B, "working")]),
    );
    // Cycle from All and assert each tab narrows to the right agent.
    let filter: WallFilter = "all";
    const tabs = deriveFilterTabs(wall.cards, filter);
    expect(tabs.find((t) => t.filter === "all")?.count).toBe(2);
    expect(tabs.find((t) => t.filter === "blocked")?.count).toBe(1);
    expect(tabs.find((t) => t.filter === "working")?.count).toBe(1);
    expect(tabs.find((t) => t.filter === "done")?.dimmed).toBe(true);

    filter = cycleFilter(filter); // → blocked / "Needs you"
    expect(filter).toBe("blocked");
    expect(filterCards(wall.cards, filter).map((c) => c.workspaceId)).toEqual([
      "wsA",
    ]);

    filter = cycleFilter(filter); // → working
    expect(filterCards(wall.cards, filter).map((c) => c.workspaceId)).toEqual([
      "wsB",
    ]);
  });

  it("`g` groups flat/Project with headers showing repo name/count/base branch", () => {
    const wall = wallFrom(foldSignals([sig(AGENT_B, "working")]));

    const flat = groupCards(wall.cards, [alpha, beta], "flat");
    expect(flat).toHaveLength(1);
    expect(flat[0].header).toBeNull();

    const grouped = groupCards(wall.cards, [alpha, beta], "project");
    expect(grouped.map((g) => g.header)).toEqual([
      { projectId: "prj-a", repoName: "alpha", count: 1, baseBranch: "main" },
      { projectId: "prj-b", repoName: "beta", count: 1, baseBranch: "develop" },
    ]);
  });

  it("`r` repo picker shows live counts + worst-status dot and scopes the wall", () => {
    const wall = wallFrom(
      foldSignals([sig(AGENT_A, "attention"), sig(AGENT_B, "working")]),
    );
    const picker = deriveRepoPicker(wall.cards, [alpha, beta]);
    expect(picker.allActive).toBe(2);
    expect(picker.entries).toEqual([
      { projectId: "prj-a", name: "alpha", baseBranch: "main", count: 1, worstStatus: "blocked" },
      { projectId: "prj-b", name: "beta", baseBranch: "develop", count: 1, worstStatus: "working" },
    ]);

    // Pick beta → the wall scopes to Project B; its filter tabs recount.
    const scoped = applyScope(wall.cards, "prj-b");
    expect(scoped.map((c) => c.workspaceId)).toEqual(["wsB"]);
    const scopedTabs = deriveFilterTabs(scoped, "all");
    expect(scopedTabs.find((t) => t.filter === "working")?.count).toBe(1);
    expect(scopedTabs.find((t) => t.filter === "blocked")?.count).toBe(0);
  });

  it("driving to done: approving agent A, agent B finishes → all To review", () => {
    // Triaged end-to-end: A was blocked, gets steered/approved back to
    // working then finishes; B finishes. Both land in "To review".
    const live = foldSignals([
      sig(AGENT_A, "attention"),
      sig(AGENT_B, "working"),
      sig(AGENT_A, "working"), // approved → back to work
      sig(AGENT_A, "finished"),
      sig(AGENT_B, "finished"),
    ]);
    const wall = wallFrom(live);
    expect(wall.cards.every((c) => c.status === "done")).toBe(true);
    expect(wall.counts).toMatchObject({ toReview: 2, needsYou: 0, needsAttention: false });

    // Filtered to "To review", both agents show; dropping a Session (exit)
    // returns that Workspace to the cut-derived Idle.
    expect(filterCards(wall.cards, "done")).toHaveLength(2);
    const afterExit = wallFrom(
      foldSignals([
        sig(AGENT_A, "finished"),
        sig(AGENT_B, "finished"),
        sig(AGENT_A, "exited"),
      ]),
    );
    const a = afterExit.cards.find((c) => c.workspaceId === "wsA");
    expect(a?.status).toBe("idle");
  });
});
