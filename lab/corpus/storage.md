# Corpus storage

Corpus documents are never committed to Git; only manifests are ([manifest-schema.md](manifest-schema.md)). The documents, and the license and notice texts of their sources, live in a **store**: a content-addressed collection of files, where every object's name is the SHA-256 of its bytes.

```text
objects/ab/cdef….docx      a document: the first two hexadecimal digits of its SHA-256 name the folder, the other 62 the file
licenses/abcdef….txt       a license or notice text of a source
```

Because the name is derived from the content, the same bytes are stored once however often they occur, an object never changes once written, and anyone holding the manifest can check every object: `bayan-lab corpus verify --store …` reads each one and compares its SHA-256 with the manifest. A store cannot substitute a document without `verify` noticing, so a store needs to be trusted for availability, not for integrity.

## Backends

`--store ADDRESS` (or the environment variable `BAYAN_LAB_STORE`) chooses one of three backends, all behind the same interface (`lab/bayan-lab/src/store/`):

| Address | Backend | Reads | Writes | Lists |
|---|---|---|---|---|
| a folder, such as `~/bayandocs-corpus/store` | local directory | yes | yes | yes |
| `s3://BUCKET/PREFIX?endpoint=https://HOST&region=REGION` | any S3-compatible object storage (Amazon S3, Cloudflare R2, Backblaze B2, MinIO…), with path-style requests | yes | yes | yes |
| `https://HOST/PATH` | a read-only store at a plain HTTPS address, such as a public bucket's own domain | yes | no | no |

- The **local directory** backend writes each object to a temporary name and renames it, so an interrupted write leaves no half object behind.
- The **S3** backend signs its requests with AWS Signature Version 4 when `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` (and, for temporary credentials, `AWS_SESSION_TOKEN`) are set, and sends them unsigned otherwise. Credentials are read only from those variables and are handed to curl on its standard input, never on a command line where other users of the machine could see them; they never appear in messages. The region defaults to `us-east-1`; Cloudflare R2 uses `auto`.
- The **HTTPS** backend never sends credentials. It reads objects at `ADDRESS/objects/ab/cdef….docx` and `ADDRESS/licenses/<sha256>.txt`. Since it cannot list, `corpus verify` with it checks every object of the manifest but cannot find objects the manifest does not know; it says so.

Both network backends use the `curl` program, which every supported platform has (Windows 10 and later include it), so the lab needs no HTTP, TLS or cryptography code of its own. They need curl 7.63 or later, and signed S3 requests need curl 7.87 or later, which signs the SHA-256 of an upload (so a store refuses a file that changed after it was hashed); the tool checks the version before its first request and says what it found. curl reads no personal configuration file, may use only HTTPS (plain HTTP only to this machine, for tests and local servers), never follows a redirect and expands no URL patterns. Every request has time limits: ten minutes in all, 30 seconds to connect, and no more than a minute below 1 KiB per second. Responses come back through a pipe, never through a temporary file, and are cut off at their size limit whatever curl's version; an upload is read by curl from the file the document was read from, so no copy of a document is written anywhere. A request is tried again from the start, up to three times, only when the failure may pass: no connection (curl exit codes 5, 6 and 7), a timeout (28), a failed TLS handshake (35), a broken transfer (18, 52, 55, 56 and 92), or HTTP status 408, 429 or 5xx. A response larger than its limit is not tried again. A store address with any other scheme than `s3://`, `https://` or `http://` (such as a mistyped `S3://`) is refused rather than taken for a folder.

## Where the public corpus lives

The public corpus v1 is 1,259 documents and 24.6 MB, and the Fidelity Lab specification's target for the end of Phase 1 is 2,000 public documents: well under 1 GB for years. Nightly CI runs and contributors will download it, so what matters is anonymous public reads without per-download fees, writes limited to the curator, and as little to administer as possible.

**Decided by the owner on 2026-10-10 (review of LAB-001): a Cloudflare R2 bucket with the custom domain `corpus.bayandocs.org`.** The project's domain, `bayandocs.org`, already uses Cloudflare (for e-mail routing), so the subdomain is one setting away. The owner creates the account and the bucket (owner checklist, item 10 covers the private corpus; this one is its public counterpart):

- Create a bucket, for example `bayandocs-corpus`, and keep the public corpus under the prefix `public/v1/`. Store nothing else in it: everything in a bucket with a public domain is public, so the private corpus (T2) must never go into this bucket or this account's public domains.
- Connect the custom domain `corpus.bayandocs.org` to the bucket (R2 → the bucket → Settings → Custom Domains). Do not enable the `r2.dev` address: Cloudflare intends it for development only and rate-limits it.
- Create an R2 API token with *Object Read & Write* permission **for this bucket only**, for the curator who publishes; keep it out of the repository and out of CI. R2's S3 API always requires credentials (it never serves anonymous requests), so this token is the only way to write.
- Readers then use `--store https://corpus.bayandocs.org/public/v1`, with no account and no credentials.
- Cost: R2 charges nothing for egress, and its free tier (10 GB-month of storage, one million write and ten million read operations a month, at the time of writing) covers this corpus many times over.

**Alternatives considered:**

| Option | Public reads | Writes | Notes |
|---|---|---|---|
| Backblaze B2 public bucket | at the bucket's download address (`https://f00N.backblazeb2.com/file/BUCKET/…`), with the HTTPS backend | S3 API with an application key limited to the bucket | low storage price; egress free only up to a monthly allowance; a new account and vendor |
| Amazon S3 bucket with a public-read bucket policy | anonymously through the S3 API itself (the `s3://` address works unsigned), or at its HTTPS address | S3 API with an IAM user or role limited to the bucket | the most configuration (Block Public Access must be relaxed for this bucket); egress is billed beyond the free allowance |
| A release asset on GitHub (a `.tar.zst` of the store) | download and unpack, then use the local directory | the owner uploads an archive per corpus version | no new account, and immutable releases (owner checklist, item 6) protect it; but no object-by-object access, and every update is a manual release |

## Publishing the public corpus

Once the bucket exists, the curator fills it from the sources, without changing the manifest. Everything that is uploaded is checked first in a local store built from the same pinned sources: the manifest pins every document's SHA-256, so the local store and the bucket hold the same bytes.

1. Build the store locally: `lab/corpus/public/import.sh ~/bayandocs-corpus/sources ~/bayandocs-corpus/store`, and check that the manifest is unchanged (`git status lab/corpus/public` shows nothing).
2. **Scan the local store for malware** with a scanner whose signatures were updated the same day, for example ClamAV: `freshclam`, then `clamscan --recursive --infected --scan-archive=yes --max-filesize=600M --max-scansize=600M ~/bayandocs-corpus/store`. Publish nothing if it reports anything: find the document and keep it out with the exclusion list. Also run the content inspection (`python3 -I lab/corpus/public/inspect-content.py ~/bayandocs-corpus/store lab/corpus/public/manifest.json`, see [public/README.md](public/README.md)) and check that it reports no program and no finding that the review did not already accept.
3. Set the write token's credentials in the shell (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`), then run the import against the bucket's S3 address. It stores every document and license text the bucket lacks: `lab/corpus/public/import.sh ~/bayandocs-corpus/sources 's3://bayandocs-corpus/public/v1?endpoint=https://ACCOUNT_ID.r2.cloudflarestorage.com&region=auto'`.
4. Verify through the S3 address, which also finds stray objects: `bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store 's3://…' --exclude lab/corpus/public/excluded.tsv`.
5. Upload `manifest.json` and `excluded.tsv` next to `objects/` and `licenses/` (for example in the R2 dashboard), so that the bucket says by itself which license and provenance belong to each document, also for someone who downloads it without this repository.
6. Verify the public address, with the credentials unset, as any reader would: `bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store https://corpus.bayandocs.org/public/v1`.
7. Record the public address in [public/README.md](public/README.md) in a pull request.

Objects are never changed in place: the bytes behind a SHA-256 never change, so older manifests stay verifiable. A new corpus version adds documents and may stop listing some. An object is deleted only by the removal procedure below.

## Removing a document

A rights holder may ask for a document to be taken down, or a person named in it may ask for their data to be erased. Either request is handled promptly and in this order, without repeating the requester's details in any public place (the pull request names the document by its SHA-256 only):

1. Add the document to `public/excluded.tsv` with a reason that names no person ("removed on request" is enough), rebuild the manifest and the report from scratch with `import.sh`, and merge the pull request.
2. Delete the object `objects/ab/cdef….docx` from the bucket (in the R2 dashboard or with any S3 client), and purge its public address from Cloudflare's cache (Caching → Configuration → Custom Purge, by URL). Set the cache lifetime of the custom domain (a Cache Rule with an Edge TTL of at most a day) so that a removal also reaches caches that were not purged.
3. Verify the bucket as in step 4 of the publishing procedure: it must report no stray object.
4. Older manifests in Git still list the document's SHA-256 and upstream path, never its content, and fail to verify that one object, as intended. If the path itself must go, the owner decides whether to rewrite the repository's history.

Until the bucket exists, anyone can build the same store locally with `lab/corpus/public/import.sh` (see [public/README.md](public/README.md)).
