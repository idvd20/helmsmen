import { describe, expect, it } from "vitest";
import type {
  HelmApproval,
  HelmProfile,
  HelmProject,
  HelmWorkspace,
} from "./api";
import {
  buildWall,
  deriveBulkApprovals,
  deriveBulkAnswerPlan,
  nextAllowAllConfirm,
  type SessionFacts,
  type WorkspaceFacts,
} from "./viewModel";

// Pure derivations behind the bulk-approvals banner (task #19). The banner's
// count + one-line preview and its bulk Allow-all/Deny-all plan are total
// transforms over the same pending queue the on-card ask blocks read, so both
// are unit-tested here (the repo has no DOM test env; the live banner render +
// key presses are human/verify items). Every preview string is hostile agent
// text — the banner renders it via escaped JSX only (guards.test enforces it).

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

/** A still-open policy `ask` card (pending), the shape the endpoint serializes.
 * Defaults to a git-history-rewrite risk so the preview carries a real rule. */
function askCard(
  id: string,
  toolUseId: string,
  command: string,
  extra: Partial<HelmApproval> = {},
): HelmApproval {
  return {
    id,
    seq: 1,
    sessionId: "sess",
    toolName: "Bash",
    toolUseId,
    status: "pending",
    decision: "ask",
    rule: { id: "git-history-rewrite", label: "git history rewrite" },
    input: { command },
    ...extra,
  };
}

const agentSession = (sessionId: string): SessionFacts => ({
  sessionId,
  kind: "agent",
  runtime: "local-pty",
  harness: "claude",
});

/** The two-Project, two-agent facts the M3.5 demo pauses on: one pending ask
 * in each Project, each Workspace holding a live agent Session. */
function twoQueuedFacts(): Record<string, WorkspaceFacts> {
  return {
    wsA: {
      profileId: "prj-a:feature",
      sessions: [agentSession("pty-a")],
      approvals: [askCard("card-a", "toolu_a", "git push --force origin main")],
    },
    wsB: {
      profileId: "prj-b:feature",
      sessions: [agentSession("pty-b")],
      approvals: [askCard("card-b", "toolu_b", "git rebase -i HEAD~3")],
    },
  };
}

function wallFrom(facts: Record<string, WorkspaceFacts>) {
  return buildWall({
    projects: [alpha, beta],
    workspaces: [wsA, wsB],
    profiles: [featureA, featureB],
    facts,
    nowMs: 0,
  });
}

describe("deriveBulkApprovals — banner count + one-line preview", () => {
  it("shows the banner only when MORE THAN ONE approval is pending", () => {
    // Zero pending → hidden.
    expect(deriveBulkApprovals(wallFrom({}).cards)).toMatchObject({
      count: 0,
      visible: false,
    });

    // Exactly one pending → the on-card ask block alone, NO banner.
    const one = wallFrom({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [askCard("card-a", "toolu_a", "git push --force")],
      },
    });
    expect(deriveBulkApprovals(one.cards)).toMatchObject({
      count: 1,
      visible: false,
    });

    // Two pending across two Projects → banner shows.
    expect(deriveBulkApprovals(wallFrom(twoQueuedFacts()).cards)).toMatchObject({
      count: 2,
      visible: true,
    });
  });

  it("counts two pending asks in ONE Workspace and shows the banner", () => {
    // The rule is >1 pending anywhere on the wall, not >1 Workspace.
    const wall = wallFrom({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [
          askCard("card-a1", "toolu_a1", "git push --force"),
          askCard("card-a2", "toolu_a2", "rm -rf /etc"),
        ],
      },
    });
    const bulk = deriveBulkApprovals(wall.cards);
    expect(bulk.count).toBe(2);
    expect(bulk.visible).toBe(true);
    expect(bulk.previews.map((p) => p.id)).toEqual(["card-a1", "card-a2"]);
  });

  it("previews EVERY pending ask with its Workspace context, in wall order", () => {
    const bulk = deriveBulkApprovals(wallFrom(twoQueuedFacts()).cards);
    // Both Projects appear in the one queue; the exact (hostile) command rides
    // each preview line for the render (escaped JSX only).
    expect(bulk.previews).toEqual([
      {
        id: "card-a",
        workspaceId: "wsA",
        projectName: "alpha",
        branch: "helm/fix-login",
        tool: "Bash",
        rule: "git history rewrite",
        command: "git push --force origin main",
      },
      {
        id: "card-b",
        workspaceId: "wsB",
        projectName: "beta",
        branch: "helm/add-signup",
        tool: "Bash",
        rule: "git history rewrite",
        command: "git rebase -i HEAD~3",
      },
    ]);
  });

  it("ignores allow/deny audit cards and already-resolved asks", () => {
    const wall = wallFrom({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [
          askCard("open", "toolu_o", "git push --force"),
          askCard("allowed", "toolu_x", "ls", { status: "allowed" }),
          askCard("audit", "toolu_y", "ls", {
            decision: "allow",
            rule: undefined,
          }),
        ],
      },
      wsB: {
        sessions: [agentSession("pty-b")],
        approvals: [askCard("open2", "toolu_o2", "git rebase -i HEAD~2")],
      },
    });
    const bulk = deriveBulkApprovals(wall.cards);
    // Only the two still-open asks are queued.
    expect(bulk.count).toBe(2);
    expect(bulk.previews.map((p) => p.id).sort()).toEqual(["open", "open2"]);
  });
});

describe("deriveBulkAnswerPlan — the Allow-all/Deny-all target list", () => {
  it("plans one answer per pending ask, resolving each Workspace's agent session", () => {
    const plan = deriveBulkAnswerPlan(twoQueuedFacts());
    expect(plan).toEqual(
      expect.arrayContaining([
        {
          workspaceId: "wsA",
          agentSession: { sessionId: "pty-a", runtime: "local-pty" },
          toolUseId: "toolu_a",
          expectedCommand: "git push --force origin main",
        },
        {
          workspaceId: "wsB",
          agentSession: { sessionId: "pty-b", runtime: "local-pty" },
          toolUseId: "toolu_b",
          expectedCommand: "git rebase -i HEAD~3",
        },
      ]),
    );
    expect(plan).toHaveLength(2);
  });

  it("carries a null agent session when a Workspace has no agent to answer", () => {
    const plan = deriveBulkAnswerPlan({
      wsA: {
        // A shell session cannot receive approval keys — the agent is absent.
        sessions: [{ sessionId: "sh", kind: "shell", runtime: "local-pty" }],
        approvals: [askCard("card-a", "toolu_a", "git push --force")],
      },
    });
    expect(plan).toEqual([
      {
        workspaceId: "wsA",
        agentSession: null,
        toolUseId: "toolu_a",
        expectedCommand: "git push --force",
      },
    ]);
  });

  it("skips Workspaces with nothing pending", () => {
    const plan = deriveBulkAnswerPlan({
      wsA: { sessions: [agentSession("pty-a")], approvals: [] },
      wsB: { sessions: [agentSession("pty-b")] },
    });
    expect(plan).toEqual([]);
  });
});

describe("nextAllowAllConfirm — two-press confirm guarding Allow-all", () => {
  it("arms on the first press and fires on the second", () => {
    const first = nextAllowAllConfirm(false);
    expect(first).toEqual({ armed: true, fire: false });
    const second = nextAllowAllConfirm(first.armed);
    expect(second).toEqual({ armed: false, fire: true });
  });
});
