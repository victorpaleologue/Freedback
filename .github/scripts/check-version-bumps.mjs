// PR gate: every package whose code changed must bump its version.
//
// Usage (from repo root, in CI):
//   BASE_REF=origin/main node .github/scripts/check-version-bumps.mjs
//
// For each package in packages.json, compare the PR head against BASE_REF:
//   - "touched" = any changed file under the package path that ISN'T docs
//     (*.md / LICENSE). Docs-only changes never require a bump.
//   - if touched, the package's version must be present and strictly greater
//     than its version at BASE_REF.
//   - if the base had no explicit version (the transition PR that first adds
//     per-package versions), the bump is not required — you can't bump from
//     "nothing".
//
// Exits non-zero with a per-package explanation when a bump is missing.
import { execFileSync } from "node:child_process";
import { manifest, parseVersion, currentVersion, compareVersions } from "./packages.mjs";

const BASE = process.env.BASE_REF || "origin/main";

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" });
}

function changedFiles() {
  // Three-dot: changes on the PR branch since it diverged from base.
  const out = git(["diff", "--name-only", `${BASE}...HEAD`]);
  return out.split(/\r?\n/).filter(Boolean);
}

function baseVersion(pkg) {
  let text;
  try {
    text = git(["show", `${BASE}:${pkg.manifest}`]);
  } catch {
    return null; // file didn't exist at base → brand-new package
  }
  return parseVersion(pkg.kind, text);
}

const isDoc = (f) => /\.md$/i.test(f) || /(^|\/)LICENSE(\.|$)/i.test(f);
const underPath = (f, path) => f === path || f.startsWith(path + "/");

const files = changedFiles();
const failures = [];
const bumped = [];

for (const pkg of manifest()) {
  const touched = files.filter((f) => underPath(f, pkg.path) && !isDoc(f));
  if (touched.length === 0) continue;

  const base = baseVersion(pkg);
  const head = currentVersion(pkg);

  if (!head) {
    failures.push(`${pkg.name}: no readable version in ${pkg.manifest}`);
    continue;
  }
  if (base === null) {
    // Newly-versioned or new package: nothing to bump from.
    bumped.push(`${pkg.name}: ${head} (new / newly-versioned — no base to bump from)`);
    continue;
  }
  const cmp = compareVersions(head, base);
  if (cmp > 0) {
    bumped.push(`${pkg.name}: ${base} → ${head}`);
  } else {
    const sample = touched.slice(0, 3).join(", ");
    failures.push(
      `${pkg.name}: version is still ${head} but these files changed: ${sample}${
        touched.length > 3 ? ` (+${touched.length - 3} more)` : ""
      }. Bump "version" in ${pkg.manifest}.`
    );
  }
}

if (bumped.length) {
  console.log("Version bumps detected:");
  for (const b of bumped) console.log("  ✓ " + b);
}
if (failures.length) {
  console.error("\nMissing version bumps — every touched package must bump its version:");
  for (const f of failures) console.error("  ✗ " + f);
  console.error(
    "\nWhy: each package is released independently on merge (one tag per package). " +
      "Touching a package's code without bumping its version would either skip its " +
      "release or collide with an existing tag."
  );
  process.exit(1);
}
if (!bumped.length) {
  console.log("No released packages touched — no version bump required.");
}
