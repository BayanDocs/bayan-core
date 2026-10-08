// bayan-worker.js: the worker host of the BayanDocs engine for the web shell (engine protocol §3.2, ADR-0012, ADR-0014).
//
// Run it as a module worker: new Worker(new URL("bayan-worker.js", base), { type: "module" }). It loads the engine
// (bayan_wasm.js and bayan_wasm_bg.wasm, next to this file), relays protocol messages between the main thread and the
// engine one at a time and in order, carries blobs as ArrayBuffers, delivers tiles as ImageBitmaps, and replaces the
// engine when it panics. It has no dependencies and never looks inside documents. It logs nothing itself; only
// wasm-bindgen's glue warns on the console, without any content, if the module is not served as application/wasm.
//
// SPDX-FileCopyrightText: 2026 BayanDocs contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission

import init, { WasmEngine, __wbg_reset_state, engineVersion, maxBlobBytes, maxMessageBytes } from "./bayan_wasm.js";

/** The largest request and blob identifier, 2⁵³ − 1 (§4); it also stands in for a blob's identifier while a message's size is checked. */
const MAX_ID = Number.MAX_SAFE_INTEGER;

/** The engine, once it is loaded. */
let engine = null;
/** The configuration as JSON text, kept to create a replacement engine after a panic. */
let configText = "";
/** "new" until the engine starts loading, then "starting", "ready" or "failed". After "failed" every message is ignored. */
let state = "new";
/** Whether a message has arrived: only the very first message may be init (§3.2). */
let firstSeen = false;
/** Work runs as a chain of promises, so messages are handled one at a time and answered in order. No link rejects. */
let queue = Promise.resolve();

self.addEventListener("message", (event) => {
  // The time the message arrived, for recordings (§10); the engine itself reads no clock.
  const receivedMs = performance.now();
  const data = event.data;
  const first = !firstSeen;
  firstSeen = true;
  if (isControl(data)) {
    // Only the very first message may be init; any other control message is ignored.
    if (first && data.worker === "init") {
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

/** The request identifier of a message, read by the engine's own rule (§4), or NaN if it has none. */
function requestId(message) {
  const id = isObject(message) ? message.id : undefined;
  return Number.isSafeInteger(id) && id >= 1 ? id : Number.NaN;
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
    // The engine comes only from where this script came from.
    if (wasmUrl.origin !== new URL(import.meta.url).origin) {
      return fail("load_failed");
    }
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
  const id = requestId(message);
  try {
    const bytes = takeBlob(message);
    let text = toJson(message);
    // A message JSON cannot hold, such as one with a BigInt, is answered without the engine (§3.2).
    if (text === null) {
      deliverAll(engine.refuseUnreadable(id));
      return;
    }
    // A message over the limit is answered without copying it into the engine's memory, which would grow to hold it
    // and never shrink again (§13). Its size is measured with a stand-in for the blob's identifier as long as any.
    if (overLimit(text)) {
      deliverAll(engine.refuseOversized(id));
      return;
    }
    let blob = null;
    if (bytes !== null) {
      // A blob over the limit is refused before it is copied into the engine's memory, as blob 0 (§3.1).
      blob = bytes.byteLength > maxBlobBytes() ? 0 : engine.blobPut(bytes);
      message.payload.blob = blob;
      text = toJson(message);
    }
    const answers = JSON.parse(engine.post(text, receivedMs));
    if (blob) {
      engine.blobRelease(blob);
    }
    const tileRequest = isObject(message) && message.type === "render.tile" && !Number.isNaN(id);
    for (const answer of answers) {
      if (tileRequest && answer.re === id && answer.ok === true) {
        await deliverTile(answer, id);
      } else {
        deliver(answer);
      }
    }
  } catch {
    // A panic stops the WebAssembly instance (it traps), and anything else thrown from inside the engine leaves it
    // in an unknown state, so either way the instance is replaced (§3.2, §12).
    restart(id);
  }
}

/** The message as JSON text, or null if JSON cannot hold it. */
function toJson(message) {
  try {
    return JSON.stringify(message) ?? null;
  } catch {
    return null;
  }
}

/**
 * Takes the bytes of a blob that a message carries in `payload.blob` (an ArrayBuffer, a SharedArrayBuffer, or a view
 * of one such as a Uint8Array) and puts the largest identifier in its place, so the message's size can be checked
 * before the bytes are copied into the engine. Returns the bytes, or null if the message carries no blob.
 */
function takeBlob(message) {
  const payload = isObject(message) ? message.payload : undefined;
  if (!isObject(payload)) {
    return null;
  }
  const value = payload.blob;
  let bytes = null;
  if (value instanceof ArrayBuffer || (typeof SharedArrayBuffer === "function" && value instanceof SharedArrayBuffer)) {
    bytes = new Uint8Array(value);
  } else if (ArrayBuffer.isView(value)) {
    bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  if (bytes !== null) {
    payload.blob = MAX_ID;
  }
  return bytes;
}

/**
 * Whether text is longer than the engine's message limit (§13) in UTF-8, the form in which wasm-bindgen copies it
 * into the engine (a lone surrogate becomes U+FFFD, 3 bytes). Each UTF-16 code unit takes 1 to 3 bytes, so only
 * lengths in between need counting.
 */
function overLimit(text) {
  const limit = maxMessageBytes();
  if (text.length > limit) {
    return true;
  }
  if (text.length * 3 <= limit) {
    return false;
  }
  let bytes = 0;
  for (let index = 0; index < text.length; index += 1) {
    const unit = text.charCodeAt(index);
    if (unit < 0x80) {
      bytes += 1;
    } else if (unit < 0x800) {
      bytes += 2;
    } else if (unit >= 0xd800 && unit <= 0xdbff && (text.charCodeAt(index + 1) & 0xfc00) === 0xdc00) {
      // A surrogate pair: one character of 4 bytes.
      bytes += 4;
      index += 1;
    } else {
      bytes += 3;
    }
    if (bytes > limit) {
      return true;
    }
  }
  return false;
}

/** Delivers the engine's answers, given as the text of a JSON array. */
function deliverAll(answersJson) {
  for (const answer of JSON.parse(answersJson)) {
    deliver(answer);
  }
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

/**
 * Delivers the reply to render.tile request `id` with an ImageBitmap in place of the blob and stride (§3.2, §6.4).
 * Every other field of the reply is kept, so fields that later versions add (§9) reach the web shell too.
 */
async function deliverTile(answer, id) {
  const { width, height, stride, blob } = answer.payload;
  const bytes = engine.blobTake(blob);
  let bitmap;
  try {
    if (bytes === undefined || bytes.length < stride * (height - 1) + width * 4) {
      throw new RangeError("tile");
    }
    bitmap = await createImageBitmap(new ImageData(unpremultiply(bytes, stride, width, height), width, height));
  } catch {
    // The engine is fine; only the browser could not make the bitmap. The request gets the error internal.
    deliverAll(engine.reportHostFailure(id));
    return;
  }
  const payload = { ...answer.payload, bitmap };
  delete payload.blob;
  delete payload.stride;
  self.postMessage({ ...answer, payload }, [bitmap]);
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
 * Replaces an engine that panicked with a new one from the same configuration, and lets the new one answer request
 * `id`, the message that caused the panic (§3.2). Only the identifier is passed, so the new instance never copies or
 * parses that message again. The old engine object belongs to the discarded instance: it must not be used or freed
 * (wasm-bindgen refuses calls on it, and its finalizer leaves the new instance alone).
 */
function restart(id) {
  engine = null;
  try {
    __wbg_reset_state();
    engine = new WasmEngine(configText);
    deliverAll(engine.reportPanic(id));
  } catch {
    fail("restart_failed");
  }
}
