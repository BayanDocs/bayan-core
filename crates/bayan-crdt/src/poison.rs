//! The poison flag of a document, shared with every handle to it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::CrdtError;

/// Whether a document was poisoned by a hostile import (see [`crate::ImportError::Panicked`] and [`crate::ImportError::ValueTooDeep`]).
///
/// Once a panic inside the CRDT library has unwound through it, the library's internal locks are poisoned and every later call that takes one panics again, outside any containment. The document and every handle to it (stories, registries, maps, lists, the undo manager) share this flag and check it before they call the library, so that a poisoned document answers with [`CrdtError::Poisoned`] or an empty value instead.
#[derive(Debug, Clone, Default)]
pub(crate) struct Poison(Arc<AtomicBool>);

impl Poison {
    pub(crate) fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// [`CrdtError::Poisoned`] once the flag is set.
    pub(crate) fn check(&self) -> Result<(), CrdtError> {
        if self.is_set() {
            Err(CrdtError::Poisoned)
        } else {
            Ok(())
        }
    }
}
