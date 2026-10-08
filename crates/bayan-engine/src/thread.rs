//! The engine on its own thread, for native hosts such as the desktop shell ([engine protocol specification][spec] §8, ADR-0012 §3).
//!
//! [`EngineThread`] is an actor: one thread owns the [`Engine`] and handles queued work in order, one item at a time. Posting a message only queues it, so the caller never waits for the engine. Rendering a tile through [`EngineThread::render_tile`] queues the request behind the messages already posted and waits for the pixels. The engine's messages go to a [`Sink`] that runs on the engine thread; a sink must hand them to the shell's own thread and must never wait for the engine, because the engine waits for the sink.
//!
//! WebAssembly in a browser has no such thread: the web binding feeds the [`Engine`] directly, inside the Web Worker.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::cell::Cell;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{JoinHandle, ThreadId};

use crate::blobs::BlobStore;
use crate::config::Config;
use crate::engine::{Engine, TileError};

/// Receives the engine's messages, as JSON, on the engine thread.
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// The name of every engine thread, as debuggers and crash reports show it.
pub const ENGINE_THREAD_NAME: &str = "bayan-engine";

thread_local! {
    /// Whether the current thread is an engine thread; set when it starts.
    static ON_ENGINE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Whether the current thread is the thread of an [`EngineThread`], so that a host can recognize the engine's own panics. Safe to call from a panic hook, even while the thread is ending.
#[must_use]
pub fn on_engine_thread() -> bool {
    ON_ENGINE_THREAD.try_with(Cell::get).unwrap_or(false)
}

/// The sink can no longer be set: messages were already posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyPosted;

/// The engine thread has stopped or is stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

enum Job {
    Message {
        bytes: Vec<u8>,
        received_ms: u64,
    },
    Tile {
        request: Vec<u8>,
        width: u32,
        height: u32,
        received_ms: u64,
        answer: SyncSender<Result<Vec<u8>, TileError>>,
    },
    /// Wakes the engine thread so it notices that it is stopping.
    Stop,
    /// A panic outside the engine's own recovery, for tests; `answer` stands for a tile request waiting for the job.
    #[cfg(test)]
    Panic {
        answer: SyncSender<Result<Vec<u8>, TileError>>,
    },
}

/// What the shell's threads and the engine thread share.
struct Shared {
    sink: Mutex<Option<Sink>>,
    posted: AtomicBool,
    stopping: AtomicBool,
}

impl Shared {
    /// Hands messages to the sink, one at a time, and stops at once when the engine is stopping, so no message reaches the sink after [`EngineThread::stop`] returns.
    fn deliver(&self, messages: Vec<String>) {
        for message in messages {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            // The sink is cloned out of the lock, so a sink that sets itself again cannot deadlock.
            let sink = self
                .sink
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(sink) = sink {
                sink(&message);
            }
        }
    }
}

/// The engine running on its own thread.
pub struct EngineThread {
    jobs: Sender<Job>,
    shared: Arc<Shared>,
    /// Taken by the first [`EngineThread::stop`]; behind a lock so that stopping needs only a shared reference.
    thread: Mutex<Option<JoinHandle<()>>>,
    thread_id: ThreadId,
}

impl std::fmt::Debug for EngineThread {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EngineThread")
            .field("thread_id", &self.thread_id)
            .finish_non_exhaustive()
    }
}

fn run(mut engine: Engine, jobs: &Receiver<Job>, shared: &Shared) {
    ON_ENGINE_THREAD.with(|flag| flag.set(true));
    for job in jobs {
        if shared.stopping.load(Ordering::SeqCst) || matches!(job, Job::Stop) {
            return;
        }
        // The engine catches the panics of its message handlers and of rendering itself (spec §12); this catches any other, so that no panic ends the thread and leaves the shell with an engine that silently refuses everything. A tile request whose job panicked gets `TileError::Internal`, because its answer channel is dropped unanswered.
        let messages = catch_unwind(AssertUnwindSafe(|| perform(&mut engine, job)))
            .unwrap_or_else(|_| engine.recover_from_escaped_panic());
        shared.deliver(messages);
    }
}

/// Does one job and returns the messages to deliver.
fn perform(engine: &mut Engine, job: Job) -> Vec<String> {
    match job {
        Job::Message { bytes, received_ms } => engine.handle(&bytes, received_ms),
        Job::Tile {
            request,
            width,
            height,
            received_ms,
            answer,
        } => {
            let outcome = engine.render_tile(&request, width, height, received_ms);
            // The caller may have given up waiting; then nobody needs the pixels.
            let _ignored = answer.send(outcome.pixels);
            outcome.messages
        }
        Job::Stop => Vec::new(),
        #[cfg(test)]
        Job::Panic { answer: _answer } => {
            panic!("a panic outside the engine's own recovery, for a test")
        }
    }
}

impl EngineThread {
    /// Starts an engine on a new thread named `bayan-engine`.
    ///
    /// # Errors
    ///
    /// Fails if the operating system cannot start a thread (and always in WebAssembly, which has none).
    pub fn spawn(config: Config, blobs: Arc<BlobStore>) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            sink: Mutex::new(None),
            posted: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        });
        let (sender, receiver) = mpsc::channel();
        let engine = Engine::new(config, blobs);
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name(ENGINE_THREAD_NAME.to_owned())
            .spawn(move || run(engine, &receiver, &thread_shared))?;
        let thread_id = thread.thread().id();
        Ok(Self {
            jobs: sender,
            shared,
            thread: Mutex::new(Some(thread)),
            thread_id,
        })
    }

    /// Sets where the engine's messages go. Only before the first message is posted, so no message can go to a sink set later.
    ///
    /// # Errors
    ///
    /// Fails once a message was posted.
    pub fn set_sink(&self, sink: Sink) -> Result<(), AlreadyPosted> {
        let mut slot = self
            .shared
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.shared.posted.load(Ordering::SeqCst) {
            return Err(AlreadyPosted);
        }
        *slot = Some(sink);
        Ok(())
    }

    /// Queues a message for the engine and returns at once.
    ///
    /// # Errors
    ///
    /// Fails if the engine is stopping.
    pub fn post(&self, bytes: Vec<u8>, received_ms: u64) -> Result<(), Stopped> {
        if self.shared.stopping.load(Ordering::SeqCst) {
            return Err(Stopped);
        }
        {
            // Taking the sink's lock orders this against `set_sink`.
            let _slot = self
                .shared
                .sink
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.shared.posted.store(true, Ordering::SeqCst);
        }
        self.jobs
            .send(Job::Message { bytes, received_ms })
            .map_err(|_| Stopped)
    }

    /// Renders a tile on the engine thread, after the messages posted before, and waits for the pixels (premultiplied RGBA8 without row padding).
    ///
    /// # Errors
    ///
    /// [`TileError::WrongThread`] when called on an engine thread, this one's or another's (from inside a sink), where waiting could deadlock: on its own thread it would wait for itself, and two engines' sinks could wait for each other; otherwise the engine's error, or [`TileError::Internal`] if the engine is stopping.
    pub fn render_tile(
        &self,
        request: Vec<u8>,
        width: u32,
        height: u32,
        received_ms: u64,
    ) -> Result<Vec<u8>, TileError> {
        if on_engine_thread() {
            return Err(TileError::WrongThread);
        }
        if self.shared.stopping.load(Ordering::SeqCst) {
            return Err(TileError::Internal);
        }
        {
            // Like a posted message, rendering can make the engine send messages (`engine.error` after a panic), so the sink can no longer change. Taking the sink's lock orders this against `set_sink`.
            let _slot = self
                .shared
                .sink
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.shared.posted.store(true, Ordering::SeqCst);
        }
        let (answer, pixels) = mpsc::sync_channel(1);
        // If the engine stops before it gets to the request, the queue and `answer` are dropped, and `recv` fails.
        self.jobs
            .send(Job::Tile {
                request,
                width,
                height,
                received_ms,
                answer,
            })
            .map_err(|_| TileError::Internal)?;
        pixels.recv().unwrap_or(Err(TileError::Internal))
    }

    /// Whether the calling thread is the engine thread, that is, whether the call comes from inside the sink.
    #[must_use]
    pub fn is_engine_thread(&self) -> bool {
        std::thread::current().id() == self.thread_id
    }

    /// Stops the engine. Called from any other thread, it waits until the engine thread has finished; called from inside the sink, it returns at once and the engine thread finishes by itself after the sink returns. Either way, the sink receives nothing after this returns.
    ///
    /// It needs only a shared reference, so the sink may still call [`EngineThread::post`] while this waits, for example while a host frees the engine from another thread: from the moment this is called, posting fails with [`Stopped`].
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wakes the loop if it is waiting for work; it fails only if the engine thread has already ended.
        let _ignored = self.jobs.send(Job::Stop);
        // The handle is taken out of the lock before joining, so the lock is not held while waiting.
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            if std::thread::current().id() == self.thread_id {
                // Dropping the handle detaches the thread, which is finishing the sink call that asked to stop.
                drop(thread);
            } else {
                // A panic in the engine thread outside a message is a bug that nothing here could recover from.
                let _ignored = thread.join();
            }
        }
    }
}

impl Drop for EngineThread {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

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
}
