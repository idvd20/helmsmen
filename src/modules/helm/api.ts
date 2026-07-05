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

  return { detectProject, addProject, listProjects, addProjectFromPath };
}

export type HelmApi = ReturnType<typeof createHelmApi>;
