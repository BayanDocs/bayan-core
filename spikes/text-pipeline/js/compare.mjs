// Compares the results files of the CORE-003 environments: every probe's layout hash (AC-1) and reference raster hash
// (AC-2), and the viewport's, must be the same in every environment and equal to the committed expectation; the other
// renderings (gamma-2 blending, and vello_cpu's configurations) are reported, as are vello_cpu's differences from the
// reference (AC-3). Plain JavaScript without dependencies, for Node.js 24.
//
// Usage: node compare.mjs <expected.json> <results.json>... [--summary <file>]
// Exits with 1 if anything that must match does not, and writes a Markdown summary (for GitHub's job summary) to
// --summary, or to standard output.
//
// node compare.mjs --expect <results.json> prints the expectation that a results file sets: only the hashes that must
// match. A deliberate change of the output (a new probe, a dependency update that changes glyphs) updates
// results/expected.json this way, and its pull request says why (ADR-0004 §4, layout epochs).

import { appendFileSync, readFileSync } from "node:fs";

const args = process.argv.slice(2);
if (args[0] === "--expect") {
  const run = JSON.parse(readFileSync(args[1], "utf8"));
  const expectation = {
    format: run.format,
    probes: run.probes.map(({ id, layout_sha256, reference_sha256 }) => ({ id, layout_sha256, reference_sha256 })),
    viewport: run.viewport && {
      width: run.viewport.width,
      height: run.viewport.height,
      layouts_sha256: run.viewport.layouts_sha256,
      reference_sha256: run.viewport.reference_sha256,
    },
  };
  process.stdout.write(`${JSON.stringify(expectation, null, 2)}\n`);
  process.exit(0);
}
const summaryIndex = args.indexOf("--summary");
const summaryFile = summaryIndex >= 0 ? args.splice(summaryIndex, 2)[1] : null;
const [expectedFile, ...resultFiles] = args;
if (!expectedFile || resultFiles.length === 0) {
  console.error("usage: node compare.mjs <expected.json> <results.json>... [--summary <file>]");
  process.exit(2);
}

const FORMAT = "bayan-core CORE-003 text-pipeline results v1";
const expected = JSON.parse(readFileSync(expectedFile, "utf8"));
const runs = resultFiles.map((file) => JSON.parse(readFileSync(file, "utf8")));
const problems = [];
const lines = [];
const out = (line = "") => lines.push(line);

for (const [index, run] of runs.entries()) {
  if (run.format !== FORMAT) {
    problems.push(`${resultFiles[index]} is not a results file of format "${FORMAT}"`);
  }
}
const environments = runs.map((run) => run.environment);
if (new Set(environments).size !== environments.length) {
  problems.push(`an environment appears twice: ${environments.join(", ")}`);
}

/** Checks that `value(run)` is the same for every run and equal to `wanted`, and records a problem naming `what`. */
function mustMatch(what, value, wanted) {
  const values = runs.map(value);
  const differing = runs.filter((_, index) => values[index] !== wanted).map((run) => run.environment);
  if (differing.length > 0) {
    problems.push(`${what}: ${differing.join(", ")} differ from the expected ${String(wanted).slice(0, 16)}`);
  }
  return differing.length === 0;
}

/** The environments grouped by the value they have, for what is reported rather than required. */
function groups(value) {
  const byValue = new Map();
  for (const run of runs) {
    const key = value(run) ?? "(missing)";
    byValue.set(key, [...(byValue.get(key) ?? []), run.environment]);
  }
  return byValue;
}

const probeIds = expected.probes.map((probe) => probe.id);
for (const run of runs) {
  const ids = run.probes.map((probe) => probe.id);
  if (ids.join(",") !== probeIds.join(",")) {
    problems.push(`${run.environment} ran the probes ${ids.join(", ")}, not those of the expectation`);
  }
}
const probe = (run, id) => run.probes.find((entry) => entry.id === id);

out("### CORE-003: the same layout and reference raster everywhere");
out();
out(`Environments: ${environments.map((environment) => `\`${environment}\``).join(", ")}.`);
out();
out("| Probe | Layout (AC-1) | Reference raster (AC-2) | Gamma-2 raster | vello_cpu u8, detected SIMD | vello_cpu u8, fallback | vello_cpu f32, fallback | vello_cpu vs reference: max difference, pixels beyond ±2 |");
out("|---|---|---|---|---|---|---|---|");
const mark = (ok) => (ok ? "same everywhere" : "**DIFFERENT**");
const variety = (byValue) =>
  byValue.size === 1 ? "same everywhere" : [...byValue.values()].map((names) => names.join(" + ")).join(" / ");
for (const wanted of expected.probes) {
  const id = wanted.id;
  const layout = mustMatch(`${id}: layout`, (run) => probe(run, id)?.layout_sha256, wanted.layout_sha256);
  const reference = mustMatch(`${id}: reference raster`, (run) => probe(run, id)?.reference_sha256, wanted.reference_sha256);
  const gamma = variety(groups((run) => probe(run, id)?.gamma2_sha256));
  const vello = [0, 1, 2].map((index) => variety(groups((run) => probe(run, id)?.vello?.[index]?.sha256)));
  const worst = runs
    .map((run) => probe(run, id)?.vello ?? [])
    .flat()
    .reduce((acc, entry) => ({ max: Math.max(acc.max, entry.difference.max), beyond: Math.max(acc.beyond, entry.difference.beyond_2), pixels: entry.difference.pixels }), { max: 0, beyond: 0, pixels: 1 });
  out(`| ${id} | ${mark(layout)} | ${mark(reference)} | ${gamma} | ${vello.join(" | ")} | ${worst.max}, ${((100 * worst.beyond) / worst.pixels).toFixed(2)} % |`);
}
if (expected.viewport) {
  const wanted = expected.viewport;
  const layouts = mustMatch("viewport: layouts", (run) => run.viewport?.layouts_sha256, wanted.layouts_sha256);
  const reference = mustMatch("viewport: reference raster", (run) => run.viewport?.reference_sha256, wanted.reference_sha256);
  const gamma = variety(groups((run) => run.viewport?.gamma2_sha256));
  const vello = [0, 1, 2].map((index) => variety(groups((run) => run.viewport?.vello?.[index]?.sha256)));
  out(`| viewport (${wanted.width} × ${wanted.height}) | ${mark(layouts)} | ${mark(reference)} | ${gamma} | ${vello.join(" | ")} | |`);
}
out();
out("SIMD levels vello_cpu detected: " + runs.map((run) => `\`${run.environment}\` ${run.probes[0]?.vello?.[0]?.level ?? "?"}`).join(", ") + ".");
out();
if (problems.length === 0) {
  out(`Every layout hash and every reference raster hash is the same in all ${runs.length} environments, and equal to spikes/text-pipeline/results/expected.json.`);
} else {
  out("**Differences:**");
  out();
  for (const problem of problems) {
    out(`- ${problem}`);
  }
}

const text = `${lines.join("\n")}\n`;
if (summaryFile) {
  appendFileSync(summaryFile, text);
} else {
  process.stdout.write(text);
}
if (problems.length > 0) {
  for (const problem of problems) {
    console.error(`compare: ${problem}`);
  }
  process.exit(1);
}
console.log(`compare: ${runs.length} environments agree on every layout and reference raster`);
