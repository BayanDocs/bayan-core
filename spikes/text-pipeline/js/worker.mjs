// The Web Worker of the Chromium runner: it loads the spike's module, runs it with the fonts the server serves, and
// sends every file back to the server, which writes it.

import init, * as glue from "./pkg/text_pipeline.js";
import { runSpike } from "./harness.mjs";

self.onmessage = async (event) => {
  try {
    const instance = await init({ module_or_path: "./pkg/text_pipeline_bg.wasm" });
    const results = await runSpike({
      glue,
      memory: () => instance.memory,
      readFont: async (file) => {
        const response = await fetch(`fonts/${file}`);
        if (!response.ok) {
          throw new Error(`cannot load the font ${file}: HTTP ${response.status}`);
        }
        return new Uint8Array(await response.arrayBuffer());
      },
      write: async (path, body) => {
        const response = await fetch(`out/${path}`, { method: "PUT", body });
        if (!response.ok) {
          throw new Error(`cannot write ${path}: HTTP ${response.status}`);
        }
      },
      environment: event.data.environment,
      committed: false,
      now: () => performance.now(),
    });
    self.postMessage({ ok: true, layouts: results.layouts_sha256, references: results.references_sha256 });
  } catch (error) {
    // A Rust panic arrives here as a trap ("unreachable").
    self.postMessage({ ok: false, message: String(error?.stack ?? error) });
  }
};
