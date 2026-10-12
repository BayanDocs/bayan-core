// Measures the size of the CORE-003 spike's WebAssembly module (AC-3, PERF-06): raw and compressed with gzip and
// brotli (both from Node.js's own zlib), as shipped (without custom sections, which hold only names and tool metadata),
// and how much of it each library's code takes, from the function names in the module's name section. Plain
// JavaScript without dependencies, for Node.js 24.
//
// Usage: node wasm-size.mjs <module.wasm>   (prints JSON)

import { readFileSync } from "node:fs";
import { basename } from "node:path";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

const [file] = process.argv.slice(2);
if (!file) {
  console.error("usage: node wasm-size.mjs <module.wasm>");
  process.exit(2);
}
const bytes = readFileSync(file);

/** Reads an unsigned LEB128 number at `offset` and returns it with the offset after it. */
function leb(offset) {
  let value = 0;
  let shift = 0;
  for (;;) {
    const byte = bytes[offset];
    offset += 1;
    value += (byte & 0x7f) * 2 ** shift;
    shift += 7;
    if ((byte & 0x80) === 0) {
      return [value, offset];
    }
  }
}

if (bytes.readUInt32LE(0) !== 0x6d736100 || bytes.readUInt32LE(4) !== 1) {
  console.error(`${file} is not a WebAssembly module`);
  process.exit(1);
}
const sections = [];
let offset = 8;
while (offset < bytes.length) {
  const id = bytes[offset];
  const [size, start] = leb(offset + 1);
  let name = null;
  if (id === 0) {
    const [length, nameStart] = leb(start);
    name = bytes.subarray(nameStart, nameStart + length).toString("utf8");
  }
  sections.push({ id, name, start, end: start + size, whole: bytes.subarray(offset, start + size) });
  offset = start + size;
}

// The module as it would ship: every section but the custom ones.
const shipped = Buffer.concat([bytes.subarray(0, 8), ...sections.filter((s) => s.id !== 0).map((s) => s.whole)]);
const gzip = (data) => gzipSync(data, { level: 9 }).length;
const brotli = (data) =>
  brotliCompressSync(data, {
    params: { [constants.BROTLI_PARAM_QUALITY]: 11, [constants.BROTLI_PARAM_SIZE_HINT]: data.length },
  }).length;

// Function bodies, numbered after the imported functions.
let importedFunctions = 0;
const importSection = sections.find((s) => s.id === 2);
if (importSection) {
  let [count, cursor] = leb(importSection.start);
  for (let i = 0; i < count; i += 1) {
    let length;
    [length, cursor] = leb(cursor);
    cursor += length;
    [length, cursor] = leb(cursor);
    cursor += length;
    const kind = bytes[cursor];
    cursor += 1;
    if (kind === 0) {
      importedFunctions += 1;
      [, cursor] = leb(cursor);
    } else if (kind === 1) {
      cursor += 1;
      const flags = bytes[cursor];
      cursor += 1;
      [, cursor] = leb(cursor);
      if (flags & 1) [, cursor] = leb(cursor);
    } else if (kind === 2) {
      const flags = bytes[cursor];
      cursor += 1;
      [, cursor] = leb(cursor);
      if (flags & 1) [, cursor] = leb(cursor);
    } else if (kind === 3) {
      cursor += 2;
    } else {
      [, cursor] = leb(cursor + 1);
    }
  }
}
const bodies = [];
const code = sections.find((s) => s.id === 10);
let [functionCount, cursor] = leb(code.start);
for (let i = 0; i < functionCount; i += 1) {
  const [size, bodyStart] = leb(cursor);
  bodies.push(bytes.subarray(bodyStart, bodyStart + size));
  cursor = bodyStart + size;
}
const names = new Map();
const nameSection = sections.find((s) => s.id === 0 && s.name === "name");
if (nameSection) {
  let [length, position] = leb(nameSection.start);
  position += length;
  while (position < nameSection.end) {
    const kind = bytes[position];
    const [size, start] = leb(position + 1);
    if (kind === 1) {
      let [count, entry] = leb(start);
      for (let i = 0; i < count; i += 1) {
        let index;
        [index, entry] = leb(entry);
        let nameLength;
        [nameLength, entry] = leb(entry);
        names.set(index, bytes.subarray(entry, entry + nameLength).toString("utf8"));
        entry += nameLength;
      }
    }
    position = start + size;
  }
}

// Libraries, by the crates their code comes from. rustc's names carry a hash after each crate name, as in
// `harfrust[bb612835d1f96986]::shape`; a generic function counts for the first library its name mentions, in this order.
const crates = (...list) => new RegExp(`(^|[^A-Za-z0-9_])(${list.join("|")})(\\[[0-9a-f]+\\])?::`);
const libraries = [
  ["vello_cpu and its crates (kurbo, peniko, fearless_simd, …)", crates("vello_cpu", "vello_common", "fearless_simd", "kurbo", "peniko", "color", "polycool", "guillotiere", "euclid", "hashbrown", "foldhash")],
  ["tiny-skia (reference rasterizer)", crates("tiny_skia", "tiny_skia_path", "strict_num", "arrayref")],
  ["harfrust (shaping)", crates("harfrust")],
  ["read-fonts and skrifa (font parsing, outlines)", crates("read_fonts", "skrifa", "font_types")],
  ["ICU4X (segmentation, properties, collections)", crates("icu_[a-z_]+", "zerovec", "zerotrie", "yoke", "zerofrom", "potential_utf", "tinystr", "writeable", "litemap", "utf8_iter")],
  ["unicode-bidi", crates("unicode_bidi")],
  ["the spike, bayan-units and serde_json", crates("text_pipeline", "bayan_units", "serde_json", "serde_core", "serde")],
  ["DEFLATE (miniz_oxide) and SHA-256 (sha2)", crates("miniz_oxide", "adler2", "sha2", "digest", "block_buffer", "crypto_common")],
  ["Rust standard library (core, alloc, std), wasm-bindgen and small helpers", crates("core", "alloc", "std", "wasm_bindgen", "dlmalloc", "compiler_builtins", "libm", "core_maths", "spin", "smallvec", "arrayvec", "bytemuck", "once_cell")],
];
const groups = libraries.map(([library]) => ({ library, functions: 0, bytes: 0, parts: [] }));
const other = { library: "unnamed or other", functions: 0, bytes: 0, parts: [] };
for (const [index, body] of bodies.entries()) {
  const name = names.get(importedFunctions + index) ?? "";
  const position = libraries.findIndex(([, pattern]) => pattern.test(name));
  const group = position >= 0 ? groups[position] : other;
  group.functions += 1;
  group.bytes += body.length;
  group.parts.push(body);
}
const data = sections.filter((s) => s.id === 11).map((s) => bytes.subarray(s.start, s.end));
const summary = [...groups, other].map((group) => {
  const joined = Buffer.concat(group.parts);
  return { library: group.library, functions: group.functions, bytes: group.bytes, gzip: gzip(joined), brotli: brotli(joined) };
});
const dataBytes = Buffer.concat(data);
summary.push({ library: "data segments (ICU4X's compiled data, tables, strings)", functions: 0, bytes: dataBytes.length, gzip: gzip(dataBytes), brotli: brotli(dataBytes) });

console.log(
  JSON.stringify(
    {
      format: "bayan-core CORE-003 text-pipeline WebAssembly size v1",
      module: basename(file),
      raw_with_custom_sections: bytes.length,
      shipped: { raw: shipped.length, gzip: gzip(shipped), brotli: brotli(shipped) },
      custom_sections: sections.filter((s) => s.id === 0).map((s) => ({ name: s.name, bytes: s.end - s.start })),
      named_functions: names.size,
      functions: bodies.length,
      // Each part compressed on its own: the parts add up to a little more than the whole, which compresses across them.
      libraries: summary,
    },
    null,
    2,
  ),
);
