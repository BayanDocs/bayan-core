//! Content-addressed storage for corpus documents and their sources' license texts.
//!
//! Every object is stored under a key derived from its SHA-256, so the same bytes are stored once and the key itself proves what the object must contain:
//!
//! ```text
//! objects/ab/cdef….docx      a document (the first two hexadecimal digits name the folder)
//! licenses/abcdef….txt       a license or notice text of a source
//! ```
//!
//! Three backends implement [`Store`]: [`dir::DirStore`], a local directory; [`s3::S3Store`], any S3-compatible object storage; and [`https::HttpsStore`], a read-only store at a plain HTTPS address, such as a public bucket's. The last two reach the network through the `curl` program ([`curl`]), so that the lab needs no HTTP or TLS library of its own. [`open`] chooses one from a store address. `lab/corpus/storage.md` explains how the public corpus is stored and published.

pub mod curl;
pub mod dir;
pub mod https;
pub mod s3;

use std::fmt;

use crate::hash::Sha256;

/// The largest object the stores read, in bytes: the scanner's package limit.
pub const MAX_OBJECT_SIZE: u64 = 512 * 1024 * 1024;

/// What an object is, which decides its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A corpus document (`objects/ab/cdef….docx`).
    Document,
    /// A license or notice text of a source (`licenses/abcdef….txt`).
    LicenseText,
}

/// The key of an object: its kind and its SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    /// What the object is.
    pub kind: Kind,
    /// The SHA-256 of its bytes.
    pub sha256: Sha256,
}

impl Key {
    /// The key of a document.
    #[must_use]
    pub const fn document(sha256: Sha256) -> Self {
        Self {
            kind: Kind::Document,
            sha256,
        }
    }

    /// The key of a license text.
    #[must_use]
    pub const fn license_text(sha256: Sha256) -> Self {
        Self {
            kind: Kind::LicenseText,
            sha256,
        }
    }

    /// The object's path relative to the store's root, with `/` separators.
    #[must_use]
    pub fn path(&self) -> String {
        let hex = self.sha256.to_string();
        match self.kind {
            Kind::Document => {
                let (folder, rest) = hex.split_at(2);
                format!("objects/{folder}/{rest}.docx")
            }
            Kind::LicenseText => format!("licenses/{hex}.txt"),
        }
    }

    /// The key whose path is `path`, if `path` is a valid object path.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        if let Some(rest) = path.strip_prefix("objects/") {
            let (folder, file) = rest.split_once('/')?;
            let digest = file.strip_suffix(".docx")?;
            if folder.len() != 2 {
                return None;
            }
            return format!("{folder}{digest}").parse().ok().map(Self::document);
        }
        let digest = path.strip_prefix("licenses/")?.strip_suffix(".txt")?;
        digest.parse().ok().map(Self::license_text)
    }
}

/// An error of a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// A content-addressed store.
pub trait Store {
    /// A description of the store for messages, never including credentials.
    fn describe(&self) -> String;

    /// The bytes of an object, or `None` if the store does not have it.
    ///
    /// # Errors
    ///
    /// When the store cannot be read, or the object is larger than [`MAX_OBJECT_SIZE`].
    fn get(&self, key: &Key) -> Result<Option<Vec<u8>>, StoreError>;

    /// Stores an object. The caller has computed `key` from `bytes`; storing the same object twice is harmless.
    ///
    /// # Errors
    ///
    /// When the store cannot be written.
    fn put(&self, key: &Key, bytes: &[u8]) -> Result<(), StoreError>;

    /// Whether the store has an object.
    ///
    /// # Errors
    ///
    /// When the store cannot be read.
    fn contains(&self, key: &Key) -> Result<bool, StoreError>;

    /// Every object path in the store under `objects/` and `licenses/`, valid or not, sorted; `None` for a store that cannot be listed ([`https::HttpsStore`]).
    ///
    /// # Errors
    ///
    /// When the store cannot be listed although it should be.
    fn list(&self) -> Result<Option<Vec<String>>, StoreError>;
}

/// Opens the store at `address`: `s3://bucket/prefix?endpoint=https://host&region=name` for S3-compatible storage, `https://host/path` for a read-only store at an HTTPS address, otherwise a local directory.
///
/// # Errors
///
/// When an S3 or HTTPS address is malformed.
pub fn open(address: &str) -> Result<Box<dyn Store>, StoreError> {
    if address.starts_with("s3://") {
        Ok(Box::new(s3::S3Store::from_address(address)?))
    } else if address.starts_with("https://") || address.starts_with("http://") {
        Ok(Box::new(https::HttpsStore::from_address(address)?))
    } else {
        Ok(Box::new(dir::DirStore::new(address)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_paths_and_back() {
        let sha256 = Sha256::of(b"abc");
        let document = Key::document(sha256);
        assert_eq!(
            document.path(),
            "objects/ba/7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.docx"
        );
        assert_eq!(Key::from_path(&document.path()), Some(document));
        let text = Key::license_text(sha256);
        assert_eq!(
            text.path(),
            "licenses/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.txt"
        );
        assert_eq!(Key::from_path(&text.path()), Some(text));
        for bad in [
            "objects/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.docx",
            "objects/b/a7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.docx",
            "objects/ba/7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.doc",
            "licenses/x.txt",
            "other/file",
        ] {
            assert_eq!(Key::from_path(bad), None, "{bad}");
        }
    }
}
