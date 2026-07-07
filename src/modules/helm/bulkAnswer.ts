// Helmsmen — the bulk Allow-all / Deny-all execution seam (task #19, made
// fail-safe under task #32).
//
// The ONE imperative composition behind the banner's bulk actions, kept out
// of the React shell so its safety ordering stays unit-tested against the
// real `createHelmApi` seam:
//
//   1. ANSWER every planned card through #18's verify-before-inject
//      `answer_prompt`, collecting each card's ACTUAL outcome — a `mismatch`
//      RESOLVES (it does not throw) and injected NOTHING, so the returned
//      value is checked per card, never discarded;
//   2. only then append the DISTINCT bulk-decision log, and only for
//      Workspaces where a fresh snapshot shows every still-pending ask is
//      one this run actually answered ([`canRecordBulkDecision`]) — the
//      backend records per still-open ask, Workspace-wide, so a partial
//      Workspace would falsely log its missed cards as decided;
//   3. distill the misses into the wall's answer note
//      ([`describeBulkOutcome`]) — a command that kept running is surfaced,
//      never silently reported as allowed/denied.
//
// Correlation stays strictly by tool_use_id; every plan item targets its own
// card's Session (see `deriveBulkAnswerPlan`).

import type { HelmApi, HelmBulkAction } from "./api";
import {
  type BulkAnswerItem,
  type BulkAnswerResult,
  canRecordBulkDecision,
  deriveApprovalAsks,
  describeBulkOutcome,
} from "./viewModel";

/** The slice of the Helm api a bulk run drives — injected so the whole
 * composition tests against a fake `invoke` (api.test.ts's pattern). */
export type BulkAnswerApi = Pick<
  HelmApi,
  "answerPrompt" | "approvalsSnapshot" | "recordBulkDecision"
>;

export interface BulkRunOutcome {
  /** Every planned card with what ACTUALLY happened to it. */
  results: BulkAnswerResult[];
  /** The wall's feedback line; null when every card injected cleanly. */
  note: string | null;
}

/** Run a bulk Allow-all / Deny-all plan: answer first, audit only what was
 * answered, surface every miss (see the module docs for why each step is
 * ordered this way). Never throws — every failure mode lands in `results` /
 * `note`, and an unverifiable audit state records nothing (fail safe). */
export async function executeBulkAnswers(
  api: BulkAnswerApi,
  plan: readonly BulkAnswerItem[],
  action: HelmBulkAction,
): Promise<BulkRunOutcome> {
  const answer = action === "allowAll" ? "allow" : "deny";

  // 1. Answer every card, collecting the per-card outcome.
  const results: BulkAnswerResult[] = [];
  for (const item of plan) {
    if (!item.agentSession) {
      results.push({ item, outcome: "noAgent" });
      continue;
    }
    try {
      const outcome = await api.answerPrompt({
        session: item.agentSession.sessionId,
        runtime: item.agentSession.runtime,
        toolUseId: item.toolUseId,
        expectedCommand: item.expectedCommand,
        action: answer,
      });
      results.push({
        item,
        outcome: outcome.status === "injected" ? "injected" : "mismatch",
      });
    } catch {
      // A single unreachable agent skips its card; the rest proceed.
      results.push({ item, outcome: "unreachable" });
    }
  }

  // 2. The distinct bulk log, AFTER the answers (task #32 — it was logged
  // before them, reporting never-answered cards as decided). Gated per
  // Workspace on a FRESH snapshot: record only when everything still pending
  // is something this run injected.
  const answeredByWorkspace = new Map<string, string[]>();
  for (const r of results) {
    if (r.outcome !== "injected") continue;
    const ids = answeredByWorkspace.get(r.item.workspaceId) ?? [];
    ids.push(r.item.askId);
    answeredByWorkspace.set(r.item.workspaceId, ids);
  }
  for (const [workspaceId, answeredIds] of answeredByWorkspace) {
    try {
      const state = await api.approvalsSnapshot(workspaceId);
      if (state === null) continue; // no endpoint → nothing verifiable to log
      const pendingIds = deriveApprovalAsks(state.cards).map((a) => a.id);
      if (!canRecordBulkDecision(pendingIds, answeredIds)) continue;
      await api.recordBulkDecision(workspaceId, action);
    } catch {
      // A transient log failure never blocks the decisions themselves; a
      // missing bulk record beats a false one.
    }
  }

  // 3. Distill the misses for the wall's answer note.
  return { results, note: describeBulkOutcome(results) };
}
