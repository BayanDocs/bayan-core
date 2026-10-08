// Runs one WebAssembly test program (Rust target wasm32-wasip1) in Node.js, for `cargo xtask verify` (work package CORE-002; ADR-0025 §1).
//
// Cargo starts it with the program's path and the test harness's arguments:
//   node --disable-warning=ExperimentalWarning wasi-runner.mjs <program.wasm> [arguments...]
//
// The program gets the WASI interface it needs to report results and nothing more: no files or folders, no environment variables, and no network (WASI has none). Its exit code becomes this process's exit code. A failing Rust test panics, which aborts a WebAssembly program (wasm32-wasip1 does not unwind); Node.js reports that as an uncaught RuntimeError and exits with a non-zero code, so the run fails.
//
// Node.js marks node:wasi as experimental; the flag above only hides the warning that says so on every run.

import { readFile } from 'node:fs/promises';
import process from 'node:process';
import { WASI } from 'node:wasi';

const [program, ...args] = process.argv.slice(2);
if (program === undefined) {
  process.stderr.write('usage: node wasi-runner.mjs <program.wasm> [arguments...]\n');
  process.exit(2);
}

const wasi = new WASI({
  version: 'preview1',
  args: [program, ...args],
  env: {},
  preopens: {},
  returnOnExit: true,
});
const module = await WebAssembly.compile(await readFile(program));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
