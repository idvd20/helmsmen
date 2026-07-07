import { describe, expect, it, vi } from "vitest";
import type { HelmSession } from "@/modules/helm/api";
import type { WorkspaceFacts } from "@/modules/helm/viewModel";
import {
  createSessionStore,
  mergeSessionFacts,
  sessionFactsByWorkspace,
  toSessionFacts,
} from "./sessionStore";

// The interim M2/M3 registry of spawned Sessions (Agent, Shell, Process) and
// its pure projection onto the wall's Session facts. Pub/sub over a list plus
// data-in/data-out folds, so all of it is unit-tested here.

const session = (over: Partial<HelmSession> = {}): HelmSession => ({
  sessionId: "s1",
  runtime: "local-pty",
  workspaceId: "ws-1",
  kind: "agent",
  harnessId: "claude-code",
  ...over,
});

describe("createSessionStore", () => {
  it("registers sessions and lists them in registration order", () => {
    const store = createSessionStore();
    store.register(session({ sessionId: "a" }));
    store.register(session({ sessionId: "b" }));
    expect(store.list().map((s) => s.sessionId)).toEqual(["a", "b"]);
  });

  it("de-dupes by session id (re-register replaces, keeps position)", () => {
    const store = createSessionStore();
    store.register(session({ sessionId: "a", runtime: "local-pty" }));
    store.register(session({ sessionId: "b" }));
    store.register(session({ sessionId: "a", runtime: "tmux" }));
    expect(store.list().map((s) => s.sessionId)).toEqual(["a", "b"]);
    expect(store.list()[0].runtime).toBe("tmux");
  });

  it("unregisters a session by id, leaving the others in order", () => {
    const store = createSessionStore();
    store.register(session({ sessionId: "a" }));
    store.register(session({ sessionId: "b", kind: "shell" }));
    store.register(session({ sessionId: "c", kind: "process" }));
    store.unregister("b");
    expect(store.list().map((s) => s.sessionId)).toEqual(["a", "c"]);
  });

  it("notifies subscribers on register and unregister, not on a no-op", () => {
    const store = createSessionStore();
    const seen = vi.fn();
    const off = store.subscribe(seen);
    store.register(session({ sessionId: "a" }));
    expect(seen).toHaveBeenCalledTimes(1);
    store.unregister("ghost"); // absent -> no emit
    expect(seen).toHaveBeenCalledTimes(1);
    store.unregister("a");
    expect(seen).toHaveBeenCalledTimes(2);
    off();
    store.register(session({ sessionId: "b" }));
    expect(seen).toHaveBeenCalledTimes(2);
  });
});

describe("toSessionFacts", () => {
  it("maps an agent session, tokenizing the harness id", () => {
    expect(toSessionFacts(session())).toEqual({
      sessionId: "s1",
      kind: "agent",
      runtime: "local-pty",
      harness: "claude",
      processName: undefined,
      port: undefined,
    });
  });

  it("maps a shell session (no harness/process facts)", () => {
    expect(toSessionFacts(session({ sessionId: "sh", kind: "shell" }))).toEqual(
      {
        sessionId: "sh",
        kind: "shell",
        runtime: "local-pty",
        harness: undefined,
        processName: undefined,
        port: undefined,
      },
    );
  });

  it("maps a process session, carrying name + port for the chip", () => {
    expect(
      toSessionFacts(
        session({
          sessionId: "p",
          kind: "process",
          processName: "dev",
          port: 5173,
        }),
      ),
    ).toEqual({
      sessionId: "p",
      kind: "process",
      runtime: "local-pty",
      harness: undefined,
      processName: "dev",
      port: 5173,
    });
  });
});

describe("sessionFactsByWorkspace / mergeSessionFacts", () => {
  it("groups session facts by workspace in spawn order", () => {
    const grouped = sessionFactsByWorkspace([
      session({ sessionId: "a", workspaceId: "ws-1" }),
      session({ sessionId: "b", workspaceId: "ws-2", kind: "shell" }),
      session({ sessionId: "c", workspaceId: "ws-1", kind: "process" }),
    ]);
    expect(grouped["ws-1"].map((s) => s.sessionId)).toEqual(["a", "c"]);
    expect(grouped["ws-2"].map((s) => s.sessionId)).toEqual(["b"]);
  });

  it("folds sessions onto workspace facts, preserving other fields", () => {
    const facts: Record<string, WorkspaceFacts> = {
      "ws-1": { startedAtMs: 100, profileId: "prj-1:feature" },
    };
    const merged = mergeSessionFacts(facts, [
      session({ sessionId: "a", workspaceId: "ws-1" }),
      session({ sessionId: "b", workspaceId: "ws-1", kind: "shell" }),
    ]);
    expect(merged["ws-1"].startedAtMs).toBe(100);
    expect(merged["ws-1"].profileId).toBe("prj-1:feature");
    expect(merged["ws-1"].sessions?.map((s) => s.kind)).toEqual([
      "agent",
      "shell",
    ]);
  });

  it("creates an entry for a workspace that has sessions but no prior facts", () => {
    const merged = mergeSessionFacts(
      {},
      [session({ sessionId: "a", workspaceId: "ws-9", kind: "shell" })],
    );
    expect(merged["ws-9"].sessions?.map((s) => s.sessionId)).toEqual(["a"]);
  });

  it("returns the input unchanged when there are no sessions (memo stability)", () => {
    const facts: Record<string, WorkspaceFacts> = { "ws-1": { startedAtMs: 1 } };
    expect(mergeSessionFacts(facts, [])).toBe(facts);
  });

  it("killing a Process Session leaves the other Sessions' chips (AC)", () => {
    // Agent + shell + process on one Workspace; kill the process.
    const store = createSessionStore();
    store.register(session({ sessionId: "agent", kind: "agent" }));
    store.register(session({ sessionId: "sh", kind: "shell" }));
    store.register(
      session({ sessionId: "dev", kind: "process", processName: "dev", port: 5173 }),
    );
    store.unregister("dev");

    const chips = sessionFactsByWorkspace(store.list())["ws-1"];
    expect(chips.map((c) => c.sessionId)).toEqual(["agent", "sh"]);
    expect(chips.some((c) => c.kind === "process")).toBe(false);
  });
});
