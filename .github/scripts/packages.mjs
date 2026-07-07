// Shared package inventory + version parsing for Freedback's per-package
// release automation. Single source of truth is ../packages.json.
//
// Version parsing is deliberately dependency-free (no toml/semver npm deps, so
// `node <script>` runs with zero install in CI):
//   - cargo            → the `[package]` section's explicit `version = "x"`
//   - cargo-workspace  → the `[workspace.package]` section's `version = "x"`
//   - npm / json       → the JSON `.version` field
//
// A crate that still inherits its version (`version.workspace = true`) parses
// to `null` — callers treat that as "not yet independently versioned" and skip
// it, which keeps the transition PR (that introduces explicit versions) from
// tripping its own bump check.
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");

/** The raw package list from packages.json. */
export function manifest() {
  const raw = readFileSync(join(HERE, "..", "packages.json"), "utf8");
  return JSON.parse(raw).packages;
}

/** Extract the version from a manifest file's text, given the package kind.
 *  Returns null when no explicit version is present (e.g. workspace-inherited). */
export function parseVersion(kind, text) {
  if (kind === "npm" || kind === "json") {
    try {
      const v = JSON.parse(text).version;
      return typeof v === "string" ? v : null;
    } catch {
      return null;
    }
  }
  // cargo / cargo-workspace: find the right TOML section, then its version line.
  const section = kind === "cargo-workspace" ? "workspace.package" : "package";
  return versionInTomlSection(text, section);
}

// Pull `version = "x"` out of a named top-level TOML section, without a TOML lib.
function versionInTomlSection(text, section) {
  const lines = text.split(/\r?\n/);
  let inSection = false;
  const header = `[${section}]`;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) {
      inSection = trimmed === header;
      continue;
    }
    if (!inSection) continue;
    // Only an explicit string assignment counts — `version.workspace = true`
    // has no quoted value and is intentionally ignored (returns null).
    const m = trimmed.match(/^version\s*=\s*"([^"]+)"/);
    if (m) return m[1];
  }
  return null;
}

/** Read a package's current version from the working tree (or null). */
export function currentVersion(pkg) {
  let text;
  try {
    text = readFileSync(join(REPO, pkg.manifest), "utf8");
  } catch {
    return null;
  }
  return parseVersion(pkg.kind, text);
}

/** The full inventory with resolved current versions and tag names. */
export function packagesWithVersions() {
  return manifest().map((pkg) => {
    const version = currentVersion(pkg);
    return { ...pkg, version, tag: version ? `${pkg.tagPrefix}-v${version}` : null };
  });
}

/** Compare two dotted numeric versions. Returns -1/0/1 (a<b / a==b / a>b).
 *  Non-numeric / pre-release suffixes fall back to a string compare of the tail. */
export function compareVersions(a, b) {
  const pa = a.split(/[.+-]/);
  const pb = b.split(/[.+-]/);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i++) {
    const na = Number(pa[i]);
    const nb = Number(pb[i]);
    const bothNum = Number.isFinite(na) && Number.isFinite(nb);
    if (bothNum) {
      if (na !== nb) return na < nb ? -1 : 1;
    } else {
      const sa = pa[i] ?? "";
      const sb = pb[i] ?? "";
      if (sa !== sb) return sa < sb ? -1 : 1;
    }
  }
  return 0;
}

// `node packages.mjs list` → JSON of the inventory with versions (for debugging
// and for the workflows that consume it via `fromJSON`).
if (process.argv[2] === "list") {
  process.stdout.write(JSON.stringify(packagesWithVersions(), null, 2) + "\n");
}
