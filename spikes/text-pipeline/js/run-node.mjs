// Runs the CORE-003 spike's WebAssembly module (wasm32-unknown-unknown, with wasm-bindgen's `--target web` glue) in
// Node.js: the probe set, the viewport and the measurements, writing the same files as the native runner. Plain
// JavaScript without dependencies, for Node.js 24.
//
// Usage: node run-node.mjs <package folder> <fonts folder> <output folder> <environment> [--committed]

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { runSpike } from "./harness.mjs";

const args = process.argv.slice(2);
const committed = args.includes("--committed");
const [packageFolder, fontsFolder, outputFolder, environment] = args.filter((arg) => arg !== "--committed");
if (!environment) {
  console.error("usage: node run-node.mjs <package folder> <fonts folder> <output folder> <environment> [--committed]");
  process.exit(2);
}

const glue = await import(pathToFileURL(resolve(packageFolder, "text_pipeline.js")).href);
const instance = glue.initSync({ module: readFileSync(join(packageFolder, "text_pipeline_bg.wasm")) });
const results = await runSpike({
  glue,
  memory: () => instance.memory,
  readFont: (file) => readFileSync(join(fontsFolder, file)),
  write: (path, data) => {
    const target = join(outputFolder, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, data);
  },
  environment,
  committed,
  now: () => performance.now(),
});
console.log(`${environment}: layouts ${results.layouts_sha256}  references ${results.references_sha256}`);
