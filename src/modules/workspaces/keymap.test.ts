import { describe, expect, it } from "vitest";
import { hopWorkspaceIndex, mapZoomKey, tabIndexForDigit } from "./keymap";

// Encodes the zoom keyboard ACs (#12) as pure key -> action mapping so the
// contract is CI-checked even though React key handling can't be unit
// tested here (no DOM env). The imperative shell in Zoom.tsx only wires a
// keydown listener to `mapZoomKey` and dispatches the returned action.

const ctx = (over: Partial<{ tabCount: number; editing: boolean }> = {}) => ({
  tabCount: 3,
  editing: false,
  ...over,
});

describe("tabIndexForDigit", () => {
  it("maps '1'..'9' to zero-based indices within range", () => {
    expect(tabIndexForDigit("1", 3)).toBe(0);
    expect(tabIndexForDigit("2", 3)).toBe(1);
    expect(tabIndexForDigit("3", 3)).toBe(2);
  });

  it("ignores digits past the last tab", () => {
    expect(tabIndexForDigit("4", 3)).toBeNull();
    expect(tabIndexForDigit("9", 3)).toBeNull();
  });

  it("ignores 0 and non-digit keys", () => {
    expect(tabIndexForDigit("0", 3)).toBeNull();
    expect(tabIndexForDigit("m", 3)).toBeNull();
    expect(tabIndexForDigit("Enter", 3)).toBeNull();
    expect(tabIndexForDigit("", 3)).toBeNull();
  });
});

describe("hopWorkspaceIndex", () => {
  it("advances and wraps forward with ]", () => {
    expect(hopWorkspaceIndex(0, 1, 3)).toBe(1);
    expect(hopWorkspaceIndex(2, 1, 3)).toBe(0);
  });

  it("retreats and wraps backward with [", () => {
    expect(hopWorkspaceIndex(2, -1, 3)).toBe(1);
    expect(hopWorkspaceIndex(0, -1, 3)).toBe(2);
  });

  it("stays put with zero or one workspace", () => {
    expect(hopWorkspaceIndex(0, 1, 1)).toBe(0);
    expect(hopWorkspaceIndex(0, -1, 0)).toBe(0);
  });
});

describe("mapZoomKey", () => {
  it("Esc returns to the Helm", () => {
    expect(mapZoomKey({ key: "Escape" }, ctx())).toEqual({ kind: "return" });
  });

  it("1..9 switch to the matching session tab", () => {
    expect(mapZoomKey({ key: "1" }, ctx())).toEqual({
      kind: "switch-tab",
      index: 0,
    });
    expect(mapZoomKey({ key: "3" }, ctx())).toEqual({
      kind: "switch-tab",
      index: 2,
    });
  });

  it("ignores a digit with no matching tab", () => {
    expect(mapZoomKey({ key: "4" }, ctx({ tabCount: 3 }))).toEqual({
      kind: "none",
    });
  });

  it("[ and ] hop across Workspaces", () => {
    expect(mapZoomKey({ key: "[" }, ctx())).toEqual({
      kind: "hop-workspace",
      delta: -1,
    });
    expect(mapZoomKey({ key: "]" }, ctx())).toEqual({
      kind: "hop-workspace",
      delta: 1,
    });
  });

  it("m opens the message-to-PTY input", () => {
    expect(mapZoomKey({ key: "m" }, ctx())).toEqual({ kind: "focus-message" });
  });

  it("a/x answer a paused approval inline (Allow / Deny), decoupled from m", () => {
    expect(mapZoomKey({ key: "a" }, ctx())).toEqual({ kind: "answer-allow" });
    expect(mapZoomKey({ key: "x" }, ctx())).toEqual({ kind: "answer-deny" });
    // Steering stays available independently of any pending ask.
    expect(mapZoomKey({ key: "m" }, ctx())).toEqual({ kind: "focus-message" });
  });

  it("yields every key while a message is being typed", () => {
    // The message box owns the keyboard; chrome keys (digits, brackets,
    // Esc) must reach the field, not the zoom navigation.
    for (const key of ["1", "[", "]", "m", "Escape", "a"]) {
      expect(mapZoomKey({ key }, ctx({ editing: true }))).toEqual({
        kind: "none",
      });
    }
  });

  it("never shadows modified chords (Ctrl-C, meta-T, Alt-*) — raw escape hatch", () => {
    expect(mapZoomKey({ key: "c", ctrlKey: true }, ctx())).toEqual({
      kind: "none",
    });
    expect(mapZoomKey({ key: "t", metaKey: true }, ctx())).toEqual({
      kind: "none",
    });
    expect(mapZoomKey({ key: "1", altKey: true }, ctx())).toEqual({
      kind: "none",
    });
  });

  it("ignores keys with no zoom binding", () => {
    expect(mapZoomKey({ key: "z" }, ctx())).toEqual({ kind: "none" });
  });
});
