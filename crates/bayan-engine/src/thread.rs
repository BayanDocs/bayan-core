//! The engine on its own thread, for native hosts such as the desktop shell ([engine protocol specification][spec] §8, ADR-0012 §3).
//!
//! [`EngineThread`] is an actor: one thread owns the [`Engine`] and handles queued work in order, one item at a time. Posting a message only queues it, so the caller never waits for the engine. Rendering a tile through [`EngineThread::render_tile`] queues the request behind the messages already posted and waits for the pixels. The engine's messages go to a [`Sink`] that runs on the engine thread; a sink must hand them to the shell's own thread and must never wait for the engine, because the engine waits for the sink.
//!
//! WebAssembly in a browser has no such thread: the web binding feeds the [`Engine`] directly, inside the Web Worker.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::cell::Cell;
use std::io;
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
    jobs: Option<Sender<Job>>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
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
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        match job {
            Job::Message { bytes, received_ms } => {
                let messages = engine.handle(&bytes, received_ms);
                shared.deliver(messages);
            }
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
                shared.deliver(outcome.messages);
            }
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
            jobs: Some(sender),
            shared,
            thread: Some(thread),
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
        let jobs = self.jobs.as_ref().ok_or(Stopped)?;
        jobs.send(Job::Message { bytes, received_ms })
            .map_err(|_| Stopped)
    }

    /// Renders a tile on the engine thread, after the messages posted before, and waits for the pixels (premultiplied RGBA8 without row padding).
    ///
    /// # Errors
    ///
    /// [`TileError::WrongThread`] when called on the engine thread (from inside the sink), where it would wait for itself; otherwise the engine's error, or [`TileError::Internal`] if the engine is stopping.
    pub fn render_tile(
        &self,
        request: Vec<u8>,
        width: u32,
        height: u32,
        received_ms: u64,
    ) -> Result<Vec<u8>, TileError> {
        if self.is_engine_thread() {
            return Err(TileError::WrongThread);
        }
        if self.shared.stopping.load(Ordering::SeqCst) {
            return Err(TileError::Internal);
        }
        let jobs = self.jobs.as_ref().ok_or(TileError::Internal)?;
        let (answer, pixels) = mpsc::sync_channel(1);
        jobs.send(Job::Tile {
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
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Closing the queue ends the engine thread's loop.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
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
        self.shutdown();
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
    fn the_sink_cannot_change_after_the_first_post() {
        let (thread, _messages) = started();
        thread.post(b"{}".to_vec(), 0).unwrap();
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
        // Drain whatever arrived before stop returned; then the channel's sender is gone with the engine.
        while messages.try_recv().is_ok() {}
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
