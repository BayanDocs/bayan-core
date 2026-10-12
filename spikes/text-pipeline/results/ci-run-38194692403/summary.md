### CORE-003 measurements

Each round shapes, or shapes and lays out, the 2951 characters of the 30 probes; the viewport is 3840 × 2160 pixels (1920 × 1080 at device scale 2), filled with the probes. Times are medians of five renderings. GitHub's runners are shared virtual machines, so compare environments only within one run.

| Environment | Shaping (characters/s) | Shaping and layout (characters/s) | Reference (ms) | Gamma-2 (ms) | vello_cpu u8, detected SIMD (ms) | vello_cpu u8, fallback (ms) | vello_cpu f32, fallback (ms) |
|---|--:|--:|--:|--:|--:|--:|--:|
| `linux-x86_64` | 2,656,964 | 1,434,407 | 52 | 79 | 32 | 40 | 61 |
| `macos-arm64` | 3,709,600 | 1,669,566 | 39 | 46 | 13 | 17 | 24 |
| `wasm32-chromium-linux-x86_64` | 2,295,534 | 1,212,073 | 194 | 230 | 173 | 173 | 231 |
| `wasm32-node-linux-x86_64` | 2,209,577 | 1,212,963 | 198 | 237 | 173 | 182 | 238 |
| `wasm32-node-macos-arm64` | 2,345,919 | 1,273,604 | 223 | 248 | 170 | 169 | 201 |
| `wasm32-node-windows-x86_64` | 1,659,561 | 860,434 | 290 | 299 | 217 | 217 | 293 |
| `wasm32-simd128-chromium-linux-x86_64` | 2,254,339 | 1,191,311 | 144 | 182 | 105 | 120 | 185 |
| `wasm32-simd128-node-linux-x86_64` | 2,265,153 | 1,209,203 | 147 | 188 | 106 | 122 | 187 |
| `wasm32-simd128-node-macos-arm64` | 2,401,847 | 1,277,515 | 177 | 204 | 131 | 136 | 171 |
| `wasm32-simd128-node-windows-x86_64` | 1,705,038 | 929,408 | 186 | 227 | 128 | 146 | 225 |
| `windows-x86_64` | 2,421,639 | 1,256,205 | 56 | 87 | 29 | 36 | 57 |

Peak memory in MiB after each phase: the process's peak resident memory on Linux (VmHWM; not measured on Windows and macOS), and the size of the module's linear memory in WebAssembly, which only grows.

| Environment | Fonts loaded | Shaping and layout | Reference raster | Gamma-2 raster | vello_cpu (all three) |
|---|--:|--:|--:|--:|--:|
| `linux-x86_64` | 17.2 | 22.1 | 53.9 | 53.9 | 89.8 |
| `macos-arm64` | – | – | – | – | – |
| `wasm32-chromium-linux-x86_64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-node-linux-x86_64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-node-macos-arm64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-node-windows-x86_64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-simd128-chromium-linux-x86_64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-simd128-node-linux-x86_64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-simd128-node-macos-arm64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `wasm32-simd128-node-windows-x86_64` | 17.2 | 20.3 | 51.9 | 51.9 | 115.3 |
| `windows-x86_64` | – | – | – | – | – |

### CORE-003 WebAssembly size

The module as shipped, without its custom sections (names and tool metadata), compressed with gzip -9 and brotli -11. The parts are compressed one by one, so they add up to a little more than the whole.

| Part | default: raw | default: brotli | simd128: raw | simd128: brotli |
|---|--:|--:|--:|--:|
| **whole module** (gzip: default 1,748,828, simd128 1,723,574) | 4,433,786 | 1,348,964 | 4,392,755 | 1,342,227 |
| vello_cpu and its crates (kurbo, peniko, fearless_simd, …) | 436,537 | 105,605 | 515,191 | 111,488 |
| tiny-skia (reference rasterizer) | 294,392 | 60,278 | 173,572 | 44,435 |
| harfrust (shaping) | 548,377 | 128,991 | 546,432 | 129,619 |
| read-fonts and skrifa (font parsing, outlines) | 673,237 | 135,684 | 684,522 | 139,657 |
| ICU4X (segmentation, properties, collections) | 49,394 | 16,658 | 49,495 | 16,794 |
| unicode-bidi | 12,652 | 5,178 | 12,652 | 5,185 |
| the spike, bayan-units and serde_json | 86,643 | 26,696 | 86,526 | 27,054 |
| DEFLATE (miniz_oxide) and SHA-256 (sha2) | 28,805 | 9,495 | 22,133 | 8,439 |
| Rust standard library (core, alloc, std), wasm-bindgen and small helpers | 113,761 | 32,176 | 113,735 | 32,415 |
| unnamed or other | 18,683 | 7,166 | 18,746 | 7,248 |
| data segments (ICU4X's compiled data, tables, strings) | 2,159,689 | 823,322 | 2,157,977 | 822,357 |

### CORE-003: the same layout and reference raster everywhere

Environments: `linux-x86_64`, `macos-arm64`, `wasm32-chromium-linux-x86_64`, `wasm32-node-linux-x86_64`, `wasm32-node-macos-arm64`, `wasm32-node-windows-x86_64`, `wasm32-simd128-chromium-linux-x86_64`, `wasm32-simd128-node-linux-x86_64`, `wasm32-simd128-node-macos-arm64`, `wasm32-simd128-node-windows-x86_64`, `windows-x86_64`.

| Probe | Layout (AC-1) | Reference raster (AC-2) | Gamma-2 raster | vello_cpu u8, detected SIMD | vello_cpu u8, fallback | vello_cpu f32, fallback | vello_cpu vs reference: max difference, pixels beyond ±2 |
|---|---|---|---|---|---|---|---|
| latin-sans-kerning | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 63, 9.04 % |
| latin-sans-no-kerning | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 64, 9.05 % |
| latin-serif-kerning | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 65, 9.31 % |
| latin-serif-no-kerning | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 66, 9.25 % |
| latin-carlito | same everywhere | same everywhere | same everywhere | linux-x86_64 + macos-arm64 + windows-x86_64 / wasm32-chromium-linux-x86_64 + wasm32-node-linux-x86_64 + wasm32-node-macos-arm64 + wasm32-node-windows-x86_64 + wasm32-simd128-chromium-linux-x86_64 + wasm32-simd128-node-linux-x86_64 + wasm32-simd128-node-macos-arm64 + wasm32-simd128-node-windows-x86_64 | same everywhere | same everywhere | 62, 8.77 % |
| latin-carlito-ligatures | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 61, 7.54 % |
| latin-carlito-plain | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 56, 7.48 % |
| latin-punctuation-numbers | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 63, 10.54 % |
| latin-diacritics | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 68, 9.17 % |
| latin-long-word | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 66, 5.51 % |
| latin-break-classes | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 71, 9.60 % |
| arabic-marks | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 64, 3.53 % |
| arabic-joining | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 73, 3.59 % |
| arabic-numbers-latin | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 73, 3.93 % |
| arabic-lam-alef | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 62, 3.79 % |
| hebrew-numbers-latin | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 70, 6.77 % |
| hebrew-points | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 67, 5.74 % |
| hebrew-brackets | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 67, 6.77 % |
| devanagari-conjuncts | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 79, 6.48 % |
| devanagari-reph | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 65, 5.93 % |
| devanagari-numbers | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 75, 7.16 % |
| thai-dictionary | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 72, 6.77 % |
| thai-marks | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 73, 6.39 % |
| thai-mixed | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 68, 6.66 % |
| chinese-punctuation | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 74, 12.43 % |
| chinese-mixed | same everywhere | same everywhere | same everywhere | linux-x86_64 + macos-arm64 + windows-x86_64 / wasm32-chromium-linux-x86_64 + wasm32-node-linux-x86_64 + wasm32-node-macos-arm64 + wasm32-node-windows-x86_64 + wasm32-simd128-chromium-linux-x86_64 + wasm32-simd128-node-linux-x86_64 + wasm32-simd128-node-macos-arm64 + wasm32-simd128-node-windows-x86_64 | same everywhere | same everywhere | 74, 10.84 % |
| japanese-kana-kanji | same everywhere | same everywhere | same everywhere | linux-x86_64 + macos-arm64 + windows-x86_64 / wasm32-chromium-linux-x86_64 + wasm32-node-linux-x86_64 + wasm32-node-macos-arm64 + wasm32-node-windows-x86_64 + wasm32-simd128-chromium-linux-x86_64 + wasm32-simd128-node-linux-x86_64 + wasm32-simd128-node-macos-arm64 + wasm32-simd128-node-windows-x86_64 | same everywhere | same everywhere | 72, 11.10 % |
| japanese-katakana | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 72, 6.69 % |
| mixed-ltr-with-rtl | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 62, 7.61 % |
| mixed-rtl-with-ltr | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | same everywhere | 74, 4.74 % |
| viewport (3840 × 2160) | same everywhere | same everywhere | same everywhere | linux-x86_64 + macos-arm64 + windows-x86_64 / wasm32-chromium-linux-x86_64 + wasm32-node-linux-x86_64 + wasm32-node-macos-arm64 + wasm32-node-windows-x86_64 + wasm32-simd128-chromium-linux-x86_64 + wasm32-simd128-node-linux-x86_64 + wasm32-simd128-node-macos-arm64 + wasm32-simd128-node-windows-x86_64 | same everywhere | same everywhere | |

SIMD levels vello_cpu detected: `linux-x86_64` Avx2, `macos-arm64` Neon, `wasm32-chromium-linux-x86_64` Fallback, `wasm32-node-linux-x86_64` Fallback, `wasm32-node-macos-arm64` Fallback, `wasm32-node-windows-x86_64` Fallback, `wasm32-simd128-chromium-linux-x86_64` WasmSimd128, `wasm32-simd128-node-linux-x86_64` WasmSimd128, `wasm32-simd128-node-macos-arm64` WasmSimd128, `wasm32-simd128-node-windows-x86_64` WasmSimd128, `windows-x86_64` Avx2.

Every layout hash and every reference raster hash is the same in all 11 environments, and equal to spikes/text-pipeline/results/expected.json.
