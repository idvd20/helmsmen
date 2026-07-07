// Helmsmen dev console — the M1 surface: add a Project, cut a Workspace,
// spawn `claude`, stream + type.
//
// Exposes `window.helmsmen` in the main webview so the whole M1 scripted
// demo runs from the devtools console:
//
//   const p = await helmsmen.addProjectFromPath("/path/to/clone")
//   const { workspace } = await helmsmen.cutWorkspace(p.id, "demo")
//
//   const s = await helmsmen.spawnAgentView(workspace.id)   // stream view
//   await helmsmen.writeAgent(s, "hello")                   // type into it
//   await helmsmen.writeAgent(s, "\r")
//   await helmsmen.agentStatus(s)
//   await helmsmen.killAgent(s)
//
//   await helmsmen.listHarnesses()                          // Caps, from code
//   await helmsmen.removeWorkspace(workspace.id)
//
// Project settings + Profiles (task #7 — definitions only; the cut
// pipeline of task #8 executes them):
//
//   await helmsmen.updateProjectSettings(p.id, {
//     setupScript: "pnpm install",
//     carryOverGlobs: [".env*"],
//     processes: [{ name: "dev", command: "pnpm dev" }],
//   })
//   const [feature] = await helmsmen.listProfiles(p.id)     // seeded copies
//   await helmsmen.updateProfile({ ...feature, verifyCommand: "pnpm test" })
//
// The full cut pipeline, ambient (task #8) — returns at enqueue; fetch,
// worktree add, authorize, carry-overs, setup script, harness wiring, and
// the first Agent Session (Profile snippet + Brief) run in the background.
// A step failure parks the Workspace in Needs you with that step's log:
//
//   const w = await helmsmen.cutPipeline({
//     projectId: p.id,
//     slug: "fix-login",
//     profileId: feature.id,
//     brief: "fix the login page",
//   })
//   const [ws] = await helmsmen.listWorkspaces()  // ws.cut: cutting |
//                                                 // complete | failed{step,log}
//
// Thin by design: every call goes straight to the Tauri commands, which
// validate at the boundary and in the pure core. The frontend never
// spawns processes, runs git, or touches repo files; session output is
// hostile bytes and only ever rendered as text (see streamView.ts).

import { Channel, invoke } from "@tauri-apps/api/core";
import {
  mountQuarterdeck,
  unmountQuarterdeck,
} from "@/modules/workspaces/Quarterdeck";
import { sessionStore } from "@/modules/workspaces/sessionStore";
import {
  type ChannelFactory,
  createHelmApi,
  type HelmAgentSession,
  type SpawnAgentOptions,
} from "./api";
import { openStreamView } from "./streamView";

const makeChannel: ChannelFactory = <T>(onMessage: (message: T) => void) => {
  const channel = new Channel<T>();
  channel.onmessage = onMessage;
  return channel;
};

function createDevConsole() {
  const api = createHelmApi(invoke, makeChannel);

  /** Spawn with a visible stream view attached — the one-call demo path.
   * The returned handle is also registered in the zoom's Session store so
   * the Workspace can be zoomed into (task #12; interim until Session facts
   * land on the wall). */
  const spawnAgentView = async (
    workspaceId: string,
    opts: SpawnAgentOptions = {},
  ): Promise<HelmAgentSession> => {
    const view = openStreamView(`agent @ ${workspaceId}`);
    try {
      const session = await api.spawnAgent(workspaceId, {
        onData: (bytes) => view.write(bytes),
        onExit: (code) => view.exit(code),
        ...opts,
      });
      sessionStore.register({ ...session, kind: "agent" });
      return session;
    } catch (error) {
      view.close();
      throw error;
    }
  };

  /** Open the quarterdeck — the Helm wall plus the Zoom it opens onto (the
   * M2 home view). `helmsmen.openHelm()` / `helmsmen.closeHelm()` from the
   * console; Session chips (and `↵` on the wall) zoom in, Esc returns. */
  const openHelm = () => mountQuarterdeck();
  const closeHelm = () => unmountQuarterdeck();

  return { ...api, spawnAgentView, openHelm, closeHelm };
}

type HelmDevConsole = ReturnType<typeof createDevConsole>;

declare global {
  interface Window {
    helmsmen?: HelmDevConsole;
  }
}

export function installHelmDevConsole(): void {
  window.helmsmen = createDevConsole();
}
