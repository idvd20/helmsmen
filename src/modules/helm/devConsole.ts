// Helmsmen dev console — the M1 surface for adding a Project.
//
// Exposes `window.helmsmen` in the main webview so a Project can be added
// end-to-end from the devtools console:
//
//   await helmsmen.detectProject("/path/to/clone")   // prefill, editable
//   await helmsmen.addProjectFromPath("/path/to/clone", {
//     baseBranch: "develop",                          // optional edits
//   })
//   await helmsmen.listProjects()
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
