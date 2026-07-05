// Helmsmen — the Helm wall (task #10), the home view.
//
// One responsive, rank-sorted wall across all Projects with a header
// count strip. Purely presentational: it renders the `WallView` derived
// in viewModel.ts (rank sort, header counts, status rollup all live
// there as tested pure functions) and delegates zoom to a callback.
// Flat by default — grouping / filters / repo picker arrive at #14, so
// no sidebar and no section headers here.

import type { CSSProperties } from "react";
import type { WallView } from "./viewModel";
import { WorkspaceCard } from "./WorkspaceCard";

export interface HelmProps {
  wall: WallView;
  /** Zoom to a Session (chip click). Placeholder target now; #12 owns
   * the zoom view and takes this over. */
  onZoomSession?: (sessionId: string) => void;
}

export function Helm({ wall, onZoomSession }: HelmProps) {
  const { counts, cards } = wall;
  return (
    <div style={wallStyle}>
      <header style={topBarStyle}>
        <span style={wordmarkStyle}>⎈ helmsmen</span>
        <span
          style={{
            ...countsStyle,
            color: counts.needsAttention ? "#e5484d" : "#8b93a7",
          }}
        >
          <span style={numStyle}>{counts.needsYou}</span> need you
          {" · "}
          <span style={numStyle}>{counts.working}</span> working
          {" · "}
          <span style={numStyle}>{counts.toReview}</span> to review
          {/* $total cost slot arrives with M6 */}
        </span>
      </header>

      {cards.length === 0 ? (
        <p style={emptyStyle}>nothing here — cut a Workspace to begin</p>
      ) : (
        <div style={gridStyle}>
          {cards.map((card) => (
            <WorkspaceCard
              key={card.workspaceId}
              card={card}
              onZoomSession={onZoomSession}
            />
          ))}
        </div>
      )}
    </div>
  );
}

const wallStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  height: "100%",
  minHeight: 0,
  background: "#070a0f",
  color: "#d7dae0",
  font: "13px/1.45 ui-sans-serif, system-ui, sans-serif",
};

const topBarStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 16,
  padding: "12px 16px",
  borderBottom: "1px solid #1b2130",
};

const wordmarkStyle: CSSProperties = {
  fontWeight: 700,
  letterSpacing: 0.3,
};

const countsStyle: CSSProperties = {
  fontSize: 12,
  fontVariantNumeric: "tabular-nums",
};

const numStyle: CSSProperties = { fontWeight: 700 };

const gridStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(340px, 1fr))",
  gap: 12,
  padding: 16,
  overflow: "auto",
};

const emptyStyle: CSSProperties = {
  margin: "auto",
  color: "#5b6273",
};
