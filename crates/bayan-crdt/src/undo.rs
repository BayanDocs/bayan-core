//! Local undo and redo.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::doc::NO_UNDO_ORIGIN;
use crate::{CrdtError, Doc};

/// Undo and redo for one replica's own changes (ADR-0008 §5).
///
/// Every commit of the document ([`Doc::commit`]) is one undo step, except commits made with [`Doc::commit_without_undo`]. Undo reverts only changes this replica made; changes received from other replicas are never undone, and the inverse of a local change is transformed against remote changes that arrived since, so undo composes with concurrent editing.
#[derive(Debug)]
pub struct UndoManager {
    /// `None` only while the manager is being dropped.
    manager: Option<loro::UndoManager>,
    /// The document's poison flag (see [`Doc::is_poisoned`]).
    poisoned: Arc<AtomicBool>,
}

/// The most undo steps kept (older ones are forgotten).
pub const MAX_UNDO_STEPS: usize = 1_000;

impl UndoManager {
    /// Starts recording the local changes of `doc` from now on.
    #[must_use]
    pub fn new(doc: &Doc) -> Self {
        let mut manager = loro::UndoManager::new(doc.loro());
        // Merging uses wall-clock time; 0 keeps every commit its own step, independent of timing.
        manager.set_merge_interval(0);
        manager.set_max_undo_steps(MAX_UNDO_STEPS);
        manager.add_exclude_origin_prefix(NO_UNDO_ORIGIN);
        Self {
            manager: Some(manager),
            poisoned: doc.poison_flag(),
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

    /// Whether there is a step to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.manager
            .as_ref()
            .is_some_and(loro::UndoManager::can_undo)
    }

    /// Whether there is a step to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.manager
            .as_ref()
            .is_some_and(loro::UndoManager::can_redo)
    }

    /// The number of steps that can be undone.
    #[must_use]
    pub fn undo_count(&self) -> usize {
        self.manager
            .as_ref()
            .map_or(0, loro::UndoManager::undo_count)
    }

    fn usable(&mut self) -> Result<Option<&mut loro::UndoManager>, CrdtError> {
        if self.poisoned.load(Ordering::Relaxed) {
            Err(CrdtError::Poisoned)
        } else {
            Ok(self.manager.as_mut())
        }
    }
}

impl Drop for UndoManager {
    fn drop(&mut self) {
        if self.poisoned.load(Ordering::Relaxed)
            && let Some(manager) = self.manager.take()
        {
            // Dropping the manager unsubscribes it from the document, whose state a panic may have left half updated; like the document (see `Doc`'s `Drop`), it is kept forever instead.
            std::mem::forget(manager);
        }
    }
}
