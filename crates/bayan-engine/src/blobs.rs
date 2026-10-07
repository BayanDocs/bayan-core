//! Blobs: bulk bytes that travel outside the JSON messages ([engine protocol specification][spec] §3).
//!
//! The store is shared between the threads of a native host: the shell puts, reads and releases blobs from its own threads through the C interface while the engine thread reads them, so it locks a mutex for every operation and hands out shared, immutable copies of the bytes, which callers then copy without holding the lock.
//!
//! Blobs that a shell puts get odd identifiers and blobs the engine creates get even ones, from two separate counters. The engine's counter therefore depends only on the messages it handled, and a recording replays with identical identifiers however the shell's own blobs were interleaved (spec §10).
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::limits::{MAX_BLOB_BYTES, MAX_BLOBS, MAX_ID, MAX_TOTAL_BLOB_BYTES};

/// A blob's identifier; 0 is never valid.
pub type BlobId = u64;

/// Why a blob could not be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobError {
    /// The blob is larger than 64 MiB.
    TooLarge,
    /// There are already 1,024 blobs, or together they would hold more than 256 MiB.
    TooMany,
}

/// The blobs of one engine.
#[derive(Debug)]
pub struct BlobStore {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    blobs: BTreeMap<BlobId, Arc<[u8]>>,
    total_bytes: usize,
    next_shell: BlobId,
    next_engine: BlobId,
}

impl Default for BlobStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BlobStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                blobs: BTreeMap::new(),
                total_bytes: 0,
                next_shell: 1,
                next_engine: 2,
            }),
        }
    }

    /// Locks the store. A panic elsewhere cannot leave it half-changed (every operation changes it in one step), so a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copies bytes from the shell into a new blob, with the next odd identifier.
    ///
    /// # Errors
    ///
    /// Fails if a limit of spec §13 would be exceeded.
    pub fn put_shell(&self, bytes: &[u8]) -> Result<BlobId, BlobError> {
        let mut inner = self.lock();
        inner.check(bytes.len())?;
        let id = inner.next_shell;
        if id > MAX_ID {
            return Err(BlobError::TooMany);
        }
        inner.next_shell += 2;
        inner.insert(id, Arc::from(bytes));
        Ok(id)
    }

    /// Stores bytes the engine created in a new blob, with the next even identifier.
    ///
    /// # Errors
    ///
    /// Fails if a limit of spec §13 would be exceeded.
    pub fn put_engine(&self, bytes: Vec<u8>) -> Result<BlobId, BlobError> {
        let mut inner = self.lock();
        inner.check(bytes.len())?;
        let id = inner.next_engine;
        if id > MAX_ID {
            return Err(BlobError::TooMany);
        }
        inner.next_engine += 2;
        inner.insert(id, Arc::from(bytes));
        Ok(id)
    }

    /// The bytes of a blob, if it exists.
    #[must_use]
    pub fn get(&self, id: BlobId) -> Option<Arc<[u8]>> {
        self.lock().blobs.get(&id).cloned()
    }

    /// Releases a blob. Returns whether it existed.
    pub fn release(&self, id: BlobId) -> bool {
        let mut inner = self.lock();
        match inner.blobs.remove(&id) {
            Some(bytes) => {
                inner.total_bytes -= bytes.len();
                true
            }
            None => false,
        }
    }

    /// Returns a blob's bytes and releases it.
    pub fn take(&self, id: BlobId) -> Option<Arc<[u8]>> {
        let mut inner = self.lock();
        let bytes = inner.blobs.remove(&id)?;
        inner.total_bytes -= bytes.len();
        Some(bytes)
    }

    /// The number of blobs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().blobs.len()
    }

    /// Whether there are no blobs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().blobs.is_empty()
    }

    /// The identifier the engine's next blob will get, for recordings (spec §10).
    pub(crate) fn next_engine_id(&self) -> BlobId {
        self.lock().next_engine
    }

    /// Continues the engine's numbering from a recording: `next` must be even and positive.
    pub(crate) fn set_next_engine_id(&self, next: BlobId) -> bool {
        if next == 0 || !next.is_multiple_of(2) || next > MAX_ID + 1 {
            return false;
        }
        self.lock().next_engine = next;
        true
    }

    /// Stores a blob of a recording under the identifier it had when it was recorded (spec §10). Returns false if the identifier is invalid or taken, or a limit would be exceeded.
    pub(crate) fn insert_recorded(&self, id: BlobId, bytes: Vec<u8>) -> bool {
        let mut inner = self.lock();
        if id == 0
            || id > MAX_ID
            || inner.blobs.contains_key(&id)
            || inner.check(bytes.len()).is_err()
        {
            return false;
        }
        inner.insert(id, Arc::from(bytes));
        true
    }
}

impl Inner {
    fn check(&self, length: usize) -> Result<(), BlobError> {
        if length > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        if self.blobs.len() >= MAX_BLOBS || self.total_bytes + length > MAX_TOTAL_BLOB_BYTES {
            return Err(BlobError::TooMany);
        }
        Ok(())
    }

    /// Stores a blob. A blob with the same identifier is replaced, and stops counting towards the limits: that happens only during a replay, when the engine creates again a blob whose recorded copy the recording carried.
    fn insert(&mut self, id: BlobId, bytes: Arc<[u8]>) {
        self.total_bytes += bytes.len();
        if let Some(replaced) = self.blobs.insert(id, bytes) {
            self.total_bytes -= replaced.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_shell_blobs_odd_and_engine_blobs_even() {
        let store = BlobStore::new();
        assert_eq!(store.put_shell(b"a"), Ok(1));
        assert_eq!(store.put_engine(b"b".to_vec()), Ok(2));
        assert_eq!(store.put_shell(b""), Ok(3));
        assert_eq!(store.put_engine(Vec::new()), Ok(4));
        assert_eq!(store.get(1).as_deref(), Some(&b"a"[..]));
        assert_eq!(store.get(3).as_deref(), Some(&b""[..]));
        assert_eq!(store.next_engine_id(), 6);
    }

    #[test]
    fn releases_and_takes() {
        let store = BlobStore::new();
        let id = store.put_shell(b"abc").unwrap();
        assert_eq!(store.take(id).as_deref(), Some(&b"abc"[..]));
        assert!(!store.release(id));
        assert!(store.get(id).is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn enforces_the_limits() {
        let store = BlobStore::new();
        assert_eq!(
            store.put_shell(&vec![0; MAX_BLOB_BYTES + 1]),
            Err(BlobError::TooLarge)
        );
        for _ in 0..MAX_BLOBS {
            store.put_shell(b"").unwrap();
        }
        assert_eq!(store.put_shell(b""), Err(BlobError::TooMany));
        assert_eq!(store.put_engine(Vec::new()), Err(BlobError::TooMany));
        assert_eq!(store.len(), MAX_BLOBS);
    }

    #[test]
    fn enforces_the_total_size() {
        let store = BlobStore::new();
        let id = store.put_shell(b"abc").unwrap();
        // Pretend that the other blobs already hold all but two bytes of the allowance, without allocating them.
        store.lock().total_bytes = MAX_TOTAL_BLOB_BYTES - 2;
        assert_eq!(store.put_shell(b"xyz"), Err(BlobError::TooMany));
        assert!(store.put_shell(b"xy").is_ok());
        assert_eq!(store.put_engine(b"z".to_vec()), Err(BlobError::TooMany));
        assert!(store.release(id));
        assert!(store.put_engine(b"z".to_vec()).is_ok());
    }

    #[test]
    fn a_blob_created_again_during_a_replay_is_counted_once() {
        let store = BlobStore::new();
        // A recording carried blob 2, which the replayed engine then creates again.
        assert!(store.insert_recorded(2, vec![0; 1000]));
        assert!(store.set_next_engine_id(2));
        assert_eq!(store.put_engine(vec![1; 1000]), Ok(2));
        assert_eq!(store.len(), 1);
        assert_eq!(store.lock().total_bytes, 1000);
        assert!(store.release(2));
        assert_eq!(store.lock().total_bytes, 0);
    }

    #[test]
    fn restores_recorded_blobs_and_numbering() {
        let store = BlobStore::new();
        assert!(store.insert_recorded(7, b"seven".to_vec()));
        assert!(!store.insert_recorded(7, b"again".to_vec()));
        assert!(!store.insert_recorded(0, Vec::new()));
        assert!(store.set_next_engine_id(10));
        assert!(!store.set_next_engine_id(9));
        assert_eq!(store.put_engine(Vec::new()), Ok(10));
        assert_eq!(store.get(7).as_deref(), Some(&b"seven"[..]));
    }
}
