// The part of the CORE-003 WebAssembly runners that is the same in Node.js and in a browser's Web Worker: it hands the
// font files to the module, runs the probe set and the viewport, writes every file the native runner writes, and times
// the measurements with the host's clock (the module itself reads no clock). Plain JavaScript without dependencies.

/** The rasterizers that the viewport measurement times, as the module names them. */
export const RASTERIZERS = ["reference", "gamma2", "vello_u8_detected", "vello_u8_fallback", "vello_f32_fallback"];

/**
 * Runs the spike with the module `glue` (wasm-bindgen's glue, already initialized) and writes its files through
 * `write(path, bytesOrText)`: `results.json`, `layout/<probe>.json`, `png/<probe>-<rendering>.png` and, with every font,
 * `bench.json`. `readFont(file)` returns a font file's bytes, `memory()` the module's WebAssembly.Memory, and `now()`
 * the time in milliseconds. Returns the parsed results file.
 */
export async function runSpike({ glue, memory, readFont, write, environment, committed, now }) {
  const pipeline = new glue.Pipeline();
  try {
    for (const font of JSON.parse(glue.Pipeline.pinnedFonts())) {
      if (!committed || font.committed) {
        pipeline.addFont(font.file, await readFont(font.file));
      }
    }
    // The measurements come first, while the module holds only its fonts: WebAssembly memory only grows, and the
    // results run below needs more, two of the viewport's images at a time and the PNG files of the output it is on.
    const bench = committed ? null : measure(pipeline, environment, memory, now);
    // One output at a time, a probe's or the viewport's, as the native runner writes them: the module keeps only the
    // files of the output it made last.
    pipeline.startRun(environment, committed);
    for (let id = pipeline.step(); id !== undefined; id = pipeline.step()) {
      const layout = pipeline.layoutJson();
      if (layout.length > 0) {
        await write(`layout/${id}.json`, layout);
      }
      for (const name of pipeline.imageNames().split(",")) {
        await write(`png/${id}-${name}.png`, pipeline.takePng(name));
      }
    }
    const text = pipeline.results();
    await write("results.json", text);
    const results = JSON.parse(text);
    if (bench) {
      // The measurement draws the same scene as the results file's viewport, so the hashes must agree.
      if (bench.viewport.rasterizers.reference.sha256 !== results.viewport.reference_sha256) {
        throw new Error("the measured viewport is not the one in results.json");
      }
      await write("bench.json", `${JSON.stringify(bench, null, 2)}\n`);
    }
    return results;
  } finally {
    pipeline.free();
  }
}

/** Runs `round` until at least two seconds have passed (after one round to warm up) and returns the rounds and the time. */
function rounds(round, now) {
  round();
  let count = 0;
  const start = now();
  let elapsed = 0;
  while (count < 3 || elapsed < 2000) {
    round();
    count += 1;
    elapsed = now() - start;
  }
  return { rounds: count, ms: elapsed };
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

/** The measurements of AC-3, in the format the native runner writes too. */
function measure(pipeline, environment, memory, now) {
  // WebAssembly memory only grows, so its size after each phase is the most that the run so far has needed: heap,
  // stack and the module's data (ICU4X's segmentation data among them) together.
  const memoryBytes = { fonts: memory().buffer.byteLength };
  const characters = pipeline.prepareBench();
  const shaping = rounds(() => pipeline.shapeRound(), now);
  const layout = rounds(() => pipeline.layoutRound(), now);
  memoryBytes.layout = memory().buffer.byteLength;
  const viewport = {};
  for (const rasterizer of RASTERIZERS) {
    const runs = [];
    let sha256 = "";
    for (let run = 0; run < 5; run += 1) {
      const start = now();
      sha256 = pipeline.renderViewport(rasterizer);
      runs.push(now() - start);
    }
    viewport[rasterizer] = { median_ms: median(runs), runs_ms: runs, sha256 };
    memoryBytes[rasterizer] = memory().buffer.byteLength;
  }
  return {
    format: "bayan-core CORE-003 text-pipeline measurements v1",
    environment,
    characters,
    shaping: { ...shaping, characters_per_second: (characters * shaping.rounds * 1000) / shaping.ms },
    layout: { ...layout, characters_per_second: (characters * layout.rounds * 1000) / layout.ms },
    viewport: { rasterizers: viewport },
    memory_bytes: memoryBytes,
  };
}
