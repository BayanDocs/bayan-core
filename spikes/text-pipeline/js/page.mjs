// The page that run-chromium.mjs opens in headless Chromium: it starts the spike in a module Web Worker, where the web
// app will run the engine (ADR-0014), and reports how the run ended to the server that served the page.

const report = (outcome) =>
  fetch("done", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(outcome) });

const worker = new Worker("worker.mjs", { type: "module" });
worker.onmessage = (event) => report(event.data);
worker.onerror = (event) => report({ ok: false, message: `the worker failed: ${event.message}` });
worker.postMessage({ environment: new URLSearchParams(location.search).get("environment") });
