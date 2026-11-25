//! The messages of engine protocol v1 as Rust types ([engine protocol specification][spec] §4–§6 and the changes of §14).
//!
//! Every payload that crosses the engine boundary is defined here once. serde turns these types into JSON and back, and schemars derives the JSON Schema from the same definitions ([`crate::schema`]), from which [`crate::typescript`] writes the TypeScript declarations for the web shell, so the schema and the declarations cannot drift from the code (ADR-0012 §1).
//!
//! Types the shell sends ([`Hello`], [`DocOpen`], …) accept missing optional fields and ignore unknown ones (spec §9). Types the engine sends ([`Welcome`], [`ViewPages`], …) always contain every field that is not marked optional. All geometry is in BLU (ADR-0005) and all text offsets are UTF-16 code units (spec §7).
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

/// The protocol version that this engine speaks, and the only one (spec §9, §14.1): every message it sends carries it in `v`, and it refuses every message with another `v` except `hello`, which it reads whatever its `v`.
pub const PROTOCOL_VERSION: u32 = 1;

/// The layout epoch (ADR-0004 §4): 0 until the first real layout engine changes layout output.
pub const LAYOUT_EPOCH: u32 = 0;

/// The features that `welcome` reports.
pub const FEATURES: [&str; 2] = ["mock_document", "record_replay"];

/// The clipboard formats that the engine reads and writes, as MIME types, preferred first (spec §14.5): `welcome` lists them, `input.paste` reads only these, and `clipboard.copy` writes only these. CORE-125 adds `text/html` for copying.
pub const CLIPBOARD_FORMATS: [&str; 1] = ["text/plain"];

/// A rectangle in BLU (1/25,400 point, ADR-0005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    /// The left edge.
    pub x: i64,
    /// The top edge.
    pub y: i64,
    /// The width; positive.
    pub width: i64,
    /// The height; positive.
    pub height: i64,
}

/// A rectangle on a page, in that page's coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageRect {
    /// The page's index, counting from 0.
    pub page: u32,
    /// The rectangle, in BLU from the page's top-left corner.
    pub rect: Rect,
}

/// A blob: bulk bytes kept outside the JSON (spec §3). On the C interface, its identifier, a positive integer; on the web, the bytes themselves as an `ArrayBuffer` (spec §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
#[schemars(extend("x-bayan-web-type" = "ArrayBuffer"))]
pub struct BlobRef(pub u64);

/// A payload without fields: the payload of requests that need none, and the reply to requests that only succeed or fail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

/// The modifier keys held during an input event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Modifiers {
    /// Shift.
    #[serde(default)]
    pub shift: bool,
    /// Control.
    #[serde(default)]
    pub ctrl: bool,
    /// Alt (Option on macOS).
    #[serde(default)]
    pub alt: bool,
    /// Meta (Command on macOS, the Windows key on Windows).
    #[serde(default)]
    pub meta: bool,
}

/// A range of UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextRange {
    /// The first code unit.
    pub start: u32,
    /// The code unit after the last one; equal to `start` for an empty range (a caret).
    pub end: u32,
}

/// One value in the `args` of an error or warning: only codes and numbers, never document content (spec §12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ArgValue {
    /// A code or a short identifier, such as a message type.
    Text(String),
    /// A number.
    Integer(i64),
    /// A flag.
    Flag(bool),
    /// A list of numbers.
    Integers(Vec<i64>),
}

/// The arguments of an error or a warning.
pub type Args = BTreeMap<String, ArgValue>;

/// What went wrong (spec §12).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Not a JSON object of the envelope's shape, or not UTF-8.
    InvalidMessage,
    /// A message other than `hello` whose `v` is not the protocol version, or a `hello` that offers no common version.
    UnsupportedProtocolVersion,
    /// A message other than `hello` before the handshake.
    HandshakeRequired,
    /// An unknown message type, or one this engine does not implement.
    UnsupportedMessage,
    /// The payload does not have the message's shape, or a value is out of range.
    InvalidRequest,
    /// An unknown document, blob, page or node.
    NotFound,
    /// A limit of spec §13.
    LimitExceeded,
    /// `diag.record.start` while a recording is running.
    AlreadyRecording,
    /// `diag.record.stop` while no recording is running.
    NotRecording,
    /// `diag.replay` cannot read the recording.
    InvalidRecording,
    /// The recording comes from another engine version, protocol version or layout epoch.
    RecordingMismatch,
    /// `diag.panic` without its test switch, or `diag.replay` while recording.
    NotAllowed,
    /// The engine panicked while handling the message.
    Panic,
    /// Any other failure inside the engine.
    Internal,
}

impl ErrorCode {
    /// The code as it appears in JSON, such as `not_found`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidMessage => "invalid_message",
            Self::UnsupportedProtocolVersion => "unsupported_protocol_version",
            Self::HandshakeRequired => "handshake_required",
            Self::UnsupportedMessage => "unsupported_message",
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::LimitExceeded => "limit_exceeded",
            Self::AlreadyRecording => "already_recording",
            Self::NotRecording => "not_recording",
            Self::InvalidRecording => "invalid_recording",
            Self::RecordingMismatch => "recording_mismatch",
            Self::NotAllowed => "not_allowed",
            Self::Panic => "panic",
            Self::Internal => "internal",
        }
    }

    /// The Fluent message identifier that shells format for people (ADR-0021), such as `engine-error-not-found`.
    #[must_use]
    pub fn message_id(self) -> String {
        format!("engine-error-{}", self.as_str().replace('_', "-"))
    }
}

/// The error of a failed reply (spec §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorInfo {
    /// What went wrong, for programs.
    pub code: ErrorCode,
    /// A Fluent message identifier, for people.
    pub message_id: String,
    /// Codes and numbers that explain the error; never document content.
    pub args: Args,
}

impl ErrorInfo {
    /// An error without arguments.
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message_id: code.message_id(),
            args: Args::new(),
        }
    }

    /// Adds an argument that is a code or a short identifier.
    #[must_use]
    pub fn with_text(mut self, name: &str, value: &str) -> Self {
        self.args
            .insert(name.to_owned(), ArgValue::Text(value.to_owned()));
        self
    }

    /// Adds an argument that is a list of numbers.
    #[must_use]
    pub fn with_integers(mut self, name: &str, values: Vec<i64>) -> Self {
        self.args
            .insert(name.to_owned(), ArgValue::Integers(values));
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Handshake and documents (spec §5, §6.1)
// ---------------------------------------------------------------------------------------------

/// `hello`: the shell starts a session (spec §5, §14.1). The engine reads it whatever its envelope's `v`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    /// The protocol versions the shell speaks; must contain 1, the only version this engine speaks.
    pub protocol_versions: Vec<u32>,
    /// Which shell this is.
    #[serde(default)]
    pub shell: Option<ShellInfo>,
    /// What the shell can do.
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    /// The interface language, as a BCP 47 tag.
    #[serde(default)]
    pub locale: Option<String>,
    /// The interface theme, such as `light` or `dark`.
    #[serde(default)]
    pub theme: Option<String>,
}

/// Which shell started the session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellInfo {
    /// The shell's name, such as `bayan-desktop`.
    #[serde(default)]
    pub name: String,
    /// The shell's version.
    #[serde(default)]
    pub version: String,
    /// The operating system whose keyboard conventions apply (spec §14.1); a browser shell names the system it runs on, as far as the browser tells it. A missing or unknown value counts as `other`.
    #[serde(default)]
    pub platform: Platform,
    /// Whether the shell runs natively or in a browser (spec §14.1). A missing or unknown value counts as `native`.
    #[serde(default)]
    pub runtime: Runtime,
}

/// The operating system whose keyboard conventions apply (spec §14.1). The engine uses it only to resolve the UI manifest's key bindings (spec §14.3); nothing that affects layout or rendering depends on it (ADR-0004).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
    /// iOS and iPadOS.
    Ios,
    /// Android.
    Android,
    /// ChromeOS.
    Chromeos,
    /// Any other system, and any value the engine does not know; Ctrl stays Ctrl.
    #[default]
    #[serde(other)]
    Other,
}

impl Platform {
    /// Every platform, in the order of the specification's list.
    pub const ALL: [Self; 7] = [
        Self::Windows,
        Self::Macos,
        Self::Linux,
        Self::Ios,
        Self::Android,
        Self::Chromeos,
        Self::Other,
    ];

    /// The platform as it appears in JSON, such as `macos`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Ios => "ios",
            Self::Android => "android",
            Self::Chromeos => "chromeos",
            Self::Other => "other",
        }
    }
}

/// Whether the shell runs natively or in a browser (spec §14.1), which keeps keys that a native app does not (spec §14.3).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// A web page in a browser.
    Browser,
    /// A native application, and any value the engine does not know.
    #[default]
    #[serde(other)]
    Native,
}

impl Runtime {
    /// Both runtimes.
    pub const ALL: [Self; 2] = [Self::Native, Self::Browser];

    /// The runtime as it appears in JSON, such as `browser`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Browser => "browser",
        }
    }
}

/// What the shell can do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// The clipboard formats the shell can exchange, as MIME types.
    #[serde(default)]
    pub clipboard_formats: Vec<String>,
    /// Whether the shell forwards input-method composition.
    #[serde(default)]
    pub ime: bool,
    /// Whether the shell bridges the accessibility tree to the platform.
    #[serde(default)]
    pub accessibility: bool,
}

/// `welcome`: the reply to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Welcome {
    /// The protocol version of the session: 1.
    pub protocol_version: u32,
    /// The core's version as Semantic Versioning 2.0 writes it, at most 64 ASCII characters (spec §14.1).
    pub engine_version: String,
    /// The layout epoch (ADR-0004 §4).
    pub layout_epoch: u32,
    /// What this engine offers, such as `mock_document`.
    pub features: Vec<String>,
    /// The clipboard formats that the engine reads and writes, as MIME types, preferred first (spec §14.5): `text/plain`. The engine always sends it; an engine of protocol v1 from before CORE-014 does not, and reads and writes no clipboard formats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard_formats: Option<Vec<String>>,
}

/// `doc.open`: opens a document from a blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocOpen {
    /// The document's bytes. Until real documents can be opened, the engine opens the mock document whatever they contain.
    pub blob: BlobRef,
    /// The format, if the shell knows it, such as `docx`.
    #[serde(default)]
    pub format_hint: Option<String>,
    /// The file name, for format detection and display only; never logged.
    #[serde(default)]
    pub file_name_hint: Option<String>,
}

/// `doc.opened`: the reply to `doc.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocOpened {
    /// The new document's identifier.
    pub doc_id: u64,
    /// The document's render revision (spec §14.6): a positive integer below 2⁵³ that grows with every change to what any of the document's tiles shows.
    pub revision: u64,
    /// How many pages the document is expected to have.
    pub page_count_estimate: u32,
    /// Problems found while opening, as Fluent messages.
    pub warnings: Vec<Warning>,
    /// Fonts the document asks for that are missing.
    pub fonts: FontReport,
}

/// A problem found while opening a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Warning {
    /// A Fluent message identifier.
    pub message_id: String,
    /// Codes and numbers for the message.
    pub args: Args,
}

/// Fonts a document asks for that the engine does not have.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FontReport {
    /// The missing font families.
    pub missing: Vec<String>,
    /// Whether the layout depends on a font found only among the user's installed fonts (ADR-0004 §3).
    pub machine_dependent: bool,
}

/// `doc.close`: closes a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocClose {
    /// The document.
    pub doc_id: u64,
}

// ---------------------------------------------------------------------------------------------
// View (spec §6.2, §6.3, §6.4)
// ---------------------------------------------------------------------------------------------

/// `view.set`: tells the engine what the shell shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewSet {
    /// The document.
    pub doc_id: u64,
    /// The visible part of the page area, in device-independent pixels from the view origin.
    pub viewport: Viewport,
    /// The zoom factor: 1 shows a document at its real size.
    pub zoom: f64,
    /// Physical pixels per device-independent pixel.
    pub device_scale: f64,
    /// The view mode.
    pub mode: ViewMode,
    /// Whether to show paragraph marks and other formatting marks.
    #[serde(default)]
    pub show_formatting_marks: bool,
}

/// A rectangle in device-independent pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Viewport {
    /// The left edge.
    pub x: f64,
    /// The top edge.
    pub y: f64,
    /// The width.
    pub width: f64,
    /// The height.
    pub height: f64,
}

/// How pages are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    /// Print layout: pages as they print.
    Print,
    /// Web layout.
    Web,
    /// Draft view.
    Draft,
    /// Focus mode (ADR-0019 §4).
    Focus,
}

/// `view.pages`: the document's pages and where they sit in view space (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewPages {
    /// The document.
    pub doc_id: u64,
    /// The pages, in order.
    pub pages: Vec<PageInfo>,
}

/// One page of `view.pages`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageInfo {
    /// The page's index, counting from 0.
    pub index: u32,
    /// The page's width.
    pub width: i64,
    /// The page's height.
    pub height: i64,
    /// The index of the page's section.
    pub section: u32,
    /// The page's left edge in view space.
    pub x: i64,
    /// The page's top edge in view space.
    pub y: i64,
}

/// `render.invalidate`: parts of pages whose tiles the shell must request again (spec §14.6). A shell keeps a tile while no invalidation with a higher revision covers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderInvalidate {
    /// The document.
    pub doc_id: u64,
    /// The document's render revision after the change.
    pub revision: u64,
    /// The regions changed since the previous revision, in page coordinates.
    pub regions: Vec<PageRect>,
}

/// `view.layout.progress`: how far layout has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewLayoutProgress {
    /// The document.
    pub doc_id: u64,
    /// The number of pages laid out so far.
    pub pages_laid_out: u32,
    /// How many pages the document is expected to have.
    pub page_count_estimate: u32,
    /// Whether layout is complete.
    pub complete: bool,
}

/// `render.tile`: asks for the pixels of part of a page (spec §6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderTile {
    /// The document.
    pub doc_id: u64,
    /// The page's index.
    pub page: u32,
    /// The part of the page, in page coordinates.
    pub rect: Rect,
    /// The tile's width in pixels, 1 to 4,096.
    pub width: u32,
    /// The tile's height in pixels, 1 to 4,096.
    pub height: u32,
    /// The zoom factor, a hint for later rasterizers; it does not change the pixels.
    #[serde(default)]
    pub zoom: Option<f64>,
    /// Physical pixels per device-independent pixel, a hint for later rasterizers; it does not change the pixels.
    #[serde(default)]
    pub device_scale: Option<f64>,
    /// The rasterizer to use (ADR-0011 §3); both give the same pixels until the real rasterizers exist.
    #[serde(default)]
    pub mode: Option<RenderMode>,
}

/// The request of `bayan_render_tile` on the C interface (spec §3.1): a `render.tile` payload whose `width` and `height` may be left out, because the function's parameters give them; if present, they must equal the parameters. It is not a message, so it is not in the schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileRequest {
    /// The document.
    pub doc_id: u64,
    /// The page's index.
    pub page: u32,
    /// The part of the page, in page coordinates.
    pub rect: Rect,
    /// The tile's width in pixels; `bayan_render_tile`'s parameter gives it.
    #[serde(default)]
    pub width: Option<u32>,
    /// The tile's height in pixels; `bayan_render_tile`'s parameter gives it.
    #[serde(default)]
    pub height: Option<u32>,
    /// As in [`RenderTile`].
    #[serde(default)]
    pub zoom: Option<f64>,
    /// As in [`RenderTile`].
    #[serde(default)]
    pub device_scale: Option<f64>,
    /// As in [`RenderTile`].
    #[serde(default)]
    pub mode: Option<RenderMode>,
}

impl TileRequest {
    /// The `render.tile` payload this request stands for, with the size `bayan_render_tile` was called with, or `None` if the request names another size.
    #[must_use]
    pub fn with_size(self, width: u32, height: u32) -> Option<RenderTile> {
        let fits = |given: Option<u32>, size: u32| given.is_none_or(|given| given == size);
        (fits(self.width, width) && fits(self.height, height)).then_some(RenderTile {
            doc_id: self.doc_id,
            page: self.page,
            rect: self.rect,
            width,
            height,
            zoom: self.zoom,
            device_scale: self.device_scale,
            mode: self.mode,
        })
    }
}

/// Which rasterizer renders a tile (ADR-0011 §2–§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// The bit-exact reference rasterizer.
    Reference,
    /// The interactive rasterizer.
    Interactive,
}

/// The reply to `render.tile` in the message form, on the C interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderTileReply {
    /// The tile's width in pixels.
    pub width: u32,
    /// The tile's height in pixels.
    pub height: u32,
    /// Bytes from the start of one row to the next: 4 × width.
    pub stride: u32,
    /// The digest of the pixels, `fnv1a64:` and 16 hexadecimal digits (spec §10).
    pub hash: String,
    /// The render revision that the tile shows (spec §14.6).
    pub revision: u64,
    /// The pixels, premultiplied RGBA8, rows from top to bottom, as a blob that the shell releases.
    pub blob: BlobRef,
}

/// The reply to `render.tile` as the web worker host delivers it (spec §3.2): the pixels as an `ImageBitmap`. This type exists for the TypeScript declarations; no JSON has this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderTileWebReply {
    /// The tile's width in pixels.
    pub width: u32,
    /// The tile's height in pixels.
    pub height: u32,
    /// The digest of the engine's premultiplied pixels (spec §10).
    pub hash: String,
    /// The render revision that the tile shows (spec §14.6).
    pub revision: u64,
    /// The pixels.
    pub bitmap: ImageBitmapRef,
}

/// An `ImageBitmap` transferred from the web worker (spec §3.2); only in the TypeScript declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(extend("x-bayan-web-type" = "ImageBitmap"))]
pub struct ImageBitmapRef {}

/// `overlay.update`: what the shell draws over the tiles (spec §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OverlayUpdate {
    /// The document.
    pub doc_id: u64,
    /// The render revision that the rectangles belong to (spec §14.6).
    pub revision: u64,
    /// The caret, or `null` when there is none.
    pub caret: Option<Caret>,
    /// The selection's rectangles, per page.
    pub selection: Vec<SelectionRects>,
    /// Other people's cursors (Phase 3); always empty so far.
    pub remote: Vec<serde_json::Value>,
    /// Find highlights (Phase 2); always empty so far.
    pub highlights: Vec<serde_json::Value>,
}

/// The caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Caret {
    /// The page it is on.
    pub page: u32,
    /// Its rectangle, in page coordinates.
    pub rect: Rect,
    /// Whether the shell should show it.
    pub visible: bool,
}

/// Selection rectangles on one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SelectionRects {
    /// The page.
    pub page: u32,
    /// The rectangles, in page coordinates.
    pub rects: Vec<Rect>,
}

// ---------------------------------------------------------------------------------------------
// Input (spec §6.5)
// ---------------------------------------------------------------------------------------------

/// `input.pointer`: a mouse, pen or touch event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InputPointer {
    /// The document.
    pub doc_id: u64,
    /// What happened.
    pub kind: PointerKind,
    /// The horizontal position, in device-independent pixels from the view origin.
    pub x: f64,
    /// The vertical position, in device-independent pixels from the view origin.
    pub y: f64,
    /// The button, numbered as the DOM's `button`: 0 is the primary button.
    #[serde(default)]
    pub button: i32,
    /// The click count: 2 for a double click.
    #[serde(default)]
    pub clicks: u32,
    /// The modifier keys held.
    #[serde(default)]
    pub modifiers: Modifiers,
    /// The kind of pointer.
    #[serde(default)]
    pub pointer_type: Option<PointerType>,
}

/// What a pointer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    /// Pressed.
    Down,
    /// Moved.
    Move,
    /// Released.
    Up,
    /// The platform cancelled the gesture.
    Cancel,
}

/// The kind of pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerType {
    /// A mouse or touchpad.
    Mouse,
    /// A pen or stylus.
    Pen,
    /// A finger on a touch screen.
    Touch,
}

/// `input.key`: a key press.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputKey {
    /// The document.
    pub doc_id: u64,
    /// The key's meaning, as the DOM's `KeyboardEvent.key`, such as `Backspace` or `ArrowLeft`.
    pub key: String,
    /// The physical key, as the DOM's `KeyboardEvent.code`, such as `KeyA`.
    #[serde(default)]
    pub code: String,
    /// The modifier keys held.
    #[serde(default)]
    pub modifiers: Modifiers,
    /// Whether the key repeats because it is held down.
    #[serde(default)]
    pub repeat: bool,
    /// Whether an input method composition owns the key; the engine then ignores it.
    #[serde(default)]
    pub composing: bool,
}

/// `input.text`: committed text (spec §6.5, §14.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputText {
    /// The document.
    pub doc_id: u64,
    /// The text. Without `replace`, it replaces the selection, or is inserted at the caret when nothing is selected; an active composition ends, and the text takes its place.
    pub text: String,
    /// The range of a node's text that `text` replaces; the caret ends after the new text (spec §14.4). A node that does not exist is `not_found`; a node that is not editable, a range outside its text or backwards, or an end inside a surrogate pair is `invalid_request`.
    #[serde(default)]
    pub replace: Option<TextReplace>,
}

/// The range of a node's text that `input.text` replaces (spec §14.4), in UTF-16 code units of the node's text as the engine last sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextReplace {
    /// The accessibility node whose text it is.
    pub node: u64,
    /// The first code unit of the range.
    pub start: u32,
    /// The code unit after the range; equal to `start` to insert without replacing.
    pub end: u32,
}

/// A position in the text of an accessibility node (spec §14.4): the node, and an offset in UTF-16 code units into its `text`, never inside a surrogate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextPosition {
    /// The node.
    pub node: u64,
    /// The offset, in UTF-16 code units into the node's text.
    pub offset: u32,
}

/// `input.select`: sets the selection, which is a caret where `anchor` and `focus` are equal (spec §14.4). The engine answers with `overlay.update` and `a11y.update` when the selection changes. A node that does not exist is `not_found`; a node that is not editable, or an offset beyond its node's text or inside a surrogate pair, is `invalid_request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputSelect {
    /// The document.
    pub doc_id: u64,
    /// Where the selection starts: the end that stays when it is extended.
    pub anchor: TextPosition,
    /// Where the selection ends: the end that moves, and where the caret is.
    pub focus: TextPosition,
}

/// `query.hit`: asks which text position lies at a point (spec §14.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QueryHit {
    /// The document.
    pub doc_id: u64,
    /// The horizontal position, in device-independent pixels from the view origin, as for `input.pointer`.
    pub x: f64,
    /// The vertical position, in device-independent pixels from the view origin.
    pub y: f64,
}

/// The reply to `query.hit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HitReply {
    /// What lies at the point, or `null` when the point is on no page.
    pub hit: Option<Hit>,
}

/// The text position at a point (spec §14.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hit {
    /// The accessibility node whose text the offsets are in.
    pub node: u64,
    /// The caret position nearest to the point: the offset at the nearer edge, in the line's visual order, of the character under or beside the point, so that in right-to-left text the right half of a character gives the offset before it.
    pub caret: u32,
    /// The offset of the character whose box holds the point, or `null` when the point lies on no character's box.
    pub character: Option<u32>,
}

/// One format of a clipboard's content, as raw bytes (spec §14.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardItem {
    /// The format, as a MIME type such as `text/plain`.
    pub mime: String,
    /// The bytes; text formats as UTF-8.
    pub blob: BlobRef,
}

/// `input.paste`: the content of the platform's clipboard, which the engine inserts at the selection (spec §14.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputPaste {
    /// The document.
    pub doc_id: u64,
    /// The clipboard's bytes in every format that both the shell and the engine list. The engine inserts the richest format it reads and ignores the others: it keeps only the first item of each format of `welcome.clipboard_formats`, and skips every other item without storing it.
    #[serde(deserialize_with = "readable_items")]
    pub items: Vec<ClipboardItem>,
}

/// `clipboard.copy`: asks for the selection's content, for the platform's copy or cut action (spec §14.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardCopy {
    /// The document.
    pub doc_id: u64,
    /// Whether to cut: the engine also deletes the selection where the document is editable.
    #[serde(default)]
    pub cut: bool,
}

/// The reply to `clipboard.copy`: the selection in every format the engine writes, which the shell writes to the platform's clipboard as raw bytes; empty when nothing is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardItems {
    /// The formats, each as a blob that the shell releases.
    pub items: Vec<ClipboardItem>,
}

/// An item of `input.paste` as the engine reads it: its format borrowed from the message where JSON allows, so that an item the engine skips costs no memory beyond its reading.
#[derive(Deserialize)]
struct ItemRef<'a> {
    #[serde(borrow)]
    mime: Cow<'a, str>,
    blob: BlobRef,
}

/// Reads the items of `input.paste`, keeping only the first item of each format the engine reads ([`CLIPBOARD_FORMATS`]) and checking every other item's shape without storing it. A message of 16 MiB can hold about 800,000 items, and keeping them all would cost the engine several times the message's size, which in WebAssembly is never returned (spec §13).
fn readable_items<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ClipboardItem>, D::Error> {
    struct Items;

    impl<'de> Visitor<'de> for Items {
        type Value = Vec<ClipboardItem>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a list of clipboard items")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Self::Value, A::Error> {
            let mut kept: Vec<ClipboardItem> = Vec::new();
            while let Some(item) = items.next_element::<ItemRef<'de>>()? {
                let readable = CLIPBOARD_FORMATS.contains(&item.mime.as_ref());
                if readable && kept.iter().all(|known| known.mime != item.mime) {
                    kept.push(ClipboardItem {
                        mime: item.mime.into_owned(),
                        blob: item.blob,
                    });
                }
            }
            Ok(kept)
        }
    }

    deserializer.deserialize_seq(Items)
}

/// `input.composition`: input method composition (pre-edit text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputComposition {
    /// The document.
    pub doc_id: u64,
    /// What happened.
    pub phase: CompositionPhase,
    /// The composition's text (for `update`).
    #[serde(default)]
    pub text: String,
    /// The input method's caret or selection within `text`.
    #[serde(default)]
    pub selection: Option<TextRange>,
}

/// The phase of an input method composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompositionPhase {
    /// A composition begins at the caret.
    Start,
    /// The composition's text or selection changed.
    Update,
    /// The composition ends; its text is discarded (committed text arrives as `input.text`).
    End,
}

// ---------------------------------------------------------------------------------------------
// Accessibility (spec §6.6)
// ---------------------------------------------------------------------------------------------

/// `query.a11y`: asks for the accessibility tree (spec §6.6, §14.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueryA11y {
    /// The document.
    pub doc_id: u64,
    /// The node whose subtree to return; the whole document when omitted.
    #[serde(default)]
    pub node: Option<u64>,
    /// A range of the node's children to load, for a node that a tree gave with `child_count` (spec §14.7). The answer holds the node with as many of these children as fit, each with as much of its own subtree as fits, and at least one whenever the node has a child at `start`.
    #[serde(default)]
    pub children: Option<ChildRange>,
}

/// A range of a node's children: at most `count` of them, from index `start` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildRange {
    /// The index of the first child, counting from 0.
    pub start: u32,
    /// The most children to return; at least 1.
    pub count: u32,
}

/// The reply to `query.a11y`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A11yTree {
    /// The document.
    pub doc_id: u64,
    /// The requested subtree.
    pub root: A11yNode,
    /// The accessibility caret, or `null`.
    pub caret: Option<A11yCaret>,
    /// The document's selection (spec §14.7): empty when nothing is selected, so that a caret alone is no selection. The engine always sends it; an engine of protocol v1 from before CORE-014 does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Vec<A11ySelection>>,
}

/// A selected range of text (spec §14.7): from `anchor`, the end that stays when the selection is extended, to `focus`, where the caret is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A11ySelection {
    /// Where the selection starts.
    pub anchor: TextPosition,
    /// Where the selection ends, and the caret is.
    pub focus: TextPosition,
}

/// A node of the accessibility tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A11yNode {
    /// The node's identifier, stable for its lifetime.
    pub id: u64,
    /// The node's role.
    pub role: A11yRole,
    /// A heading's level, 1 to 9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// An accessible name where the content does not provide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The node's own text, on nodes that hold text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether the user can edit the node's text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub editable: bool,
    /// The node's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<PageRect>,
    /// The box of every character of `text`, on the page of `bounds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<Vec<CharBox>>,
    /// The node's language, as a BCP 47 tag, on a node that sets it; its descendants inherit it unless they set their own (spec §14.7). The document node always has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// The node's direction, on a node that sets it; its descendants inherit it unless they set their own. The document node always has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<TextDirection>,
    /// The parts of the node's own text whose language or direction differs from the node's, in order and without overlapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<Vec<TextRun>>,
    /// The offsets in `text` at which its lines start, as laid out, from 0 in increasing order; on every node that holds text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_starts: Option<Vec<u32>>,
    /// The number of the node's children, on a node that holds fewer than all of them (spec §14.7): the shell loads the rest with `query.a11y` and a `children` range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_count: Option<u32>,
    /// The index of the first child that the node holds, on a node that holds some but not all of its children; they are always one unbroken run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children_start: Option<u32>,
    /// The node's children, in reading order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<A11yNode>>,
}

/// The role of an accessibility node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum A11yRole {
    /// The whole document.
    Document,
    /// A heading.
    Heading,
    /// A paragraph.
    Paragraph,
    /// A list.
    List,
    /// An item of a list.
    ListItem,
    /// A table.
    Table,
    /// A row of a table.
    Row,
    /// A header cell of a column.
    ColumnHeader,
    /// A cell of a table.
    Cell,
}

/// The direction of text (spec §14.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextDirection {
    /// Left to right.
    Ltr,
    /// Right to left.
    Rtl,
}

/// A part of a node's own text whose language or direction differs from the node's (spec §14.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextRun {
    /// The first UTF-16 code unit of the part.
    pub start: u32,
    /// The code unit after the part.
    pub end: u32,
    /// The part's language, as a BCP 47 tag, where it differs from the node's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// The part's direction, where it differs from the node's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<TextDirection>,
}

/// The box of one character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CharBox {
    /// The character's first UTF-16 code unit in the node's text.
    pub start: u32,
    /// The code unit after the character.
    pub end: u32,
    /// The character's box, in page coordinates.
    pub rect: Rect,
}

/// The accessibility caret: a position in a node's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A11yCaret {
    /// The node.
    pub node: u64,
    /// The position, in UTF-16 code units into the node's text.
    pub offset: u32,
}

/// `a11y.update`: changes to the accessibility tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A11yUpdate {
    /// The document.
    pub doc_id: u64,
    /// The changes, in order.
    pub changes: Vec<A11yChange>,
    /// The accessibility caret, or `null`.
    pub caret: Option<A11yCaret>,
    /// The document's selection (spec §14.7): empty when nothing is selected. The engine always sends it; an engine of protocol v1 from before CORE-014 does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Vec<A11ySelection>>,
}

/// One change to the accessibility tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A11yChange {
    /// What changed.
    pub kind: A11yChangeKind,
    /// The new version of the node, children included.
    pub node: A11yNode,
}

/// The kind of an accessibility change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum A11yChangeKind {
    /// The node replaces the node with the same `id`.
    Replace,
}

// ---------------------------------------------------------------------------------------------
// UI manifest (spec §6.8)
// ---------------------------------------------------------------------------------------------

/// `ui.manifest`: asks for the UI manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiManifestRequest {
    /// The language of the strings, as a BCP 47 tag.
    #[serde(default)]
    pub locale: Option<String>,
}

/// The UI manifest: a stub until ADR-0019's manifest (spec §6.8), with its key data resolved for the session's platform and runtime (spec §14.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiManifest {
    /// The stub manifest's format version: 0, until ADR-0019's manifest.
    pub manifest_version: u32,
    /// The language of the strings.
    pub locale: String,
    /// The commands.
    pub commands: Vec<UiCommand>,
    /// The keys that the engine handles in the document area, and those that the shell keeps (spec §14.3).
    pub keys: UiKeys,
    /// The ribbon.
    pub ribbon: UiRibbon,
}

/// A command of the UI manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiCommand {
    /// The command's identifier, such as `format.bold.toggle`.
    pub id: String,
    /// Its label, formatted.
    pub label: String,
    /// Its key bindings, resolved for the session's platform and runtime (spec §14.3).
    pub bindings: Vec<KeyBinding>,
    /// For a command that the shell carries out itself through the platform, such as the clipboard's: `copy`, `cut` or `paste` in v1. A shell that meets a value it does not know neither sends the command's keys nor offers the command (spec §14.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_action: Option<String>,
}

/// A key binding of a command (spec §14.3): a key, named by `key` or by `code`, with all four modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyBinding {
    /// The key as the DOM's `KeyboardEvent.key` names it: a lower-case letter, a digit, a punctuation character, or a name such as `F7`. Exactly one of `key` and `code` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The physical key as the DOM's `KeyboardEvent.code` names it, such as `KeyA`, for a binding that follows a key's position rather than its character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Control.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// Meta (Command on macOS and iOS).
    pub meta: bool,
    /// The binding as the platform writes it, such as `Ctrl+B` or `⌘B`.
    pub label: String,
}

/// The key data of the UI manifest (spec §14.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiKeys {
    /// The keys that the engine handles in the document area with any modifiers.
    pub document: Vec<DocumentKey>,
    /// The presses that the shell keeps for itself, the operating system or the browser, even where a binding or a document key matches them.
    pub reserved: Vec<ReservedKey>,
}

/// A key that the engine handles in the document area, with any modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentKey {
    /// The key, as the DOM's `KeyboardEvent.key` names it.
    pub key: String,
}

/// A press that the shell keeps (spec §14.3): a key, named by `key` or by `code`, with only the modifiers that matter; one that it leaves out matches either state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReservedKey {
    /// The key as the DOM's `KeyboardEvent.key` names it. Exactly one of `key` and `code` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The physical key as the DOM's `KeyboardEvent.code` names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Control, if it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctrl: Option<bool>,
    /// Shift, if it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<bool>,
    /// Alt, if it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<bool>,
    /// Meta, if it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<bool>,
}

/// The ribbon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiRibbon {
    /// The tabs, in order.
    pub tabs: Vec<UiTab>,
}

/// A ribbon tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiTab {
    /// The tab's identifier.
    pub id: String,
    /// Its label, formatted.
    pub label: String,
    /// Its groups, in order.
    pub groups: Vec<UiGroup>,
}

/// A group of a ribbon tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiGroup {
    /// The group's identifier.
    pub id: String,
    /// Its label, formatted.
    pub label: String,
    /// The identifiers of its commands, in order.
    pub commands: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Diagnostics (spec §6.7, §10, §12)
// ---------------------------------------------------------------------------------------------

/// The reply to `diag.record.stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordStopped {
    /// The recording (spec §10), as a blob that the shell releases.
    pub blob: BlobRef,
    /// The number of entries.
    pub entries: u32,
    /// Whether a limit stopped the recording early.
    pub truncated: bool,
}

/// `diag.replay`: replays a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiagReplay {
    /// The recording.
    pub blob: BlobRef,
}

/// The reply to `diag.replay`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayReport {
    /// The number of entries replayed.
    pub entries: u32,
    /// Whether every entry produced exactly what it produced when it was recorded.
    pub identical: bool,
    /// The index of the first entry that did not, or `null`.
    pub first_difference: Option<u32>,
    /// Every tile rendered during the replay.
    pub tiles: Vec<ReplayTile>,
}

/// A tile rendered during a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayTile {
    /// The index of the entry that rendered it.
    pub entry: u32,
    /// The page.
    pub page: u32,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The digest of the pixels (spec §10).
    pub hash: String,
}

/// `engine.error`: an error without a request to answer, or a panic (spec §12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EngineError {
    /// What went wrong, for programs.
    pub code: ErrorCode,
    /// A Fluent message identifier, for people.
    pub message_id: String,
    /// `true`: the engine kept its state. `false`: it discarded its session; `restart` says what follows.
    pub recoverable: bool,
    /// Codes and numbers that explain the error; never document content.
    pub args: Args,
    /// On every `engine.error` with `recoverable: false`, added by the engine's binding (spec §14.2): `{ number, limit }` while the restart allowance lasts, and the shell starts restart `number` of `limit` with `hello`; `null` when it is used up, and the engine has stopped and answers nothing more. Absent on errors the engine recovered from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<Option<Restart>>,
}

/// A restart that the restart allowance grants (spec §14.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Restart {
    /// Which restart this is within the last ten minutes, from 1 to `limit`.
    pub number: u32,
    /// How many restarts the allowance grants within ten minutes: 3.
    pub limit: u32,
}

// ---------------------------------------------------------------------------------------------
// The catalog of messages
// ---------------------------------------------------------------------------------------------

/// Who sends a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The shell sends it to the engine, which replies.
    ToEngine,
    /// The engine sends it to the shell as an event.
    ToShell,
}

/// Adds the schema of a payload type to a schema generator and returns a reference to it.
pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

/// The [`SchemaFn`] of the payload type `T`.
#[must_use]
pub fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<T>()
}

/// A message type that the engine implements, with its payload type and reply. The schema and the TypeScript declarations are generated from this list ([`MESSAGES`]), and a test checks that the engine handles every entry.
#[derive(Debug, Clone, Copy)]
pub struct MessageSpec {
    /// The message type, such as `doc.open`.
    pub name: &'static str,
    /// Who sends it.
    pub direction: Direction,
    /// Its payload type.
    pub payload: SchemaFn,
    /// For messages to the engine: the reply's payload type.
    pub reply: Option<SchemaFn>,
    /// For messages to the engine: the name of the reply, carried in its `type` (spec §4).
    pub reply_type: Option<&'static str>,
    /// For messages to the engine: the reply's payload type as the web worker host delivers it, where it differs (spec §3.2).
    pub web_reply: Option<SchemaFn>,
}

const fn request(name: &'static str, payload: SchemaFn, reply: SchemaFn) -> MessageSpec {
    MessageSpec {
        name,
        direction: Direction::ToEngine,
        payload,
        reply: Some(reply),
        reply_type: None,
        web_reply: None,
    }
}

const fn event(name: &'static str, payload: SchemaFn) -> MessageSpec {
    MessageSpec {
        name,
        direction: Direction::ToShell,
        payload,
        reply: None,
        reply_type: None,
        web_reply: None,
    }
}

/// Every message type that the engine implements (spec §6), in the order of the specification, with those of spec §14.4 and §14.5 after the messages of their kind.
pub const MESSAGES: [MessageSpec; 25] = [
    MessageSpec {
        reply_type: Some("welcome"),
        ..request("hello", schema_of::<Hello>, schema_of::<Welcome>)
    },
    MessageSpec {
        reply_type: Some("doc.opened"),
        ..request("doc.open", schema_of::<DocOpen>, schema_of::<DocOpened>)
    },
    request("doc.close", schema_of::<DocClose>, schema_of::<Empty>),
    request("view.set", schema_of::<ViewSet>, schema_of::<Empty>),
    event("view.layout.progress", schema_of::<ViewLayoutProgress>),
    event("view.pages", schema_of::<ViewPages>),
    event("render.invalidate", schema_of::<RenderInvalidate>),
    MessageSpec {
        web_reply: Some(schema_of::<RenderTileWebReply>),
        ..request(
            "render.tile",
            schema_of::<RenderTile>,
            schema_of::<RenderTileReply>,
        )
    },
    event("overlay.update", schema_of::<OverlayUpdate>),
    request(
        "input.pointer",
        schema_of::<InputPointer>,
        schema_of::<Empty>,
    ),
    request("input.key", schema_of::<InputKey>, schema_of::<Empty>),
    request("input.text", schema_of::<InputText>, schema_of::<Empty>),
    request(
        "input.composition",
        schema_of::<InputComposition>,
        schema_of::<Empty>,
    ),
    request("input.select", schema_of::<InputSelect>, schema_of::<Empty>),
    request("input.paste", schema_of::<InputPaste>, schema_of::<Empty>),
    request("query.a11y", schema_of::<QueryA11y>, schema_of::<A11yTree>),
    event("a11y.update", schema_of::<A11yUpdate>),
    request("query.hit", schema_of::<QueryHit>, schema_of::<HitReply>),
    request(
        "clipboard.copy",
        schema_of::<ClipboardCopy>,
        schema_of::<ClipboardItems>,
    ),
    request(
        "ui.manifest",
        schema_of::<UiManifestRequest>,
        schema_of::<UiManifest>,
    ),
    request("diag.record.start", schema_of::<Empty>, schema_of::<Empty>),
    request(
        "diag.record.stop",
        schema_of::<Empty>,
        schema_of::<RecordStopped>,
    ),
    request(
        "diag.replay",
        schema_of::<DiagReplay>,
        schema_of::<ReplayReport>,
    ),
    request("diag.panic", schema_of::<Empty>, schema_of::<Empty>),
    event("engine.error", schema_of::<EngineError>),
];

/// The catalog entry of a message type, if the engine implements it.
#[must_use]
pub fn message_spec(name: &str) -> Option<&'static MessageSpec> {
    MESSAGES.iter().find(|spec| spec.name == name)
}
