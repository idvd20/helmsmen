// Helmsmen — dev-console stream view for Agent Sessions (task #6).
//
// A deliberately plain overlay: hostile PTY bytes land in a <pre> via
// textContent only, so no output can ever become markup, script, or a
// privileged action. The real Zoom terminal (xterm) arrives at M2; this
// view exists so the M1 demo can watch the stream without devtools
// spam. Render is pure: this module builds DOM from local state only
// and performs no process/git/filesystem operations.

import { createStreamBuffer } from "@/modules/helm/stream";

export interface StreamView {
  write(chunk: Uint8Array | string): void;
  exit(code: number): void;
  close(): void;
}

export function openStreamView(title: string): StreamView {
  const root = document.createElement("div");
  root.style.cssText =
    "position:fixed;bottom:12px;right:12px;width:640px;height:360px;" +
    "z-index:2147483647;display:flex;flex-direction:column;" +
    "background:#0b0e14;color:#d7dae0;border:1px solid #2d3343;" +
    "border-radius:8px;font:12px/1.4 monospace;box-shadow:0 8px 24px rgba(0,0,0,.5)";

  const header = document.createElement("div");
  header.style.cssText =
    "padding:6px 10px;border-bottom:1px solid #2d3343;display:flex;justify-content:space-between";
  const label = document.createElement("span");
  label.textContent = title;
  const closeButton = document.createElement("button");
  closeButton.textContent = "close";
  closeButton.style.cssText =
    "background:none;border:none;color:#8b93a7;cursor:pointer;font:inherit";
  header.append(label, closeButton);

  const pre = document.createElement("pre");
  pre.style.cssText =
    "flex:1;margin:0;padding:10px;overflow:auto;white-space:pre-wrap;word-break:break-all";

  root.append(header, pre);
  document.body.append(root);
  closeButton.addEventListener("click", () => root.remove());

  const buffer = createStreamBuffer();
  return {
    write(chunk) {
      pre.textContent = buffer.append(chunk);
      pre.scrollTop = pre.scrollHeight;
    },
    exit(code) {
      label.textContent = `${title} (exited ${code})`;
    },
    close() {
      root.remove();
    },
  };
}
