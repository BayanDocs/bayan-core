//! The engine's resource limits ([engine protocol specification][spec] §13, ADR-0006 §5).
//!
//! Every message, blob, tile and recording is untrusted input, so each has a hard limit, checked before the engine allocates or parses anything in proportion to it.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

/// The largest message, in either direction: 16 MiB.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// The largest blob: 64 MiB, which holds the largest tile (4,096 × 4,096 pixels of 4 bytes).
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

/// The most bytes all blobs together may hold: 256 MiB.
pub const MAX_TOTAL_BLOB_BYTES: usize = 256 * 1024 * 1024;

/// The most blobs that may exist at once.
pub const MAX_BLOBS: usize = 1024;

/// The most documents that may be open at once.
pub const MAX_DOCUMENTS: usize = 16;

/// The largest width and height of a tile, in pixels.
pub const MAX_TILE_SIDE: u32 = 4096;

/// The largest magnitude of a tile rectangle's coordinates, and the largest width and height, in BLU: 2⁴⁰ BLU, about 1,100 kilometres, so the rasterizer's integer arithmetic cannot overflow.
pub const MAX_TILE_COORDINATE: i64 = 1 << 40;

/// The most entries a recording may hold.
pub const MAX_RECORDING_ENTRIES: usize = 100_000;

/// The most pixels one replay may render, in all its tiles together: 2³⁰, as many as 64 tiles of the largest size, about 20 seconds of rendering natively. A recording is small for the work it can ask for (a tile entry of about 150 bytes can ask for 4,096 × 4,096 pixels), so without this limit one recording of 15 MB could keep the engine busy for hours.
pub const MAX_REPLAY_PIXELS: u64 = 1 << 30;

/// The largest recording, as JSON: 64 MiB, so it fits in one blob.
pub const MAX_RECORDING_BYTES: usize = MAX_BLOB_BYTES;

/// The largest identifier of a document, blob, request or event: 2⁵³ − 1, so that JavaScript numbers hold every identifier exactly.
pub const MAX_ID: u64 = (1 << 53) - 1;

/// The smallest zoom factor `view.set` accepts.
pub const MIN_ZOOM: f64 = 0.05;

/// The largest zoom factor `view.set` accepts.
pub const MAX_ZOOM: f64 = 64.0;

/// The largest device scale `view.set` accepts.
pub const MAX_DEVICE_SCALE: f64 = 16.0;

/// The largest magnitude of a viewport or pointer coordinate, in device-independent pixels.
pub const MAX_VIEW_COORDINATE: f64 = 1.0e9;

/// The longest text of the mock document's editable line, in UTF-16 code units.
pub const MAX_EDITABLE_TEXT_UNITS: usize = 1024;

/// The longest input-method composition in the editable line, in UTF-16 code units.
pub const MAX_COMPOSITION_UNITS: usize = 256;
