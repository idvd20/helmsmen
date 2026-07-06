import { describe, expect, it } from "vitest";
import type { HelmProject, HelmWorkspaceStatus } from "./api";
import {
  applyScope,
  cardMatchesFilter,
  cycleFilter,
  cycleGroup,
  deriveFilterTabs,
  deriveRepoPicker,
  FILTER_CYCLE,
  filterCards,
  FLAT_GROUP_KEY,
  GROUP_CYCLE,
  groupCards,
  type HelmWallKeyContext,
  mapHelmWallKey,
  type WorkspaceCardView,
  worstStatus,
} from "./viewModel";

// Pure scoping controls for the Helm wall (task #14): the filter tabs (`f`),
// grouping (`g`), and repo picker (`r`) are lenses over the already-derived,
// rank-sorted cards. Every acceptance criterion is encoded here as a unit
// test over deterministic data, because the repo has no DOM test env — the
// actual key-press rendering in the live webview is a human/verify item.

// A minimal card factory: only the fields the scoping lenses read matter.
function card(
  workspaceId: string,
  projectId: string,
  status: HelmWorkspaceStatus,
  extra: Partial<WorkspaceCardView> = {},
): WorkspaceCardView {
  return {
    workspaceId,
    projectId,
    branch: `helm/${workspaceId}`,
    projectName: projectId,
    baseBranch: "main",
    status,
    statusLabel: status,
    rank: 0,
    profileColor: "#000",
    elapsedMinutes: 0,
    pulse: false,
    body: { kind: "activity", lines: ["…"] },
    chips: [],
    ...extra,
  };
}

const project = (id: string, name: string, baseBranch: string): HelmProject => ({
  id,
  name,
  repoRoot: `/src/${name}`,
  baseBranch,
  worktreeHome: `/wt/${name}`,
  branchTemplate: "helm/{slug}",
  settings: { setupScript: "", carryOverGlobs: [], processes: [] },
});

// Two Projects × two agents — the M2 shape.
const alpha = project("prj-a", "alpha", "main");
const beta = project("prj-b", "beta", "develop");

const blockedA = card("wa1", "prj-a", "blocked");
const workingA = card("wa2", "prj-a", "working");
const doneB = card("wb1", "prj-b", "done");
const idleB = card("wb2", "prj-b", "idle");
const wall = [blockedA, doneB, workingA, idleB];

describe("filter cycle (`f`)", () => {
  it("cycles All → Needs you → Working → To review → Idle → wrap", () => {
    expect(cycleFilter("all")).toBe("blocked");
    expect(cycleFilter("blocked")).toBe("working");
    expect(cycleFilter("working")).toBe("done");
    expect(cycleFilter("done")).toBe("idle");
    expect(cycleFilter("idle")).toBe("all");
  });

  it("follows the spec's tab order exactly", () => {
    expect(FILTER_CYCLE).toEqual(["all", "blocked", "working", "done", "idle"]);
  });
});

describe("cardMatchesFilter / filterCards", () => {
  it("`all` matches everything and returns the same reference", () => {
    expect(cardMatchesFilter("idle", "all")).toBe(true);
    expect(filterCards(wall, "all")).toBe(wall);
  });

  it("a status filter keeps only that status, in incoming order", () => {
    expect(filterCards(wall, "blocked")).toEqual([blockedA]);
    expect(filterCards(wall, "idle")).toEqual([idleB]);
    expect(filterCards(wall, "working").map((c) => c.workspaceId)).toEqual([
      "wa2",
    ]);
  });
});

describe("deriveFilterTabs — live counts + dimmed dots", () => {
  it("counts each status, marks the active tab, and colors dots except All", () => {
    const tabs = deriveFilterTabs(wall, "working");
    expect(tabs.map((t) => [t.filter, t.count])).toEqual([
      ["all", 4],
      ["blocked", 1],
      ["working", 1],
      ["done", 1],
      ["idle", 1],
    ]);
    const all = tabs.find((t) => t.filter === "all");
    expect(all?.status).toBeNull();
    expect(all?.dimmed).toBe(false);
    expect(tabs.find((t) => t.filter === "working")?.active).toBe(true);
    expect(tabs.find((t) => t.filter === "blocked")?.status).toBe("blocked");
  });

  it("uses the PRD display aliases for the tab labels", () => {
    const labels = Object.fromEntries(
      deriveFilterTabs(wall, "all").map((t) => [t.filter, t.label]),
    );
    expect(labels.all).toBe("All");
    expect(labels.blocked).toBe("Needs you");
    expect(labels.done).toBe("To review");
    expect(labels.working).toBe("Working");
  });

  it("dims a status tab to zero when nothing has that status (All never dims)", () => {
    const tabs = deriveFilterTabs([blockedA, workingA], "all");
    expect(tabs.find((t) => t.filter === "done")).toMatchObject({
      count: 0,
      dimmed: true,
    });
    expect(tabs.find((t) => t.filter === "idle")).toMatchObject({
      count: 0,
      dimmed: true,
    });
    expect(tabs.find((t) => t.filter === "blocked")?.dimmed).toBe(false);
    expect(tabs.find((t) => t.filter === "all")?.dimmed).toBe(false);
  });

  it("stays correct live when a card's status changes (recompute)", () => {
    const before = deriveFilterTabs(wall, "all");
    expect(before.find((t) => t.filter === "blocked")?.count).toBe(1);
    // The blocked agent gets unblocked → now working.
    const after = deriveFilterTabs(
      [card("wa1", "prj-a", "working"), doneB, workingA, idleB],
      "all",
    );
    expect(after.find((t) => t.filter === "blocked")?.count).toBe(0);
    expect(after.find((t) => t.filter === "blocked")?.dimmed).toBe(true);
    expect(after.find((t) => t.filter === "working")?.count).toBe(2);
  });
});

describe("group cycle (`g`)", () => {
  it("toggles flat ⇄ project (ship is M8, absent)", () => {
    expect(GROUP_CYCLE).toEqual(["flat", "project"]);
    expect(cycleGroup("flat")).toBe("project");
    expect(cycleGroup("project")).toBe("flat");
  });
});

describe("groupCards", () => {
  it("flat = one headerless group holding every card untouched", () => {
    const groups = groupCards(wall, [alpha, beta], "flat");
    expect(groups).toHaveLength(1);
    expect(groups[0].key).toBe(FLAT_GROUP_KEY);
    expect(groups[0].header).toBeNull();
    expect(groups[0].cards).toBe(wall);
  });

  it("project = per-Project sections headed by repo name, count, base branch", () => {
    const groups = groupCards(wall, [alpha, beta], "project");
    expect(groups).toHaveLength(2);
    // Registry order: alpha before beta.
    expect(groups[0].header).toEqual({
      projectId: "prj-a",
      repoName: "alpha",
      count: 2,
      baseBranch: "main",
    });
    expect(groups[1].header).toEqual({
      projectId: "prj-b",
      repoName: "beta",
      count: 2,
      baseBranch: "develop",
    });
    expect(groups[0].cards.map((c) => c.workspaceId)).toEqual(["wa1", "wa2"]);
    expect(groups[1].cards.map((c) => c.workspaceId)).toEqual(["wb1", "wb2"]);
  });

  it("omits Projects that have no cards after filtering", () => {
    const groups = groupCards([blockedA], [alpha, beta], "project");
    expect(groups.map((g) => g.key)).toEqual(["prj-a"]);
    expect(groups[0].header?.count).toBe(1);
  });
});

describe("worstStatus — the repo dot", () => {
  it("returns the minimum-rank status (Blocked ≺ Done ≺ Working ≺ Idle)", () => {
    expect(worstStatus(["idle", "working", "done"])).toBe("done");
    expect(worstStatus(["idle", "working"])).toBe("working");
    expect(worstStatus(["done", "blocked", "working"])).toBe("blocked");
    expect(worstStatus(["idle"])).toBe("idle");
  });

  it("is null for an empty set", () => {
    expect(worstStatus([])).toBeNull();
  });
});

describe("deriveRepoPicker — per-repo live count + worst-status dot", () => {
  it("lists every Project in registry order with its own count and worst dot", () => {
    const view = deriveRepoPicker(wall, [alpha, beta]);
    expect(view.allActive).toBe(4);
    expect(view.entries).toEqual([
      {
        projectId: "prj-a",
        name: "alpha",
        baseBranch: "main",
        count: 2,
        worstStatus: "blocked", // blocked ≺ working
      },
      {
        projectId: "prj-b",
        name: "beta",
        baseBranch: "develop",
        count: 2,
        worstStatus: "done", // done ≺ idle
      },
    ]);
  });

  it("shows a Project with no Workspaces as count 0 / no dot", () => {
    const gamma = project("prj-c", "gamma", "main");
    const view = deriveRepoPicker(wall, [alpha, beta, gamma]);
    expect(view.entries[2]).toMatchObject({
      projectId: "prj-c",
      count: 0,
      worstStatus: null,
    });
  });

  it("counts are independent of the active status filter (full set)", () => {
    // Even scoped-down/filtered views feed the picker the FULL card set.
    const view = deriveRepoPicker(filterCards(wall, "blocked"), [alpha, beta]);
    // Here we deliberately pass a filtered set to show the caller controls
    // it; over the full wall each repo keeps its true count.
    expect(view.allActive).toBe(1);
    const full = deriveRepoPicker(wall, [alpha, beta]);
    expect(full.entries.map((e) => e.count)).toEqual([2, 2]);
  });
});

describe("applyScope — scoping to one Project", () => {
  it("null keeps every card (same reference)", () => {
    expect(applyScope(wall, null)).toBe(wall);
  });

  it("a Project id narrows the wall to that Project's cards", () => {
    expect(applyScope(wall, "prj-a").map((c) => c.workspaceId)).toEqual([
      "wa1",
      "wa2",
    ]);
    expect(applyScope(wall, "prj-b").map((c) => c.workspaceId)).toEqual([
      "wb1",
      "wb2",
    ]);
  });

  it("scoped filter tabs count only within the scope", () => {
    const scoped = applyScope(wall, "prj-b");
    const tabs = deriveFilterTabs(scoped, "all");
    expect(tabs.find((t) => t.filter === "all")?.count).toBe(2);
    expect(tabs.find((t) => t.filter === "done")?.count).toBe(1);
    expect(tabs.find((t) => t.filter === "blocked")?.count).toBe(0);
    expect(tabs.find((t) => t.filter === "blocked")?.dimmed).toBe(true);
  });
});

describe("mapHelmWallKey — `f` / `g` / `r` / esc", () => {
  const base: HelmWallKeyContext = {
    editing: false,
    overlayActive: false,
    pickerOpen: false,
  };

  it("maps the bare wall keys to their actions", () => {
    expect(mapHelmWallKey({ key: "f" }, base)).toEqual({ kind: "cycle-filter" });
    expect(mapHelmWallKey({ key: "g" }, base)).toEqual({ kind: "cycle-group" });
    expect(mapHelmWallKey({ key: "r" }, base)).toEqual({
      kind: "open-repo-picker",
    });
    expect(mapHelmWallKey({ key: "Escape" }, base)).toEqual({
      kind: "clear-filters",
    });
  });

  it("maps `a`/`x` to Allow/Deny of the selected/blocked card (task #18)", () => {
    expect(mapHelmWallKey({ key: "a" }, base)).toEqual({ kind: "answer-allow" });
    expect(mapHelmWallKey({ key: "x" }, base)).toEqual({ kind: "answer-deny" });
  });

  it("maps `A`/`X` (shift) to the bulk banner's Allow-all/Deny-all (task #19)", () => {
    // Uppercase (shift) is the bulk pair; lowercase stays per-card. Shift is
    // not treated as a chord, so `A`/`X` map through with no modifier.
    expect(mapHelmWallKey({ key: "A" }, base)).toEqual({
      kind: "bulk-allow-all",
    });
    expect(mapHelmWallKey({ key: "X" }, base)).toEqual({
      kind: "bulk-deny-all",
    });
  });

  it("yields while a field is focused (letters must type)", () => {
    const ctx = { ...base, editing: true };
    for (const key of ["f", "g", "r", "a", "x", "A", "X", "Escape"]) {
      expect(mapHelmWallKey({ key }, ctx)).toEqual({ kind: "none" });
    }
  });

  it("yields entirely while an overlay (Zoom / New Workspace) is open", () => {
    const ctx = { ...base, overlayActive: true };
    expect(mapHelmWallKey({ key: "f" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "a" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "x" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "A" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "X" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "Escape" }, ctx)).toEqual({ kind: "none" });
  });

  it("never shadows a modified chord (⌘R reloads, etc.)", () => {
    expect(mapHelmWallKey({ key: "r", metaKey: true }, base)).toEqual({
      kind: "none",
    });
    expect(mapHelmWallKey({ key: "f", ctrlKey: true }, base)).toEqual({
      kind: "none",
    });
    expect(mapHelmWallKey({ key: "a", metaKey: true }, base)).toEqual({
      kind: "none",
    });
    expect(mapHelmWallKey({ key: "x", ctrlKey: true }, base)).toEqual({
      kind: "none",
    });
    // The bulk pair yields to a modified chord too (⌘A selects all, etc.).
    expect(mapHelmWallKey({ key: "A", metaKey: true }, base)).toEqual({
      kind: "none",
    });
    expect(mapHelmWallKey({ key: "X", ctrlKey: true }, base)).toEqual({
      kind: "none",
    });
  });

  it("while the picker is open, `r`/esc close it and f/g/a/x/A/X are inert", () => {
    const ctx = { ...base, pickerOpen: true };
    expect(mapHelmWallKey({ key: "r" }, ctx)).toEqual({
      kind: "close-repo-picker",
    });
    expect(mapHelmWallKey({ key: "Escape" }, ctx)).toEqual({
      kind: "close-repo-picker",
    });
    expect(mapHelmWallKey({ key: "f" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "g" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "a" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "x" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "A" }, ctx)).toEqual({ kind: "none" });
    expect(mapHelmWallKey({ key: "X" }, ctx)).toEqual({ kind: "none" });
  });
});
