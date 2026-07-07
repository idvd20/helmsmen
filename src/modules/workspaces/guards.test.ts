import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Architecture guards for the zoom module (#12), the same battery the helm
// module runs: the zoom shows hostile PTY output and drives the live PTY,
// so it must render output only through inert textContent (never an
// HTML/script sink) and must reach the OS only through the invoke seam
// (never process/git/fs itself). Run against the source tree so a violating
// line fails CI even if it typechecks.

const moduleDir = dirname(fileURLToPath(import.meta.url));

function sourceFiles(): string[] {
  return readdirSync(moduleDir)
    .filter(
      (f) =>
        (f.endsWith(".ts") || f.endsWith(".tsx")) &&
        !f.endsWith(".test.ts") &&
        !f.endsWith(".test.tsx"),
    )
    .map((f) => join(moduleDir, f));
}

describe("workspaces (zoom) frontend guards", () => {
  it("has zoom sources to guard", () => {
    expect(sourceFiles().length).toBeGreaterThanOrEqual(4);
  });

  it("never renders PTY output through an HTML or script sink", () => {
    const forbidden = [
      "innerHTML",
      "outerHTML",
      "insertAdjacentHTML",
      "document.write",
      "dangerouslySetInnerHTML",
      "eval(",
      "new Function",
    ];
    for (const file of sourceFiles()) {
      const source = readFileSync(file, "utf8");
      for (const token of forbidden) {
        expect(source, `${file} must not use ${token}`).not.toContain(token);
      }
    }
  });

  it("performs no process, git, or filesystem operations", () => {
    const forbidden = [
      "child_process",
      "node:fs",
      "node:child_process",
      "@tauri-apps/plugin-shell",
      "plugin-fs",
      "shell_run_command",
      "shell_bg_spawn",
      "fs_write_file",
      "fs_create_file",
      "fs_delete",
    ];
    for (const file of sourceFiles()) {
      const source = readFileSync(file, "utf8");
      for (const token of forbidden) {
        expect(source, `${file} must not use ${token}`).not.toContain(token);
      }
    }
  });
});
