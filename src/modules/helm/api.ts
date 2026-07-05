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

export type InvokeFn = <T>(
  cmd: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function createHelmApi(invoke: InvokeFn) {
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

  return {
    detectProject,
    addProject,
    listProjects,
    addProjectFromPath,
    cutWorkspace,
    removeWorkspace,
    listWorkspaces,
    workspaceEnv,
  };
}

export type HelmApi = ReturnType<typeof createHelmApi>;
