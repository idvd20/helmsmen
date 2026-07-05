import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Architecture guards for the task #6 frontend ACs, in the same spirit as
// the backend's pure-core guard: run against the source tree so a
// violating line fails CI even if it typechecks.
//
// - Render is pure: hostile PTY output may only reach the DOM through
//   inert textContent assignment, never through an HTML/script sink.
// - The frontend performs no process/git/filesystem operations: the helm
//   module talks to the backend exclusively via invoke + channels.

const helmDir = dirname(fileURLToPath(import.meta.url));

function sourceFiles(): string[] {
  return readdirSync(helmDir)
    .filter((f) => f.endsWith(".ts") && !f.endsWith(".test.ts"))
    .map((f) => join(helmDir, f));
}

describe("helm frontend guards", () => {
  it("has helm sources to guard", () => {
    expect(sourceFiles().length).toBeGreaterThanOrEqual(4);
  });

  it("never renders session output through an HTML or script sink", () => {
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
