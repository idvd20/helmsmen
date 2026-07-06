// Helmsmen — zoom keyboard map (task #12), new module per docs/fork-posture.md.
//
// Pure key -> action mapping for the Zoom ("take the wheel") view. The
// imperative shell (Zoom.tsx) owns a single keydown listener that calls
// `mapZoomKey` and dispatches the returned action; keeping the decision
// here makes the zoom keyboard contract testable without a DOM env (this
// repo has none) and keeps the shell dumb.
//
// Every action stays inert data: the map never spawns, writes, or touches
// the OS — that is exclusively the backend's job across the invoke seam.

/** What a key press means inside the zoom. `none` = not a zoom binding;
 * the shell lets it fall through (to the browser / a focused field). */
export type ZoomAction =
  | { kind: "return" }
  | { kind: "switch-tab"; index: number }
  | { kind: "hop-workspace"; delta: -1 | 1 }
  | { kind: "focus-message" }
  | { kind: "none" };

/** The subset of a KeyboardEvent the map reads. Modelled as plain data so
 * tests need no synthetic DOM events. */
export interface ZoomKeyInput {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

export interface ZoomKeyContext {
  /** How many Session tabs the active Workspace has (bounds `1…9`). */
  tabCount: number;
  /** An editable field (the message box) holds focus. Chrome keys must
   * yield so the user can type digits, brackets, and Esc into it. */
  editing: boolean;
}

/** `'1'…'9'` -> zero-based tab index, or null if the digit has no tab
 * (out of range, `'0'`, or a non-digit key). */
export function tabIndexForDigit(key: string, tabCount: number): number | null {
  if (key.length !== 1 || key < "1" || key > "9") return null;
  const index = key.charCodeAt(0) - "1".charCodeAt(0);
  return index < tabCount ? index : null;
}

/** Next Workspace index for a `[`/`]` hop, wrapping both directions. A
 * count of 0 or 1 keeps the index put. */
export function hopWorkspaceIndex(
  current: number,
  delta: number,
  count: number,
): number {
  if (count <= 1) return count <= 0 ? 0 : current;
  return (((current + delta) % count) + count) % count;
}

/** Map a key press to a zoom action. Modified chords (Ctrl/Meta/Alt) and
 * every key while typing a message are left for the terminal / field —
 * the raw escape hatch to the live PTY is never shadowed. */
export function mapZoomKey(
  ev: ZoomKeyInput,
  ctx: ZoomKeyContext,
): ZoomAction {
  if (ctx.editing) return { kind: "none" };
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return { kind: "none" };

  switch (ev.key) {
    case "Escape":
      return { kind: "return" };
    case "[":
      return { kind: "hop-workspace", delta: -1 };
    case "]":
      return { kind: "hop-workspace", delta: 1 };
    case "m":
      return { kind: "focus-message" };
    default: {
      const index = tabIndexForDigit(ev.key, ctx.tabCount);
      return index == null
        ? { kind: "none" }
        : { kind: "switch-tab", index };
    }
  }
}
