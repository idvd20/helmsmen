import { describe, expect, it, vi } from "vitest";
import {
  type ChannelFactory,
  createHelmApi,
  type InvokeFn,
} from "@/modules/helm/api";

// The zoom's add-session path at the runtime seam, exercised with a fake
// invoke (the frontend stand-in for a fake PTY): adding a Shell / Process
// Session names a Workspace (and, for a Process, a definition) and both
// stream channels, exactly like an Agent spawn. The real byte transport is
// proven against a real PTY by the Rust spawn suite
// (a_real_shell/process_runs_in_the_worktree_with_the_helmsmen_env); this
// pins the command + payload the zoom sends, and the kill path a killed
// Session takes.

function fakeInvoke(returns: Record<string, unknown> = {}): {
  invoke: InvokeFn;
  calls: Array<[string, unknown]>;
} {
  const calls: Array<[string, unknown]> = [];
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    return returns[cmd];
  }) as InvokeFn;
  return { invoke, calls };
}

const noopChannel: ChannelFactory = () => ({});

describe("zoom add-session path", () => {
  it("spawnShell invokes helm_spawn_shell with the workspace + both channels", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_spawn_shell: {
        sessionId: "lpty-2",
        runtime: "local-pty",
        workspaceId: "ws-1",
        kind: "shell",
      },
    });
    const api = createHelmApi(invoke, noopChannel);
    const session = await api.spawnShell("ws-1", { cols: 100, rows: 30 });
    expect(session.kind).toBe("shell");
    const [cmd, args] = calls[0];
    expect(cmd).toBe("helm_spawn_shell");
    const a = args as Record<string, unknown>;
    expect(a.input).toEqual({
      workspaceId: "ws-1",
      runtime: undefined,
      cols: 100,
      rows: 30,
    });
    expect(a.onData).toBeDefined();
    expect(a.onExit).toBeDefined();
  });

  it("spawnProcess names a definition (never a command) + both channels", async () => {
    const { invoke, calls } = fakeInvoke({
      helm_spawn_process: {
        sessionId: "lpty-3",
        runtime: "local-pty",
        workspaceId: "ws-1",
        kind: "process",
        processName: "dev",
        port: 5173,
      },
    });
    const api = createHelmApi(invoke, noopChannel);
    const session = await api.spawnProcess("ws-1", "dev");
    expect(session.kind).toBe("process");
    expect(session.processName).toBe("dev");
    expect(session.port).toBe(5173);
    const [cmd, args] = calls[0];
    expect(cmd).toBe("helm_spawn_process");
    const a = args as Record<string, unknown>;
    expect(a.input).toEqual({
      workspaceId: "ws-1",
      processName: "dev",
      runtime: undefined,
      cols: undefined,
      rows: undefined,
    });
    expect(a.onData).toBeDefined();
    expect(a.onExit).toBeDefined();
  });

  it("killing a Session echoes runtime + id to helm_kill_agent", async () => {
    const { invoke, calls } = fakeInvoke();
    const api = createHelmApi(invoke, noopChannel);
    await api.killAgent({ sessionId: "lpty-3", runtime: "local-pty" });
    expect(calls).toContainEqual([
      "helm_kill_agent",
      { runtime: "local-pty", session: "lpty-3" },
    ]);
  });

  it("spawn add-session without a channel factory fails loudly, not silently", () => {
    const { invoke, calls } = fakeInvoke();
    const api = createHelmApi(invoke);
    expect(() => api.spawnShell("ws-1")).toThrow(/channel factory/);
    expect(() => api.spawnProcess("ws-1", "dev")).toThrow(/channel factory/);
    expect(calls).toEqual([]);
  });
});
