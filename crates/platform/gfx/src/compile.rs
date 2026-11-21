//! Render pipelines built off the calling thread.
//!
//! On Metal almost all of a first-time pipeline is the system shader
//! compiler (`newLibraryWithSource` and `newRenderPipelineState`): about
//! 70 ms for a typical NV2A program on an idle machine and 220 ms under
//! load, against about 5 ms of WGSL translation, naga validation and MSL
//! generation (docs/graphics/graphics.md, "Pipeline compiles"). wgpu's pipeline
//! cache is a no-op on Metal, so the way to keep that off the game thread
//! is to start the compile early on a worker and only collect it when the
//! draw is encoded.
//!
//! A [`PendingPipeline`] is a pipeline that is being (or will be) built.
//! [`Compiler::submit`] queues it for the worker threads; whoever needs it
//! first gets it with [`PendingPipeline::get`]: from the worker when it is
//! done, by waiting when the worker is on it, or by building it on the
//! calling thread when no worker has started it yet. It is built exactly
//! once either way. Draw-time misses jump the queue ahead of prewarming.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};

type Build<T> = Box<dyn FnOnce() -> T + Send>;

/// A value built at most once, on a worker or by its first user.
pub struct Pending<T> {
    value: OnceLock<T>,
    build: Mutex<Option<Build<T>>>,
    built_on: OnceLock<std::thread::ThreadId>,
    completed_frame: OnceLock<u64>,
}

impl<T> Pending<T> {
    /// A value `build` makes when first needed.
    pub fn new(build: impl FnOnce() -> T + Send + 'static) -> Arc<Self> {
        Arc::new(Self {
            value: OnceLock::new(),
            build: Mutex::new(Some(Box::new(build))),
            built_on: OnceLock::new(),
            completed_frame: OnceLock::new(),
        })
    }

    /// An already built value.
    pub fn ready(value: T) -> Arc<Self> {
        let p = Self {
            value: OnceLock::new(),
            build: Mutex::new(None),
            built_on: OnceLock::new(),
            completed_frame: OnceLock::new(),
        };
        let _ = p.value.set(value);
        Arc::new(p)
    }

    /// The value if it is built, without waiting.
    pub fn try_get(&self) -> Option<&T> {
        self.value.get()
    }

    /// A worker result visible at the start of this guest frame. A native
    /// pass can retain its D3D draws until the swap after compilation.
    pub fn previous_frame(&self) -> Option<&T> {
        self.visible_at_frame(timeline::frames())
    }

    /// Whether a pass extracted in this guest frame can use the value.
    /// A deferred render worker can be several swaps behind extraction.
    pub fn visible_at_frame(&self, frame: u64) -> Option<&T> {
        self.value.get().filter(|_| {
            self.completed_frame.get().is_none_or(|&completed| completed < frame)
        })
    }

    /// Whether the value is built.
    pub fn is_ready(&self) -> bool {
        self.value.get().is_some()
    }

    /// The value: built here if nobody has started it, else waited for.
    pub fn get(&self) -> &T {
        self.value.get_or_init(|| {
            let build = self
                .build
                .lock()
                .unwrap()
                .take()
                .expect("a pending value is built once");
            let _ = self.built_on.set(std::thread::current().id());
            let value = build();
            let _ = self.completed_frame.set(timeline::frames());
            value
        })
    }

    /// The thread that built the value (`None` before, and for
    /// [`ready`](Self::ready) values).
    pub fn built_on(&self) -> Option<std::thread::ThreadId> {
        self.built_on.get().copied()
    }
}

/// A render pipeline that may still be compiling. `None` once built means
/// it failed validation (the error was logged): draws using it are skipped
/// instead of reaching `set_pipeline` with an invalid pipeline, which wgpu
/// turns into a panic.
pub type PendingPipeline = Pending<Option<wgpu::RenderPipeline>>;

/// Queue priority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// A draw is waiting for it.
    Draw,
    /// Prewarming: nothing needs it yet.
    Background,
}

type Job = Box<dyn FnOnce() + Send>;

/// Worker thread names start with this.
pub const WORKER_NAME: &str = "gfx compile";

#[derive(Default)]
struct Queues {
    draw: VecDeque<Job>,
    background: VecDeque<Job>,
}

/// The worker threads that build pipelines.
pub struct Compiler {
    queues: Mutex<Queues>,
    ready: Condvar,
}

impl Compiler {
    /// The process-wide compiler: `H2_GFX_COMPILE_THREADS` workers (default
    /// half the cores, 2 to 4; 0 builds every pipeline on its first user).
    pub fn global() -> &'static Arc<Compiler> {
        static GLOBAL: LazyLock<Arc<Compiler>> = LazyLock::new(|| {
            let threads = std::env::var("H2_GFX_COMPILE_THREADS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4))
                });
            Compiler::new(threads)
        });
        &GLOBAL
    }

    /// A compiler with `threads` workers (0: nothing runs in the background).
    pub fn new(threads: usize) -> Arc<Self> {
        Self::with_worker_setup(threads, WORKER_NAME, || {})
    }

    /// A separate pool for callers with a different scheduling policy.
    pub fn with_worker_setup(threads: usize, name: &str, setup: fn()) -> Arc<Self> {
        let c = Arc::new(Self {
            queues: Mutex::default(),
            ready: Condvar::new(),
        });
        for n in 0..threads {
            let c = c.clone();
            std::thread::Builder::new()
                .name(format!("{name} {n}"))
                .spawn(move || {
                    setup();
                    c.work();
                })
                .expect("spawn a pipeline compile thread");
        }
        c
    }

    fn work(&self) {
        loop {
            let job = {
                let mut q = self.queues.lock().unwrap();
                loop {
                    if let Some(j) = q.draw.pop_front().or_else(|| q.background.pop_front()) {
                        break j;
                    }
                    q = self.ready.wait(q).unwrap();
                }
            };
            job();
        }
    }

    /// Queues `pending` to be built on a worker.
    pub fn submit<T: Send + Sync + 'static>(&self, pending: &Arc<Pending<T>>, priority: Priority) {
        let p = pending.clone();
        let job: Job = Box::new(move || {
            // No timeline span: the game thread's frame budget would count
            // it. A worker's compile shows only as the draw's `pipeline wait`.
            p.get();
        });
        let mut q = self.queues.lock().unwrap();
        match priority {
            Priority::Draw => q.draw.push_back(job),
            Priority::Background => q.background.push_back(job),
        }
        drop(q);
        self.ready.notify_one();
    }

    /// Jobs not started yet.
    pub fn queued(&self) -> usize {
        let q = self.queues.lock().unwrap();
        q.draw.len() + q.background.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn built_once_by_worker_or_user() {
        let c = Compiler::new(2);
        let runs = Arc::new(AtomicUsize::new(0));
        let all: Vec<_> = (0..64)
            .map(|i| {
                let runs = runs.clone();
                let p = Pending::new(move || {
                    runs.fetch_add(1, Ordering::SeqCst);
                    i * 2
                });
                c.submit(
                    &p,
                    if i % 2 == 0 {
                        Priority::Draw
                    } else {
                        Priority::Background
                    },
                );
                p
            })
            .collect();
        // Racing the workers: every value is right and built exactly once.
        for (i, p) in all.iter().enumerate() {
            assert_eq!(*p.get(), i * 2);
        }
        assert_eq!(runs.load(Ordering::SeqCst), 64);
        assert!(all.iter().all(|p| p.built_on().is_some()));
    }

    #[test]
    fn no_workers_builds_on_first_use() {
        let c = Compiler::new(0);
        let p = Pending::new(|| 7);
        c.submit(&p, Priority::Background);
        assert!(!p.is_ready());
        assert_eq!(*p.get(), 7);
        assert_eq!(p.built_on(), Some(std::thread::current().id()));
        assert_eq!(c.queued(), 1);
        assert_eq!(*Pending::ready(3).try_get().unwrap(), 3);
    }

    #[test]
    fn a_completed_job_becomes_visible_at_the_following_guest_swap() {
        let pending = Pending::new(|| 9);
        assert!(pending.previous_frame().is_none());
        assert_eq!(pending.get(), &9);
        let completed = *pending.completed_frame.get().unwrap();
        assert!(pending.visible_at_frame(completed).is_none());
        assert_eq!(pending.visible_at_frame(completed + 1), Some(&9));
        assert_eq!(Pending::ready(3).visible_at_frame(0), Some(&3));
    }

    #[test]
    fn draw_jobs_run_before_background() {
        let c = Compiler::new(0);
        let order = Arc::new(Mutex::new(Vec::new()));
        let mk = |n: u32| {
            let order = order.clone();
            Pending::new(move || order.lock().unwrap().push(n))
        };
        let (a, b, d) = (mk(1), mk(2), mk(3));
        c.submit(&a, Priority::Background);
        c.submit(&b, Priority::Background);
        c.submit(&d, Priority::Draw);
        // Drain by hand, as a worker would.
        for _ in 0..3 {
            let job = {
                let mut q = c.queues.lock().unwrap();
                q.draw
                    .pop_front()
                    .or_else(|| q.background.pop_front())
                    .unwrap()
            };
            job();
        }
        assert_eq!(*order.lock().unwrap(), vec![3, 1, 2]);
    }
}
