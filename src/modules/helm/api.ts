// Helmsmen — helm module (new module per docs/fork-posture.md).
//
// Typed frontend seam for the Helmsmen registry commands. Pure argument
// mapping only: the factory takes an `invoke` function so the module has
// no import-time Tauri dependency and is unit-testable without a webview.
// All real validation happens in the backend (boundary + pure core).

export interface HelmProject {
  id: string;
  name: string;
  repoRoot: string;
  baseBranch: string;
  worktreeHome: string;
  branchTemplate: string;
}

/** Prefill computed by the backend for a picked clone; every field is
 * editable before `addProject` commits it to the registry. */
export interface HelmProjectDetection {
  repoRoot: string;
  name: string;
  baseBranch: string;
  worktreeHome: string;
  branchTemplate: string;
}

export interface AddProjectInput {
  repoRoot: string;
  name?: string;
  baseBranch: string;
  worktreeHome: string;
  branchTemplate: string;
}

/** One task: one git worktree on its own branch under a Project. */
export interface HelmWorkspace {
  id: string;
  projectId: string;
  slug: string;
  branch: string;
  worktreePath: string;
  slot: number;
}

/** What a cut returns: the live Workspace plus the assembled `HELMSMEN_*`
 * env every later pipeline step spawns with. */
export interface HelmCutWorkspace {
  workspace: HelmWorkspace;
  env: Record<string, string>;
}

/** Cap set a Harness declares in code (backend `harness::Caps`); a
 * missing Cap switches off its UI surface, never the architecture. */
export interface HelmCaps {
  resume: boolean;
  controlPlaneHooks: boolean;
  agentSignal: boolean;
  costTelemetry: boolean;
  mcpConfig: boolean;
  modelSelect: boolean;
}

export interface HelmHarness {
  id: string;
  displayName: string;
  caps: HelmCaps;
}

/** Opaque handle for a spawned Agent Session; echo `runtime` +
 * `sessionId` back on every session operation. */
export interface HelmAgentSession {
  sessionId: string;
  runtime: string;
  harnessId: string;
  workspaceId: string;
}

export type HelmSessionStatus =
  | { state: "running" }
  | { state: "exited"; code: number };

/** Stream callbacks for a session. Output is hostile PTY data: treat it
 * as text, never as markup or instructions. */
export interface AgentStreamHandlers {
  onData?: (bytes: Uint8Array) => void;
  onExit?: (code: number) => void;
}

export interface SpawnAgentOptions extends AgentStreamHandlers {
  harnessId?: string;
  runtime?: string;
  cols?: number;
  rows?: number;
}

export type InvokeFn = <T>(
  cmd: string,
  args?: Record<string, unknown>,
) => Promise<T>;

/** Builds a Tauri Channel-like object delivering messages to `onMessage`.
 * Injected (like `invoke`) so the module stays unit-testable without a
 * webview. */
export type ChannelFactory = <T>(onMessage: (message: T) => void) => unknown;

export function createHelmApi(invoke: InvokeFn, makeChannel?: ChannelFactory) {
  const detectProject = (path: string) =>
    invoke<HelmProjectDetection>("helm_detect_project", { path });

  const addProject = (input: AddProjectInput) =>
    invoke<HelmProject>("helm_add_project", { input });

  const listProjects = () => invoke<HelmProject[]>("helm_list_projects");

  /** Detect a clone's prefill, apply the user's edits on top, add it. */
  const addProjectFromPath = async (
    path: string,
    overrides: Partial<AddProjectInput> = {},
  ) => {
    const d = await detectProject(path);
    return addProject({
      repoRoot: d.repoRoot,
      name: d.name,
      baseBranch: d.baseBranch,
      worktreeHome: d.worktreeHome,
      branchTemplate: d.branchTemplate,
      ...overrides,
    });
  };

  /** Cut a Workspace: worktree + branch off base with the branch
   * template, Slot, workspace-root authorization, `HELMSMEN_*` env. */
  const cutWorkspace = (projectId: string, slug: string) =>
    invoke<HelmCutWorkspace>("helm_cut_workspace", {
      input: { projectId, slug },
    });

  /** Remove a Workspace: delete worktree and branch, free the Slot. */
  const removeWorkspace = (workspaceId: string) =>
    invoke<void>("helm_remove_workspace", { workspaceId });

  const listWorkspaces = () =>
    invoke<HelmWorkspace[]>("helm_list_workspaces");

  /** The `HELMSMEN_*` env assembled for everything spawned in the
   * Workspace (setup script, Processes, Agent Sessions). */
  const workspaceEnv = (workspaceId: string) =>
    invoke<Record<string, string>>("helm_workspace_env", { workspaceId });

  /** Every Harness with its in-code Cap set. */
  const listHarnesses = () => invoke<HelmHarness[]>("helm_list_harnesses");

  const streamChannels = (handlers: AgentStreamHandlers) => {
    if (!makeChannel) {
      throw new Error("helm api was created without a channel factory");
    }
    return {
      onData: makeChannel<ArrayBuffer>((buf) =>
        handlers.onData?.(new Uint8Array(buf)),
      ),
      onExit: makeChannel<number>((code) => handlers.onExit?.(code)),
    };
  };

  /** Spawn an Agent Session in a cut Workspace. The backend resolves
   * worktree, env, and launch command; the frontend only ever names ids
   * and receives a byte stream. */
  const spawnAgent = (workspaceId: string, opts: SpawnAgentOptions = {}) =>
    invoke<HelmAgentSession>("helm_spawn_agent", {
      input: {
        workspaceId,
        harnessId: opts.harnessId,
        runtime: opts.runtime,
        cols: opts.cols,
        rows: opts.rows,
      },
      ...streamChannels(opts),
    });

  /** Re-point a session's stream at new handlers (webview reload); the
   * scrollback replays first, then live output. */
  const attachAgent = (
    session: Pick<HelmAgentSession, "sessionId" | "runtime">,
    handlers: AgentStreamHandlers,
  ) =>
    invoke<void>("helm_attach_agent", {
      runtime: session.runtime,
      session: session.sessionId,
      ...streamChannels(handlers),
    });

  /** Type into a session. */
  const writeAgent = (
    session: Pick<HelmAgentSession, "sessionId" | "runtime">,
    data: string,
  ) =>
    invoke<void>("helm_write_agent", {
      runtime: session.runtime,
      session: session.sessionId,
      data,
    });

  const resizeAgent = (
    session: Pick<HelmAgentSession, "sessionId" | "runtime">,
    cols: number,
    rows: number,
  ) =>
    invoke<void>("helm_resize_agent", {
      runtime: session.runtime,
      session: session.sessionId,
      cols,
      rows,
    });

  const agentStatus = (
    session: Pick<HelmAgentSession, "sessionId" | "runtime">,
  ) =>
    invoke<HelmSessionStatus>("helm_agent_status", {
      runtime: session.runtime,
      session: session.sessionId,
    });

  const killAgent = (
    session: Pick<HelmAgentSession, "sessionId" | "runtime">,
  ) =>
    invoke<void>("helm_kill_agent", {
      runtime: session.runtime,
      session: session.sessionId,
    });

  return {
    detectProject,
    addProject,
    listProjects,
    addProjectFromPath,
    cutWorkspace,
    removeWorkspace,
    listWorkspaces,
    workspaceEnv,
    listHarnesses,
    spawnAgent,
    attachAgent,
    writeAgent,
    resizeAgent,
    agentStatus,
    killAgent,
  };
}

export type HelmApi = ReturnType<typeof createHelmApi>;
