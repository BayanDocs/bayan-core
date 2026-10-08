// The script of the engine test page (test-page.html): a minimal web shell that starts bayan-worker.js, shows the mock
// document's pages as tiles, forwards typing, composition, clicks and keys, and can record, replay and force a panic.
// It uses safe DOM APIs only (textContent, no HTML from strings) and no dependencies.
//
// SPDX-FileCopyrightText: 2026 BayanDocs contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission

/** 1 device-independent pixel is 19,050 BLU at zoom 1 (engine protocol §7). */
const BLU_PER_DIP = 19_050;
const MAX_TILE_SIDE = 4096;
const MAX_LOG_LINES = 200;

const element = (id) => document.getElementById(id);
const canvas = element("canvas");
const context = canvas.getContext("2d");
const caretBox = element("caret");
const typing = element("typing");
const status = element("status");
const buttons = { record: element("record"), replay: element("replay"), panic: element("panic") };

let worker = null;
let nextId = 1;
let docId = null;
let pages = [];
let scale = 1;
const waiting = new Map();

function log(text) {
  const line = document.createElement("li");
  line.textContent = text;
  const list = element("log");
  list.prepend(line);
  while (list.childElementCount > MAX_LOG_LINES) {
    list.lastElementChild.remove();
  }
}

function setStatus(text) {
  status.textContent = text;
}

function enable(on) {
  for (const button of Object.values(buttons)) {
    button.disabled = !on;
  }
  typing.disabled = !on;
}

/** Sends a request and resolves with its reply. */
function request(type, payload, transfer = []) {
  const id = nextId;
  nextId += 1;
  worker.postMessage({ v: 0, id, type, payload }, transfer);
  return new Promise((resolve) => waiting.set(id, resolve));
}

function start() {
  worker?.terminate();
  for (const resolve of waiting.values()) {
    resolve({ ok: false, error: { code: "worker_replaced" } });
  }
  waiting.clear();
  enable(false);
  setStatus("Starting…");
  worker = new Worker(new URL("bayan-worker.js", import.meta.url), { type: "module" });
  worker.addEventListener("message", (event) => receive(event.data));
  worker.postMessage({ worker: "init", config: { test: { allow_panic: element("allow-panic").checked } } });
}

function receive(message) {
  if ("worker" in message) {
    if (message.worker === "ready") {
      setStatus(`Engine ${message.engine_version} ready.`);
      begin();
    } else {
      setStatus(`The worker failed: ${message.reason}.`);
    }
    return;
  }
  if (message.re !== undefined) {
    log(`reply to ${message.re}: ${message.ok ? "ok" : message.error.code}`);
    const resolve = waiting.get(message.re);
    waiting.delete(message.re);
    resolve?.(message);
    return;
  }
  log(`event ${message.seq}: ${message.type}`);
  const payload = message.payload;
  switch (message.type) {
    case "view.pages":
      pages = payload.pages;
      layOut();
      break;
    case "render.invalidate":
      for (const index of new Set(payload.regions.map((region) => region.page))) {
        renderPage(index);
      }
      break;
    case "overlay.update":
      showCaret(payload.caret);
      break;
    case "engine.error":
      if (!payload.recoverable) {
        setStatus(`The engine reported ${payload.code} and started a new session.`);
        begin();
      }
      break;
    default:
      break;
  }
}

/** Handshake, open the mock document from an empty file, and set the view. */
async function begin() {
  docId = null;
  enable(false);
  const welcome = await request("hello", { protocol_versions: [0], locale: navigator.language });
  if (!welcome.ok) {
    return;
  }
  const file = new ArrayBuffer(0);
  const opened = await request("doc.open", { blob: file }, [file]);
  if (!opened.ok) {
    return;
  }
  docId = opened.payload.doc_id;
  scale = Math.min(window.devicePixelRatio || 1, 4);
  const box = element("pages");
  await request("view.set", {
    doc_id: docId,
    viewport: { x: 0, y: 0, width: box.clientWidth || 800, height: box.clientHeight || 600 },
    zoom: 1,
    device_scale: scale,
    mode: "print",
  });
  enable(true);
}

/** Sizes the canvas to the page area: the pages' positions are in view space, BLU at zoom 1 (§7). */
function layOut() {
  const width = Math.max(...pages.map((page) => page.x + page.width)) / BLU_PER_DIP;
  const height = Math.max(...pages.map((page) => page.y + page.height)) / BLU_PER_DIP;
  canvas.width = Math.ceil(width * scale);
  canvas.height = Math.ceil(height * scale);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
}

/** Renders a whole page as one tile and draws its ImageBitmap. */
async function renderPage(index) {
  const page = pages[index];
  if (page === undefined || docId === null) {
    return;
  }
  const width = Math.min(MAX_TILE_SIDE, Math.max(1, Math.round((page.width / BLU_PER_DIP) * scale)));
  const height = Math.min(MAX_TILE_SIDE, Math.max(1, Math.round((page.height / BLU_PER_DIP) * scale)));
  const reply = await request("render.tile", {
    doc_id: docId,
    page: index,
    rect: { x: 0, y: 0, width: page.width, height: page.height },
    width,
    height,
    zoom: 1,
    device_scale: scale,
    mode: "interactive",
  });
  if (!reply.ok) {
    return;
  }
  const bitmap = reply.payload.bitmap;
  const left = Math.round((page.x / BLU_PER_DIP) * scale);
  const top = Math.round((page.y / BLU_PER_DIP) * scale);
  context.clearRect(left, top, width, height);
  context.drawImage(bitmap, left, top, width, height);
  bitmap.close();
}

function showCaret(caret) {
  const page = caret ? pages[caret.page] : undefined;
  if (!caret || !caret.visible || page === undefined) {
    caretBox.hidden = true;
    return;
  }
  caretBox.hidden = false;
  caretBox.style.left = `${(page.x + caret.rect.x) / BLU_PER_DIP}px`;
  caretBox.style.top = `${(page.y + caret.rect.y) / BLU_PER_DIP}px`;
  caretBox.style.width = `${Math.max(1, caret.rect.width / BLU_PER_DIP)}px`;
  caretBox.style.height = `${caret.rect.height / BLU_PER_DIP}px`;
}

function modifiers(event) {
  return { shift: event.shiftKey, ctrl: event.ctrlKey, alt: event.altKey, meta: event.metaKey };
}

// Input: the field forwards what is typed and composed, then empties itself; the document shows the result.
const EDITING_KEYS = new Set(["Backspace", "Delete", "ArrowLeft", "ArrowRight", "Home", "End"]);

typing.addEventListener("keydown", (event) => {
  if (docId !== null && !event.isComposing && EDITING_KEYS.has(event.key)) {
    event.preventDefault();
    request("input.key", { doc_id: docId, key: event.key, code: event.code, modifiers: modifiers(event), repeat: event.repeat });
  }
});
typing.addEventListener("input", (event) => {
  if (docId !== null && !event.isComposing && event.inputType === "insertText" && event.data) {
    request("input.text", { doc_id: docId, text: event.data });
    typing.value = "";
  }
});
typing.addEventListener("compositionstart", () => {
  if (docId !== null) {
    request("input.composition", { doc_id: docId, phase: "start" });
  }
});
typing.addEventListener("compositionupdate", (event) => {
  if (docId !== null) {
    request("input.composition", { doc_id: docId, phase: "update", text: event.data });
  }
});
typing.addEventListener("compositionend", (event) => {
  if (docId !== null) {
    request("input.composition", { doc_id: docId, phase: "end" });
    if (event.data) {
      request("input.text", { doc_id: docId, text: event.data });
    }
    typing.value = "";
  }
});

canvas.addEventListener("pointerdown", (event) => {
  if (docId !== null) {
    request("input.pointer", {
      doc_id: docId,
      kind: "down",
      x: event.offsetX,
      y: event.offsetY,
      button: event.button,
      clicks: event.detail,
      modifiers: modifiers(event),
      pointer_type: ["mouse", "pen", "touch"].includes(event.pointerType) ? event.pointerType : undefined,
    });
  }
});

element("start").addEventListener("click", start);
buttons.record.addEventListener("click", async () => {
  const reply = await request("diag.record.start");
  setStatus(reply.ok ? "Recording." : `Recording refused: ${reply.error.code}.`);
});
buttons.replay.addEventListener("click", async () => {
  const stopped = await request("diag.record.stop");
  if (!stopped.ok) {
    setStatus(`No recording: ${stopped.error.code}.`);
    return;
  }
  const recording = stopped.payload.blob;
  const replayed = await request("diag.replay", { blob: recording }, [recording]);
  if (!replayed.ok) {
    setStatus(`The replay failed: ${replayed.error.code}.`);
    return;
  }
  const report = replayed.payload;
  setStatus(
    `Replayed ${report.entries} entries: ${report.identical ? "identical" : `first difference at entry ${report.first_difference}`}, ${report.tiles.length} tiles.`,
  );
});
buttons.panic.addEventListener("click", () => request("diag.panic"));
