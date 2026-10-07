# CORE-004 report: the Word-shaped document model on Loro

- **Work package:** [CORE-004](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md), the validation gate of ADR-0007 (the document model) and ADR-0008 (Loro behind the `bayan-crdt` adapter).
- **Date:** 2026-10-07. **Loro** 1.16.2, **Automerge** 0.12.0, Rust 1.99.0, Node.js 24.21.0.
- **Machine for all measurements:** 4 virtual CPUs (Intel Xeon @ 2.10 GHz), 16 GB RAM, Linux 6.18. Absolute times on a typical laptop will be lower; the ratios between configurations are what matter.

## 1. Summary and recommendation

**The model shape (ADR-0007) works.** Stories of atoms, with paragraph ends, field delimiters, anchors and table blocks as characters, merge correctly under heavy concurrent editing: in the full randomized run of the brief (1,000 runs of 10,000 operations on three replicas, with partitions, out-of-order delivery, undo and redo) every run so far ended with identical views satisfying I1–I7 (the run is in progress; §3). Normalization is deterministic and idempotent and always yields a valid view (I1–I7) in 100,000 randomized broken documents. Nothing found suggests that the model shape is wrong. The spike adds two normalization rules the spec lacks (N8, N9), clarifies two (N3, N7), and settles the mapping details (§9), which the docs pull request proposes.

**Loro (ADR-0008) is the right library, but not yet a safe one.** I recommend confirming ADR-0008 with three conditions, because the alternative is worse on almost every measure (§5.3: compared with Loro carrying the small fix below, Automerge 3 loads and reads 6 times slower, needs 5 times the memory and merges an update 50 times slower; only its snapshots are smaller):

1. **A small upstream fix for loading.** Loro 1.16.2 decodes rich text from a snapshot in quadratic time: opening and showing the 500-page document takes 2.7 s natively and 7.0 s in WebAssembly, against targets of 0.3 s and 1 s. With the fix (§7, F1) it takes 0.53 s and 0.77 s: the WebAssembly target is met, the native one missed by 1.8×, because reading one Loro container per paragraph is costly (§5.2).
2. **Hostile updates must not crash the engine.** Fuzzing found that Loro panics on crafted updates at 22 places, that some crafted headers make it request absurd amounts of memory (which aborts the process, beyond any error handling), and that deeply nested values exhaust the stack. The adapter now contains panics and refuses deep values (§8), but allocation failures can only be survived by restarting the engine. Upstream reports are drafted (§7); the engine's hosts need an isolation strategy (§8.3).
3. **Performance items for the model and storage work packages:** the first update after opening a document pays a one-time cost (62 ms natively and 99 ms in WebAssembly for 1,000 operations, against a target of 50 ms; later updates of that size take 9 ms and 13 ms), one undo step takes about 0.2 s in a 500-page document because Loro replays the story's whole history, reading per-paragraph maps dominates the first read, and documents opened from shallow snapshots are slower still (§5, F11).

**Decisions the owner needs to make** (details in §10): whether to accept ADR-0008 with these conditions; whether to revise the native load target or the per-entity mapping; whether the libFuzzer licence (NCSA) may be added to the allowlist for coverage-guided fuzzing; the four advisory exceptions for crates that Loro depends on; and permission to file the upstream issues drafted here.

## 2. What was built

| Where | What |
|---|---|
| `crates/bayan-crdt` | The adapter (ADR-0008): Loro never appears in its public interface. Stories (rich text with marks and placeholder atoms), property maps, movable identifier lists, registries of entities, undo, snapshots, shallow snapshots, checkout, and **guarded import** of untrusted blobs (size, change and operation limits checked before decoding, nesting depth checked after, panics contained). |
| `crates/bayan-model` | A minimal Bayan Document Model: 128-bit identifiers, atoms and their bindings, mark families, the raw state, the pure normalization N1–N7 (plus N8 and N9), the invariant checker I1–I7, the operations of the brief, materialization, undo, and a seeded multi-replica simulation with shrinking. |
| `spikes/crdt-workload` | The shared, dependency-free workload: the synthetic 500-page document, the edit script, memory probes. |
| `spikes/crdt-model` | The driver: `generate`, `bench`, `converge`, `fuzz`, `supervise`, `replay`, `minimize`; `run-wasi.mjs` runs it as WebAssembly under Node.js. Raw results in `results/`. |
| `spikes/crdt-model-automerge` | The Automerge 3 comparison on the same workload. |
| `scripts/dev-setup.sh` | Installs the `wasm32-wasip1` target that the WebAssembly benchmarks need. |

**Operations** (all from the brief, plus moving a range, inserting objects and bookmarks): insert text, delete a range, format and clear formatting, split and merge paragraphs, set paragraph properties, insert a table, insert, delete and move rows, insert and delete columns, add a comment over a range, insert a field, insert an object, insert a bookmark, move a range (cut and paste). Each operation is one transaction: one commit, one undo step, one change that other replicas receive. Operations refuse what Word refuses (typing after the final paragraph end, a table in the middle of a paragraph, a table inside itself).

**Tests:** 16 adapter tests, 26 model tests (operations, undo, mark expansion), 4 normalization property tests, the convergence test (6 runs of 400 operations in CI), and unit tests; `cargo xtask verify` runs them all.

## 3. Results against the acceptance criteria

| | Criterion | Result |
|---|---|---|
| AC-1 | 100% of randomized runs converge to identical, invariant-satisfying views | **In progress.** The CI-sized test passes (6 runs of 400 operations). The full run (1,000 runs of 10,000 operations) has completed runs 0 to 274, all converged (`results/converge-1000x10000.log`), and continues on four threads; this line is updated when it finishes. |
| AC-2 | Normalization deterministic and idempotent | **Met.** Property test over randomly broken raw documents (every inconsistency concurrent editing can create, and more): 1,500 cases in CI and 100,000 in a release build (18 s, `results/normalization-100000.txt`), each case also normalized again from its own view's raw form and from the same content split into runs differently. The convergence runs check the same properties on every final view and every 1,000 operations. |
| AC-3 | Undo and mark-expansion tests pass | **Met**, with two Loro limitations pinned by tests (§7, F6 and F7) and worked around where it matters. |
| AC-4 | Performance against the targets, natively and in WebAssembly, every miss explained | **Reported** (§5). Misses: load and read (Loro bug F1; with the fix only native misses, by 1.8×), and the first 1,000-operation update after opening (62 ms native, 99 ms WebAssembly, target 50 ms), a one-time cost: later updates take 9 ms and 13 ms. Met: WebAssembly memory (141 MB after loading and reading, target 300 MB). |
| AC-5 | Fuzz target ≥ 1 CPU-hour without crashes, or crashes reported upstream with minimal reproductions and mitigated by limits | **Partly met.** 1.25 CPU-hours, 2.0 million inputs (§8.2): no panic crashed the engine (all 4,196 were contained by the adapter) and no import broke an invariant, but 11 inputs aborted the process through allocation failures, which no limit inside the process can prevent. Minimal reproductions are ready (most are a valid 90-byte update with one changed byte); reporting them upstream awaits the owner's decision (§10, question 5); the mitigation is containment and isolation (§8.3). |
| AC-6 | Every question of spec §18 answered | **Met** (§6). |

## 4. The synthetic document

Generated deterministically by `crdt-model generate` (byte-identical snapshot on every run on the same platform):

| | Full model | Subset (Automerge comparison) |
|---|---|---|
| Paragraphs | 15,000 in the main story (21,000 with cell and comment paragraphs) | 15,000 |
| Characters (atoms) in the main story | 1,528,987 | 1,521,287 |
| Tables | 200 (5 rows × 4 cells, 4,000 cell stories) | — |
| Comments | 2,000 (each with a reference atom and its own story) | 2,000 (marks only) |
| Fields, objects, bookmarks | 500, 100, 200 | — |
| Formatting | bold on part of every 10th paragraph, italic on part of every 7th, a heading every 20th paragraph | bold and italic as in the full model |
| Operations in the history | 1,727,169 in 640 changes | 1,592,573 in 606 changes |
| Snapshot (state and full history) | 9,190,558 bytes (6.1 MB gzipped) | 5,009,149 bytes |
| Shallow snapshot (state, no old history) | 5,407,520 bytes | 2,699,660 bytes |

The subset has the same text (identical FNV-1a hash `2d2e9e2dc8af0e5b`) as the Automerge document.

## 5. Performance

"Load" is opening a snapshot; Loro decodes containers lazily, so a document is ready to show only after its first full read, and the targets apply to **load and read** together. Medians of three runs; native = x86-64 release build; WebAssembly = `wasm32-wasip1` under Node.js 24 (V8), which runs the same code a browser worker would. "With fix" is Loro 1.16.2 with the three-line fix of F1 applied (measured only; the repository uses unmodified Loro).

### 5.1 Full model, 500 pages

| Measurement | Target | Native | Native, with fix | WebAssembly | WebAssembly, with fix |
|---|---|---|---|---|---|
| Load (open the snapshot) | | 27 ms | 28 ms | 54 ms | 52 ms |
| First read (raw state + normalization) | | 2,619 ms | 498 ms | 6,975 ms | 723 ms |
| **Load and read** | ≤ 300 ms native, ≤ 1 s WebAssembly | **2,655 ms** ✗ | **527 ms** ✗ | **7,029 ms** ✗ | **773 ms** ✓ |
| Memory after load and read (one replica) | ≤ 300 MB WebAssembly | +176 MB resident | +176 MB | **141 MB** ✓ | 141 MB ✓ |
| 1,000 local edits (typing, deleting, bold, splits) | | 259 ms | 261 ms | 413 ms | 430 ms |
| Update carrying those 1,000 edits | | 18,640 bytes | | | |
| Average update when each edit is sent alone | | 105 bytes | | | |
| **Apply the 1,000-edit update, first after opening** | ≤ 50 ms | **62 ms** ✗ | 63 ms ✗ | **99 ms** ✗ | 99 ms ✗ |
| **Apply later 1,000-edit updates** (average of 3) | ≤ 50 ms | **8.7 ms** ✓ | 9.4 ms ✓ | **12.8 ms** ✓ | 12.3 ms ✓ |
| Read after the update | | 387 ms | 381 ms | 460 ms | 478 ms |
| One undo step (average of 20) | | 205 ms | 207 ms | 255 ms | 264 ms |
| One concurrent keystroke, imported | | 0.2 ms | 0.2 ms | 0.3 ms | 0.3 ms |
| Apply 1,000 concurrent edits (the receiver typed meanwhile) | | 132 ms | 124 ms | 171 ms | 172 ms |
| Snapshot after the edits | | 9.24 MB (export 94 ms) | | 9.21 MB | |
| Shallow snapshot after the edits | | 5.44 MB | | 5.42 MB | |
| Whole benchmark, two replicas | | 477 MB peak resident | | 400 MB linear memory | |

The WebAssembly module of the benchmark (Loro, the model and the driver) is 4.7 MB uncompressed and compiles in 11 ms.

### 5.2 Why the targets are missed

**Load (F1, a Loro bug).** 92% of the first read is spent in Loro's rich-text snapshot decoder, which, for every span of text, counts the characters of the *entire remaining text* to validate the span's length (`loro-internal` `src/state/richtext_state.rs`, `decode_snapshot_fast`). That is quadratic in the length of a story: about 2·10¹⁰ character visits for the 1.5-million-character main story. Counting once and subtracting each span's length (three lines, §7 F1) removes it. Releases 1.13.9 through 1.16.2 all contain the check, so no available version avoids it.

**Load, after the fix (our mapping).** The remaining 498 ms of the native first read split as follows (instruction counts, callgrind): turning 6,001 rich-text stories into runs 31%, reading the property maps of about 27,000 entities 28% (of which recognizing Loro's mergeable-container markers in every value, a parse and CRC-24 per value, is 17%, and fetching each child container from the snapshot's key-value store 11%), memory allocation 25%, normalization the rest. One Loro container per paragraph is therefore expensive to read. Three remedies, best first: read entity properties lazily, when layout first needs a paragraph, instead of all at load (layout of a 500-page document needs only the first pages to show something); store properties in a few flat maps keyed `<id>/<property>` instead of one container per entity, which keeps per-property merging; ask upstream to make the marker check cheaper. The first is a design rule for CORE-101; the second would amend the mapping of spec §16 and should be measured first.

**Applying 1,000 operations.** The miss is a one-time cost of the first update after opening a document. Of its 62 ms, 52% is decoding the state of the containers the update touches, which the lazy load had not decoded yet (mostly the root map of 21,000 paragraph entities, including sorting its keys), and 45% is loading the part of the history that Loro needs to compute version vectors (reading and LZ4-decompressing history blocks of the snapshot). Later updates of the same size take 8.7 ms natively and 12.8 ms in WebAssembly (`apply_later_update_ms_avg`), well within the target. An engine can pay the first cost in the background right after opening, before the first update arrives.

**Undo.** One undo step takes about 0.2 s in the 500-page document (0.25 s in WebAssembly), whatever the size of the step, against 3 ms in a 10-page document. Loro computes the inverse of a change by checking out the versions around it, and for rich text that rebuilds the story's CRDT state by replaying every operation of the story from the beginning (98% of the time): about 1.5 million operations for the main story. Undo therefore costs time in proportion to the story's whole history. The remedy is upstream (keep the rebuilt state between undo steps, or rebuild only what follows the change being undone); trimming the history with a shallow snapshot does not help in Loro 1.16.2, it makes undo slower (F11). Until then, an undo in a long document is noticeable but usable.

**Concurrent updates.** A single concurrent keystroke is imported in 0.2 ms natively and 0.3 ms in WebAssembly. Applying 1,000 edits made concurrently with the receiver's own typing costs 132 ms natively and 171 ms in WebAssembly, and 93% of that is one-time work: the first concurrent change to a map makes Loro build its history cache for maps by decoding every change of the document's history (640 change blocks, 1.7 million operations). The cache is kept, so the cost is paid once per opened document, in proportion to the history's length. Opening from a shallow snapshot, whose old history is trimmed, does not reduce it in Loro 1.16.2: the first concurrent update then takes 569 ms (F11).

**A lesson for the engine:** an import into a document that has a subscriber (an undo manager, or the engine's own change listener) makes Loro compute the whole imported state as one change event. Opening the 500-page snapshot with the undo manager already attached took 2.7 s instead of 26 ms (before the read). The adapter therefore loads first and subscribes afterwards; the engine must do the same.

### 5.3 Subset, and the Automerge comparison

| Measurement | Target | Loro native | Loro native, with fix | Loro WebAssembly | Loro WebAssembly, with fix | Automerge native | Automerge WebAssembly |
|---|---|---|---|---|---|---|---|
| Load and read | ≤ 300 ms / ≤ 1 s | 2,061 ms ✗ | **229 ms** ✓ | 6,026 ms ✗ | **319 ms** ✓ | 1,473 ms ✗ | 2,627 ms ✗ |
| Memory after load and read | ≤ 300 MB | +77 MB | +77 MB | 66 MB ✓ | 66 MB ✓ | +400 MB | 330 MB ✗ |
| 1,000 local edits | | 36 ms | 39 ms | 69 ms | 77 ms | 59 ms | 125 ms |
| Update with the 1,000 edits | | 17,559 bytes | | | | 15,024 bytes as one bundle; 135,241 bytes as 1,000 changes | |
| Average update per edit | | 104 bytes | | | | 135 bytes | |
| Apply the 1,000-edit update, first after opening | ≤ 50 ms | 38 ms ✓ | 40 ms ✓ | 68 ms ✗ | 71 ms ✗ | 1,936 ms ✗ | 2,204 ms ✗ |
| Apply later 1,000-edit updates | ≤ 50 ms | 4.1 ms ✓ | 4.4 ms ✓ | 7.1 ms ✓ | 6.8 ms ✓ | | |
| Apply 1,000 concurrent edits | | 5.9 ms | 6.0 ms | 10.0 ms | 9.3 ms | | |
| One undo step | | 155 ms | 154 ms | 196 ms | 201 ms | (no undo manager) | |
| Snapshot | | 5.0 MB (shallow 2.7 MB) | | | | **0.93 MB** | |
| Peak memory, whole benchmark | | 228 MB | | 201 MB | | 1,063 MB | 909 MB |

In the subset, Loro's receiving replica has no undo manager, so no subscriber asks for change events; that is one reason why applying updates is cheaper than in the full model. Automerge was measured by a time-boxed sub-task on the same workload (`spikes/crdt-model-automerge/results/`): its load is not lazy and decodes everything (1.3 s), its memory is five times Loro's, and every applied update pays a fixed 0.37 s (0.53 s in WebAssembly) before any change is applied, so collaboration latency would be unacceptable. Its snapshots are five times smaller, because it compresses with DEFLATE and stores the text once, where Loro stores the history and the current state separately and compresses only lightly (gzip shrinks a Loro snapshot by a third). The final texts differ between the two libraries although their lengths agree, because Automerge marks paragraphs at their start and the BayanDocs model at their end, so the same positions land on different characters; within each library both replicas agree.

### 5.4 Determinism

Generating the synthetic document twice gives byte-identical snapshots on the same platform. Across platforms, the **state** is identical (same view, same raw state, same version) but the **snapshot bytes differ**: the 10-page document is 185,188 bytes natively and 184,066 bytes in WebAssembly, and re-exporting the WebAssembly snapshot natively gives a third size, 183,678 bytes. Loro's encoding depends on how a document reached its state. Nothing in ADR-0005 needs canonical snapshot bytes, but nothing may assume them either: content addressing, deduplication or signatures must hash the view or a canonical export, never a Loro snapshot.

## 6. Answers to spec §18

**Q1. Per-key expansion for dynamically named keys such as `cmt:<id>`?** Yes. Loro resolves a mark key's expansion from the part before its first colon, so the family `cmt` configured with no expansion governs every `cmt:<id>` key, however many comments exist (tested with 200 overlapping comments at every boundary). Two consequences for the spec: Loro refuses (panics on) a configured name that contains a colon, and a key's family is whatever precedes its first colon. The spec's keys must therefore use the colon only as the family separator: `r:b`, `r:rFonts.ascii`, `r:w14.ligatures`, `rev:ins`, not `r.b` or `r.w14:ligatures` (whose family would be `r.w14`, unconfigured).

**Q2. Is the placeholder-plus-mark binding robust under concurrent deletion and re-insertion?** Yes. The binding is a mark `atom` = `"<kind code>:<32 hexadecimal digits>"` with no expansion on exactly one placeholder character. Typing next to an atom never extends the binding; undoing a delete restores every atom with its binding; a cut and paste racing with formatting, deletion, splits and other moves converges in all simulation runs. Races can leave two atoms bound to the same entity (two replicas move the same object): normalization rule N7 keeps the first in document order. Moving a table into one of its own cells is refused locally, but two concurrent moves (each replica moves one table into a cell of the other) nest the tables in each other. Normalization, which applies N7 in the order it traverses stories, never loops on such a cycle: both tables become unreachable from the main story, so their content is retained but invisible (N5) until an edit or undo reattaches them. That is safe, but surprising for users; CORE-101 should decide whether to reattach orphaned tables automatically. Run-property marks must not touch structural atoms (paragraph ends, range delimiters, table blocks), because a mark that ends on a paragraph end and expands after would spread into the next paragraph; the view ignores them there, and paragraph-mark formatting is stored in the paragraph's properties (`rPr.*`) instead of as marks on the paragraph end, which amends spec §5.

**Q3. Memory and load time of many small maps (one per paragraph) for a 500-page document in WebAssembly?** 141 MB of linear memory after loading and reading the full model, within the 300 MB target; 0.77 s to load and read with the F1 fix (7.0 s without). The per-entity maps cost about 28% of the first read (§5.2): acceptable with lazy reading, and replaceable by flat maps if measurements in CORE-101 demand it.

**Q4. Movable lists or a movable tree for table rows and nested structures?** Movable lists suffice. Rows are a movable list per table and cells a movable list per row; concurrent row insertions, deletions and moves merge (a row moved concurrently by two replicas exists once, at the position of one of the moves), and ragged rows are valid. Nesting is expressed by stories (a cell's story contains a table block atom), not by a tree of containers, so no tree is needed. The one hazard a movable tree would prevent, cycles, comes from moving table atoms; it is handled by refusing local cycles and by a normalization that cannot loop on concurrent ones (Q2). Two safeguards are needed: tables nested deeper than 32 levels are dropped from the view (new rule N9), which bounds the recursion of normalization through nested tables so that hostile documents cannot exhaust the stack; and N7's check that an entity was already reached serves as the visited set that stops cycles.

**Q5. How does undo interact with materialization?** Cleanly, if materialization is not an undo step. When an edit touches structure that only normalization shows (a virtual final paragraph end, a virtual paragraph end before a table found mid-paragraph), the edit first writes that structure with the identifiers and properties the view already shows, commits it with an origin the undo manager ignores, and then performs the edit as its own undo step. Undo then returns the view exactly to the state before the edit, and concurrent materializations by several replicas merge into the same entities, because the identifiers are derived deterministically. Entity creation follows the same pattern: Loro 1.16.2 duplicates the content of a child container when redoing its creation (F6), so entities are created outside undo and undo removes only their reference (normalization rule N5 keeps unreferenced entities invisible).

**Q6. Snapshot and update sizes for typical editing sessions?** A single edit sent on its own costs about 105 bytes; a session of 1,000 edits sent as one update about 18.6 KB. The 500-page full model is 9.2 MB with its whole history and 5.4 MB as a shallow snapshot (state with only the latest history), and grows by 48 KB with 1,000 edits. gzip saves only a third; Automerge's snapshot of the subset is five times smaller than Loro's. Replicas should exchange updates, not snapshots. Shallow snapshots are 40% smaller, but in Loro 1.16.2 a document opened from one pays more for its first update, its undo steps and its first concurrent update (F11), so the storage and sync work packages must measure that trade-off before choosing the at-rest format.

**Q7. Resource limits needed when importing untrusted updates?** Limits are necessary but not sufficient. The adapter's `ImportLimits::UPDATE` refuses blobs over 1 MiB, over 100,000 changes or over 2 million operations before decoding, refuses snapshots where only updates are expected, and inspects every imported value for nesting deeper than 16 levels. Fuzzing (§8) showed what limits cannot prevent: Loro panics on crafted updates whose checksum is valid (the checksum is not a signature; anyone can recompute it), it requests absurd allocations while decoding some crafted headers (an allocation failure aborts the process), and values nested about 20,000 levels deep overflow a 2 MiB stack when freed or decoded. The adapter contains panics (the replica is poisoned, refuses further use and is never freed) and refuses deep values; allocation failures need process or worker isolation and an upstream fix (§8.3).

## 7. Findings in Loro (upstream issue drafts)

None of these has been reported upstream yet: filing issues on another project is the owner's decision (§10). **The minimized reproductions of F2 and F4 are deliberately not in this repository:** they crash any application that uses Loro 1.16.2, so they should reach Loro's maintainers privately first (coordinated disclosure). The owner received them as a separate bundle, and anyone can regenerate them with `crdt-model supervise` and `crdt-model minimize` from the fixed seeds. The one exception is the 88-byte update that the adapter's regression test needs (`crates/bayan-crdt/tests/fixtures/`): a single panic, of a kind that ADR-0008 already notes Loro's recent releases keep fixing ("crashes on malformed imports").

| | Finding | Severity for BayanDocs | Workaround here |
|---|---|---|---|
| F1 | Rich-text snapshot decoding is quadratic in the text's length (a validation that counts the remaining characters for every span). | High: loading and reading a 500-page document takes 2.7 s natively, 7.0 s in WebAssembly. | None possible outside Loro; measured with a patched copy. |
| F2 | Importing a crafted update (usually one changed byte, with the checksum recomputed) panics, at 22 source locations (§8.2). | High: any collaborator, or a crafted file, can crash every replica that imports it. | Panics contained in `Doc::import`; the replica is poisoned. |
| F3 | Freeing a document after a panic panics again on a poisoned lock (`loro-internal` `src/sync.rs:34`), and a second panic during unwinding aborts the process. | High: turns every F2 panic into a process abort. | Poisoned documents and their undo managers are leaked. |
| F4 | Decoding some crafted change headers requests absurd allocations in `serde_columnar`'s run-length decoder (193 GB for a valid 94-byte update with 8 bytes changed); the allocation failure aborts the process. It happens while reading the header, before anything is applied, even in `LoroDoc::decode_import_blob_meta`. | High: cannot be caught. | None; needs isolation (§8.3). |
| F5 | Values nested about 20,000 levels deep (60 KB of update) overflow a 2 MiB stack when freed, read deeply or, for mark values, decoded. | Medium: an 8 MiB stack survives 50,000 levels. | Depth inspection after import; the replica is poisoned and leaked. Mark values need an upstream fix. |
| F6 | Redoing the creation of a mergeable child container duplicates its content, including other replicas' edits. | Medium: corrupts content on redo. | Entities are created outside undo. |
| F7 | Undoing a mark leaves it on text another replica inserted inside the marked range in the meantime. | Low: formatting survives where a user expects it undone. | Documented by a test; acceptable for now. |
| F8 | Decoding errors are printed to standard error ("Column Deserialize Error: …") by the library. | Low. | None needed in the engine (no standard error in WebAssembly); noted. |
| F9 | Reading a value checks it for the mergeable-container marker (parse and CRC-24) every time. | Low: 17% of the first read. | None. |
| F10 | An import into a document with a subscriber computes the whole state as an event (2.7 s for 500 pages); not a bug, but undocumented. | Medium. | Load before subscribing. |
| F11 | A document opened from a shallow snapshot (old history trimmed) is slower than one opened from a full snapshot: with the F1 fix, the first update after opening takes 274 ms instead of 63 ms, an undo step 427 ms instead of 207 ms, and the first concurrent update 569 ms instead of 124 ms; without the fix the first update takes 2.3 s, because the trimmed history's starting state goes through the quadratic decoder too. With the fix, 86% of the first update is seeding Loro's rich-text CRDT state from that starting state, chunk by chunk with a sort (`seed_tracker_from_shallow_root`), which costs more than replaying the full history (`results/profile-shallow-first-update.txt`). | Medium: ADR-0008 trims history with shallow snapshots. | None yet; the trade-off must be measured before choosing an at-rest format. |

**F1, proposed fix** (`loro-internal` 1.16.2, `src/state/richtext_state.rs`, in `decode_snapshot_fast`): count the characters once before the loop (`let mut remaining_chars = string.chars().count();`), compare each span's length with `remaining_chars` instead of `s.as_str().chars().count()`, and subtract the span's length after splitting it off. Measured effect on the 500-page document: reading the raw state 2,511 ms → 386 ms natively, 6,795 ms → 529 ms in WebAssembly.

**F2 and F4, reproductions:** each is a valid update for a small base document with one or a few bytes changed and the checksum recomputed (§8.2). A reproduction needs only Loro: import the base snapshot into a new `LoroDoc`, then the crafted update (for F4, decoding the update's header is enough).

## 8. Security: importing untrusted blobs

### 8.1 How the fuzzer works

`crdt-model fuzz` is a structure-aware mutation fuzzer that needs no third-party code (cargo-fuzz would need libFuzzer, whose NCSA licence is not on the ADR-0017 allowlist; §10). It builds 16 small base documents by random editing, exports their snapshots and the updates two replicas made concurrently after them, and mutates those blobs: bit flips, interesting byte values, inserted, deleted, duplicated and spliced ranges, truncation, rewritten integers and header modes. Nine inputs in ten get a recomputed checksum, so they reach the decoders. Each input is imported, through the adapter with `ImportLimits::UPDATE` (or snapshot limits), into a freshly loaded copy of its base; a successful import must still normalize into a valid view. Panics are recorded with their source location; inputs are written to disk before they are imported, so even an input that aborts the process is kept. `crdt-model supervise` runs the fuzzer in child processes and restarts a child that aborts; `crdt-model minimize` shrinks a finding by setting changed bytes back to their original value (most findings shrink to one or two changed bytes in a valid 90-byte update).

### 8.2 Results

**The main run** (`crdt-model supervise --seconds 4500 --workers 1`, `results/fuzz-supervise-4500s.log`): 2,019,898 inputs in 4,500 s, that is 1.25 CPU-hours at about 450 inputs per second, in 12 rounds (the fuzzer was restarted after each of 11 aborts).

| Outcome | Inputs | What it means |
|---|---|---|
| Refused | 1,872,861 | The checksum, the limits or Loro's decoders rejected the blob with an error: the expected outcome for most mutations. |
| Imported | 142,841 | Loro accepted the blob; every resulting document still normalized into a valid view (no invariant failure). |
| Panicked | 4,196 | At 19 source locations, all during the import and all contained by the adapter (the replica was poisoned and leaked). |
| Aborted the process | 11 | Allocation failures while decoding a crafted header (F4). |

The panic locations of the main run, by number of inputs: `generic-btree` 0.10.7 `src/lib.rs:618` (2,115), `loro-internal` 1.16.2 `richtext_state.rs:2277` (610), `movable_list_state.rs:1056` (578), `richtext_state.rs:1718` (304), `tracker/crdt_rope.rs:106` (283), `movable_list_state.rs:1053` (202), `diff_calc.rs:1630` (45), `diff_calc.rs:1071` (44), `oplog/loro_dag.rs:445` (7), `movable_list_state.rs:1210` (6), `diff_calc.rs:1296` (3), `diff_calc.rs:2183` (3), `serde_columnar` 0.3.14 `strategy/rle.rs:637` (3), `loro-common` 1.16.0 `src/lib.rs:1001` (2), `history_cache.rs:980` (2), `history_cache.rs:921` (1), `loro-delta` 1.13.0 `delta_rope/compose.rs:136` (1), `rle.rs:564` (1) and `rle.rs:575` (1). The messages are failed assertions, `unwrap()` on a missing value, out-of-range positions and "entered unreachable code": internal consistency checks that crafted operations violate.

**Short runs on tiny documents** (`--small`, 4.5 minutes in total), made to obtain short reproductions, found three more locations (`change.rs:244`, `diff_calc.rs:1069`, `rle.rs:632`): 22 in all, and the first allocation failure within a minute. Minimized, 10 of the 12 panic locations of those runs need only one changed byte in a valid update of 88 to 371 bytes, one needs two and one a longer change, and the allocation failure needs 8 changed bytes in a 94-byte update.

**Slow inputs.** The slowest accepted input took 5.4 s to import, within the limits. Imports must therefore never run on a thread that the user interface waits for (the engine runs in a worker), and the sync layer should stop accepting updates from a peer whose updates fail or take too long.

### 8.3 What the engine must do

- **Contain panics where the document is known.** `Doc::import` runs Loro's import under `catch_unwind`; on a panic the replica is poisoned: it refuses further imports, exports and edits, and it is never freed (freeing it can panic again, F3). The host discards it and reloads the last good state without the offending update. This works where panics unwind (native builds). In the browser, Rust panics abort the WebAssembly instance; the engine already runs in a Web Worker (ADR-0014), so the web shell must restart the worker and reload, and must not re-import the offending update.
- **Isolate imports from untrusted peers.** Allocation failures (F4) and stack overflows abort the process in every build. On the web, the worker is the isolation boundary. On desktop, the engine runs inside the application process, so an abort loses the user's unsaved work. Options, best first: decode and apply untrusted updates first in a separate import process or worker that holds a copy of the document state, and forward them to the main replica only after it succeeded; or save continuously so that an abort loses nothing; or wait for upstream fixes. This is a decision for the engine and sync work packages (CORE-007, and the sync work packages of phase 1), recorded here as a follow-up.
- **Never pass panic messages on.** Loro's panic messages contain document text (a fuzzer finding printed the characters it was processing). The engine's panic hook must record only that a panic happened, never its message, in logs, crash reports or anything sent to a server (docs AGENTS.md §6).
- **Keep fuzzing.** The fuzzer found the first panic within seconds and the first allocation failure within minutes. Coverage-guided fuzzing (cargo-fuzz) would find more; it needs the libFuzzer licence decision (§10).
- **Limits stay.** The size, change, operation and depth limits refuse cheaply what is obviously too big; they are the first fence, not the last.

## 9. Changes this spike proposes to the spec and the ADRs

The docs pull request (CORE-004, `BayanDocs/docs`) proposes:

- **§5 Marks:** keys are `family:path`, with the colon only as the family separator (Q1); paragraph-mark formatting is stored in the paragraph's properties as `rPr.<property>`, not as marks on the paragraph end (Q2).
- **§14 Invariants and normalization:** N8, the final section gets default properties (A4, 2.54 cm margins) when it has none, which I7 needs; N9, tables nested deeper than 32 levels are dropped from the view; N7 is applied in traversal order, so a table reached again inside its own cells is dropped there and cycles cannot make normalization loop; N3 applies per story (a range end in another story than its start is unmatched); range delimiters at block positions are inline atoms and do not make a position a block position.
- **§16 CRDT mapping, as validated:** stories are mergeable child texts of a root `stories` map (the main story a root text); registries are root maps of mergeable child maps; rows and cells are movable lists; the atom binding is a string `"<kind code>:<id>"`; entity creation and materialization are committed outside undo; a snapshot is imported before anything subscribes to the document.
- **§18:** the answers of §6, with a link to this report.
- **ADR-0007 and ADR-0008:** a dated validation section each. ADR-0007: validated. ADR-0008: criteria partly met (load time), with the recommendation of §1 recorded for the owner's decision; it becomes binding only when accepted as an amendment.

## 10. Questions for the owner

1. **ADR-0008 with conditions.** Do you accept Loro with the three conditions of §1 (the F1 fix upstream before the model work packages measure load again; engine isolation for untrusted imports; the performance items in CORE-101)? My recommendation: yes. Automerge is worse in every measurement except snapshot size, and every Loro problem found has a workaround or a small fix.
2. **The native load target.** With the F1 fix, the full model loads and reads in 527 ms natively (target 300 ms) and 773 ms in WebAssembly (target 1 s). Options: (a) keep the target and require lazy reading of entity properties in CORE-101 (recommended; showing the first page needs only the first pages); (b) revise the native target to 600 ms; (c) change the mapping to flat property maps now, before measuring (a).
3. **libFuzzer licence.** cargo-fuzz depends on `libfuzzer-sys`, whose licence is `(MIT OR Apache-2.0) AND NCSA`; NCSA (a permissive licence, close to MIT and BSD) is not on the ADR-0017 allowlist. Options: (a) allow NCSA for fuzzing tools only, which do not ship (recommended); (b) keep the dependency-free mutation fuzzer of this spike.
4. **Unmaintained crates.** Loro depends on `im`, `bitmaps` and `sized-chunks`, flagged unmaintained by RustSec advisories RUSTSEC-2026-0248, RUSTSEC-2026-0247 and RUSTSEC-2026-0251 (no known vulnerability), and on `atomic-polyfill` (RUSTSEC-2023-0089), which is never compiled for our targets. `deny.toml` ignores these four advisories with reasons. Do you accept these exceptions until Loro replaces them? Recommendation: yes, and ask upstream.
5. **Upstream reports.** May I (or will you) file the issues drafted in §7 with the Loro project? F1 should be filed with its fix, and F6, F7, F9 and F11 as ordinary issues; F2 to F5 as one security report through Loro's private channel if it has one, because they let one collaborator crash the others.

## 11. Follow-ups

- **CORE-101 (model v1):** lazy reading of entity properties; measure flat property maps against per-entity maps; incremental normalization (the spike normalizes whole documents); undo latency (§5.2); the spec amendments of §9.
- **Engine and sync:** import isolation and restart strategy (§8.3); a panic hook that never records messages; load before subscribing.
- **Fuzzing:** a continuous fuzzing job (nightly CI or OSS-Fuzz) once the libFuzzer question is settled; the fuzzer of this spike can run in CI as it is.
- **Packaging (bayan-wasm):** Loro's WebAssembly build enables `js-sys`'s default `unsafe-eval` feature through `getrandom` 0.2. The generated JavaScript must not use `eval` or `new Function` (the content security policy forbids them); check the bindings when the engine is packaged.
- **Storage and sync:** measure full against shallow snapshots as the at-rest format (F11), with the first update, undo and the first concurrent update after opening.
- **Dependency session:** move to the Loro release that fixes F1 and, if it does, re-run `crdt-model bench`; review the advisory exceptions.

## 12. How to reproduce

All commands from the repository root, after `scripts/dev-setup.sh`. Seeds are fixed, so every run repeats exactly.

```sh
# Tests, including the convergence test at CI size and the normalization property test
cargo xtask verify
BAYAN_NORMALIZATION_CASES=100000 cargo test --release -p bayan-model --test normalization
BAYAN_CONVERGENCE_RUNS=20 BAYAN_CONVERGENCE_OPERATIONS=2000 cargo test --release -p bayan-model --test convergence

# The synthetic documents and the native benchmarks
cargo build --release -p crdt-model
target/release/crdt-model generate --mode full --out /tmp/full-500.bin
target/release/crdt-model generate --mode subset --out /tmp/subset-500.bin
target/release/crdt-model bench --mode full --snapshot /tmp/full-500.bin
target/release/crdt-model bench --mode subset --snapshot /tmp/subset-500.bin

# The same benchmarks in WebAssembly under Node.js (the snapshot folder appears as /data)
cargo build --release -p crdt-model --target wasm32-wasip1
node spikes/crdt-model/run-wasi.mjs target/wasm32-wasip1/release/crdt-model.wasm /tmp bench --mode full --snapshot /data/full-500.bin

# The full convergence run of the brief (about 6 hours on 3 cores of the machine above)
target/release/crdt-model converge --runs 1000 --operations 10000 --threads 3

# Fuzzing: 75 minutes in child processes that are restarted after an abort; then minimize a finding
target/release/crdt-model supervise --seconds 4500 --workers 1 --out /tmp/fuzz
target/release/crdt-model minimize --input /tmp/fuzz/<finding> [--isolated]

# The Automerge comparison
cargo build --release -p crdt-model-automerge
target/release/crdt-model-automerge generate --out /tmp/am-500.bin
target/release/crdt-model-automerge bench --snapshot /tmp/am-500.bin
```

The raw output of every measurement in this report is in `spikes/crdt-model/results/` and `spikes/crdt-model-automerge/results/`.
