//! The engine on its own thread, for native hosts such as the desktop shell ([engine protocol specification][spec] §8, ADR-0012 §3).
//!
//! [`EngineThread`] is an actor: one thread owns the [`Engine`] and handles queued work in order, one item at a time. Posting a message only queues it, so the caller never waits for the engine. Rendering a tile through [`EngineThread::render_tile`] queues the request behind the messages already posted and waits for the pixels. The engine's messages go to a [`Sink`] that runs on the engine thread; a sink must hand them to the shell's own thread and must never wait for the engine, because the engine waits for the sink.
//!
//! WebAssembly in a browser has no such thread: the web binding feeds the [`Engine`] directly, inside the Web Worker.
//!
//! The engine thread's stack has the fixed size [`ENGINE_THREAD_STACK_BYTES`], whatever the environment of the process says, so how deeply an input may nest before it exhausts the stack does not depend on the machine.
//!
//! **Restarts.** The engine thread is the native binding of spec §14.2: when the engine discards its session after a panic, the thread applies the restart allowance ([`crate::restart`]) at the time of the job that failed, which the host passed with it, and adds `restart` to the `engine.error` that reports it, after the engine has digested its messages for a recording. A replay runs in its own session inside the engine, so a panic there never reaches this thread and never counts. Once the allowance is used up, the thread delivers that last `engine.error`, with `restart: null`, and stops: from then on [`EngineThread::post`] fails with [`Stopped`] and [`EngineThread::render_tile`] with [`TileError::Internal`], and the jobs still queued are dropped unanswered.
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
use crate::restart::{RestartAllowance, add_restart};

/// Receives the engine's messages, as JSON, on the engine thread.
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// The name of every engine thread, as debuggers and crash reports show it.
pub const ENGINE_THREAD_NAME: &str = "bayan-engine";

/// The size of every engine thread's stack, in bytes: 2 MiB.
///
/// It is set explicitly because, without a size, Rust's standard library takes the stack size of a new thread from the `RUST_MIN_STACK` environment variable, so the environment would decide how deeply an input may nest before it overflows the stack, and the engine reads no environment variables (architecture §12, ADR-0005 §5). 2 MiB is the size that the engine's thread had by default and that Rust gives each test's thread: CORE-004 fuzzed the CRDT import on stacks of this size and measured its stack-overflow finding F5 on one (a value nested about 21,800 levels deep overflows it in a release build, 6,000 levels in a debug build). A larger stack would only raise that depth: the 1 MiB that an update may hold can nest values deeper than about 50 MiB of stack can free, so imports from untrusted peers need isolation instead (CORE-004 report §8.3).
pub const ENGINE_THREAD_STACK_BYTES: usize = 2 * 1024 * 1024;

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

/// The engine thread has stopped or is stopping, or the engine has used up its restarts (spec §14.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

/// A rendered tile: its pixels, premultiplied RGBA8 without row padding, and the render revision that it shows (spec §14.6).
#[derive(Clone, PartialEq, Eq)]
pub struct Tile {
    /// The pixels.
    pub pixels: Vec<u8>,
    /// The render revision.
    pub revision: u64,
}

/// Shows the size of the pixels, never the pixels, which show a document (AGENTS.md §6).
impl std::fmt::Debug for Tile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Tile")
            .field("pixels", &self.pixels.len())
            .field("revision", &self.revision)
            .finish()
    }
}

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
        answer: SyncSender<Result<Tile, TileError>>,
    },
    /// Wakes the engine thread so it notices that it is stopping.
    Stop,
    /// A panic outside the engine's own recovery, for tests, at the time `received_ms`; `answer` stands for a tile request waiting for the job. Host only, like the tests that send it.
    #[cfg(test)]
    #[cfg(not(target_arch = "wasm32"))]
    Panic {
        received_ms: u64,
        answer: SyncSender<Result<Tile, TileError>>,
    },
}

impl Job {
    /// The time the host passed with the job: the time of a restart if the engine discards its session while doing it (spec §14.2).
    const fn received_ms(&self) -> u64 {
        match self {
            Self::Message { received_ms, .. } | Self::Tile { received_ms, .. } => *received_ms,
            #[cfg(test)]
            #[cfg(not(target_arch = "wasm32"))]
            Self::Panic { received_ms, .. } => *received_ms,
            Self::Stop => 0,
        }
    }
}

/// What the shell's threads and the engine thread share.
struct Shared {
    sink: Mutex<Option<Sink>>,
    posted: AtomicBool,
    stopping: AtomicBool,
    /// Set when the engine has used up its restarts (spec §14.2), before the `engine.error` that says so is delivered.
    used_up: AtomicBool,
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
    let mut allowance = RestartAllowance::new();
    for job in jobs {
        if shared.stopping.load(Ordering::SeqCst) || matches!(job, Job::Stop) {
            return;
        }
        let received_ms = job.received_ms();
        // The engine catches the panics of its message handlers and of rendering itself (spec §12); this catches any other, so that no panic ends the thread and leaves the shell with an engine that silently refuses everything. A tile request whose job panicked gets `TileError::Internal`, because its answer channel is dropped unanswered.
        let mut messages = catch_unwind(AssertUnwindSafe(|| perform(&mut engine, job)))
            .unwrap_or_else(|_| engine.recover_from_escaped_panic());
        // The engine has digested its messages for a recording; the restart that follows is added now, outside the digest (spec §14.2). A panic outside a message counts at the time of the job that was running.
        let mut used_up = false;
        if engine.take_session_discarded() {
            let restart = allowance.failure(received_ms);
            add_restart(&mut messages, restart);
            used_up = restart.is_none();
            if used_up {
                // Before the last engine.error is delivered, so that a post from inside the callback already fails.
                shared.used_up.store(true, Ordering::SeqCst);
            }
        }
        shared.deliver(messages);
        if used_up {
            // The engine has stopped and answers nothing more; the jobs still queued are dropped with the queue.
            return;
        }
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
            let revision = outcome.revision;
            // The caller may have given up waiting; then nobody needs the pixels.
            let _ignored = answer.send(outcome.pixels.map(|pixels| Tile { pixels, revision }));
            outcome.messages
        }
        Job::Stop => Vec::new(),
        #[cfg(test)]
        #[cfg(not(target_arch = "wasm32"))]
        Job::Panic {
            answer: _answer, ..
        } => {
            panic!("a panic outside the engine's own recovery, for a test")
        }
    }
}

impl EngineThread {
    /// Starts an engine on a new thread named `bayan-engine`, with a stack of [`ENGINE_THREAD_STACK_BYTES`].
    ///
    /// # Errors
    ///
    /// Fails if the operating system cannot start a thread (and always in WebAssembly, which has none).
    pub fn spawn(config: Config, blobs: Arc<BlobStore>) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            sink: Mutex::new(None),
            posted: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            used_up: AtomicBool::new(false),
        });
        let (sender, receiver) = mpsc::channel();
        let engine = Engine::new(config, blobs);
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name(ENGINE_THREAD_NAME.to_owned())
            .stack_size(ENGINE_THREAD_STACK_BYTES)
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

    /// Queues a message for the engine and returns at once. `received_ms` is the time on the host's monotonic clock, which recordings and the restart allowance use (spec §10, §14.2).
    ///
    /// # Errors
    ///
    /// Fails if the engine is stopping, or has used up its restarts (spec §14.2).
    pub fn post(&self, bytes: Vec<u8>, received_ms: u64) -> Result<(), Stopped> {
        if self.has_stopped() {
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

    /// Renders a tile on the engine thread, after the messages posted before, and waits for its pixels and the render revision it shows.
    ///
    /// # Errors
    ///
    /// [`TileError::WrongThread`] when called on an engine thread, this one's or another's (from inside a sink), where waiting could deadlock: on its own thread it would wait for itself, and two engines' sinks could wait for each other; otherwise the engine's error, or [`TileError::Internal`] if the engine is stopping or has used up its restarts (spec §14.2).
    pub fn render_tile(
        &self,
        request: Vec<u8>,
        width: u32,
        height: u32,
        received_ms: u64,
    ) -> Result<Tile, TileError> {
        if on_engine_thread() {
            return Err(TileError::WrongThread);
        }
        if self.has_stopped() {
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

    /// Whether the engine is stopping, or has used up its restarts and answers nothing more (spec §14.2).
    #[must_use]
    pub fn has_stopped(&self) -> bool {
        self.shared.stopping.load(Ordering::SeqCst) || self.shared.used_up.load(Ordering::SeqCst)
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

// Host only: these tests start the engine thread, and WebAssembly (wasm32-wasip1, where the gate also runs the tests) has no threads. The condition is a second attribute, not part of `cfg(all(test, …))`, because Clippy treats a module as test code, where `unwrap()` is allowed, only under a plain `#[cfg(test)]`.
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;
