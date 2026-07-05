import { useState, useEffect, useMemo, useCallback, useRef } from "react";

// ─── Helmsmen ⎈ — you hold the conn. Helm (home) → take the wheel of one
// workspace → esc back. Keyboard-first: vim keys AND arrows everywhere. ────
const T = {
  bg: "#0D1420", panel: "#131B28", panelAlt: "#182231",
  border: "#243247", borderSoft: "#1C2939",
  text: "#E8EDF5", muted: "#8B96AA", faint: "#5D6A80",
  amber: "#E8A33D", green: "#3FCF8E", red: "#F0555D", grey: "#5D6A80",
  teal: "#2DD4BF",
  mono: "'IBM Plex Mono', ui-monospace, SFMono-Regular, Menlo, monospace",
  sans: "'Inter', ui-sans-serif, system-ui, sans-serif",
};

const PROFILES = {
  bugfix:   { name: "Bugfix",    color: "#8B7CF6" },
  frontend: { name: "Frontend",  color: "#38BDF8" },
  rustcore: { name: "Rust Core", color: "#A3E635" },
  backend:  { name: "Backend",   color: "#FB7185" },
};
const STATUS = {
  working: { color: T.amber }, blocked: { color: T.red },
  done: { color: T.green }, idle: { color: T.grey },
};
const ACTIONS = [
  "Edit(src/hooks/useLease.ts)", "Bash(pnpm vitest run lease)", "Read(src/billing/proration.ts)",
  "Edit(src-tauri/src/core/state.rs)", "Bash(cargo test -p core)", "Grep(\"RuntimeKind\")",
];

const initialWorkspaces = [
  { id: "ws1", project: "rentvine-app", branch: "fix/lease-renewal-proration", profile: "bugfix",
    sessions: [{ kind: "agent", label: "claude·tmux", status: "working" }, { kind: "shell", label: "shell", status: "idle" }],
    lastAction: "Edit(src/billing/proration.ts)", elapsed: "18m", tokens: 223600, cost: 3.84, verify: null, diffstat: null },
  { id: "ws2", project: "rentvine-app", branch: "feat/tenant-portal-a11y", profile: "frontend",
    sessions: [{ kind: "agent", label: "claude·pty", status: "blocked" }, { kind: "process", label: "dev:5173", status: "working" }],
    lastAction: null, elapsed: "31m", tokens: 118900, cost: 1.92, verify: null, diffstat: null,
    ask: { tool: "Bash", cmd: "git push --force-with-lease origin feat/tenant-portal-a11y", rule: "force push", apId: "ap1" } },
  // ws3 runs on a Ship ("rig", a Linux box) — Fleet concept, M8. Flagship holds the conn.
  { id: "ws3", project: "helmsmen", branch: "feat/m3-hook-control-plane", profile: "rustcore",
    sessions: [{ kind: "agent", label: "claude·tmux@rig", status: "working" }],
    lastAction: "Bash(cargo check)", elapsed: "42m", tokens: 304300, cost: 5.61, verify: null, diffstat: null },
  { id: "ws4", project: "helmsmen", branch: "feat/helm-triage-view", profile: "frontend",
    sessions: [{ kind: "agent", label: "codex·pty", status: "done" }, { kind: "reviewer", label: "reviewer", status: "done" }],
    lastAction: null, elapsed: null, tokens: 73400, cost: 0.97, verify: "pass", diffstat: { files: 5, add: 120, del: 34 } },
  { id: "ws5", project: "chalkbeta", branch: "chore/drizzle-neon-migration", profile: "backend",
    sessions: [{ kind: "agent", label: "claude·tmux", status: "idle" }],
    lastAction: "session resumed (--resume)", elapsed: null, tokens: 14400, cost: 0.21, verify: null, diffstat: null },
];
const initialApprovals = [
  { id: "ap1", ws: "ws2", tool: "Bash", cmd: "git push --force-with-lease origin feat/tenant-portal-a11y", reason: "Rule: force push → defer", age: "2m" },
  { id: "ap2", ws: "ws1", tool: "Write", cmd: ".env.example  (+HELMSMEN_HOOK_PORT)", reason: "Rule: env-adjacent write → defer", age: "6m" },
];
const PROJECTS = [
  { id: "rentvine-app", label: "rentvine-app", base: "main" },
  { id: "helmsmen", label: "helmsmen", base: "dev" },
  { id: "chalkbeta", label: "chalkbeta", base: "main" },
];
const DIFF = [
  { t: "h", s: "src/billing/proration.ts" },
  { t: "c", s: "@@ -41,7 +41,9 @@ export function prorate(lease: Lease) {" },
  { t: " ", s: "  const days = daysInMonth(lease.start);" },
  { t: "-", s: "  const factor = lease.start.getDate() / days;" },
  { t: "+", s: "  const occupied = days - lease.start.getDate() + 1;" },
  { t: "+", s: "  const factor = occupied / days;" },
  { t: " ", s: "  return round2(lease.rent * factor);" },
  { t: "+", s: "  it('charges full month when starting on the 1st', () => {" },
  { t: "+", s: "    expect(prorate(leaseOn('2026-07-01'))).toBe(1450);" },
  { t: "+", s: "  });" },
];

function rollup(w) {
  const s = w.sessions.map((x) => x.status);
  if (s.includes("blocked")) return "blocked";
  if (s.includes("working")) return "working";
  if (s.every((x) => x === "done")) return "done";
  return "idle";
}
const fmtTok = (n) => (n >= 1000 ? (n / 1000).toFixed(0) + "k" : n);
const RANK = { blocked: 0, done: 1, working: 2, idle: 3 };

function Dot({ status, size = 8 }) {
  const c = STATUS[status].color;
  return (
    <span className="relative inline-flex shrink-0" style={{ width: size, height: size }}>
      {status === "working" && <span className="absolute h-full w-full rounded-full animate-ping" style={{ background: c, opacity: 0.3 }} />}
      <span className="relative rounded-full h-full w-full" style={{ background: c }} />
    </span>
  );
}
function Key({ k }) {
  return <kbd className="px-1 rounded" style={{ fontFamily: T.mono, fontSize: 9.5, color: T.muted, border: `1px solid ${T.border}`, background: T.panelAlt }}>{k}</kbd>;
}
function SessionChips({ sessions }) {
  return (
    <span className="flex items-center gap-1">
      {sessions.map((s, i) => (
        <span key={i} className="px-1.5 rounded flex items-center gap-1"
          style={{ fontFamily: T.mono, fontSize: 9.5, color: T.muted, border: `1px solid ${T.borderSoft}`, paddingTop: 1, paddingBottom: 1 }}>
          <span className="rounded-full" style={{ width: 4, height: 4, background: STATUS[s.status].color }} />
          {s.label}
        </span>
      ))}
    </span>
  );
}

// ─── Card: per-state content + expandable reply-in-place (`m`) ─────────────
function Card({ w, selected, expanded, onOpen, onDecide, onInbox, onExpand, onSend }) {
  const st = rollup(w);
  const prof = PROFILES[w.profile];
  const needsYou = st === "blocked";
  const inputRef = useRef(null);
  const [msg, setMsg] = useState("");
  useEffect(() => { if (expanded) { setMsg(""); setTimeout(() => inputRef.current?.focus(), 0); } }, [expanded]);

  return (
    <div className="rounded-lg overflow-hidden flex flex-col cursor-pointer" onClick={() => onOpen(w.id)}
      style={{
        background: T.panel,
        border: `1px solid ${needsYou ? T.red + "88" : selected ? T.muted : T.border}`,
        borderLeft: `3px solid ${prof.color}`,
        boxShadow: selected ? `0 0 0 1px ${T.muted}` : "none",
        opacity: st === "idle" ? 0.75 : 1,
      }}>
      <div className="px-3 pt-2 pb-1.5 flex items-center gap-2">
        <Dot status={st} />
        <span className="truncate flex-1" style={{ fontFamily: T.mono, fontSize: 12, color: T.text }}>{w.branch}</span>
        <span style={{ fontFamily: T.sans, fontSize: 9.5, color: T.faint }}>{w.project}</span>
        <button onClick={(e) => { e.stopPropagation(); onExpand(w.id); }} title="Message the agent (m)"
          style={{ fontFamily: T.mono, fontSize: 11, color: expanded ? T.teal : T.faint }}>✉</button>
      </div>

      {needsYou && w.ask && (
        <div className="mx-3 mb-2 px-2.5 py-2 rounded" onClick={(e) => e.stopPropagation()}
          style={{ background: T.bg, border: `1px solid ${T.red}44` }}>
          <div style={{ fontFamily: T.mono, fontSize: 9.5, color: T.red }}>{w.ask.tool} · {w.ask.rule}</div>
          <div className="break-all" style={{ fontFamily: T.mono, fontSize: 11, color: T.text, lineHeight: 1.5 }}>{w.ask.cmd}</div>
          <div className="mt-2 flex gap-1.5 items-center">
            <button onClick={() => onDecide(w.ask.apId, true)} className="px-2.5 py-0.5 rounded"
              style={{ fontFamily: T.sans, fontSize: 11, fontWeight: 600, color: T.bg, background: T.green }}>Allow <Key k="a" /></button>
            <button onClick={() => onDecide(w.ask.apId, false)} className="px-2.5 py-0.5 rounded"
              style={{ fontFamily: T.sans, fontSize: 11, color: T.red, border: `1px solid ${T.red}55` }}>Deny <Key k="x" /></button>
            <button onClick={onInbox} className="px-2 py-0.5 rounded ml-auto" style={{ fontFamily: T.sans, fontSize: 10.5, color: T.faint }}>inbox →</button>
          </div>
        </div>
      )}
      {st === "working" && (
        <div className="px-3 pb-2 flex items-baseline gap-2">
          <span className="truncate" style={{ fontFamily: T.mono, fontSize: 10.5, color: T.muted }}>{w.lastAction}</span>
          <span className="ml-auto shrink-0" style={{ fontFamily: T.mono, fontSize: 10, color: T.faint }}>{w.elapsed}</span>
        </div>
      )}
      {st === "done" && (
        <div className="px-3 pb-2 flex items-center gap-2.5" style={{ fontFamily: T.mono, fontSize: 11 }}>
          <span style={{ color: T.muted }}>{w.diffstat.files} files</span>
          <span style={{ color: T.green }}>+{w.diffstat.add}</span>
          <span style={{ color: T.red }}>−{w.diffstat.del}</span>
          {w.verify === "pass" && <span style={{ color: T.green, fontSize: 10 }}>✓ verify</span>}
        </div>
      )}

      {/* Expandable reply-in-place: steer without leaving the Helm */}
      {expanded && (
        <div className="mx-3 mb-2" onClick={(e) => e.stopPropagation()}>
          <input ref={inputRef} value={msg} onChange={(e) => setMsg(e.target.value)}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter" && msg.trim()) { onSend(w.id, msg.trim()); setMsg(""); }
              if (e.key === "Escape") onExpand(null);
            }}
            placeholder={`Message ${w.sessions[0].label} → PTY  (enter send · esc close)`}
            className="w-full px-2.5 py-1.5 rounded outline-none"
            style={{ background: T.bg, border: `1px solid ${T.teal}55`, fontFamily: T.mono, fontSize: 11, color: T.text }} />
        </div>
      )}

      <div className="px-3 py-1.5 mt-auto flex items-center gap-2" style={{ borderTop: `1px solid ${T.borderSoft}` }}>
        <SessionChips sessions={w.sessions} />
        <span className="ml-auto" style={{ fontFamily: T.mono, fontSize: 9.5, color: T.faint }}>{fmtTok(w.tokens)} · ${w.cost.toFixed(2)}</span>
      </div>
    </div>
  );
}

// ─── Helm (home dashboard): triage sections ─────────────────────────────────
function Helm({ ordered, selectedId, expandedId, onOpen, onDecide, onInbox, onExpand, onSend }) {
  const by = (s) => ordered.filter((w) => rollup(w) === s);
  const sections = [
    ["Needs you", T.red, by("blocked")], ["To review", T.green, by("done")],
    ["Working", T.amber, by("working")], ["Idle", T.faint, by("idle")],
  ];
  return (
    <div className="flex-1 overflow-y-auto p-4">
      {sections.map(([label, color, list]) => list.length > 0 && (
        <div key={label} className="mb-5">
          <div className="flex items-center gap-2 mb-2">
            <span style={{ fontFamily: T.sans, fontSize: 10.5, fontWeight: 600, letterSpacing: 0.8, color, textTransform: "uppercase" }}>{label}</span>
            <span style={{ fontFamily: T.mono, fontSize: 10, color: T.faint }}>{list.length}</span>
            <div className="flex-1" style={{ height: 1, background: T.borderSoft }} />
          </div>
          {label === "Idle" ? (
            <div className="flex flex-col gap-1.5">
              {list.map((w) => (
                <button key={w.id} onClick={() => onOpen(w.id)} className="w-full flex items-center gap-2.5 px-3 py-1.5 rounded"
                  style={{ background: selectedId === w.id ? T.panelAlt : T.panel, border: `1px solid ${selectedId === w.id ? T.muted : T.borderSoft}`, borderLeft: `3px solid ${PROFILES[w.profile].color}`, opacity: 0.8 }}>
                  <Dot status="idle" size={6} />
                  <span style={{ fontFamily: T.mono, fontSize: 11, color: T.muted }}>{w.branch}</span>
                  <span style={{ fontFamily: T.sans, fontSize: 9.5, color: T.faint }}>{w.project}</span>
                  <span className="ml-auto" style={{ fontFamily: T.mono, fontSize: 10, color: T.faint }}>{w.lastAction}</span>
                </button>
              ))}
            </div>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
              {list.map((w) => (
                <Card key={w.id} w={w} selected={selectedId === w.id} expanded={expandedId === w.id}
                  onOpen={onOpen} onDecide={onDecide} onInbox={onInbox} onExpand={onExpand} onSend={onSend} />
              ))}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

// ─── Workspace (take the wheel) ──────────────────────────────────────────────
function Workspace({ w, sess, setSess, rtab, setRtab, onInbox }) {
  const st = rollup(w);
  const prof = PROFILES[w.profile];
  const cur = w.sessions[Math.min(sess, w.sessions.length - 1)];
  return (
    <div className="flex-1 flex min-h-0">
      <section className="flex-1 flex flex-col min-w-0" style={{ borderRight: `1px solid ${T.border}` }}>
        <div className="flex items-center gap-1 px-3 shrink-0" style={{ height: 38, borderBottom: `1px solid ${T.border}` }}>
          {w.sessions.map((s, i) => (
            <button key={i} onClick={() => setSess(i)} className="px-2.5 py-1 rounded flex items-center gap-1.5"
              style={{ fontFamily: T.mono, fontSize: 11, color: i === sess ? T.text : T.faint, background: i === sess ? T.panelAlt : "transparent", border: `1px solid ${i === sess ? T.border : "transparent"}` }}>
              <span className="rounded-full" style={{ width: 5, height: 5, background: STATUS[s.status].color }} />
              {s.label} <Key k={i + 1} />
            </button>
          ))}
          <button className="px-2 py-1" style={{ fontFamily: T.mono, fontSize: 11, color: T.faint }}>＋</button>
          <span className="ml-auto px-1.5 rounded" style={{ fontFamily: T.sans, fontSize: 9.5, color: prof.color, border: `1px solid ${prof.color}44` }}>{prof.name}</span>
        </div>
        <div className="flex-1 overflow-y-auto px-4 py-3" style={{ fontFamily: T.mono, fontSize: 12, lineHeight: 1.9 }}>
          {cur.kind === "agent" && (<>
            <div style={{ color: T.faint }}>helmsmen-{w.id}-0 · {cur.label}</div>
            <div className="mt-1" style={{ color: T.muted }}>&gt; Fix the proration bug where mid-month leases are charged from day 0. Add a regression test.</div>
            <div style={{ color: T.text }}>⏺ Read(src/billing/proration.ts)</div>
            <div style={{ color: T.text }}>⏺ Edit(src/billing/proration.ts)</div>
            <div style={{ color: T.muted }}>  ⎿ Updated 2 hunks</div>
            {st === "blocked" && w.ask && (
              <div className="mt-3 px-3 py-2 rounded flex items-center gap-3" style={{ border: `1px solid ${T.red}55`, background: T.red + "10" }}>
                <span style={{ color: T.red, fontSize: 11 }}>⏸ {w.ask.tool}: {w.ask.cmd}</span>
                <button onClick={onInbox} className="ml-auto px-2 py-0.5 rounded shrink-0" style={{ fontFamily: T.sans, fontSize: 11, color: T.red, border: `1px solid ${T.red}55` }}>inbox</button>
              </div>
            )}
            {st === "working" && <div className="animate-pulse" style={{ color: T.amber }}>▋</div>}
          </>)}
          {cur.kind === "shell" && (<>
            <div style={{ color: T.faint }}>your shell · same worktree</div>
            <div style={{ color: T.text }}>$ git log --oneline -3</div>
            <div style={{ color: T.muted }}>a41f2c9 wip: proration factor<br />8de11b0 checkpoint: failing test<br />31c07aa branch off main</div>
            <div style={{ color: T.text }}>$ ▋</div>
          </>)}
          {cur.kind === "process" && (<>
            <div style={{ color: T.faint }}>dev server · vite</div>
            <div style={{ color: T.green }}>➜ Local: http://localhost:5173/</div>
            <div style={{ color: T.muted }}>hmr update /src/components/PortalNav.tsx</div>
          </>)}
          {cur.kind === "reviewer" && (<>
            <div style={{ color: T.faint }}>reviewer agent · read-only pass</div>
            <div style={{ color: T.text }}>⏺ Read(diff vs dev)</div>
            <div style={{ color: T.muted }}>Verdict: LGTM. One nit — Helm.tsx:88 duplicated key prop.</div>
          </>)}
        </div>
        <div className="px-4 py-2.5 shrink-0" style={{ borderTop: `1px solid ${T.border}` }}>
          <input placeholder="Message this session… (straight to the PTY)" className="w-full px-3 py-2 rounded outline-none"
            style={{ background: T.panelAlt, border: `1px solid ${T.border}`, fontFamily: T.mono, fontSize: 12, color: T.text }} />
        </div>
      </section>
      <section className="flex flex-col min-w-0 shrink-0" style={{ width: "42%" }}>
        <div className="flex items-center gap-1 px-3 shrink-0" style={{ height: 38, borderBottom: `1px solid ${T.border}` }}>
          {[["diff", "d"], ["preview", "p"], ["verify", "v"]].map(([t, k]) => (
            <button key={t} onClick={() => setRtab(t)} className="px-2.5 py-1 rounded capitalize flex items-center gap-1.5"
              style={{ fontFamily: T.sans, fontSize: 11.5, color: rtab === t ? T.text : T.faint, background: rtab === t ? T.panelAlt : "transparent" }}>
              {t} <Key k={k} />
            </button>
          ))}
          <button className="ml-auto px-2.5 py-1 rounded" style={{ fontFamily: T.sans, fontSize: 11.5, fontWeight: 600, color: T.bg, background: T.green }}>Open PR</button>
        </div>
        <div className="flex-1 overflow-y-auto">
          {rtab === "diff" && DIFF.map((l, i) => (
            <div key={i} className="px-4" style={{
              fontFamily: T.mono, fontSize: 11.5, lineHeight: 1.85,
              color: l.t === "+" ? T.green : l.t === "-" ? T.red : l.t === "h" ? T.text : l.t === "c" ? "#7AA2F7" : T.muted,
              background: l.t === "+" ? T.green + "0D" : l.t === "-" ? T.red + "0D" : l.t === "h" ? T.panelAlt : "transparent",
              fontWeight: l.t === "h" ? 600 : 400, paddingTop: l.t === "h" ? 5 : 0, paddingBottom: l.t === "h" ? 5 : 0,
            }}>{l.t === "h" || l.t === "c" ? l.s : l.t + " " + l.s}</div>
          ))}
          {rtab === "preview" && <div className="h-full flex items-center justify-center" style={{ color: T.faint, fontFamily: T.mono, fontSize: 12 }}>iframe → http://localhost:5173 (sandboxed)</div>}
          {rtab === "verify" && (
            <div className="px-4 py-3" style={{ fontFamily: T.mono, fontSize: 11.5, lineHeight: 2 }}>
              <div style={{ color: T.muted }}>$ pnpm vitest run billing</div>
              <div style={{ color: T.green }}>✓ prorate charges partial month</div>
              <div style={{ color: T.green }}>✓ full month when starting on the 1st</div>
              <div style={{ color: T.text }}>Tests 2 passed · 1.4s</div>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

// ─── Palette ─────────────────────────────────────────────────────────────────
function Palette({ open, workspaces, onClose, onOpen, run }) {
  const [q, setQ] = useState("");
  const ref = useRef(null);
  useEffect(() => { if (open) { setQ(""); setTimeout(() => ref.current?.focus(), 0); } }, [open]);
  if (!open) return null;
  const cmds = [
    { label: "⎈ Go to Helm", hint: "h", act: () => run("helm") },
    ...workspaces.map((w) => ({ label: `→ ${w.project} / ${w.branch}`, hint: rollup(w), act: () => onOpen(w.id) })),
    { label: "New workspace…", hint: "n", act: () => run("new") },
    { label: "Toggle sidebar", hint: "s", act: () => run("sidebar") },
    { label: "Open approval inbox", hint: "i", act: () => run("inbox") },
  ].filter((c) => c.label.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="absolute inset-0 z-30 flex items-start justify-center pt-24" style={{ background: "#0D142099" }} onClick={onClose}>
      <div className="rounded-lg overflow-hidden" style={{ width: 460, background: T.panel, border: `1px solid ${T.border}`, boxShadow: "0 16px 48px #00000088" }} onClick={(e) => e.stopPropagation()}>
        <input ref={ref} value={q} onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Enter" && cmds[0]) { cmds[0].act(); onClose(); } if (e.key === "Escape") onClose(); }}
          placeholder="Jump to workspace or run a command…"
          className="w-full px-4 py-3 outline-none" style={{ background: "transparent", fontFamily: T.mono, fontSize: 13, color: T.text, borderBottom: `1px solid ${T.border}` }} />
        <div className="max-h-64 overflow-y-auto py-1">
          {cmds.map((c, i) => (
            <button key={i} onClick={() => { c.act(); onClose(); }} className="w-full text-left px-4 py-1.5 flex items-center"
              style={{ fontFamily: T.mono, fontSize: 11.5, color: i === 0 ? T.text : T.muted, background: i === 0 ? T.panelAlt : "transparent" }}>
              <span className="truncate">{c.label}</span>
              <span className="ml-auto" style={{ fontSize: 9.5, color: T.faint }}>{c.hint}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

// ─── Inbox: keyboard selection + bulk actions ───────────────────────────────
function Inbox({ open, approvals, workspaces, sel, confirmAll, onDecide, onDecideAll, onClose }) {
  if (!open) return null;
  return (
    <div className="absolute inset-y-0 right-0 flex flex-col z-20 shadow-2xl" style={{ width: 380, background: T.panel, borderLeft: `1px solid ${T.border}` }}>
      <div className="flex items-center gap-2 px-4 shrink-0" style={{ height: 42, borderBottom: `1px solid ${T.border}` }}>
        <span style={{ fontFamily: T.sans, fontSize: 12.5, fontWeight: 600 }}>Approval Inbox</span>
        <span className="px-1.5 rounded-full" style={{ fontFamily: T.mono, fontSize: 10, background: T.red + "22", color: T.red }}>{approvals.length}</span>
        {approvals.length > 0 && (
          <span className="ml-auto flex items-center gap-1.5">
            <button onClick={() => onDecideAll(true)} className="px-2 py-0.5 rounded"
              style={{ fontFamily: T.sans, fontSize: 10.5, fontWeight: 600, color: confirmAll ? T.bg : T.green, background: confirmAll ? T.green : "transparent", border: `1px solid ${T.green}66` }}>
              {confirmAll ? "Confirm all? (A)" : "Allow all (A)"}
            </button>
            <button onClick={() => onDecideAll(false)} className="px-2 py-0.5 rounded"
              style={{ fontFamily: T.sans, fontSize: 10.5, color: T.red, border: `1px solid ${T.red}55` }}>Deny all (X)</button>
          </span>
        )}
        <button onClick={onClose} className={approvals.length ? "" : "ml-auto"} style={{ color: T.faint }}>✕</button>
      </div>
      <div className="flex-1 overflow-y-auto p-3 flex flex-col gap-2.5">
        {approvals.length === 0 && <div className="text-center mt-8" style={{ fontFamily: T.sans, fontSize: 11.5, color: T.faint }}>Clear. Deferred calls land here<br />with full context preserved.</div>}
        {approvals.map((a, i) => {
          const w = workspaces.find((x) => x.id === a.ws);
          const isSel = i === sel;
          return (
            <div key={a.id} className="rounded-lg p-2.5"
              style={{ background: T.panelAlt, border: `1px solid ${isSel ? T.muted : T.border}`, borderLeft: `3px solid ${PROFILES[w.profile].color}`, boxShadow: isSel ? `0 0 0 1px ${T.muted}` : "none" }}>
              <div className="flex items-center gap-2">
                <span className="truncate" style={{ fontFamily: T.mono, fontSize: 10.5, color: T.muted }}>{w.branch}</span>
                <span className="ml-auto shrink-0" style={{ fontFamily: T.mono, fontSize: 9.5, color: T.faint }}>{a.age}</span>
              </div>
              <div className="mt-1.5 px-2 py-1.5 rounded" style={{ background: T.bg }}>
                <span style={{ fontFamily: T.mono, fontSize: 9.5, color: T.amber }}>{a.tool} </span>
                <span className="break-all" style={{ fontFamily: T.mono, fontSize: 11, color: T.text }}>{a.cmd}</span>
              </div>
              <div className="mt-1" style={{ fontFamily: T.sans, fontSize: 10, color: T.faint }}>{a.reason}</div>
              <div className="mt-2 flex gap-1.5">
                <button onClick={() => onDecide(a.id, true)} className="flex-1 py-1 rounded" style={{ fontFamily: T.sans, fontSize: 11, fontWeight: 600, color: T.bg, background: T.green }}>Allow{isSel && <> <Key k="a" /></>}</button>
                <button onClick={() => onDecide(a.id, false)} className="flex-1 py-1 rounded" style={{ fontFamily: T.sans, fontSize: 11, color: T.red, border: `1px solid ${T.red}55` }}>Deny{isSel && <> <Key k="x" /></>}</button>
                <button className="px-2 py-1 rounded" style={{ fontFamily: T.sans, fontSize: 11, color: T.muted, border: `1px solid ${T.border}` }}>Edit</button>
              </div>
            </div>
          );
        })}
      </div>
      <div className="px-4 py-2 shrink-0 flex items-center gap-2.5" style={{ borderTop: `1px solid ${T.borderSoft}`, fontFamily: T.mono, fontSize: 9.5, color: T.faint }}>
        <span><Key k="↑" /><Key k="↓" /> select</span><span><Key k="a" /> allow</span><span><Key k="x" /> deny</span>
        <span><Key k="A" /> allow all</span><span><Key k="X" /> deny all</span>
      </div>
    </div>
  );
}

// ─── App ────────────────────────────────────────────────────────────────────
export default function HelmsmenPrototype() {
  const [openWs, setOpenWs] = useState(null);          // null = Helm, id = at the wheel
  const [selectedId, setSelectedId] = useState("ws2");
  const [expandedId, setExpandedId] = useState(null);  // card reply-in-place
  const [sess, setSess] = useState(0);
  const [rtab, setRtab] = useState("diff");
  const [sidebar, setSidebar] = useState(true);
  const [inboxOpen, setInboxOpen] = useState(false);
  const [inboxSel, setInboxSel] = useState(0);
  const [confirmAll, setConfirmAll] = useState(false); // two-press guard on Allow all
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [workspaces, setWorkspaces] = useState(initialWorkspaces);
  const [approvals, setApprovals] = useState(initialApprovals);

  useEffect(() => {
    let t = 0;
    const id = setInterval(() => {
      t += 1;
      setWorkspaces((ws) => ws.map((w) => rollup(w) === "working"
        ? { ...w, lastAction: ACTIONS[(t + w.id.charCodeAt(2)) % ACTIONS.length], tokens: w.tokens + 1400, cost: w.cost + 0.02 } : w));
    }, 2600);
    return () => clearInterval(id);
  }, []);

  const ordered = useMemo(() => [...workspaces].sort((a, b) => RANK[rollup(a)] - RANK[rollup(b)]), [workspaces]);

  const decide = useCallback((apId, allow) => {
    setApprovals((as) => {
      const ap = as.find((a) => a.id === apId);
      if (!ap) return as;
      setWorkspaces((ws) => ws.map((w) => {
        if (w.id !== ap.ws) return w;
        const remaining = as.some((a) => a.id !== apId && a.ws === w.id);
        return { ...w, ask: remaining ? w.ask : null,
          lastAction: allow ? "▶ resumed — approved" : "↺ denied — rerouting",
          sessions: w.sessions.map((s) => s.status === "blocked" && !remaining ? { ...s, status: "working" } : s) };
      }));
      setInboxSel((i) => Math.max(0, Math.min(i, as.length - 2)));
      return as.filter((a) => a.id !== apId);
    });
  }, []);

  // Bulk: Deny all is immediate; Allow all needs a second press/click.
  const decideAll = useCallback((allow) => {
    if (allow && !confirmAll) { setConfirmAll(true); return; }
    setConfirmAll(false);
    setApprovals((as) => {
      setWorkspaces((ws) => ws.map((w) => as.some((a) => a.ws === w.id)
        ? { ...w, ask: null, lastAction: allow ? "▶ resumed — approved (bulk)" : "↺ denied — rerouting (bulk)",
            sessions: w.sessions.map((s) => s.status === "blocked" ? { ...s, status: "working" } : s) } : w));
      return [];
    });
    setInboxSel(0);
  }, [confirmAll]);

  const openWorkspace = useCallback((id) => { setOpenWs(id); setSelectedId(id); setSess(0); setRtab("diff"); setExpandedId(null); }, []);
  const sendToAgent = useCallback((id, msg) => {
    setWorkspaces((ws) => ws.map((w) => w.id === id ? { ...w, lastAction: `you ▸ "${msg}"` } : w));
    setExpandedId(null);
  }, []);

  // ── One keymap. Vim keys AND arrows. Contexts: helm / workspace / inbox. ──
  useEffect(() => {
    const onKey = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") { e.preventDefault(); setPaletteOpen((o) => !o); return; }
      if (["INPUT", "TEXTAREA"].includes(e.target.tagName)) return;
      if (paletteOpen) return;
      const down = e.key === "j" || e.key === "ArrowDown";
      const up = e.key === "k" || e.key === "ArrowUp";
      const prev = e.key === "[" || e.key === "ArrowLeft";
      const next = e.key === "]" || e.key === "ArrowRight";
      if (down || up || prev || next) e.preventDefault();

      if (e.key === "Escape") {
        if (inboxOpen) { setInboxOpen(false); setConfirmAll(false); }
        else if (expandedId) setExpandedId(null);
        else setOpenWs(null);
        return;
      }
      if (e.key === "h") { setOpenWs(null); return; }          // ⎈ Helm nav
      if (e.key === "i") { setInboxOpen((o) => !o); setConfirmAll(false); return; }
      if (e.key === "s") { setSidebar((o) => !o); return; }

      if (inboxOpen) {                                          // inbox keymap
        if (down) setInboxSel((i) => Math.min(i + 1, approvals.length - 1));
        if (up) setInboxSel((i) => Math.max(i - 1, 0));
        if (e.key === "a" && approvals[inboxSel]) decide(approvals[inboxSel].id, true);
        if (e.key === "x" && approvals[inboxSel]) decide(approvals[inboxSel].id, false);
        if (e.key === "A") decideAll(true);
        if (e.key === "X") decideAll(false);
        return;
      }

      if (openWs === null) {                                    // helm keymap
        const idx = ordered.findIndex((w) => w.id === selectedId);
        if (down) setSelectedId(ordered[Math.min(idx + 1, ordered.length - 1)]?.id);
        if (up) setSelectedId(ordered[Math.max(idx - 1, 0)]?.id);
        if (e.key === "Enter" && selectedId) openWorkspace(selectedId);
        if (e.key === "m" && selectedId) setExpandedId((x) => (x === selectedId ? null : selectedId));
        if (e.key === "a" || e.key === "x") {
          const w = workspaces.find((x) => x.id === selectedId);
          if (w?.ask) decide(w.ask.apId, e.key === "a");
        }
      } else {                                                  // workspace keymap
        const w = workspaces.find((x) => x.id === openWs);
        const idx = ordered.findIndex((x) => x.id === openWs);
        if (next) openWorkspace(ordered[(idx + 1) % ordered.length].id);
        if (prev) openWorkspace(ordered[(idx - 1 + ordered.length) % ordered.length].id);
        if (/^[1-9]$/.test(e.key)) setSess(Math.min(+e.key - 1, w.sessions.length - 1));
        if (e.key === "d") setRtab("diff");
        if (e.key === "p") setRtab("preview");
        if (e.key === "v") setRtab("verify");
        if (e.key === "a" || e.key === "x") { if (w?.ask) decide(w.ask.apId, e.key === "a"); }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openWs, ordered, selectedId, workspaces, decide, decideAll, inboxOpen, inboxSel, approvals, paletteOpen, expandedId, openWorkspace]);

  const focused = workspaces.find((w) => w.id === openWs);
  const counts = {
    blocked: workspaces.filter((w) => rollup(w) === "blocked").length,
    working: workspaces.filter((w) => rollup(w) === "working").length,
    done: workspaces.filter((w) => rollup(w) === "done").length,
  };
  const totalCost = workspaces.reduce((s, w) => s + w.cost, 0);

  return (
    <div className="w-full h-screen flex flex-col relative overflow-hidden" style={{ background: T.bg, color: T.text }}>
      <style>{`
        @import url('https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&family=Inter:wght@400;500;600&display=swap');
        ::-webkit-scrollbar { width: 8px; } ::-webkit-scrollbar-thumb { background: ${T.border}; border-radius: 4px; }
        button { cursor: pointer; }
        @media (prefers-reduced-motion: reduce) { .animate-ping, .animate-pulse { animation: none; } }
      `}</style>

      {/* Header: ⎈ Helm nav + breadcrumb + summary strip */}
      <header className="flex items-center gap-3 px-4 shrink-0" style={{ height: 44, borderBottom: `1px solid ${T.border}`, background: T.panel }}>
        <div className="flex items-center gap-2">
          <div className="w-5 h-5 rounded flex items-center justify-center" style={{ background: `linear-gradient(135deg,${T.teal},#38BDF8)`, fontFamily: T.mono, fontSize: 12, color: T.bg, fontWeight: 700 }}>⎈</div>
          <span style={{ fontFamily: T.mono, fontSize: 13, fontWeight: 600, letterSpacing: 1 }}>helmsmen</span>
        </div>
        <nav className="flex items-center gap-1">
          <button onClick={() => setOpenWs(null)} className="px-2.5 py-1 rounded-md flex items-center gap-1.5"
            style={{ fontFamily: T.sans, fontSize: 11.5, fontWeight: 500, color: openWs === null ? T.teal : T.muted,
              background: openWs === null ? T.teal + "14" : "transparent", border: `1px solid ${openWs === null ? T.teal + "55" : T.border}` }}>
            ⎈ Helm <Key k="h" />
          </button>
          {focused && (
            <span className="flex items-center gap-2 truncate px-1" style={{ fontFamily: T.mono, fontSize: 11.5 }}>
              <span style={{ color: T.faint }}>›</span>
              <span style={{ color: T.text }}>{focused.project} / {focused.branch}</span>
            </span>
          )}
        </nav>
        <div className="ml-auto flex items-center gap-4" style={{ fontFamily: T.mono, fontSize: 11 }}>
          <span>
            <span style={{ color: counts.blocked ? T.red : T.faint }}>{counts.blocked} need you</span>
            <span style={{ color: T.faint }}> · </span>
            <span style={{ color: T.muted }}>{counts.working} working · {counts.done} to review</span>
            <span style={{ color: T.faint }}> · </span>
            <span style={{ color: T.text }}>${totalCost.toFixed(2)}</span>
          </span>
          <button onClick={() => setInboxOpen((o) => !o)} className="px-3 py-1 rounded-md"
            style={{ fontFamily: T.sans, fontSize: 11.5, fontWeight: 500, color: approvals.length ? T.red : T.muted,
              border: `1px solid ${approvals.length ? T.red + "66" : T.border}`, background: approvals.length ? T.red + "10" : "transparent" }}>
            Inbox{approvals.length ? ` · ${approvals.length}` : ""}
          </button>
        </div>
      </header>

      <div className="flex-1 flex min-h-0">
        {sidebar ? (
          <aside className="flex flex-col shrink-0 overflow-y-auto" style={{ width: 232, background: T.panel, borderRight: `1px solid ${T.border}` }}>
            {PROJECTS.map((p) => (
              <div key={p.id} className="mb-1">
                <div className="px-4 pt-3 pb-1 flex items-center justify-between">
                  <span style={{ fontFamily: T.sans, fontSize: 10.5, fontWeight: 600, color: T.muted, letterSpacing: 0.6 }}>{p.label}</span>
                  <span style={{ fontFamily: T.mono, fontSize: 9.5, color: T.faint }}>⎇ {p.base}</span>
                </div>
                {workspaces.filter((w) => w.project === p.id).map((w) => (
                  <button key={w.id} onClick={() => openWorkspace(w.id)} className="w-full text-left px-3 py-1.5 flex items-center gap-2"
                    style={{ background: openWs === w.id ? T.panelAlt : "transparent", borderLeft: `2px solid ${openWs === w.id ? PROFILES[w.profile].color : "transparent"}` }}>
                    <Dot status={rollup(w)} size={7} />
                    <span className="truncate flex-1" style={{ fontFamily: T.mono, fontSize: 11, color: openWs === w.id ? T.text : T.muted }}>{w.branch}</span>
                    {w.sessions.length > 1 && <span style={{ fontFamily: T.mono, fontSize: 9, color: T.faint }}>×{w.sessions.length}</span>}
                  </button>
                ))}
              </div>
            ))}
          </aside>
        ) : (
          <aside className="flex flex-col items-center gap-2 pt-3 shrink-0" style={{ width: 36, background: T.panel, borderRight: `1px solid ${T.border}` }}>
            {PROJECTS.map((p) => {
              const worst = workspaces.filter((w) => w.project === p.id).map(rollup).sort((a, b) => RANK[a] - RANK[b])[0];
              return <span key={p.id} title={p.label}><Dot status={worst || "idle"} size={8} /></span>;
            })}
          </aside>
        )}

        <main className="flex-1 flex flex-col min-w-0 relative">
          {openWs === null
            ? <Helm ordered={ordered} selectedId={selectedId} expandedId={expandedId} onOpen={openWorkspace}
                onDecide={decide} onInbox={() => setInboxOpen(true)} onExpand={setExpandedId} onSend={sendToAgent} />
            : <Workspace w={focused} sess={sess} setSess={setSess} rtab={rtab} setRtab={setRtab} onInbox={() => setInboxOpen(true)} />}
          <Inbox open={inboxOpen} approvals={approvals} workspaces={workspaces} sel={inboxSel} confirmAll={confirmAll}
            onDecide={decide} onDecideAll={decideAll} onClose={() => { setInboxOpen(false); setConfirmAll(false); }} />
          <Palette open={paletteOpen} workspaces={workspaces} onClose={() => setPaletteOpen(false)} onOpen={openWorkspace}
            run={(c) => { if (c === "helm") setOpenWs(null); if (c === "sidebar") setSidebar((o) => !o); if (c === "inbox") setInboxOpen(true); }} />
        </main>
      </div>

      {/* Statusline: context-sensitive keymap (vim + arrows) */}
      <footer className="flex items-center gap-3 px-4 shrink-0" style={{ height: 26, borderTop: `1px solid ${T.border}`, background: T.panel, fontFamily: T.mono, fontSize: 10, color: T.faint }}>
        <span style={{ color: T.teal }}>⎈ {inboxOpen ? "inbox" : openWs ? "workspace" : "helm"}</span>
        {inboxOpen ? (
          <span className="flex items-center gap-2.5">
            <span><Key k="j/↓" /><Key k="k/↑" /> select</span><span><Key k="a" /> allow</span><span><Key k="x" /> deny</span>
            <span><Key k="A" /> all</span><span><Key k="X" /> all</span>
          </span>
        ) : openWs === null ? (
          <span className="flex items-center gap-2.5">
            <span><Key k="j/↓" /><Key k="k/↑" /> select</span><span><Key k="↵" /> open</span>
            <span><Key k="m" /> message</span><span><Key k="a" /> allow</span><span><Key k="x" /> deny</span>
          </span>
        ) : (
          <span className="flex items-center gap-2.5">
            <span><Key k="esc" /> helm</span><span><Key k="[/←" /><Key k="]/→" /> workspace</span>
            <span><Key k="1" />…<Key k="9" /> session</span><span><Key k="d" /><Key k="p" /><Key k="v" /> panel</span>
          </span>
        )}
        <span className="ml-auto flex items-center gap-2.5">
          <span><Key k="⌘K" /> palette</span><span><Key k="h" /> helm</span><span><Key k="i" /> inbox</span><span><Key k="s" /> sidebar</span>
        </span>
      </footer>
    </div>
  );
}
