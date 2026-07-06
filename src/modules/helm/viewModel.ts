// Helmsmen — pure view-model derivation for the Helm wall (task #10).
//
// The wall is a pure projection of registry data: given Projects,
// Workspaces (with their cut lifecycle), Profiles, and host-supplied
// facts (Session snapshots + timestamps), it derives every card and the
// header counts. Nothing here reaches for a clock, a process, or the DOM
// — `nowMs` and Session facts arrive as data so the derivation stays
// deterministic under test. The React shell (Helm.tsx) only renders what
// these functions return.
//
// Status is a derived rollup, never stored: it extends the existing
// `deriveWorkspaceStatus` seam (the cut-derived status) with the Session
// rollup the PRD specifies, mirroring the pure core in
// `core::cut::roll_up_status`.

import {
  deriveWorkspaceStatus,
  HELM_STATUS_ALIAS,
  type HelmAnswerOutcome,
  type HelmApproval,
  type HelmCutState,
  type HelmCutStep,
  type HelmProfile,
  type HelmProject,
  type HelmWorkspace,
  type HelmWorkspaceStatus,
} from "./api";

/** The wall's rank order: Blocked 0 -> Done 1 -> Working 2 -> Idle 3 —
 * the canonical attention order (Needs you, To review, Working, Idle),
 * applied as a sort across all Projects, not as sections. */
export const STATUS_RANK: Record<HelmWorkspaceStatus, number> = {
  blocked: 0,
  done: 1,
  working: 2,
  idle: 3,
};

/** Neutral card color when no Profile resolves for a Workspace. */
export const DEFAULT_PROFILE_COLOR = "#8b93a7";

/** A Session as far as the wall is concerned: enough to render a chip and
 * feed the status rollup. Populated by later slices (#11/#13); the pure
 * derivation only ever reads it. */
export type SessionKind = "agent" | "shell" | "process" | "reviewer";

export interface SessionFacts {
  /** Zoom target — the chip navigates straight to this Session (#12). */
  sessionId: string;
  kind: SessionKind;
  /** Transport, e.g. `tmux` / `pty`. */
  runtime: string;
  /** Agent harness token for the chip, e.g. `claude` / `codex`. */
  harness?: string;
  /** Process Session name, e.g. `dev`. */
  processName?: string;
  /** Process Session port, e.g. `5173`. */
  port?: number;
  /** Per-Session status feeding the rollup (absent until #11 wires it). */
  status?: HelmWorkspaceStatus;
}

export interface SessionChipView {
  sessionId: string;
  kind: SessionKind;
  label: string;
  status: HelmWorkspaceStatus | null;
}

export type VerifyState = "passed" | "none" | "unknown";

/** One paused approval, distilled for the ask block on the card (task #17):
 * the tool, the rule that fired, and the exact command. Every field is
 * hostile agent text — the card renders it via escaped JSX only. */
export interface ApprovalAskView {
  /** Stable card id (the correlation anchor; the answer path keys on it). */
  id: string;
  /** The tool that paused, e.g. `Bash`. */
  tool: string;
  /** The human label of the risk rule that fired. */
  rule: string;
  /** The exact command (or file path) the decision was made on. */
  command: string;
}

/** The card body, shaped by status. Blocked = the ask block: the M3.5
 * approval ask(s) when a risky call is paused, else the cut-failure /
 * placeholder ask. Working/Idle = latest activity lines. Done = diffstat
 * (verify badge at M6). */
export type CardBody =
  | { kind: "ask"; prompt: string; step: string | null; log: string | null }
  | { kind: "approval"; asks: ApprovalAskView[] }
  | { kind: "activity"; lines: string[] }
  | {
      kind: "diffstat";
      files: number;
      added: number;
      removed: number;
      verify: VerifyState;
    };

/** A still-open policy `ask` card: a risky call paused, awaiting a decision.
 * `allow`/`deny` cards are the audit trail, never asks; a resolved card
 * (allowed/closed) has left the queue. The one predicate the ask block, the
 * bulk banner, and the bulk answer plan all pause on — kept single-sourced so
 * the on-card and banner views can never disagree on what is pending. */
function isPendingAsk(a: HelmApproval): boolean {
  return (
    a.decision === "ask" &&
    (a.status === "pending" || a.status === "surfaced")
  );
}

/** The still-open approvals a permission answer is waiting on: only the
 * policy `ask` cards (allow/deny are the audit trail, never ask blocks) that
 * have not yet resolved (pending/surfaced). Pure and total; safe on an
 * absent/empty list. Kept in incoming (seq) order. */
export function deriveApprovalAsks(
  approvals: HelmApproval[] | undefined,
): ApprovalAskView[] {
  if (!approvals) return [];
  return approvals.filter(isPendingAsk).map((a) => ({
    id: a.id,
    tool: a.toolName,
    rule: a.rule?.label ?? "risk-list rule",
    command: a.input.command ?? a.input.filePath ?? "",
  }));
}

/** Host-supplied facts a Workspace card needs beyond the core entity
 * (which carries no timestamps or Session list at M2). Everything is
 * data so the derivation stays deterministic. */
export interface WorkspaceFacts {
  /** Epoch ms the elapsed clock counts from (e.g. the cut's start). */
  startedAtMs?: number;
  /** Which Profile drives this Workspace's color. */
  profileId?: string;
  sessions?: SessionFacts[];
  activityLines?: string[];
  diffstat?: { files: number; added: number; removed: number };
  verify?: VerifyState;
  /** The Workspace's control-plane approval cards (task #17). A pending
   * `ask` among them pauses the Workspace: it forces Blocked and renders the
   * ask block. Snapshot of `EndpointRegistry::snapshot(workspaceId)`. */
  approvals?: HelmApproval[];
}

export interface WorkspaceCardView {
  workspaceId: string;
  /** Owning Project id — the stable key the repo picker scopes on and
   * Project grouping partitions by (names can collide; ids never). */
  projectId: string;
  branch: string;
  projectName: string;
  baseBranch: string;
  status: HelmWorkspaceStatus;
  statusLabel: string;
  rank: number;
  profileColor: string;
  elapsedMinutes: number;
  /** Attention rule: Working dots never pulse (pulse is an M5 setting,
   * shipped Off). Always `false` at this milestone. */
  pulse: false;
  body: CardBody;
  chips: SessionChipView[];
}

export interface HeaderCounts {
  needsYou: number;
  working: number;
  toReview: number;
  /** The header turns red when there is anything waiting on the user. */
  needsAttention: boolean;
}

export interface WallView {
  /** Rank-sorted across all Projects (flat wall). */
  cards: WorkspaceCardView[];
  counts: HeaderCounts;
}

export interface WallInput {
  projects: HelmProject[];
  workspaces: HelmWorkspace[];
  profiles: HelmProfile[];
  facts?: Record<string, WorkspaceFacts>;
  /** Epoch ms; the elapsed clock reads from here, never `Date.now()`. */
  nowMs: number;
}

const CUT_STEP_LABEL: Record<HelmCutStep, string> = {
  fetch: "fetch",
  worktreeAdd: "worktree add",
  authorizeRoot: "authorize workspace root",
  copyCarryOvers: "copy carry-overs",
  setupScript: "setup script",
  harnessWiring: "harness wiring",
  launchSession: "launch first session",
};

/** Roll a Workspace's status up from its Sessions, per the PRD rule: any
 * Session blocked -> Blocked; else any working -> Working; else all done
 * -> Done; else Idle. A failed cut (`cutStatus === "blocked"`) parks the
 * Workspace as Blocked regardless of Sessions; with no Sessions the
 * cut-derived status stands (M2: the cut is the only status source). */
export function rollUpStatus(
  cutStatus: HelmWorkspaceStatus,
  sessionStatuses: HelmWorkspaceStatus[],
): HelmWorkspaceStatus {
  if (cutStatus === "blocked") return "blocked";
  if (sessionStatuses.length === 0) return cutStatus;
  if (sessionStatuses.includes("blocked")) return "blocked";
  if (sessionStatuses.includes("working")) return "working";
  if (sessionStatuses.every((s) => s === "done")) return "done";
  return "idle";
}

/** The Session chip label: `claude·tmux` / `codex·pty` for agents,
 * `dev:5173` for Processes, `shell`, `reviewer`. */
export function sessionChipLabel(session: SessionFacts): string {
  switch (session.kind) {
    case "agent":
      return `${session.harness ?? "agent"}·${session.runtime}`;
    case "shell":
      return "shell";
    case "process":
      if (session.processName && session.port != null) {
        return `${session.processName}:${session.port}`;
      }
      return session.processName ?? "process";
    case "reviewer":
      return "reviewer";
  }
}

/** Whole elapsed minutes between two timestamps, clamped to zero. */
export function elapsedMinutes(nowMs: number, startedAtMs: number): number {
  return Math.max(0, Math.floor((nowMs - startedAtMs) / 60_000));
}

interface CardBodyFacts {
  cut: HelmCutState;
  activityLines?: string[];
  diffstat?: { files: number; added: number; removed: number };
  verify?: VerifyState;
  /** Still-open approval asks (from [`deriveApprovalAsks`]). A non-empty
   * list turns a Blocked body into the approval ask block. */
  approvalAsks?: ApprovalAskView[];
}

/** The card body for a derived status. A Blocked body prefers the approval
 * ask block (a risky call paused, task #17); else a failed cut surfaces the
 * failing step and its (hostile) log; else the waiting placeholder. */
export function deriveCardBody(
  status: HelmWorkspaceStatus,
  facts: CardBodyFacts,
): CardBody {
  switch (status) {
    case "blocked": {
      if (facts.approvalAsks && facts.approvalAsks.length > 0) {
        return { kind: "approval", asks: facts.approvalAsks };
      }
      if (facts.cut.phase === "failed") {
        const step = CUT_STEP_LABEL[facts.cut.step];
        return {
          kind: "ask",
          prompt: `cut failed at ${step} — needs you`,
          step,
          log: facts.cut.log,
        };
      }
      return {
        kind: "ask",
        prompt: "waiting on you",
        step: null,
        log: null,
      };
    }
    case "working": {
      const lines =
        facts.activityLines && facts.activityLines.length > 0
          ? facts.activityLines
          : [facts.cut.phase === "cutting" ? "cutting workspace…" : "working…"];
      return { kind: "activity", lines };
    }
    case "idle": {
      const lines =
        facts.activityLines && facts.activityLines.length > 0
          ? facts.activityLines
          : ["session ready · waiting for a prompt"];
      return { kind: "activity", lines };
    }
    case "done": {
      const d = facts.diffstat ?? { files: 0, added: 0, removed: 0 };
      return {
        kind: "diffstat",
        files: d.files,
        added: d.added,
        removed: d.removed,
        verify: facts.verify ?? "unknown",
      };
    }
  }
}

/** Header counts + the red rule. Idle Workspaces are intentionally not
 * surfaced in the header copy. */
export function deriveHeaderCounts(
  statuses: HelmWorkspaceStatus[],
): HeaderCounts {
  let needsYou = 0;
  let working = 0;
  let toReview = 0;
  for (const s of statuses) {
    if (s === "blocked") needsYou++;
    else if (s === "working") working++;
    else if (s === "done") toReview++;
  }
  return { needsYou, working, toReview, needsAttention: needsYou > 0 };
}

/** Stable rank sort (does not mutate its input). */
export function rankSort<T extends { rank: number }>(cards: T[]): T[] {
  return [...cards].sort((a, b) => a.rank - b.rank);
}

function resolveProfileColor(
  profiles: HelmProfile[],
  projectId: string,
  profileId: string | undefined,
): string {
  if (profileId) {
    const exact = profiles.find((p) => p.id === profileId);
    if (exact) return exact.color;
  }
  const seeded = profiles.find((p) => p.projectId === projectId);
  return seeded?.color ?? DEFAULT_PROFILE_COLOR;
}

/** Build the whole wall view-model: one card per Workspace across all
 * Projects, rank-sorted, plus the header counts. */
export function buildWall(input: WallInput): WallView {
  const { projects, workspaces, profiles, nowMs } = input;
  const facts = input.facts ?? {};
  const projectById = new Map(projects.map((p) => [p.id, p]));

  const cards: WorkspaceCardView[] = workspaces.map((workspace) => {
    const f = facts[workspace.id] ?? {};
    const project = projectById.get(workspace.projectId);
    const sessions = f.sessions ?? [];

    const cutStatus = deriveWorkspaceStatus(workspace);
    const sessionStatuses = sessions
      .map((s) => s.status)
      .filter((s): s is HelmWorkspaceStatus => s != null);
    const rolledUp = rollUpStatus(cutStatus, sessionStatuses);

    // A paused approval (a risk-list `ask`) blocks the Workspace regardless of
    // the rolled-up session status — the agent is waiting on the decision.
    const approvalAsks = deriveApprovalAsks(f.approvals);
    const status: HelmWorkspaceStatus =
      approvalAsks.length > 0 ? "blocked" : rolledUp;

    return {
      workspaceId: workspace.id,
      projectId: workspace.projectId,
      branch: workspace.branch,
      projectName: project?.name ?? workspace.projectId,
      baseBranch: project?.baseBranch ?? "",
      status,
      statusLabel: HELM_STATUS_ALIAS[status],
      rank: STATUS_RANK[status],
      profileColor: resolveProfileColor(
        profiles,
        workspace.projectId,
        f.profileId,
      ),
      elapsedMinutes:
        f.startedAtMs != null ? elapsedMinutes(nowMs, f.startedAtMs) : 0,
      pulse: false,
      body: deriveCardBody(status, {
        cut: workspace.cut,
        activityLines: f.activityLines,
        diffstat: f.diffstat,
        verify: f.verify,
        approvalAsks,
      }),
      chips: sessions.map((s) => ({
        sessionId: s.sessionId,
        kind: s.kind,
        label: sessionChipLabel(s),
        status: s.status ?? null,
      })),
    };
  });

  const sorted = rankSort(cards);
  return { cards: sorted, counts: deriveHeaderCounts(sorted.map((c) => c.status)) };
}

// === the bulk-approvals banner (task #19) ===
//
// The Approval Inbox, rendered as a banner between the toolbar and the grid
// when MORE THAN ONE approval is pending across the whole wall. Per-approval
// decisions stay on the cards (#18); the banner hosts BULK actions only, and
// its count + one-line preview are PURE derivations over the same pending
// queue the cards read (`isPendingAsk`) — so the on-card asks and the banner
// can never disagree. Every preview string is hostile agent text; the banner
// renders it via escaped JSX only.

/** One line of the banner: a single pending ask with its Workspace context,
 * distilled for the one-line preview. Every string is hostile agent text —
 * render via escaped JSX only. */
export interface BulkApprovalPreview {
  /** The card id (stable correlation anchor; matches the on-card ask). */
  id: string;
  workspaceId: string;
  projectName: string;
  branch: string;
  tool: string;
  rule: string;
  command: string;
}

/** The bulk-approvals banner model: the live pending-ask count, whether the
 * banner shows (STRICTLY more than one pending — exactly one is the on-card
 * ask block alone), and a one-line preview of every pending ask. */
export interface BulkApprovalsView {
  count: number;
  /** Banner rule: render only when `count > 1`. */
  visible: boolean;
  previews: BulkApprovalPreview[];
}

/** Derive the bulk-approvals banner from the (rank-sorted) wall cards. Reads
 * each card's already-derived approval ask block (`deriveApprovalAsks`, folded
 * into the card body by `buildWall`), so the banner's count/preview are exactly
 * the wall's pending asks, in wall (rank) order — Blocked cards float to the
 * top, so the most urgent asks preview first. Pure and total. */
export function deriveBulkApprovals(
  cards: WorkspaceCardView[],
): BulkApprovalsView {
  const previews: BulkApprovalPreview[] = [];
  for (const card of cards) {
    if (card.body.kind !== "approval") continue;
    for (const ask of card.body.asks) {
      previews.push({
        id: ask.id,
        workspaceId: card.workspaceId,
        projectName: card.projectName,
        branch: card.branch,
        tool: ask.tool,
        rule: ask.rule,
        command: ask.command,
      });
    }
  }
  return { count: previews.length, visible: previews.length > 1, previews };
}

/** One target of a bulk decision: the agent Session keys inject into (the
 * Workspace's interactive agent, null if none is live to answer), the card's
 * correlation anchor, and the exact command the answer seam verifies is on
 * screen before injecting. */
export interface BulkAnswerItem {
  workspaceId: string;
  /** The Session the keys inject into — the Workspace's Agent Session, never a
   * shell/process. Null when no agent Session is live to receive keys. */
  agentSession: { sessionId: string; runtime: string } | null;
  toolUseId: string | null;
  expectedCommand: string;
}

/** Plan a bulk Allow-all / Deny-all: one [`BulkAnswerItem`] per pending ask
 * across every Workspace, resolved from the host facts (each Workspace's
 * approval cards + its live Sessions). The shell iterates this applying #18's
 * `answer_prompt` per card; correlation stays strictly by `tool_use_id`, so a
 * bulk answer resumes each paused call exactly where it paused. Pure and
 * total. */
export function deriveBulkAnswerPlan(
  facts: Record<string, WorkspaceFacts>,
): BulkAnswerItem[] {
  const plan: BulkAnswerItem[] = [];
  for (const [workspaceId, f] of Object.entries(facts)) {
    const asks = (f.approvals ?? []).filter(isPendingAsk);
    if (asks.length === 0) continue;
    const agent = (f.sessions ?? []).find((s) => s.kind === "agent");
    const agentSession = agent
      ? { sessionId: agent.sessionId, runtime: agent.runtime }
      : null;
    for (const a of asks) {
      plan.push({
        workspaceId,
        agentSession,
        toolUseId: a.toolUseId,
        expectedCommand: a.input.command ?? a.input.filePath ?? "",
      });
    }
  }
  return plan;
}

/** The two-press confirm guarding Allow-all specifically (Deny-all is a single
 * action). Given the current armed flag, the next press either ARMS the
 * confirm (first press) or FIRES it (second press) and disarms. Pure state
 * transition so the banner button and the `A` key share one contract. */
export function nextAllowAllConfirm(armed: boolean): {
  armed: boolean;
  fire: boolean;
} {
  return armed ? { armed: false, fire: true } : { armed: true, fire: false };
}

// === the Helm's scoping controls (task #14) ===
//
// The wall stays one flat, rank-sorted projection (`buildWall` above); these
// are the pure lenses over it that `f` (filter), `g` (group), and `r` (repo
// picker) drive. Every function is a total transform of already-derived
// cards — no clock, no DOM, no fetch — so the Helm's whole scoping contract
// is unit-tested in a repo with no DOM test env; Helm.tsx only holds the
// active filter/group/scope state and renders what these return.

// --- status filter tabs (`f`) ---

/** A wall status filter. `all` shows every card; the rest narrow to one
 * derived status. `f` cycles them in the spec's tab order. */
export type WallFilter = "all" | HelmWorkspaceStatus;

/** `f`'s cycle order, matching the spec's tab strip: All → Needs you →
 * Working → To review → Idle → (wrap). */
export const FILTER_CYCLE: readonly WallFilter[] = [
  "all",
  "blocked",
  "working",
  "done",
  "idle",
];

/** Tab labels (aliases per the PRD: Blocked = "Needs you", Done = "To
 * review"); the All tab has no status alias of its own. */
const FILTER_LABEL: Record<WallFilter, string> = {
  all: "All",
  ...HELM_STATUS_ALIAS,
};

/** Next filter for an `f` press, wrapping. An unrecognized current value
 * restarts at `all` (defensive: `indexOf` → -1 → 0). */
export function cycleFilter(current: WallFilter): WallFilter {
  const at = FILTER_CYCLE.indexOf(current);
  return FILTER_CYCLE[(at + 1) % FILTER_CYCLE.length] ?? "all";
}

/** Does a card's status pass a filter? `all` passes everything. */
export function cardMatchesFilter(
  status: HelmWorkspaceStatus,
  filter: WallFilter,
): boolean {
  return filter === "all" || status === filter;
}

/** Keep only the cards passing the active filter, in their incoming (rank)
 * order. Returns the same reference for `all` (nothing to narrow). */
export function filterCards(
  cards: WorkspaceCardView[],
  filter: WallFilter,
): WorkspaceCardView[] {
  return filter === "all"
    ? cards
    : cards.filter((c) => cardMatchesFilter(c.status, filter));
}

/** One status filter tab: label, live count, the status color the dot
 * shows (null for All), and whether that dot is dimmed (count 0). */
export interface FilterTabView {
  filter: WallFilter;
  label: string;
  count: number;
  active: boolean;
  /** The status whose color the dot renders; null for the All tab. */
  status: HelmWorkspaceStatus | null;
  /** Dimmed (30% opacity) when the tab's count is 0. All never dims. */
  dimmed: boolean;
}

/** Build the filter-tab strip from a set of cards: All + one tab per
 * status, each with its live count and dimmed-when-empty dot, and which
 * one is active. Counts reflect exactly the cards passed in (the scoped
 * set), so the tabs stay correct live as statuses and scope change. */
export function deriveFilterTabs(
  cards: WorkspaceCardView[],
  active: WallFilter,
): FilterTabView[] {
  return FILTER_CYCLE.map((filter) => {
    const status = filter === "all" ? null : filter;
    const count =
      filter === "all"
        ? cards.length
        : cards.filter((c) => c.status === filter).length;
    return {
      filter,
      label: FILTER_LABEL[filter],
      count,
      active: filter === active,
      status,
      dimmed: filter !== "all" && count === 0,
    };
  });
}

// --- grouping (`g`) ---

/** How the wall is arranged. `flat` = one rank-sorted grid; `project` =
 * per-Project sections. `ship` is M8 (Fleet) — deliberately absent. */
export type GroupMode = "flat" | "project";

/** `g`'s toggle order: flat ⇄ project. */
export const GROUP_CYCLE: readonly GroupMode[] = ["flat", "project"];

/** Next grouping for a `g` press, wrapping (flat ⇄ project). */
export function cycleGroup(current: GroupMode): GroupMode {
  const at = GROUP_CYCLE.indexOf(current);
  return GROUP_CYCLE[(at + 1) % GROUP_CYCLE.length] ?? "flat";
}

/** A Project group's header: repo name, live count, base branch —
 * rendered as `{repo} (n) ⎇ {base}`. Null for the single flat group. */
export interface GroupHeader {
  projectId: string;
  repoName: string;
  count: number;
  baseBranch: string;
}

export interface WallGroup {
  /** Stable React key + scope key. `"__flat__"` for the flat group. */
  key: string;
  header: GroupHeader | null;
  cards: WorkspaceCardView[];
}

/** The flat group's sentinel key (no Project id can collide: ids are
 * registry-issued, never this literal). */
export const FLAT_GROUP_KEY = "__flat__";

/** Group cards for the wall. `flat` returns a single headerless group with
 * the cards untouched (already rank-sorted). `project` partitions by
 * Project: groups ordered by the `projects` input (registry order) then
 * any leftover Projects in first-appearance order, each headed by repo
 * name / count / base branch, cards within a group kept in incoming rank
 * order. A Project with no cards is omitted (nothing to head). */
export function groupCards(
  cards: WorkspaceCardView[],
  projects: HelmProject[],
  mode: GroupMode,
): WallGroup[] {
  if (mode === "flat") {
    return [{ key: FLAT_GROUP_KEY, header: null, cards }];
  }

  const byProject = new Map<string, WorkspaceCardView[]>();
  for (const card of cards) {
    const list = byProject.get(card.projectId);
    if (list) list.push(card);
    else byProject.set(card.projectId, [card]);
  }

  const groups: WallGroup[] = [];
  const emitted = new Set<string>();
  const emit = (
    projectId: string,
    repoName: string,
    baseBranch: string,
    groupCardList: WorkspaceCardView[],
  ) => {
    emitted.add(projectId);
    groups.push({
      key: projectId,
      header: {
        projectId,
        repoName,
        count: groupCardList.length,
        baseBranch,
      },
      cards: groupCardList,
    });
  };

  // Registry order first, so the wall's Projects list stays stable.
  for (const project of projects) {
    const list = byProject.get(project.id);
    if (list) emit(project.id, project.name, project.baseBranch, list);
  }
  // Any cards whose Project is not in `projects` (defensive) trail in
  // first-appearance order, headed from the card's own facts.
  for (const [projectId, list] of byProject) {
    if (emitted.has(projectId)) continue;
    emit(projectId, list[0].projectName, list[0].baseBranch, list);
  }
  return groups;
}

// --- repo picker (`r`) ---

/** One row of the repo picker: the Project, its live Workspace count, and
 * the worst (most attention-needing) status among them for its dot. */
export interface RepoPickerEntry {
  projectId: string;
  name: string;
  baseBranch: string;
  /** Live count of this Project's Workspaces (cards). */
  count: number;
  /** Worst status across this Project's cards (min STATUS_RANK); null when
   * the Project has no Workspaces. */
  worstStatus: HelmWorkspaceStatus | null;
}

export interface RepoPickerView {
  /** Total Workspaces across all Projects — the picker's "All repos · N
   * active" line. */
  allActive: number;
  entries: RepoPickerEntry[];
}

/** The worst (most attention-needing) status of a set: the minimum
 * STATUS_RANK (Blocked ≺ Done ≺ Working ≺ Idle). Null for an empty set.
 * This is the dot a repo shows so its most urgent Workspace surfaces at
 * the repo level. */
export function worstStatus(
  statuses: HelmWorkspaceStatus[],
): HelmWorkspaceStatus | null {
  let worst: HelmWorkspaceStatus | null = null;
  for (const s of statuses) {
    if (worst === null || STATUS_RANK[s] < STATUS_RANK[worst]) worst = s;
  }
  return worst;
}

/** Build the repo-picker model over the FULL (unscoped, unfiltered) card
 * set, so each repo shows its own live count + worst-status dot regardless
 * of the active status filter or current scope. Every Project is listed in
 * registry order (a Project with no Workspaces shows count 0 / no dot), so
 * the wall can be scoped to any Project. */
export function deriveRepoPicker(
  cards: WorkspaceCardView[],
  projects: HelmProject[],
): RepoPickerView {
  const byProject = new Map<string, WorkspaceCardView[]>();
  for (const card of cards) {
    const list = byProject.get(card.projectId);
    if (list) list.push(card);
    else byProject.set(card.projectId, [card]);
  }
  const entries = projects.map((project) => {
    const list = byProject.get(project.id) ?? [];
    return {
      projectId: project.id,
      name: project.name,
      baseBranch: project.baseBranch,
      count: list.length,
      worstStatus: worstStatus(list.map((c) => c.status)),
    };
  });
  return { allActive: cards.length, entries };
}

/** Scope the wall to one Project (a repo-picker pick); `null` = all repos.
 * Returns the same reference when unscoped. */
export function applyScope(
  cards: WorkspaceCardView[],
  scopeProjectId: string | null,
): WorkspaceCardView[] {
  return scopeProjectId === null
    ? cards
    : cards.filter((c) => c.projectId === scopeProjectId);
}

// --- wall keyboard contract (`f` / `g` / `r`, pure key → action) ---

/** What a wall key press means. `none` falls through to the shell / a
 * focused field. `clear-filters` is `esc` on the wall (reset scope +
 * status filter to show everything). */
export type HelmWallAction =
  | { kind: "cycle-filter" }
  | { kind: "cycle-group" }
  | { kind: "open-repo-picker" }
  | { kind: "close-repo-picker" }
  | { kind: "clear-filters" }
  | { kind: "answer-allow" }
  | { kind: "answer-deny" }
  | { kind: "bulk-allow-all" }
  | { kind: "bulk-deny-all" }
  | { kind: "none" };

/** The subset of a KeyboardEvent the map reads (plain data, so tests need
 * no synthetic DOM events). */
export interface HelmWallKeyInput {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

export interface HelmWallKeyContext {
  /** An editable field holds focus (repo-picker filter box, command bar):
   * the letter keys must type, so no wall action fires. */
  editing: boolean;
  /** An overlay (Zoom / New Workspace) owns the keyboard — the wall keys
   * yield entirely (Zoom handles its own `esc`). */
  overlayActive: boolean;
  /** The repo picker dropdown is open: it captures the wall keys (`r` /
   * `esc` close it), so `f`/`g` don't cycle behind it. */
  pickerOpen: boolean;
}

/** Map a wall key press to an action. Modified chords (Ctrl/Meta/Alt),
 * every key while a field is focused, and every key while an overlay is
 * open are left alone. While the repo picker is open it owns the keys
 * (`r`/`esc` close it; everything else is inert). `a`/`x` answer the top
 * visible Blocked card's paused approval (Allow / Deny) — the wall resolves
 * which card ([`deriveWallAnswerTarget`]) and injects nothing (the press
 * falls through) if none is answerable. `A`/`X`
 * (shift) are the bulk banner's Allow-all / Deny-all; `A` two-presses to
 * confirm (the container arms it, and only acts while the banner shows).
 * Shift is not a chord here — the letter keys are already single-letter — so
 * only Ctrl/Meta/Alt yield to the browser. */
export function mapHelmWallKey(
  ev: HelmWallKeyInput,
  ctx: HelmWallKeyContext,
): HelmWallAction {
  if (ctx.editing || ctx.overlayActive) return { kind: "none" };
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return { kind: "none" };

  if (ctx.pickerOpen) {
    return ev.key === "Escape" || ev.key === "r"
      ? { kind: "close-repo-picker" }
      : { kind: "none" };
  }

  switch (ev.key) {
    case "f":
      return { kind: "cycle-filter" };
    case "g":
      return { kind: "cycle-group" };
    case "r":
      return { kind: "open-repo-picker" };
    case "a":
      return { kind: "answer-allow" };
    case "x":
      return { kind: "answer-deny" };
    case "A":
      return { kind: "bulk-allow-all" };
    case "X":
      return { kind: "bulk-deny-all" };
    case "Escape":
      return { kind: "clear-filters" };
    default:
      return { kind: "none" };
  }
}

// === per-card Allow/Deny from the wall (`a`/`x`, task #34) ===
//
// The wall counterpart of the zoom's answer path: `a`/`x` answer ONE paused
// approval through #18's verify-before-inject seam. These are the pure halves
// Helm.tsx / HelmView.tsx compose: which card the key acts on (over the cards
// the user can SEE — the scoped/filtered set), and that ask resolved to its
// live agent Session + correlation anchor. Both fail safe: an unanswerable
// wall yields null and the shell injects nothing.

/** The card a wall `a`/`x` press acts on: the Workspace plus the exact ask
 * card id (the correlation anchor the shell resolves back to a live agent
 * Session + tool_use_id via [`deriveCardAnswerItem`]). */
export interface WallAnswerTarget {
  workspaceId: string;
  askId: string;
}

/** Resolve which card the wall's `a`/`x` answers: the FIRST card in the given
 * (visible, rank-ordered) list showing an approval ask block, taking its first
 * pending ask — Blocked cards float to the top, so this is the wall's most
 * urgent ask, matching the bulk banner's preview order. Null when nothing
 * visible is answerable (no approval ask block — a cut-failed Blocked card
 * does not count): the key press must stay a no-op and inject nothing. */
export function deriveWallAnswerTarget(
  cards: WorkspaceCardView[],
): WallAnswerTarget | null {
  for (const card of cards) {
    if (card.body.kind === "approval" && card.body.asks.length > 0) {
      return { workspaceId: card.workspaceId, askId: card.body.asks[0].id };
    }
  }
  return null;
}

/** One per-card answer resolved from the host facts: the agent Session the
 * keys inject into (null when none is live to receive them) and the paused
 * call's correlation anchor + exact command the answer seam verifies is on
 * screen before injecting. Mirrors the bulk plan's Session resolution for a
 * single ask. */
export interface CardAnswerItem {
  workspaceId: string;
  agentSession: { sessionId: string; runtime: string } | null;
  toolUseId: string | null;
  expectedCommand: string;
}

/** Resolve one [`WallAnswerTarget`] against the live facts. Null when the ask
 * is no longer pending (it resolved between the key press and now) or the
 * Workspace/ask is unknown — a stale press answers NOTHING rather than the
 * wrong card (fail safe; verify-before-inject then guards the send itself). */
export function deriveCardAnswerItem(
  facts: Record<string, WorkspaceFacts>,
  target: WallAnswerTarget,
): CardAnswerItem | null {
  const f = facts[target.workspaceId];
  if (!f) return null;
  const ask = (f.approvals ?? []).find(
    (a) => a.id === target.askId && isPendingAsk(a),
  );
  if (!ask) return null;
  const agent = (f.sessions ?? []).find((s) => s.kind === "agent");
  return {
    workspaceId: target.workspaceId,
    agentSession: agent
      ? { sessionId: agent.sessionId, runtime: agent.runtime }
      : null,
    toolUseId: ask.toolUseId,
    expectedCommand: ask.input.command ?? ask.input.filePath ?? "",
  };
}

/** The wall's feedback line for a per-card answer outcome. `injected` needs
 * no note (the approvals poll resolves the card); a `mismatch` means the
 * backend verified the visible dialog was NOT this card's and injected
 * NOTHING — that must reach the user, never be discarded. */
export function describeAnswerOutcome(
  outcome: HelmAnswerOutcome,
): string | null {
  return outcome.status === "mismatch"
    ? "dialog changed — not answered; re-check the call"
    : null;
}
