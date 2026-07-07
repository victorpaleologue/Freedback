// Decide which packages need a release right now: those whose current version
// has no corresponding git tag yet. Idempotent — re-running after everything is
// tagged plans nothing.
//
// Usage (repo root, in CI):
//   node .github/scripts/plan-releases.mjs
// Prints a JSON array of {name, kind, tag, version, path, bin?, releaseWorkflow?}
// for the packages to release. Writes it to $GITHUB_OUTPUT as `packages=` and
// `any=` when that env var is set.
import { appendFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { packagesWithVersions } from "./packages.mjs";

function existingTags() {
  // Local tags plus whatever the checkout fetched. CI fetches tags first.
  const out = execFileSync("git", ["tag", "--list"], { encoding: "utf8" });
  return new Set(out.split(/\r?\n/).filter(Boolean));
}

const tags = existingTags();
const toRelease = packagesWithVersions()
  .filter((p) => p.version && !tags.has(p.tag))
  .map((p) => ({
    name: p.name,
    kind: p.kind,
    tag: p.tag,
    version: p.version,
    path: p.path,
    // Empty string, NOT null: GitHub Actions evaluates `null != ''` as true,
    // which would make the binary-build steps run for library crates. `''`
    // makes `matrix.package.bin != ''` correctly false for non-binary packages.
    bin: p.bin ?? "",
    releaseWorkflow: p.releaseWorkflow ?? "",
  }));

const json = JSON.stringify(toRelease);
process.stdout.write(json + "\n");

if (process.env.GITHUB_OUTPUT) {
  appendFileSync(process.env.GITHUB_OUTPUT, `packages=${json}\n`);
  appendFileSync(process.env.GITHUB_OUTPUT, `any=${toRelease.length > 0}\n`);
}

if (toRelease.length) {
  console.error(`Planning ${toRelease.length} release(s): ${toRelease.map((p) => p.tag).join(", ")}`);
} else {
  console.error("Nothing to release — every package's current version is already tagged.");
}
