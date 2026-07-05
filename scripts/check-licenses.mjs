#!/usr/bin/env node
// JS dependency license gate (M0 CI gate; PRD security invariant: Apache-2.0
// posture matching Terax). Pure core: an SPDX expression evaluator plus a
// policy check over the tree that `pnpm licenses list --prod --json` reports.
// Imperative shell: run pnpm, parse, print violations, exit non-zero.
//
// CLI:  pnpm check-licenses   (or: node scripts/check-licenses.mjs)
// Used as a library by src/lib/licensePolicy.test.ts, which locks the policy.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// Licenses vetted as compatible with distributing Helmsmen under Apache-2.0.
// Deliberately minimal: covers the tree observed on the unmodified fork, so a
// dependency bump that introduces a new license fails CI and adding it here
// becomes an explicit decision. OFL-1.1 covers bundled fonts only (Inter,
// JetBrains Mono).
export const ALLOWED_LICENSES = [
  "0BSD",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "MIT",
  "OFL-1.1",
  "Unlicense",
];

// Packages whose package.json omits/garbles the `license` field (pnpm reports
// "Unknown") but whose shipped license text was manually verified. The mapped
// SPDX id is re-checked against ALLOWED_LICENSES — an exception can never
// bypass the allowlist.
export const VERIFIED_PACKAGE_LICENSES = {
  // Ships an MIT license file (node_modules/khroma/license) and states
  // "MIT © Fabio Spampinato" in its readme; package.json lacks the field.
  khroma: "MIT",
};

/** Split an SPDX expression on a top-level (outside parentheses) operator. */
function splitTopLevel(expression, operator) {
  const parts = [];
  let depth = 0;
  let current = "";
  const tokens = expression.split(/(\s+|\(|\))/).filter((t) => t !== "");
  for (const token of tokens) {
    if (token === "(") depth += 1;
    if (token === ")") depth -= 1;
    if (depth === 0 && token === operator) {
      parts.push(current);
      current = "";
    } else {
      current += token;
    }
  }
  parts.push(current);
  return parts.map((p) => p.trim()).filter((p) => p !== "");
}

/** True when the whole expression is a single parenthesised group. */
function isWrappedInParens(expression) {
  if (!expression.startsWith("(") || !expression.endsWith(")")) return false;
  let depth = 0;
  for (let i = 0; i < expression.length; i += 1) {
    if (expression[i] === "(") depth += 1;
    if (expression[i] === ")") depth -= 1;
    if (depth === 0 && i < expression.length - 1) return false;
  }
  return depth === 0;
}

/**
 * Evaluate an SPDX license expression against the allowlist.
 * OR passes when any branch passes; AND requires every branch; `WITH`
 * exceptions and `+` ranges are treated as atomic ids needing an explicit
 * allowlist entry. Unknown or empty expressions fail closed.
 */
export function isLicenseAllowed(expression, allowed = ALLOWED_LICENSES) {
  const expr = expression.trim();
  if (expr === "") return false;
  const orParts = splitTopLevel(expr, "OR");
  if (orParts.length > 1) {
    return orParts.some((part) => isLicenseAllowed(part, allowed));
  }
  const andParts = splitTopLevel(expr, "AND");
  if (andParts.length > 1) {
    return andParts.every((part) => isLicenseAllowed(part, allowed));
  }
  if (isWrappedInParens(expr)) {
    return isLicenseAllowed(expr.slice(1, -1), allowed);
  }
  return allowed.includes(expr);
}

/**
 * Check a `pnpm licenses list --json` tree ({ licenseExpr: [pkg, ...] })
 * against the policy. Returns violations as { name, license }; verified
 * per-package exceptions substitute the vetted id and are re-checked.
 */
export function checkPackages(licenseMap, policy = {}) {
  const allowed = policy.allowed ?? ALLOWED_LICENSES;
  const verified = policy.verified ?? VERIFIED_PACKAGE_LICENSES;
  const violations = [];
  for (const [license, packages] of Object.entries(licenseMap)) {
    for (const pkg of packages) {
      const verifiedLicense = Object.hasOwn(verified, pkg.name)
        ? verified[pkg.name]
        : undefined;
      if (verifiedLicense !== undefined) {
        if (!isLicenseAllowed(verifiedLicense, allowed)) {
          violations.push({
            name: pkg.name,
            license: `${verifiedLicense} (verified)`,
          });
        }
        continue;
      }
      if (!isLicenseAllowed(license, allowed)) {
        violations.push({ name: pkg.name, license });
      }
    }
  }
  return violations;
}

const isCli = process.argv[1] === fileURLToPath(import.meta.url);
if (isCli) {
  const raw = execFileSync("pnpm", ["licenses", "list", "--prod", "--json"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const licenseMap = JSON.parse(raw);
  const total = Object.values(licenseMap).reduce(
    (count, packages) => count + packages.length,
    0,
  );
  const violations = checkPackages(licenseMap);
  if (violations.length > 0) {
    console.error(
      `License check FAILED: ${violations.length} package(s) outside the Apache-2.0 posture:\n`,
    );
    for (const { name, license } of violations) {
      console.error(`  x ${name}: ${license}`);
    }
    console.error(
      "\nEither replace the dependency or, after review, extend the policy in scripts/check-licenses.mjs (locked by src/lib/licensePolicy.test.ts).",
    );
    process.exit(1);
  }
  console.log(
    `License check passed: ${total} production packages within the Apache-2.0 posture.`,
  );
}
