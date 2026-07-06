import { describe, expect, it } from "vitest";
import {
  type AnswerPromptInput,
  createHelmApi,
  type HelmApproval,
  type HelmProfile,
  type HelmProject,
  type HelmWorkspace,
  type InvokeFn,
} from "./api";
import {
  buildWall,
  deriveCardAnswerItem,
  deriveWallAnswerTarget,
  describeAnswerOutcome,
  mapHelmWallKey,
  type SessionFacts,
  type WorkspaceFacts,
} from "./viewModel";

// Per-card Allow/Deny from the wall (task #34): `a` / `x` answer the top
// visible Blocked card's paused approval, mirroring the zoom's answer path.
// The repo has no DOM test env, so this drives the exact composition
// Helm.tsx + HelmView.tsx wire:
//
//   mapHelmWallKey → deriveWallAnswerTarget (which card the key acts on)
//     → deriveCardAnswerItem (its agent Session + correlation anchor)
//     → api.answerPrompt (#18's verify-before-inject seam)
//     → describeAnswerOutcome (mismatch surfaced, never discarded)
//
// Every assertion is on real production functions; the literal keydown
// listener render is the thin shell in Helm.tsx (human/verify item).

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

const feature: HelmProfile = {
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

const agentSession = (sessionId: string): SessionFacts => ({
  sessionId,
  kind: "agent",
  runtime: "local-pty",
  harness: "claude",
});

/** A still-open (pending/surfaced) policy `ask` card. */
function ask(
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
    status: "surfaced",
    decision: "ask",
    rule: { id: "git-history-rewrite", label: "git history rewrite" },
    input: { command },
    ...extra,
  };
}

function wallFrom(facts: Record<string, WorkspaceFacts>) {
  return buildWall({
    projects: [alpha, beta],
    workspaces: [wsA, wsB],
    profiles: [feature],
    facts,
    nowMs: 0,
  });
}

/** One agent paused in wsA; wsB is just working (no approval). */
function onePausedFacts(): Record<string, WorkspaceFacts> {
  return {
    wsA: {
      sessions: [agentSession("pty-a")],
      approvals: [ask("card-a", "toolu_a", "git push --force origin main")],
    },
    wsB: {
      sessions: [{ ...agentSession("pty-b"), status: "working" }],
    },
  };
}

/** A recording fake `invoke` (api.test.ts's pattern) so the answer rides the
 * real `createHelmApi` seam and the exact payload is asserted. */
function fakeInvoke(returns: Record<string, unknown> = {}) {
  const calls: Array<[string, Record<string, unknown> | undefined]> = [];
  const invoke: InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    return Promise.resolve((returns[cmd] ?? null) as T);
  };
  return { invoke, calls };
}

describe("deriveWallAnswerTarget — which card `a`/`x` acts on", () => {
  it("targets the top-ranked visible card showing an approval ask block", () => {
    const wall = wallFrom(onePausedFacts());
    // The Blocked card floats to the top of the rank order.
    expect(wall.cards[0].workspaceId).toBe("wsA");
    expect(deriveWallAnswerTarget(wall.cards)).toEqual({
      workspaceId: "wsA",
      askId: "card-a",
    });
  });

  it("with several pending, targets the first card in wall order and its FIRST ask", () => {
    const wall = wallFrom({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [
          ask("card-a1", "toolu_a1", "git push --force"),
          ask("card-a2", "toolu_a2", "rm -rf /etc"),
        ],
      },
      wsB: {
        sessions: [agentSession("pty-b")],
        approvals: [ask("card-b", "toolu_b", "git rebase -i HEAD~3")],
      },
    });
    expect(deriveWallAnswerTarget(wall.cards)).toEqual({
      workspaceId: "wsA",
      askId: "card-a1",
    });
  });

  it("returns null when no visible card has a pending approval (keys stay no-ops)", () => {
    // Nothing pending anywhere: working/idle cards only.
    const wall = wallFrom({
      wsA: { sessions: [{ ...agentSession("pty-a"), status: "working" }] },
      wsB: { sessions: [agentSession("pty-b")] },
    });
    expect(deriveWallAnswerTarget(wall.cards)).toBeNull();
  });

  it("never targets a Blocked card without an ask block (a failed cut is not answerable)", () => {
    const failed: HelmWorkspace = {
      ...wsA,
      cut: {
        phase: "failed",
        step: "setupScript",
        log: "setup exploded",
      },
    };
    const wall = buildWall({
      projects: [alpha],
      workspaces: [failed],
      profiles: [feature],
      facts: {},
      nowMs: 0,
    });
    expect(wall.cards[0].status).toBe("blocked");
    expect(deriveWallAnswerTarget(wall.cards)).toBeNull();
  });
});

describe("deriveCardAnswerItem — one ask resolved to its live answer inputs", () => {
  it("resolves the workspace's agent Session + the ask's correlation anchor", () => {
    expect(
      deriveCardAnswerItem(onePausedFacts(), {
        workspaceId: "wsA",
        askId: "card-a",
      }),
    ).toEqual({
      workspaceId: "wsA",
      agentSession: { sessionId: "pty-a", runtime: "local-pty" },
      toolUseId: "toolu_a",
      expectedCommand: "git push --force origin main",
    });
  });

  it("carries a null agent Session when no agent is live to receive keys", () => {
    const item = deriveCardAnswerItem(
      {
        wsA: {
          sessions: [{ sessionId: "sh", kind: "shell", runtime: "local-pty" }],
          approvals: [ask("card-a", "toolu_a", "git push --force")],
        },
      },
      { workspaceId: "wsA", askId: "card-a" },
    );
    expect(item?.agentSession).toBeNull();
  });

  it("returns null once the ask has resolved (stale key press injects nothing)", () => {
    const facts: Record<string, WorkspaceFacts> = {
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [
          ask("card-a", "toolu_a", "git push --force", { status: "allowed" }),
        ],
      },
    };
    expect(
      deriveCardAnswerItem(facts, { workspaceId: "wsA", askId: "card-a" }),
    ).toBeNull();
    // Unknown workspace / unknown ask id fail safe the same way.
    expect(
      deriveCardAnswerItem(facts, { workspaceId: "wsZ", askId: "card-a" }),
    ).toBeNull();
    expect(
      deriveCardAnswerItem(facts, { workspaceId: "wsA", askId: "nope" }),
    ).toBeNull();
  });
});

describe("wall `a`/`x` → answerPrompt (the composition Helm.tsx wires)", () => {
  const keyCtx = { editing: false, overlayActive: false, pickerOpen: false };

  it("`a` allows THAT card: its session, tool_use_id and command reach the seam", async () => {
    const facts = onePausedFacts();
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: { status: "injected" },
    });
    const api = createHelmApi(invoke);

    const action = mapHelmWallKey({ key: "a" }, keyCtx);
    expect(action).toEqual({ kind: "answer-allow" });

    const target = deriveWallAnswerTarget(wallFrom(facts).cards);
    if (!target) throw new Error("expected an answerable card");
    const item = deriveCardAnswerItem(facts, target);
    if (!item?.agentSession) throw new Error("expected a live agent session");

    const outcome = await api.answerPrompt({
      session: item.agentSession.sessionId,
      runtime: item.agentSession.runtime,
      toolUseId: item.toolUseId,
      expectedCommand: item.expectedCommand,
      action: "allow",
    });

    expect(calls).toHaveLength(1);
    expect(calls[0][0]).toBe("helm_answer_prompt");
    expect(calls[0][1]?.input as AnswerPromptInput).toMatchObject({
      session: "pty-a",
      runtime: "local-pty",
      toolUseId: "toolu_a",
      expectedCommand: "git push --force origin main",
      action: "allow",
    });
    // A clean injection needs no feedback line.
    expect(describeAnswerOutcome(outcome)).toBeNull();
  });

  it("`x` denies THAT card through the same seam", async () => {
    const facts = onePausedFacts();
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: { status: "injected" },
    });
    const api = createHelmApi(invoke);

    const action = mapHelmWallKey({ key: "x" }, keyCtx);
    expect(action).toEqual({ kind: "answer-deny" });

    const target = deriveWallAnswerTarget(wallFrom(facts).cards);
    if (!target) throw new Error("expected an answerable card");
    const item = deriveCardAnswerItem(facts, target);
    if (!item?.agentSession) throw new Error("expected a live agent session");

    await api.answerPrompt({
      session: item.agentSession.sessionId,
      runtime: item.agentSession.runtime,
      toolUseId: item.toolUseId,
      expectedCommand: item.expectedCommand,
      action: "deny",
    });

    expect(calls[0][1]?.input as AnswerPromptInput).toMatchObject({
      session: "pty-a",
      toolUseId: "toolu_a",
      action: "deny",
    });
  });

  it("with nothing pending the keys resolve no target and NOTHING is injected", () => {
    const { calls } = fakeInvoke();
    const wall = wallFrom({
      wsA: { sessions: [{ ...agentSession("pty-a"), status: "working" }] },
    });
    expect(deriveWallAnswerTarget(wall.cards)).toBeNull();
    // No target → the shell never reaches the seam.
    expect(calls).toHaveLength(0);
  });

  it("a mismatch outcome surfaces as a user-facing note, never discarded", async () => {
    const facts = onePausedFacts();
    const { invoke } = fakeInvoke({
      helm_answer_prompt: { status: "mismatch", reason: "dialogNotVisible" },
    });
    const api = createHelmApi(invoke);

    const target = deriveWallAnswerTarget(wallFrom(facts).cards);
    if (!target) throw new Error("expected an answerable card");
    const item = deriveCardAnswerItem(facts, target);
    if (!item?.agentSession) throw new Error("expected a live agent session");

    const outcome = await api.answerPrompt({
      session: item.agentSession.sessionId,
      runtime: item.agentSession.runtime,
      toolUseId: item.toolUseId,
      expectedCommand: item.expectedCommand,
      action: "allow",
    });

    // The backend verified the visible dialog was NOT this card's and
    // injected nothing — the wall must say so.
    const note = describeAnswerOutcome(outcome);
    expect(note).toBe("dialog changed — not answered; re-check the call");
  });
});
