//! # bayan-ffi
//!
//! The C ABI of the engine, for the desktop shell.
//!
//! The desktop shell (C++ and Qt) calls the engine through nine C functions ([engine protocol specification][protocol-spec] §3.1, [ADR-0012] §3). They wrap [`bayan_engine::thread::EngineThread`], which runs the engine on its own thread, and the engine's blob store. cbindgen writes their C header, `include/bayan_ffi.h`, from this file; a test fails when the committed header is out of date. This crate is built for the desktop platforms only, not for WebAssembly (the web uses bayan-wasm).
//!
//! **Rules for callers** (also in the header):
//!
//! - All JSON is UTF-8 and is passed with an explicit length; it is not NUL-terminated. Pointers passed to the engine are borrowed for the duration of the call only.
//! - The engine runs on its own thread and calls the message callback on that thread, one message at a time. The `json` pointer given to the callback is valid only during the call: copy the bytes and hand them to your own thread.
//! - Never call back into the engine synchronously from inside the callback (spec §8): `bayan_render_tile` returns `BAYAN_STATUS_WRONG_THREAD` there, because it would wait for the thread it runs on. Posting from inside the callback is allowed.
//! - Every function may be called from any thread, also from several threads at once, except `bayan_engine_free`: it must be the last call on an engine, made when no other thread is inside a call on that engine. It stops the engine; after it returns, the callback is never called again.
//! - No function lets a Rust panic escape; failures are reported through return values and `engine.error` messages. The callback must not let a C++ exception escape either.
//! - Panics inside the engine are not printed, because their messages could quote document content: the first `bayan_engine_new` installs a panic hook that stays silent for the engine's threads and calls, and hands every other panic in the process to the hook installed before.
//!
//! **Unsafe code.** Talking to C requires `unsafe` code, so this is one of the two crates where it is not forbidden ([ADR-0006] §2). It is still denied by default and allowed only where `#[expect(unsafe_code, reason = "…")]` marks it: the exported functions (whose `#[unsafe(no_mangle)]` attribute and raw pointers are unsafe by nature), the call of the shell's callback, and the pointer helpers in `pointers.rs`, which Miri checks. A comment above every `unsafe` block explains why it is sound.
//!
//! ## Layer
//!
//! bayan-ffi belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-ffi may therefore depend on crates of every layer and on external libraries.
//!
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [protocol-spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md
//! [ADR-0012]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0012-engine-boundary.md
//! [ADR-0006]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0006-rust-core-and-memory-safety.md

mod pointers;

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Arc, Once};
use std::time::Instant;

use bayan_engine::TileError;
use bayan_engine::blobs::BlobStore;
use bayan_engine::config::Config;
use bayan_engine::limits::{MAX_BLOB_BYTES, MAX_MESSAGE_BYTES, MAX_TILE_SIDE};
use bayan_engine::thread::{EngineThread, on_engine_thread};

/// An engine instance. Opaque: only pointers to it exist outside the engine.
pub struct BayanEngine {
    thread: EngineThread,
    blobs: Arc<BlobStore>,
    started: Instant,
}

impl BayanEngine {
    /// Milliseconds since the engine was created, the clock of its recordings (spec §10).
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Identifies a blob (bulk bytes kept by the engine, such as file contents). 0 is never a valid blob.
pub type BayanBlobId = u64;

/// The result of a call.
pub type BayanStatus = i32;

/// The call succeeded.
pub const BAYAN_STATUS_OK: BayanStatus = 0;
/// A null pointer, a malformed request, or a size outside the engine's limits (spec §13).
pub const BAYAN_STATUS_INVALID_ARGUMENT: BayanStatus = 1;
/// An unknown blob, document or page.
pub const BAYAN_STATUS_NOT_FOUND: BayanStatus = 2;
/// The output buffer is too small; the function says how to learn the size needed.
pub const BAYAN_STATUS_BUFFER_TOO_SMALL: BayanStatus = 3;
/// The engine failed or is stopping; it also reports an engine.error message when it can.
pub const BAYAN_STATUS_INTERNAL_ERROR: BayanStatus = 4;
/// Called from inside the engine's callback, where the function would wait for the engine thread it runs on (spec §8).
pub const BAYAN_STATUS_WRONG_THREAD: BayanStatus = 5;

/// Receives one message from the engine (a reply or an event, as UTF-8 JSON of `json_len` bytes), on the engine thread. `json` is valid only during the call.
pub type BayanMessageCallback =
    Option<unsafe extern "C" fn(user_data: *mut c_void, json: *const u8, json_len: usize)>;

/// The engine's semantic version, NUL-terminated.
const VERSION: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(version) => version,
        Err(_) => panic!("the crate version contains a NUL byte"),
    };

/// The shell's callback and its user data, handed to the engine thread.
#[derive(Clone, Copy)]
struct Callback {
    function: unsafe extern "C" fn(user_data: *mut c_void, json: *const u8, json_len: usize),
    user_data: *mut c_void,
}

#[expect(
    unsafe_code,
    reason = "the shell's callback runs on the engine thread, as the C interface documents"
)]
// SAFETY: the C interface requires `user_data` to be usable from the engine thread (bayan_ffi.h, "Rules for callers"), and the callback is only ever called from that thread, one message at a time; `Callback` itself is two plain pointers.
unsafe impl Send for Callback {}

#[expect(
    unsafe_code,
    reason = "the shell's callback runs on the engine thread, as the C interface documents"
)]
// SAFETY: as for `Send`: the engine thread is the only thread that calls the callback, so sharing the pointers cannot race.
unsafe impl Sync for Callback {}

impl Callback {
    #[expect(unsafe_code, reason = "calls the shell's C callback")]
    fn deliver(&self, message: &str) {
        // SAFETY: the shell registered `function` for exactly this signature and promised that it may be called on the engine thread with `user_data` (bayan_ffi.h); `message` stays valid for the whole call, which is all the callback may rely on.
        unsafe { (self.function)(self.user_data, message.as_ptr(), message.len()) };
    }
}

thread_local! {
    /// Whether this thread is inside one of the C functions.
    static IN_CALL: Cell<bool> = const { Cell::new(false) };
}

/// Runs `body`, turning a panic into `fallback`, so no panic can cross into C (ADR-0006 §4).
fn guarded<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    let outer = IN_CALL.with(|in_call| in_call.replace(true));
    let result = catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback);
    IN_CALL.with(|in_call| in_call.set(outer));
    result
}

/// Keeps the engine's panics off the host's standard error. Rust's default panic hook prints a panic's message, which could quote document content, and desktop environments may keep a program's standard error in the system log (AGENTS.md §6). The engine reports its panics as `engine.error` instead, so the hook installed here prints nothing for a panic on an engine thread or inside one of the C functions, and hands every other panic of the process to the hook that was installed before it. It is installed once, by the first `bayan_engine_new`.
fn silence_engine_panics() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !is_engine_panic() {
                previous(info);
            }
        }));
    });
}

/// Whether the current thread is an engine thread, or is inside one of the C functions. Reads only thread-local flags, which a panic hook may do even while a thread is ending.
fn is_engine_panic() -> bool {
    on_engine_thread() || IN_CALL.try_with(Cell::get).unwrap_or(false)
}

/// Returns the engine's semantic version, as a static NUL-terminated string.
#[expect(unsafe_code, reason = "exported to C without name mangling")]
#[unsafe(no_mangle)]
pub extern "C" fn bayan_version() -> *const c_char {
    VERSION.as_ptr()
}

/// Creates an engine from a JSON configuration object of `config_len` bytes (spec §3.3); a null pointer with length 0 means the defaults. Starts the engine thread. Returns NULL if the configuration is invalid or the thread cannot start.
///
/// # Safety
///
/// Unless `config_len` is 0, `config_json` must point to `config_len` readable bytes.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and reads memory that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_engine_new(
    config_json: *const u8,
    config_len: usize,
) -> *mut BayanEngine {
    guarded(ptr::null_mut(), || {
        silence_engine_panics();
        // SAFETY: the caller guarantees `config_len` readable bytes at `config_json` for the duration of this call.
        let Some(json) =
            (unsafe { pointers::borrowed(config_json, config_len, MAX_MESSAGE_BYTES) })
        else {
            return ptr::null_mut();
        };
        let Ok(config) = Config::from_json(json) else {
            return ptr::null_mut();
        };
        let blobs = Arc::new(BlobStore::new());
        let Ok(thread) = EngineThread::spawn(config, Arc::clone(&blobs)) else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(BayanEngine {
            thread,
            blobs,
            started: Instant::now(),
        }))
    })
}

/// Stops and destroys an engine; accepts NULL. Called from any other thread, it waits until the engine thread has stopped. Called from inside the callback, it returns at once and the engine thread finishes by itself. Either way, the callback is never called again after this returns.
///
/// # Safety
///
/// `engine` must be NULL or a pointer that `bayan_engine_new` returned and that was not freed yet. This must be the last call on the engine: no other thread may be inside a call on it (a callback that frees the engine must first make sure of that), and the engine must not be used afterwards.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and takes back ownership of a pointer given to C"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_engine_free(engine: *mut BayanEngine) {
    if engine.is_null() {
        return;
    }
    // SAFETY: the caller guarantees that `engine` came from `bayan_engine_new` (which made it with `Box::into_raw`) and is freed only once, so this `Box` is its only owner.
    let engine = unsafe { Box::from_raw(engine) };
    guarded((), || engine.thread.stop());
}

/// Registers the message callback. Call it once, before the first `bayan_engine_post`; later calls return `BAYAN_STATUS_INVALID_ARGUMENT`. `user_data` is passed back unchanged and must be usable from the engine thread.
///
/// # Safety
///
/// `engine` must be a live engine. `callback` must be safe to call on the engine thread with `user_data` until the engine is freed.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and uses a pointer that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_engine_set_callback(
    engine: *mut BayanEngine,
    callback: BayanMessageCallback,
    user_data: *mut c_void,
) -> BayanStatus {
    // SAFETY: the caller guarantees that a non-null `engine` is a live engine; the engine is only read, and its methods synchronize internally.
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    let Some(function) = callback else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    guarded(BAYAN_STATUS_INTERNAL_ERROR, || {
        let target = Callback {
            function,
            user_data,
        };
        match engine
            .thread
            .set_sink(Arc::new(move |message: &str| target.deliver(message)))
        {
            Ok(()) => BAYAN_STATUS_OK,
            Err(_) => BAYAN_STATUS_INVALID_ARGUMENT,
        }
    })
}

/// Queues one message (a protocol envelope of `json_len` bytes of UTF-8 JSON, spec §4) for the engine thread and returns without waiting for it to be handled. Returns `BAYAN_STATUS_INVALID_ARGUMENT` for a null pointer, an empty message or one over 16 MiB, and `BAYAN_STATUS_INTERNAL_ERROR` if the engine is stopping.
///
/// # Safety
///
/// `engine` must be a live engine, and `json` must point to `json_len` readable bytes.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and reads memory that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_engine_post(
    engine: *mut BayanEngine,
    json: *const u8,
    json_len: usize,
) -> BayanStatus {
    // SAFETY: the caller guarantees that a non-null `engine` is a live engine; it is only read.
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    if json_len == 0 {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: the caller guarantees `json_len` readable bytes at `json` for the duration of this call.
    let Some(message) = (unsafe { pointers::borrowed(json, json_len, MAX_MESSAGE_BYTES) }) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    guarded(BAYAN_STATUS_INTERNAL_ERROR, || {
        match engine.thread.post(message.to_vec(), engine.now_ms()) {
            Ok(()) => BAYAN_STATUS_OK,
            Err(_) => BAYAN_STATUS_INTERNAL_ERROR,
        }
    })
}

/// Copies `len` bytes into a new blob (an empty blob is allowed) and returns its identifier, or 0 if a pointer is null, the blob is larger than 64 MiB, or there are too many blobs (spec §13).
///
/// # Safety
///
/// `engine` must be a live engine, and unless `len` is 0, `bytes` must point to `len` readable bytes.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and reads memory that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_blob_put(
    engine: *mut BayanEngine,
    bytes: *const u8,
    len: usize,
) -> BayanBlobId {
    // SAFETY: the caller guarantees that a non-null `engine` is a live engine; it is only read.
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return 0;
    };
    // SAFETY: the caller guarantees `len` readable bytes at `bytes` for the duration of this call.
    let Some(bytes) = (unsafe { pointers::borrowed(bytes, len, MAX_BLOB_BYTES) }) else {
        return 0;
    };
    guarded(0, || engine.blobs.put_shell(bytes).unwrap_or(0))
}

/// Copies a blob's bytes into `out`, which has room for `out_capacity` bytes, and always sets `*out_len` to the blob's size. If `out` is NULL or too small, returns `BAYAN_STATUS_BUFFER_TOO_SMALL` and still sets `*out_len`, so callers can ask for the size first. Returns `BAYAN_STATUS_NOT_FOUND` for an unknown blob.
///
/// # Safety
///
/// `engine` must be a live engine, `out_len` must point to a writable `size_t`, and unless `out` is NULL, it must point to `out_capacity` writable bytes.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and writes memory that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_blob_get(
    engine: *mut BayanEngine,
    blob: BayanBlobId,
    out: *mut u8,
    out_capacity: usize,
    out_len: *mut usize,
) -> BayanStatus {
    // SAFETY: the caller guarantees that a non-null `engine` is a live engine; it is only read.
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    if out_len.is_null() {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    }
    let Some(bytes) = guarded(None, || engine.blobs.get(blob)) else {
        return BAYAN_STATUS_NOT_FOUND;
    };
    // SAFETY: `out_len` is not null and, as the caller guarantees, points to a writable `size_t`.
    unsafe { pointers::write_size(out_len, bytes.len()) };
    if bytes.is_empty() {
        return BAYAN_STATUS_OK;
    }
    // SAFETY: the caller guarantees that a non-null `out` points to `out_capacity` writable bytes for the duration of this call.
    if unsafe { pointers::copy_out(&bytes, out, out_capacity) } {
        BAYAN_STATUS_OK
    } else {
        BAYAN_STATUS_BUFFER_TOO_SMALL
    }
}

/// Releases a blob. Returns `BAYAN_STATUS_NOT_FOUND` if it did not exist.
///
/// # Safety
///
/// `engine` must be a live engine.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and uses a pointer that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_blob_release(
    engine: *mut BayanEngine,
    blob: BayanBlobId,
) -> BayanStatus {
    // SAFETY: the caller guarantees that a non-null `engine` is a live engine; it is only read.
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    guarded(BAYAN_STATUS_INTERNAL_ERROR, || {
        if engine.blobs.release(blob) {
            BAYAN_STATUS_OK
        } else {
            BAYAN_STATUS_NOT_FOUND
        }
    })
}

/// Renders a tile into a caller-provided buffer of premultiplied RGBA8 pixels, rows from top to bottom, and returns when it is done (spec §3.1, §6.4).
///
/// - `request_json`: a `render.tile` payload of `request_len` bytes: `doc_id`, `page`, `rect` (x, y, width and height in BLU), and optionally `zoom`, `device_scale` and `mode`. If it has `width` and `height`, they must equal the parameters.
/// - `width`, `height`: the tile's size in pixels, 1 to 4,096 each.
/// - `stride`: the distance between the starts of two rows, in bytes; at least 4 × `width`. The bytes between rows are left untouched.
/// - `out_capacity`: the size of `rgba_out` in bytes; at least `stride` × (`height` − 1) + 4 × `width`, otherwise `BAYAN_STATUS_BUFFER_TOO_SMALL`.
///
/// The request is handled on the engine thread in order with the posted messages, so it waits for messages posted before it. From inside the callback it returns `BAYAN_STATUS_WRONG_THREAD`.
///
/// # Safety
///
/// `engine` must be a live engine, `request_json` must point to `request_len` readable bytes, and `rgba_out` to `out_capacity` writable bytes that nothing else uses during the call.
#[expect(
    unsafe_code,
    reason = "exported to C without name mangling, and reads and writes memory that C passes in"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bayan_render_tile(
    engine: *mut BayanEngine,
    request_json: *const u8,
    request_len: usize,
    rgba_out: *mut u8,
    out_capacity: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> BayanStatus {
    // SAFETY: the caller guarantees that a non-null `engine` is a live engine; it is only read.
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    if rgba_out.is_null() || request_len == 0 || width > MAX_TILE_SIDE || height > MAX_TILE_SIDE {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    }
    let Some(needed) = pointers::tile_buffer_len(width, height, stride) else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    if out_capacity < needed {
        return BAYAN_STATUS_BUFFER_TOO_SMALL;
    }
    // SAFETY: the caller guarantees `request_len` readable bytes at `request_json` for the duration of this call.
    let Some(request) =
        (unsafe { pointers::borrowed(request_json, request_len, MAX_MESSAGE_BYTES) })
    else {
        return BAYAN_STATUS_INVALID_ARGUMENT;
    };
    let rendered = guarded(Err(TileError::Internal), || {
        engine
            .thread
            .render_tile(request.to_vec(), width, height, engine.now_ms())
    });
    let pixels = match rendered {
        Ok(pixels) => pixels,
        Err(TileError::InvalidArgument) => return BAYAN_STATUS_INVALID_ARGUMENT,
        Err(TileError::NotFound) => return BAYAN_STATUS_NOT_FOUND,
        Err(TileError::WrongThread) => return BAYAN_STATUS_WRONG_THREAD,
        Err(TileError::Internal) => return BAYAN_STATUS_INTERNAL_ERROR,
    };
    let row = usize::try_from(width)
        .unwrap_or(usize::MAX)
        .saturating_mul(4);
    // SAFETY: the caller guarantees that `rgba_out` points to `out_capacity` writable bytes that nothing else uses during this call; `write_rows` checks that the rows fit in them.
    if unsafe { pointers::write_rows(&pixels, row, rgba_out, out_capacity, stride) } {
        BAYAN_STATUS_OK
    } else {
        BAYAN_STATUS_INTERNAL_ERROR
    }
}

#[cfg(test)]
#[expect(
    unsafe_code,
    reason = "the tests call the C functions the way a C program does"
)]
mod tests;
