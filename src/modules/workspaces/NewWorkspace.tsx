// Helmsmen — the New Workspace screen (task #9).
//
// `n` on the Helm briefs a Workspace from ONE screen: Project chips, Brief
// textarea, Profile pick, Runtime pick, and a branch prefilled from the
// Project's template and editable. Enter (Cmd/Ctrl+Enter from the Brief)
// enqueues the cut and returns to the Helm immediately — the cut runs
// ambient on a backend thread via the existing `helm_cut_pipeline`
// command (#8). The whole flow is meant to cost under 60 seconds and zero
// terminal commands.
//
// Purely an imperative shell: it gathers Projects/Profiles over the invoke
// seam, holds form state, and fires one enqueue command. It never spawns,
// runs git, or touches files — the cut is entirely backend-side. Every
// decision (slug derivation, branch round-trip, validation, key mapping)
// lives in the pure, tested newWorkspaceModel; hostile registry text
// (names, branch) renders as escaped JSX only.

import {
  type CSSProperties,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { HelmApi, HelmProfile, HelmProject } from "@/modules/helm/api";
import {
  deriveSlug,
  expandBranchTemplate,
  initialForm,
  mapNewWorkspaceKey,
  type NewWorkspaceForm,
  profilesForProject,
  RUNTIME_OPTIONS,
  toCutSubmission,
  validateForm,
} from "./newWorkspaceModel";

const DEFAULT_TEMPLATE = "helm/{slug}";

export interface NewWorkspaceProps {
  api: HelmApi;
  /** Esc / after enqueue — back to the Helm wall. */
  onReturn: () => void;
  /** Surface an enqueue rejection (the screen has already returned, so the
   * cut never landed). Defaults to a console warning. */
  onError?: (message: string) => void;
}

export function NewWorkspace({ api, onReturn, onError }: NewWorkspaceProps) {
  const [projects, setProjects] = useState<HelmProject[]>([]);
  const [profiles, setProfiles] = useState<HelmProfile[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [form, setForm] = useState<NewWorkspaceForm>(() =>
    initialForm([], []),
  );
  const briefRef = useRef<HTMLTextAreaElement>(null);

  // Load Projects and Profiles once, then seed the form's defaults (sole
  // Project auto-selected, its first Profile, default runtime, branch
  // prefilled from the template) — the zero-setup starting point.
  useEffect(() => {
    let live = true;
    void Promise.all([api.listProjects(), api.listProfiles()])
      .then(([ps, prof]) => {
        if (!live) return;
        setProjects(ps);
        setProfiles(prof);
        setForm(initialForm(ps, prof));
        setLoaded(true);
      })
      .catch(() => {
        if (live) setLoaded(true);
      });
    return () => {
      live = false;
    };
  }, [api]);

  // Focus the Brief on open so the fast path is: type, then submit.
  useEffect(() => {
    briefRef.current?.focus();
  }, []);

  const selectedProject = useMemo(
    () => projects.find((p) => p.id === form.projectId) ?? null,
    [projects, form.projectId],
  );
  const branchTemplate = selectedProject?.branchTemplate ?? DEFAULT_TEMPLATE;
  const projectProfiles = useMemo(
    () => profilesForProject(profiles, form.projectId),
    [profiles, form.projectId],
  );

  const validation = useMemo(
    () => validateForm(form, branchTemplate),
    [form, branchTemplate],
  );

  const submit = useCallback(() => {
    const submission = toCutSubmission(form, branchTemplate);
    if (!submission) return; // invalid form — the Cut button is disabled too
    // Fire-and-forget: the enqueue command validates + commits the Cutting
    // Workspace fast and returns; every slow step runs ambient. We return
    // to the Helm immediately (AC) and let the wall poll pick up the card.
    void api.cutPipeline(submission).catch((err) => {
      const message = err instanceof Error ? err.message : String(err);
      if (onError) onError(message);
      else console.warn("[new-workspace] cut enqueue failed:", message);
    });
    onReturn();
  }, [api, form, branchTemplate, onReturn, onError]);

  // One screen-level keyboard listener → pure action. Esc cancels;
  // Cmd/Ctrl+Enter submits from anywhere; plain Enter submits unless the
  // Brief textarea holds focus (there it stays a newline).
  useEffect(() => {
    const onKeyDown = (ev: KeyboardEvent) => {
      const inTextarea =
        (document.activeElement as HTMLElement | null)?.tagName === "TEXTAREA";
      const action = mapNewWorkspaceKey(
        {
          key: ev.key,
          ctrlKey: ev.ctrlKey,
          metaKey: ev.metaKey,
          altKey: ev.altKey,
          shiftKey: ev.shiftKey,
        },
        { inTextarea },
      );
      if (action.kind === "none") return;
      ev.preventDefault();
      if (action.kind === "cancel") onReturn();
      else submit();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [submit, onReturn]);

  const pickProject = (project: HelmProject) => {
    setForm((prev) => {
      const nextProfile =
        profilesForProject(profiles, project.id)[0]?.id ?? null;
      return {
        ...prev,
        projectId: project.id,
        profileId: nextProfile,
        // Re-prefill the branch from the new Project's template unless the
        // user has already hand-edited it.
        branch: prev.branchTouched
          ? prev.branch
          : expandBranchTemplate(project.branchTemplate, deriveSlug(prev.brief)),
      };
    });
  };

  const changeBrief = (brief: string) => {
    setForm((prev) => ({
      ...prev,
      brief,
      branch: prev.branchTouched
        ? prev.branch
        : expandBranchTemplate(branchTemplate, deriveSlug(brief)),
    }));
  };

  const baseBranch = selectedProject?.baseBranch ?? "main";
  const projectLabel = selectedProject?.name ?? "no project";

  return (
    <section style={rootStyle} aria-label="new workspace">
      <header style={headerStyle}>
        <span style={titleStyle}>＋ new workspace</span>
        <span style={subStyle}>
          worktree off {projectLabel} ⎇ {baseBranch}
        </span>
        <span style={spacerStyle} />
        <span style={hintStyle}>
          ⌘↵ cut ·{" "}
          <button type="button" style={escStyle} onClick={onReturn}>
            esc cancel
          </button>
        </span>
      </header>

      <div style={bodyStyle}>
        {!loaded ? (
          <p style={mutedStyle}>loading projects…</p>
        ) : projects.length === 0 ? (
          <p style={mutedStyle}>
            no projects yet — add one from Settings before cutting a Workspace
          </p>
        ) : (
          <>
            <Field label="Project">
              <div style={chipRowStyle}>
                {projects.map((project) => (
                  <button
                    key={project.id}
                    type="button"
                    aria-pressed={project.id === form.projectId}
                    style={
                      project.id === form.projectId ? chipActiveStyle : chipStyle
                    }
                    onClick={() => pickProject(project)}
                  >
                    {project.name}
                  </button>
                ))}
              </div>
            </Field>

            <Field label="Brief" hint="becomes the opening prompt">
              <textarea
                ref={briefRef}
                style={textareaStyle}
                value={form.brief}
                aria-label="brief"
                placeholder="what should the agent do?"
                rows={4}
                onChange={(ev) => changeBrief(ev.target.value)}
              />
            </Field>

            <Field label="Profile">
              <div style={chipRowStyle}>
                {projectProfiles.map((profile) => (
                  <button
                    key={profile.id}
                    type="button"
                    aria-pressed={profile.id === form.profileId}
                    style={
                      profile.id === form.profileId ? chipActiveStyle : chipStyle
                    }
                    onClick={() =>
                      setForm((prev) => ({ ...prev, profileId: profile.id }))
                    }
                  >
                    <span
                      aria-hidden
                      style={{ ...swatchStyle, background: profile.color }}
                    />
                    {profile.name}
                  </button>
                ))}
              </div>
            </Field>

            <Field label="Runtime">
              <div style={chipRowStyle}>
                {RUNTIME_OPTIONS.map((runtime) => (
                  <button
                    key={runtime.id}
                    type="button"
                    disabled={!runtime.available}
                    aria-pressed={runtime.id === form.runtimeId}
                    title={
                      runtime.available
                        ? undefined
                        : `${runtime.label} arrives at ${runtime.note}`
                    }
                    style={
                      !runtime.available
                        ? chipDisabledStyle
                        : runtime.id === form.runtimeId
                          ? chipActiveStyle
                          : chipStyle
                    }
                    onClick={() =>
                      runtime.available &&
                      setForm((prev) => ({ ...prev, runtimeId: runtime.id }))
                    }
                  >
                    {runtime.label}
                    {runtime.note ? ` · ${runtime.note}` : ""}
                  </button>
                ))}
              </div>
            </Field>

            <Field
              label="Branch"
              hint={`prefilled from ${branchTemplate} — editable`}
            >
              <input
                type="text"
                style={inputStyle}
                value={form.branch}
                aria-label="branch"
                spellCheck={false}
                onChange={(ev) =>
                  setForm((prev) => ({
                    ...prev,
                    branch: ev.target.value,
                    branchTouched: true,
                  }))
                }
              />
              {validation.errors.branch ? (
                <span style={errorStyle}>{validation.errors.branch}</span>
              ) : null}
            </Field>

            <div style={footerStyle}>
              <button
                type="button"
                disabled={!validation.ok}
                style={validation.ok ? cutStyle : cutDisabledStyle}
                onClick={submit}
              >
                Create workspace ↵
              </button>
              <span style={mutedSmallStyle}>
                worktree + hook config + {projectLabel} Profile injected at
                spawn · returns to the Helm immediately
              </span>
            </div>
          </>
        )}
      </div>
    </section>
  );
}

// A labelled group. Chip rows are not a single labelable control, so this
// is a plain grouping `div` with a heading span (not a `<label>`); the two
// free-text controls (Brief, Branch) carry their own `aria-label`.
function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div style={fieldStyle}>
      <span style={fieldLabelStyle}>
        {label}
        {hint ? <span style={fieldHintStyle}> · {hint}</span> : null}
      </span>
      {children}
    </div>
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
  gap: 12,
  padding: "10px 16px",
  borderBottom: "1px solid #1b2130",
};

const titleStyle: CSSProperties = { fontWeight: 700, letterSpacing: 0.3 };

const subStyle: CSSProperties = {
  color: "#8b93a7",
  font: "12px/1 ui-monospace, monospace",
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

const bodyStyle: CSSProperties = {
  flex: "1 1 auto",
  overflow: "auto",
  padding: 16,
  display: "flex",
  flexDirection: "column",
  gap: 16,
  maxWidth: 720,
  width: "100%",
  margin: "0 auto",
  boxSizing: "border-box",
};

const mutedStyle: CSSProperties = { color: "#5b6273", margin: "auto" };

const fieldStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 6,
};

const fieldLabelStyle: CSSProperties = {
  fontSize: 11,
  textTransform: "uppercase",
  letterSpacing: 0.6,
  color: "#8b93a7",
};

const fieldHintStyle: CSSProperties = { textTransform: "none", color: "#5b6273" };

const chipRowStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 6,
};

const chipStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  padding: "5px 12px",
  background: "#141a26",
  border: "1px solid #2d3343",
  borderRadius: 999,
  color: "#d7dae0",
  font: "12px/1 ui-sans-serif, system-ui, sans-serif",
  cursor: "pointer",
};

const chipActiveStyle: CSSProperties = {
  ...chipStyle,
  background: "#1e2636",
  borderColor: "#3b4a63",
  color: "#fff",
};

const chipDisabledStyle: CSSProperties = {
  ...chipStyle,
  opacity: 0.4,
  cursor: "not-allowed",
};

const swatchStyle: CSSProperties = {
  width: 10,
  height: 10,
  borderRadius: 2,
  flex: "0 0 auto",
};

const textareaStyle: CSSProperties = {
  width: "100%",
  padding: "8px 10px",
  background: "#05070b",
  border: "1px solid #2d3343",
  borderRadius: 6,
  color: "#d7dae0",
  font: "13px/1.5 ui-sans-serif, system-ui, sans-serif",
  resize: "vertical",
  boxSizing: "border-box",
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

const errorStyle: CSSProperties = { color: "#e5484d", fontSize: 11 };

const footerStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 12,
  marginTop: 4,
  flexWrap: "wrap",
};

const cutStyle: CSSProperties = {
  padding: "8px 16px",
  background: "#1e6b3a",
  border: "1px solid #2f8f52",
  borderRadius: 6,
  color: "#fff",
  font: "13px/1 ui-sans-serif, system-ui, sans-serif",
  fontWeight: 600,
  cursor: "pointer",
};

const cutDisabledStyle: CSSProperties = {
  ...cutStyle,
  background: "#141a26",
  borderColor: "#2d3343",
  color: "#5b6273",
  cursor: "not-allowed",
};

const mutedSmallStyle: CSSProperties = { color: "#5b6273", fontSize: 11 };
