//! A read-only store at a plain HTTPS address: the public face of a bucket, such as a Cloudflare R2 bucket's custom domain or a Backblaze B2 bucket's download address, or any web server that serves the store's files.
//!
//! Some providers allow anonymous reads of a public bucket only at such an address, not through the S3 API, so this is how anyone can read and verify the public corpus without credentials: the objects are fetched at `ADDRESS/objects/ab/cdef….docx` and `ADDRESS/licenses/<sha256>.txt`. The store sends no credentials, ever, and it can be neither written nor listed: write through the bucket's S3 address ([`super::s3`]), and verify with that address, or with a local copy, to find objects the manifest does not know.
//!
//! **Address:** `https://HOST[:PORT][/PATH]`, without a query, a fragment or a user name. Plain `http://` is accepted only to this machine (`127.0.0.1`, `localhost`, `[::1]`), for tests and local servers. Requests go through the `curl` program ([`super::curl`]); redirects are never followed.

use super::curl::{self, Method, Request, TempFile};
use super::{Key, MAX_OBJECT_SIZE, Store, StoreError};

/// The largest response to a HEAD request accepted, in bytes (it has no body).
const MAX_HEAD_RESPONSE: u64 = 64 * 1024;

/// A read-only store at an HTTPS address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsStore {
    /// The address without a trailing `/`.
    base: String,
    /// Whether it is plain HTTP on this machine.
    local_http: bool,
}

impl HttpsStore {
    /// Parses an address such as `https://corpus.example.org/public/v1`.
    ///
    /// # Errors
    ///
    /// When the address is malformed.
    pub fn from_address(address: &str) -> Result<Self, StoreError> {
        let error = |what: &str| StoreError(format!("invalid HTTPS store address: {what}"));
        let address = address.trim_end_matches('/');
        if address.contains(['?', '#']) {
            return Err(error("it may hold no query and no fragment"));
        }
        let (scheme, rest) = address
            .split_once("://")
            .ok_or_else(|| error("it must start with https://"))?;
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (endpoint, local_http) =
            curl::endpoint(&format!("{scheme}://{host}")).map_err(error)?;
        if !curl::is_simple_path(path) {
            return Err(error(
                "the path may hold only letters, digits, dots, underscores, hyphens and `/` between them",
            ));
        }
        let base = if path.is_empty() {
            endpoint
        } else {
            format!("{endpoint}/{path}")
        };
        Ok(Self { base, local_http })
    }

    fn url(&self, key: &Key) -> String {
        format!("{}/{}", self.base, key.path())
    }

    fn request(
        &self,
        method: Method,
        url: &str,
        output: &TempFile,
        max_size: u64,
    ) -> Result<u16, StoreError> {
        curl::run(&Request {
            method,
            url,
            shown: url,
            output,
            upload: None,
            max_size,
            local_http: self.local_http,
            signing: None,
        })
    }
}

impl Store for HttpsStore {
    fn describe(&self) -> String {
        format!("{} (read-only)", self.base)
    }

    fn get(&self, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        let output = TempFile::new("get");
        let url = self.url(key);
        match self.request(Method::Get, &url, &output, MAX_OBJECT_SIZE)? {
            200 => output.read(MAX_OBJECT_SIZE).map(Some),
            404 => Ok(None),
            code => Err(StoreError(format!("GET {url}: HTTP status {code}"))),
        }
    }

    fn put(&self, _: &Key, _: &[u8]) -> Result<(), StoreError> {
        Err(StoreError(format!(
            "{} is read-only; write to the bucket through its s3:// address",
            self.base
        )))
    }

    fn contains(&self, key: &Key) -> Result<bool, StoreError> {
        let output = TempFile::new("head");
        let url = self.url(key);
        match self.request(Method::Head, &url, &output, MAX_HEAD_RESPONSE)? {
            200 => Ok(true),
            404 => Ok(false),
            code => Err(StoreError(format!("HEAD {url}: HTTP status {code}"))),
        }
    }

    fn list(&self) -> Result<Option<Vec<String>>, StoreError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Sha256;

    #[test]
    fn parses_addresses() {
        let store = HttpsStore::from_address("https://corpus.example.org/public/v1/").unwrap();
        assert_eq!(store.base, "https://corpus.example.org/public/v1");
        assert!(!store.local_http);
        assert_eq!(
            store.url(&Key::license_text(Sha256::of(b"abc"))),
            "https://corpus.example.org/public/v1/licenses/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.txt"
        );
        let root = HttpsStore::from_address("https://corpus.example.org").unwrap();
        assert_eq!(root.base, "https://corpus.example.org");
        let local = HttpsStore::from_address("http://127.0.0.1:8080/store").unwrap();
        assert!(local.local_http);
        for bad in [
            "http://corpus.example.org/public",
            "https://user:secret@corpus.example.org/",
            "https://corpus.example.org/a/../b",
            "https://corpus.example.org/public?list=1",
            "https://corpus.example.org/public#x",
            "https:///public",
            "corpus.example.org/public",
        ] {
            assert!(HttpsStore::from_address(bad).is_err(), "{bad} was accepted");
        }
        assert!(
            HttpsStore::from_address("https://corpus.example.org")
                .unwrap()
                .put(&Key::document(Sha256::of(b"x")), b"x")
                .is_err()
        );
    }
}
