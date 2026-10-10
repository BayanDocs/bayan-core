//! The S3-compatible backend: objects in a bucket of any storage that speaks the S3 API (Amazon S3, Cloudflare R2, Backblaze B2, MinIO and others), with path-style addresses.
//!
//! Requests go through the `curl` program ([`super::curl`]). They are signed with AWS Signature Version 4 by curl itself when the credentials are set in the environment; without them, requests are anonymous, which some providers allow for reading a public bucket (others serve public buckets only at plain HTTPS addresses, which [`super::https`] reads).
//!
//! **Address:** `s3://BUCKET/PREFIX?endpoint=https://HOST&region=REGION`. The prefix is optional; the region defaults to `us-east-1` (Cloudflare R2 uses `auto`). Only HTTPS is accepted, except plain HTTP to the local machine (`127.0.0.1`, `localhost`, `[::1]`) for tests and local servers. Redirects are never followed.

use std::path::Path;

use super::curl::{self, Credentials, Method, Request, Response};
use super::{Key, MAX_OBJECT_SIZE, Store, StoreError};
use crate::scan::xml::{self, Handler, Name, Start, Text, XmlLimits};

/// The most pages of a listing followed (a page holds up to 1,000 keys).
const MAX_LIST_PAGES: usize = 10_000;
/// The largest listing page accepted, in bytes.
const MAX_LIST_PAGE: u64 = 16 * 1024 * 1024;
/// The most objects a listing may report: far more than any corpus, but a bound on memory.
const MAX_LISTED_OBJECTS: usize = 1_000_000;
/// The largest response to a PUT or HEAD request accepted, in bytes (neither has a meaningful body).
const MAX_SMALL_RESPONSE: u64 = 64 * 1024;
/// The longest key a listing may report, in bytes (S3 allows 1,024).
const MAX_KEY: usize = 1_024;
/// The most text kept of one element of a listing, in bytes: room for a key or a continuation token.
const MAX_TEXT: usize = 4_096;

/// The content type of a stored document.
const DOCX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// A store in an S3-compatible bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Store {
    /// `https://host[:port]`, without a trailing `/`.
    endpoint: String,
    bucket: String,
    /// The key prefix, without leading or trailing `/`; empty for none.
    prefix: String,
    region: String,
    /// Whether the endpoint is plain HTTP on the local machine.
    local_http: bool,
}

impl S3Store {
    /// Parses an address such as `s3://bayandocs-corpus/public?endpoint=https://s3.example.com&region=auto`.
    ///
    /// # Errors
    ///
    /// When the address is malformed.
    pub fn from_address(address: &str) -> Result<Self, StoreError> {
        let error = |what: &str| StoreError(format!("invalid S3 store address: {what}"));
        let rest = address
            .strip_prefix("s3://")
            .ok_or_else(|| error("it must start with s3://"))?;
        let (path, query) = rest
            .split_once('?')
            .ok_or_else(|| error("the endpoint is missing (add ?endpoint=https://…)"))?;
        let (bucket, prefix) = path.split_once('/').unwrap_or((path, ""));
        let prefix = prefix.trim_end_matches('/');
        let bucket_ok = (3..=63).contains(&bucket.len())
            && bucket.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
            })
            && !bucket.starts_with(['.', '-'])
            && !bucket.ends_with(['.', '-']);
        if !bucket_ok {
            return Err(error(
                "the bucket name must be 3 to 63 lower-case letters, digits, dots or hyphens",
            ));
        }
        if !curl::is_simple_path(prefix) {
            return Err(error(
                "the prefix may hold only letters, digits, dots, underscores, hyphens and `/` between them",
            ));
        }
        let mut endpoint = None;
        let mut region = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("endpoint", value)) if endpoint.is_none() => endpoint = Some(value),
                Some(("region", value)) if region.is_none() => region = Some(value),
                _ => {
                    return Err(error(
                        "the query may hold `endpoint` and `region`, once each",
                    ));
                }
            }
        }
        let (endpoint, local_http) =
            curl::endpoint(endpoint.ok_or_else(|| error("the endpoint is missing"))?)
                .map_err(|what| error(&format!("endpoint: {what}")))?;
        let region = region.unwrap_or("us-east-1");
        if region.is_empty()
            || region.len() > 32
            || !region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(error(
                "the region may hold only letters, digits and hyphens",
            ));
        }
        Ok(Self {
            endpoint,
            bucket: bucket.to_owned(),
            prefix: prefix.to_owned(),
            region: region.to_owned(),
            local_http,
        })
    }

    /// The object's key in the bucket.
    fn object_key(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", self.prefix)
        }
    }

    fn object_url(&self, path: &str) -> String {
        format!(
            "{}/{}/{}",
            self.endpoint,
            self.bucket,
            self.object_key(path)
        )
    }

    /// Runs one request, signed when the environment holds credentials. `upload` is the file sent as the request body of a PUT.
    fn request(
        &self,
        method: Method,
        url: &str,
        upload: Option<(&Path, &str)>,
        max_size: u64,
    ) -> Result<Response, StoreError> {
        let credentials: Option<Credentials> = curl::credentials()?;
        let shown = self.redact(url);
        curl::run(&Request {
            method,
            url,
            shown: &shown,
            upload,
            max_size,
            local_http: self.local_http,
            signing: credentials
                .as_ref()
                .map(|credentials| (credentials, self.region.as_str())),
        })
    }

    /// The URL without the endpoint, for messages.
    fn redact(&self, url: &str) -> String {
        url.strip_prefix(&self.endpoint)
            .map_or_else(|| url.to_owned(), |rest| format!("…{rest}"))
    }
}

impl Store for S3Store {
    fn describe(&self) -> String {
        let prefix = if self.prefix.is_empty() {
            String::new()
        } else {
            format!("/{}", self.prefix)
        };
        format!("s3://{}{prefix} at {}", self.bucket, self.endpoint)
    }

    fn get(&self, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        let url = self.object_url(&key.path());
        let response = self.request(Method::Get, &url, None, MAX_OBJECT_SIZE)?;
        match response.status {
            200 => Ok(Some(response.body)),
            404 => Ok(None),
            code => Err(StoreError(format!(
                "GET {}: HTTP status {code}",
                self.redact(&url)
            ))),
        }
    }

    fn put(&self, key: &Key, bytes: &[u8], source: &Path) -> Result<(), StoreError> {
        let url = self.object_url(&key.path());
        // curl sends the file the bytes were read from, so no copy of the document is written anywhere; a file that changed since is refused.
        let unchanged = std::fs::metadata(source).is_ok_and(|metadata| {
            u64::try_from(bytes.len()).is_ok_and(|len| len == metadata.len())
        });
        if !unchanged {
            return Err(StoreError(format!(
                "PUT {}: the file to send changed since it was read",
                self.redact(&url)
            )));
        }
        let content_type = match key.kind {
            super::Kind::Document => DOCX_CONTENT_TYPE,
            super::Kind::LicenseText => "text/plain; charset=utf-8",
        };
        let response = self.request(
            Method::Put,
            &url,
            Some((source, content_type)),
            MAX_SMALL_RESPONSE,
        )?;
        match response.status {
            200 | 201 | 204 => Ok(()),
            code => Err(StoreError(format!(
                "PUT {}: HTTP status {code}",
                self.redact(&url)
            ))),
        }
    }

    fn contains(&self, key: &Key) -> Result<bool, StoreError> {
        let url = self.object_url(&key.path());
        match self
            .request(Method::Head, &url, None, MAX_SMALL_RESPONSE)?
            .status
        {
            200 => Ok(true),
            404 => Ok(false),
            code => Err(StoreError(format!(
                "HEAD {}: HTTP status {code} (a bucket that does not let this client list it answers 403 for missing objects)",
                self.redact(&url)
            ))),
        }
    }

    fn list(&self) -> Result<Option<Vec<String>>, StoreError> {
        let mut found = Vec::new();
        for area in ["objects/", "licenses/"] {
            let listed_prefix = self.object_key(area);
            let mut token: Option<String> = None;
            for page in 0.. {
                if page >= MAX_LIST_PAGES {
                    return Err(StoreError(
                        "the bucket listing has more pages than the limit".to_owned(),
                    ));
                }
                let mut url = format!(
                    "{}/{}?list-type=2&prefix={}",
                    self.endpoint,
                    self.bucket,
                    percent_encode(&listed_prefix)
                );
                if let Some(token) = &token {
                    url.push_str("&continuation-token=");
                    url.push_str(&percent_encode(token));
                }
                let response = self.request(Method::Get, &url, None, MAX_LIST_PAGE)?;
                if response.status != 200 {
                    return Err(StoreError(format!(
                        "listing the bucket: HTTP status {}",
                        response.status
                    )));
                }
                let listing = parse_listing(&response.body)?;
                if found.len().saturating_add(listing.keys.len()) > MAX_LISTED_OBJECTS {
                    return Err(StoreError(format!(
                        "the bucket lists more than {MAX_LISTED_OBJECTS} objects, the limit"
                    )));
                }
                for key in listing.keys {
                    let path = if self.prefix.is_empty() {
                        Some(key.as_str())
                    } else {
                        key.strip_prefix(&self.prefix)
                            .and_then(|rest| rest.strip_prefix('/'))
                    };
                    if let Some(path) = path {
                        found.push(path.to_owned());
                    }
                }
                match (listing.truncated, listing.next) {
                    (true, Some(next)) => token = Some(next),
                    (true, None) => {
                        return Err(StoreError(
                            "the bucket listing is cut without a continuation token".to_owned(),
                        ));
                    }
                    (false, _) => break,
                }
            }
        }
        found.sort();
        found.dedup();
        Ok(Some(found))
    }
}

/// Percent-encodes everything except the characters RFC 3986 leaves unreserved.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One page of a `ListObjectsV2` response.
#[derive(Debug, Default, PartialEq, Eq)]
struct Listing {
    keys: Vec<String>,
    truncated: bool,
    next: Option<String>,
}

/// Reads a `ListObjectsV2` response (`ListBucketResult`), by local names, since some S3-compatible servers leave out the namespace.
fn parse_listing(bytes: &[u8]) -> Result<Listing, StoreError> {
    #[derive(Default)]
    struct Reader {
        listing: Listing,
        path: Vec<String>,
        text: String,
        too_long: bool,
    }
    impl Handler for Reader {
        fn start(&mut self, start: &Start<'_, '_>) {
            self.path.push(start.name().local.to_owned());
            self.text.clear();
        }
        fn end(&mut self, name: Name<'_>) {
            let parent = self
                .path
                .len()
                .checked_sub(2)
                .and_then(|index| self.path.get(index))
                .map(String::as_str);
            match (parent, name.local) {
                (Some("Contents"), "Key") if self.text.len() > MAX_KEY => self.too_long = true,
                (Some("Contents"), "Key") => self.listing.keys.push(std::mem::take(&mut self.text)),
                (Some("ListBucketResult"), "IsTruncated") => {
                    self.listing.truncated = self.text.trim() == "true"
                }
                (Some("ListBucketResult"), "NextContinuationToken") => {
                    self.listing.next = Some(self.text.trim().to_owned())
                }
                _ => {}
            }
            self.path.pop();
            self.text.clear();
        }
        fn text(&mut self, text: Text<'_>) {
            // Keeps a little more than any key or token needs, so an overlong key is recognized as such without holding all of it.
            for character in text.decoded().chars() {
                if self.text.len() > MAX_TEXT {
                    break;
                }
                self.text.push(character);
            }
        }
    }
    let limits = XmlLimits {
        max_depth: 16,
        max_attributes: 16,
        max_name_len: 128,
        max_namespaces: 16,
    };
    let mut reader = Reader::default();
    xml::scan(bytes, &limits, &mut reader)
        .map_err(|error| StoreError(format!("the bucket listing is not valid: {error}")))?;
    if reader.too_long {
        return Err(StoreError(format!(
            "the bucket listing has a key longer than {MAX_KEY} bytes"
        )));
    }
    Ok(reader.listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses() {
        let store = S3Store::from_address(
            "s3://bayandocs-corpus/public/v1?endpoint=https://s3.example.com/&region=auto",
        )
        .unwrap();
        assert_eq!(store.endpoint, "https://s3.example.com");
        assert_eq!(store.bucket, "bayandocs-corpus");
        assert_eq!(store.prefix, "public/v1");
        assert_eq!(store.region, "auto");
        assert!(!store.local_http);
        assert_eq!(
            store.object_url("objects/ab/cd.docx"),
            "https://s3.example.com/bayandocs-corpus/public/v1/objects/ab/cd.docx"
        );
        let local = S3Store::from_address("s3://bucket?endpoint=http://127.0.0.1:9000").unwrap();
        assert!(local.local_http);
        assert_eq!(local.region, "us-east-1");
        assert_eq!(
            local.object_url("licenses/x.txt"),
            "http://127.0.0.1:9000/bucket/licenses/x.txt"
        );
        for bad in [
            "s3://bucket",
            "s3://Bucket?endpoint=https://h",
            "s3://bu?endpoint=https://h",
            "s3://bucket/../x?endpoint=https://h",
            "s3://bucket?endpoint=http://example.com",
            "s3://bucket?endpoint=https://user@h",
            "s3://bucket?endpoint=https://h/path",
            "s3://bucket?endpoint=https://h&region=a b",
            "s3://bucket?endpoint=https://h&other=1",
            "s3://bucket?endpoint=https://h&endpoint=https://i",
            "https://bucket",
        ] {
            assert!(S3Store::from_address(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn reads_listing_pages() {
        let page = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>b</Name><Prefix>p/objects/</Prefix><KeyCount>2</KeyCount><IsTruncated>true</IsTruncated><NextContinuationToken>a+b/c=</NextContinuationToken><Contents><Key>p/objects/ab/cd.docx</Key><Size>3</Size></Contents><Contents><Key>p/objects/ef/&amp;.docx</Key></Contents></ListBucketResult>"#;
        assert_eq!(
            parse_listing(page).unwrap(),
            Listing {
                keys: vec!["p/objects/ab/cd.docx".into(), "p/objects/ef/&.docx".into()],
                truncated: true,
                next: Some("a+b/c=".into()),
            }
        );
        assert!(parse_listing(b"<!DOCTYPE x><x/>").is_err());
        let long = format!(
            "<ListBucketResult><Contents><Key>{}</Key></Contents></ListBucketResult>",
            "k".repeat(MAX_KEY + 1)
        );
        assert!(parse_listing(long.as_bytes()).is_err());
        assert_eq!(percent_encode("a+b/c= d~"), "a%2Bb%2Fc%3D%20d~");
    }
}
