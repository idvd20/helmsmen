// Helmsmen — the Helm wall (task #10) + its scoping controls (task #14).
//
// One responsive, rank-sorted wall across all Projects with a header count
// strip. #14 adds the Helm's scoping controls on top of the same derived
// view-model: status filter tabs (`f`), flat/Project grouping (`g`), and a
// repo picker (`r`) that scopes the wall to one Project. No sidebar (PRD).
//
// Still render-pure: every projection (rank sort, header counts, status
// rollup, filter/group/scope) is a tested pure function in viewModel.ts;
// this component only holds the active filter/group/scope UI state, maps
// `f`/`g`/`r`/esc through the pure `mapHelmWallKey`, and renders what the
// derivations return. Every dynamic string (repo name, branch, activity)
// reaches the DOM as an escaped JSX text node — never an HTML sink.

import type { CSSProperties } from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { HelmProject, HelmWorkspaceStatus } from "./api";
import {
  applyScope,
  cycleFilter,
  cycleGroup,
  deriveFilterTabs,
  deriveRepoPicker,
  filterCards,
  type GroupMode,
  groupCards,
  mapHelmWallKey,
  type RepoPickerEntry,
  type WallFilter,
  type WallGroup,
  type WallView,
} from "./viewModel";
import { WorkspaceCard } from "./WorkspaceCard";

const STATUS_COLOR: Record<HelmWorkspaceStatus, string> = {
  blocked: "#e5484d",
  working: "#f5a623",
  done: "#30a46c",
  idle: "#8b93a7",
};

const MUTED = "#8b93a7";
const FAINT = "#5b6273";

export interface HelmProps {
  wall: WallView;
  /** Every Project in the registry — the repo picker lists them all and
   * Project grouping reads names + base branches from here. */
  projects?: HelmProject[];
  /** The wall owns the keyboard only when no overlay (Zoom / New Workspace)
   * is open; the container passes `false` while one is. Defaults true (the
   * standalone dev-console mount). */
  keyboardActive?: boolean;
  /** Zoom to a Session (chip click). Placeholder target now; #12 owns
   * the zoom view and takes this over. */
  onZoomSession?: (sessionId: string) => void;
}

function isEditable(el: Element | null): boolean {
  if (!el) return false;
  const tag = el.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    (el as HTMLElement).isContentEditable
  );
}

export function Helm({
  wall,
  projects = [],
  keyboardActive = true,
  onZoomSession,
}: HelmProps) {
  const { counts, cards } = wall;
  const [filter, setFilter] = useState<WallFilter>("all");
  const [group, setGroup] = useState<GroupMode>("flat");
  const [scope, setScope] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);

  // Scope → filter tabs (counts within scope) → filtered → grouped. The
  // repo picker reads the FULL, unscoped/unfiltered set so each repo's live
  // count + worst-status dot stay independent of the active view.
  const scoped = useMemo(() => applyScope(cards, scope), [cards, scope]);
  const tabs = useMemo(() => deriveFilterTabs(scoped, filter), [scoped, filter]);
  const visible = useMemo(() => filterCards(scoped, filter), [scoped, filter]);
  const groups = useMemo(
    () => groupCards(visible, projects, group),
    [visible, projects, group],
  );
  const repoPicker = useMemo(
    () => deriveRepoPicker(cards, projects),
    [cards, projects],
  );
  const scopedRepo = useMemo(
    () =>
      scope === null
        ? null
        : (repoPicker.entries.find((e) => e.projectId === scope) ?? null),
    [scope, repoPicker],
  );

  const pickRepo = useCallback((projectId: string | null) => {
    setScope(projectId);
    setPickerOpen(false);
  }, []);

  // Wall keyboard: `f` cycles filters, `g` toggles grouping, `r` opens the
  // repo picker, `esc` clears filters. All decisions live in the pure
  // `mapHelmWallKey`; it yields while a field is focused, while an overlay
  // owns the keyboard, and never shadows a modified chord.
  useEffect(() => {
    const onKeyDown = (ev: KeyboardEvent) => {
      const action = mapHelmWallKey(
        {
          key: ev.key,
          ctrlKey: ev.ctrlKey,
          metaKey: ev.metaKey,
          altKey: ev.altKey,
        },
        {
          editing: isEditable(document.activeElement),
          overlayActive: !keyboardActive,
          pickerOpen,
        },
      );
      if (action.kind === "none") return;
      ev.preventDefault();
      switch (action.kind) {
        case "cycle-filter":
          setFilter((f) => cycleFilter(f));
          break;
        case "cycle-group":
          setGroup((g) => cycleGroup(g));
          break;
        case "open-repo-picker":
          setPickerOpen(true);
          break;
        case "close-repo-picker":
          setPickerOpen(false);
          break;
        case "clear-filters":
          setFilter("all");
          setScope(null);
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [keyboardActive, pickerOpen]);

  return (
    <div style={wallStyle}>
      <header style={topBarStyle}>
        <span style={wordmarkStyle}>⎈ helmsmen</span>

        <div style={pickerWrapStyle}>
          <button
            type="button"
            style={pickerButtonStyle}
            aria-haspopup="true"
            aria-expanded={pickerOpen}
            title="Repo picker (r)"
            onClick={() => setPickerOpen((open) => !open)}
          >
            {scopedRepo ? (
              <>
                {scopedRepo.worstStatus ? (
                  <span
                    aria-hidden
                    style={{
                      ...dotStyle,
                      background: STATUS_COLOR[scopedRepo.worstStatus],
                    }}
                  />
                ) : null}
                <span style={pickerLabelStyle}>{scopedRepo.name}</span>
                <span style={pickerSubStyle}>⎇ {scopedRepo.baseBranch}</span>
              </>
            ) : (
              <>
                <span style={pickerLabelStyle}>All repos</span>
                <span style={pickerSubStyle}>
                  {repoPicker.allActive} active
                </span>
              </>
            )}
            <span style={caretStyle}>▾</span>
          </button>

          {pickerOpen ? (
            <ul style={dropdownStyle} aria-label="Repos">
              <li>
                <button
                  type="button"
                  style={dropdownRowStyle}
                  aria-current={scope === null}
                  onClick={() => pickRepo(null)}
                >
                  <span style={{ ...dotStyle, background: "transparent" }} />
                  <span style={dropdownNameStyle}>All repos</span>
                  <span style={spacerStyle} />
                  <span style={dropdownCountStyle}>
                    {repoPicker.allActive} active
                  </span>
                </button>
              </li>
              {repoPicker.entries.map((entry) => (
                <RepoRow
                  key={entry.projectId}
                  entry={entry}
                  selected={scope === entry.projectId}
                  onPick={() => pickRepo(entry.projectId)}
                />
              ))}
            </ul>
          ) : null}
        </div>

        <span style={spacerStyle} />

        <span
          style={{
            ...countsStyle,
            color: counts.needsAttention ? STATUS_COLOR.blocked : MUTED,
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

      <div style={toolbarStyle}>
        <nav style={tabsStyle} aria-label="Status filter (f)">
          {tabs.map((tab) => (
            <button
              key={tab.filter}
              type="button"
              aria-pressed={tab.active}
              style={{
                ...tabStyle,
                ...(tab.active ? tabActiveStyle : null),
              }}
              onClick={() => setFilter(tab.filter)}
            >
              {tab.status ? (
                <span
                  aria-hidden
                  style={{
                    ...dotStyle,
                    background: STATUS_COLOR[tab.status],
                    opacity: tab.dimmed ? 0.3 : 1,
                  }}
                />
              ) : null}
              {tab.label}
              <span style={tabCountStyle}>{tab.count}</span>
            </button>
          ))}
        </nav>

        <span style={spacerStyle} />

        <button
          type="button"
          style={groupButtonStyle}
          title="Grouping (g)"
          onClick={() => setGroup((g) => cycleGroup(g))}
        >
          {group === "flat" ? "▤ flat" : "▦ project"}
        </button>
      </div>

      {cards.length === 0 ? (
        <p style={emptyStyle}>nothing here — cut a Workspace to begin</p>
      ) : visible.length === 0 ? (
        <p style={emptyStyle}>nothing here — esc clears filters</p>
      ) : (
        <div style={groupsStyle}>
          {groups.map((wallGroup) => (
            <GroupSection
              key={wallGroup.key}
              group={wallGroup}
              onZoomSession={onZoomSession}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function RepoRow({
  entry,
  selected,
  onPick,
}: {
  entry: RepoPickerEntry;
  selected: boolean;
  onPick: () => void;
}) {
  return (
    <li>
      <button
        type="button"
        aria-current={selected}
        style={dropdownRowStyle}
        onClick={onPick}
      >
        <span
          aria-hidden
          style={{
            ...dotStyle,
            background: entry.worstStatus
              ? STATUS_COLOR[entry.worstStatus]
              : "transparent",
          }}
        />
        <span style={dropdownNameStyle}>{entry.name}</span>
        <span style={dropdownSubStyle}>⎇ {entry.baseBranch}</span>
        <span style={spacerStyle} />
        <span style={dropdownCountStyle}>{entry.count}</span>
      </button>
    </li>
  );
}

function GroupSection({
  group,
  onZoomSession,
}: {
  group: WallGroup;
  onZoomSession?: (sessionId: string) => void;
}) {
  return (
    <section>
      {group.header ? (
        <h2 style={groupHeaderStyle}>
          <span style={groupNameStyle}>{group.header.repoName}</span>
          <span style={groupCountStyle}>({group.header.count})</span>
          <span style={groupBaseStyle}>⎇ {group.header.baseBranch}</span>
        </h2>
      ) : null}
      <div style={gridStyle}>
        {group.cards.map((card) => (
          <WorkspaceCard
            key={card.workspaceId}
            card={card}
            onZoomSession={onZoomSession}
          />
        ))}
      </div>
    </section>
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

const spacerStyle: CSSProperties = { flex: "1 1 auto" };

const countsStyle: CSSProperties = {
  fontSize: 12,
  fontVariantNumeric: "tabular-nums",
};

const numStyle: CSSProperties = { fontWeight: 700 };

const dotStyle: CSSProperties = {
  width: 8,
  height: 8,
  borderRadius: "50%",
  flex: "0 0 auto",
};

// --- repo picker ---

const pickerWrapStyle: CSSProperties = { position: "relative" };

const pickerButtonStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  padding: "4px 10px",
  background: "#0b0e14",
  border: "1px solid #2d3343",
  borderRadius: 6,
  color: "#d7dae0",
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
  cursor: "pointer",
};

const pickerLabelStyle: CSSProperties = { fontWeight: 600 };
const pickerSubStyle: CSSProperties = { color: MUTED, fontSize: 11 };
const caretStyle: CSSProperties = { color: FAINT, fontSize: 10 };

const dropdownStyle: CSSProperties = {
  position: "absolute",
  top: "calc(100% + 4px)",
  left: 0,
  zIndex: 10,
  minWidth: 260,
  margin: 0,
  padding: 4,
  listStyle: "none",
  background: "#0b0e14",
  border: "1px solid #2d3343",
  borderRadius: 8,
  boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
};

const dropdownRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  width: "100%",
  padding: "6px 8px",
  background: "transparent",
  border: "none",
  borderRadius: 4,
  color: "#d7dae0",
  font: "12px/1.2 ui-sans-serif, system-ui, sans-serif",
  cursor: "pointer",
  textAlign: "left",
};

const dropdownNameStyle: CSSProperties = { fontWeight: 600 };
const dropdownSubStyle: CSSProperties = { color: MUTED, fontSize: 11 };
const dropdownCountStyle: CSSProperties = {
  color: MUTED,
  fontSize: 11,
  fontVariantNumeric: "tabular-nums",
};

// --- toolbar: filter tabs + grouping ---

const toolbarStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "8px 16px",
  borderBottom: "1px solid #12161f",
};

const tabsStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 4,
};

const tabStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  padding: "3px 10px",
  background: "transparent",
  border: "1px solid transparent",
  borderRadius: 999,
  color: MUTED,
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
  cursor: "pointer",
};

const tabActiveStyle: CSSProperties = {
  background: "#141a26",
  border: "1px solid #2d3343",
  color: "#d7dae0",
};

const tabCountStyle: CSSProperties = {
  fontVariantNumeric: "tabular-nums",
  fontWeight: 700,
  fontSize: 11,
};

const groupButtonStyle: CSSProperties = {
  padding: "3px 10px",
  background: "transparent",
  border: "1px solid #2d3343",
  borderRadius: 6,
  color: MUTED,
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
  cursor: "pointer",
};

// --- grid + groups ---

const groupsStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 16,
  padding: 16,
  overflow: "auto",
};

const groupHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "baseline",
  gap: 8,
  margin: "0 0 8px",
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
};

const groupNameStyle: CSSProperties = { fontWeight: 700, color: "#d7dae0" };
const groupCountStyle: CSSProperties = {
  color: MUTED,
  fontVariantNumeric: "tabular-nums",
};
const groupBaseStyle: CSSProperties = { color: MUTED, fontSize: 11 };

const gridStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(340px, 1fr))",
  gap: 12,
};

const emptyStyle: CSSProperties = {
  margin: "auto",
  color: FAINT,
};
