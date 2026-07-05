// Helmsmen dev console — the M1 surface for adding a Project and cutting
// a Workspace.
//
// Exposes `window.helmsmen` in the main webview so both flows run
// end-to-end from the devtools console:
//
//   await helmsmen.detectProject("/path/to/clone")   // prefill, editable
//   await helmsmen.addProjectFromPath("/path/to/clone", {
//     baseBranch: "develop",                          // optional edits
//   })
//   await helmsmen.listProjects()
//
//   const { workspace, env } = await helmsmen.cutWorkspace("prj-…", "fix-login")
//   await helmsmen.listWorkspaces()
//   await helmsmen.workspaceEnv(workspace.id)         // HELMSMEN_* set
//   await helmsmen.removeWorkspace(workspace.id)      // frees the Slot
//
// Thin by design: every call goes straight to the Tauri commands, which
// validate at the boundary and in the pure core.

import { invoke } from "@tauri-apps/api/core";
import { createHelmApi, type HelmApi } from "./api";

declare global {
  interface Window {
    helmsmen?: HelmApi;
  }
}

export function installHelmDevConsole(): void {
  window.helmsmen = createHelmApi(invoke);
}
