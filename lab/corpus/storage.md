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

Both network backends use the `curl` program, which every supported platform has (Windows 10 and later include it; the S3 backend needs curl 7.75 or later for signing), so the lab needs no HTTP, TLS or cryptography code of its own. curl reads no personal configuration file, may use only HTTPS (plain HTTP only to this machine, for tests and local servers), never follows a redirect and expands no URL patterns, and every request has a time limit. Responses come back through a pipe, never through a temporary file, and are cut off at their size limit whatever curl's version; an upload is read by curl from the file the document was read from, so no copy of a document is written anywhere. Requests that fail transiently are tried again from the start, up to three times. A store address with any other scheme than `s3://`, `https://` or `http://` (such as a mistyped `S3://`) is refused rather than taken for a folder.

## Proposal: where the public corpus lives

The public corpus v1 is 1,368 documents and 44.6 MB, and the Fidelity Lab specification's target for the end of Phase 1 is 2,000 public documents: well under 1 GB for years. Nightly CI runs and contributors will download it, so what matters is anonymous public reads without per-download fees, writes limited to the curator, and as little to administer as possible. This is a proposal for the owner, who decides and creates the account (owner checklist, item 10 covers the private corpus; this one is its public counterpart).

**Recommended: a Cloudflare R2 bucket with a custom domain.** The project's domain, `bayandocs.org`, already uses Cloudflare (for e-mail routing), so a subdomain such as `corpus.bayandocs.org` is one setting away.

- Create a bucket, for example `bayandocs-corpus`, and keep the public corpus under the prefix `public/v1/`. Store nothing else in it: everything in a bucket with a public domain is public, so the private corpus (T2) must never go into this bucket or this account's public domains.
- Connect the custom domain `corpus.bayandocs.org` to the bucket (R2 → the bucket → Settings → Custom Domains). Do not enable the `r2.dev` address: Cloudflare intends it for development only and rate-limits it.
- Create an R2 API token with *Object Read & Write* permission **for this bucket only**, for the curator who publishes; keep it out of the repository and out of CI. R2's S3 API always requires credentials (it never serves anonymous requests), so this token is the only way to write.
- Readers then use `--store https://corpus.bayandocs.org/public/v1`, with no account and no credentials.
- Cost: R2 charges nothing for egress, and its free tier (10 GB-month of storage, one million write and ten million read operations a month, at the time of writing) covers this corpus many times over.

**Alternatives:**

| Option | Public reads | Writes | Notes |
|---|---|---|---|
| Backblaze B2 public bucket | at the bucket's download address (`https://f00N.backblazeb2.com/file/BUCKET/…`), with the HTTPS backend | S3 API with an application key limited to the bucket | low storage price; egress free only up to a monthly allowance; a new account and vendor |
| Amazon S3 bucket with a public-read bucket policy | anonymously through the S3 API itself (the `s3://` address works unsigned), or at its HTTPS address | S3 API with an IAM user or role limited to the bucket | the most configuration (Block Public Access must be relaxed for this bucket); egress is billed beyond the free allowance |
| A release asset on GitHub (a `.tar.zst` of the store) | download and unpack, then use the local directory | the owner uploads an archive per corpus version | no new account, and immutable releases (owner checklist, item 6) protect it; but no object-by-object access, and every update is a manual release |

## Publishing the public corpus

Once the bucket exists, the curator fills it from the sources, without changing the manifest:

1. Set the write token's credentials in the shell (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`), then run the import against the bucket's S3 address. It clones the sources at their pinned revisions and stores every document and license text the bucket lacks: `lab/corpus/public/import.sh ~/bayandocs-corpus/sources 's3://bayandocs-corpus/public/v1?endpoint=https://ACCOUNT_ID.r2.cloudflarestorage.com&region=auto'`.
2. Check that the manifest is unchanged (`git status lab/corpus/public` shows nothing): the import reproduces it exactly.
3. Verify through the S3 address, which also finds stray objects: `bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store 's3://…' --exclude lab/corpus/public/excluded.tsv`.
4. Verify the public address, with the credentials unset, as any reader would: `bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store https://corpus.bayandocs.org/public/v1`.
5. Record the public address in [public/README.md](public/README.md) in a pull request.

Objects are never changed or deleted in place: a new corpus version adds documents (and may stop listing some in the manifest), but the bytes behind a SHA-256 never change, so caches can keep them forever and older manifests stay verifiable.

Until the bucket exists, anyone can build the same store locally with `lab/corpus/public/import.sh` (see [public/README.md](public/README.md)).
