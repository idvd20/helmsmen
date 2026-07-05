// Helmsmen — one Workspace card on the Helm wall (task #10).
//
// Purely presentational: it renders a `WorkspaceCardView` (derived in
// viewModel.ts) and nothing else — no data fetching, no clocks, no
// process/git/fs. Every string it shows (branch, activity lines, the
// hostile cut log) reaches the DOM as an escaped JSX text node, never
// through an HTML sink, so agent/PTY output can never become markup.

import type { CSSProperties } from "react";
import type { HelmWorkspaceStatus } from "./api";
import type { CardBody, SessionChipView, WorkspaceCardView } from "./viewModel";

const STATUS_COLOR: Record<HelmWorkspaceStatus, string> = {
  blocked: "#e5484d",
  working: "#f5a623",
  done: "#30a46c",
  idle: "#8b93a7",
};

const MUTED = "#8b93a7";
const FAINT = "#5b6273";

export interface WorkspaceCardProps {
  card: WorkspaceCardView;
  /** Zoom to a Session — a placeholder target now; #12 takes it over. */
  onZoomSession?: (sessionId: string) => void;
}

export function WorkspaceCard({ card, onZoomSession }: WorkspaceCardProps) {
  return (
    <article style={cardStyle}>
      <header style={headerStyle}>
        {/* Attention rule: the dot never pulses (pulse is an M5 setting). */}
        <span
          aria-hidden
          style={{ ...dotStyle, background: STATUS_COLOR[card.status] }}
        />
        <span style={branchStyle}>{card.branch}</span>
        <span style={subStyle}>
          {card.projectName} ⎇ {card.baseBranch}
        </span>
        <span style={spacerStyle} />
        <span
          aria-hidden
          title="Profile"
          style={{ ...swatchStyle, background: card.profileColor }}
        />
        <span style={metaStyle}>{card.elapsedMinutes}m · $—</span>
      </header>

      <div style={bodyStyle}>
        <CardBodyView body={card.body} />
      </div>

      <footer style={footerStyle}>
        {card.chips.length === 0 ? (
          <span style={{ color: FAINT, fontSize: 11 }}>no sessions yet</span>
        ) : (
          card.chips.map((chip) => (
            <ChipButton
              key={chip.sessionId}
              chip={chip}
              onZoomSession={onZoomSession}
            />
          ))
        )}
      </footer>
    </article>
  );
}

function CardBodyView({ body }: { body: CardBody }) {
  switch (body.kind) {
    case "ask":
      return (
        <div>
          <p style={{ margin: 0, color: STATUS_COLOR.blocked }}>
            {body.prompt}
          </p>
          {body.log ? <pre style={logStyle}>{body.log}</pre> : null}
        </div>
      );
    case "activity":
      return (
        <div>
          <p style={{ margin: 0 }}>⏺ {body.lines[0]}</p>
          {body.lines.length > 1 ? (
            <p style={{ margin: "4px 0 0", color: FAINT }}>
              {body.lines.slice(1).join(" · ")}
            </p>
          ) : null}
        </div>
      );
    case "diffstat":
      return (
        <div>
          <p style={{ margin: 0 }}>
            {body.files} files{" "}
            <span style={{ color: STATUS_COLOR.done }}>+{body.added}</span>{" "}
            <span style={{ color: STATUS_COLOR.blocked }}>−{body.removed}</span>
          </p>
          <p style={{ margin: "4px 0 0", color: FAINT }}>
            {body.verify === "passed"
              ? "✓ verify passed — ready to review"
              : "no verify — look closer"}
          </p>
        </div>
      );
  }
}

function ChipButton({
  chip,
  onZoomSession,
}: {
  chip: SessionChipView;
  onZoomSession?: (sessionId: string) => void;
}) {
  return (
    <button
      type="button"
      style={chipStyle}
      title={`zoom to ${chip.label}`}
      onClick={() => onZoomSession?.(chip.sessionId)}
    >
      <span
        aria-hidden
        style={{
          ...chipDotStyle,
          background: chip.status ? STATUS_COLOR[chip.status] : MUTED,
        }}
      />
      {chip.label}
    </button>
  );
}

const cardStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  background: "#0b0e14",
  border: "1px solid #2d3343",
  borderRadius: 8,
  padding: 12,
  color: "#d7dae0",
  font: "13px/1.45 ui-sans-serif, system-ui, sans-serif",
};

const headerStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
};

const dotStyle: CSSProperties = {
  width: 8,
  height: 8,
  borderRadius: "50%",
  flex: "0 0 auto",
};

const branchStyle: CSSProperties = {
  font: "12px/1 ui-monospace, monospace",
  fontWeight: 600,
};

const subStyle: CSSProperties = { color: MUTED, fontSize: 11 };

const spacerStyle: CSSProperties = { flex: "1 1 auto" };

const swatchStyle: CSSProperties = {
  width: 10,
  height: 10,
  borderRadius: 2,
  flex: "0 0 auto",
};

const metaStyle: CSSProperties = {
  color: MUTED,
  fontSize: 11,
  fontVariantNumeric: "tabular-nums",
};

const bodyStyle: CSSProperties = {
  height: 96,
  marginTop: 10,
  overflow: "hidden",
  fontSize: 12,
};

const logStyle: CSSProperties = {
  margin: "6px 0 0",
  padding: 6,
  maxHeight: 60,
  overflow: "auto",
  background: "#05070b",
  borderRadius: 4,
  whiteSpace: "pre-wrap",
  wordBreak: "break-all",
  font: "11px/1.35 ui-monospace, monospace",
  color: MUTED,
};

const footerStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 6,
  marginTop: 10,
  paddingTop: 8,
  borderTop: "1px solid #1b2130",
};

const chipStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 5,
  padding: "2px 8px",
  background: "#141a26",
  border: "1px solid #2d3343",
  borderRadius: 999,
  color: "#d7dae0",
  font: "11px/1 ui-monospace, monospace",
  cursor: "pointer",
};

const chipDotStyle: CSSProperties = {
  width: 6,
  height: 6,
  borderRadius: "50%",
};
