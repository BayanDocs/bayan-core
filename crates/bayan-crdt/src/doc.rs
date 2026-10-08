//! The replicated document.

use std::panic::AssertUnwindSafe;

use loro::{
    ExpandType, ExportMode, Frontiers, LoroDoc, LoroMap, LoroText, StyleConfig, StyleConfigMap,
};

use crate::import::{check_header, imported_values_too_deep};
use crate::poison::Poison;
use crate::story::ATOM_FAMILY;
use crate::{
    CrdtError, Expand, ImportError, ImportLimits, ImportReport, MarkFamily, PropertyMap, Registry,
    Stories, Story,
};

/// The name of the root sequence that holds the main story. It is a root container, so every replica has it from the start and two replicas can never create competing main stories.
const MAIN_STORY: &str = "main";

/// The name of the root map that holds every other story.
const STORIES: &str = "stories";

/// The commit origin of changes that are not undo steps ([`Doc::commit_without_undo`]).
pub(crate) const NO_UNDO_ORIGIN: &str = "bayan:no-undo";

/// The identifier of one replica (one editing session). Two replicas that edit concurrently must never share a peer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(pub u64);

/// A version of the document: the set of latest changes it includes. Used to look at or export the document as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(Frontiers);

impl Version {
    /// The version as bytes, for storing it (for example as a named version).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.0.encode()
    }

    /// Reads a version stored with [`Version::encode`].
    ///
    /// # Errors
    ///
    /// [`CrdtError::Library`] if the bytes are not a version.
    pub fn decode(bytes: &[u8]) -> Result<Self, CrdtError> {
        Frontiers::decode(bytes)
            .map(Self)
            .map_err(CrdtError::library)
    }
}

/// How many operations of each replica a document has seen. A replica sends another the changes the other's version vector does not include ([`Doc::export_updates`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionVector(loro::VersionVector);

impl VersionVector {
    /// The version vector of an empty document.
    #[must_use]
    pub fn new() -> Self {
        Self(loro::VersionVector::new())
    }

    /// The version vector as bytes, for sending it to another replica.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.0.encode()
    }

    /// Reads a version vector sent with [`VersionVector::encode`].
    ///
    /// # Errors
    ///
    /// [`CrdtError::Library`] if the bytes are not a version vector.
    pub fn decode(bytes: &[u8]) -> Result<Self, CrdtError> {
        loro::VersionVector::decode(bytes)
            .map(Self)
            .map_err(CrdtError::library)
    }
}

impl Default for VersionVector {
    fn default() -> Self {
        Self::new()
    }
}

/// Size counters of a document, for tests and measurements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocStats {
    /// The number of operations in the history.
    pub ops: usize,
    /// The number of changes (groups of operations) in the history.
    pub changes: usize,
}

/// One replica of a document.
///
/// A document holds stories ([`Doc::main_story`], [`Doc::stories`]), registries of entities ([`Doc::registry`]) and root property maps ([`Doc::root_map`]). Edits are grouped into changes by [`Doc::commit`]; each change is one undo step ([`crate::UndoManager`]) and the unit that replicas exchange ([`Doc::export_updates`], [`Doc::import`]).
///
/// **Poisoning.** After a contained panic in an import ([`ImportError::Panicked`]) or a value nested too deep ([`ImportError::ValueTooDeep`]), the document is poisoned. Every method then refuses with [`CrdtError::Poisoned`] or [`ImportError::Poisoned`], or, where it returns no error, answers as for an empty document (an empty version, no stories, nothing to commit); so do the stories, registries, maps, lists and undo managers of the document, also those obtained before. None of them calls the CRDT library any more, whose locks a panic left poisoned. Discard the document and reload the last good state.
#[derive(Debug)]
pub struct Doc {
    loro: LoroDoc,
    /// The replica identifier, kept so that it can be answered without the library.
    peer: PeerId,
    /// Shared with every handle to this document and its undo managers, which must not be freed either once it is poisoned.
    poison: Poison,
}

impl Doc {
    /// A new, empty document for replica `peer`, whose marks expand as `families` say. Every replica must use the same families, and the [`crate::ATOM_KEY`] family is added automatically. Keys of unknown families do not expand.
    ///
    /// # Errors
    ///
    /// [`CrdtError::InvalidFamily`] if a family name is empty, contains a colon, or is given twice, or [`CrdtError::Library`] if the peer identifier is refused (`u64::MAX` is reserved).
    pub fn new(peer: PeerId, families: &[MarkFamily]) -> Result<Self, CrdtError> {
        let loro = LoroDoc::new();
        loro.set_peer_id(peer.0).map_err(CrdtError::library)?;
        configure(&loro, families)?;
        Ok(Self {
            loro,
            peer,
            poison: Poison::default(),
        })
    }

    /// A document loaded from a snapshot or update blob, for replica `peer`, within `limits`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Import`] if the blob is refused, or the errors of [`Doc::new`].
    pub fn load(
        bytes: &[u8],
        peer: PeerId,
        families: &[MarkFamily],
        limits: &ImportLimits,
    ) -> Result<Self, CrdtError> {
        let doc = Self::new(peer, families)?;
        doc.import(bytes, limits).map_err(CrdtError::Import)?;
        Ok(doc)
    }

    /// The replica identifier.
    #[must_use]
    pub const fn peer(&self) -> PeerId {
        self.peer
    }

    /// Ends the current change: everything edited since the last commit becomes one change and one undo step. Does nothing once the document is poisoned.
    pub fn commit(&self) {
        if self.is_poisoned() {
            return;
        }
        self.loro.commit();
    }

    /// Ends the current change without making it an undo step: [`crate::UndoManager`] treats it like a change from another replica, never undoing it but transforming later undo steps against it.
    ///
    /// For changes that create entities before an undoable change references them. Undo then removes only the reference, and the entity stays stored but invisible, as the document model intends (normalization rule N5). Undoing the creation itself is harmful with Loro 1.16.2: redoing the creation of a child container duplicates its content, including edits other replicas made to it in between (CORE-004 report).
    pub fn commit_without_undo(&self) {
        if self.is_poisoned() {
            return;
        }
        self.loro.set_next_commit_origin(NO_UNDO_ORIGIN);
        self.loro.commit();
        // An empty commit leaves the origin waiting for the next commit, which must stay undoable.
        self.loro.clear_next_commit_options();
    }

    /// The main story (once the document is poisoned, a handle that refuses everything).
    #[must_use]
    pub fn main_story(&self) -> Story {
        if self.is_poisoned() {
            return Story::new(LoroText::new(), self.poison.clone());
        }
        Story::new(self.loro.get_text(MAIN_STORY), self.poison.clone())
    }

    /// Every story other than the main one, by identifier (once the document is poisoned, a handle that refuses everything).
    #[must_use]
    pub fn stories(&self) -> Stories {
        if self.is_poisoned() {
            return Stories::new(LoroMap::new(), self.poison.clone());
        }
        Stories::new(self.loro.get_map(STORIES), self.poison.clone())
    }

    /// The registry of entities called `name` (for example `"paragraphs"`).
    ///
    /// The name becomes the name of a root container, which every replica that receives a change to it decodes. The CRDT library accepts any name locally but panics when it decodes a name that is empty, contains `/` or NUL, or starts with its prefix for mergeable containers (`🤝:`) (loro-common 1.16.0), so such names are refused here, before anything is written.
    ///
    /// # Errors
    ///
    /// [`CrdtError::InvalidName`] or [`CrdtError::Poisoned`].
    pub fn registry(&self, name: &str) -> Result<Registry, CrdtError> {
        check_root_name(name)?;
        self.check_poisoned()?;
        Ok(Registry::new(self.loro.get_map(name), self.poison.clone()))
    }

    /// The root property map called `name` (for example `"body"`), for document-wide properties.
    ///
    /// # Errors
    ///
    /// As for [`Doc::registry`].
    pub fn root_map(&self, name: &str) -> Result<PropertyMap, CrdtError> {
        check_root_name(name)?;
        self.check_poisoned()?;
        Ok(PropertyMap::new(
            self.loro.get_map(name),
            self.poison.clone(),
        ))
    }

    /// The latest version this replica knows (empty once the document is poisoned).
    #[must_use]
    pub fn version(&self) -> Version {
        if self.is_poisoned() {
            return Version(Frontiers::default());
        }
        Version(self.loro.oplog_frontiers())
    }

    /// The version vector of everything this replica has received (empty once the document is poisoned).
    #[must_use]
    pub fn version_vector(&self) -> VersionVector {
        if self.is_poisoned() {
            return VersionVector::new();
        }
        VersionVector(self.loro.oplog_vv())
    }

    /// Everything: the current state and the whole history.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`].
    pub fn export_snapshot(&self) -> Result<Vec<u8>, CrdtError> {
        self.check_poisoned()?;
        self.loro
            .export(ExportMode::Snapshot)
            .map_err(CrdtError::library)
    }

    /// The current state with only the history since `since` (older history is trimmed, ADR-0008 §6). Replicas that have not seen `since` cannot merge with it.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`].
    pub fn export_shallow_snapshot(&self, since: &Version) -> Result<Vec<u8>, CrdtError> {
        self.check_poisoned()?;
        self.loro
            .export(ExportMode::shallow_snapshot(&since.0))
            .map_err(CrdtError::library)
    }

    /// The changes this replica has that a replica at version vector `since` lacks.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`].
    pub fn export_updates(&self, since: &VersionVector) -> Result<Vec<u8>, CrdtError> {
        self.check_poisoned()?;
        self.loro
            .export(ExportMode::updates(&since.0))
            .map_err(CrdtError::library)
    }

    /// Imports a blob from another replica or from storage, within `limits`. Changes that depend on changes not received yet wait and are applied when those arrive, so blobs may arrive in any order.
    ///
    /// Every blob that does not come from this device is untrusted input, and the limits narrow what it can do without making the CRDT library safe (see [`ImportLimits`] and the crate documentation): a crafted blob can make Loro 1.16.2 panic, which is contained here where panics unwind and poisons the document; it can make it abort the process, which nothing here can catch; and **the panic message, which Rust's panic hook prints before the panic is contained, can quote document text**, whichever crate raises it, so the host must install a panic hook that never prints or records any panic's message before it imports untrusted blobs.
    ///
    /// # Errors
    ///
    /// [`ImportError`] when the blob is refused; with [`ImportError::ValueTooDeep`] and [`ImportError::Panicked`] the document is poisoned and must be discarded.
    pub fn import(&self, bytes: &[u8], limits: &ImportLimits) -> Result<ImportReport, ImportError> {
        if self.is_poisoned() {
            return Err(ImportError::Poisoned);
        }
        // Loro 1.16.2 panics on many crafted blobs whose checksum is correct (CORE-004 report). Such a panic is contained here, where the document is known: its state may be half updated and the library's locks are poisoned, so the document is poisoned too, which also means it is never freed, because freeing it can panic again. Where panics abort instead of unwinding (WebAssembly builds), the host must restart the engine and reload the last good state.
        std::panic::catch_unwind(AssertUnwindSafe(|| self.import_unguarded(bytes, limits)))
            .unwrap_or_else(|_| {
                self.poison.set();
                Err(ImportError::Panicked)
            })
    }

    fn import_unguarded(
        &self,
        bytes: &[u8],
        limits: &ImportLimits,
    ) -> Result<ImportReport, ImportError> {
        let mut report = check_header(bytes, limits)?;
        let status = self
            .loro
            .import(bytes)
            .map_err(|error| ImportError::Rejected(error.to_string()))?;
        if limits.inspect_values && imported_values_too_deep(&self.loro, &status) {
            self.poison.set();
            return Err(ImportError::ValueTooDeep);
        }
        report.pending = status.pending.is_some();
        Ok(report)
    }

    /// Shows the document as it was at `version` (read-only until [`Doc::checkout_latest`]).
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`] if this replica does not have that version.
    pub fn checkout(&self, version: &Version) -> Result<(), CrdtError> {
        self.check_poisoned()?;
        self.loro.checkout(&version.0).map_err(CrdtError::library)
    }

    /// Returns to the latest version after [`Doc::checkout`]. Does nothing once the document is poisoned.
    pub fn checkout_latest(&self) {
        if self.is_poisoned() {
            return;
        }
        self.loro.checkout_to_latest();
    }

    /// Whether the document shows an older version (see [`Doc::checkout`]); false once the document is poisoned.
    #[must_use]
    pub fn is_detached(&self) -> bool {
        !self.is_poisoned() && self.loro.is_detached()
    }

    /// Whether a hostile import poisoned the document (see [`ImportError::ValueTooDeep`] and [`ImportError::Panicked`]).
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poison.is_set()
    }

    /// Size counters (zero once the document is poisoned).
    #[must_use]
    pub fn stats(&self) -> DocStats {
        if self.is_poisoned() {
            return DocStats { ops: 0, changes: 0 };
        }
        DocStats {
            ops: self.loro.len_ops(),
            changes: self.loro.len_changes(),
        }
    }

    pub(crate) const fn loro(&self) -> &LoroDoc {
        &self.loro
    }

    pub(crate) fn poison(&self) -> Poison {
        self.poison.clone()
    }

    fn check_poisoned(&self) -> Result<(), CrdtError> {
        self.poison.check()
    }
}

/// Refuses a root container name that the CRDT library would accept here but panic on when another replica decodes it (see [`Doc::registry`]).
fn check_root_name(name: &str) -> Result<(), CrdtError> {
    if name.is_empty() || name.contains(['/', '\0']) || name.starts_with("🤝:") {
        Err(CrdtError::InvalidName(name.to_owned()))
    } else {
        Ok(())
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        if self.is_poisoned() {
            // A poisoned document holds a value whose release would recurse once per nesting level and could exhaust the stack, or a state that a panic left half updated, whose release can panic again (and a panic while another unwinds aborts the process). Keeping one reference forever means its memory is never released, which is the lesser harm; it happens only after hostile input.
            std::mem::forget(self.loro.clone());
        }
    }
}

/// Configures how marks expand, by family, with the atom binding never expanding and keys of unknown families not expanding either.
fn configure(loro: &LoroDoc, families: &[MarkFamily]) -> Result<(), CrdtError> {
    let mut config = StyleConfigMap::new();
    let mut seen: Vec<&str> = Vec::new();
    for family in families.iter().chain([&ATOM_FAMILY]) {
        if family.name.is_empty() || family.name.contains(':') || seen.contains(&family.name) {
            return Err(CrdtError::InvalidFamily(family.name.to_owned()));
        }
        seen.push(family.name);
        config.insert(
            family.name.into(),
            StyleConfig {
                expand: expand_type(family.expand),
            },
        );
    }
    loro.config_text_style(config);
    loro.config_default_text_style(Some(StyleConfig {
        expand: ExpandType::None,
    }));
    Ok(())
}

const fn expand_type(expand: Expand) -> ExpandType {
    match expand {
        Expand::None => ExpandType::None,
        Expand::Before => ExpandType::Before,
        Expand::After => ExpandType::After,
        Expand::Both => ExpandType::Both,
    }
}
