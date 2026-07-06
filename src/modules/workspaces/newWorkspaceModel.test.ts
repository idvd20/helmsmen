import { describe, expect, it } from "vitest";
import type { HelmProfile, HelmProject } from "@/modules/helm/api";
import {
  DEFAULT_RUNTIME_ID,
  deriveSlug,
  expandBranchTemplate,
  initialForm,
  isValidSlug,
  mapNewWorkspaceKey,
  mapWallKey,
  requiredSetupSteps,
  RUNTIME_OPTIONS,
  slugFromBranch,
  toCutSubmission,
  validateForm,
} from "./newWorkspaceModel";

// The New Workspace screen (#9) is a React overlay with keyboard handling,
// which this repo has no DOM env to unit-test. These specs cover the pure
// core the shell delegates to — slug derivation, branch-template
// prefill/round-trip, form validation mirroring the backend, key -> action
// mapping, and the "<60s / zero-setup" flow as a keystroke-count invariant.
// What only a human (or a verify pass) can judge — that `n` actually opens
// the screen and Enter actually returns to the Helm under a stopwatch — is
// called out in the module journal.

const project = (over: Partial<HelmProject> = {}): HelmProject => ({
  id: "prj-1",
  name: "helmsmen",
  repoRoot: "/home/dev/src/helmsmen",
  baseBranch: "main",
  worktreeHome: "/home/dev/.helmsmen/worktrees/helmsmen",
  branchTemplate: "helm/{slug}",
  settings: { setupScript: "", carryOverGlobs: [], processes: [] },
  ...over,
});

const profile = (over: Partial<HelmProfile> = {}): HelmProfile => ({
  id: "prj-1:feature",
  projectId: "prj-1",
  name: "Feature",
  promptSnippet: "/tdd {brief}",
  model: "",
  mcpServers: [],
  verifyCommand: "",
  color: "#3b82f6",
  harnessId: "claude-code",
  ...over,
});

describe("deriveSlug", () => {
  it("slugifies the Brief's first line into the backend-safe charset", () => {
    expect(deriveSlug("Fix the login page")).toBe("fix-the-login-page");
    expect(deriveSlug("Add OAuth2 support!")).toBe("add-oauth2-support");
  });

  it("uses only the first line of a multiline Brief", () => {
    expect(deriveSlug("fix login\nwith tests\nand docs")).toBe("fix-login");
  });

  it("trims leading/trailing separators and collapses runs", () => {
    expect(deriveSlug("  ...weird---input...  ")).toBe("weird-input");
  });

  it("caps length without leaving a trailing dash", () => {
    const slug = deriveSlug(`${"a".repeat(30)} ${"b".repeat(30)}`, 40);
    expect(slug.length).toBeLessThanOrEqual(40);
    expect(slug.endsWith("-")).toBe(false);
  });

  it("is empty when the Brief has nothing sluggable", () => {
    expect(deriveSlug("")).toBe("");
    expect(deriveSlug("   ")).toBe("");
    expect(deriveSlug("!!!")).toBe("");
  });

  it("only ever emits characters the backend validate_slug accepts", () => {
    const slug = deriveSlug("Wild: $tuff & (things) — 你好 v2");
    expect(slug.length).toBeGreaterThan(0);
    expect(isValidSlug(slug)).toBe(true);
  });
});

describe("isValidSlug (mirror of backend validate_slug)", () => {
  it("accepts the same reasonable slugs the backend does", () => {
    for (const good of ["fix-login", "a", "V2", "db_seed", "release-1.2"]) {
      expect(isValidSlug(good)).toBe(true);
    }
  });

  it("rejects the same hostile slugs the backend does", () => {
    for (const bad of [
      "",
      "has space",
      "a/b",
      "..",
      "a..b",
      "-leading-dash",
      ".leading-dot",
      "trailing-dash-",
      "trailing.",
      "back\\slash",
      "semi;colon",
      "dollar$",
      "tick`",
      "x".repeat(101),
    ]) {
      expect(isValidSlug(bad)).toBe(false);
    }
  });
});

describe("expandBranchTemplate (mirror of backend expand_branch_template)", () => {
  it("substitutes {slug} in the default template", () => {
    expect(expandBranchTemplate("helm/{slug}", "fix-login")).toBe(
      "helm/fix-login",
    );
  });

  it("substitutes every {slug} occurrence", () => {
    expect(expandBranchTemplate("wip/{slug}/{slug}", "x")).toBe("wip/x/x");
  });
});

describe("slugFromBranch (branch field is editable; slug is what the cut takes)", () => {
  it("round-trips a branch prefilled and left within the template frame", () => {
    // AC: branch prefilled from the template AND editable. The backend cut
    // re-expands the template with the slug we send, so an in-frame edit
    // must reproduce exactly the branch the user sees.
    const template = "helm/{slug}";
    const branch = expandBranchTemplate(template, "fix-login");
    const slug = slugFromBranch(template, branch);
    expect(slug).toBe("fix-login");
    expect(expandBranchTemplate(template, slug)).toBe(branch);
  });

  it("recovers the edited slug when the user changes only the variable part", () => {
    expect(slugFromBranch("helm/{slug}", "helm/my-experiment")).toBe(
      "my-experiment",
    );
    expect(slugFromBranch("feature/{slug}-wip", "feature/login-wip")).toBe(
      "login",
    );
  });

  it("falls back to slugifying a branch typed outside the template frame", () => {
    // The Project template always frames the branch (the backend re-expands
    // it), so an off-frame edit still yields a safe slug the cut accepts.
    const slug = slugFromBranch("helm/{slug}", "Totally Different Branch");
    expect(isValidSlug(slug)).toBe(true);
  });
});

describe("validateForm", () => {
  const template = "helm/{slug}";
  const okForm = {
    projectId: "prj-1",
    profileId: "prj-1:feature",
    runtimeId: DEFAULT_RUNTIME_ID,
    brief: "fix the login page",
    branch: "helm/fix-login",
    branchTouched: false,
  };

  it("accepts a fully-specified form and derives the slug from the branch", () => {
    const v = validateForm(okForm, template);
    expect(v.ok).toBe(true);
    expect(v.slug).toBe("fix-login");
    expect(v.errors).toEqual({});
  });

  it("requires a Project", () => {
    const v = validateForm({ ...okForm, projectId: null }, template);
    expect(v.ok).toBe(false);
    expect(v.errors.project).toBeDefined();
  });

  it("requires a Profile", () => {
    const v = validateForm({ ...okForm, profileId: null }, template);
    expect(v.ok).toBe(false);
    expect(v.errors.profile).toBeDefined();
  });

  it("rejects a branch whose derived slug is invalid", () => {
    const v = validateForm({ ...okForm, branch: "helm/" }, template);
    expect(v.ok).toBe(false);
    expect(v.errors.branch).toBeDefined();
  });

  it("rejects an unavailable runtime (tmux is M4)", () => {
    const v = validateForm({ ...okForm, runtimeId: "tmux" }, template);
    expect(v.ok).toBe(false);
    expect(v.errors.runtime).toBeDefined();
  });

  it("allows an empty Brief but rejects a NUL byte in it", () => {
    expect(validateForm({ ...okForm, brief: "" }, template).ok).toBe(true);
    const v = validateForm({ ...okForm, brief: "a\0b" }, template);
    expect(v.ok).toBe(false);
    expect(v.errors.brief).toBeDefined();
  });
});

describe("toCutSubmission", () => {
  const template = "helm/{slug}";
  it("maps a valid form to the enqueue payload the cut pipeline takes", () => {
    const sub = toCutSubmission(
      {
        projectId: "prj-1",
        profileId: "prj-1:feature",
        runtimeId: DEFAULT_RUNTIME_ID,
        brief: "fix the login page",
        branch: "helm/fix-login",
        branchTouched: true,
      },
      template,
    );
    expect(sub).toEqual({
      projectId: "prj-1",
      slug: "fix-login",
      profileId: "prj-1:feature",
      brief: "fix the login page",
      fetch: false,
    });
  });

  it("returns null for an invalid form (nothing gets enqueued)", () => {
    const sub = toCutSubmission(
      {
        projectId: null,
        profileId: null,
        runtimeId: DEFAULT_RUNTIME_ID,
        brief: "",
        branch: "helm/",
        branchTouched: false,
      },
      template,
    );
    expect(sub).toBeNull();
  });
});

describe("runtime options", () => {
  it("advertises local pty as available and tmux as the M4 placeholder", () => {
    const local = RUNTIME_OPTIONS.find((r) => r.id === DEFAULT_RUNTIME_ID);
    const tmux = RUNTIME_OPTIONS.find((r) => r.id === "tmux");
    expect(local?.available).toBe(true);
    expect(tmux?.available).toBe(false);
  });

  it("defaults to the one runtime that exists today", () => {
    expect(DEFAULT_RUNTIME_ID).toBe("local-pty");
  });
});

describe("mapWallKey (WALL-level `n` trigger)", () => {
  const ctx = (over = {}) => ({ editing: false, overlayOpen: false, ...over });

  it("`n` opens the New Workspace screen from the wall", () => {
    expect(mapWallKey({ key: "n" }, ctx())).toEqual({ kind: "new-workspace" });
  });

  it("yields `n` while a field is focused (types into the field)", () => {
    expect(mapWallKey({ key: "n" }, ctx({ editing: true }))).toEqual({
      kind: "none",
    });
  });

  it("yields `n` while an overlay is already open (zoom or the screen)", () => {
    expect(mapWallKey({ key: "n" }, ctx({ overlayOpen: true }))).toEqual({
      kind: "none",
    });
  });

  it("never fires on a modified chord (⌘N new window etc.)", () => {
    expect(mapWallKey({ key: "n", metaKey: true }, ctx())).toEqual({
      kind: "none",
    });
    expect(mapWallKey({ key: "n", ctrlKey: true }, ctx())).toEqual({
      kind: "none",
    });
  });
});

describe("mapNewWorkspaceKey (on-screen keyboard contract)", () => {
  it("Esc cancels and returns to the Helm", () => {
    expect(mapNewWorkspaceKey({ key: "Escape" }, { inTextarea: false })).toEqual(
      { kind: "cancel" },
    );
  });

  it("plain Enter submits when focus is not in the Brief textarea", () => {
    expect(mapNewWorkspaceKey({ key: "Enter" }, { inTextarea: false })).toEqual(
      { kind: "submit" },
    );
  });

  it("plain Enter in the Brief textarea inserts a newline, never submits", () => {
    expect(mapNewWorkspaceKey({ key: "Enter" }, { inTextarea: true })).toEqual({
      kind: "none",
    });
  });

  it("Cmd/Ctrl+Enter submits from anywhere, including the Brief", () => {
    expect(
      mapNewWorkspaceKey({ key: "Enter", metaKey: true }, { inTextarea: true }),
    ).toEqual({ kind: "submit" });
    expect(
      mapNewWorkspaceKey({ key: "Enter", ctrlKey: true }, { inTextarea: true }),
    ).toEqual({ kind: "submit" });
  });

  it("ignores unrelated keys (they reach the focused control)", () => {
    expect(mapNewWorkspaceKey({ key: "a" }, { inTextarea: false })).toEqual({
      kind: "none",
    });
  });
});

describe("initialForm + requiredSetupSteps (<60s, zero-terminal AC as a count)", () => {
  const profiles = [
    profile({ id: "prj-1:feature", name: "Feature" }),
    profile({ id: "prj-1:bugfix", name: "Bugfix", color: "#ef4444" }),
  ];

  it("auto-selects the sole Project, its first Profile, and the default runtime", () => {
    const form = initialForm([project()], profiles);
    expect(form.projectId).toBe("prj-1");
    expect(form.profileId).toBe("prj-1:feature");
    expect(form.runtimeId).toBe(DEFAULT_RUNTIME_ID);
    expect(form.branchTouched).toBe(false);
  });

  it("prefills the branch from the Project's template on open", () => {
    const form = initialForm([project({ branchTemplate: "wip/{slug}" })], []);
    expect(form.branch.startsWith("wip/")).toBe(true);
  });

  it("costs zero setup steps beyond typing the Brief for a single-Project registry", () => {
    // The whole flow is then: press `n`, type the Brief, press Enter — no
    // Project/Profile/Runtime/branch interaction required. This is the
    // testable proxy for the stopwatch AC (<60s, zero terminal commands).
    expect(requiredSetupSteps([project()], profiles)).toBe(0);
  });

  it("costs exactly one extra step (pick a Project) when several exist", () => {
    const projects = [project(), project({ id: "prj-2", name: "other" })];
    expect(requiredSetupSteps(projects, profiles)).toBe(1);
  });
});
