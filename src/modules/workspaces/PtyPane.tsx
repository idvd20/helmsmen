// Helmsmen — the zoom's live PTY pane (task #12).
//
// Renders one Agent Session's output by re-pointing its stream at this pane
// (helm_attach_agent: the retained scrollback replays first, then live
// output). Rendering reuses the helm module's safe path exactly: hostile
// PTY bytes are accumulated in the pure `createStreamBuffer` and land in a
// <pre> via `textContent` only — never an HTML/script sink — so output can
// never become markup or a privileged action (the same invariant
// streamView.ts and the helm guards enforce).
//
// Input to the live process goes through the message box in Zoom.tsx
// (helm_write_agent); this pane is render-only. Full escape-sequence / TUI
// fidelity (an xterm grid) is the earmarked next step — see the module's
// journal.

import { type CSSProperties, useEffect, useRef, useState } from "react";
import type { HelmApi } from "@/modules/helm/api";
import { createStreamBuffer } from "@/modules/helm/stream";

export interface PtyPaneSession {
  sessionId: string;
  runtime: string;
}

export interface PtyPaneProps {
  api: HelmApi;
  session: PtyPaneSession;
}

// Approximate monospace cell used to translate the pane's pixel size into a
// PTY grid on attach. Rough on purpose: it only needs to give the process a
// sane winsize, and the backend clamps non-zero dims.
const CELL_W = 8;
const CELL_H = 17;

export function PtyPane({ api, session }: PtyPaneProps) {
  const preRef = useRef<HTMLPreElement>(null);
  const [exitCode, setExitCode] = useState<number | null>(null);

  // Re-attach whenever the active tab (session) changes. Attaching replays
  // scrollback, so switching tabs and switching back both restore context.
  useEffect(() => {
    const pre = preRef.current;
    if (!pre) return;
    let live = true;
    setExitCode(null);
    pre.textContent = "";
    const buffer = createStreamBuffer();

    void api
      .attachAgent(session, {
        onData: (bytes) => {
          if (!live) return;
          pre.textContent = buffer.append(bytes);
          pre.scrollTop = pre.scrollHeight;
        },
        onExit: (code) => {
          if (live) setExitCode(code);
        },
      })
      .catch(() => {
        // A transient attach failure leaves the last snapshot on screen.
      });

    // Best-effort winsize so alt-screen TUIs lay out to the pane.
    const rect = pre.getBoundingClientRect();
    const cols = Math.max(20, Math.floor(rect.width / CELL_W));
    const rows = Math.max(6, Math.floor(rect.height / CELL_H));
    void api.resizeAgent(session, cols, rows).catch(() => {});

    return () => {
      live = false;
    };
  }, [api, session]);

  return (
    <div style={paneStyle}>
      <pre ref={preRef} style={outputStyle} role="log" aria-label="session output" />
      {exitCode !== null ? (
        <div style={exitStyle}>session exited ({exitCode})</div>
      ) : null}
    </div>
  );
}

const paneStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  flex: "1 1 auto",
  minHeight: 0,
};

const outputStyle: CSSProperties = {
  flex: "1 1 auto",
  margin: 0,
  padding: 10,
  overflow: "auto",
  whiteSpace: "pre-wrap",
  wordBreak: "break-all",
  background: "#05070b",
  color: "#d7dae0",
  font: "12px/1.4 ui-monospace, monospace",
};

const exitStyle: CSSProperties = {
  padding: "4px 10px",
  color: "#8b93a7",
  fontSize: 11,
  borderTop: "1px solid #1b2130",
};
