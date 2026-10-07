//! Local undo and redo.

use crate::doc::NO_UNDO_ORIGIN;
use crate::{CrdtError, Doc};

/// Undo and redo for one replica's own changes (ADR-0008 §5).
///
/// Every commit of the document ([`Doc::commit`]) is one undo step, except commits made with [`Doc::commit_without_undo`]. Undo reverts only changes this replica made; changes received from other replicas are never undone, and the inverse of a local change is transformed against remote changes that arrived since, so undo composes with concurrent editing.
#[derive(Debug)]
pub struct UndoManager {
    manager: loro::UndoManager,
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
        Self { manager }
    }

    /// Undoes the latest local step that is not undone yet. Returns whether there was one.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Library`].
    pub fn undo(&mut self) -> Result<bool, CrdtError> {
        self.manager.undo().map_err(CrdtError::library)
    }

    /// Redoes the latest undone step. Returns whether there was one.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Library`].
    pub fn redo(&mut self) -> Result<bool, CrdtError> {
        self.manager.redo().map_err(CrdtError::library)
    }

    /// Whether there is a step to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.manager.can_undo()
    }

    /// Whether there is a step to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.manager.can_redo()
    }

    /// The number of steps that can be undone.
    #[must_use]
    pub fn undo_count(&self) -> usize {
        self.manager.undo_count()
    }
}
