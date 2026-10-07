// bayan-worker.js: the worker host of the BayanDocs engine for the web shell (engine protocol §3.2, ADR-0012, ADR-0014).
//
// Run it as a module worker: new Worker(new URL("bayan-worker.js", base), { type: "module" }). It loads the engine
// (bayan_wasm.js and bayan_wasm_bg.wasm, next to this file), relays protocol messages between the main thread and the
// engine one at a time and in order, carries blobs as ArrayBuffers, delivers tiles as ImageBitmaps, and replaces the
// engine when it panics. It has no dependencies, logs nothing and never looks inside documents.
//
// SPDX-FileCopyrightText: 2026 BayanDocs contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission

import init, { WasmEngine, __wbg_reset_state, engineVersion, maxBlobBytes } from "./bayan_wasm.js";

/** The engine, once it is loaded. */
let engine = null;
/** The configuration as JSON text, kept to create a replacement engine after a panic. */
let configText = "";
/** "new" until the first message, then "starting", "ready" or "failed". After "failed" every message is ignored. */
let state = "new";
/** Work runs as a chain of promises, so messages are handled one at a time and answered in order. No link rejects. */
let queue = Promise.resolve();

self.addEventListener("message", (event) => {
  // The time the message arrived, for recordings (§10); the engine itself reads no clock.
  const receivedMs = performance.now();
  const data = event.data;
  if (isControl(data)) {
    // Only the first message may be init; any other control message is ignored.
    if (state === "new" && data.worker === "init") {
      state = "starting";
      queue = queue.then(() => start(data));
    }
    return;
  }
  if (state === "new") {
    state = "starting";
    queue = queue.then(() => start({}));
  }
  queue = queue.then(() => relay(data, receivedMs));
});

/** Whether a message is a control message: an object with a `worker` field and no `v` (§3.2). */
function isControl(data) {
  return isObject(data) && "worker" in data && !("v" in data);
}

function isObject(value) {
  return typeof value === "object" && value !== null;
}

/** Loads the engine and answers "ready", or "failed" with a fixed reason. */
async function start(control) {
  try {
    configText = control.config === undefined ? "" : JSON.stringify(control.config);
  } catch {
    return fail("invalid_config");
  }
  try {
    const wasmUrl = new URL(control.wasm_url ?? "bayan_wasm_bg.wasm", import.meta.url);
    await init({ module_or_path: wasmUrl });
  } catch {
    return fail("load_failed");
  }
  try {
    engine = new WasmEngine(configText);
    state = "ready";
    self.postMessage({ worker: "ready", engine_version: engineVersion() });
  } catch {
    fail("invalid_config");
  }
}

function fail(reason) {
  state = "failed";
  engine = null;
  self.postMessage({ worker: "failed", reason });
}

/** Hands one protocol message to the engine and delivers its answers, in order. */
async function relay(message, receivedMs) {
  if (state !== "ready") {
    return;
  }
  // The message as it arrived, to report a panic while its blob is stored (an ArrayBuffer becomes {} in JSON).
  let text = toJson(message);
  try {
    const blob = storeBlob(message);
    if (blob !== null) {
      text = toJson(message);
    }
    const answers = JSON.parse(engine.post(text, receivedMs));
    if (blob) {
      engine.blobRelease(blob);
    }
    const tileRequest = isObject(message) && message.type === "render.tile" ? message.id : undefined;
    for (const answer of answers) {
      if (tileRequest !== undefined && answer.re === tileRequest && answer.ok === true) {
        await deliverTile(answer, text);
      } else {
        deliver(answer);
      }
    }
  } catch {
    // A panic stops the WebAssembly instance (it traps), and anything else thrown from inside the engine leaves it
    // in an unknown state, so either way the instance is replaced (§3.2, §12).
    restart(text);
  }
}

/** The message as JSON text. A value JSON cannot hold becomes text the engine refuses as invalid_message (§12). */
function toJson(message) {
  try {
    return JSON.stringify(message) ?? "";
  } catch {
    return "";
  }
}

/**
 * Moves an ArrayBuffer in `payload.blob` into a new blob and puts the blob's identifier in its place. Returns the
 * identifier, or null if the message has no ArrayBuffer there. A buffer that cannot be stored becomes blob 0, which
 * the engine answers with limit_exceeded (§3.1).
 */
function storeBlob(message) {
  const payload = isObject(message) ? message.payload : undefined;
  if (!isObject(payload) || !(payload.blob instanceof ArrayBuffer)) {
    return null;
  }
  const buffer = payload.blob;
  // A buffer over the limit is refused before it is copied into the engine's memory (§13).
  payload.blob = buffer.byteLength > maxBlobBytes() ? 0 : engine.blobPut(new Uint8Array(buffer));
  return payload.blob;
}

/** Sends one of the engine's messages to the main thread; a blob it names travels as a transferred ArrayBuffer. */
function deliver(answer) {
  const payload = answer.payload;
  if (isObject(payload) && typeof payload.blob === "number") {
    const bytes = engine.blobTake(payload.blob);
    if (bytes !== undefined) {
      payload.blob = bytes.buffer;
      self.postMessage(answer, [bytes.buffer]);
      return;
    }
  }
  self.postMessage(answer);
}

/** Delivers the reply to render.tile with an ImageBitmap in place of the blob and stride (§3.2, §6.4). */
async function deliverTile(answer, requestText) {
  const { width, height, stride, hash, blob } = answer.payload;
  const bytes = engine.blobTake(blob);
  let bitmap;
  try {
    if (bytes === undefined || bytes.length < stride * (height - 1) + width * 4) {
      throw new RangeError("tile");
    }
    bitmap = await createImageBitmap(new ImageData(unpremultiply(bytes, stride, width, height), width, height));
  } catch {
    // The engine is fine; only the browser could not make the bitmap. The request gets the error internal.
    for (const failure of JSON.parse(engine.reportHostFailure(requestText))) {
      deliver(failure);
    }
    return;
  }
  const reply = { v: answer.v, re: answer.re, ok: true, payload: { width, height, hash, bitmap } };
  self.postMessage(reply, [bitmap]);
}

/**
 * Converts the engine's premultiplied RGBA8 pixels, rows `stride` bytes apart, to the non-premultiplied RGBA of
 * ImageData. Pixels with alpha 0 or 255 are unchanged.
 */
function unpremultiply(source, stride, width, height) {
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    let from = y * stride;
    let to = y * width * 4;
    for (let x = 0; x < width; x += 1, from += 4, to += 4) {
      const alpha = source[from + 3];
      if (alpha === 0 || alpha === 255) {
        pixels[to] = source[from];
        pixels[to + 1] = source[from + 1];
        pixels[to + 2] = source[from + 2];
      } else {
        const half = alpha >> 1;
        pixels[to] = Math.min(255, Math.floor((source[from] * 255 + half) / alpha));
        pixels[to + 1] = Math.min(255, Math.floor((source[from + 1] * 255 + half) / alpha));
        pixels[to + 2] = Math.min(255, Math.floor((source[from + 2] * 255 + half) / alpha));
      }
      pixels[to + 3] = alpha;
    }
  }
  return pixels;
}

/**
 * Replaces an engine that panicked with a new one from the same configuration, and lets the new one answer the
 * message that caused the panic (§3.2). The old engine object belongs to the discarded instance: it must not be used
 * or freed (wasm-bindgen refuses calls on it, and its finalizer leaves the new instance alone).
 */
function restart(messageText) {
  engine = null;
  try {
    __wbg_reset_state();
    engine = new WasmEngine(configText);
    for (const answer of JSON.parse(engine.reportPanic(messageText))) {
      deliver(answer);
    }
  } catch {
    fail("restart_failed");
  }
}
