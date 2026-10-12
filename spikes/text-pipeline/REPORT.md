# CORE-003 report: a deterministic text pipeline across platforms

- **Work package:** [CORE-003](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-003-spike-deterministic-text.md), the validation gate of ADR-0009 (the text stack) and ADR-0011 (the rendering pipeline).
- **Date:** 2026-10-12. **harfrust** 0.14.0, **skrifa** 0.48.0 (read-fonts 0.45.0), **ICU4X** 2.3.0, **unicode-bidi** 0.3.18, **tiny-skia** 0.12.0, **vello_cpu** 0.3.0; Rust 1.99.0, Node.js 24.21.0, Chrome for Testing 153.0.8010.12.
- **Results and measurements:** from the first run of `.github/workflows/text-pipeline.yml`, on commit ddd39a5 of the pull request, on GitHub's runners ([run 38194692403](https://github.com/BayanDocs/bayan-core/actions/runs/38194692403), 2026-10-12), unless marked *local* (a cloud machine with an Intel Xeon at 2.8 GHz and 4 virtual cores, Linux, the same day). The run's artifact `text-pipeline-summary` holds every environment's `results.json` and `bench.json`; `results/ci-run-38194692403/` keeps the run's summary and measurements in the repository (`results/README.md`).

**Words used in this report.** *BLU* is the engine's integer length unit, 1/1,828,800 inch (ADR-0005). *Shaping* turns characters into positioned glyphs (the shapes of a font), with ligatures, kerning and the joining of Arabic letters. *Bidi* is the Unicode Bidirectional Algorithm, which orders right-to-left and left-to-right text on a line. A *hash* (here SHA-256) is a short fingerprint of a file: two files with the same hash are, for every practical purpose, the same bytes. A *raster* is an image made of pixels; *rasterizing* draws outlines into one. *Anti-aliasing* gives pixels on the edge of a shape a shade between the shape's colour and the background's, depending on how much of the pixel the shape covers. *SIMD* instructions process several numbers at once; *FMA* (fused multiply-add) computes `a × b + c` with one rounding instead of two, so its result can differ from the separate operations in the last bit.

## 1. Summary and recommendation

**The text stack of ADR-0009 is deterministic.** For the 30 probes of the brief and for a full screen of text, the layout (every glyph's identity and position in BLU, every line box) is bit-identical in eleven environments: natively on Linux x86-64, Windows x86-64 and macOS arm64, and in WebAssembly, built with and without SIMD, in Node.js on the same three systems and in headless Chromium. That meets AC-1 with six environments to spare (the brief asks for five).

**tiny-skia's reference mode is bit-exact** (AC-2) in the same eleven environments, for every probe and for the full screen, once two of its build settings are chosen: without its `simd` feature, so that every machine runs the same portable code, and with `no-std-float`, so that the few mathematical functions it calls come from a pure-Rust library instead of each platform's own (§4). Nothing needs escalating: the portable code is fast enough (below), so ADR-0011's decision stands as written.

**Performance is comfortable** (AC-3, §7). On GitHub's runners shaping runs at 2.4 to 3.7 million characters per second natively and 1.7 to 2.4 million in WebAssembly (shaping and line breaking together: 1.3 to 1.7 and 0.9 to 1.3 million). Drawing a whole 1920 × 1080 screen at device scale 2 (3840 × 2160 pixels) full of text takes the reference rasterizer 39 to 56 ms natively and 144 to 290 ms in WebAssembly, on one core, without the tiling and caching that ADR-0011 requires anyway. The WebAssembly module of the whole spike, two rasterizers included, is 1.35 MB with brotli (PERF-06 allows 5 MB for the whole web app), of which 0.82 MB is data, most of it ICU4X's dictionaries for Thai, Lao, Khmer and Burmese, which ADR-0009 already plans to slice.

**vello_cpu is not recommended for on-screen rendering now** (AC-3, §5). It is faster than tiny-skia (1.7 to 2.9 times natively, the most on Apple silicon, and 1.1 to 1.5 times in WebAssembly), but it differs from the reference by up to 79 of 255 levels per colour channel on 3.5 to 12.4 % of the pixels, where ADR-0011 allows ±2 at anti-aliased edges. The cause is not a bug: tiny-skia measures how much of a pixel a glyph covers by sampling 4 × 4 points in it, vello_cpu computes the exact area, so their edge pixels can differ by about a quarter of the range. No rasterizer with a different anti-aliasing method can meet ±2. vello_cpu's output also depends on whether the processor fuses multiply-adds: natively (AVX2 and NEON fuse, and agree with each other) three probes and the screen differ by one level in one pixel each from the WebAssembly runs, which never fuse. On-screen rendering should use the reference rasterizer, with tiles. One finding matters for later: vello_cpu forced to its portable level was **also bit-identical in all eleven environments**, which makes it a concrete replacement if tiny-skia, in maintenance mode, stops being maintained (ADR-0011, "Revisit when").

**Gamma and blending are fixed** (AC-4, §6): grayscale coverage from tiny-skia's anti-aliasing, black text composited source-over onto white directly in sRGB values (no gamma correction), 8-bit premultiplied RGBA, no hinting, no subpixel rendering, glyph positions not snapped to pixels. Blending with gamma 2 was measured as the alternative and is not recommended for the reference.

**Every dependency is pinned exactly, at least seven days old, on the license allowlist, and justified** (AC-5, §8). They add 50 packages to `Cargo.lock`, all from crates.io, all checked by `cargo xtask check-lockfile-age` and `cargo deny`.

**What the gate checks from now on.** The three probes whose fonts are small enough to commit run in the verification gate's determinism step on every pull request, on the host and in WebAssembly (`tests/determinism.rs`); the whole probe set runs in `.github/workflows/text-pipeline.yml`, in all eleven environments, whenever the spike or anything it depends on changes, and weekly. Both compare with `results/expected.json`, so a dependency update that moves a single glyph or pixel fails and must say why (ADR-0004 §4, layout epochs).

## 2. What was built

| Where | What |
|---|---|
| `fonts/manifest.txt`, `fonts/fetch.sh` | The nine fonts of the brief, each pinned by size and SHA-256 with its source: Liberation Sans and Serif 2.1.5, Carlito, Noto Naskh Arabic, Noto Sans Hebrew, Noto Sans Devanagari, Noto Sans Thai, and Noto Sans SC and JP as the CJK subset (the "SubsetOTF" builds of Noto CJK, 8.3 and 4.5 MB). `fetch.sh` (bash 3.2, curl, tar, sha256sum or shasum) downloads them into `target/text-pipeline/fonts/`, checks every download before reading it and every font again before moving it into place, tries a mirror only when a download fails (never after a checksum mismatch, which may mean tampering), and replaces a font that is there with another checksum. |
| `fonts/NotoSansHebrew-Regular.ttf`, `fonts/NotoSansThai-Regular.ttf` | The only committed fonts (17 and 21 KB, OFL-1.1, recorded in `REUSE.toml`), so that three probes run in the gate without downloads. |
| `src/fonts.rs` | Loads a font only if its file is the one the manifest pins (its name and SHA-256), before any parser sees it; the font book. |
| `src/itemize.rs` | Splits a paragraph into runs of one bidi level, one script and one font. |
| `src/segment.rs` | Line break opportunities (UAX #14) and grapheme clusters, from ICU4X. |
| `src/shape.rs` | Shapes a run with harfrust, kerning and ligatures switched on or off. |
| `src/linebreak.rs` | The greedy (first-fit) line breaker. |
| `src/layout.rs` | One paragraph from text to positioned glyphs: itemize, shape, measure in BLU, break, reorder, place; writes the layout JSON. |
| `src/raster.rs`, `src/png.rs` | Glyph outlines from skrifa, the reference rasterizer (tiny-skia) and vello_cpu, raster hashes, image comparison, a PNG writer. |
| `src/probes.rs` | The 30 probes (§2.2). |
| `src/run.rs`, `src/bench.rs` | A run's results file (every hash, per probe and in summary) and the measurements, including the full-screen viewport (§2.3). |
| `src/main.rs`, `src/wasm.rs` | The native runner (`text-pipeline run` and `bench`) and the WebAssembly interface (wasm-bindgen), which do the same. |
| `js/` | The WebAssembly runners, plain JavaScript without dependencies: `run-node.mjs` for Node.js, `run-chromium.mjs` for headless Chromium (a small server on 127.0.0.1 that serves the page, the module and the fonts, runs the spike in a module Web Worker as the web app runs the engine, and collects the files it writes), the shared `harness.mjs`, and `compare.mjs`, `summary.mjs` and `wasm-size.mjs` for the comparison job. |
| `results/expected.json` | The expected layout hash and reference raster hash of every probe and of the viewport. |
| `tests/determinism.rs` | The gate's check (§3.3). |
| `.github/workflows/text-pipeline.yml` | CI in eleven environments and the comparison job (§3.2). |

The crate has 65 tests (64 unit tests and the determinism test), which the gate runs on the host and in WebAssembly like every other crate's.

### 2.1 The pipeline, step by step

1. **Fonts.** A probe names its fonts, first choice first. Each is loaded from bytes whose SHA-256 the manifest pins, and the layout JSON records the file and the hash of every font it used, so that a layout can never be drawn with another font by mistake (ADR-0004 §2: fonts come from content-hashed files).
2. **Bidi levels.** unicode-bidi resolves the embedding level of every character for the paragraph's direction, with the Bidi_Class property from ICU4X's data rather than its own tables (ADR-0009 §3).
3. **Scripts and fonts.** ICU4X's Script and Script_Extensions properties give every character a script; characters common to many scripts (spaces, digits, punctuation) take the script of the letters around them, and combining marks take their base's. Each grapheme cluster (a letter with its marks, which must stay together) goes to the first font that has a glyph for all of it, so Latin words inside Chinese text fall back from Noto Sans SC to Liberation Sans. A run ends wherever the level, the script or the font changes.
4. **Line break opportunities.** ICU4X's line segmenter (UAX #14) with its dictionary models, which find word boundaries in Thai, Lao, Khmer and Burmese, where words are not separated by spaces. The LSTM models (machine-learning models with floating-point inference) are not even compiled (ADR-0009 §2). Mandatory breaks are those after a line or paragraph separator class (BK, CR, LF, NL) and at the end of the text.
5. **Shaping.** harfrust shapes each run with its script, its direction, the paragraph's language and the text around it (so that Arabic letters join across a run boundary), and with kerning and ligatures as the probe asks. Its output is in the font's own integer units.
6. **Measuring in BLU.** Every advance and offset is converted on its own, `units × size ÷ units_per_em`, rounded half to even with `bayan_units::scale`, and a line adds those integers. The line height is the largest `usWinAscent` plus the largest `usWinDescent` of the line's fonts, scaled the same way. Where Word rounds, and which metrics it uses, is for the Fidelity Lab to find out (LAB-005); the spike only needs a fixed rule.
7. **Breaking lines.** First fit at the opportunities of step 4: a line takes as many segments as fit in the measure; spaces at the end of a line hang outside it; a segment wider than the measure gets a line of its own and overflows (emergency breaking inside words is Word-specific work for later). Where a line boundary cuts a shaped run, the run's glyphs are kept if HarfBuzz marks the cut as safe and the pieces are shaped again if it is not (`UNSAFE_TO_BREAK`), so that, for example, an Arabic word broken across lines gets its final and initial forms.
8. **Reordering lines.** Each line is reordered with the rules L1 and L2 of the bidi algorithm (unicode-bidi's `visual_runs`); harfrust already returns right-to-left runs in visual order and mirrors brackets in them (rule L4). A left-to-right paragraph starts at the left edge, a right-to-left one ends at the right edge.
9. **The layout JSON and its hash.** For every line: its text range, its top, ascent, descent, left edge and advance; for every run on it, in display order: its font, script, level and text range, and every glyph's identifier, cluster, position and advance, all in BLU. The JSON is written without spaces and with a final line feed; its SHA-256 is the probe's *layout hash*.
10. **Rasterizing.** skrifa draws each glyph's outline unhinted, in font units (ADR-0011 §4). Each point becomes a device pixel coordinate with exact IEEE-754 operations in `f64`, `(origin + point × size ÷ units_per_em) ÷ 9,525`, rounded to the nearest `f32`: the device is a 96 dpi screen at device scale 2, so one pixel is 9,525 BLU. tiny-skia fills every glyph in black on a white image with an 8-pixel margin (§6 gives the details). The *raster hash* is the SHA-256 of the line `RGBA8 premultiplied <width>x<height>` followed by the pixels, so that the size is part of it; the PNG files are only for people to look at. The same scene is drawn with tiny-skia with gamma-2 blending and with three configurations of vello_cpu, for comparison.

### 2.2 The probe set

Thirty paragraphs, one per probe, at 10.5 to 14 pt on measures of 1 to 3.5 inches (21 of them 3.5 inches), chosen so that every feature of the brief appears and every probe needs two or more lines:

| Probe | What it checks |
|---|---|
| latin-sans-kerning, latin-sans-no-kerning | Latin with pair kerning (GPOS) in Liberation Sans, and the same text without it |
| latin-serif-kerning, latin-serif-no-kerning | the same in Liberation Serif |
| latin-carlito, latin-carlito-ligatures, latin-carlito-plain | Carlito (metric-compatible with Calibri) with kerning and the standard ligatures, the ligatures fi, fl, ff, ffi and ffl alone, and neither, as Word sets text by default; Liberation Sans and Serif have no ligatures, like Arial and Times New Roman |
| latin-punctuation-numbers | digits, currency, typographic quotes, dashes and fractions |
| latin-diacritics | precomposed letters and combining marks placed by GPOS |
| latin-long-word | a word wider than the measure |
| latin-break-classes | no break at a no-break space or a non-breaking hyphen, breaks after a hyphen and around an em dash, a soft hyphen (which the shaper hides) |
| arabic-marks, arabic-joining, arabic-lam-alef | Arabic with full vowel marks placed by mark attachment, joining forms across a paragraph, the obligatory lam-alef ligatures |
| arabic-numbers-latin | European numbers and Latin words in Arabic text (levels 1 and 2) |
| hebrew-numbers-latin, hebrew-points, hebrew-brackets | numbers, a Latin word and a maqaf in Hebrew; vowel points and dagesh (committed font only); mirrored brackets |
| devanagari-conjuncts, devanagari-reph, devanagari-numbers | conjuncts, half forms and vowel signs; reph and rakar; European and Devanagari digits |
| thai-dictionary, thai-marks, thai-mixed | Thai without spaces between words, broken with the dictionary model; stacked tone marks (both committed font only); Thai with digits and a Latin word |
| chinese-punctuation, chinese-mixed | Simplified Chinese with full-width punctuation (UAX #14 only, no kinsoku tailoring yet), and with English words and digits in a CFF font with Latin fallback |
| japanese-kana-kanji, japanese-katakana | hiragana, katakana and kanji, which itemize as three scripts; the prolonged sound mark, a character common to two scripts |
| mixed-ltr-with-rtl, mixed-rtl-with-ltr | Hebrew with numbers and an Arabic word inside an English paragraph; an English quotation with parentheses and a time inside an Arabic paragraph |

The optional colour emoji was left out: none of the pinned fonts has colour glyphs, and rendering COLR glyphs is a separate rasterization path (ADR-0011 §4 names grayscale glyphs only). It is listed as a follow-up (§9).

### 2.3 The viewport

For the timing of AC-3 the spike fills one screen, 1920 × 1080 at device scale 2, that is 3840 × 2160 pixels: the 30 probes, laid out again at the width of the screen less its margins, stacked from the top until the screen is full (46 paragraphs, 4,859 glyphs). Its layouts and its rasters are hashed like a probe's, so the measurement draws exactly what the comparison checks.

## 3. Results against the acceptance criteria

| | Criterion | Result |
|---|---|---|
| AC-1 | Layout JSON hashes identical across all five environments for every probe | **Met** in eleven environments, for all 30 probes and the viewport (§3.1, §3.2). |
| AC-2 | Reference-mode raster hashes identical across all five environments | **Met** in the same eleven environments, with tiny-skia built without `simd` and with `no-std-float` (§4). |
| AC-3 | Throughput, raster timing, size and memory; a recommendation on vello_cpu with its measured tolerance | **Reported** (§7); vello_cpu not recommended for on-screen use under ADR-0011 §3, with its tolerance measured (§5). |
| AC-4 | Gamma and blending choices documented | **Met** (§6). |
| AC-5 | Dependencies pinned, at least 24 hours old, allowlisted, justified | **Met** (§8): the youngest was 7 days old when it was added. |

### 3.1 The environments

| Environment | What runs |
|---|---|
| `linux-x86_64`, `windows-x86_64`, `macos-arm64` | the native runner, built for each system by GitHub's `ubuntu-24.04`, `windows-latest` and `macos-latest` (Apple silicon) runners |
| `wasm32-node-linux-x86_64`, `wasm32-node-windows-x86_64`, `wasm32-node-macos-arm64` | the WebAssembly module (wasm32-unknown-unknown, without SIMD) in Node.js 24.21.0 on the same three systems |
| `wasm32-simd128-node-…` (three) | the same module built with WebAssembly's 128-bit SIMD (`+simd128`; relaxed SIMD stays off), in Node.js on the three systems |
| `wasm32-chromium-linux-x86_64`, `wasm32-simd128-chromium-linux-x86_64` | both modules in Chrome for Testing's headless shell 153, in a module Web Worker |

The WebAssembly modules are built once, on Linux, and the same files run everywhere, as the web app's will; a module's results therefore show whether the WebAssembly engines (V8 on x86-64 and arm64, in Node.js and in Chromium) compute it identically. The simd128 module lets the compiler, and vello_cpu's SIMD layer, use WebAssembly's SIMD instructions, which every engine must compute identically; relaxed SIMD, whose results the standard leaves to the engine, is never enabled.

### 3.2 The hashes

In the [CI run](https://github.com/BayanDocs/bayan-core/actions/runs/38194692403) on commit ddd39a5, all eleven environments produced the same two summary hashes, which cover the 30 probes (each the SHA-256 of one line per probe: its name and its hash), and the same viewport hashes; the comparison job also checked every probe's two hashes one by one against `results/expected.json` (its job summary has a table with every probe and every rendering, and its log ends with `compare: 11 environments agree on every layout and reference raster`):

| | SHA-256 |
|---|---|
| Layouts of the 30 probes | `d6b2627427347b553a8d11723b1d1ea5bdc93532423a3dbfa81c2276f0c1192b` |
| Reference rasters of the 30 probes | `b4bbab9dc4783a840f919dcec3da858458259abf4d5dc459be9ac5a2d94c41d3` |
| Layouts of the viewport (46 paragraphs) | `5c6407f3c91a5d33fed46c06343d3d2dab14fe3707b283944107303bd36d99d4` |
| Reference raster of the viewport | `4bc6b158b56998e32cfc43befd0d52b74557e71fed8daf44c3bf316ceebbc3a3` |

The renderings that are only reported, not required, agreed too: tiny-skia with gamma-2 blending gave the same hashes in all eleven environments, and so did vello_cpu at its portable level, with both pipelines; vello_cpu at the detected SIMD level split into two groups (§5.1). Before the first push the same commands ran locally in the five Linux x86-64 environments, with the same hashes. Each environment's files (results, layout JSON and PNG images of every rendering) are artifacts of the run, kept 14 days; `text-pipeline-summary`, with every `results.json` and `bench.json`, is kept 30 days.

### 3.3 In the gate

`tests/determinism.rs` lays out and rasterizes the three probes whose fonts are committed (hebrew-points, thai-dictionary and thai-marks: Hebrew with vowel points, Thai broken with the dictionary model, and stacked Thai marks) and compares their layout hashes and reference raster hashes with `results/expected.json`. The fonts are compiled into the test, so it reads no files and runs in WebAssembly too. The gate's `determinism` step runs it on the host and in WebAssembly (`xtask/src/verify.rs`, next to bayan-units' fingerprints), and CI runs the gate on Linux, Windows and macOS: every pull request therefore checks these three probes on four platforms, whatever it changes. A one-digit change in the expected file makes the test fail (checked before committing).

## 4. Determinism: what could differ between platforms, and how each is ruled out

The pipeline is deterministic by construction where the spike's own code is concerned: layout is integer BLU arithmetic (ADR-0005), maps are ordered (`BTreeMap`), nothing reads a clock, a locale, an environment variable or the installed fonts, everything runs on one thread, and Rust never fuses or reorders floating-point operations on its own. The risks are in the libraries. Each one found is in this table, with what rules it out; the eleven environments in CI are the test.

| Risk | Where | What rules it out | Evidence |
|---|---|---|---|
| SIMD code paths that differ by CPU: approximate reciprocals and square roots (`_mm_rcp_ps`, `_mm_rsqrt_ps`, NEON's `vrsqrteq_f32`), and min and max instructions that treat NaN and the sign of zero differently | tiny-skia's `simd` feature (`src/wide/`) | tiny-skia is built without `simd` (`default-features = false` in `Cargo.toml`): every target runs the same portable code, whose min and max are explicit comparisons | identical reference rasters natively on x86-64 and arm64 and in WebAssembly on both |
| Platform math functions, which may differ in the last bit between C libraries: `acosf`/`acos` and `cos` in the cubic root finder of the edge clipper (`path64`), `powf` for the cube root in path geometry | tiny-skia-path through tiny-skia | `no-std-float`: tiny-skia-path's `Scalar` functions call the libm crate, pure Rust, the same on every target (ADR-0005 §4) | same |
| `hypot` for the offsets of transformed components in TrueType composite glyphs | skrifa's outline code (and read-fonts) with `std` | both are built with their `libm` feature and without `std`; harfrust, which shares read-fonts, likewise (then it brings spin, for a lock in its caches, and core_maths) | none of the 108 composite glyphs the probes draw has a transformed component, so the probes do not reach this code: the `libm` feature makes it deterministic by construction (a probe that reaches it is a follow-up, §9) |
| Floating-point inference in the LSTM line-break models | icu_segmenter's `lstm` and `auto` features | neither feature is enabled, so the LSTM code is not compiled; Thai, Lao, Khmer and Burmese use the dictionary models (ADR-0009 §2) | thai-dictionary breaks Thai into the same lines everywhere |
| Unicode tables bundled with a library in another version than ICU4X's | unicode-bidi's `hardcoded-data` | off: unicode-bidi reads Bidi_Class from ICU4X's data, so one Unicode version serves bidi, scripts and segmentation | — |
| Fused multiply-add where the CPU has it | fearless_simd, vello_cpu's SIMD layer: `mul_add` is fused on AVX2 and NEON, separate operations on SSE4.2, the portable level and WebAssembly (relaxed SIMD off) | only concerns vello_cpu, which is not the reference; forcing its portable level removes it (§5) | vello_cpu at its detected level gives one result natively (AVX2 and NEON agree: a fused multiply-add is itself exactly rounded) and another in WebAssembly, for 3 of 30 probes and the viewport; at its portable level it is identical everywhere |
| WebAssembly relaxed SIMD, implementation-defined | any crate that checks `target_feature = "relaxed-simd"` (tiny-skia and fearless_simd do) | the modules are built without it (the default; `+simd128` only for the SIMD module); the workflow builds them, and nothing enables more features | identical rasters in V8 on x86-64 and arm64 |
| The sign of zero in `f32::min` and `f32::max`, which Rust leaves unspecified for `min(-0.0, +0.0)` | tiny-skia-path's `Scalar::bound` (used for t values and the cosine argument in the cubic clipper) and tiny-skia's edge clipper, in the portable code too | none needed for pixels: the values are only compared, passed to `acos` (where −0 and +0 give the same result) or turned into fixed point, where −0 and +0 are the same; still a follow-up for the production rasterizer (§9) | identical rasters |
| NaN payloads, which WebAssembly may set differently | any floating-point code | images and layouts hold no floating-point values (u8 pixels, integer BLU), so a NaN could only matter through a comparison, which treats all NaNs alike | identical hashes |
| Iteration order of hash maps | the spike (forbidden by Clippy), dependencies (hashbrown in vello_cpu) | the spike uses ordered maps; the reference path has no hash map | — |
| Compiled-in data replaced at build time | ICU4X's data crates: their build scripts switch to the data in `ICU4X_DATA_DIR` if that environment variable is set | nothing sets it, here or in CI; a follow-up for the work package that ships ICU4X (§9) | — |

**Every risk in this table was removed by choosing Cargo features**, found by reading the libraries' code: nothing had to be patched, no code path had to be forced beyond those features, and no comparison in any environment has shown a difference in a layout or a reference raster.

## 5. vello_cpu for interactive rendering

ADR-0011 §3 asks CORE-003 to benchmark vello_cpu against tiny-skia, and allows it on screen only if it differs from the reference "by at most ±2 per color channel at anti-aliased edges and never in geometry". The spike draws every probe's scene with three configurations: the u8 pipeline (vello_cpu's fast one) at the best SIMD level the machine offers, as an application would run it; the u8 pipeline forced to the portable level (`Level::fallback()`); and the f32 pipeline (its quality one) at the portable level. All run on one thread.

### 5.1 Is it deterministic?

On its portable level, yes, in every environment: the u8 and f32 renderings of every probe and of the viewport had the same hash in all eleven. At the level it detects, its result depends on whether the processor fuses multiply-adds. Natively it chose AVX2 on the Linux and Windows runners and NEON on macOS, which both fuse, and the three native results are identical to each other: a fused multiply-add is exactly rounded, so it gives the same bits on every processor that has one. In WebAssembly it chose its portable level, or `WasmSimd128` in the simd128 module, neither of which fuses, and all eight WebAssembly results are identical to each other and to the portable level's. The two groups differ in three probes (latin-carlito, chinese-mixed, japanese-kana-kanji) and in the viewport, in one pixel each, by one level (measured locally on AVX2). On an x86-64 processor without AVX2 and FMA (an older one, or a virtual machine that hides them) the detected level would be SSE4.2, which does not fuse, so it would join the second group.

### 5.2 How far is it from the reference?

| | Largest difference per channel (of 255) | Pixels differing by more than 2 |
|---|---|---|
| Across the 30 probes | 56 to 79 | 3.5 % to 12.4 % of each image |
| ADR-0011 §3 allows | 2 | none beyond 2 |

The differences are anti-aliasing, not geometry. In four probes examined pixel by pixel (Latin, Arabic, Chinese, Thai), 82 to 91 % of the reference's edge pixels (neither white nor black) differ by more than 2, roughly as many lighter as darker (44 to 50 % of them darker), so neither rasterizer draws glyphs fatter or thinner than the other; fewer than 2 % of the pixels the reference draws pure white, and 10 to 17 % of those it draws pure black, also differ by more than 2, all next to edges; and at most five pixels in a probe differ by more than 64. The reason is how coverage is computed. tiny-skia, like Skia's CPU rasterizer, supersamples: it samples each pixel on a grid of 4 × 4 positions, so an almost horizontal edge (the top of an x, a stroke of a Chinese character) moves its pixels' shade in steps of a quarter, 64 levels. vello_cpu computes the exact area of each pixel that the outline covers. Up to about 64 levels apart is therefore what the two methods must give, and a little more where an edge crosses a pixel twice. The gap is a property of tiny-skia's coarser anti-aliasing as much as of vello_cpu, and no rasterizer that anti-aliases differently from tiny-skia can meet ±2.

### 5.3 How fast is it?

Drawing the 3840 × 2160 viewport, median of five, on one thread (§7.2 has every environment):

| CI | tiny-skia, reference | vello_cpu u8, detected SIMD | vello_cpu u8, portable | vello_cpu f32, portable |
|---|--:|--:|--:|--:|
| Native, Linux x86-64 | 52 ms | 32 ms (AVX2) | 40 ms | 61 ms |
| Native, macOS arm64 | 39 ms | 13 ms (NEON) | 17 ms | 24 ms |
| Node.js on Linux, WebAssembly without SIMD | 198 ms | 173 ms | 182 ms | 238 ms |
| Node.js on Linux, WebAssembly with simd128 | 147 ms | 106 ms | 122 ms | 187 ms |

vello_cpu's u8 pipeline is 1.7 to 2.9 times as fast as the reference natively (1.3 to 2.3 at its portable level) and 1.1 to 1.5 times in WebAssembly. Its f32 pipeline gives almost exactly the u8 pipeline's pixels (identical in the probes examined, or at most one level apart in a few pixels) and is slower than tiny-skia except natively on Apple silicon.

### 5.4 Recommendation

1. **Do not adopt vello_cpu for on-screen rendering now.** Under ADR-0011 §3 it fails the ±2 tolerance by a wide margin, and the margin is inherent to its better anti-aliasing. Its speed advantage is real natively but modest in WebAssembly, where the web app runs the engine (at most 1.5 times); the reference is fast enough when only changed tiles are redrawn (a 256 × 256 tile is 1/127 of the viewport: about 1 to 2.5 ms even in WebAssembly, if the time scales with the area); and one rasterizer means what users see is exactly what tests and exports check. It would also add about 105 KB (brotli) to the web download, and it is young: ADR-0011 records its first release in July 2026, and the pinned 0.3.0 was published on 2026-10-02.
2. **Keep vello_cpu as the named successor if tiny-skia loses maintenance** (ADR-0011, "Revisit when"). Its portable level was bit-identical in all eleven environments and faster than tiny-skia, with exact-area anti-aliasing. Before it could become the reference, a work package would have to confirm that upstream keeps that level free of fused operations and platform math, pin it in a test like this spike's, and accept that every reference raster changes (a layout epoch, ADR-0004 §4).
3. **If an interactive rasterizer is wanted later** (for example for very large screens or GPU-less zooming), ADR-0011 §3's tolerance should be restated for what can be measured: the same display list and glyph positions, differences only at anti-aliased edges, and a bound that a different anti-aliasing method can meet (for example a limit on how many pixels may differ by more than 2, plus a perceptual check), decided with the visual tests of the web and desktop shells. This spike's numbers (§5.2) are the starting point.

## 6. Gamma and blending

The reference mode fixes these choices (`src/raster.rs`); together with the pinned libraries and the rule for placing outlines (§2.1, step 10) they determine every pixel.

- **Outlines:** skrifa's, unhinted (ADR-0011 §4), at the font's default instance (the pinned fonts are not variable), converted to device pixels with exact arithmetic and rounded once to `f32`. Glyphs are placed where the layout puts them, without snapping to whole pixels, so the same word can look slightly different at different positions; that is what makes the raster depend only on the layout.
- **Coverage:** tiny-skia's anti-aliasing, a 4 × 4 supersampling of each pixel (Skia's `SUPERSAMPLE_SHIFT` of 2), grayscale only. No subpixel (ClearType-style) rendering, which depends on the screen (ADR-0011 §4). Fill rule non-zero, as TrueType and CFF outlines need.
- **Colour and blending:** an opaque white page, glyphs in opaque black, composited source-over, directly on the stored sRGB values (tiny-skia's `ColorSpace::Linear`, which means "no conversion": the coverage is applied linearly to the encoded values). Pixels are 8-bit RGBA, premultiplied, with no colour management (ADR-0011 §7 brings colour management for PDF, not for the screen raster).
- **The alternative, measured:** blending in an approximation of linear light, with gamma 2 (tiny-skia's `ColorSpace::Gamma2`: square, blend, square root with IEEE-754's exactly rounded square root). It is deterministic too (identical hashes in all eleven environments) and changes every edge pixel, by up to 64 levels, making black text on white look slightly lighter and thinner. It costs 3 to 55 % more time (§7.2).

**Recommendation: keep the direct sRGB blend for the reference mode.** Gamma-correct blending matters for how text looks, not for what the reference is for (comparing our output across platforms and versions, and with Word's in the Fidelity Lab, where the lab's own comparison decides which tolerances apply). It is cheaper, it is the default of every 2D library the project uses, and changing it later only changes the screen raster, never a layout. If the shells want text that looks closer to each platform's, the place for gamma and contrast adjustment is the on-screen path, and ADR-0011 would have to say so explicitly, because it would make the screen differ from the reference.

## 7. Measurements

The measurements run in the same harness in every environment (`src/bench.rs` natively, `js/harness.mjs` in WebAssembly, with the host's clock): shaping and layout rounds over the 30 probes until at least two seconds have passed, after one round to warm up; five renderings of the viewport with each rasterizer, reporting the median. Everything runs on one thread. CI runners are shared virtual machines, so the CI figures vary from run to run by tens of percent and are best compared within one run; the job summary of the comparison job has them all.

### 7.1 Shaping and layout throughput

One round shapes, or shapes and lays out, the 2,951 characters of the 30 probes (all scripts mixed, so these are not Latin-only figures). "Shaping and layout" includes itemization, segmentation, measuring, line breaking, reshaping at unsafe breaks, reordering and building the layout, but not writing the JSON.

| Environment (CI) | Shaping (characters per second) | Shaping and layout (characters per second) |
|---|--:|--:|
| Native, Linux x86-64 | 2,656,964 | 1,434,407 |
| Native, Windows x86-64 | 2,421,639 | 1,256,205 |
| Native, macOS arm64 | 3,709,600 | 1,669,566 |
| Node.js, Linux | 2,209,577 | 1,212,963 |
| Node.js, Windows | 1,659,561 | 860,434 |
| Node.js, macOS | 2,345,919 | 1,273,604 |
| Node.js, Linux, simd128 | 2,265,153 | 1,209,203 |
| Node.js, Windows, simd128 | 1,705,038 | 929,408 |
| Node.js, macOS, simd128 | 2,401,847 | 1,277,515 |
| Chromium, Linux | 2,295,534 | 1,212,073 |
| Chromium, Linux, simd128 | 2,254,339 | 1,191,311 |

At these rates a 1,000-page document of about 3 million characters is shaped in 0.8 to 1.2 s natively and 1.3 to 1.8 s in WebAssembly, on one core, and fully laid out in 1.8 to 2.4 s and 2.3 to 3.5 s. PERF-03 asks for 10 s of complete background layout for such a document, so the stack leaves room for the Word measurement model and the rest of layout, which will cost more than this spike's simple rules; shaping caches (the same word in the same font gives the same glyphs) are the obvious first optimization. Locally the same harness measured 2.3 million characters per second natively and 1.3 to 1.5 million in WebAssembly: shared machines vary.

### 7.2 Rasterizing a 1920 × 1080 viewport at device scale 2

The viewport of §2.3, 3840 × 2160 pixels, full of text, drawn from scratch, median of five:

| Environment (CI) | tiny-skia reference | tiny-skia gamma 2 | vello_cpu u8, detected | vello_cpu u8, portable | vello_cpu f32, portable |
|---|--:|--:|--:|--:|--:|
| Native, Linux x86-64 | 52 ms | 79 ms | 32 ms | 40 ms | 61 ms |
| Native, Windows x86-64 | 56 ms | 87 ms | 29 ms | 36 ms | 57 ms |
| Native, macOS arm64 | 39 ms | 46 ms | 13 ms | 17 ms | 24 ms |
| Node.js, Linux | 198 ms | 237 ms | 173 ms | 182 ms | 238 ms |
| Node.js, Windows | 290 ms | 299 ms | 217 ms | 217 ms | 293 ms |
| Node.js, macOS | 223 ms | 248 ms | 170 ms | 169 ms | 201 ms |
| Node.js, Linux, simd128 | 147 ms | 188 ms | 106 ms | 122 ms | 187 ms |
| Node.js, Windows, simd128 | 186 ms | 227 ms | 128 ms | 146 ms | 225 ms |
| Node.js, macOS, simd128 | 177 ms | 204 ms | 131 ms | 136 ms | 171 ms |
| Chromium, Linux | 194 ms | 230 ms | 173 ms | 173 ms | 231 ms |
| Chromium, Linux, simd128 | 144 ms | 182 ms | 105 ms | 120 ms | 185 ms |

A full redraw of a whole high-resolution screen is the worst case; scrolling and typing redraw a few tiles. The reference rasterizer is therefore fast enough on its own for on-screen use. The simd128 module makes the reference 21 to 36 % faster, by letting the compiler vectorize tiny-skia's portable code, with the same pixels; whether the web app ships it is a decision for the web shell (every browser the project supports has had WebAssembly SIMD since 2021–2023).

### 7.3 WebAssembly size

The spike's module as shipped, that is without its custom sections (function names and tool metadata), compressed with gzip at level 9 and brotli at quality 11 (`js/wasm-size.mjs`):

| | Raw | gzip | brotli |
|---|--:|--:|--:|
| Whole module, without SIMD | 4,433,786 | 1,748,828 | 1,348,964 |
| Whole module, with simd128 | 4,392,755 | 1,723,574 | 1,342,227 |

By library (module without SIMD, as built in CI; each part compressed on its own, so the parts add up to a little more than the whole):

| Part | Raw | brotli |
|---|--:|--:|
| ICU4X's compiled data, the other libraries' tables and strings (data segments) | 2,159,689 | 823,322 |
| read-fonts and skrifa (font parsing, outlines) | 673,237 | 135,684 |
| harfrust (shaping) | 548,377 | 128,991 |
| vello_cpu and its crates | 436,537 | 105,605 |
| tiny-skia | 294,392 | 60,278 |
| Rust's standard library, wasm-bindgen and small helpers | 113,761 | 32,176 |
| the spike, bayan-units and serde_json | 86,643 | 26,696 |
| ICU4X's code | 49,394 | 16,658 |
| DEFLATE and SHA-256 (for PNG files and hashes) | 28,805 | 9,495 |
| unicode-bidi | 12,652 | 5,178 |
| unnamed or other | 18,683 | 7,166 |

**What the engine would add** with this stack is about 1.2 MB with brotli without vello_cpu (and less once ICU4X's data is sliced), against the 5 MB of PERF-06 for the whole web app; ADR-0008 estimates about 1 MB gzipped for Loro. The data segments hold mostly ICU4X's compiled data: the four dictionaries of the line segmenter, for Burmese, Khmer, Lao and Thai, alone take 1,773,172 bytes (82 % of the data, by the count in ICU4X's generated data files); the rest is the line and grapheme break rules, the property tables, and the other libraries' tables and strings. ADR-0009 §2 already plans to slice the data to what the engine ships; the dictionaries are needed only for text in those four scripts and could be downloaded on first use, like fonts. The CI build is 1.4 KB larger than the local one because the module's panic messages name 196 source files by their place in the build machine's Cargo folder (`/home/runner/…` instead of `/root/…`): a module built on another machine is not byte for byte the same, and it shows the build machine's folder names (§9).

### 7.4 Memory

Peak memory after each phase of the measurement, in MiB: natively the process's peak resident memory (Linux's VmHWM; the runner reports none on Windows and macOS), in WebAssembly the size of the module's linear memory, which never shrinks, so each figure is the most needed so far:

| Environment (CI) | Fonts loaded | Shaping and layout | Reference raster | vello_cpu |
|---|--:|--:|--:|--:|
| Native, Linux x86-64 | 17.2 | 22.1 | 53.9 | 89.8 |
| WebAssembly (all eight runs) | 17.2 | 20.3 | 51.9 | 115.3 |

The nine font files take 14.0 MiB, 12.3 MiB of it the two CJK fonts; the layout itself needs a few MiB; one 3840 × 2160 RGBA image is 31.6 MiB, which explains the step to the reference raster (a tiled renderer holds a few tiles instead). vello_cpu keeps its own buffers besides the image.

## 8. Dependencies

### 8.1 What was added and why

All are declared once in the root `Cargo.toml`, pinned with `=`, and used only by this spike. The publish dates are crates.io's; every license is on ADR-0017's allowlist (`cargo deny` checks it).

| Crate | Version, published | License | What it does here | Why not something else |
|---|---|---|---|---|
| harfrust | 0.14.0, 2026-10-04 | MIT | shaping (HarfBuzz 14.5 in Rust, `#![forbid(unsafe_code)]`) | ADR-0009 §1; HarfBuzz itself is C++ (ADR-0006), rustybuzz is archived |
| skrifa | 0.48.0, 2026-10-03 | MIT OR Apache-2.0 | glyph outlines; read-fonts, its parser, is harfrust's too (`#![forbid(unsafe_code)]`) | ADR-0009 §1 and ADR-0011 §4; FreeType is C |
| icu_segmenter | 2.3.0, 2026-08-13 | Unicode-3.0 | UAX #14 line breaks with dictionaries, grapheme clusters | ADR-0009 §2 |
| icu_properties | 2.3.0, 2026-08-13 | Unicode-3.0 | Script, Script_Extensions, Line_Break and Bidi_Class | ADR-0009 §3 |
| unicode-bidi | 0.3.18, 2024-12-16 | MIT OR Apache-2.0 | the bidi algorithm | ADR-0009 §3; ICU4X has no bidi algorithm of its own |
| tiny-skia | 0.12.0, 2026-02-02 | BSD-3-Clause | the reference rasterizer | ADR-0011 §2 |
| vello_cpu | 0.3.0, 2026-10-02 | Apache-2.0 OR MIT | the rasterizer ADR-0011 §3 asks to evaluate (`#![forbid(unsafe_code)]`) | the only candidate the ADR names |
| fearless_simd | 0.7.0, 2026-08-11 | Apache-2.0 OR MIT | vello_cpu's SIMD layer, named only to switch on its portable level for the comparison | (the version vello_cpu requires) |

The spike also uses crates the workspace already pins: bayan-units (BLU), miniz_oxide (DEFLATE for PNG files, CORE-005's pin), sha2 (hashes), serde and serde_json (the JSON files) and wasm-bindgen (the WebAssembly interface).

### 8.2 What they bring in

50 packages new to `Cargo.lock`, all from crates.io. `cargo xtask check-lockfile-age` checked each against crates.io before the first push: the youngest, harfrust 0.14.0, was published 7 days 23 hours before the commit that added it, and every checksum is the one crates.io published. No version of a package that was already in the lockfile changed. Besides the eight above: arrayref, bytemuck, bytemuck_derive, color, core_maths, displaydoc, euclid, font-types, foldhash, glifo, guillotiere, icu_collections, icu_locale_core, icu_locale_fallback, icu_locale_fallback_data, icu_properties_data, icu_provider, icu_segmenter_data, kurbo, linebender_resource_handle, litemap, peniko, polycool, potential_utf, read-fonts (0.41.0 and 0.45.0), skrifa 0.44.0, spin, strict-num, synstructure, tiny-skia-path, tinystr, utf8_iter, vello_common, writeable, yoke, yoke-derive, zerofrom, zerofrom-derive, zerotrie, zerovec and zerovec-derive. Two versions of read-fonts and skrifa appear because vello_cpu names glifo, which uses the older ones, through a feature that nothing switches on (`glifo?/libm`): Cargo writes such packages into the lockfile, and cargo-deny checks them, but they are never compiled (`cargo tree` shows none of them).

### 8.3 Code that runs at build time

Three new packages have build scripts: icu_locale_fallback_data, icu_properties_data and icu_segmenter_data, each the same short program, read in full: if the environment variable `ICU4X_DATA_DIR` is set, it tells the compiler to take the data from that folder instead of the compiled-in data (`cargo:rustc-cfg=icu4x_custom_data`), and it declares one more condition name. That variable could therefore change the Unicode data, and with it every layout, without any file in the repository changing (§9). Five are derive macros, which run while their users compile: bytemuck_derive, displaydoc, yoke-derive, zerofrom-derive and zerovec-derive (with their helper library synstructure). None of these is new in kind: the workspace already compiles derive macros of the same sort, such as serde's, thiserror's and wasm-bindgen's.

### 8.4 unsafe code

harfrust, read-fonts, skrifa, vello_cpu and vello_common forbid unsafe code. tiny-skia uses it for SIMD intrinsics, which this build leaves out, and in its portable code only for constructors of its checked number types and two `bytemuck::Pod` declarations; ICU4X's zero-copy data structures (zerovec, yoke, zerotrie) and fearless_simd's SIMD levels use it extensively, as is normal for such crates. None of it parses untrusted input in a way ADR-0006 rules out (no C or C++ code), and none of it is in the spike's own code (`#![forbid(unsafe_code)]`).

## 9. Limitations and follow-ups

- **The spike is not Word's layout** (out of scope, as the brief says): the line breaker has no Word tailoring (kinsoku, hanging punctuation, hyphenation, emergency breaking inside words), the measurement and line-height rules are the spike's own, font fallback is per grapheme cluster with a fixed font list, and there is no justification, tab, vertical text or kashida. CORE-008 and LAB-005 build these, and this spike's harness can carry their probes.
- **Make the determinism suite permanent in the engine.** When the text and raster crates exist (Phase 1), move `tests/determinism.rs`, the probe set and the eleven-environment workflow into them, and make the comparison a required check, which needs it to run on every pull request instead of behind a path filter (`docs/scripts/github-settings.py`).
- **Refuse `ICU4X_DATA_DIR` where engine code is built** (§8.3): the work package that first ships ICU4X should make the gate's preflight and the artifact builds (`cargo xtask sdk`, `cargo xtask wasm-package`) fail when that variable is set, as they already do for `CLIPPY_CONF_DIR`.
- **tiny-skia's remaining `f32::min` and `f32::max`** (§4): harmless for pixels as argued there, but the production rasterizer (bayan-raster) should replace them with explicit comparisons, preferably upstream, so that no argument is needed.
- **Slice ICU4X's data** (ADR-0009 §2) and consider downloading the segmentation dictionaries on first use (§7.3).
- **Colour glyphs** (COLR, the brief's optional colour emoji) need a rendering path of their own and a pinned colour font; not tested here.
- **A probe with transformed composite glyphs** (§4): the only place where the font crates call `hypot`. The pinned fonts have none; a small font that has them, or a synthetic one, would make the determinism suite exercise that path instead of relying on the `libm` feature alone.
- **Fonts are hostile input in production.** Here every font is pinned by SHA-256 before a parser sees it; documents bring their own fonts, so the work package that loads embedded fonts needs fuzz targets for read-fonts, skrifa and harfrust with the engine's size and count limits (ADR-0006), next to the OSS-Fuzz coverage fontations already has.
- **Shaping caches** (§7.1), and tiled rendering with caching (ADR-0011 consequences), are the first performance work for the production crates.
- **Align the layout output with the lab's schema** (LAB-003, "CORE-003 output format alignment"): the spike's layout JSON is per paragraph and records what determinism needs (glyph identifiers, font hashes, positions in BLU); when LAB-003's schema v1 exists, the production layout output should emit it, with these fields kept for the determinism suite.
- **Reproducible engine artifacts** (§7.3): the WebAssembly module, and presumably the C SDK's libraries, name dependency source files by their path in the build machine's Cargo folder. `cargo xtask sdk` and `cargo xtask wasm-package` (CORE-007) could pass `--remap-path-prefix` for that folder, so that the same commit gives the same bytes everywhere and no build machine's folder names ship.

## 10. Questions for the owner

1. **vello_cpu (§5.4):** do you agree not to adopt it for on-screen rendering now, to keep it as the named successor of tiny-skia, and to restate ADR-0011 §3's tolerance only when an interactive rasterizer is wanted again? The docs pull request records it in ADR-0011's validation result.
2. **Gamma (§6):** do you agree that the reference mode blends directly on sRGB values (no gamma correction), with gamma-correct text a possible later choice for the screen path only?
3. **The two committed fonts** (OFL-1.1, 38 KB together, `REUSE.toml`): acceptable? They let the gate check three probes on every pull request without downloading anything.

## 11. How to reproduce

```sh
# Fonts (about 15 MB, checked against manifest.txt) into target/text-pipeline/fonts
spikes/text-pipeline/fonts/fetch.sh

# Natively: every probe, the viewport and the measurements
cargo build --package text-pipeline --release --locked
target/release/text-pipeline run --fonts target/text-pipeline/fonts --out target/text-pipeline/out/native --environment linux-x86_64
target/release/text-pipeline bench --fonts target/text-pipeline/fonts --out target/text-pipeline/out/native/bench.json --environment linux-x86_64

# The WebAssembly module, without and with simd128 (wasm-bindgen 0.2.129, as scripts/dev-setup.sh installs it)
cargo rustc --package text-pipeline --lib --release --locked --target wasm32-unknown-unknown --crate-type cdylib
wasm-bindgen --target web --out-dir target/text-pipeline/wasm/default target/wasm32-unknown-unknown/release/text_pipeline.wasm
CARGO_TARGET_DIR=target/simd128 RUSTFLAGS="-C target-feature=+simd128" cargo rustc --package text-pipeline --lib --release --locked --target wasm32-unknown-unknown --crate-type cdylib
wasm-bindgen --target web --out-dir target/text-pipeline/wasm/simd128 target/simd128/wasm32-unknown-unknown/release/text_pipeline.wasm

# In Node.js 24, and in a Chromium headless shell (the one the workflow pins)
node spikes/text-pipeline/js/run-node.mjs target/text-pipeline/wasm/default target/text-pipeline/fonts target/text-pipeline/out/node wasm32-node
node spikes/text-pipeline/js/run-chromium.mjs <chrome-headless-shell> target/text-pipeline/wasm/default target/text-pipeline/fonts target/text-pipeline/out/chromium wasm32-chromium

# Compare with the expectation and with each other, and summarize the measurements
node spikes/text-pipeline/js/compare.mjs spikes/text-pipeline/results/expected.json target/text-pipeline/out/*/results.json
node spikes/text-pipeline/js/summary.mjs target/text-pipeline/out/*/bench.json
node spikes/text-pipeline/js/wasm-size.mjs target/text-pipeline/wasm/default/text_pipeline_bg.wasm

# Only the probes with committed fonts (no download), on the host; the gate's determinism step runs them in WebAssembly too
cargo test --package text-pipeline --test determinism --locked -- --show-output
```

`run` writes `results.json`, the layout JSON of every probe (`layout/`) and every rendering as a PNG file (`png/`); `node compare.mjs --expect <results.json>` prints a new expectation after a deliberate change of the output, which then replaces `results/expected.json` in a pull request that says why.
