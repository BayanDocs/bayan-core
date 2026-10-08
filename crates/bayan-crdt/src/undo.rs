//! Local undo and redo.

use crate::doc::NO_UNDO_ORIGIN;
use crate::poison::Poison;
use crate::{CrdtError, Doc};

/// Undo and redo for one replica's own changes (ADR-0008 §5).
///
/// Every commit of the document ([`Doc::commit`]) is one undo step, except commits made with [`Doc::commit_without_undo`]. Undo reverts only changes this replica made; changes received from other replicas are never undone, and the inverse of a local change is transformed against remote changes that arrived since, so undo composes with concurrent editing. Once the document is poisoned, the manager refuses ([`CrdtError::Poisoned`]) and reports nothing to undo or redo.
#[derive(Debug)]
pub struct UndoManager {
    /// `None` while the manager is being dropped, and for a document that was poisoned when the manager was created.
    manager: Option<loro::UndoManager>,
    /// The document's poison flag (see [`Doc::is_poisoned`]).
    poison: Poison,
}

/// The most undo steps kept (older ones are forgotten).
pub const MAX_UNDO_STEPS: usize = 1_000;

/// The interval within which Loro merges consecutive commits into one undo step, in milliseconds: none at all, whatever the clock does.
///
/// Loro 1.16.2 merges a commit into the previous undo step when `now - last_undo_time < merge_interval`, with `now` from the system clock (`Date.now()` in WebAssembly). An interval of 0 merges nothing while the clock moves forward, but when it steps backwards (a clock correction) the difference is negative, so every commit until the clock catches up would join one step, and undo grouping would depend on the clock, which ADR-0005 forbids for anything that affects output (CORE-004 report, F12). No difference of two timestamps is below `i64::MIN`, so with it the comparison is never true.
const NEVER_MERGE: i64 = i64::MIN;

impl UndoManager {
    /// Starts recording the local changes of `doc` from now on.
    #[must_use]
    pub fn new(doc: &Doc) -> Self {
        let poison = doc.poison();
        if poison.is_set() {
            // Subscribing to a poisoned document would take one of its poisoned locks and panic.
            return Self {
                manager: None,
                poison,
            };
        }
        let mut manager = loro::UndoManager::new(doc.loro());
        manager.set_merge_interval(NEVER_MERGE);
        manager.set_max_undo_steps(MAX_UNDO_STEPS);
        manager.add_exclude_origin_prefix(NO_UNDO_ORIGIN);
        Self {
            manager: Some(manager),
            poison,
        }
    }

    /// Undoes the latest local step that is not undone yet. Returns whether there was one.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`].
    pub fn undo(&mut self) -> Result<bool, CrdtError> {
        match self.usable()? {
            Some(manager) => manager.undo().map_err(CrdtError::library),
            None => Ok(false),
        }
    }

    /// Redoes the latest undone step. Returns whether there was one.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`].
    pub fn redo(&mut self) -> Result<bool, CrdtError> {
        match self.usable()? {
            Some(manager) => manager.redo().map_err(CrdtError::library),
            None => Ok(false),
        }
    }

    /// Whether there is a step to undo (false once the document is poisoned).
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.readable().is_some_and(loro::UndoManager::can_undo)
    }

    /// Whether there is a step to redo (false once the document is poisoned).
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.readable().is_some_and(loro::UndoManager::can_redo)
    }

    /// The number of steps that can be undone (0 once the document is poisoned).
    #[must_use]
    pub fn undo_count(&self) -> usize {
        self.readable().map_or(0, loro::UndoManager::undo_count)
    }

    fn usable(&mut self) -> Result<Option<&mut loro::UndoManager>, CrdtError> {
        self.poison.check()?;
        Ok(self.manager.as_mut())
    }

    fn readable(&self) -> Option<&loro::UndoManager> {
        if self.poison.is_set() {
            None
        } else {
            self.manager.as_ref()
        }
    }
}

impl Drop for UndoManager {
    fn drop(&mut self) {
        if self.poison.is_set()
            && let Some(manager) = self.manager.take()
        {
            // Dropping the manager unsubscribes it from the document, whose state a panic may have left half updated; like the document (see `Doc`'s `Drop`), it is kept forever instead.
            std::mem::forget(manager);
        }
    }
}
