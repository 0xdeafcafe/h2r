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
//! once either way. Draw-time misses jump the queue ahead of prewarming,
//! and a draw that needs a value still queued as a prewarm moves it there
//! too ([`Compiler::submit`] at [`Priority::Draw`]).
//!
//! A prewarm belongs to an owner ([`Compiler::prewarm`]): when the work it
//! guessed at stops being likely (the game left the map it was for), the
//! owner drops what is still queued ([`Compiler::drop_prewarms`]). A
//! dropped value is not lost: its first user builds it, or it is queued
//! again.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};

type Build<T> = Box<dyn FnOnce() -> T + Send>;

/// A value built at most once, on a worker or by its first user.
pub struct Pending<T> {
    value: OnceLock<T>,
    build: Mutex<Option<Build<T>>>,
    built_on: OnceLock<std::thread::ThreadId>,
    completed_frame: OnceLock<u64>,
    /// The highest queue a job for it waits in ([`NOT_QUEUED`],
    /// [`QUEUED_BACKGROUND`], [`QUEUED_DRAW`]).
    queued: AtomicU8,
}

const NOT_QUEUED: u8 = 0;
const QUEUED_BACKGROUND: u8 = 1;
const QUEUED_DRAW: u8 = 2;

impl<T> Pending<T> {
    /// A value `build` makes when first needed.
    pub fn new(build: impl FnOnce() -> T + Send + 'static) -> Arc<Self> {
        Arc::new(Self {
            value: OnceLock::new(),
            build: Mutex::new(Some(Box::new(build))),
            built_on: OnceLock::new(),
            completed_frame: OnceLock::new(),
            queued: AtomicU8::new(NOT_QUEUED),
        })
    }

    /// An already built value.
    pub fn ready(value: T) -> Arc<Self> {
        let p = Self {
            value: OnceLock::new(),
            build: Mutex::new(None),
            built_on: OnceLock::new(),
            completed_frame: OnceLock::new(),
            queued: AtomicU8::new(NOT_QUEUED),
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

    /// Whether the value is built or being built: queueing it again does
    /// nothing.
    pub fn is_started(&self) -> bool {
        self.value.get().is_some() || self.built_on.get().is_some()
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

/// A queued build, type-erased.
trait Job: Send + Sync {
    /// Builds the value unless someone has started it.
    fn run(&self);
    /// Taken out of the background queue unrun: a later submit queues it
    /// again.
    fn dropped(&self);
}

impl<T: Send + Sync> Job for Pending<T> {
    fn run(&self) {
        if !self.is_started() {
            self.get();
        }
    }

    fn dropped(&self) {
        let _ = self.queued.compare_exchange(
            QUEUED_BACKGROUND,
            NOT_QUEUED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Worker thread names start with this.
pub const WORKER_NAME: &str = "gfx compile";

#[derive(Default)]
struct Queues {
    draw: VecDeque<Arc<dyn Job>>,
    /// Prewarms, with the owner each was queued for.
    background: VecDeque<(u64, Arc<dyn Job>)>,
}

impl Queues {
    fn next(&mut self) -> Option<Arc<dyn Job>> {
        self.draw
            .pop_front()
            .or_else(|| self.background.pop_front().map(|(_, j)| j))
    }
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
                    if let Some(j) = q.next() {
                        break j;
                    }
                    q = self.ready.wait(q).unwrap();
                }
            };
            // No timeline span: the game thread's frame budget would count
            // it. A worker's compile shows only as the draw's `pipeline wait`.
            job.run();
        }
    }

    /// Queues `pending` to be built on a worker. Returns `false`, queueing
    /// nothing, when it is built or being built, or already waits at
    /// `priority` or above: a draw's [`Priority::Draw`] moves a queued
    /// prewarm ahead, and the same draw every frame queues it once.
    /// Background work queued here has no owner: use
    /// [`prewarm`](Self::prewarm) for work that may be dropped.
    pub fn submit<T: Send + Sync + 'static>(
        &self,
        pending: &Arc<Pending<T>>,
        priority: Priority,
    ) -> bool {
        self.queue(pending, priority, 0)
    }

    /// Queues `pending` as a prewarm for `owner` (a nonzero id the caller
    /// picks, such as [`Compiler::owner`]), which
    /// [`drop_prewarms`](Self::drop_prewarms) can take back. Returns what
    /// [`submit`](Self::submit) does.
    pub fn prewarm<T: Send + Sync + 'static>(&self, pending: &Arc<Pending<T>>, owner: u64) -> bool {
        self.queue(pending, Priority::Background, owner)
    }

    fn queue<T: Send + Sync + 'static>(
        &self,
        pending: &Arc<Pending<T>>,
        priority: Priority,
        owner: u64,
    ) -> bool {
        if pending.is_started() {
            return false;
        }
        let level = match priority {
            Priority::Draw => QUEUED_DRAW,
            Priority::Background => QUEUED_BACKGROUND,
        };
        if pending.queued.fetch_max(level, Ordering::AcqRel) >= level {
            return false;
        }
        let job: Arc<dyn Job> = pending.clone();
        let mut q = self.queues.lock().unwrap();
        match priority {
            Priority::Draw => q.draw.push_back(job),
            Priority::Background => q.background.push_back((owner, job)),
        }
        drop(q);
        self.ready.notify_one();
        true
    }

    /// Takes `owner`'s prewarms that no worker has started out of the
    /// queue, and returns how many. Each stays unbuilt until its first
    /// user builds it or it is queued again; one a draw moved ahead stays
    /// queued for that draw.
    pub fn drop_prewarms(&self, owner: u64) -> usize {
        let mut q = self.queues.lock().unwrap();
        let mut dropped = Vec::new();
        q.background.retain(|(o, job)| {
            if *o == owner {
                dropped.push(job.clone());
                false
            } else {
                true
            }
        });
        drop(q);
        for job in &dropped {
            job.dropped();
        }
        dropped.len()
    }

    /// A fresh prewarm owner id, unique in the process (never 0).
    pub fn owner() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
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
            let job = c.queues.lock().unwrap().next().unwrap();
            job.run();
        }
        assert_eq!(*order.lock().unwrap(), vec![3, 1, 2]);
    }

    /// Drains the queues by hand, as a worker would.
    fn drain(c: &Compiler) {
        while let Some(job) = c.queues.lock().unwrap().next() {
            job.run();
        }
    }

    #[test]
    fn a_draw_moves_a_queued_prewarm_ahead_once() {
        let c = Compiler::new(0);
        let order = Arc::new(Mutex::new(Vec::new()));
        let mk = |n: u32| {
            let order = order.clone();
            Pending::new(move || order.lock().unwrap().push(n))
        };
        let (a, b) = (mk(1), mk(2));
        assert!(c.prewarm(&a, 7));
        assert!(c.prewarm(&b, 7));
        assert!(!c.prewarm(&b, 7), "already queued");
        // The draw that needs `b` every frame queues it ahead once.
        assert!(c.submit(&b, Priority::Draw));
        assert!(!c.submit(&b, Priority::Draw));
        assert!(!c.prewarm(&b, 7), "a prewarm never lowers it");
        assert_eq!(c.queued(), 3);
        drain(&c);
        assert_eq!(
            *order.lock().unwrap(),
            vec![2, 1],
            "each built once, b first"
        );
        assert!(!c.submit(&a, Priority::Draw), "built: nothing to queue");
    }

    #[test]
    fn dropped_prewarms_are_only_the_owner_s_and_can_be_queued_again() {
        let c = Compiler::new(0);
        let runs = Arc::new(Mutex::new(Vec::new()));
        let mk = |n: u32| {
            let runs = runs.clone();
            Pending::new(move || runs.lock().unwrap().push(n))
        };
        let (old, other, promoted, draw) = (mk(1), mk(2), mk(3), mk(4));
        let (me, them) = (Compiler::owner(), Compiler::owner());
        assert_ne!(me, them);
        c.prewarm(&old, me);
        c.prewarm(&other, them);
        c.prewarm(&promoted, me);
        c.submit(&promoted, Priority::Draw);
        c.submit(&draw, Priority::Draw);
        // A map change: my prewarms go, another owner's and draws' stay.
        assert_eq!(c.drop_prewarms(me), 2);
        assert_eq!(c.drop_prewarms(me), 0);
        assert_eq!(c.queued(), 3);
        drain(&c);
        let mut built = runs.lock().unwrap().clone();
        built.sort_unstable();
        assert_eq!(built, vec![2, 3, 4]);
        assert!(!old.is_started(), "dropped, not built");
        // Queued again later (or built by its first user), it builds.
        assert!(c.prewarm(&old, me));
        drain(&c);
        assert!(old.is_ready());
    }
}
