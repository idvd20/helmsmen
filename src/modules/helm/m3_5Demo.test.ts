import { describe, expect, it } from "vitest";
import {
  type AnswerPromptInput,
  createHelmApi,
  type HelmApproval,
  type HelmBulkAction,
  type HelmProfile,
  type HelmProject,
  type HelmWorkspace,
  type InvokeFn,
} from "./api";
import { executeBulkAnswers } from "./bulkAnswer";
import {
  buildWall,
  deriveBulkApprovals,
  deriveBulkAnswerPlan,
  nextAllowAllConfirm,
  type SessionFacts,
  type WorkspaceFacts,
} from "./viewModel";

// ═══════════════════════════════════════════════════════════════════════
// M3.5 scripted demo — the milestone "Done when" at the FRONTEND seam.
//
//   Two agents Blocked in DIFFERENT Projects, both visible in ONE queue;
//   one ALLOWED and resumed exactly where it paused, one DENIED and
//   rerouted — driven from the bulk-approvals banner + the per-card seam.
//
// This is the re-runnable, deterministic frontend half of the M3.5 demo. The
// backend half — the real control-plane reducer resolving one Allow to
// `Allowed` (by tool_use_id) and one Deny to `ClosedNoRun`, plus the distinct
// bulk logging — lives in `src-tauri/.../core/control_plane.rs`'s
// `m3_5_demo_two_projects_one_queue_allow_one_deny_one`. The one fragile live
// link (`answer_prompt` against a real `claude` PTY) is the `#[ignore]`d
// `live_claude_answer_prompt_seam` in `runtime::answer`. Here we drive the wall
// exactly as `HelmView` does:
//
//   facts (control-plane snapshots + sessions) → buildWall
//     → deriveBulkApprovals (banner count/preview, >1-pending rule)
//     → deriveBulkAnswerPlan → api.answerPrompt / api.recordBulkDecision
//
// Every assertion is on real production functions.
//
// COVERED here (automated): the unified queue across two Projects, the banner
// >1-pending rule, the bulk answer plan, the per-card Allow/Deny+reroute seam
// calls (resuming each paused call by tool_use_id), and the queue draining as
// answered cards resolve.
// NOT covered (human/verify at the running Tauri app, per the no-DOM-test
// constraint): the literal `A`/`X` key presses, the two-press confirm, the
// banner render + dot colors. Those are journaled as verify items.
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

/** A pending, surfaced (blocked on a permission prompt) risk `ask` card. */
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

/** The demo's paused state: agent A (Project alpha) paused on a force-push,
 * agent B (Project beta) on an interactive rebase — both blocked, one queue. */
function pausedFacts(): Record<string, WorkspaceFacts> {
  return {
    wsA: {
      profileId: "prj-a:feature",
      sessions: [agentSession("pty-a")],
      approvals: [ask("card-a", "toolu_a", "git push --force origin main")],
    },
    wsB: {
      profileId: "prj-b:feature",
      sessions: [agentSession("pty-b")],
      approvals: [ask("card-b", "toolu_b", "git rebase -i HEAD~3")],
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

/** A recording fake `invoke`, like api.test.ts's, so the demo drives the real
 * `createHelmApi` seam and asserts the exact commands + payloads. */
function fakeInvoke(returns: Record<string, unknown> = {}) {
  const calls: Array<[string, Record<string, unknown> | undefined]> = [];
  const invoke: InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    return Promise.resolve((returns[cmd] ?? null) as T);
  };
  return { invoke, calls };
}

describe("M3.5 demo — two Projects Blocked, one queue, one allowed + one denied", () => {
  it("both agents surface in ONE banner queue; the banner shows only when >1", () => {
    const wall = wallFrom(pausedFacts());
    // Both Workspaces are Blocked ("Needs you") and float to the top.
    expect(wall.counts).toMatchObject({ needsYou: 2, needsAttention: true });
    expect(wall.cards.every((c) => c.status === "blocked")).toBe(true);

    const bulk = deriveBulkApprovals(wall.cards);
    expect(bulk.visible).toBe(true);
    expect(bulk.count).toBe(2);
    // One queue spanning two Projects.
    expect(bulk.previews.map((p) => p.projectName)).toEqual(["alpha", "beta"]);
    expect(bulk.previews.map((p) => p.command)).toEqual([
      "git push --force origin main",
      "git rebase -i HEAD~3",
    ]);

    // Banner rule: exactly one pending is the on-card ask block alone.
    const onlyOne = wallFrom({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [ask("card-a", "toolu_a", "git push --force origin main")],
      },
    });
    expect(deriveBulkApprovals(onlyOne.cards).visible).toBe(false);
  });

  it("triage through the ONE queue: allow A (resume exactly), deny+reroute B", async () => {
    const facts = pausedFacts();
    const { invoke, calls } = fakeInvoke({ helm_answer_prompt: { status: "injected" } });
    const api = createHelmApi(invoke);

    // The bulk plan resolves each pending ask to its agent Session + the
    // correlation anchor the answer seam verifies on screen.
    const plan = deriveBulkAnswerPlan(facts);
    const itemA = plan.find((p) => p.workspaceId === "wsA");
    const itemB = plan.find((p) => p.workspaceId === "wsB");
    if (!itemA?.agentSession || !itemB?.agentSession) throw new Error("no agent");

    // Allow agent A: no reroute reason — the paused call resumes and runs,
    // correlated strictly by tool_use_id (resumed exactly where it paused).
    await api.answerPrompt({
      session: itemA.agentSession.sessionId,
      runtime: itemA.agentSession.runtime,
      toolUseId: itemA.toolUseId,
      expectedCommand: itemA.expectedCommand,
      action: "allow",
    });
    // Deny agent B with a reroute instruction (lands as a user message; the
    // tool verifiably never runs).
    await api.answerPrompt({
      session: itemB.agentSession.sessionId,
      runtime: itemB.agentSession.runtime,
      toolUseId: itemB.toolUseId,
      expectedCommand: itemB.expectedCommand,
      action: "deny",
      reason: "open a PR against main instead",
    });

    const allowCall = calls[0][1]?.input as AnswerPromptInput;
    const denyCall = calls[1][1]?.input as AnswerPromptInput;
    expect(allowCall).toMatchObject({
      session: "pty-a",
      toolUseId: "toolu_a",
      expectedCommand: "git push --force origin main",
      action: "allow",
    });
    expect(allowCall.reason).toBeUndefined();
    expect(denyCall).toMatchObject({
      session: "pty-b",
      toolUseId: "toolu_b",
      expectedCommand: "git rebase -i HEAD~3",
      action: "deny",
      reason: "open a PR against main instead",
    });

    // After the reducer resolves both (A ran → Allowed, B never ran →
    // ClosedNoRun), neither is pending: the queue drains and the banner clears.
    const resolved = wallFrom({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [
          ask("card-a", "toolu_a", "git push --force origin main", {
            status: "allowed",
          }),
        ],
      },
      wsB: {
        sessions: [agentSession("pty-b")],
        approvals: [
          ask("card-b", "toolu_b", "git rebase -i HEAD~3", {
            status: "closedNoRun",
          }),
        ],
      },
    });
    const drained = deriveBulkApprovals(resolved.cards);
    expect(drained.count).toBe(0);
    expect(drained.visible).toBe(false);
  });

  it("bulk Allow-all: two-press confirm, one distinct log per Workspace + a keyed answer per card", async () => {
    const facts = pausedFacts();
    // Snapshots at record time still show each card pending (its resolution
    // rides the async hook path), so the answered set covers the queue.
    const snapshots: Record<string, unknown> = {
      wsA: {
        cards: [ask("card-a", "toolu_a", "git push --force origin main")],
        warnings: [],
        eventCount: 1,
        records: [],
      },
      wsB: {
        cards: [ask("card-b", "toolu_b", "git rebase -i HEAD~3")],
        warnings: [],
        eventCount: 1,
        records: [],
      },
    };
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke: InvokeFn = <T>(
      cmd: string,
      args?: Record<string, unknown>,
    ) => {
      calls.push([cmd, args]);
      if (cmd === "helm_answer_prompt")
        return Promise.resolve({ status: "injected" } as T);
      if (cmd === "helm_approvals_snapshot")
        return Promise.resolve(snapshots[args?.workspaceId as string] as T);
      if (cmd === "helm_record_bulk_decision") return Promise.resolve(1 as T);
      return Promise.resolve(null as T);
    };
    const api = createHelmApi(invoke);

    // Two-press confirm guards Allow-all: the first press only arms it.
    const first = nextAllowAllConfirm(false);
    expect(first.fire).toBe(false);
    const second = nextAllowAllConfirm(first.armed);
    expect(second.fire).toBe(true);

    // On fire, the shell runs the bulk plan through `executeBulkAnswers`
    // (task #32): inject Allow keys per pending card (reusing #18's seam),
    // check every outcome, THEN log the decision DISTINCTLY per Workspace —
    // only cards actually answered enter the audit trail.
    const action: HelmBulkAction = "allowAll";
    const plan = deriveBulkAnswerPlan(facts);
    const { note } = await executeBulkAnswers(api, plan, action);
    expect(note).toBeNull(); // every card injected — nothing to surface

    // One keyed answer per pending card, each targeting its own paused call…
    const answers = calls
      .filter(([cmd]) => cmd === "helm_answer_prompt")
      .map(([, args]) => (args?.input as AnswerPromptInput).toolUseId);
    expect(answers).toEqual(["toolu_a", "toolu_b"]);
    // …then one distinct bulk-log call per Workspace, after the answers.
    const logs = calls.filter(([cmd]) => cmd === "helm_record_bulk_decision");
    expect(logs).toEqual([
      ["helm_record_bulk_decision", { workspaceId: "wsA", action: "allowAll" }],
      ["helm_record_bulk_decision", { workspaceId: "wsB", action: "allowAll" }],
    ]);
    const lastAnswerAt = calls.reduce(
      (last, [cmd], i) => (cmd === "helm_answer_prompt" ? i : last),
      -1,
    );
    expect(
      calls.findIndex(([cmd]) => cmd === "helm_record_bulk_decision"),
    ).toBeGreaterThan(lastAnswerAt);
  });
});
