// Runs the CORE-003 spike's WebAssembly module in headless Chromium (Chrome for Testing's headless shell), in a module
// Web Worker as the web app runs the engine, and writes the same files as the native runner. Plain JavaScript without
// dependencies, for Node.js 24: a small HTTP server on 127.0.0.1 serves the page, the module and the fonts, receives
// the files the worker sends back, and ends the browser when the worker reports.
//
// Usage: node run-chromium.mjs <chrome-headless-shell> <package folder> <fonts folder> <output folder> <environment>
//
// The browser runs with --no-sandbox: its sandbox needs user namespaces, which Ubuntu 24.04 restricts (and which the
// root user of a container lacks), and the only page it opens is this spike's own, from 127.0.0.1.

import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const [chrome, packageFolder, fontsFolder, outputFolder, environment] = process.argv.slice(2);
if (!environment) {
  console.error(
    "usage: node run-chromium.mjs <chrome-headless-shell> <package folder> <fonts folder> <output folder> <environment>",
  );
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const plainName = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
// The only files the server serves: everything the page and the worker load, and the font files (fonts/fetch.sh
// checked each against its SHA-256, and the module checks them again). A request names a file only as a key of this map.
const files = new Map([
  ["/page.html", join(here, "page.html")],
  ["/page.mjs", join(here, "page.mjs")],
  ["/worker.mjs", join(here, "worker.mjs")],
  ["/harness.mjs", join(here, "harness.mjs")],
  ["/pkg/text_pipeline.js", join(packageFolder, "text_pipeline.js")],
  ["/pkg/text_pipeline_bg.wasm", join(packageFolder, "text_pipeline_bg.wasm")],
  ...readdirSync(fontsFolder)
    .filter((name) => plainName.test(name) && /\.(?:ttf|otf)$/.test(name))
    .map((name) => [`/fonts/${name}`, join(fontsFolder, name)]),
]);
const types = { ".html": "text/html; charset=utf-8", ".mjs": "text/javascript", ".js": "text/javascript", ".wasm": "application/wasm" };
// Where the worker may write: results.json, bench.json, layout/<probe>.json and png/<probe>-<rendering>.png.
const outputPath = /^(?:results\.json|bench\.json|layout\/[a-z0-9-]+\.json|png\/[a-z0-9_-]+\.png)$/;
const outputRoot = resolve(outputFolder);
const maxUpload = 128 * 1024 * 1024;

/** The file in the output folder that the worker's `name` stands for, or null if it may not write there. */
function outputFile(name) {
  if (!outputPath.test(name)) {
    return null;
  }
  const target = resolve(outputRoot, name);
  return target.startsWith(outputRoot + sep) ? target : null;
}

let finish;
const finished = new Promise((resolve) => {
  finish = resolve;
});

function readBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    request.on("data", (chunk) => {
      size += chunk.length;
      if (size > maxUpload) {
        reject(new Error("upload too large"));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => resolve(Buffer.concat(chunks)));
    request.on("error", reject);
  });
}

const server = createServer(async (request, response) => {
  const path = new URL(request.url, "http://127.0.0.1").pathname;
  const target = request.method === "PUT" && path.startsWith("/out/") ? outputFile(path.slice(5)) : null;
  try {
    if (request.method === "GET" && files.has(path)) {
      const body = readFileSync(files.get(path));
      const extension = path.slice(path.lastIndexOf("."));
      response.writeHead(200, { "content-type": types[extension] ?? "application/octet-stream" });
      response.end(body);
    } else if (target) {
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, await readBody(request));
      response.writeHead(204).end();
    } else if (request.method === "POST" && path === "/done") {
      const outcome = JSON.parse((await readBody(request)).toString("utf8"));
      response.writeHead(204).end();
      finish(outcome);
    } else {
      response.writeHead(404).end();
    }
  } catch (error) {
    if (!response.headersSent) {
      response.writeHead(500);
    }
    response.end();
    finish({ ok: false, message: `the server failed on ${request.method} ${path}: ${error.message}` });
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const url = `http://127.0.0.1:${server.address().port}/page.html?environment=${encodeURIComponent(environment)}`;

const profile = mkdtempSync(join(tmpdir(), "text-pipeline-chromium-"));
const browser = spawn(
  chrome,
  [
    "--no-sandbox",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    `--user-data-dir=${profile}`,
    url,
  ],
  { stdio: ["ignore", "pipe", "pipe"] },
);
// The browser's own messages (about the missing D-Bus or GPU of a CI machine) are shown only if the run fails: the last
// 64 KiB of them.
let browserOutput = "";
const keep = (chunk) => {
  browserOutput = (browserOutput + chunk.toString("utf8")).slice(-64 * 1024);
};
browser.stdout.on("data", keep);
browser.stderr.on("data", keep);
browser.on("exit", (code, signal) => finish({ ok: false, message: `the browser exited early (${signal ?? code})` }));
// `close` comes once the browser has exited and every process it started that shares its output pipes has exited too.
const closed = new Promise((resolve) => browser.once("close", resolve));
console.log(`${environment}: ${chrome} is running the spike from ${url}`);

/** Ends the browser (SIGTERM, then SIGKILL after five seconds) and waits, at most ten seconds, until its processes are gone: they write into the profile folder until they exit. */
async function stopBrowser() {
  if (browser.exitCode === null && browser.signalCode === null) {
    browser.kill("SIGTERM");
  }
  const force = setTimeout(() => browser.kill("SIGKILL"), 5000);
  let giveUp;
  await Promise.race([closed, new Promise((resolve) => (giveUp = setTimeout(resolve, 10000)))]);
  clearTimeout(force);
  clearTimeout(giveUp);
}

// A run takes well under a minute; two runs must fit in the CI job's 30 minutes with their downloads.
const minutes = 10;
const timer = setTimeout(() => finish({ ok: false, message: `no result after ${minutes} minutes` }), minutes * 60 * 1000);
const outcome = await finished;
clearTimeout(timer);
browser.removeAllListeners("exit");
await stopBrowser();
server.close();
try {
  rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
} catch (error) {
  // A browser profile left in the temporary folder does not change the run's results.
  console.error(`${environment}: could not delete the browser profile ${profile}: ${error.message}`);
}
if (outcome.ok) {
  console.log(`${environment}: layouts ${outcome.layouts}  references ${outcome.references}`);
} else {
  console.error(`${environment}: ${outcome.message}`);
  console.error(`The browser's messages:\n${browserOutput}`);
  process.exit(1);
}
