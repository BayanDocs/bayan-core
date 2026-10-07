//! Importing changes from other replicas, which are untrusted input (ADR-0008 §4), within resource limits.

use std::fmt;

use loro::{
    IdSpan, ImportStatus, JsonChange, JsonListOp, JsonMapOp, JsonMovableListOp, JsonOpContent,
    JsonTextOp, LoroDoc, LoroValue,
};

use crate::value::{MAX_VALUE_DEPTH, loro_depth_exceeds};

/// Resource limits for one import.
///
/// The checks run in two stages. Before anything is decoded, the adapter reads the blob's header (format, number of changes, and the range of operations of each replica) and rejects blobs that are too big, in a format it does not accept, or that announce too much work. After a successful import, if [`ImportLimits::inspect_values`] is set, it walks every value that the imported changes contain (map entries, list items, mark values), without recursion, and rejects the import if one is nested deeper than [`MAX_VALUE_DEPTH`]: the CRDT library frees and copies such values recursively, so a deeply nested value can exhaust the stack later, for example when the document is dropped (see [`ImportError::ValueTooDeep`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportLimits {
    /// The largest blob accepted, in bytes.
    pub max_bytes: usize,
    /// The most changes (commits) one blob may contain.
    pub max_changes: u32,
    /// The most operations one blob may contain (inserted characters, marks, map writes and so on each count).
    pub max_ops: u64,
    /// Whether full snapshots are accepted, or only incremental updates.
    pub accept_snapshots: bool,
    /// Whether to inspect the values of the imported changes after the import.
    pub inspect_values: bool,
}

impl ImportLimits {
    /// Limits for an incremental update received from another replica: at most 1 MiB, 100,000 changes and 2,000,000 operations, no snapshots, values inspected. (A document is shared initially as a series of updates; the size limit keeps the nesting depth that one blob can carry within what the engine's stack can afford, see the CORE-004 report.)
    pub const UPDATE: Self = Self {
        max_bytes: 1 << 20,
        max_changes: 100_000,
        max_ops: 2_000_000,
        accept_snapshots: false,
        inspect_values: true,
    };

    /// Limits for a snapshot that this device wrote itself (auto-recovery journal, version history): at most 256 MiB and 50,000,000 operations, values not inspected.
    pub const LOCAL_SNAPSHOT: Self = Self {
        max_bytes: 256 << 20,
        max_changes: 10_000_000,
        max_ops: 50_000_000,
        accept_snapshots: true,
        inspect_values: false,
    };
}

/// What a successful import brought in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    /// The number of changes the blob announced.
    pub changes: u32,
    /// The number of operations the blob announced.
    pub ops: u64,
    /// Whether some changes wait for changes this replica has not received yet (they are applied as soon as those arrive).
    pub pending: bool,
}

/// Why an import was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// The blob is bigger than [`ImportLimits::max_bytes`].
    TooLarge {
        /// Its size.
        bytes: usize,
        /// The limit.
        limit: usize,
    },
    /// The blob's header could not be read, its checksum is wrong, or its format is outdated.
    Malformed(String),
    /// The blob is a snapshot, and only updates are accepted here.
    SnapshotNotAccepted,
    /// The blob announces more changes than [`ImportLimits::max_changes`].
    TooManyChanges {
        /// The number announced.
        changes: u32,
        /// The limit.
        limit: u32,
    },
    /// The blob announces more operations than [`ImportLimits::max_ops`].
    TooManyOps {
        /// The number announced.
        ops: u64,
        /// The limit.
        limit: u64,
    },
    /// The CRDT library refused the blob while decoding or applying it.
    Rejected(String),
    /// The imported changes contain a value nested deeper than [`MAX_VALUE_DEPTH`]. The document now holds that value, and freeing it would recurse once per level, so the document is **poisoned**: it refuses further imports and exports, and when it is dropped its memory is deliberately leaked instead of freed. Discard it and reload the last good state (for example the last snapshot) without the offending update.
    ValueTooDeep,
    /// The document was poisoned by an earlier import (see [`ImportError::ValueTooDeep`]).
    Poisoned,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "the blob has {bytes} bytes; the limit is {limit}"
                )
            }
            Self::Malformed(reason) => write!(formatter, "the blob is malformed: {reason}"),
            Self::SnapshotNotAccepted => formatter.write_str("snapshots are not accepted here"),
            Self::TooManyChanges { changes, limit } => {
                write!(
                    formatter,
                    "the blob has {changes} changes; the limit is {limit}"
                )
            }
            Self::TooManyOps { ops, limit } => {
                write!(
                    formatter,
                    "the blob has {ops} operations; the limit is {limit}"
                )
            }
            Self::Rejected(reason) => {
                write!(formatter, "the CRDT library refused the blob: {reason}")
            }
            Self::ValueTooDeep => write!(
                formatter,
                "the blob contains a value nested deeper than {MAX_VALUE_DEPTH} levels; the document is poisoned"
            ),
            Self::Poisoned => formatter.write_str("the document was poisoned by an earlier import"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Checks a blob's header against the limits, before anything is decoded.
pub(crate) fn check_header(
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<ImportReport, ImportError> {
    if bytes.len() > limits.max_bytes {
        return Err(ImportError::TooLarge {
            bytes: bytes.len(),
            limit: limits.max_bytes,
        });
    }
    // Reads the header and verifies the checksum; outdated formats (whose decoders are not fuzzed by us) are refused here.
    let meta = LoroDoc::decode_import_blob_meta(bytes, true)
        .map_err(|error| ImportError::Malformed(error.to_string()))?;
    if meta.mode.is_snapshot() && !limits.accept_snapshots {
        return Err(ImportError::SnapshotNotAccepted);
    }
    if meta.change_num > limits.max_changes {
        return Err(ImportError::TooManyChanges {
            changes: meta.change_num,
            limit: limits.max_changes,
        });
    }
    let ops: u64 = meta
        .partial_end_vv
        .iter()
        .map(|(peer, end)| {
            let start = meta.partial_start_vv.get(peer).copied().unwrap_or(0);
            u64::try_from(i64::from(*end) - i64::from(start)).unwrap_or(0)
        })
        .fold(0_u64, u64::saturating_add);
    if ops > limits.max_ops {
        return Err(ImportError::TooManyOps {
            ops,
            limit: limits.max_ops,
        });
    }
    Ok(ImportReport {
        changes: meta.change_num,
        ops,
        pending: false,
    })
}

/// Whether any value carried by the changes that `status` reports as imported is nested deeper than [`MAX_VALUE_DEPTH`]. The changes are read back as the library's JSON change records, which hold the values as they were imported, including values that a later change in the same blob overwrote or deleted (those stay in the history and are freed with the document).
pub(crate) fn imported_values_too_deep(doc: &LoroDoc, status: &ImportStatus) -> bool {
    let spans: Vec<IdSpan> = status
        .success
        .iter()
        .map(|(peer, (start, end))| IdSpan::new(*peer, *start, *end))
        .collect();
    for span in spans {
        let changes = doc.export_json_in_id_span(span);
        let too_deep = changes.iter().any(change_too_deep);
        if too_deep {
            // The copies in `changes` may be the only references to the deep values, and dropping them would recurse once per level: leak them instead (this path runs only for hostile input).
            std::mem::forget(changes);
            return true;
        }
    }
    false
}

fn change_too_deep(change: &JsonChange) -> bool {
    change.ops.iter().any(|op| {
        let too_deep = |value: &LoroValue| loro_depth_exceeds(value, MAX_VALUE_DEPTH);
        match &op.content {
            JsonOpContent::Map(JsonMapOp::Insert { value, .. })
            | JsonOpContent::MovableList(JsonMovableListOp::Set { value, .. })
            | JsonOpContent::Text(JsonTextOp::Mark {
                style_value: value, ..
            }) => too_deep(value),
            JsonOpContent::List(JsonListOp::Insert { value, .. })
            | JsonOpContent::MovableList(JsonMovableListOp::Insert { value, .. }) => {
                value.iter().any(too_deep)
            }
            _ => false,
        }
    })
}
