import { describe, expect, it } from "vitest";
import {
  ALLOWED_LICENSES,
  VERIFIED_PACKAGE_LICENSES,
  checkPackages,
  isLicenseAllowed,
} from "../../scripts/check-licenses.mjs";

// Locks the M0 license gate (PRD security invariant: Apache-2.0 posture
// matching Terax). The pure core evaluated here is what CI runs against the
// real `pnpm licenses list --prod --json` output; the fixture below encodes
// the license set observed on the unmodified fork so the gate is proven green
// at this seam, not just claimed.

describe("isLicenseAllowed (SPDX expression evaluation)", () => {
  it("accepts an exactly allowlisted license", () => {
    expect(isLicenseAllowed("MIT")).toBe(true);
    expect(isLicenseAllowed("Apache-2.0")).toBe(true);
  });

  it("rejects licenses outside the allowlist", () => {
    expect(isLicenseAllowed("GPL-3.0-only")).toBe(false);
    expect(isLicenseAllowed("AGPL-3.0-or-later")).toBe(false);
    expect(isLicenseAllowed("Unknown")).toBe(false);
    expect(isLicenseAllowed("")).toBe(false);
  });

  it("accepts an OR expression when any branch is allowed", () => {
    expect(isLicenseAllowed("GPL-3.0-only OR MIT")).toBe(true);
    expect(isLicenseAllowed("MIT OR Apache-2.0")).toBe(true);
  });

  it("rejects an OR expression when no branch is allowed", () => {
    expect(isLicenseAllowed("GPL-3.0-only OR AGPL-3.0-only")).toBe(false);
  });

  it("accepts an AND expression only when every branch is allowed", () => {
    expect(isLicenseAllowed("MIT AND Apache-2.0")).toBe(true);
    expect(isLicenseAllowed("MIT AND GPL-3.0-only")).toBe(false);
  });

  it("handles parenthesised expressions as pnpm reports them", () => {
    // MPL-2.0 and AFL-2.1 are deliberately NOT allowlisted; the OR branch
    // must satisfy the gate on its own.
    expect(isLicenseAllowed("(MPL-2.0 OR Apache-2.0)")).toBe(true);
    expect(isLicenseAllowed("(AFL-2.1 OR BSD-3-Clause)")).toBe(true);
    expect(isLicenseAllowed("(MPL-2.0 OR GPL-2.0-only)")).toBe(false);
  });

  it("handles nested groups with correct precedence", () => {
    expect(
      isLicenseAllowed("(GPL-2.0-only AND (MIT OR GPL-3.0-only)) OR Apache-2.0"),
    ).toBe(true);
    expect(
      isLicenseAllowed("GPL-2.0-only AND (MIT OR Apache-2.0)"),
    ).toBe(false);
  });

  it("treats WITH exceptions as atomic ids needing an explicit allow", () => {
    expect(isLicenseAllowed("Apache-2.0 WITH LLVM-exception")).toBe(false);
    expect(
      isLicenseAllowed("Apache-2.0 WITH LLVM-exception", [
        "Apache-2.0 WITH LLVM-exception",
      ]),
    ).toBe(true);
  });
});

describe("checkPackages (policy over a pnpm licenses tree)", () => {
  it("flags packages with disallowed licenses", () => {
    const violations = checkPackages({
      MIT: [{ name: "ok-pkg", versions: ["1.0.0"] }],
      "GPL-3.0-only": [{ name: "bad-pkg", versions: ["2.0.0"] }],
    });
    expect(violations).toEqual([{ name: "bad-pkg", license: "GPL-3.0-only" }]);
  });

  it("resolves Unknown via the verified-exception map, then re-checks", () => {
    // khroma ships an MIT license file but omits the package.json `license`
    // field, so pnpm reports "Unknown" — the exception carries the verified id.
    const violations = checkPackages({
      Unknown: [{ name: "khroma", versions: ["2.1.0"] }],
    });
    expect(violations).toEqual([]);
  });

  it("does not let an exception bypass the allowlist", () => {
    const violations = checkPackages(
      { Unknown: [{ name: "sneaky", versions: ["1.0.0"] }] },
      { verified: { sneaky: "GPL-3.0-only" } },
    );
    expect(violations).toEqual([
      { name: "sneaky", license: "GPL-3.0-only (verified)" },
    ]);
  });

  it("flags Unknown licenses that have no verified exception", () => {
    const violations = checkPackages({
      Unknown: [{ name: "mystery", versions: ["0.0.1"] }],
    });
    expect(violations).toEqual([{ name: "mystery", license: "Unknown" }]);
  });

  it("passes the license set observed on the unmodified fork", () => {
    // Every license key `pnpm licenses list --prod --json` reported on the
    // unmodified Terax fork. If a dependency bump introduces a new license,
    // CI fails and adding it to the allowlist becomes a deliberate decision.
    const observed = [
      "Apache-2.0",
      "MIT",
      "OFL-1.1",
      "Apache-2.0 OR MIT",
      "MIT OR Apache-2.0",
      "ISC",
      "BSD-3-Clause",
      "(MPL-2.0 OR Apache-2.0)",
      "BSD-2-Clause",
      "(AFL-2.1 OR BSD-3-Clause)",
      "Unlicense",
      "0BSD",
    ];
    const tree = Object.fromEntries(
      observed.map((license, i) => [
        license,
        [{ name: `pkg-${i}`, versions: ["1.0.0"] }],
      ]),
    );
    tree.Unknown = [{ name: "khroma", versions: ["2.1.0"] }];
    expect(checkPackages(tree)).toEqual([]);
  });

  it("keeps the shipped policy itself inside the allowlist", () => {
    // Exceptions map to concrete SPDX ids that must themselves be allowed —
    // a policy edit can never smuggle in a disallowed license.
    for (const license of Object.values(VERIFIED_PACKAGE_LICENSES)) {
      expect(ALLOWED_LICENSES).toContain(license);
    }
  });
});
