// The Node.js test driver of the WebAssembly package (CORE-007): it runs bayan-worker.js the way the web shell does,
// in a worker of its own, and goes through handshake → open → render a tile → record → replay, then the panic path and
// the worker host's edge cases. It uses only Node.js itself: no npm packages.
//
// Usage: node node-driver.mjs <directory of the WebAssembly package>
// It prints one line per check and, last, "tile <hash>": the digest of the tile, which the C driver prints for the
// same tile, so CI can compare native and WebAssembly output across platforms. It exits with status 1 on failure.
//
// SPDX-FileCopyrightText: 2026 BayanDocs contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission

import assert from "node:assert/strict";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { Worker } from "node:worker_threads";

const packageDir = path.resolve(process.argv[2] ?? ".");
const host = pathToFileURL(path.join(packageDir, "bayan-worker.js")).href;
const scope = new URL("./worker-scope.mjs", import.meta.url);
/** The width of a tile whose ImageBitmap the fake scope refuses to create. */
const FAILING_BITMAP_WIDTH = 7;
const TIMEOUT_MS = 30_000;

/** 1 DIP (device-independent pixel, 1/96 inch) is 19,050 BLU at zoom 1 (§7). */
const BLU_PER_DIP = 19_050;

/** A web shell's side of one worker. */
class Shell {
  constructor() {
    this.inbox = [];
    this.wake = () => {};
    this.failure = null;
    this.worker = new Worker(scope, { workerData: { host, failBitmapWidth: FAILING_BITMAP_WIDTH } });
    this.worker.on("message", (message) => {
      this.inbox.push(message);
      this.wake();
    });
    this.worker.on("error", (error) => {
      this.failure = error;
      this.wake();
    });
  }

  send(message, transfer = []) {
    this.worker.postMessage(message, transfer);
  }

  /** Waits for the first message that matches, and takes it out of the inbox. */
  async take(matches, what) {
    const deadline = Date.now() + TIMEOUT_MS;
    for (;;) {
      const index = this.inbox.findIndex(matches);
      if (index >= 0) {
        return this.inbox.splice(index, 1)[0];
      }
      if (this.failure !== null) {
        throw this.failure;
      }
      const left = deadline - Date.now();
      if (left <= 0) {
        throw new Error(`no ${what} within ${TIMEOUT_MS} ms`);
      }
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, left);
        this.wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
    }
  }

  status() {
    return this.take((message) => "worker" in message, "worker status");
  }

  reply(id) {
    return this.take((message) => message.re === id, `reply to ${id}`);
  }

  event(type) {
    return this.take((message) => message.re === undefined && message.type === type, `${type} event`);
  }

  request(id, type, payload, transfer = []) {
    this.send({ v: 0, id, type, ...(payload === undefined ? {} : { payload }) }, transfer);
    return this.reply(id);
  }

  close() {
    return this.worker.terminate();
  }
}

function ok(reply) {
  assert.equal(reply.ok, true, JSON.stringify(reply));
  return reply.payload;
}

function errorOf(reply) {
  assert.equal(reply.ok, false, JSON.stringify(reply));
  return reply.error;
}

/** FNV-1a 64, the digest of tiles and recordings (§10). */
function fnv1a64(bytes) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of bytes) {
    hash = ((hash ^ BigInt(byte)) * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return `fnv1a64:${hash.toString(16).padStart(16, "0")}`;
}

/** A 64 × 64 pixel tile one inch inside page 0, at zoom 1: entirely on the page, so every pixel is opaque. */
function tileRequest(docId, size = 64) {
  return {
    doc_id: docId,
    page: 0,
    rect: { x: 96 * BLU_PER_DIP, y: 96 * BLU_PER_DIP, width: size * BLU_PER_DIP, height: size * BLU_PER_DIP },
    width: size,
    height: size,
    zoom: 1,
    device_scale: 1,
  };
}

function bytesOf(text) {
  return new TextEncoder().encode(text).buffer;
}

const checks = [];
async function check(name, run) {
  await run();
  checks.push(name);
  console.log(`ok - ${name}`);
}

async function main() {
  let tileHash = "";
  const shell = new Shell();
  try {
    await check("the worker starts with a configuration and reports the engine version", async () => {
      shell.send({ worker: "init", config: { test: { allow_panic: true } } });
      const ready = await shell.status();
      assert.equal(ready.worker, "ready");
      assert.match(ready.engine_version, /^\d+\.\d+\.\d+/);
    });

    await check("handshake → open → render → record → replay gives identical tile hashes", async () => {
      const welcome = await shell.request(1, "hello", { protocol_versions: [0] });
      assert.equal(welcome.type, "welcome");
      assert.equal(ok(welcome).protocol_version, 0);
      ok(await shell.request(2, "diag.record.start"));
      const file = bytesOf("any bytes: protocol v0 opens the mock document");
      const opened = await shell.request(3, "doc.open", { blob: file }, [file]);
      assert.equal(opened.type, "doc.opened");
      const docId = ok(opened).doc_id;
      const tile = ok(await shell.request(4, "render.tile", tileRequest(docId)));
      assert.equal(tile.width, 64);
      assert.equal(tile.height, 64);
      assert.equal(tile.blob, undefined, "the web delivers a bitmap, not a blob");
      assert.equal(tile.stride, undefined);
      assert.equal(tile.bitmap.width, 64);
      // Every pixel is opaque, so the bitmap's pixels equal the engine's premultiplied pixels and their digest.
      const pixels = tile.bitmap.data;
      assert.ok(pixels.every((value, index) => index % 4 !== 3 || value === 255));
      assert.equal(fnv1a64(pixels), tile.hash);
      tileHash = tile.hash;
      const stopped = ok(await shell.request(5, "diag.record.stop"));
      assert.ok(stopped.blob instanceof ArrayBuffer, "the recording arrives as an ArrayBuffer");
      assert.equal(stopped.truncated, false);
      // doc.open and render.tile; the recording starts after diag.record.start and ends before diag.record.stop.
      assert.equal(stopped.entries, 2);
      const report = ok(await shell.request(6, "diag.replay", { blob: stopped.blob }, [stopped.blob]));
      assert.equal(report.identical, true, JSON.stringify(report));
      assert.equal(report.first_difference, null);
      assert.deepEqual(
        report.tiles.map((replayed) => replayed.hash),
        [tileHash],
      );
    });

    await check("a forced panic produces engine.error, and a new engine renders the same tile", async () => {
      const error = errorOf(await shell.request(7, "diag.panic"));
      assert.equal(error.code, "panic");
      assert.equal(error.message_id, "engine-error-panic");
      const event = await shell.event("engine.error");
      assert.equal(event.payload.code, "panic");
      assert.equal(event.payload.recoverable, false);
      // The replacement instance numbers its events from 1 again (§4).
      assert.equal(event.seq, 1);
      // The session is gone; a new handshake starts a working one.
      assert.equal(errorOf(await shell.request(8, "query.a11y", { doc_id: 1 })).code, "handshake_required");
      ok(await shell.request(9, "hello", { protocol_versions: [0] }));
      const file = bytesOf("again");
      const docId = ok(await shell.request(10, "doc.open", { blob: file }, [file])).doc_id;
      assert.equal(ok(await shell.request(11, "render.tile", tileRequest(docId))).hash, tileHash);
    });

    await check("a blob over the limit is refused with limit_exceeded", async () => {
      const huge = new ArrayBuffer(64 * 1024 * 1024 + 1);
      const error = errorOf(await shell.request(12, "doc.open", { blob: huge }, [huge]));
      assert.equal(error.code, "limit_exceeded");
      assert.equal(error.args.limit, "blob");
    });

    await check("a tile whose bitmap the browser cannot create gets the error internal", async () => {
      const error = errorOf(await shell.request(13, "render.tile", tileRequest(1, FAILING_BITMAP_WIDTH)));
      assert.equal(error.code, "internal");
      // The engine is unaffected.
      assert.equal(ok(await shell.request(14, "render.tile", tileRequest(1))).hash, tileHash);
    });

    await check("messages JSON cannot hold are refused as invalid_message", async () => {
      shell.send({ v: 0, id: 15, type: "hello", payload: { protocol_versions: [0n] } });
      const event = await shell.event("engine.error");
      assert.equal(event.payload.code, "invalid_message");
      assert.equal(event.payload.recoverable, true);
      shell.send("not an envelope");
      assert.equal((await shell.event("engine.error")).payload.code, "invalid_message");
    });

    await check("later control messages are ignored, and answers keep their order", async () => {
      shell.send({ worker: "init", config: { test: { allow_panic: false } } });
      shell.send({ worker: "something else" });
      for (let id = 20; id < 30; id += 1) {
        shell.send({ v: 0, id, type: "query.a11y", payload: { doc_id: 1 } });
      }
      for (let id = 20; id < 30; id += 1) {
        const next = await shell.take(() => true, "any message");
        assert.equal(next.re, id);
      }
      // Still the configuration from the first init: panics are allowed.
      assert.equal(errorOf(await shell.request(30, "diag.panic")).code, "panic");
      await shell.event("engine.error");
    });
  } finally {
    await shell.close();
  }

  await check("without init, the worker starts with the defaults on the first message", async () => {
    const plain = new Shell();
    try {
      plain.send({ v: 0, id: 1, type: "hello", payload: { protocol_versions: [0] } });
      assert.equal((await plain.take(() => true, "any message")).worker, "ready");
      ok(await plain.reply(1));
      // The default configuration refuses diag.panic.
      assert.equal(errorOf(await plain.request(2, "diag.panic")).code, "not_allowed");
    } finally {
      await plain.close();
    }
  });

  await check("an invalid configuration or a missing engine fails with a fixed reason", async () => {
    for (const [init, reason] of [
      [{ worker: "init", config: { test: { allow_panics: true } } }, "invalid_config"],
      [{ worker: "init", wasm_url: "missing.wasm" }, "load_failed"],
    ]) {
      const failing = new Shell();
      try {
        failing.send(init);
        const status = await failing.status();
        assert.deepEqual(status, { worker: "failed", reason });
        // After failing, the worker ignores protocol messages.
        failing.send({ v: 0, id: 1, type: "hello", payload: { protocol_versions: [0] } });
        await new Promise((resolve) => setTimeout(resolve, 200));
        assert.deepEqual(failing.inbox, []);
      } finally {
        await failing.close();
      }
    }
  });

  console.log(`${checks.length} checks passed`);
  console.log(`tile ${tileHash}`);
}

main().catch((error) => {
  console.error(`not ok - ${error?.stack ?? error}`);
  process.exitCode = 1;
});
