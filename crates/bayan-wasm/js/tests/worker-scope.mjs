// Runs bayan-worker.js inside a Node.js worker thread that imitates a browser's dedicated worker: `self`, message
// events, postMessage, fetch for file: addresses, ImageData and createImageBitmap. Started by node-driver.mjs with
// workerData = { host: <file URL of bayan-worker.js>, failBitmapWidth: <a tile width whose bitmap creation fails>,
// slowBitmapWidth: <a tile width whose bitmap takes a while> }.
//
// For the driver's checks, and never seen by the worker host, the message { "bayan-test": "memory" } is answered with
// the size of the engine's WebAssembly memory: { "bayan-test": "memory", bytes }.
//
// SPDX-FileCopyrightText: 2026 BayanDocs contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission

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
// Node.js's fetch does not read files; serve the engine module the way a web server would. Any http: or https: address
// serves the same module, so the driver can check that the worker host refuses one from another origin.
globalThis.fetch = async (url) => {
  const address = new URL(url);
  let file;
  if (address.protocol === "file:") {
    file = fileURLToPath(address);
  } else if (address.protocol === "http:" || address.protocol === "https:") {
    file = fileURLToPath(new URL(address.pathname.split("/").pop(), workerData.host));
  } else {
    throw new TypeError("only file:, http: and https: addresses in this test");
  }
  const bytes = await readFile(file);
  return new Response(bytes, { headers: { "Content-Type": "application/wasm" } });
};
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
