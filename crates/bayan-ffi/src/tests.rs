//! Tests of the C interface, calling the exported functions the way a C program does. They avoid anything Miri cannot run, so `cargo +nightly miri test -p bayan-ffi --lib` checks them for undefined behaviour too.

use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::*;

/// Collects the messages the engine sends to a callback.
#[derive(Default)]
struct Collector {
    messages: Mutex<Vec<String>>,
    arrived: Condvar,
}

impl Collector {
    /// Waits until a message satisfies `found`, and returns it.
    fn wait_for(&self, found: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        let mut messages = self.messages.lock().unwrap();
        loop {
            if let Some(message) = messages
                .iter()
                .map(|text| serde_json::from_str::<serde_json::Value>(text).unwrap())
                .find(|message| found(message))
            {
                return message;
            }
            let (next, timeout) = self
                .arrived
                .wait_timeout(messages, Duration::from_secs(20))
                .unwrap();
            assert!(!timeout.timed_out(), "the engine did not answer");
            messages = next;
        }
    }

    fn reply(&self, id: u64) -> serde_json::Value {
        self.wait_for(|message| message["re"] == serde_json::json!(id))
    }
}

/// The callback the tests register: `user_data` is a `Collector`.
unsafe extern "C-unwind" fn collect(user_data: *mut c_void, json: *const u8, json_len: usize) {
    // SAFETY: the tests register this callback with a pointer to a `Collector` that outlives the engine.
    let collector = unsafe { &*user_data.cast::<Collector>() };
    // SAFETY: the engine passes `json_len` readable bytes that stay valid during this call.
    let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    collector
        .messages
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(text);
    collector.arrived.notify_all();
}

fn user_data(collector: &Collector) -> *mut c_void {
    ptr::from_ref(collector).cast_mut().cast::<c_void>()
}

/// Creates an engine with a configuration and registers `collect` with `collector`.
fn start(config: &str, collector: &Collector) -> *mut BayanEngine {
    // SAFETY: `config` is readable for the whole call.
    let engine = unsafe { bayan_engine_new(config.as_ptr(), config.len()) };
    assert!(!engine.is_null());
    // SAFETY: `engine` is live, and `collector` outlives it in every test.
    let status = unsafe { bayan_engine_set_callback(engine, Some(collect), user_data(collector)) };
    assert_eq!(status, BAYAN_STATUS_OK);
    engine
}

fn post(engine: *mut BayanEngine, message: &str) -> BayanStatus {
    // SAFETY: `engine` is live and `message` is readable for the whole call.
    unsafe { bayan_engine_post(engine, message.as_ptr(), message.len()) }
}

fn put(engine: *mut BayanEngine, bytes: &[u8]) -> BayanBlobId {
    // SAFETY: `engine` is live and `bytes` is readable for the whole call.
    unsafe { bayan_blob_put(engine, bytes.as_ptr(), bytes.len()) }
}

fn get(engine: *mut BayanEngine, blob: BayanBlobId) -> Result<Vec<u8>, BayanStatus> {
    let mut length = 0_usize;
    // SAFETY: `engine` is live; a null buffer asks for the size only.
    let status = unsafe { bayan_blob_get(engine, blob, ptr::null_mut(), 0, &raw mut length) };
    if status != BAYAN_STATUS_BUFFER_TOO_SMALL && !(status == BAYAN_STATUS_OK && length == 0) {
        return Err(status);
    }
    let mut bytes = vec![0_u8; length];
    // SAFETY: `bytes` has room for `length` bytes.
    let status = unsafe {
        bayan_blob_get(
            engine,
            blob,
            bytes.as_mut_ptr(),
            bytes.len(),
            &raw mut length,
        )
    };
    if status == BAYAN_STATUS_OK {
        Ok(bytes)
    } else {
        Err(status)
    }
}

fn render(
    engine: *mut BayanEngine,
    request: &str,
    width: u32,
    height: u32,
    stride: usize,
    out: &mut [u8],
) -> BayanStatus {
    // SAFETY: `engine` is live, `request` is readable, and `out` is writable for the whole call.
    unsafe {
        bayan_render_tile(
            engine,
            request.as_ptr(),
            request.len(),
            out.as_mut_ptr(),
            out.len(),
            width,
            height,
            stride,
        )
    }
}

fn free(engine: *mut BayanEngine) {
    // SAFETY: `engine` came from `bayan_engine_new` and is freed once.
    unsafe { bayan_engine_free(engine) };
}

/// Says hello and opens the mock document; returns its identifier.
fn open(engine: *mut BayanEngine, collector: &Collector) -> u64 {
    assert_eq!(
        post(
            engine,
            r#"{"v":0,"id":1,"type":"hello","payload":{"protocol_versions":[0]}}"#
        ),
        BAYAN_STATUS_OK
    );
    assert_eq!(collector.reply(1)["type"], "welcome");
    let blob = put(engine, b"");
    assert_ne!(blob, 0);
    let message = format!(r#"{{"v":0,"id":2,"type":"doc.open","payload":{{"blob":{blob}}}}}"#);
    assert_eq!(post(engine, &message), BAYAN_STATUS_OK);
    collector.reply(2)["payload"]["doc_id"].as_u64().unwrap()
}

fn page_request(doc_id: u64) -> String {
    format!(
        r#"{{"doc_id":{doc_id},"page":0,"rect":{{"x":0,"y":0,"width":15544800,"height":20116800}}}}"#
    )
}

#[test]
fn reports_its_version() {
    // SAFETY: `bayan_version` returns a static NUL-terminated string.
    let version = unsafe { CStr::from_ptr(bayan_version()) };
    assert_eq!(version.to_str().unwrap(), bayan_engine::ENGINE_VERSION);
}

#[test]
fn refuses_invalid_configurations_and_accepts_none() {
    let bad = r#"{"test":{"no_such_switch":true}}"#;
    // SAFETY: `bad` is readable for the whole call.
    let refused = unsafe { bayan_engine_new(bad.as_ptr(), bad.len()) };
    assert!(refused.is_null());
    // SAFETY: a null configuration of length 0 means the defaults.
    let engine = unsafe { bayan_engine_new(ptr::null(), 0) };
    assert!(!engine.is_null());
    free(engine);
    free(ptr::null_mut());
}

#[test]
fn handshake_open_render_record_and_replay() {
    let collector = Collector::default();
    let engine = start("{}", &collector);
    let doc_id = open(engine, &collector);
    assert_eq!(
        post(engine, r#"{"v":0,"id":3,"type":"diag.record.start"}"#),
        BAYAN_STATUS_OK
    );
    collector.reply(3);
    // A tile of 34 × 44 pixels in rows padded to 140 bytes.
    let (width, height, stride) = (34_u32, 44_u32, 140_usize);
    let mut buffer = vec![0xEE_u8; stride * 43 + 34 * 4];
    assert_eq!(
        render(
            engine,
            &page_request(doc_id),
            width,
            height,
            stride,
            &mut buffer
        ),
        BAYAN_STATUS_OK
    );
    let tight: Vec<u8> = buffer
        .chunks(stride)
        .flat_map(|row| row[..34 * 4].to_vec())
        .collect();
    assert_eq!(tight.len(), 34 * 44 * 4);
    // The padding between rows is left alone.
    assert_eq!(buffer[34 * 4], 0xEE);
    let first_hash = bayan_engine::digest::of_bytes(&tight);
    let stop = r#"{"v":0,"id":4,"type":"diag.record.stop"}"#;
    assert_eq!(post(engine, stop), BAYAN_STATUS_OK);
    let recording = collector.reply(4)["payload"]["blob"].as_u64().unwrap();
    let bytes = get(engine, recording).unwrap();
    assert!(bytes.starts_with(b"{\"format\":\"bayan-engine-recording\""));
    let replay =
        format!(r#"{{"v":0,"id":5,"type":"diag.replay","payload":{{"blob":{recording}}}}}"#);
    assert_eq!(post(engine, &replay), BAYAN_STATUS_OK);
    let report = collector.reply(5);
    assert_eq!(report["payload"]["identical"], true);
    assert_eq!(report["payload"]["tiles"][0]["hash"], first_hash.as_str());
    // SAFETY: `engine` is live.
    let released = unsafe { bayan_blob_release(engine, recording) };
    assert_eq!(released, BAYAN_STATUS_OK);
    free(engine);
}

#[test]
fn checks_every_argument() {
    let collector = Collector::default();
    let engine = start("", &collector);
    // Posting.
    assert_eq!(post(ptr::null_mut(), "{}"), BAYAN_STATUS_INVALID_ARGUMENT);
    assert_eq!(post(engine, ""), BAYAN_STATUS_INVALID_ARGUMENT);
    // SAFETY: a null message is refused before it is read.
    let null_message = unsafe { bayan_engine_post(engine, ptr::null(), 5) };
    assert_eq!(null_message, BAYAN_STATUS_INVALID_ARGUMENT);
    // SAFETY: the length is refused before the pointer is read.
    let oversized = unsafe { bayan_engine_post(engine, b"x".as_ptr(), MAX_MESSAGE_BYTES + 1) };
    assert_eq!(oversized, BAYAN_STATUS_INVALID_ARGUMENT);
    // The callback can be set only before the first post.
    assert_eq!(post(engine, "{}"), BAYAN_STATUS_OK);
    // SAFETY: `engine` is live.
    let again = unsafe { bayan_engine_set_callback(engine, Some(collect), user_data(&collector)) };
    assert_eq!(again, BAYAN_STATUS_INVALID_ARGUMENT);
    // SAFETY: `engine` is live; a null callback is refused.
    let no_callback = unsafe { bayan_engine_set_callback(engine, None, ptr::null_mut()) };
    assert_eq!(no_callback, BAYAN_STATUS_INVALID_ARGUMENT);
    // Blobs: the two-call pattern, empty blobs, unknown blobs.
    let blob = put(engine, b"hello");
    assert_eq!(get(engine, blob).unwrap(), b"hello");
    let mut small = [0_u8; 2];
    let mut length = 0_usize;
    // SAFETY: `small` has room for two bytes.
    let status = unsafe {
        bayan_blob_get(
            engine,
            blob,
            small.as_mut_ptr(),
            small.len(),
            &raw mut length,
        )
    };
    assert_eq!((status, length), (BAYAN_STATUS_BUFFER_TOO_SMALL, 5));
    // SAFETY: a null length pointer is refused.
    let no_length = unsafe { bayan_blob_get(engine, blob, ptr::null_mut(), 0, ptr::null_mut()) };
    assert_eq!(no_length, BAYAN_STATUS_INVALID_ARGUMENT);
    let empty = put(engine, b"");
    assert_eq!(get(engine, empty).unwrap(), b"");
    // SAFETY: a null pointer with length 0 is an empty blob.
    let empty_blob = unsafe { bayan_blob_put(engine, ptr::null(), 0) };
    assert_ne!(empty_blob, 0);
    // SAFETY: a null pointer with a length is refused.
    let null_with_length = unsafe { bayan_blob_put(engine, ptr::null(), 3) };
    assert_eq!(null_with_length, 0);
    assert_eq!(get(engine, 999), Err(BAYAN_STATUS_NOT_FOUND));
    // SAFETY: `engine` is live.
    let released = unsafe { bayan_blob_release(engine, blob) };
    assert_eq!(released, BAYAN_STATUS_OK);
    // SAFETY: `engine` is live.
    let released_again = unsafe { bayan_blob_release(engine, blob) };
    assert_eq!(released_again, BAYAN_STATUS_NOT_FOUND);
    // Tiles.
    let mut buffer = vec![0_u8; 64];
    let request = page_request(1);
    assert_eq!(
        render(engine, &request, 0, 4, 16, &mut buffer),
        BAYAN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        render(engine, &request, 4, 4, 15, &mut buffer),
        BAYAN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        render(engine, &request, 4, 5, 16, &mut buffer),
        BAYAN_STATUS_BUFFER_TOO_SMALL
    );
    assert_eq!(
        render(engine, &request, 5000, 1, 20_000, &mut buffer),
        BAYAN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        render(engine, "{", 4, 4, 16, &mut buffer),
        BAYAN_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        render(engine, &request, 4, 4, 16, &mut buffer),
        BAYAN_STATUS_NOT_FOUND
    );
    // SAFETY: a null buffer is refused before it is written.
    let null_out = unsafe {
        bayan_render_tile(
            engine,
            request.as_ptr(),
            request.len(),
            ptr::null_mut(),
            64,
            4,
            4,
            16,
        )
    };
    assert_eq!(null_out, BAYAN_STATUS_INVALID_ARGUMENT);
    free(engine);
}

#[test]
fn a_panic_in_a_handler_becomes_engine_error_and_the_engine_stays_usable() {
    let collector = Collector::default();
    let engine = start(r#"{"test":{"allow_panic":true}}"#, &collector);
    open(engine, &collector);
    assert_eq!(
        post(engine, r#"{"v":0,"id":3,"type":"diag.panic"}"#),
        BAYAN_STATUS_OK
    );
    assert_eq!(collector.reply(3)["error"]["code"], "panic");
    let error = collector.wait_for(|message| message["type"] == "engine.error");
    assert_eq!(error["payload"]["recoverable"], false);
    // A new session works on the same engine.
    assert_eq!(
        post(
            engine,
            r#"{"v":0,"id":4,"type":"hello","payload":{"protocol_versions":[0]}}"#
        ),
        BAYAN_STATUS_OK
    );
    assert_eq!(collector.reply(4)["ok"], true);
    free(engine);
}

/// A callback that tries to render a tile from inside the callback, and records the status.
unsafe extern "C-unwind" fn render_from_callback(
    user_data: *mut c_void,
    _json: *const u8,
    _json_len: usize,
) {
    // SAFETY: the test registers this callback with a pointer to a `Reentrant` that outlives the engine.
    let reentrant = unsafe { &*user_data.cast::<Reentrant>() };
    let engine = *reentrant.engine.lock().unwrap();
    let mut buffer = [0_u8; 4];
    let status = render(engine.0, "{}", 1, 1, 4, &mut buffer);
    reentrant.statuses.lock().unwrap().push(status);
    reentrant.done.notify_all();
}

#[derive(Clone, Copy)]
struct EnginePointer(*mut BayanEngine);

#[expect(
    unsafe_code,
    reason = "a test hands the engine pointer to its callback"
)]
// SAFETY: the tests share the engine pointer only with the engine's own callback, which runs while the engine is alive.
unsafe impl Send for EnginePointer {}

struct Reentrant {
    engine: Mutex<EnginePointer>,
    statuses: Mutex<Vec<BayanStatus>>,
    done: Condvar,
}

#[test]
fn only_the_engine_s_own_panics_are_kept_quiet() {
    // A test thread is not the engine: its panics, such as failed assertions, are printed as usual.
    assert!(!is_engine_panic());
    // Inside a C function, or on an engine thread (bayan-engine tests that flag), a panic is reported as engine.error and not printed.
    assert!(guarded(false, is_engine_panic));
    assert!(!is_engine_panic());
    // A panic inside a guarded call gives the fallback and leaves the flag as it was.
    assert!(guarded(true, || -> bool { panic!("a test panic") }));
    assert!(!is_engine_panic());
}

#[test]
fn an_engine_can_be_used_from_several_threads_and_freed_on_any() {
    // Calls borrow the engine from any thread at once (`Sync`), and `bayan_engine_free` may drop it on the engine thread (`Send`).
    fn shareable<T: Send + Sync>() {}
    shareable::<BayanEngine>();
}

#[test]
fn calling_back_into_the_engine_from_the_callback_is_refused_instead_of_deadlocking() {
    let reentrant = Reentrant {
        engine: Mutex::new(EnginePointer(ptr::null_mut())),
        statuses: Mutex::new(Vec::new()),
        done: Condvar::new(),
    };
    // SAFETY: a null configuration of length 0 means the defaults.
    let engine = unsafe { bayan_engine_new(ptr::null(), 0) };
    *reentrant.engine.lock().unwrap() = EnginePointer(engine);
    let data = ptr::from_ref(&reentrant).cast_mut().cast::<c_void>();
    // SAFETY: `engine` is live and `reentrant` outlives it.
    let registered = unsafe { bayan_engine_set_callback(engine, Some(render_from_callback), data) };
    assert_eq!(registered, BAYAN_STATUS_OK);
    assert_eq!(post(engine, "[]"), BAYAN_STATUS_OK);
    let mut statuses = reentrant.statuses.lock().unwrap();
    while statuses.is_empty() {
        let (next, timeout) = reentrant
            .done
            .wait_timeout(statuses, Duration::from_secs(20))
            .unwrap();
        assert!(!timeout.timed_out(), "the callback never ran");
        statuses = next;
    }
    assert_eq!(statuses[0], BAYAN_STATUS_WRONG_THREAD);
    drop(statuses);
    free(engine);
}

/// A callback that frees the engine from inside the callback.
unsafe extern "C-unwind" fn free_from_callback(
    user_data: *mut c_void,
    _json: *const u8,
    _json_len: usize,
) {
    // SAFETY: the test registers this callback with a pointer to a `Reentrant` that outlives the engine.
    let reentrant = unsafe { &*user_data.cast::<Reentrant>() };
    let mut engine = reentrant.engine.lock().unwrap();
    if !engine.0.is_null() {
        free(engine.0);
        *engine = EnginePointer(ptr::null_mut());
    }
    reentrant.statuses.lock().unwrap().push(BAYAN_STATUS_OK);
    reentrant.done.notify_all();
}

#[test]
fn freeing_the_engine_from_the_callback_returns_and_stops_further_callbacks() {
    let reentrant = Reentrant {
        engine: Mutex::new(EnginePointer(ptr::null_mut())),
        statuses: Mutex::new(Vec::new()),
        done: Condvar::new(),
    };
    // SAFETY: a null configuration of length 0 means the defaults.
    let engine = unsafe { bayan_engine_new(ptr::null(), 0) };
    // `bayan_engine_free` must be the last call on an engine. The callback takes this lock before it frees the engine, so holding it while posting makes the callback wait until both posts have returned.
    let mut shared_engine = reentrant.engine.lock().unwrap();
    *shared_engine = EnginePointer(engine);
    let data = ptr::from_ref(&reentrant).cast_mut().cast::<c_void>();
    // SAFETY: `engine` is live and `reentrant` outlives it.
    let registered = unsafe { bayan_engine_set_callback(engine, Some(free_from_callback), data) };
    assert_eq!(registered, BAYAN_STATUS_OK);
    // Two messages, so the second callback would come after the free.
    assert_eq!(post(engine, "[]"), BAYAN_STATUS_OK);
    assert_eq!(post(engine, "[]"), BAYAN_STATUS_OK);
    drop(shared_engine);
    let mut statuses = reentrant.statuses.lock().unwrap();
    while statuses.is_empty() {
        let (next, timeout) = reentrant
            .done
            .wait_timeout(statuses, Duration::from_secs(20))
            .unwrap();
        assert!(!timeout.timed_out(), "the callback never ran");
        statuses = next;
    }
    drop(statuses);
    // Give a stray second callback the chance to run; it must not.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(reentrant.statuses.lock().unwrap().len(), 1);
    assert!(reentrant.engine.lock().unwrap().0.is_null());
}

#[test]
fn calling_another_engine_synchronously_from_a_callback_is_refused_too() {
    // Two engines whose callbacks rendered on each other's engine would wait for each other forever.
    let reentrant = Reentrant {
        engine: Mutex::new(EnginePointer(ptr::null_mut())),
        statuses: Mutex::new(Vec::new()),
        done: Condvar::new(),
    };
    // SAFETY: a null configuration of length 0 means the defaults.
    let first = unsafe { bayan_engine_new(ptr::null(), 0) };
    // SAFETY: as above.
    let second = unsafe { bayan_engine_new(ptr::null(), 0) };
    *reentrant.engine.lock().unwrap() = EnginePointer(second);
    let data = ptr::from_ref(&reentrant).cast_mut().cast::<c_void>();
    // SAFETY: `first` is live and `reentrant` outlives it.
    let registered = unsafe { bayan_engine_set_callback(first, Some(render_from_callback), data) };
    assert_eq!(registered, BAYAN_STATUS_OK);
    assert_eq!(post(first, "[]"), BAYAN_STATUS_OK);
    let mut statuses = reentrant.statuses.lock().unwrap();
    while statuses.is_empty() {
        let (next, timeout) = reentrant
            .done
            .wait_timeout(statuses, Duration::from_secs(20))
            .unwrap();
        assert!(!timeout.timed_out(), "the callback never ran");
        statuses = next;
    }
    assert_eq!(statuses[0], BAYAN_STATUS_WRONG_THREAD);
    drop(statuses);
    free(first);
    free(second);
}

/// What `post_from_callback` shares with its test.
struct Poster {
    engine: AtomicPtr<BayanEngine>,
    statuses: Mutex<Vec<BayanStatus>>,
    posted: Condvar,
}

/// A callback that answers every message by posting another one, so the engine thread keeps calling it.
unsafe extern "C-unwind" fn post_from_callback(
    user_data: *mut c_void,
    _json: *const u8,
    _json_len: usize,
) {
    // SAFETY: the test registers this callback with a pointer to a `Poster` that outlives the engine.
    let poster = unsafe { &*user_data.cast::<Poster>() };
    let status = post(poster.engine.load(Ordering::SeqCst), "[]");
    poster.statuses.lock().unwrap().push(status);
    poster.posted.notify_all();
}

#[test]
fn freeing_while_the_callback_posts_is_safe() {
    let poster = Poster {
        engine: AtomicPtr::new(ptr::null_mut()),
        statuses: Mutex::new(Vec::new()),
        posted: Condvar::new(),
    };
    // SAFETY: a null configuration of length 0 means the defaults.
    let engine = unsafe { bayan_engine_new(ptr::null(), 0) };
    poster.engine.store(engine, Ordering::SeqCst);
    let data = ptr::from_ref(&poster).cast_mut().cast::<c_void>();
    // SAFETY: `engine` is live and `poster` outlives it.
    let registered = unsafe { bayan_engine_set_callback(engine, Some(post_from_callback), data) };
    assert_eq!(registered, BAYAN_STATUS_OK);
    assert_eq!(post(engine, "[]"), BAYAN_STATUS_OK);
    // Wait until the callback is busy posting, then free the engine from this thread while it does.
    let mut statuses = poster.statuses.lock().unwrap();
    while statuses.len() < 3 {
        let (next, timeout) = poster
            .posted
            .wait_timeout(statuses, Duration::from_secs(20))
            .unwrap();
        assert!(!timeout.timed_out(), "the callback did not keep posting");
        statuses = next;
    }
    drop(statuses);
    free(engine);
    let after_free = poster.statuses.lock().unwrap().clone();
    // A post made while the engine was stopping failed instead of reaching a freed engine.
    assert!(
        after_free
            .iter()
            .all(|status| *status == BAYAN_STATUS_OK || *status == BAYAN_STATUS_INTERNAL_ERROR),
        "{after_free:?}"
    );
    // And the callback is never called again.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(poster.statuses.lock().unwrap().len(), after_free.len());
}

#[test]
fn empty_blobs_and_callback_registration_behave_as_the_header_says() {
    // SAFETY: a null configuration of length 0 means the defaults.
    let engine = unsafe { bayan_engine_new(ptr::null(), 0) };
    // An empty blob may come from a null pointer, and asking for its size returns OK with 0.
    // SAFETY: a null pointer with length 0 is an empty blob.
    let empty = unsafe { bayan_blob_put(engine, ptr::null(), 0) };
    assert_ne!(empty, 0);
    let mut length = 1_usize;
    // SAFETY: `engine` is live; a null buffer without room asks for the size.
    let status = unsafe { bayan_blob_get(engine, empty, ptr::null_mut(), 0, &raw mut length) };
    assert_eq!((status, length), (BAYAN_STATUS_OK, 0));
    // SAFETY: a null pointer with a length is refused before it is used.
    let refused = unsafe { bayan_blob_put(engine, ptr::null(), 4) };
    assert_eq!(refused, 0);
    // Until the first post, registering again replaces the callback; after it, registering is refused.
    let replaced = Collector::default();
    let current = Collector::default();
    // SAFETY: `engine` is live, and both collectors outlive it.
    let first = unsafe { bayan_engine_set_callback(engine, Some(collect), user_data(&replaced)) };
    // SAFETY: as above.
    let second = unsafe { bayan_engine_set_callback(engine, Some(collect), user_data(&current)) };
    assert_eq!((first, second), (BAYAN_STATUS_OK, BAYAN_STATUS_OK));
    assert_eq!(
        post(
            engine,
            r#"{"v":0,"id":1,"type":"hello","payload":{"protocol_versions":[0]}}"#
        ),
        BAYAN_STATUS_OK
    );
    assert_eq!(current.reply(1)["type"], "welcome");
    // SAFETY: as above.
    let late = unsafe { bayan_engine_set_callback(engine, Some(collect), user_data(&replaced)) };
    assert_eq!(late, BAYAN_STATUS_INVALID_ARGUMENT);
    free(engine);
    assert!(replaced.messages.lock().unwrap().is_empty());
}

/// A callback that records whether a panic at this point would count as the engine's: `user_data` is an `AtomicU8`, set to 1 for yes and 2 for no.
unsafe extern "C-unwind" fn record_quietness(
    user_data: *mut c_void,
    _json: *const u8,
    _json_len: usize,
) {
    // SAFETY: the test registers this callback with a pointer to an `AtomicU8` that outlives the engine.
    let seen = unsafe { &*user_data.cast::<std::sync::atomic::AtomicU8>() };
    seen.store(if is_engine_panic() { 1 } else { 2 }, Ordering::SeqCst);
}

#[test]
fn panics_inside_the_shell_s_callback_are_the_shell_s() {
    // The callback runs on the engine thread, but it is the shell's code: if it is written in Rust and panics, the panic is printed as usual, and the process stops (the callback must not unwind).
    let seen = std::sync::atomic::AtomicU8::new(0);
    // SAFETY: a null configuration of length 0 means the defaults.
    let engine = unsafe { bayan_engine_new(ptr::null(), 0) };
    let data = ptr::from_ref(&seen).cast_mut().cast::<c_void>();
    // SAFETY: `engine` is live, and `seen` outlives it.
    let status = unsafe { bayan_engine_set_callback(engine, Some(record_quietness), data) };
    assert_eq!(status, BAYAN_STATUS_OK);
    assert_eq!(post(engine, "{}"), BAYAN_STATUS_OK);
    let start = std::time::Instant::now();
    while seen.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < Duration::from_secs(20), "no callback");
        std::thread::yield_now();
    }
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    free(engine);
}

#[test]
fn blobs_and_tiles_are_written_into_buffers_that_were_never_initialized() {
    // A shell's buffers may be fresh memory, such as a new QImage's pixels: the engine writes them through raw pointers and never reads them (checked by Miri).
    let collector = Collector::default();
    let engine = start("", &collector);
    let doc_id = open(engine, &collector);
    let blob = put(engine, b"some bytes");
    let mut out = Box::<[u8]>::new_uninit_slice(10);
    let mut length = 0_usize;
    // SAFETY: `engine` is live, and `out` has room for 10 bytes, which the engine only writes.
    let status = unsafe {
        bayan_blob_get(
            engine,
            blob,
            out.as_mut_ptr().cast::<u8>(),
            out.len(),
            &raw mut length,
        )
    };
    assert_eq!((status, length), (BAYAN_STATUS_OK, 10));
    // SAFETY: the engine wrote all 10 bytes.
    let out = unsafe { out.assume_init() };
    assert_eq!(&*out, b"some bytes");
    // A tile of 3 x 2 pixels whose rows start 16 bytes apart: the 4 bytes after the first row stay uninitialized and are never read.
    let (width, height, stride) = (3_u32, 2_u32, 16_usize);
    let capacity = stride + 12;
    let mut tile = Box::<[u8]>::new_uninit_slice(capacity);
    let request = page_request(doc_id);
    // SAFETY: `engine` is live, `request` is readable, and `tile` has room for `capacity` bytes, which the engine only writes.
    let status = unsafe {
        bayan_render_tile(
            engine,
            request.as_ptr(),
            request.len(),
            tile.as_mut_ptr().cast::<u8>(),
            capacity,
            width,
            height,
            stride,
        )
    };
    assert_eq!(status, BAYAN_STATUS_OK);
    for start in [0, stride] {
        let pixels: Vec<u8> = tile[start..start + 12]
            .iter()
            // SAFETY: the engine wrote the 12 bytes of each row.
            .map(|byte| unsafe { byte.assume_init() })
            .collect();
        // Every pixel lies on the page, so it is opaque.
        assert!(pixels.chunks(4).all(|pixel| pixel[3] == 255), "{pixels:?}");
    }
    free(engine);
}
