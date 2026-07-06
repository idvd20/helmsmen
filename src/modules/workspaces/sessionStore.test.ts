import { describe, expect, it, vi } from "vitest";
import type { HelmAgentSession } from "@/modules/helm/api";
import { createSessionStore } from "./sessionStore";

// The interim M2 registry of spawned Agent Sessions. Chips/session facts on
// the wall arrive with later slices; until then this store lets the zoom
// resolve a Session's Workspace and sibling tabs. Pure pub/sub over a list,
// so it is unit-tested here.

const session = (over: Partial<HelmAgentSession> = {}): HelmAgentSession => ({
  sessionId: "s1",
  runtime: "local-pty",
  harnessId: "claude-code",
  workspaceId: "ws-1",
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

  it("notifies subscribers on change and stops after unsubscribe", () => {
    const store = createSessionStore();
    const seen = vi.fn();
    const off = store.subscribe(seen);
    store.register(session({ sessionId: "a" }));
    expect(seen).toHaveBeenCalledTimes(1);
    off();
    store.register(session({ sessionId: "b" }));
    expect(seen).toHaveBeenCalledTimes(1);
  });
});
