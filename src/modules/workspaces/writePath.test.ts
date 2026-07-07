import { describe, expect, it, vi } from "vitest";
import {
  type ChannelFactory,
  createHelmApi,
  type HelmAgentSession,
  type InvokeFn,
} from "@/modules/helm/api";
import { messageToPtyLine } from "./zoomModel";

// The zoom's write/attach path at the runtime seam, exercised with a fake
// invoke (the frontend stand-in for a fake PTY): entering a zoom re-points
// the session's stream (helm_attach_agent) and `m` delivers a line to the
// live PTY (helm_write_agent). The real byte transport is proven against a
// real PTY by the Rust conformance suite (case_attach_replays_scrollback_
// then_streams, case_write_reaches_stdin); this pins the command + payload
// the zoom sends.

const session: HelmAgentSession = {
  sessionId: "lpty-1",
  runtime: "local-pty",
  harnessId: "claude-code",
  workspaceId: "ws-1",
};

function fakeInvoke(): { invoke: InvokeFn; calls: Array<[string, unknown]> } {
  const calls: Array<[string, unknown]> = [];
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    return undefined;
  }) as InvokeFn;
  return { invoke, calls };
}

const noopChannel: ChannelFactory = () => ({});

describe("zoom write/attach path", () => {
  it("attaches to the active session on zoom, echoing runtime + id", async () => {
    const { invoke, calls } = fakeInvoke();
    const api = createHelmApi(invoke, noopChannel);
    await api.attachAgent(session, { onData: () => {}, onExit: () => {} });
    const attach = calls.find(([cmd]) => cmd === "helm_attach_agent");
    expect(attach).toBeTruthy();
    expect((attach?.[1] as Record<string, unknown>).runtime).toBe("local-pty");
    expect((attach?.[1] as Record<string, unknown>).session).toBe("lpty-1");
  });

  it("m delivers the typed line to the PTY mid-run via helm_write_agent", async () => {
    const { invoke, calls } = fakeInvoke();
    const api = createHelmApi(invoke, noopChannel);
    await api.writeAgent(session, messageToPtyLine("npm test"));
    expect(calls).toContainEqual([
      "helm_write_agent",
      { runtime: "local-pty", session: "lpty-1", data: "npm test\r" },
    ]);
  });
});
