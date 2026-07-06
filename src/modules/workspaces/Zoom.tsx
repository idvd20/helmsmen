// Helmsmen — the Zoom view ("take the wheel", task #12).
//
// Enter zooms from the Helm into one Workspace: the left pane renders the
// active Session's live PTY (PtyPane), `1…9` switch Session tabs, `m` opens
// a message box that delivers a line straight to the PTY mid-run, `[`/`]`
// hop Workspaces, and Esc returns to the Helm. Keyboard-only throughout:
// one window-level keydown listener maps to a pure `ZoomAction` and
// dispatches it (see keymap.ts); modified chords and every key typed into
// the message box fall through untouched, so the interactive `claude` PTY
// keeps its raw escape hatch.
//
// Purely a shell over data + the invoke seam: it never spawns, gits, or
// touches files, and all Session output stays inert text in PtyPane.

import { type CSSProperties, useEffect, useRef, useState } from "react";
import type { HelmApi } from "@/modules/helm/api";
import { mapZoomKey } from "./keymap";
import { PtyPane } from "./PtyPane";
import { messageToPtyLine, type ZoomTarget } from "./zoomModel";

export interface ZoomProps {
  api: HelmApi;
  target: ZoomTarget;
  /** Esc — back to the Helm wall. */
  onReturn: () => void;
  /** `[`/`]` — hop to the previous/next Workspace in zoom. */
  onHopWorkspace: (delta: -1 | 1) => void;
}

export function Zoom({ api, target, onReturn, onHopWorkspace }: ZoomProps) {
  const [activeIndex, setActiveIndex] = useState(target.activeIndex);
  const [messageOpen, setMessageOpen] = useState(false);
  const [messageText, setMessageText] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // A new target (a `[`/`]` hop or a fresh zoom) resets the active tab and
  // closes any open message box.
  useEffect(() => {
    setActiveIndex(target.activeIndex);
    setMessageOpen(false);
    setMessageText("");
  }, [target]);

  useEffect(() => {
    if (messageOpen) inputRef.current?.focus();
  }, [messageOpen]);

  // The single zoom keyboard listener. `editing` yields the whole keyboard
  // to the message box when it is open.
  useEffect(() => {
    const onKeyDown = (ev: KeyboardEvent) => {
      const action = mapZoomKey(
        {
          key: ev.key,
          ctrlKey: ev.ctrlKey,
          metaKey: ev.metaKey,
          altKey: ev.altKey,
        },
        { tabCount: target.tabs.length, editing: messageOpen },
      );
      if (action.kind === "none") return;
      ev.preventDefault();
      switch (action.kind) {
        case "return":
          onReturn();
          break;
        case "switch-tab":
          setActiveIndex(action.index);
          break;
        case "hop-workspace":
          onHopWorkspace(action.delta);
          break;
        case "focus-message":
          setMessageOpen(true);
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [target, messageOpen, onReturn, onHopWorkspace]);

  const active = target.tabs[activeIndex] ?? target.tabs[0];

  const sendMessage = () => {
    if (!active) return;
    void api.writeAgent(active, messageToPtyLine(messageText)).catch(() => {});
    setMessageText("");
  };

  const onInputKeyDown = (ev: React.KeyboardEvent<HTMLInputElement>) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      sendMessage();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      setMessageOpen(false);
    }
  };

  return (
    <section style={rootStyle} aria-label={`zoom ${target.branch}`}>
      <header style={headerStyle}>
        <span aria-hidden style={dotStyle} />
        <span style={branchStyle}>{target.branch}</span>
        <span style={spacerStyle} />
        <span style={hintStyle}>
          [ ] workspace · 1…9 session · m message ·{" "}
          <button type="button" style={escStyle} onClick={onReturn}>
            ⤡ esc
          </button>
        </span>
      </header>

      <nav style={tabBarStyle} aria-label="sessions">
        {target.tabs.map((tab, index) => (
          <button
            key={tab.sessionId}
            type="button"
            aria-current={index === activeIndex}
            style={index === activeIndex ? tabActiveStyle : tabStyle}
            title={`session ${index + 1}`}
            onClick={() => setActiveIndex(index)}
          >
            <span style={tabNumStyle}>{index + 1}</span> {tab.label}
          </button>
        ))}
      </nav>

      {active ? (
        <PtyPane key={active.sessionId} api={api} session={active} />
      ) : (
        <div style={emptyStyle}>no session</div>
      )}

      {messageOpen ? (
        <div style={messageBarStyle}>
          <input
            ref={inputRef}
            style={inputStyle}
            value={messageText}
            aria-label={`message ${active?.label ?? "session"}`}
            placeholder={`message ${active?.label ?? "session"} — straight to the PTY (↵ send · esc close)`}
            onChange={(ev) => setMessageText(ev.target.value)}
            onKeyDown={onInputKeyDown}
          />
        </div>
      ) : null}
    </section>
  );
}

const rootStyle: CSSProperties = {
  position: "absolute",
  inset: 0,
  display: "flex",
  flexDirection: "column",
  background: "#070a0f",
  color: "#d7dae0",
  font: "13px/1.45 ui-sans-serif, system-ui, sans-serif",
};

const headerStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "10px 14px",
  borderBottom: "1px solid #1b2130",
};

const dotStyle: CSSProperties = {
  width: 8,
  height: 8,
  borderRadius: "50%",
  background: "#f5a623",
  flex: "0 0 auto",
};

const branchStyle: CSSProperties = {
  font: "12px/1 ui-monospace, monospace",
  fontWeight: 600,
};

const spacerStyle: CSSProperties = { flex: "1 1 auto" };

const hintStyle: CSSProperties = {
  color: "#5b6273",
  fontSize: 11,
  display: "flex",
  alignItems: "center",
  gap: 6,
};

const escStyle: CSSProperties = {
  background: "#141a26",
  border: "1px solid #2d3343",
  borderRadius: 4,
  color: "#d7dae0",
  cursor: "pointer",
  font: "11px/1 ui-monospace, monospace",
  padding: "3px 6px",
};

const tabBarStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 6,
  padding: "8px 14px",
  borderBottom: "1px solid #1b2130",
};

const tabStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  padding: "3px 10px",
  background: "#141a26",
  border: "1px solid #2d3343",
  borderRadius: 999,
  color: "#d7dae0",
  font: "11px/1 ui-monospace, monospace",
  cursor: "pointer",
};

const tabActiveStyle: CSSProperties = {
  ...tabStyle,
  background: "#1e2636",
  borderColor: "#3b4a63",
  color: "#fff",
};

const tabNumStyle: CSSProperties = { color: "#8b93a7", fontWeight: 700 };

const emptyStyle: CSSProperties = {
  flex: "1 1 auto",
  display: "grid",
  placeItems: "center",
  color: "#5b6273",
};

const messageBarStyle: CSSProperties = {
  padding: 10,
  borderTop: "1px solid #1b2130",
  background: "#0b0e14",
};

const inputStyle: CSSProperties = {
  width: "100%",
  padding: "8px 10px",
  background: "#05070b",
  border: "1px solid #2d3343",
  borderRadius: 6,
  color: "#d7dae0",
  font: "12px/1.4 ui-monospace, monospace",
  boxSizing: "border-box",
};
