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

/** The card body, shaped by status. Blocked = the ask block (a
 * placeholder now; the approval ask renders fully at M3.5, the verify
 * badge at M6). Working/Idle = latest activity lines. Done = diffstat. */
export type CardBody =
  | { kind: "ask"; prompt: string; step: string | null; log: string | null }
  | { kind: "activity"; lines: string[] }
  | {
      kind: "diffstat";
      files: number;
      added: number;
      removed: number;
      verify: VerifyState;
    };

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
}

export interface WorkspaceCardView {
  workspaceId: string;
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
}

/** The card body for a derived status. Blocked bodies born from a failed
 * cut surface the failing step and its (hostile) log; other blocked
 * bodies are the approval placeholder until M3.5. */
export function deriveCardBody(
  status: HelmWorkspaceStatus,
  facts: CardBodyFacts,
): CardBody {
  switch (status) {
    case "blocked": {
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
        prompt: "waiting on you — approval renders at M3.5",
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
    const status = rollUpStatus(cutStatus, sessionStatuses);

    return {
      workspaceId: workspace.id,
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
