//! The engine core: one session that handles one message at a time and returns the messages it sends in answer ([engine protocol specification][spec] §4–§8).
//!
//! [`Engine`] knows nothing about threads, callbacks or JavaScript: the native engine thread ([`crate::thread`]), the WebAssembly binding and the replay of a recording all feed it the same way, so the protocol behaves identically in every host. It never reads a clock: the host passes the time a message arrived, which only recordings use (spec §10).
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::blobs::{BlobError, BlobId, BlobStore};
use crate::config::Config;
use crate::digest;
use crate::envelope::{Envelope, EnvelopeError, invalid_request, with_type};
use crate::limits::{
    MAX_DEVICE_SCALE, MAX_DOCUMENTS, MAX_ID, MAX_MESSAGE_BYTES, MAX_TILE_COORDINATE, MAX_TILE_SIDE,
    MAX_VIEW_COORDINATE, MAX_ZOOM, MIN_ZOOM,
};
use crate::manifest;
use crate::mock::{MockDocument, View};
use crate::protocol::{
    A11yChange, A11yChangeKind, A11yTree, A11yUpdate, BlobRef, CompositionPhase, DiagReplay,
    DocClose, DocOpen, DocOpened, Empty, EngineError, ErrorCode, ErrorInfo, FEATURES, FontReport,
    Hello, InputComposition, InputKey, InputPointer, InputText, LAYOUT_EPOCH, OverlayUpdate,
    PROTOCOL_VERSION, PointerKind, QueryA11y, RecordStopped, RenderInvalidate, RenderTile,
    RenderTileReply, ReplayTile, TileRequest, UiManifestRequest, ViewLayoutProgress, ViewPages,
    ViewSet, Welcome,
};
use crate::raster::TileGeometry;
use crate::recording::{self, Recorder};

/// The engine's semantic version, from the workspace version.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why a tile rendered through `bayan_render_tile` failed. On the C interface each maps to a `BayanStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileError {
    /// The request is malformed, or a size is outside the limits.
    InvalidArgument,
    /// The document or page does not exist.
    NotFound,
    /// The engine failed, panicked or is stopping.
    Internal,
    /// The call came from inside any engine's callback, where waiting could deadlock: on the engine's own thread it would wait for itself, and two engines' callbacks could wait for each other (spec §8).
    WrongThread,
}

impl TileError {
    /// A short name for recordings and messages, such as `not_found`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::Internal => "internal",
            Self::WrongThread => "wrong_thread",
        }
    }
}

/// What rendering a tile through `bayan_render_tile` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileOutcome {
    /// The pixels, premultiplied RGBA8 without row padding, or why there are none.
    pub pixels: Result<Vec<u8>, TileError>,
    /// Messages to deliver to the shell: an `engine.error` if rendering panicked (spec §12).
    pub messages: Vec<String>,
}

/// The state of one session: the handshake and the open documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Session {
    handshake: bool,
    next_doc_id: u64,
    documents: BTreeMap<u64, MockDocument>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            handshake: false,
            next_doc_id: 1,
            documents: BTreeMap::new(),
        }
    }
}

impl Session {
    fn is_valid(&self) -> bool {
        self.next_doc_id >= 1
            && self.next_doc_id <= MAX_ID + 1
            && self.documents.len() <= MAX_DOCUMENTS
            && self
                .documents
                .iter()
                .all(|(&id, document)| id >= 1 && id < self.next_doc_id && document.is_valid())
    }
}

/// Everything a recording needs to start in the middle of a session (spec §10): the session, and the counters that number events and engine-created blobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    session: Session,
    pub(crate) next_seq: u64,
    next_engine_blob: BlobId,
}

/// The messages the engine sends while handling one message, and the next event sequence number.
struct Outbox {
    messages: Vec<String>,
    next_seq: u64,
}

#[derive(Serialize)]
struct ReplyOk<'a, T: Serialize> {
    v: u32,
    re: u64,
    ok: bool,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
    payload: &'a T,
}

#[derive(Serialize)]
struct ReplyError<'a> {
    v: u32,
    re: u64,
    ok: bool,
    error: &'a ErrorInfo,
}

#[derive(Serialize)]
struct Event<'a, T: Serialize> {
    v: u32,
    #[serde(rename = "type")]
    kind: &'a str,
    seq: u64,
    payload: &'a T,
}

impl Outbox {
    const fn new(next_seq: u64) -> Self {
        Self {
            messages: Vec::new(),
            next_seq,
        }
    }

    /// Serializes a message, or returns `None` if it would exceed the message limit (spec §13).
    fn encode<T: Serialize>(message: &T) -> Option<String> {
        serde_json::to_string(message)
            .ok()
            .filter(|json| json.len() <= MAX_MESSAGE_BYTES)
    }

    /// Replies with a payload, if the request had an identifier.
    fn reply<T: Serialize>(&mut self, id: Option<u64>, kind: Option<&str>, payload: &T) {
        let Some(re) = id else {
            return;
        };
        let reply = ReplyOk {
            v: PROTOCOL_VERSION,
            re,
            ok: true,
            kind,
            payload,
        };
        match Self::encode(&reply) {
            Some(json) => self.messages.push(json),
            None => self.fail(
                id,
                ErrorInfo::new(ErrorCode::LimitExceeded).with_text("limit", "message_size"),
            ),
        }
    }

    /// Reports an error: as the reply, if the request had an identifier, otherwise as an `engine.error` event the engine recovered from (spec §4).
    fn fail(&mut self, id: Option<u64>, error: ErrorInfo) {
        match id {
            Some(re) => {
                let reply = ReplyError {
                    v: PROTOCOL_VERSION,
                    re,
                    ok: false,
                    error: &error,
                };
                if let Some(json) = Self::encode(&reply) {
                    self.messages.push(json);
                }
            }
            None => self.error_event(error, true),
        }
    }

    /// Sends `engine.error`.
    fn error_event(&mut self, error: ErrorInfo, recoverable: bool) {
        let payload = EngineError {
            code: error.code,
            message_id: error.message_id,
            recoverable,
            args: error.args,
        };
        self.event("engine.error", &payload);
    }

    /// Sends an event; one that would exceed the message limit becomes an `engine.error` instead.
    fn event<T: Serialize>(&mut self, kind: &str, payload: &T) {
        let event = Event {
            v: PROTOCOL_VERSION,
            kind,
            seq: self.next_seq,
            payload,
        };
        if let Some(json) = Self::encode(&event) {
            self.messages.push(json);
            self.next_seq += 1;
        } else if kind != "engine.error" {
            self.error_event(
                ErrorInfo::new(ErrorCode::LimitExceeded).with_text("limit", "message_size"),
                true,
            );
        }
    }
}

fn unsupported_version() -> ErrorInfo {
    ErrorInfo::new(ErrorCode::UnsupportedProtocolVersion)
        .with_integers("supported", vec![i64::from(PROTOCOL_VERSION)])
}

fn not_found(what: &str) -> ErrorInfo {
    ErrorInfo::new(ErrorCode::NotFound).with_text("what", what)
}

fn limit_exceeded(limit: &str) -> ErrorInfo {
    ErrorInfo::new(ErrorCode::LimitExceeded).with_text("limit", limit)
}

fn blob_error(error: BlobError) -> ErrorInfo {
    match error {
        BlobError::TooLarge => limit_exceeded("blob_size"),
        BlobError::TooMany => limit_exceeded("blobs"),
    }
}

/// Whether a floating-point value is finite and within `low` to `high`; uses only comparisons (ADR-0005 §4).
fn within(value: f64, low: f64, high: f64) -> bool {
    value.is_finite() && value >= low && value <= high
}

/// The geometry of a tile request, checked against the limits of spec §13.
fn tile_geometry(request: &RenderTile) -> Result<TileGeometry, TileFailure> {
    let (width, height) = (request.width, request.height);
    if width == 0 || height == 0 || width > MAX_TILE_SIDE || height > MAX_TILE_SIDE {
        return Err(TileFailure::Size);
    }
    let rect = request.rect;
    let coordinate_ok = |value: i64| (-MAX_TILE_COORDINATE..=MAX_TILE_COORDINATE).contains(&value);
    let extent_ok = |value: i64| (1..=MAX_TILE_COORDINATE).contains(&value);
    let hint_ok =
        |hint: Option<f64>| hint.is_none_or(|value| within(value, f64::MIN_POSITIVE, f64::MAX));
    if !(coordinate_ok(rect.x)
        && coordinate_ok(rect.y)
        && extent_ok(rect.width)
        && extent_ok(rect.height)
        && hint_ok(request.zoom)
        && hint_ok(request.device_scale))
    {
        return Err(TileFailure::Invalid);
    }
    Ok(TileGeometry {
        rect,
        width,
        height,
    })
}

/// Why a tile could not be rendered, before it is reported as a message error or a [`TileError`].
enum TileFailure {
    Invalid,
    Size,
    NoDocument,
    NoPage,
    Memory,
    /// A replay has rendered as many pixels as it may (spec §13); the replay stops.
    ReplayBudget,
}

impl TileFailure {
    fn as_message_error(&self, kind: &str) -> ErrorInfo {
        match self {
            Self::Invalid => invalid_request(kind),
            Self::Size => limit_exceeded("tile_size"),
            Self::NoDocument => not_found("document"),
            Self::NoPage => not_found("page"),
            Self::Memory => ErrorInfo::new(ErrorCode::Internal),
            Self::ReplayBudget => limit_exceeded("replay_pixels"),
        }
    }

    const fn as_tile_error(&self) -> TileError {
        match self {
            Self::Invalid | Self::Size | Self::ReplayBudget => TileError::InvalidArgument,
            Self::NoDocument | Self::NoPage => TileError::NotFound,
            Self::Memory => TileError::Internal,
        }
    }
}

/// An engine: one session, its blobs, and a recording if one is running.
#[derive(Debug)]
pub struct Engine {
    config: Config,
    blobs: Arc<BlobStore>,
    session: Session,
    next_seq: u64,
    recorder: Option<Recorder>,
    /// Blobs read while handling the current message, for the recording (spec §10).
    blob_reads: Vec<(BlobId, Arc<[u8]>)>,
    /// While this engine replays a recording: the index of the entry being replayed, and the tiles rendered so far.
    replay: Option<ReplayLog>,
}

#[derive(Debug)]
struct ReplayLog {
    entry: u32,
    tiles: Vec<ReplayTile>,
    /// How many more pixels the replay may render (spec §13).
    pixels_left: u64,
    /// Whether a tile was refused because the replay had rendered as many pixels as it may.
    over_budget: bool,
}

impl Engine {
    /// An engine with a configuration and a blob store, which a native host shares with the shell's threads.
    #[must_use]
    pub fn new(config: Config, blobs: Arc<BlobStore>) -> Self {
        Self {
            config,
            blobs,
            session: Session::default(),
            next_seq: 1,
            recorder: None,
            blob_reads: Vec::new(),
            replay: None,
        }
    }

    /// The engine's blob store.
    #[must_use]
    pub fn blobs(&self) -> &Arc<BlobStore> {
        &self.blobs
    }

    /// Handles one message (a protocol envelope, spec §4) that arrived `received_ms` milliseconds after some fixed moment the host chooses, and returns the messages the engine sends in answer, in order, as JSON. A panic while handling it is caught: the engine then discards its session and reports the panic (spec §12).
    pub fn handle(&mut self, message: &[u8], received_ms: u64) -> Vec<String> {
        let recording = self.recorder.is_some();
        self.blob_reads.clear();
        let messages = self.handle_message(message);
        if recording {
            let digest = digest::of_messages(&messages);
            let reads = std::mem::take(&mut self.blob_reads);
            if let Some(recorder) = &mut self.recorder {
                recorder.record_message(message, received_ms, digest, reads);
            }
        }
        messages
    }

    /// Reports, from a new engine, that the engine instance it replaces panicked while handling a message with the identifier `id`: the error `panic` as the reply if the message had a valid identifier, then `engine.error` with `recoverable: false` (spec §3.2, §12). The web worker host calls it, because a panic in WebAssembly stops the whole instance instead of unwinding to [`Engine::handle`]. It passes only the identifier, which it reads itself, so the new instance never copies or parses the message that may have caused the panic.
    pub fn report_panic(&mut self, id: Option<u64>) -> Vec<String> {
        self.recover_from_panic(id.filter(|id| (1..=MAX_ID).contains(id)))
    }

    /// Recovers from a panic that escaped [`Engine::handle`] or [`Engine::render_tile`], which catch the panics of message handlers and of rendering themselves: discards the session and any running recording, whose state the panic may have left half-changed, and reports `engine.error` with `recoverable: false` (spec §12). The native engine thread calls it, so that no panic ends the thread.
    pub fn recover_from_escaped_panic(&mut self) -> Vec<String> {
        self.recorder = None;
        self.blob_reads.clear();
        self.recover_from_panic(None)
    }

    /// Answers the message with the identifier `id` with the error `internal`, for a host that could not deliver the engine's reply to it, such as the web worker host when the browser cannot create a tile's `ImageBitmap` (spec §3.2). A message without a valid identifier gets no reply, so nothing is sent for it.
    pub fn report_host_failure(&mut self, id: Option<u64>) -> Vec<String> {
        let id = id.filter(|id| (1..=MAX_ID).contains(id));
        let mut outbox = Outbox::new(self.next_seq);
        if id.is_some() {
            outbox.fail(id, ErrorInfo::new(ErrorCode::Internal));
        }
        outbox.messages
    }

    /// The answer to a message that the host did not hand to the engine because it is longer than the message limit (spec §13), as the web worker host does for JSON text over 16 MiB rather than copy it into the module's memory: `limit_exceeded` (`args.limit`: `message_size`), as the reply if the host could read the message's identifier `id`, otherwise as `engine.error`, as for a message the engine refuses itself. A recording cannot hold a message the engine never saw, so when the answer is an event (which counts towards `seq`), a running recording stops there and is marked truncated.
    pub fn refuse_oversized(&mut self, id: Option<u64>) -> Vec<String> {
        self.refuse_unseen(id, limit_exceeded("message_size"))
    }

    /// The answer to a message that the host could not turn into JSON text, such as a JavaScript object holding a `BigInt` that the web worker host cannot serialize: `invalid_message`, as the reply if the host could read the message's identifier `id`, otherwise as `engine.error`, with the same effect on a running recording as [`Engine::refuse_oversized`] (spec §3.2).
    pub fn refuse_unreadable(&mut self, id: Option<u64>) -> Vec<String> {
        self.refuse_unseen(id, ErrorInfo::new(ErrorCode::InvalidMessage))
    }

    /// Answers a message the engine never saw with `error`. A recording cannot hold such a message, so when the answer is an event (which counts towards `seq`), a running recording stops there and is marked truncated.
    fn refuse_unseen(&mut self, id: Option<u64>, error: ErrorInfo) -> Vec<String> {
        let id = id.filter(|id| (1..=MAX_ID).contains(id));
        let mut outbox = Outbox::new(self.next_seq);
        outbox.fail(id, error);
        self.next_seq = outbox.next_seq;
        if id.is_none()
            && let Some(recorder) = &mut self.recorder
        {
            recorder.stop_early();
        }
        outbox.messages
    }

    /// Renders a tile for `bayan_render_tile`: `request` is a `render.tile` payload (spec §6.4) whose `width` and `height`, if present, must equal the parameters.
    pub fn render_tile(
        &mut self,
        request: &[u8],
        width: u32,
        height: u32,
        received_ms: u64,
    ) -> TileOutcome {
        let recording = self.recorder.is_some();
        self.blob_reads.clear();
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            self.render_direct(request, width, height)
        }));
        let outcome = match attempt {
            Ok(pixels) => TileOutcome {
                pixels,
                messages: Vec::new(),
            },
            Err(_) => TileOutcome {
                pixels: Err(TileError::Internal),
                messages: self.recover_from_panic(None),
            },
        };
        if recording {
            let digest = match &outcome.pixels {
                Ok(pixels) => digest::of_bytes(pixels),
                Err(error) => format!("error:{}", error.as_str()),
            };
            if let Some(recorder) = &mut self.recorder {
                recorder.record_tile(request, width, height, received_ms, digest);
            }
        }
        outcome
    }

    fn render_direct(
        &mut self,
        request: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, TileError> {
        if request.len() > MAX_MESSAGE_BYTES {
            return Err(TileError::InvalidArgument);
        }
        let request: TileRequest =
            serde_json::from_slice(request).map_err(|_| TileError::InvalidArgument)?;
        let request = request
            .with_size(width, height)
            .ok_or(TileError::InvalidArgument)?;
        let pixels = self
            .render(&request)
            .map_err(|failure| failure.as_tile_error())?;
        self.log_tile(request.page, width, height, digest::of_bytes(&pixels));
        Ok(pixels)
    }

    fn render(&mut self, request: &RenderTile) -> Result<Vec<u8>, TileFailure> {
        let geometry = tile_geometry(request)?;
        if let Some(replay) = &mut self.replay {
            // Charged before rendering, so a replay never renders more than its budget.
            let pixels = u64::from(geometry.width) * u64::from(geometry.height);
            if pixels > replay.pixels_left {
                replay.over_budget = true;
                return Err(TileFailure::ReplayBudget);
            }
            replay.pixels_left -= pixels;
        }
        let document = self
            .session
            .documents
            .get(&request.doc_id)
            .ok_or(TileFailure::NoDocument)?;
        if request.page >= MockDocument::page_count() {
            return Err(TileFailure::NoPage);
        }
        document
            .render(request.page, &geometry)
            .ok_or(TileFailure::Memory)
    }

    /// Records a tile in the replay's list of tiles, when replaying.
    fn log_tile(&mut self, page: u32, width: u32, height: u32, hash: String) {
        if let Some(replay) = &mut self.replay {
            replay.tiles.push(ReplayTile {
                entry: replay.entry,
                page,
                width,
                height,
                hash,
            });
        }
    }

    fn handle_message(&mut self, message: &[u8]) -> Vec<String> {
        let mut outbox = Outbox::new(self.next_seq);
        let envelope = match Envelope::parse(message) {
            Ok(envelope) => envelope,
            Err((id, error)) => {
                let error = match error {
                    EnvelopeError::TooLarge => limit_exceeded("message_size"),
                    EnvelopeError::Invalid => ErrorInfo::new(ErrorCode::InvalidMessage),
                    EnvelopeError::UnsupportedVersion => unsupported_version(),
                };
                outbox.fail(id, error);
                self.next_seq = outbox.next_seq;
                return outbox.messages;
            }
        };
        let attempt = catch_unwind(AssertUnwindSafe(|| self.dispatch(&envelope, &mut outbox)));
        match attempt {
            Ok(()) => {
                self.next_seq = outbox.next_seq;
                outbox.messages
            }
            // Whatever the handler sent before it panicked is discarded with the session.
            Err(_) => self.recover_from_panic(envelope.id),
        }
    }

    /// Discards the session after a panic and reports it: as the reply to the message that caused it, if it had an identifier, and as `engine.error` with `recoverable: false` (spec §12). A recording keeps running, so it can reproduce the panic.
    fn recover_from_panic(&mut self, id: Option<u64>) -> Vec<String> {
        self.session = Session::default();
        let mut outbox = Outbox::new(self.next_seq);
        if let Some(re) = id {
            outbox.fail(Some(re), ErrorInfo::new(ErrorCode::Panic));
        }
        outbox.error_event(ErrorInfo::new(ErrorCode::Panic), false);
        self.next_seq = outbox.next_seq;
        outbox.messages
    }

    fn dispatch(&mut self, envelope: &Envelope<'_>, outbox: &mut Outbox) {
        let result = match envelope.kind.as_str() {
            "hello" => self.hello(envelope, outbox),
            _ if !self.session.handshake => Err(ErrorInfo::new(ErrorCode::HandshakeRequired)),
            "doc.open" => self.doc_open(envelope, outbox),
            "doc.close" => self.doc_close(envelope, outbox),
            "view.set" => self.view_set(envelope, outbox),
            "render.tile" => self.render_message(envelope, outbox),
            "input.pointer" => self.input_pointer(envelope, outbox),
            "input.key" => self.input_key(envelope, outbox),
            "input.text" => self.input_text(envelope, outbox),
            "input.composition" => self.input_composition(envelope, outbox),
            "query.a11y" => self.query_a11y(envelope, outbox),
            "ui.manifest" => Self::ui_manifest(envelope, outbox),
            "diag.record.start" => self.record_start(envelope, outbox),
            "diag.record.stop" => self.record_stop(envelope, outbox),
            "diag.replay" => self.replay_message(envelope, outbox),
            "diag.panic" => self.panic_message(),
            other => Err(with_type(
                ErrorInfo::new(ErrorCode::UnsupportedMessage),
                other,
            )),
        };
        if let Err(error) = result {
            outbox.fail(envelope.id, error);
        }
    }

    /// Reads a blob and remembers it for the recording. Blob 0 stands for bytes that could not be stored: `bayan_blob_put` returns it when a limit refuses them, and the web worker host passes it for an `ArrayBuffer` it cannot store (spec §3).
    fn read_blob(&mut self, blob: BlobRef) -> Result<Arc<[u8]>, ErrorInfo> {
        if blob.0 == 0 {
            return Err(limit_exceeded("blob"));
        }
        let bytes = self.blobs.get(blob.0).ok_or_else(|| not_found("blob"))?;
        if self.recorder.is_some() {
            self.blob_reads.push((blob.0, Arc::clone(&bytes)));
        }
        Ok(bytes)
    }

    fn document(&mut self, doc_id: u64) -> Result<&mut MockDocument, ErrorInfo> {
        self.session
            .documents
            .get_mut(&doc_id)
            .ok_or_else(|| not_found("document"))
    }

    fn hello(&mut self, envelope: &Envelope<'_>, outbox: &mut Outbox) -> Result<(), ErrorInfo> {
        let hello: Hello = envelope.payload()?;
        if !hello.protocol_versions.contains(&PROTOCOL_VERSION) {
            return Err(unsupported_version());
        }
        // A new session starts without documents (spec §5).
        self.session = Session {
            handshake: true,
            ..Session::default()
        };
        let welcome = Welcome {
            protocol_version: PROTOCOL_VERSION,
            engine_version: ENGINE_VERSION.to_owned(),
            layout_epoch: LAYOUT_EPOCH,
            features: FEATURES.iter().map(|&feature| feature.to_owned()).collect(),
        };
        outbox.reply(envelope.id, Some("welcome"), &welcome);
        Ok(())
    }

    fn doc_open(&mut self, envelope: &Envelope<'_>, outbox: &mut Outbox) -> Result<(), ErrorInfo> {
        let open: DocOpen = envelope.payload()?;
        // The v0 skeleton opens the mock document whatever the blob holds, but the blob must exist.
        self.read_blob(open.blob)?;
        if self.session.documents.len() >= MAX_DOCUMENTS {
            return Err(limit_exceeded("documents"));
        }
        let doc_id = self.session.next_doc_id;
        if doc_id > MAX_ID {
            return Err(limit_exceeded("documents"));
        }
        self.session.next_doc_id += 1;
        self.session
            .documents
            .insert(doc_id, MockDocument::default());
        let opened = DocOpened {
            doc_id,
            page_count_estimate: MockDocument::page_count(),
            warnings: Vec::new(),
            fonts: FontReport::default(),
        };
        outbox.reply(envelope.id, Some("doc.opened"), &opened);
        Ok(())
    }

    fn doc_close(&mut self, envelope: &Envelope<'_>, outbox: &mut Outbox) -> Result<(), ErrorInfo> {
        let close: DocClose = envelope.payload()?;
        self.session
            .documents
            .remove(&close.doc_id)
            .ok_or_else(|| not_found("document"))?;
        outbox.reply(envelope.id, None, &Empty {});
        Ok(())
    }

    fn view_set(&mut self, envelope: &Envelope<'_>, outbox: &mut Outbox) -> Result<(), ErrorInfo> {
        let view: ViewSet = envelope.payload()?;
        let viewport = view.viewport;
        let valid = within(view.zoom, MIN_ZOOM, MAX_ZOOM)
            && within(view.device_scale, f64::MIN_POSITIVE, MAX_DEVICE_SCALE)
            && within(viewport.x, -MAX_VIEW_COORDINATE, MAX_VIEW_COORDINATE)
            && within(viewport.y, -MAX_VIEW_COORDINATE, MAX_VIEW_COORDINATE)
            && within(viewport.width, 0.0, MAX_VIEW_COORDINATE)
            && within(viewport.height, 0.0, MAX_VIEW_COORDINATE);
        if !valid {
            return Err(invalid_request(&envelope.kind));
        }
        let document = self.document(view.doc_id)?;
        document.view = Some(View { zoom: view.zoom });
        let overlay = overlay(view.doc_id, document);
        let doc_id = view.doc_id;
        let pages = MockDocument::page_count();
        outbox.reply(envelope.id, None, &Empty {});
        outbox.event(
            "view.pages",
            &ViewPages {
                doc_id,
                pages: MockDocument::pages(),
            },
        );
        outbox.event(
            "render.invalidate",
            &RenderInvalidate {
                doc_id,
                regions: MockDocument::page_regions(),
            },
        );
        outbox.event(
            "view.layout.progress",
            &ViewLayoutProgress {
                doc_id,
                pages_laid_out: pages,
                page_count_estimate: pages,
                complete: true,
            },
        );
        outbox.event("overlay.update", &overlay);
        Ok(())
    }

    fn render_message(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let request: RenderTile = envelope.payload()?;
        let (width, height) = (request.width, request.height);
        let pixels = self
            .render(&request)
            .map_err(|failure| failure.as_message_error(&envelope.kind))?;
        let hash = digest::of_bytes(&pixels);
        self.log_tile(request.page, width, height, hash.clone());
        // Without an identifier there is no reply, so nobody could learn the blob's identifier and release it.
        if envelope.id.is_none() {
            return Ok(());
        }
        let blob = self.blobs.put_engine(pixels).map_err(blob_error)?;
        let reply = RenderTileReply {
            width,
            height,
            stride: width * 4,
            hash,
            blob: BlobRef(blob),
        };
        outbox.reply(envelope.id, None, &reply);
        Ok(())
    }

    fn input_pointer(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let pointer: InputPointer = envelope.payload()?;
        let document = self.document(pointer.doc_id)?;
        let pressed = pointer.kind == PointerKind::Down && pointer.button == 0;
        let changed = pressed && document.press(pointer.x, pointer.y);
        outbox.reply(envelope.id, None, &Empty {});
        if changed {
            line_changed(pointer.doc_id, document, outbox);
        }
        Ok(())
    }

    fn input_key(&mut self, envelope: &Envelope<'_>, outbox: &mut Outbox) -> Result<(), ErrorInfo> {
        let key: InputKey = envelope.payload()?;
        let document = self.document(key.doc_id)?;
        let changed = !key.composing && document.key(&key.key);
        outbox.reply(envelope.id, None, &Empty {});
        if changed {
            line_changed(key.doc_id, document, outbox);
        }
        Ok(())
    }

    fn input_text(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let input: InputText = envelope.payload()?;
        let document = self.document(input.doc_id)?;
        let changed = document
            .insert_text(&input.text)
            .map_err(|_| limit_exceeded("text_length"))?;
        outbox.reply(envelope.id, None, &Empty {});
        if changed {
            line_changed(input.doc_id, document, outbox);
        }
        Ok(())
    }

    fn input_composition(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let input: InputComposition = envelope.payload()?;
        let document = self.document(input.doc_id)?;
        let changed = match input.phase {
            CompositionPhase::Start => document.start_composition(),
            CompositionPhase::Update => document
                .update_composition(&input.text, input.selection)
                .map_err(|_| limit_exceeded("composition_length"))?,
            CompositionPhase::End => document.end_composition(),
        };
        outbox.reply(envelope.id, None, &Empty {});
        if changed {
            line_changed(input.doc_id, document, outbox);
        }
        Ok(())
    }

    fn query_a11y(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let query: QueryA11y = envelope.payload()?;
        let document = self.document(query.doc_id)?;
        let root = match query.node {
            Some(id) => document.a11y_subtree(id).ok_or_else(|| not_found("node"))?,
            None => document.a11y_tree(),
        };
        let tree = A11yTree {
            doc_id: query.doc_id,
            root,
            caret: Some(document.a11y_caret()),
        };
        outbox.reply(envelope.id, None, &tree);
        Ok(())
    }

    fn ui_manifest(envelope: &Envelope<'_>, outbox: &mut Outbox) -> Result<(), ErrorInfo> {
        // The stub is in English whatever locale is asked for (spec §6.8).
        let _request: UiManifestRequest = envelope.payload()?;
        outbox.reply(envelope.id, None, &manifest::stub());
        Ok(())
    }

    fn record_start(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let _request: Empty = envelope.payload()?;
        // A replay behaves as the recording session did, where a recording was running.
        if self.recorder.is_some() || self.replay.is_some() {
            return Err(ErrorInfo::new(ErrorCode::AlreadyRecording));
        }
        self.recorder = Some(Recorder::start(&self.snapshot())?);
        outbox.reply(envelope.id, None, &Empty {});
        Ok(())
    }

    fn record_stop(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        let _request: Empty = envelope.payload()?;
        let recorder = self
            .recorder
            .take()
            .ok_or_else(|| ErrorInfo::new(ErrorCode::NotRecording))?;
        let (bytes, entries, truncated) = recorder.finish()?;
        // As for tiles: without an identifier, nobody could release the recording's blob.
        if envelope.id.is_none() {
            return Ok(());
        }
        let blob = self.blobs.put_engine(bytes).map_err(blob_error)?;
        let stopped = RecordStopped {
            blob: BlobRef(blob),
            entries,
            truncated,
        };
        outbox.reply(envelope.id, None, &stopped);
        Ok(())
    }

    fn replay_message(
        &mut self,
        envelope: &Envelope<'_>,
        outbox: &mut Outbox,
    ) -> Result<(), ErrorInfo> {
        if self.recorder.is_some() || self.replay.is_some() {
            return Err(ErrorInfo::new(ErrorCode::NotAllowed).with_text("reason", "recording"));
        }
        let request: DiagReplay = envelope.payload()?;
        let bytes = self.read_blob(request.blob)?;
        let report = recording::replay(self.config, &bytes)?;
        outbox.reply(envelope.id, None, &report);
        Ok(())
    }

    fn panic_message(&self) -> Result<(), ErrorInfo> {
        if !self.config.allow_panic {
            return Err(
                ErrorInfo::new(ErrorCode::NotAllowed).with_text("reason", "test_switch_off")
            );
        }
        panic!("diag.panic: a panic that a test asked for (engine protocol §6.7)");
    }

    /// The state a recording starts from (spec §10).
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            session: self.session.clone(),
            next_seq: self.next_seq,
            next_engine_blob: self.blobs.next_engine_id(),
        }
    }

    /// An engine that replays a recording: it starts from the recording's snapshot, behaves as an engine that is recording, lists the tiles it renders, and renders at most `pixel_budget` pixels.
    pub(crate) fn for_replay(
        config: Config,
        blobs: Arc<BlobStore>,
        snapshot: Snapshot,
        pixel_budget: u64,
    ) -> Option<Self> {
        let valid = snapshot.session.is_valid()
            && (1..=MAX_ID).contains(&snapshot.next_seq)
            && blobs.set_next_engine_id(snapshot.next_engine_blob);
        valid.then(|| Self {
            session: snapshot.session,
            next_seq: snapshot.next_seq,
            replay: Some(ReplayLog {
                entry: 0,
                tiles: Vec::new(),
                pixels_left: pixel_budget,
                over_budget: false,
            }),
            ..Self::new(config, blobs)
        })
    }

    /// Whether this replaying engine refused a tile because the replay had rendered as many pixels as it may.
    pub(crate) fn replay_over_budget(&self) -> bool {
        self.replay
            .as_ref()
            .is_some_and(|replay| replay.over_budget)
    }

    /// Sets the index of the recording entry being replayed.
    pub(crate) fn set_replay_entry(&mut self, entry: u32) {
        if let Some(replay) = &mut self.replay {
            replay.entry = entry;
        }
    }

    /// The tiles rendered during a replay.
    pub(crate) fn take_replay_tiles(&mut self) -> Vec<ReplayTile> {
        self.replay
            .as_mut()
            .map(|replay| std::mem::take(&mut replay.tiles))
            .unwrap_or_default()
    }
}

/// The overlay of a document.
fn overlay(doc_id: u64, document: &MockDocument) -> OverlayUpdate {
    OverlayUpdate {
        doc_id,
        caret: Some(document.caret()),
        selection: Vec::new(),
        remote: Vec::new(),
        highlights: Vec::new(),
    }
}

/// The events after the editable line changed: the region to draw again, the caret, and the accessibility node.
fn line_changed(doc_id: u64, document: &MockDocument, outbox: &mut Outbox) {
    outbox.event(
        "render.invalidate",
        &RenderInvalidate {
            doc_id,
            regions: vec![MockDocument::editable_region()],
        },
    );
    outbox.event("overlay.update", &overlay(doc_id, document));
    outbox.event(
        "a11y.update",
        &A11yUpdate {
            doc_id,
            changes: vec![A11yChange {
                kind: A11yChangeKind::Replace,
                node: document.editable_node(),
            }],
            caret: Some(document.a11y_caret()),
        },
    );
}
