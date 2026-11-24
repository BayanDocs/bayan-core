// Runs bayan-worker.js inside a Node.js worker thread that imitates a browser's dedicated worker: `self`, message
// events, postMessage, fetch for file: addresses, ImageData and createImageBitmap. Started by node-driver.mjs with
// workerData = { host: <file URL of bayan-worker.js>, failBitmapWidth: <a tile width whose bitmap creation fails>,
// slowBitmapWidth: <a tile width whose bitmap takes a while> }.
//
// For the driver's checks, and never seen by the worker host, the message { "bayan-test": "memory" } is answered with
// the size of the engine's WebAssembly memory: { "bayan-test": "memory", bytes }; and { "bayan-test": "clock", ms } sets
// the time that performance.now() gives from then on, so the driver can show the restart allowance over ten minutes
// without waiting them (ms null lets the real clock run again).
//
// SPDX-FileCopyrightText: 2026 BayanDocs contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { parentPort, workerData } from "node:worker_threads";

const listeners = [];

/** A stand-in for ImageBitmap: Node.js has none. It carries the pixels so the driver can check them. */
class FakeImageBitmap {
  constructor(imageData) {
    this.width = imageData.width;
    this.height = imageData.height;
    this.data = new Uint8ClampedArray(imageData.data);
  }
}

globalThis.self = globalThis;
globalThis.addEventListener = (type, listener) => {
  if (type === "message") {
    listeners.push(listener);
  }
};
// A browser transfers an ImageBitmap; Node.js cannot, so the stand-in's pixel buffer is transferred instead.
globalThis.postMessage = (message, transfer = []) => {
  parentPort.postMessage(
    message,
    transfer.map((item) => (item instanceof FakeImageBitmap ? item.data.buffer : item)),
  );
};
/**
 * Whether bytes match Subresource Integrity metadata as a browser decides it: the strongest algorithm the metadata
 * names must have a matching hash among its tokens.
 */
function integrityMatches(bytes, metadata) {
  const strength = { sha256: 1, sha384: 2, sha512: 3 };
  const tokens = metadata
    .split(/\s+/)
    .map((token) => token.match(/^(sha256|sha384|sha512)-([A-Za-z0-9+/]+={0,2})$/))
    .filter((match) => match !== null);
  if (tokens.length === 0) {
    return true;
  }
  const strongest = Math.max(...tokens.map((match) => strength[match[1]]));
  return tokens
    .filter((match) => strength[match[1]] === strongest)
    .some((match) => createHash(match[1]).update(bytes).digest("base64") === match[2]);
}

// Node.js's fetch does not read files; serve the engine module the way a web server would. Any http: or https: address
// serves the same module, so the driver can check that the worker host refuses one from another origin. A file named
// REDIRECTING_FILE, next to the worker host, answers with a redirect to another origin that serves the module, which a
// fetch with redirect "error" refuses, as a browser's does. Like a browser, it refuses bytes that do not match the
// request's integrity metadata.
const REDIRECTING_FILE = "redirect-to-elsewhere.wasm";
globalThis.fetch = async (url, init = {}) => {
  let address = new URL(url);
  if (address.protocol === "file:" && address.pathname.endsWith(`/${REDIRECTING_FILE}`)) {
    if (init.redirect === "error") {
      throw new TypeError("Failed to fetch: the response is a redirect");
    }
    address = new URL("https://elsewhere.example/bayan_wasm_bg.wasm");
  }
  let file;
  if (address.protocol === "file:") {
    file = fileURLToPath(address);
  } else if (address.protocol === "http:" || address.protocol === "https:") {
    file = fileURLToPath(new URL(address.pathname.split("/").pop(), workerData.host));
  } else {
    throw new TypeError("only file:, http: and https: addresses in this test");
  }
  const bytes = await readFile(file);
  if (typeof init.integrity === "string" && init.integrity !== "" && !integrityMatches(bytes, init.integrity)) {
    throw new TypeError("Failed to fetch: the response does not match its integrity metadata");
  }
  return new Response(bytes, { headers: { "Content-Type": "application/wasm" } });
};

// The worker host's clock, which the driver can set (see the top of this file).
let fixedClock = null;
const realNow = performance.now.bind(performance);
performance.now = () => fixedClock ?? realNow();
globalThis.ImageData = class ImageData {
  constructor(data, width, height) {
    if (!(data instanceof Uint8ClampedArray) || data.length !== width * height * 4) {
      throw new RangeError("ImageData: the data does not match the size");
    }
    this.data = data;
    this.width = width;
    this.height = height;
  }
};
globalThis.createImageBitmap = async (imageData) => {
  if (imageData.width === workerData.failBitmapWidth) {
    throw new DOMException("the test makes this bitmap fail", "InvalidStateError");
  }
  if (imageData.width === workerData.slowBitmapWidth) {
    // A browser creates bitmaps asynchronously; a slow one shows whether later answers wait for it.
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  return new FakeImageBitmap(imageData);
};

// The engine's WebAssembly memory, from its first instance (a panic replaces the instance, and with it the memory).
// Node.js runs WebAssembly of its own too (its HTTP parser), so the engine's instance is the one with the engine's
// exports.
let memory = null;
for (const name of ["instantiate", "instantiateStreaming"]) {
  const original = WebAssembly[name];
  WebAssembly[name] = async (...args) => {
    const result = await original.apply(WebAssembly, args);
    const exports = (result.instance ?? result).exports;
    if (memory === null && "wasmengine_new" in exports) {
      memory = exports.memory;
    }
    return result;
  };
}

// A browser queues a module worker's messages until its script has run; so does this scope.
const early = [];
let loaded = false;
function dispatch(data) {
  for (const listener of listeners) {
    listener({ data });
  }
}
parentPort.on("message", (data) => {
  if (data?.["bayan-test"] === "memory") {
    parentPort.postMessage({ "bayan-test": "memory", bytes: memory?.buffer.byteLength ?? null });
    return;
  }
  if (data?.["bayan-test"] === "clock") {
    fixedClock = data.ms;
    return;
  }
  if (loaded) {
    dispatch(data);
  } else {
    early.push(data);
  }
});

await import(workerData.host);
loaded = true;
for (const data of early.splice(0)) {
  dispatch(data);
}
