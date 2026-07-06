// Helmsmen — New Workspace screen model (task #9), a new module in the
// workspaces module per docs/fork-posture.md.
//
// Pure functional core for the "brief a Workspace from one screen" flow:
// slug derivation, branch-template prefill and its editable round-trip,
// form validation mirroring the backend boundary, key -> action mapping,
// and the "<60s / zero terminal commands" AC expressed as a
// keystroke-count invariant. The imperative shell (NewWorkspace.tsx) owns
// React state, focus, and the invoke seam; it never spawns, gits, or
// touches files — the cut runs entirely backend-side through the existing
// `helm_cut_pipeline` enqueue command (#8). Keeping every decision here
// makes the screen's contract CI-checked in a repo with no DOM test env.

import type { HelmProfile, HelmProject } from "@/modules/helm/api";

/** A Runtime the cut can launch on. At M2 only LocalPty exists and the
 * pipeline hardcodes it; Tmux arrives at M4 (`docs/design` — "survives
 * quit, not sleep"). We advertise what exists and mark the rest, rather
 * than hide the axis. The selected runtime is not yet part of the enqueue
 * payload (the M2 pipeline always launches on LocalPty); this option set
 * is the seam that feeds it when Tmux lands. */
export interface RuntimeOption {
  id: string;
  label: string;
  available: boolean;
  /** Milestone tag shown on an unavailable option (e.g. "M4"). */
  note?: string;
}

/** Matches the backend `runtime::LOCAL_PTY` id. */
export const DEFAULT_RUNTIME_ID = "local-pty";

export const RUNTIME_OPTIONS: readonly RuntimeOption[] = [
  { id: DEFAULT_RUNTIME_ID, label: "local pty", available: true },
  { id: "tmux", label: "flagship tmux", available: false, note: "M4" },
];

/** Longest slug the Brief prefill emits. Well under the backend's 100-char
 * cap, chosen so a prefilled branch stays glanceable. */
const SLUG_PREFILL_MAX = 40;

/** Derive a default slug from the Brief's first line: lowercase, collapse
 * every run of non-alphanumerics to a single dash, trim separators, cap
 * length. The output is always a subset of what the backend
 * `validate_slug` accepts, so the prefilled branch is never rejected.
 * Empty when the first line has nothing sluggable. */
export function deriveSlug(brief: string, maxLen = SLUG_PREFILL_MAX): string {
  const firstLine = brief.split("\n", 1)[0] ?? "";
  return firstLine
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+/, "")
    .slice(0, maxLen)
    .replace(/-+$/, "");
}

/** Client mirror of the backend `core::workspace::validate_slug`: same
 * charset and boundary rules, so the form rejects exactly what the cut
 * would reject — before any invoke, not as a surprise failure. */
export function isValidSlug(slug: string): boolean {
  if (slug.length === 0 || slug.length > 100) return false;
  if (!/^[A-Za-z0-9._-]+$/.test(slug)) return false;
  const alnum = /[A-Za-z0-9]/;
  if (!alnum.test(slug[0]) || !alnum.test(slug[slug.length - 1])) return false;
  if (slug.includes("..")) return false;
  return true;
}

/** Client mirror of the backend `expand_branch_template` for the branch
 * prefill: substitute every `{slug}`. `{slot}` is allocated backend-side
 * at cut, so it is left as-is here (the default template `helm/{slug}`
 * carries none); the slug — not this preview string — is what the cut
 * takes, so a residual placeholder never reaches git. */
export function expandBranchTemplate(template: string, slug: string): string {
  // split/join is a global literal replace without needing ES2021's
  // String.replaceAll (tsconfig target is ES2020).
  return template.split("{slug}").join(slug);
}

/** Recover the slug from a (possibly edited) Branch field, given the
 * Project's template. The backend always re-expands the template with the
 * slug we send, so the template frames every branch: we strip the
 * template's literal prefix/suffix around `{slug}` to read back the
 * variable part. An edit that leaves the frame is slugified whole as a
 * best-effort fallback (the cut still lands on a template-framed branch).
 * The caller validates the result with [`isValidSlug`]. */
export function slugFromBranch(template: string, branch: string): string {
  const marker = "{slug}";
  const at = template.indexOf(marker);
  if (at >= 0) {
    // `{slot}` is numeric and backend-filled; drop it from the literal
    // frame so an in-frame edit still matches.
    const prefix = template.slice(0, at).split("{slot}").join("");
    const suffix = template.slice(at + marker.length).split("{slot}").join("");
    if (
      branch.length >= prefix.length + suffix.length &&
      branch.startsWith(prefix) &&
      branch.endsWith(suffix)
    ) {
      const middle = branch.slice(prefix.length, branch.length - suffix.length);
      return isValidSlug(middle) ? middle : deriveSlug(middle);
    }
  }
  return isValidSlug(branch) ? branch : deriveSlug(branch);
}

/** The one-screen form state. `branchTouched` records whether the user has
 * edited the branch directly; while false the shell keeps the branch
 * prefilled live from the Brief-derived slug. */
export interface NewWorkspaceForm {
  projectId: string | null;
  profileId: string | null;
  runtimeId: string;
  brief: string;
  branch: string;
  branchTouched: boolean;
}

export interface NewWorkspaceValidation {
  ok: boolean;
  /** The slug the cut will take, derived from the branch field. */
  slug: string;
  errors: {
    project?: string;
    profile?: string;
    runtime?: string;
    branch?: string;
    brief?: string;
  };
}

/** Validate the whole form as pure data, deriving the slug the cut takes
 * from the (editable) branch. Mirrors the backend boundary so an invalid
 * form never reaches the enqueue command. */
export function validateForm(
  form: NewWorkspaceForm,
  branchTemplate: string,
): NewWorkspaceValidation {
  const errors: NewWorkspaceValidation["errors"] = {};
  const slug = slugFromBranch(branchTemplate, form.branch);

  if (!form.projectId) errors.project = "pick a Project";
  if (!form.profileId) errors.profile = "pick a Profile";

  const runtime = RUNTIME_OPTIONS.find((r) => r.id === form.runtimeId);
  if (!runtime?.available) errors.runtime = "that Runtime is not available yet";

  if (!isValidSlug(slug)) errors.branch = "branch needs a valid name";

  // The backend `validate_brief` allows any multiline text but a NUL byte
  // (it becomes an argv element of the launch command).
  if (form.brief.includes("\0")) errors.brief = "Brief must not contain NUL";

  return { ok: Object.keys(errors).length === 0, slug, errors };
}

/** The enqueue payload the cut pipeline (`helm_cut_pipeline`, #8) takes.
 * `fetch` is the pipeline's optional first step (cut off a fresh base);
 * defaulted off — the modal exposes no toggle at M2. */
export interface CutSubmission {
  projectId: string;
  slug: string;
  profileId: string;
  brief: string;
  fetch: boolean;
}

/** Map a valid form to the cut-pipeline payload, or null when the form is
 * not submittable (so the shell can never enqueue an invalid cut). */
export function toCutSubmission(
  form: NewWorkspaceForm,
  branchTemplate: string,
): CutSubmission | null {
  const v = validateForm(form, branchTemplate);
  if (!v.ok || !form.projectId || !form.profileId) return null;
  return {
    projectId: form.projectId,
    slug: v.slug,
    profileId: form.profileId,
    brief: form.brief,
    fetch: false,
  };
}

/** The Profiles belonging to a Project, in registry order. */
export function profilesForProject(
  profiles: readonly HelmProfile[],
  projectId: string | null,
): HelmProfile[] {
  if (!projectId) return [];
  return profiles.filter((p) => p.projectId === projectId);
}

/** The form the screen opens with: the sole Project auto-selected (its
 * template prefills the branch), that Project's first Profile
 * auto-selected, runtime = the one that exists. This is what lets the flow
 * cost zero setup keystrokes beyond the Brief. With several Projects none
 * is assumed — the user picks one. */
export function initialForm(
  projects: readonly HelmProject[],
  profiles: readonly HelmProfile[],
): NewWorkspaceForm {
  const project = projects.length === 1 ? projects[0] : null;
  const projectId = project?.id ?? null;
  const profileId = profilesForProject(profiles, projectId)[0]?.id ?? null;
  const template = project?.branchTemplate ?? "helm/{slug}";
  return {
    projectId,
    profileId,
    runtimeId: DEFAULT_RUNTIME_ID,
    brief: "",
    branch: expandBranchTemplate(template, ""),
    branchTouched: false,
  };
}

/** Count the interactions the user MUST perform to reach a submittable
 * form, EXCLUDING typing the Brief (and the opening `n` / final Enter).
 * For a single-Project registry this is 0: Project, Profile, Runtime, and
 * branch are all pre-set, so the entire flow is `n` -> type Brief ->
 * Enter. This is the testable proxy for the stopwatch AC ("<60s, zero
 * terminal commands"): the fixed keyboard overhead is two keys and setup
 * beyond the Brief is nil. Each additional Project adds one required pick. */
export function requiredSetupSteps(
  projects: readonly HelmProject[],
  _profiles: readonly HelmProfile[],
): number {
  // A single Project is auto-selected (its first Profile auto-selected,
  // runtime and branch pre-set), so no setup step is required beyond the
  // Brief. Each additional Project adds exactly one required pick.
  return projects.length <= 1 ? 0 : 1;
}

// --- keyboard contract (pure key -> action, dispatched by the shell) ---

export interface KeyInput {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

export type WallAction = { kind: "new-workspace" } | { kind: "none" };

export interface WallKeyContext {
  /** An editable field holds focus (command bar etc.) — `n` must type. */
  editing: boolean;
  /** An overlay (zoom or the New Workspace screen) is already open. */
  overlayOpen: boolean;
}

/** WALL-level `n` opens the New Workspace screen. Additive, localized to
 * the Helm: it yields while a field is focused or any overlay is open, and
 * never shadows a modified chord (⌘N). */
export function mapWallKey(ev: KeyInput, ctx: WallKeyContext): WallAction {
  if (ctx.editing || ctx.overlayOpen) return { kind: "none" };
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return { kind: "none" };
  return ev.key === "n" ? { kind: "new-workspace" } : { kind: "none" };
}

export type NewWorkspaceAction =
  | { kind: "submit" }
  | { kind: "cancel" }
  | { kind: "none" };

export interface NewWorkspaceKeyContext {
  /** The Brief textarea holds focus: plain Enter must insert a newline, so
   * only Cmd/Ctrl+Enter submits from there. */
  inTextarea: boolean;
}

/** On-screen keys: Esc cancels; Cmd/Ctrl+Enter submits from anywhere
 * (including the multiline Brief); plain Enter submits only when focus is
 * outside the Brief textarea. Everything else falls through to the focused
 * control. */
export function mapNewWorkspaceKey(
  ev: KeyInput,
  ctx: NewWorkspaceKeyContext,
): NewWorkspaceAction {
  if (ev.altKey) return { kind: "none" };
  if (ev.key === "Escape") return { kind: "cancel" };
  if (ev.key === "Enter") {
    if (ev.metaKey || ev.ctrlKey) return { kind: "submit" };
    if (!ctx.inTextarea && !ev.shiftKey) return { kind: "submit" };
    return { kind: "none" };
  }
  return { kind: "none" };
}
