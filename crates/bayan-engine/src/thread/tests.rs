//! Tests of the engine thread: that it answers, recovers from panics, stops, and keeps its stack size whatever the environment says. Host only (see the declaration in `thread.rs`). They live in a file of their own, declared with `#[cfg(test)] mod tests;`, because they start child processes, read an environment variable and measure time, and the gate's outside-world check reads every line of the files that ship (`xtask/src/outside_world.rs`); the compiler does not read this file for the code that ships.

use std::process::Command;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use super::*;

const WAIT: Duration = Duration::from_secs(10);

/// The environment variable that tells [`stack_probe`] how many bytes of the engine thread's stack to use, when [`the_environment_does_not_choose_the_engine_thread_s_stack`] runs it in a child process.
const PROBE_BYTES: &str = "BAYAN_ENGINE_STACK_PROBE_BYTES";

/// Uses at least `bytes` of the stack below `top`, the address of a variable where the probe began, in frames of 16 KiB that stay in use until the frames below them return, and returns how many bytes it used. Measuring the addresses makes the probe independent of how large the compiler makes each frame.
#[inline(never)]
fn use_stack(top: usize, bytes: usize) -> usize {
    let mut frame = [0_u8; 16 * 1024];
    std::hint::black_box(&mut frame);
    let used = top.abs_diff(frame.as_ptr().addr());
    if used >= bytes {
        return used;
    }
    let deeper = use_stack(top, bytes);
    std::hint::black_box(&frame);
    deeper
}

/// A helper for [`the_environment_does_not_choose_the_engine_thread_s_stack`], which runs it in child processes: uses as many bytes of the engine thread's stack as [`PROBE_BYTES`] says, from inside the sink, which runs on the engine thread. Run without that variable, it does nothing.
#[test]
#[ignore = "a helper that the_environment_does_not_choose_the_engine_thread_s_stack runs in child processes"]
fn stack_probe() {
    let Some(bytes) = std::env::var_os(PROBE_BYTES) else {
        return;
    };
    let bytes: usize = bytes.to_str().unwrap().parse().unwrap();
    let thread = EngineThread::spawn(Config::default(), Arc::new(BlobStore::new())).unwrap();
    let (sender, used) = mpsc::channel();
    let sender = Mutex::new(sender);
    thread
        .set_sink(Arc::new(move |_message: &str| {
            let top = 0_u8;
            let used = use_stack((&raw const top).addr(), bytes);
            let _ignored = sender.lock().unwrap().send(used);
        }))
        .unwrap();
    thread.post(b"{}".to_vec(), 0).unwrap();
    assert!(used.recv_timeout(WAIT).unwrap() >= bytes);
    thread.stop();
}

#[test]
fn the_environment_does_not_choose_the_engine_thread_s_stack() {
    // The probe runs in a child process, this test program itself, because a stack overflow ends the whole process, and because the standard library reads RUST_MIN_STACK only once per process.
    let probe = |bytes: usize, min_stack: usize| {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "thread::tests::stack_probe",
                "--include-ignored",
                "--test-threads=1",
            ])
            .env("RUST_MIN_STACK", min_stack.to_string())
            .env(PROBE_BYTES, bytes.to_string())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        (output.status.success(), stdout, stderr)
    };
    // Three quarters of the engine's stack are there, although RUST_MIN_STACK asks for a quarter.
    let (passed, stdout, stderr) = probe(
        ENGINE_THREAD_STACK_BYTES / 4 * 3,
        ENGINE_THREAD_STACK_BYTES / 4,
    );
    assert!(passed && stdout.contains("1 passed"), "{stdout}\n{stderr}");
    // A quarter more than the engine's stack overflows it, although RUST_MIN_STACK offers 32 times as much.
    let (passed, stdout, stderr) = probe(
        ENGINE_THREAD_STACK_BYTES / 4 * 5,
        ENGINE_THREAD_STACK_BYTES * 32,
    );
    assert!(
        !passed
            && stderr.contains(&format!("thread '{ENGINE_THREAD_NAME}'"))
            && stderr.contains("has overflowed its stack"),
        "{stdout}\n{stderr}"
    );
}

fn started() -> (EngineThread, Receiver<String>) {
    started_with(Arc::new(BlobStore::new()))
}

fn started_with(blobs: Arc<BlobStore>) -> (EngineThread, Receiver<String>) {
    let thread = EngineThread::spawn(Config::default(), blobs).unwrap();
    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    thread
        .set_sink(Arc::new(move |message: &str| {
            let _ignored = sender.lock().unwrap().send(message.to_owned());
        }))
        .unwrap();
    (thread, receiver)
}

#[test]
fn handles_messages_in_order_and_answers_through_the_sink() {
    let (thread, messages) = started();
    thread
        .post(
            br#"{"v":0,"id":1,"type":"hello","payload":{"protocol_versions":[0]}}"#.to_vec(),
            0,
        )
        .unwrap();
    thread
        .post(br#"{"v":0,"id":2,"type":"nope"}"#.to_vec(), 0)
        .unwrap();
    let first = messages.recv_timeout(WAIT).unwrap();
    let second = messages.recv_timeout(WAIT).unwrap();
    assert!(
        first.contains(r#""re":1"#) && first.contains(r#""type":"welcome""#),
        "{first}"
    );
    assert!(
        second.contains(r#""re":2"#) && second.contains("unsupported_message"),
        "{second}"
    );
    thread.stop();
}

#[test]
fn the_engine_thread_knows_itself() {
    assert!(!on_engine_thread());
    let thread = EngineThread::spawn(Config::default(), Arc::new(BlobStore::new())).unwrap();
    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    thread
        .set_sink(Arc::new(move |_message: &str| {
            let _ignored = sender.lock().unwrap().send(on_engine_thread());
        }))
        .unwrap();
    thread.post(b"{}".to_vec(), 0).unwrap();
    assert_eq!(receiver.recv_timeout(WAIT), Ok(true));
    thread.stop();
}

#[test]
fn rendering_from_any_engine_s_sink_is_refused() {
    // Two engines whose sinks rendered on each other would wait for each other forever.
    let other =
        Arc::new(EngineThread::spawn(Config::default(), Arc::new(BlobStore::new())).unwrap());
    let thread = EngineThread::spawn(Config::default(), Arc::new(BlobStore::new())).unwrap();
    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    let target = Arc::clone(&other);
    thread
        .set_sink(Arc::new(move |_message: &str| {
            let result = target.render_tile(b"{}".to_vec(), 1, 1, 0);
            let _ignored = sender.lock().unwrap().send(result);
        }))
        .unwrap();
    thread.post(b"{}".to_vec(), 0).unwrap();
    assert_eq!(receiver.recv_timeout(WAIT), Ok(Err(TileError::WrongThread)));
    thread.stop();
    other.stop();
}

#[test]
fn a_panic_outside_the_engine_s_own_recovery_does_not_end_the_thread() {
    let (thread, messages) = started();
    let hello = |id: u64| {
        format!(r#"{{"v":0,"id":{id},"type":"hello","payload":{{"protocol_versions":[0]}}}}"#)
            .into_bytes()
    };
    thread.post(hello(1), 0).unwrap();
    assert!(messages.recv_timeout(WAIT).unwrap().contains(r#""re":1"#));
    let (answer, waiting) = mpsc::sync_channel(1);
    thread.jobs.send(Job::Panic { answer }).unwrap();
    // The shell learns that the session is gone, and a waiting tile request fails instead of waiting forever.
    let error = messages.recv_timeout(WAIT).unwrap();
    assert!(
        error.contains(r#""type":"engine.error""#) && error.contains(r#""recoverable":false"#),
        "{error}"
    );
    assert!(waiting.recv_timeout(WAIT).is_err());
    // The thread goes on: a new handshake works.
    thread.post(hello(2), 0).unwrap();
    let welcome = messages.recv_timeout(WAIT).unwrap();
    assert!(
        welcome.contains(r#""re":2"#) && welcome.contains(r#""ok":true"#),
        "{welcome}"
    );
    thread.stop();
}

#[test]
fn the_sink_cannot_change_after_the_first_post() {
    let (thread, _messages) = started();
    thread.post(b"{}".to_vec(), 0).unwrap();
    assert_eq!(thread.set_sink(Arc::new(|_: &str| {})), Err(AlreadyPosted));
}

#[test]
fn the_sink_cannot_change_after_the_first_tile_either() {
    let (thread, _messages) = started();
    assert_eq!(
        thread.render_tile(b"{}".to_vec(), 1, 1, 0),
        Err(TileError::InvalidArgument)
    );
    assert_eq!(thread.set_sink(Arc::new(|_: &str| {})), Err(AlreadyPosted));
}

#[test]
fn rendering_from_inside_the_sink_is_refused_instead_of_waiting_forever() {
    let thread =
        Arc::new(EngineThread::spawn(Config::default(), Arc::new(BlobStore::new())).unwrap());
    let (sender, results) = mpsc::channel();
    let sender = Mutex::new(sender);
    let inner = Arc::downgrade(&thread);
    thread
        .set_sink(Arc::new(move |_: &str| {
            if let Some(thread) = inner.upgrade() {
                let result = thread.render_tile(b"{}".to_vec(), 1, 1, 0);
                let _ignored = sender.lock().unwrap().send(result);
            }
        }))
        .unwrap();
    thread.post(b"[]".to_vec(), 0).unwrap();
    assert_eq!(
        results.recv_timeout(WAIT).unwrap(),
        Err(TileError::WrongThread)
    );
}

#[test]
fn nothing_reaches_the_sink_after_stop_returns() {
    let (thread, messages) = started();
    for _ in 0..100 {
        thread.post(b"[]".to_vec(), 0).unwrap();
    }
    thread.stop();
    // Drain whatever arrived before stop returned; nothing arrives after it.
    while messages.try_recv().is_ok() {}
    assert_eq!(
        messages.recv_timeout(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout)
    );
    // Dropping the stopped engine drops the sink, and with it the channel's sender.
    drop(thread);
    assert_eq!(
        messages.recv_timeout(Duration::from_millis(50)),
        Err(RecvTimeoutError::Disconnected)
    );
}

#[test]
fn renders_tiles_in_order_with_messages() {
    let blobs = Arc::new(BlobStore::new());
    let blob = blobs.put_shell(b"").unwrap();
    let (thread, messages) = started_with(blobs);
    thread
        .post(
            br#"{"v":0,"id":1,"type":"hello","payload":{"protocol_versions":[0]}}"#.to_vec(),
            0,
        )
        .unwrap();
    thread
        .post(
            format!(r#"{{"v":0,"id":2,"type":"doc.open","payload":{{"blob":{blob}}}}}"#)
                .into_bytes(),
            0,
        )
        .unwrap();
    // The tile request waits for the document opened by the message before it.
    let request =
        br#"{"doc_id":1,"page":0,"rect":{"x":0,"y":0,"width":15544800,"height":20116800}}"#;
    let pixels = thread.render_tile(request.to_vec(), 17, 22, 0).unwrap();
    assert_eq!(pixels.len(), 17 * 22 * 4);
    assert_eq!(
        messages.recv_timeout(WAIT).map(|m| m.contains("welcome")),
        Ok(true)
    );
    thread.stop();
}
