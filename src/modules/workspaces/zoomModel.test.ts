import { describe, expect, it } from "vitest";
import type { HelmProcessDef, HelmSession } from "@/modules/helm/api";
import {
  firstZoomTarget,
  groupSessions,
  harnessToken,
  hopZoomTarget,
  messageToPtyLine,
  processDefLabel,
  resolveZoomTarget,
  sessionTabLabel,
  toZoomSession,
} from "./zoomModel";

// Pure derivation for the zoom view (#12/#13): which Workspace owns a clicked
// Session, its sibling Sessions as tabs (Agent, Shell, Process), the active
// tab, and the [ ] workspace hop. Deterministic over data so it is CI-checked
// without a DOM.

const session = (over: Partial<HelmSession> = {}): HelmSession => ({
  sessionId: "s1",
  runtime: "local-pty",
  workspaceId: "ws-1",
  kind: "agent",
  harnessId: "claude-code",
  ...over,
});

const workspaces = [
  { id: "ws-1", branch: "helm/fix-login" },
  { id: "ws-2", branch: "helm/add-cache" },
  { id: "ws-3", branch: "helm/no-sessions" },
];

describe("sessionTabLabel", () => {
  it("renders harness token + runtime, mapping claude-code to claude", () => {
    expect(sessionTabLabel(session())).toBe("claude·local-pty");
  });

  it("falls back to the raw harness id for unknown harnesses", () => {
    expect(sessionTabLabel(session({ harnessId: "codex" }))).toBe(
      "codex·local-pty",
    );
  });
});

describe("sessionTabLabel by kind", () => {
  it("labels a shell tab", () => {
    expect(sessionTabLabel(session({ kind: "shell" }))).toBe("shell");
  });

  it("labels a process tab name-colon-port, or bare name without a port", () => {
    expect(
      sessionTabLabel(
        session({ kind: "process", processName: "dev", port: 5173 }),
      ),
    ).toBe("dev:5173");
    expect(
      sessionTabLabel(session({ kind: "process", processName: "worker" })),
    ).toBe("worker");
  });
});

describe("harnessToken / processDefLabel", () => {
  it("maps claude-code to claude and passes unknown ids through", () => {
    expect(harnessToken("claude-code")).toBe("claude");
    expect(harnessToken("codex")).toBe("codex");
  });

  it("labels a Process add-button from its definition (name/port)", () => {
    const withPort: HelmProcessDef = {
      name: "dev",
      command: "pnpm dev",
      port: 5173,
    };
    const noPort: HelmProcessDef = { name: "db", command: "docker compose up" };
    expect(processDefLabel(withPort)).toBe("dev:5173");
    expect(processDefLabel(noPort)).toBe("db");
  });
});

describe("toZoomSession / groupSessions", () => {
  it("maps an agent session to a labelled tab", () => {
    expect(toZoomSession(session())).toEqual({
      sessionId: "s1",
      runtime: "local-pty",
      kind: "agent",
      label: "claude·local-pty",
    });
  });

  it("maps shell and process sessions to their kinds and labels", () => {
    expect(toZoomSession(session({ sessionId: "sh", kind: "shell" }))).toEqual({
      sessionId: "sh",
      runtime: "local-pty",
      kind: "shell",
      label: "shell",
    });
    expect(
      toZoomSession(
        session({
          sessionId: "p",
          kind: "process",
          processName: "dev",
          port: 5173,
        }),
      ),
    ).toEqual({
      sessionId: "p",
      runtime: "local-pty",
      kind: "process",
      label: "dev:5173",
    });
  });

  it("groups mixed-kind sessions by workspace preserving spawn order", () => {
    const grouped = groupSessions([
      session({ sessionId: "a", workspaceId: "ws-1" }),
      session({ sessionId: "b", workspaceId: "ws-2", kind: "shell" }),
      session({ sessionId: "c", workspaceId: "ws-1", kind: "process" }),
    ]);
    expect(grouped["ws-1"].map((s) => s.sessionId)).toEqual(["a", "c"]);
    expect(grouped["ws-1"].map((s) => s.kind)).toEqual(["agent", "process"]);
    expect(grouped["ws-2"].map((s) => s.sessionId)).toEqual(["b"]);
  });
});

describe("resolveZoomTarget", () => {
  it("finds the owning Workspace and sets the clicked session active", () => {
    const grouped = groupSessions([
      session({ sessionId: "a", workspaceId: "ws-1" }),
      session({ sessionId: "b", workspaceId: "ws-1" }),
      session({ sessionId: "c", workspaceId: "ws-2" }),
    ]);
    const target = resolveZoomTarget("b", workspaces, grouped);
    expect(target).not.toBeNull();
    expect(target?.workspaceId).toBe("ws-1");
    expect(target?.branch).toBe("helm/fix-login");
    expect(target?.tabs.map((t) => t.sessionId)).toEqual(["a", "b"]);
    expect(target?.activeIndex).toBe(1);
  });

  it("returns null for an unknown session id", () => {
    const grouped = groupSessions([session({ sessionId: "a" })]);
    expect(resolveZoomTarget("ghost", workspaces, grouped)).toBeNull();
  });
});

describe("hopZoomTarget", () => {
  const grouped = groupSessions([
    session({ sessionId: "a", workspaceId: "ws-1" }),
    session({ sessionId: "c", workspaceId: "ws-2" }),
  ]);

  it("] moves to the next Workspace that has sessions, tab 0 active", () => {
    const next = hopZoomTarget("ws-1", 1, workspaces, grouped);
    expect(next?.workspaceId).toBe("ws-2");
    expect(next?.activeIndex).toBe(0);
    expect(next?.branch).toBe("helm/add-cache");
  });

  it("[ wraps around, skipping session-less Workspaces (ws-3)", () => {
    const prev = hopZoomTarget("ws-1", -1, workspaces, grouped);
    expect(prev?.workspaceId).toBe("ws-2");
  });

  it("returns null when the current Workspace is not zoomable", () => {
    expect(hopZoomTarget("ws-3", 1, workspaces, grouped)).toBeNull();
  });
});

describe("firstZoomTarget", () => {
  it("picks the first Workspace with a session (the ↵-zoom entry)", () => {
    const grouped = groupSessions([session({ workspaceId: "ws-2" })]);
    const t = firstZoomTarget(workspaces, grouped);
    expect(t?.workspaceId).toBe("ws-2");
    expect(t?.activeIndex).toBe(0);
  });

  it("returns null when nothing is zoomable", () => {
    expect(firstZoomTarget(workspaces, {})).toBeNull();
  });
});

describe("messageToPtyLine", () => {
  it("delivers the typed text followed by a carriage return (Enter)", () => {
    expect(messageToPtyLine("git status")).toBe("git status\r");
  });

  it("preserves interior content verbatim (hostile-safe: it is only data)", () => {
    expect(messageToPtyLine("rm -rf / # $(whoami)")).toBe(
      "rm -rf / # $(whoami)\r",
    );
  });

  it("sends a bare carriage return for an empty message", () => {
    expect(messageToPtyLine("")).toBe("\r");
  });
});
