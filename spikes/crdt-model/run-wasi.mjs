// Runs a Rust program compiled for wasm32-wasip1 under Node.js's built-in WASI support (node:wasi), the way the CORE-004 benchmarks measure WebAssembly (spikes/crdt-model/REPORT.md).
//
// Usage: node run-wasi.mjs <program.wasm> <data-directory> [program arguments...]
//
// The program sees <data-directory> as /data (for example a snapshot to load), receives the remaining arguments, and has no environment variables. It prints its own measurements; after it exits, this script prints the time V8 took to compile the module and the final size of its linear memory, which only ever grows, so it is the program's peak memory. Node.js prints a warning that WASI is experimental; that is expected.
import { readFile } from "node:fs/promises";
import { argv, exit, stderr } from "node:process";
import { performance } from "node:perf_hooks";
import { WASI } from "node:wasi";

const [wasmPath, dataDirectory, ...programArguments] = argv.slice(2);
if (!wasmPath || !dataDirectory) {
  stderr.write("usage: node run-wasi.mjs <program.wasm> <data-directory> [program arguments...]\n");
  exit(2);
}

const wasi = new WASI({
  version: "preview1",
  args: ["program", ...programArguments],
  env: {},
  preopens: { "/data": dataDirectory },
  returnOnExit: true,
});

const bytes = await readFile(wasmPath);
const compileStart = performance.now();
const module = await WebAssembly.compile(bytes);
const compileMilliseconds = performance.now() - compileStart;
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
const code = wasi.start(instance);

console.log(`metric wasm.module_bytes ${bytes.length} bytes`);
console.log(`metric wasm.compile_ms ${compileMilliseconds.toFixed(3)} ms`);
console.log(`metric wasm.linear_memory_final ${instance.exports.memory.buffer.byteLength} bytes`);
exit(code);
