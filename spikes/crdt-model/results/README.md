# CORE-004 raw results

The measurements behind `../REPORT.md`, as the programs printed them (`metric <name> <value> <unit>` lines, or the drivers' own lines). All were taken on 2026-10-07 on the machine described in the report (4 virtual CPUs, Intel Xeon @ 2.10 GHz, 16 GB RAM, Linux 6.18), with Rust 1.99.0 and Node.js 24.21.0, from the code in this branch.

| Files | What | Produced by |
|---|---|---|
| `generate-full.txt`, `generate-subset.txt` | Building the synthetic 500-page documents: size, content counts, the view's checks | `crdt-model generate --mode full\|subset --out <file>` |
| `native-full-{1,2,3}.txt`, `native-subset-{1,2,3}.txt` | Benchmarks, native x86-64 release build, unmodified Loro 1.16.2 | `crdt-model bench --mode full\|subset --snapshot <file>` |
| `wasm-full-{1,2,3}.txt`, `wasm-subset-{1,2,3}.txt` | The same in WebAssembly (`wasm32-wasip1` under Node.js), unmodified Loro | `node run-wasi.mjs target/wasm32-wasip1/release/crdt-model.wasm <folder> bench --mode … --snapshot /data/<file>` |
| `native-full-shallow-{1,2,3}.txt` | The full-model benchmark opened from the shallow snapshot (old history trimmed) instead of the full one, natively (report §7, F11) | `crdt-model bench --mode full --snapshot <file>.shallow` |
| `native-patched-loro-*.txt`, `wasm-patched-loro-*.txt` | The same with Loro's quadratic rich-text decoding fixed (report §7, F1): a copy of `loro-internal` 1.16.2 with that one change, used through `[patch.crates-io]` in a scratch copy of the workspace. Measured to show the effect of the fix; nothing in the repository uses the patched copy. | as above |
| `converge-1000x10000.log` | The full convergence run of the brief, in two parts (the first was stopped after run 274 to free the machine for the final benchmarks, and the second continued from run 275 on four threads): one line per run, then a combined summary that counts each run once | `crdt-model converge --runs 1000 --operations 10000 --threads 3`, then `--runs 725 --threads 4 --seed 0xc0de000400010113` |
| `normalization-100000.txt` | The normalization property test with 100,000 randomly broken documents | `BAYAN_NORMALIZATION_CASES=100000 cargo test --release -p bayan-model --test normalization` |
| `fuzz-supervise-4500s.log` | The 75-minute fuzzing run (one worker, restarted after each abort), with the panic sites and aborts it found | `crdt-model supervise --seconds 4500 --workers 1 --out <folder>` |
| `profile-*.txt` | Excerpts of callgrind profiles (inclusive instruction counts) that explain the performance misses: the first read, the first update after opening (from the full and from the shallow snapshot), 1,000 concurrent edits and undo | `valgrind --tool=callgrind --collect-atstart=no --toggle-collect=<function> …` on the patched build |

The benchmark runs were made one after another on an otherwise idle machine (the convergence run was stopped meanwhile); the convergence and fuzzing runs ran in parallel with each other, so their run times are longer than on an idle machine. Every run starts from fixed seeds, so it can be repeated exactly (commands in the report, §12); the timings will differ with the machine.
