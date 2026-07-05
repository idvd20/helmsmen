import { describe, expect, it, vi } from "vitest";
import {
  createHelmApi,
  type HelmCutWorkspace,
  type HelmProject,
  type HelmProjectDetection,
  type InvokeFn,
} from "./api";

// Locks the frontend seam of task #4 (M1: add a Project end-to-end): the
// dev console / UI talks to exactly these Tauri commands with exactly these
// payloads, and the add-from-path flow keeps detection prefills *editable*
// (user overrides win). The backend behavior itself is covered by the Rust
// tests in src-tauri/src/modules/{core,registry}.

const detection: HelmProjectDetection = {
  repoRoot: "/home/dev/src/demo",
  name: "demo",
  baseBranch: "main",
  worktreeHome: "/home/dev/.helmsmen/worktrees/demo",
  branchTemplate: "helm/{slug}",
};

const project: HelmProject = { id: "prj-1", ...detection };

// Locks the frontend seam of task #5 (M1: cut a worktree): cut / remove /
// list / env talk to exactly these Tauri commands with exactly these
// payloads. Slot rule, git mechanics, and authorization are covered by
// the Rust tests in src-tauri/src/modules/{core,registry}.
const cutResult: HelmCutWorkspace = {
  workspace: {
    id: "ws-1",
    projectId: "prj-1",
    slug: "fix-login",
    branch: "helm/fix-login",
    worktreePath: "/home/dev/.helmsmen/worktrees/demo/fix-login-1",
    slot: 1,
  },
  env: {
    HELMSMEN_SLOT: "1",
    HELMSMEN_WORKSPACE: "/home/dev/.helmsmen/worktrees/demo/fix-login-1",
    HELMSMEN_PROJECT: "demo",
    HELMSMEN_MAIN_CHECKOUT: "/home/dev/src/demo",
  },
};

function fakeInvoke(
  responses: Record<string, unknown>,
): { invoke: InvokeFn; calls: Array<[string, unknown]> } {
  const calls: Array<[string, unknown]> = [];
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    if (!(cmd in responses)) throw new Error(`unexpected command ${cmd}`);
    return responses[cmd];
  }) as InvokeFn;
  return { invoke, calls };
}

describe("createHelmApi", () => {
  it("detectProject invokes helm_detect_project with the picked path", async () => {
    const { invoke, calls } = fakeInvoke({ helm_detect_project: detection });
    const api = createHelmApi(invoke);
    await expect(api.detectProject("/home/dev/src/demo")).resolves.toEqual(
      detection,
    );
    expect(calls).toEqual([
      ["helm_detect_project", { path: "/home/dev/src/demo" }],
    ]);
  });

  it("addProject invokes helm_add_project with the input payload", async () => {
    const { invoke, calls } = fakeInvoke({ helm_add_project: project });
    const api = createHelmApi(invoke);
    const input = {
      repoRoot: detection.repoRoot,
      name: detection.name,
      baseBranch: detection.baseBranch,
      worktreeHome: detection.worktreeHome,
      branchTemplate: detection.branchTemplate,
    };
    await expect(api.addProject(input)).resolves.toEqual(project);
    expect(calls).toEqual([["helm_add_project", { input }]]);
  });

  it("listProjects invokes helm_list_projects", async () => {
    const { invoke, calls } = fakeInvoke({ helm_list_projects: [project] });
    const api = createHelmApi(invoke);
    await expect(api.listProjects()).resolves.toEqual([project]);
    expect(calls).toEqual([["helm_list_projects", undefined]]);
  });

  it("addProjectFromPath feeds detection prefills into the add", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_detect_project: detection,
      helm_add_project: project,
    });
    const api = createHelmApi(invoke);
    await expect(api.addProjectFromPath("/home/dev/src/demo")).resolves.toEqual(
      project,
    );
    expect(calls[1]).toEqual([
      "helm_add_project",
      {
        input: {
          repoRoot: detection.repoRoot,
          name: detection.name,
          baseBranch: detection.baseBranch,
          worktreeHome: detection.worktreeHome,
          branchTemplate: detection.branchTemplate,
        },
      },
    ]);
  });

  it("cutWorkspace invokes helm_cut_workspace with projectId and slug", async () => {
    const { invoke, calls } = fakeInvoke({ helm_cut_workspace: cutResult });
    const api = createHelmApi(invoke);
    await expect(api.cutWorkspace("prj-1", "fix-login")).resolves.toEqual(
      cutResult,
    );
    expect(calls).toEqual([
      ["helm_cut_workspace", { input: { projectId: "prj-1", slug: "fix-login" } }],
    ]);
  });

  it("cutWorkspace hands back the assembled HELMSMEN_* env", async () => {
    const { invoke } = fakeInvoke({ helm_cut_workspace: cutResult });
    const api = createHelmApi(invoke);
    const cut = await api.cutWorkspace("prj-1", "fix-login");
    expect(Object.keys(cut.env).sort()).toEqual([
      "HELMSMEN_MAIN_CHECKOUT",
      "HELMSMEN_PROJECT",
      "HELMSMEN_SLOT",
      "HELMSMEN_WORKSPACE",
    ]);
    expect(cut.env.HELMSMEN_SLOT).toBe(String(cut.workspace.slot));
    expect(cut.env.HELMSMEN_WORKSPACE).toBe(cut.workspace.worktreePath);
  });

  it("removeWorkspace invokes helm_remove_workspace with the id", async () => {
    const { invoke, calls } = fakeInvoke({ helm_remove_workspace: undefined });
    const api = createHelmApi(invoke);
    await expect(api.removeWorkspace("ws-1")).resolves.toBeUndefined();
    expect(calls).toEqual([
      ["helm_remove_workspace", { workspaceId: "ws-1" }],
    ]);
  });

  it("listWorkspaces invokes helm_list_workspaces", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_list_workspaces: [cutResult.workspace],
    });
    const api = createHelmApi(invoke);
    await expect(api.listWorkspaces()).resolves.toEqual([cutResult.workspace]);
    expect(calls).toEqual([["helm_list_workspaces", undefined]]);
  });

  it("workspaceEnv invokes helm_workspace_env with the id", async () => {
    const { invoke, calls } = fakeInvoke({ helm_workspace_env: cutResult.env });
    const api = createHelmApi(invoke);
    await expect(api.workspaceEnv("ws-1")).resolves.toEqual(cutResult.env);
    expect(calls).toEqual([["helm_workspace_env", { workspaceId: "ws-1" }]]);
  });

  it("addProjectFromPath keeps prefills editable — overrides win", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_detect_project: detection,
      helm_add_project: project,
    });
    const api = createHelmApi(invoke);
    await api.addProjectFromPath("/home/dev/src/demo", {
      baseBranch: "develop",
      worktreeHome: "/tank/worktrees/demo",
      branchTemplate: "dave/{slug}",
    });
    expect(calls[1]).toEqual([
      "helm_add_project",
      {
        input: {
          repoRoot: detection.repoRoot,
          name: detection.name,
          baseBranch: "develop",
          worktreeHome: "/tank/worktrees/demo",
          branchTemplate: "dave/{slug}",
        },
      },
    ]);
  });
});
