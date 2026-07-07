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

import {
  type CSSProperties,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { HelmApi, HelmApproval, HelmProcessDef } from "@/modules/helm/api";
import { mapZoomKey } from "./keymap";
import { PtyPane } from "./PtyPane";
import { sessionStore } from "./sessionStore";
import {
  derivePausedCalls,
  groupSessions,
  messageToPtyLine,
  type PausedCall,
  pickAgentSession,
  processDefLabel,
  type ZoomSession,
  type ZoomTarget,
} from "./zoomModel";

/** How often the zoom re-reads the Workspace's approval state so a newly
 * paused call surfaces inline (and answered ones clear). */
const APPROVALS_POLL_MS = 1200;

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
  const [liveSessions, setLiveSessions] = useState(() => sessionStore.list());
  const [processes, setProcesses] = useState<HelmProcessDef[]>([]);
  const [approvals, setApprovals] = useState<HelmApproval[]>([]);
  // The paused call whose Deny reason is being typed (null = box closed). `x`
  // targets the top call; a card's Deny button targets that card.
  const [denyTarget, setDenyTarget] = useState<PausedCall | null>(null);
  const [denyText, setDenyText] = useState("");
  const [answerNote, setAnswerNote] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const denyRef = useRef<HTMLInputElement>(null);

  // Live Session set: a Session added or killed from the zoom appears /
  // disappears here at once, so the tab bar tracks it without waiting on the
  // container to re-derive the target.
  useEffect(
    () => sessionStore.subscribe(() => setLiveSessions(sessionStore.list())),
    [],
  );

  // The active Workspace's Session tabs, live from the store (spawn order).
  // Falls back to the container-supplied tabs before the first subscription
  // fire so the initial render is never empty.
  const tabs = useMemo<ZoomSession[]>(() => {
    const grouped = groupSessions(liveSessions)[target.workspaceId];
    return grouped && grouped.length > 0 ? grouped : target.tabs;
  }, [liveSessions, target.workspaceId, target.tabs]);

  // A new target (a `[`/`]` hop or a fresh zoom) resets the active tab and
  // closes any open message / deny box.
  useEffect(() => {
    setActiveIndex(target.activeIndex);
    setMessageOpen(false);
    setMessageText("");
    setDenyTarget(null);
    setDenyText("");
    setAnswerNote(null);
    setApprovals([]);
  }, [target]);

  const denyOpen = denyTarget !== null;

  useEffect(() => {
    if (messageOpen) inputRef.current?.focus();
  }, [messageOpen]);

  useEffect(() => {
    if (denyOpen) denyRef.current?.focus();
  }, [denyOpen]);

  // Poll the Workspace's approval state so a paused call surfaces inline (and
  // clears once answered). Snapshot of the control-plane endpoint; a null (no
  // endpoint) or a transient failure just leaves the last set.
  useEffect(() => {
    let live = true;
    const tick = async () => {
      try {
        const state = await api.approvalsSnapshot(target.workspaceId);
        if (live) setApprovals(state?.cards ?? []);
      } catch {
        // transient invoke failure — keep the last snapshot
      }
    };
    void tick();
    const id = setInterval(tick, APPROVALS_POLL_MS);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [api, target.workspaceId]);

  // The Project's Process definitions for the add-session menu. Resolved
  // through the api (workspace -> project -> settings.processes); a raw
  // command never crosses the seam — the spawn names a definition.
  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const [workspaces, projects] = await Promise.all([
          api.listWorkspaces(),
          api.listProjects(),
        ]);
        const ws = workspaces.find((w) => w.id === target.workspaceId);
        const project = projects.find((p) => p.id === ws?.projectId);
        if (live) setProcesses(project?.settings.processes ?? []);
      } catch {
        // A transient invoke failure just leaves the last process list.
      }
    })();
    return () => {
      live = false;
    };
  }, [api, target.workspaceId]);

  // Paused approvals for this Workspace (inline Allow/Deny). `a`/`x` act on
  // the TOP (oldest open) call; the agent Session is where keys inject.
  const pausedCalls = useMemo(() => derivePausedCalls(approvals), [approvals]);
  const topCall = pausedCalls[0] ?? null;
  const agentSession = useMemo(() => pickAgentSession(tabs), [tabs]);

  const refreshApprovals = useCallback(async () => {
    try {
      const state = await api.approvalsSnapshot(target.workspaceId);
      setApprovals(state?.cards ?? []);
    } catch {
      // keep the last snapshot
    }
  }, [api, target.workspaceId]);

  // Answer a paused call — the ONE seam, over the invoke boundary. On a
  // mismatch the backend injected nothing (the visible dialog was not this
  // card's); surface that instead of pretending it resolved. Always re-poll so
  // the card reconciles by tool_use_id.
  const answerCall = useCallback(
    async (call: PausedCall, action: "allow" | "deny", reason?: string) => {
      if (!agentSession) {
        setAnswerNote("no agent session in this workspace to answer");
        return;
      }
      setAnswerNote(null);
      try {
        const outcome = await api.answerPrompt({
          session: agentSession.sessionId,
          runtime: agentSession.runtime,
          toolUseId: call.toolUseId,
          expectedCommand: call.command,
          action,
          reason,
        });
        if (outcome.status === "mismatch") {
          setAnswerNote("dialog changed — not answered; re-check the call");
        }
      } catch {
        setAnswerNote("could not reach the agent");
      }
      void refreshApprovals();
    },
    [agentSession, api, refreshApprovals],
  );

  const allowCall = useCallback(
    (call: PausedCall) => void answerCall(call, "allow"),
    [answerCall],
  );
  const submitDeny = useCallback(() => {
    if (denyTarget)
      void answerCall(denyTarget, "deny", denyText.trim() || undefined);
    setDenyTarget(null);
    setDenyText("");
  }, [denyTarget, denyText, answerCall]);

  // The single zoom keyboard listener. `editing` yields the whole keyboard to
  // the message box or the deny-reason box when either is open.
  useEffect(() => {
    const onKeyDown = (ev: KeyboardEvent) => {
      const action = mapZoomKey(
        {
          key: ev.key,
          ctrlKey: ev.ctrlKey,
          metaKey: ev.metaKey,
          altKey: ev.altKey,
        },
        { tabCount: tabs.length, editing: messageOpen || denyOpen },
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
        case "answer-allow":
          if (topCall) allowCall(topCall);
          break;
        case "answer-deny":
          if (topCall) setDenyTarget(topCall);
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    tabs.length,
    messageOpen,
    denyOpen,
    topCall,
    allowCall,
    onReturn,
    onHopWorkspace,
  ]);

  // Clamp the active tab into the live set: killing a Session shrinks the
  // tabs, so an out-of-range index falls back to the last remaining tab.
  const safeIndex = Math.min(Math.max(activeIndex, 0), Math.max(tabs.length - 1, 0));
  const active = tabs[safeIndex];

  const sendMessage = () => {
    if (!active) return;
    void api.writeAgent(active, messageToPtyLine(messageText)).catch(() => {});
    setMessageText("");
  };

  // Add-session controls. The backend spawns (shell / named Process) in the
  // worktree with the `HELMSMEN_*` env; the returned handle is registered so
  // it appears as a new tab here and a new chip on the wall. Focus the new
  // tab: it is appended, so its index is the pre-add tab count.
  const addShell = () => {
    const nextIndex = tabs.length;
    void api
      .spawnShell(target.workspaceId)
      .then((session) => {
        sessionStore.register(session);
        setActiveIndex(nextIndex);
      })
      .catch(() => {});
  };

  const addProcess = (name: string) => {
    const nextIndex = tabs.length;
    void api
      .spawnProcess(target.workspaceId, name)
      .then((session) => {
        sessionStore.register(session);
        setActiveIndex(nextIndex);
      })
      .catch(() => {});
  };

  // Kill a Session: terminate its process, then drop it from the registry so
  // its tab and wall chip disappear and the status rollup recomputes over the
  // remaining live Sessions.
  const killSession = (session: ZoomSession) => {
    void api.killAgent(session).catch(() => {});
    sessionStore.unregister(session.sessionId);
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

  const onDenyKeyDown = (ev: React.KeyboardEvent<HTMLInputElement>) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      submitDeny();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      setDenyTarget(null);
      setDenyText("");
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
        {tabs.map((tab, index) => (
          <span
            key={tab.sessionId}
            style={index === safeIndex ? tabActiveStyle : tabStyle}
          >
            <button
              type="button"
              aria-current={index === safeIndex}
              style={tabSelectStyle}
              title={`session ${index + 1}`}
              onClick={() => setActiveIndex(index)}
            >
              <span style={tabNumStyle}>{index + 1}</span> {tab.label}
            </button>
            <button
              type="button"
              style={killStyle}
              title={`kill ${tab.label}`}
              aria-label={`kill ${tab.label}`}
              onClick={() => killSession(tab)}
            >
              ×
            </button>
          </span>
        ))}
        <span style={addBarStyle}>
          <button
            type="button"
            style={addStyle}
            title="add a shell session in this worktree"
            onClick={addShell}
          >
            ＋ shell
          </button>
          {processes.map((def) => (
            <button
              key={def.name}
              type="button"
              style={addStyle}
              title={`add the ${def.name} process in this worktree`}
              onClick={() => addProcess(def.name)}
            >
              ＋ {processDefLabel(def)}
            </button>
          ))}
        </span>
      </nav>

      {active ? (
        <PtyPane key={active.sessionId} api={api} session={active} />
      ) : (
        <div style={emptyStyle}>no session</div>
      )}

      {pausedCalls.length > 0 ? (
        <div style={approvalBarStyle} aria-label="paused approvals">
          {pausedCalls.map((call, index) => (
            <div
              key={call.id}
              style={index === 0 ? approvalCardTopStyle : approvalCardStyle}
            >
              <div style={approvalMetaStyle}>
                <span style={approvalToolStyle}>⏸ {call.tool}</span>
                <span style={approvalRuleStyle}>{call.rule}</span>
                {index === 0 ? (
                  <span style={approvalHintStyle}>a allow · x deny</span>
                ) : null}
              </div>
              {/* Hostile agent text: rendered as an escaped JSX text node,
                  never an HTML sink. */}
              <code style={approvalCmdStyle}>{call.command}</code>
              <div style={approvalActionsStyle}>
                <button
                  type="button"
                  style={allowBtnStyle}
                  onClick={() => allowCall(call)}
                >
                  Allow
                </button>
                <button
                  type="button"
                  style={denyBtnStyle}
                  onClick={() => setDenyTarget(call)}
                >
                  Deny
                </button>
              </div>
            </div>
          ))}
          {answerNote ? <div style={answerNoteStyle}>{answerNote}</div> : null}
          {denyOpen ? (
            <input
              ref={denyRef}
              style={inputStyle}
              value={denyText}
              aria-label="deny reason"
              placeholder="why — the agent reroutes with this (↵ deny · esc cancel)"
              onChange={(ev) => setDenyText(ev.target.value)}
              onKeyDown={onDenyKeyDown}
            />
          ) : null}
        </div>
      ) : null}

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
  gap: 2,
  padding: "1px 4px 1px 8px",
  background: "#141a26",
  border: "1px solid #2d3343",
  borderRadius: 999,
  color: "#d7dae0",
  font: "11px/1 ui-monospace, monospace",
};

const tabActiveStyle: CSSProperties = {
  ...tabStyle,
  background: "#1e2636",
  borderColor: "#3b4a63",
  color: "#fff",
};

const tabSelectStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  padding: "3px 2px",
  background: "transparent",
  border: "none",
  color: "inherit",
  font: "inherit",
  cursor: "pointer",
};

const killStyle: CSSProperties = {
  background: "transparent",
  border: "none",
  color: "#8b93a7",
  cursor: "pointer",
  font: "13px/1 ui-monospace, monospace",
  padding: "2px 4px",
};

const addBarStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  marginLeft: "auto",
  flexWrap: "wrap",
};

const addStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 4,
  padding: "3px 10px",
  background: "#0b1220",
  border: "1px dashed #3b4a63",
  borderRadius: 999,
  color: "#8b93a7",
  font: "11px/1 ui-monospace, monospace",
  cursor: "pointer",
};

const tabNumStyle: CSSProperties = { color: "#8b93a7", fontWeight: 700 };

const emptyStyle: CSSProperties = {
  flex: "1 1 auto",
  display: "grid",
  placeItems: "center",
  color: "#5b6273",
};

const approvalBarStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 8,
  padding: 10,
  borderTop: "1px solid #3a1c20",
  background: "#160c0e",
};

const approvalCardStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 6,
  padding: "8px 10px",
  background: "#0b0e14",
  border: "1px solid #2d3343",
  borderRadius: 6,
};

const approvalCardTopStyle: CSSProperties = {
  ...approvalCardStyle,
  border: "1px solid #e5484d",
};

const approvalMetaStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  flexWrap: "wrap",
};

const approvalToolStyle: CSSProperties = {
  color: "#e5484d",
  fontWeight: 700,
  font: "12px/1 ui-monospace, monospace",
};

const approvalRuleStyle: CSSProperties = {
  color: "#f5a623",
  fontSize: 11,
};

const approvalHintStyle: CSSProperties = {
  marginLeft: "auto",
  color: "#5b6273",
  fontSize: 11,
  font: "11px/1 ui-monospace, monospace",
};

const approvalCmdStyle: CSSProperties = {
  display: "block",
  padding: "6px 8px",
  background: "#05070b",
  border: "1px solid #1b2130",
  borderRadius: 4,
  color: "#d7dae0",
  font: "12px/1.4 ui-monospace, monospace",
  whiteSpace: "pre-wrap",
  wordBreak: "break-all",
};

const approvalActionsStyle: CSSProperties = {
  display: "flex",
  gap: 8,
};

const allowBtnStyle: CSSProperties = {
  padding: "4px 14px",
  background: "#123524",
  border: "1px solid #30a46c",
  borderRadius: 4,
  color: "#4fd18b",
  cursor: "pointer",
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
  fontWeight: 600,
};

const denyBtnStyle: CSSProperties = {
  padding: "4px 14px",
  background: "#2a1214",
  border: "1px solid #e5484d",
  borderRadius: 4,
  color: "#ff7a7f",
  cursor: "pointer",
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
  fontWeight: 600,
};

const answerNoteStyle: CSSProperties = {
  color: "#f5a623",
  fontSize: 11,
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
