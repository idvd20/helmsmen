import { describe, expect, it, vi } from "vitest";
import {
  type ChannelFactory,
  createHelmApi,
  type HelmAgentSession,
  type HelmCutWorkspace,
  type HelmHarness,
  type HelmProfile,
  type HelmProject,
  type HelmProjectDetection,
  type HelmProjectSettings,
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

const emptySettings: HelmProjectSettings = {
  setupScript: "",
  carryOverGlobs: [],
  processes: [],
};

const project: HelmProject = {
  id: "prj-1",
  ...detection,
  settings: emptySettings,
};

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

// Locks the frontend seam of task #7 (M2: Project settings + Profiles):
// settings and Profiles are edited through exactly these Tauri commands
// — the frontend never writes a file, and nothing is executed at this
// slice (definitions only; the cut pipeline of task #8 runs them).
// Seeding, divergence isolation, and validation are covered by the Rust
// tests in src-tauri/src/modules/{core,registry}.

const settings: HelmProjectSettings = {
  setupScript: "pnpm install --frozen-lockfile\ncp .env.example .env",
  carryOverGlobs: [".env*"],
  processes: [{ name: "dev", command: "pnpm dev" }],
};

const profile: HelmProfile = {
  id: "prj-1:feature",
  projectId: "prj-1",
  name: "Feature",
  promptSnippet: "/tdd {brief}",
  model: "",
  mcpServers: [],
  verifyCommand: "",
  color: "#3b82f6",
  harnessId: "claude-code",
};

describe("createHelmApi project settings and profiles", () => {
  it("updateProjectSettings sends the whole settings blob for one project", async () => {
    const updated: HelmProject = { ...project, settings };
    const { invoke, calls } = fakeInvoke({
      helm_update_project_settings: updated,
    });
    const api = createHelmApi(invoke);
    await expect(
      api.updateProjectSettings("prj-1", settings),
    ).resolves.toEqual(updated);
    expect(calls).toEqual([
      ["helm_update_project_settings", { projectId: "prj-1", settings }],
    ]);
  });

  it("listProfiles asks for one project's seeded copies", async () => {
    const { invoke, calls } = fakeInvoke({ helm_list_profiles: [profile] });
    const api = createHelmApi(invoke);
    await expect(api.listProfiles("prj-1")).resolves.toEqual([profile]);
    expect(calls).toEqual([["helm_list_profiles", { projectId: "prj-1" }]]);
  });

  it("listProfiles without a project lists everything", async () => {
    const { invoke, calls } = fakeInvoke({ helm_list_profiles: [profile] });
    const api = createHelmApi(invoke);
    await api.listProfiles();
    expect(calls).toEqual([["helm_list_profiles", { projectId: undefined }]]);
  });

  it("updateProfile sends the full profile — every field of the set", async () => {
    const edited: HelmProfile = {
      ...profile,
      promptSnippet: "/tdd {brief} — keep commits small",
      model: "claude-opus-4-6",
      mcpServers: ["playwright"],
      verifyCommand: "pnpm test",
      color: "#123abc",
    };
    const { invoke, calls } = fakeInvoke({ helm_update_profile: edited });
    const api = createHelmApi(invoke);
    await expect(api.updateProfile(edited)).resolves.toEqual(edited);
    expect(calls).toEqual([["helm_update_profile", { profile: edited }]]);
  });
});

// Locks the frontend seam of task #6 (M1: Runtime + Harness traits): the
// dev console talks to exactly these session commands with opaque ids
// and injected channels; every OS decision (worktree, env, launch
// command) stays backend-side. Runtime/Harness behavior itself is
// covered by the Rust conformance suite and spawn tests.

interface FakeChannel<T = unknown> {
  deliver: (message: T) => void;
}

function fakeChannels(): {
  makeChannel: ChannelFactory;
  channels: FakeChannel[];
} {
  const channels: FakeChannel[] = [];
  const makeChannel: ChannelFactory = <T>(onMessage: (message: T) => void) => {
    const channel: FakeChannel<T> = { deliver: onMessage };
    channels.push(channel as FakeChannel);
    return channel;
  };
  return { makeChannel, channels };
}

const session: HelmAgentSession = {
  sessionId: "lpty-1",
  runtime: "local-pty",
  harnessId: "claude-code",
  workspaceId: "ws-1",
};

const harness: HelmHarness = {
  id: "claude-code",
  displayName: "Claude Code",
  caps: {
    resume: true,
    controlPlaneHooks: true,
    agentSignal: true,
    costTelemetry: true,
    mcpConfig: true,
    modelSelect: true,
  },
};

describe("createHelmApi agent sessions", () => {
  it("spawnAgent invokes helm_spawn_agent with input and both channels", async () => {
    const { invoke, calls } = fakeInvoke({ helm_spawn_agent: session });
    const { makeChannel, channels } = fakeChannels();
    const api = createHelmApi(invoke, makeChannel);
    await expect(
      api.spawnAgent("ws-1", { cols: 120, rows: 32 }),
    ).resolves.toEqual(session);
    expect(channels).toHaveLength(2);
    expect(calls).toEqual([
      [
        "helm_spawn_agent",
        {
          input: {
            workspaceId: "ws-1",
            harnessId: undefined,
            runtime: undefined,
            cols: 120,
            rows: 32,
          },
          onData: channels[0],
          onExit: channels[1],
        },
      ],
    ]);
  });

  it("delivers stream bytes as Uint8Array and exit codes verbatim", async () => {
    const { invoke } = fakeInvoke({ helm_spawn_agent: session });
    const { makeChannel, channels } = fakeChannels();
    const api = createHelmApi(invoke, makeChannel);
    const seen: Uint8Array[] = [];
    const exits: number[] = [];
    await api.spawnAgent("ws-1", {
      onData: (bytes) => seen.push(bytes),
      onExit: (code) => exits.push(code),
    });
    const hostile = new TextEncoder().encode("\x1b]0;owned\x07data");
    (channels[0] as FakeChannel<ArrayBuffer>).deliver(
      hostile.buffer as ArrayBuffer,
    );
    (channels[1] as FakeChannel<number>).deliver(7);
    expect(seen).toHaveLength(1);
    expect(new TextDecoder().decode(seen[0])).toBe("\x1b]0;owned\x07data");
    expect(exits).toEqual([7]);
  });

  it("spawnAgent without a channel factory fails loudly, not silently", async () => {
    const { invoke, calls } = fakeInvoke({ helm_spawn_agent: session });
    const api = createHelmApi(invoke);
    expect(() => api.spawnAgent("ws-1")).toThrow(/channel factory/);
    expect(calls).toEqual([]);
  });

  it("attachAgent invokes helm_attach_agent with the echoed handle", async () => {
    const { invoke, calls } = fakeInvoke({ helm_attach_agent: undefined });
    const { makeChannel, channels } = fakeChannels();
    const api = createHelmApi(invoke, makeChannel);
    await api.attachAgent(session, {});
    expect(calls).toEqual([
      [
        "helm_attach_agent",
        {
          runtime: "local-pty",
          session: "lpty-1",
          onData: channels[0],
          onExit: channels[1],
        },
      ],
    ]);
  });

  it("writeAgent types into the session by id only", async () => {
    const { invoke, calls } = fakeInvoke({ helm_write_agent: undefined });
    const api = createHelmApi(invoke);
    await api.writeAgent(session, "hello\r");
    expect(calls).toEqual([
      [
        "helm_write_agent",
        { runtime: "local-pty", session: "lpty-1", data: "hello\r" },
      ],
    ]);
  });

  it("resizeAgent, agentStatus, and killAgent echo the handle", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_resize_agent: undefined,
      helm_agent_status: { state: "running" },
      helm_kill_agent: undefined,
    });
    const api = createHelmApi(invoke);
    await api.resizeAgent(session, 200, 50);
    await expect(api.agentStatus(session)).resolves.toEqual({
      state: "running",
    });
    await api.killAgent(session);
    expect(calls).toEqual([
      [
        "helm_resize_agent",
        { runtime: "local-pty", session: "lpty-1", cols: 200, rows: 50 },
      ],
      ["helm_agent_status", { runtime: "local-pty", session: "lpty-1" }],
      ["helm_kill_agent", { runtime: "local-pty", session: "lpty-1" }],
    ]);
  });

  it("listHarnesses surfaces the in-code Cap sets", async () => {
    const { invoke, calls } = fakeInvoke({ helm_list_harnesses: [harness] });
    const api = createHelmApi(invoke);
    await expect(api.listHarnesses()).resolves.toEqual([harness]);
    expect(calls).toEqual([["helm_list_harnesses", undefined]]);
  });
});
