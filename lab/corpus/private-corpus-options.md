# Private corpus (T2): options for the owner

**Status: for the owner's decision** (LAB-001 AC-4; owner checklist item 10). Nothing described here exists yet: setting up the private corpus is part of LAB-101, once the owner has decided the questions below and created the accounts.

## What the private corpus is for

The public corpus (T1) can only hold documents with redistributable licenses, and those are mostly test files of software projects: they exercise features more than everyday documents do, and they come from few authors and few languages. The private corpus (T2) is a large sample of **real-world documents found on the open web**, used to measure fidelity on the documents people actually write ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers): 10,000 or more documents by the end of Phase 1, private storage chosen by the owner, aggregate metrics only, never redistributed; weekly runs, ADR-0025). Its documents belong to third parties and may contain personal data or malware, so it is a liability as well as an asset: the threat model lists its leak as T19.

## Decisions needed

| # | Decision | Recommendation | Alternatives |
|---|---|---|---|
| 1 | How documents are collected | Sample `.docx` records from **Common Crawl**'s public archive | A crawler of our own; documents donated under an agreement (good later, for the torture tier) |
| 2 | Where they are stored | A **private Cloudflare R2 bucket**, separate from the public corpus's bucket and without any public domain, in the EU jurisdiction if EU data protection law applies to the owner; a working copy only on the reference machine's encrypted disk | Encrypted disk of the reference machine alone, with an encrypted offline backup; a private Git repository with Git LFS (not recommended) |
| 3 | Where the manifest lives | A **private repository** (for example `BayanDocs/lab-private`), owner-only | Only in the private bucket |
| 4 | Who has access | The owner; a **read-only** token on the reference machine; a write token only for the collection runs; no GitHub-hosted runners | |
| 5 | What leaves the private environment | **Aggregate numbers only**, and a name (font, language, program) only when at least 10 documents share it | |
| 6 | Legal review | The owner checks copyright and data-protection law for their jurisdiction **before** the first collection (see below) | |

The rest of this page explains each decision.

## 1. Collection: a Common Crawl sample (recommended)

[Common Crawl](https://commoncrawl.org/) publishes a monthly crawl of the web as WARC files in a public bucket, with an index in columnar (Parquet) form that records each capture's URL, detected content type, and where its record lies in the WARC files.

- **Selecting:** query the columnar index (with Amazon Athena or DuckDB) for records whose detected content type (`content_mime_detected`) is the `.docx` type, `application/vnd.openxmlformats-officedocument.wordprocessingml.document`, in one or a few recent crawls. Sample them stratified by top-level domain and language, so the private corpus reaches the scripts the public corpus lacks (Arabic, Hebrew, Chinese, Japanese, Korean, Indic scripts), and cap the documents per host so no single site dominates.
- **Fetching:** download only the sampled records, by byte range, from Common Crawl's bucket. Nobody else's server is contacted, the crawl already honoured `robots.txt`, and every document's origin is recorded exactly (crawl, WARC file, offset), which also makes deletion on request possible.
- **Filtering:** drop truncated records (Common Crawl cut payloads at 1 MiB before the March 2025 crawl, CC-MAIN-2025-13, and at 5 MiB since; truncated records carry a `WARC-Truncated` header since CC-MAIN-2019-47), then let `bayan-lab corpus add` refuse what it refuses (encrypted, damaged, macro-enabled and template files) and deduplicate by SHA-256. Older crawls' 1 MiB limit biases a sample towards small documents; prefer recent crawls.
- **Recording:** register each crawl as a source with license `NOASSERTION` (for example `common-crawl-2026-38`), and add documents with `--tier T2`, without `--root`: the tool then records no path or file name for them, and its messages name them only by their position on the command line and their SHA-256. The record locations needed for deletion requests are kept beside the manifest, privately, not in it.

**A crawler of our own** could target gaps precisely, but it means operating a crawler politely (robots.txt, rate limits, identification), handling more legal exposure, and more code to maintain. **Donated documents**, given under an agreement that allows private testing, are the cleanest legally and good for the torture tier (T3), but slow to gather.

## 2. Storage

- **Recommended:** a private R2 bucket (for example `bayandocs-corpus-private`). R2's S3 API always requires credentials, so a bucket without a custom domain or `r2.dev` address is not reachable anonymously. Use a separate bucket from the public corpus, ideally in a separate Cloudflare account, so that no public-domain setting on the public bucket can ever expose it. If the owner is in the EU, or the documents' data subjects are, create the bucket with R2's EU jurisdiction (chosen at creation, it cannot be changed later; its S3 endpoint is `ACCOUNT_ID.eu.r2.cloudflarestorage.com`). 10,000 documents of the size of the public corpus's take about 350 MB; even ten times that stays within R2's free tier. `bayan-lab` already works with it: `--store 's3://bayandocs-corpus-private/t2?endpoint=https://ACCOUNT_ID.eu.r2.cloudflarestorage.com&region=auto'`.
- The reference machine keeps a **working copy** on its encrypted disk (BitLocker), because Word must open the documents there for ground truth.
- **Alternative:** the reference machine's encrypted disk alone, with an encrypted offline backup. No cloud at all, but no second machine can run the weekly reports, and a lost disk loses the corpus.
- **Not recommended:** a private Git repository with Git LFS. Deleting one document, for example after an erasure request, means rewriting history and asking GitHub to purge the LFS object, and LFS storage and bandwidth quotas cost extra.

Encrypting the documents ourselves before upload (so that the provider cannot read them) would be possible with an established tool, but it is a cryptographic design decision that this work package does not make (AGENTS.md §6 and §8); if the owner wants it, it needs its own decision.

## 3. Manifest

The private corpus's manifest uses the same format as the public one ([manifest-schema.md](manifest-schema.md)), and the tools enforce its privacy rules: `NOASSERTION` license, no paths, nothing from the documents' content. It still lists a SHA-256 for each document, which someone holding a copy of a document could use to learn that the corpus contains it, so it belongs in a **private repository**, not in bayan-core. Reviewed pull requests there keep the history of the corpus as for the public one.

## 4. Access

- The owner holds the account. Named maintainers get access only if they need it.
- The reference machine gets an R2 token that can **only read** the private bucket. The collection runs use a separate write token, kept by whoever runs them, never stored in a repository or in CI logs.
- Weekly runs happen on private infrastructure (the reference machine, or a self-hosted runner the owner controls), not on GitHub-hosted runners, so no third-party machine holds the documents.
- The machines that open the documents treat them as hostile: macros and external content disabled, no personal data on the machine (Fidelity Lab specification §3, "Corpus hygiene"; owner checklist item 9).

## 5. What leaves the private environment

Only aggregate numbers: how many documents use each feature, script or compatibility mode, and the distributions of the fidelity metrics. Never a document, a SHA-256, a URL, a file name, a title or any content (the tools record none of these in the first place). A rare font, program or language name could point to a single document, so reports show a name only when at least **10 documents** share it, and fold the rest into "other". The `bayan-lab corpus stats` report does not do that folding yet; adding it is a follow-up for LAB-101, before the first private report.

## 6. Legal and privacy considerations

These need the owner's legal judgement, for the owner's jurisdiction and where the storage is; this page only lists what to check.

- **Copyright.** Keeping copies of whole documents for internal measurement, never redistributed and never shown, is a classic text-and-data-mining use. In the EU, the DSM Directive's Article 4 allows reproductions for text and data mining unless the rights holder reserved them in a machine-readable way; in the United States, such non-expressive use is usually argued as fair use; other jurisdictions differ (the United Kingdom's exception, for example, covers only non-commercial research). Common Crawl's terms of use apply to its data as well.
- **Personal data.** Documents from the web contain names, addresses, CVs and worse. Where the GDPR applies, the project needs a lawful basis (likely legitimate interests, with a written balancing test), a narrow purpose (fidelity measurement only), minimization (which the tools enforce: no content in manifests, logs or reports), security (private storage, least-privilege tokens, encrypted disks), a retention limit (for example, replace the sample yearly and delete the old one), and a way to honour erasure requests (the SHA-256 and the record location find a document). A short data-protection impact assessment would document all of this.
- **Malware.** Every document is handled as hostile, as above.

## Proposed next steps

1. The owner decides questions 1 to 6 (or accepts the recommendations) and does the legal check.
2. The owner creates the private bucket, the private repository and the two tokens.
3. LAB-101 builds the collection script, the name-folding rule for reports, and the weekly report, and collects the first 10,000 documents.
