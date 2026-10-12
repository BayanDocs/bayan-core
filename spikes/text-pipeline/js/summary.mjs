// Writes the measurements of the CORE-003 spike (AC-3) as Markdown tables, for GitHub's job summary and the report:
// shaping and layout throughput, the viewport's rasterization time, memory, and the WebAssembly module's size. Plain
// JavaScript without dependencies, for Node.js 24.
//
// Usage: node summary.mjs <bench.json>... [--size <name>=<size.json>]... [--summary <file>]
// (the bench.json files of the runners, and the files wasm-size.mjs wrote; the tables go to --summary, or to standard output)

import { appendFileSync, readFileSync } from "node:fs";

const args = process.argv.slice(2);
const benches = [];
const sizes = [];
let summaryFile = null;
for (let index = 0; index < args.length; index += 1) {
  if (args[index] === "--summary") {
    summaryFile = args[++index];
  } else if (args[index] === "--size") {
    const [name, file] = args[++index].split("=");
    sizes.push({ name, size: JSON.parse(readFileSync(file, "utf8")) });
  } else {
    benches.push(JSON.parse(readFileSync(args[index], "utf8")));
  }
}
if (benches.length === 0 && sizes.length === 0) {
  console.error("usage: node summary.mjs <bench.json>... [--size <name>=<size.json>]... [--summary <file>]");
  process.exit(2);
}
for (const bench of benches) {
  if (bench.format !== "bayan-core CORE-003 text-pipeline measurements v1") {
    console.error(`summary: ${bench.environment ?? "a file"} is not a measurements file`);
    process.exit(1);
  }
}

const RASTERIZERS = ["reference", "gamma2", "vello_u8_detected", "vello_u8_fallback", "vello_f32_fallback"];
const lines = [];
const out = (line = "") => lines.push(line);
const thousands = (value) => Math.round(value).toLocaleString("en-US");
const mebibytes = (bytes) => (bytes == null ? "–" : (bytes / 1024 / 1024).toFixed(1));

if (benches.length > 0) {
  benches.sort((a, b) => a.environment.localeCompare(b.environment));
  out("### CORE-003 measurements");
  out();
  out(
    `Each round shapes, or shapes and lays out, the ${benches[0].characters} characters of the 30 probes; the viewport is 3840 × 2160 pixels (1920 × 1080 at device scale 2), filled with the probes. Times are medians of five renderings. GitHub's runners are shared virtual machines, so compare environments only within one run.`,
  );
  out();
  out("| Environment | Shaping (characters/s) | Shaping and layout (characters/s) | Reference (ms) | Gamma-2 (ms) | vello_cpu u8, detected SIMD (ms) | vello_cpu u8, fallback (ms) | vello_cpu f32, fallback (ms) |");
  out("|---|--:|--:|--:|--:|--:|--:|--:|");
  for (const bench of benches) {
    const times = RASTERIZERS.map((name) => bench.viewport.rasterizers[name].median_ms.toFixed(0));
    out(`| \`${bench.environment}\` | ${thousands(bench.shaping.characters_per_second)} | ${thousands(bench.layout.characters_per_second)} | ${times.join(" | ")} |`);
  }
  out();
  out("Peak memory in MiB after each phase: the process's peak resident memory on Linux (VmHWM; not measured on Windows and macOS), and the size of the module's linear memory in WebAssembly, which only grows.");
  out();
  out("| Environment | Fonts loaded | Shaping and layout | Reference raster | Gamma-2 raster | vello_cpu (all three) |");
  out("|---|--:|--:|--:|--:|--:|");
  for (const bench of benches) {
    const memory = bench.memory_bytes ?? {};
    out(`| \`${bench.environment}\` | ${mebibytes(memory.fonts)} | ${mebibytes(memory.layout)} | ${mebibytes(memory.reference)} | ${mebibytes(memory.gamma2)} | ${mebibytes(memory.vello_f32_fallback)} |`);
  }
  out();
}

if (sizes.length > 0) {
  out("### CORE-003 WebAssembly size");
  out();
  out("The module as shipped, without its custom sections (names and tool metadata), compressed with gzip -9 and brotli -11. The parts are compressed one by one, so they add up to a little more than the whole.");
  out();
  out(`| Part | ${sizes.map(({ name }) => `${name}: raw | ${name}: brotli`).join(" | ")} |`);
  out(`|---|${sizes.map(() => "--:|--:").join("|")}|`);
  out(`| **whole module** (gzip: ${sizes.map(({ name, size }) => `${name} ${thousands(size.shipped.gzip)}`).join(", ")}) | ${sizes.map(({ size }) => `${thousands(size.shipped.raw)} | ${thousands(size.shipped.brotli)}`).join(" | ")} |`);
  for (const [index, part] of sizes[0].size.libraries.entries()) {
    out(`| ${part.library} | ${sizes.map(({ size }) => `${thousands(size.libraries[index].bytes)} | ${thousands(size.libraries[index].brotli)}`).join(" | ")} |`);
  }
  out();
}

const text = `${lines.join("\n")}\n`;
if (summaryFile) {
  appendFileSync(summaryFile, text);
} else {
  process.stdout.write(text);
}
