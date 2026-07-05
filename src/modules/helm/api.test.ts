import { describe, expect, it, vi } from "vitest";
import {
  createHelmApi,
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
