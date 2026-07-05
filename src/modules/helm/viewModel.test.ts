import { describe, expect, it } from "vitest";
import type {
  HelmCutState,
  HelmProfile,
  HelmProject,
  HelmWorkspace,
} from "./api";
import { HELM_STATUS_ALIAS } from "./api";
import {
  buildWall,
  DEFAULT_PROFILE_COLOR,
  deriveCardBody,
  deriveHeaderCounts,
  elapsedMinutes,
  rankSort,
  rollUpStatus,
  type SessionFacts,
  sessionChipLabel,
  STATUS_RANK,
  type WorkspaceCardView,
} from "./viewModel";

// Pure view-model derivation for the Helm wall (task #10). Encodes the
// acceptance criteria as unit tests over deterministic data:
//   - status is a rolled-up derivation, never stored;
//   - the wall is rank-sorted Needs you -> To review -> Working -> Idle
//     across all Projects (a sort, not sections);
//   - header counts are correct and turn red when needs-you > 0;
//   - the card body shape matches the status;
//   - elapsed minutes derive from timestamps passed in as data;
//   - Working dots never pulse.

const projectA: HelmProject = {
  id: "prj-a",
  name: "alpha",
  repoRoot: "/src/alpha",
  baseBranch: "main",
  worktreeHome: "/wt/alpha",
  branchTemplate: "helm/{slug}",
  settings: { setupScript: "", carryOverGlobs: [], processes: [] },
};
const projectB: HelmProject = {
  ...projectA,
  id: "prj-b",
  name: "beta",
  repoRoot: "/src/beta",
  baseBranch: "develop",
};
const projectC: HelmProject = {
  ...projectA,
  id: "prj-c",
  name: "gamma",
  repoRoot: "/src/gamma",
};

const profileA: HelmProfile = {
  id: "prof-a",
  projectId: "prj-a",
  name: "Feature",
  promptSnippet: "/tdd {brief}",
  model: "",
  mcpServers: [],
  verifyCommand: "",
  color: "#7c3aed",
  harnessId: "claude-code",
};
const profileB: HelmProfile = {
  ...profileA,
  id: "prof-b",
  projectId: "prj-b",
  color: "#0ea5e9",
};

function ws(
  id: string,
  projectId: string,
  cut: HelmCutState,
  branch = `helm/${id}`,
): HelmWorkspace {
  return {
    id,
    projectId,
    slug: id,
    branch,
    worktreePath: `/wt/${id}`,
    slot: 1,
    cut,
  };
}

const complete: HelmCutState = { phase: "complete", firstSessionId: "" };
const cutting: HelmCutState = { phase: "cutting" };
const failed: HelmCutState = {
  phase: "failed",
  step: "setupScript",
  log: "pnpm ERR! boom",
};

describe("STATUS_RANK", () => {
  it("orders the wall Needs you -> To review -> Working -> Idle", () => {
    expect(STATUS_RANK.blocked).toBe(0);
    expect(STATUS_RANK.done).toBe(1);
    expect(STATUS_RANK.working).toBe(2);
    expect(STATUS_RANK.idle).toBe(3);
  });
});

describe("rollUpStatus", () => {
  it("a failed cut parks the Workspace as blocked regardless of Sessions", () => {
    expect(rollUpStatus("blocked", ["working", "done"])).toBe("blocked");
  });

  it("with no Sessions the cut-derived status stands (M2 seam)", () => {
    expect(rollUpStatus("idle", [])).toBe("idle");
    expect(rollUpStatus("working", [])).toBe("working");
  });

  it("any blocked Session wins", () => {
    expect(rollUpStatus("idle", ["working", "blocked", "done"])).toBe(
      "blocked",
    );
  });

  it("else any working Session wins", () => {
    expect(rollUpStatus("idle", ["idle", "working", "done"])).toBe("working");
  });

  it("else all done rolls up to done", () => {
    expect(rollUpStatus("idle", ["done", "done"])).toBe("done");
  });

  it("else (some idle, none working/blocked) rolls up to idle", () => {
    expect(rollUpStatus("idle", ["done", "idle"])).toBe("idle");
    expect(rollUpStatus("done", ["idle"])).toBe("idle");
  });
});

describe("sessionChipLabel", () => {
  const base: SessionFacts = {
    sessionId: "s1",
    kind: "agent",
    runtime: "tmux",
  };

  it("labels an agent chip harness-dot-runtime", () => {
    expect(sessionChipLabel({ ...base, harness: "claude" })).toBe(
      "claude·tmux",
    );
    expect(
      sessionChipLabel({ ...base, harness: "codex", runtime: "pty" }),
    ).toBe("codex·pty");
  });

  it("falls back to a generic harness token", () => {
    expect(sessionChipLabel(base)).toBe("agent·tmux");
  });

  it("labels a shell chip", () => {
    expect(sessionChipLabel({ ...base, kind: "shell" })).toBe("shell");
  });

  it("labels a process chip name-colon-port", () => {
    expect(
      sessionChipLabel({
        ...base,
        kind: "process",
        processName: "dev",
        port: 5173,
      }),
    ).toBe("dev:5173");
    expect(
      sessionChipLabel({ ...base, kind: "process", processName: "worker" }),
    ).toBe("worker");
  });

  it("labels a reviewer chip", () => {
    expect(sessionChipLabel({ ...base, kind: "reviewer" })).toBe("reviewer");
  });
});

describe("elapsedMinutes", () => {
  it("floors whole minutes from timestamps passed in as data", () => {
    const now = 1_000_000_000;
    expect(elapsedMinutes(now, now - 5 * 60_000)).toBe(5);
    expect(elapsedMinutes(now, now - 5 * 60_000 - 59_000)).toBe(5);
  });

  it("clamps to zero for a future or equal start", () => {
    const now = 1_000_000_000;
    expect(elapsedMinutes(now, now)).toBe(0);
    expect(elapsedMinutes(now, now + 60_000)).toBe(0);
  });
});

describe("deriveCardBody", () => {
  it.each([
    ["blocked", "ask"],
    ["working", "activity"],
    ["idle", "activity"],
    ["done", "diffstat"],
  ] as const)("body for %s status is %s", (status, kind) => {
    const body = deriveCardBody(status, { cut: complete });
    expect(body.kind).toBe(kind);
  });

  it("a failed-cut blocked body carries the step label and hostile log verbatim", () => {
    const body = deriveCardBody("blocked", { cut: failed });
    expect(body.kind).toBe("ask");
    if (body.kind !== "ask") throw new Error("unreachable");
    expect(body.step).toBe("setup script");
    expect(body.log).toBe("pnpm ERR! boom");
  });

  it("a session-blocked body is an ask placeholder (approval renders at M3.5)", () => {
    const body = deriveCardBody("blocked", { cut: complete });
    expect(body.kind).toBe("ask");
    if (body.kind !== "ask") throw new Error("unreachable");
    expect(body.step).toBeNull();
    expect(body.log).toBeNull();
  });

  it("a done body carries diffstat numbers and a verify slot", () => {
    const body = deriveCardBody("done", {
      cut: complete,
      diffstat: { files: 3, added: 40, removed: 7 },
    });
    expect(body).toEqual({
      kind: "diffstat",
      files: 3,
      added: 40,
      removed: 7,
      verify: "unknown",
    });
  });

  it("activity bodies prefer supplied lines over the default", () => {
    const body = deriveCardBody("working", {
      cut: complete,
      activityLines: ["running tests"],
    });
    expect(body).toEqual({ kind: "activity", lines: ["running tests"] });
  });
});

describe("deriveHeaderCounts", () => {
  it("counts needs-you / working / to-review and never counts idle", () => {
    const counts = deriveHeaderCounts([
      "blocked",
      "blocked",
      "working",
      "done",
      "idle",
      "idle",
    ]);
    expect(counts).toEqual({
      needsYou: 2,
      working: 1,
      toReview: 1,
      needsAttention: true,
    });
  });

  it("turns red only when needs-you > 0", () => {
    expect(deriveHeaderCounts(["working", "done"]).needsAttention).toBe(false);
    expect(deriveHeaderCounts(["idle"]).needsAttention).toBe(false);
    expect(deriveHeaderCounts(["blocked"]).needsAttention).toBe(true);
  });
});

describe("rankSort", () => {
  it("sorts by rank and is stable within a rank", () => {
    const cards = [
      { workspaceId: "i1", rank: STATUS_RANK.idle },
      { workspaceId: "w1", rank: STATUS_RANK.working },
      { workspaceId: "b1", rank: STATUS_RANK.blocked },
      { workspaceId: "d1", rank: STATUS_RANK.done },
      { workspaceId: "b2", rank: STATUS_RANK.blocked },
    ];
    expect(rankSort(cards).map((c) => c.workspaceId)).toEqual([
      "b1",
      "b2",
      "d1",
      "w1",
      "i1",
    ]);
  });

  it("does not mutate its input", () => {
    const cards = [
      { workspaceId: "i1", rank: STATUS_RANK.idle },
      { workspaceId: "b1", rank: STATUS_RANK.blocked },
    ];
    rankSort(cards);
    expect(cards.map((c) => c.workspaceId)).toEqual(["i1", "b1"]);
  });
});

describe("buildWall", () => {
  const now = 2_000_000_000;
  const facts = {
    w1: { profileId: "prof-a", startedAtMs: now - 5 * 60_000 },
    w4: {
      startedAtMs: now - 12 * 60_000,
      sessions: [
        {
          sessionId: "sess-4",
          kind: "agent",
          runtime: "tmux",
          harness: "claude",
          status: "done",
        } satisfies SessionFacts,
      ],
    },
  };

  const wall = buildWall({
    projects: [projectA, projectB, projectC],
    profiles: [profileA, profileB],
    workspaces: [
      ws("w1", "prj-a", complete), // idle
      ws("w2", "prj-b", failed), // blocked (failed cut)
      ws("w3", "prj-a", cutting), // working
      ws("w4", "prj-b", complete), // done (session rollup)
      ws("w5", "prj-c", complete), // idle, project has no profile
    ],
    facts,
    nowMs: now,
  });

  it("rank-sorts cards across Projects (a flat wall, not per-project)", () => {
    expect(wall.cards.map((c) => c.workspaceId)).toEqual([
      "w2", // blocked
      "w4", // done
      "w3", // working
      "w1", // idle (prj-a comes before prj-c at equal rank, input order)
      "w5", // idle
    ]);
  });

  it("derives header counts with the red rule", () => {
    expect(wall.counts).toEqual({
      needsYou: 1,
      working: 1,
      toReview: 1,
      needsAttention: true,
    });
  });

  it("labels each card with its status alias", () => {
    const w2 = wall.cards.find((c) => c.workspaceId === "w2");
    const w4 = wall.cards.find((c) => c.workspaceId === "w4");
    expect(w2?.statusLabel).toBe(HELM_STATUS_ALIAS.blocked); // "Needs you"
    expect(w4?.statusLabel).toBe(HELM_STATUS_ALIAS.done); // "To review"
  });

  it("shapes the body to the status and names the project + base branch", () => {
    const w2 = wall.cards.find((c) => c.workspaceId === "w2");
    expect(w2?.body.kind).toBe("ask");
    expect(w2?.projectName).toBe("beta");
    expect(w2?.baseBranch).toBe("develop");
  });

  it("follows the Profile color, falling back when unresolved", () => {
    const w1 = wall.cards.find((c) => c.workspaceId === "w1");
    const w3 = wall.cards.find((c) => c.workspaceId === "w3");
    const w5 = wall.cards.find((c) => c.workspaceId === "w5");
    expect(w1?.profileColor).toBe("#7c3aed"); // explicit profileId
    expect(w3?.profileColor).toBe("#7c3aed"); // sole prj-a profile
    expect(w5?.profileColor).toBe(DEFAULT_PROFILE_COLOR); // no prj-c profile
  });

  it("puts elapsed minutes on every card from the passed-in timestamps", () => {
    const w1 = wall.cards.find((c) => c.workspaceId === "w1");
    const w4 = wall.cards.find((c) => c.workspaceId === "w4");
    const w3 = wall.cards.find((c) => c.workspaceId === "w3");
    expect(w1?.elapsedMinutes).toBe(5);
    expect(w4?.elapsedMinutes).toBe(12);
    expect(w3?.elapsedMinutes).toBe(0); // no timestamp supplied
  });

  it("builds clickable Session chips that target the Session id", () => {
    const w4 = wall.cards.find((c) => c.workspaceId === "w4");
    expect(w4?.chips).toEqual([
      {
        sessionId: "sess-4",
        kind: "agent",
        label: "claude·tmux",
        status: "done",
      },
    ]);
  });

  it("never pulses a Working dot (pulse is an M5 setting, shipped Off)", () => {
    for (const card of wall.cards) {
      expect(card.pulse).toBe(false);
    }
    const working = wall.cards.find(
      (c) => c.status === "working",
    ) as WorkspaceCardView;
    expect(working.pulse).toBe(false);
  });
});
