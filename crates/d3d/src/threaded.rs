//! A render thread: a [`Backend`] that runs another backend's work on a
//! host thread of its own, the way the NV2A consumes the push buffer while
//! the CPU goes on (docs/render-thread.md).
//!
//! The device still decodes the push buffer, translates state and resolves
//! every draw on the guest's thread, reading guest memory there. What
//! crosses to the render thread is the backend call: a [`ResolvedDraw`]
//! holds no guest pointers, so shader preparation, encoding, uploads and
//! submission can run later without the guest's memory.
//!
//! Every call whose answer the guest can observe waits for the render
//! thread to reach it, so the guest sees exactly the values a synchronous
//! backend gives:
//!
//! - **Reads** (`read_surface`, `read_depth`, `read_pixel_count`) run on
//!   the render thread after everything queued before them, and the guest
//!   waits for the bytes. These were GPU round trips already.
//! - **Handles** (`create_surface`, `upload_texture`, `snapshot_surface`,
//!   `surface_texture`, `report_pixel_count`) are proxies made on the
//!   guest's side at once; the render thread maps them to the inner
//!   backend's.
//! - **Draw outcomes** come from the inner backend's [`DrawJudge`]
//!   ([`Backend::judge`]), which decides on the guest's side, from the
//!   draw and whether each bound texture has a view, what the inner
//!   backend's `draw` will answer: an untranslatable draw is skipped with
//!   the same reason, as with a synchronous backend. Without a judge they
//!   are speculated as drawn.
//! - **Other outcomes** (`clear_rects`, `write_surface`, `copy_surface`,
//!   `snapshot_surface`, `surface_texture`, `depth_alias_texture`) are
//!   answered as the inner backend answers every valid call: drawn,
//!   `true`, `Some`.
//! - When the inner backend later gives another answer than the one
//!   returned, that is latched as an error, so the next call stops the
//!   guest instead of running on with device state that differs from a
//!   synchronous run. [`ThreadedBackend::misses`] counts them.
//! - **Errors** from the inner backend are latched the same way and
//!   returned by the next call (a failure is a stop either way; the stop
//!   comes at a later entry than with a synchronous backend).
//!
//! The render thread never touches guest memory or the logical clock, and
//! the guest never waits for it except at the reads above and the
//! frame-in-flight limit, which wait on wall time only. So logical time,
//! and everything the guest computes, is the same as with the synchronous
//! backend; only wall-clock overlap changes.
//!
//! Commands run in the order they were issued, on one queue, so the GPU
//! sees exactly the submissions the synchronous backend makes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{Backend, DrawJudge, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};

/// Presents the guest may run ahead of the render thread before `Swap`
/// waits (wall time only). Two, like D3D's default swap throttling.
pub const MAX_FRAMES_IN_FLIGHT: u64 = 2;

type Job<B> = Box<dyn FnOnce(&mut Worker<B>) + Send>;

/// What lives on the render thread.
pub struct Worker<B: Backend> {
    inner: B,
    surfaces: HashMap<u32, B::Surface>,
    textures: HashMap<u64, B::Texture>,
    tickets: HashMap<u64, u64>,
    /// The depth buffer read back for depth-alias textures: surface,
    /// device depth generation, guest words.
    depth_words: Option<(u32, u64, Vec<u8>)>,
    shared: Arc<Shared>,
    exit: bool,
    /// [`ThreadedBackend::with_jitter`]: a xorshift state.
    jitter: Option<u64>,
}

#[derive(Default)]
struct Shared {
    /// The first error or wrong speculation, returned by the next call.
    error: Mutex<Option<String>>,
    misses: AtomicU64,
    /// Presents the render thread has finished.
    presented: Mutex<u64>,
    presented_cv: Condvar,
    /// Wall time the render thread spent in the inner backend (ns).
    busy_ns: AtomicU64,
    /// Pixel counts already back from the GPU, by proxy ticket: the render
    /// thread asks the inner backend after each present
    /// ([`Backend::try_pixel_count`]), so a read of an older report answers
    /// without waiting for the queue.
    landed: Mutex<HashMap<u64, u64>>,
}

static READS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);
static THROTTLES: AtomicU64 = AtomicU64::new(0);
static THROTTLE_NS: AtomicU64 = AtomicU64::new(0);
static BUSY_NS: AtomicU64 = AtomicU64::new(0);
static LANDED_COUNTS: AtomicU64 = AtomicU64::new(0);

/// [`Waits`] summed over every render thread in the process so far (for
/// a measurement window: take it twice and subtract).
pub fn total_waits() -> Waits {
    let d = |a: &AtomicU64| std::time::Duration::from_nanos(a.load(Ordering::Relaxed));
    Waits {
        reads: READS.load(Ordering::Relaxed),
        read_wait: d(&READ_NS),
        throttles: THROTTLES.load(Ordering::Relaxed),
        throttle_wait: d(&THROTTLE_NS),
        busy: d(&BUSY_NS),
        landed_counts: LANDED_COUNTS.load(Ordering::Relaxed),
    }
}

/// Where the guest's thread waited for the render thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Waits {
    /// Calls that waited for an answer (reads, `finish`, `with_inner`).
    pub reads: u64,
    /// Wall time those waited.
    pub read_wait: std::time::Duration,
    /// `Swap`s that waited for the frame-in-flight limit.
    pub throttles: u64,
    /// Wall time those waited.
    pub throttle_wait: std::time::Duration,
    /// Wall time the render thread spent running backend work.
    pub busy: std::time::Duration,
    /// Pixel-count reads answered from a report already back, without a
    /// wait (every thread's, in [`total_waits`]).
    pub landed_counts: u64,
}

impl Shared {
    fn latch(&self, e: String) {
        let mut g = self.error.lock().unwrap();
        if g.is_none() {
            *g = Some(e);
        }
    }
    fn miss(&self, e: String) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.latch(format!("render thread: wrong speculation: {e}"));
    }
}

impl<B: Backend> Worker<B> {
    fn surface(&self, id: u32) -> Result<B::Surface, String> {
        self.surfaces
            .get(&id)
            .copied()
            .ok_or_else(|| format!("render thread: surface {id} is not live"))
    }
    fn depth(&self, id: Option<u32>) -> Result<Option<B::Surface>, String> {
        id.map(|d| self.surface(d)).transpose()
    }
    /// Moves the pixel counts the GPU has written into
    /// [`Shared::landed`]. Same values as a read: only the wait goes.
    fn land_pixel_counts(&mut self) -> Result<(), String> {
        let mut landed = Vec::new();
        for (&proxy, &t) in &self.tickets {
            if let Some(c) = self.inner.try_pixel_count(t)? {
                landed.push((proxy, c));
            }
        }
        if landed.is_empty() {
            return Ok(());
        }
        let mut l = self.shared.landed.lock().unwrap();
        for (proxy, c) in landed {
            self.tickets.remove(&proxy);
            l.insert(proxy, c);
        }
        Ok(())
    }
    /// Runs `f`, latching an error.
    fn run(&mut self, f: impl FnOnce(&mut Self) -> Result<(), String>) {
        if let Err(e) = f(self) {
            self.shared.latch(e);
        }
    }
}

/// A texture handle: the render thread's texture of the same id. Dropping
/// it frees that texture after everything queued before the drop.
pub struct ProxyTexture<B: Backend + 'static> {
    id: u64,
    /// Whether the inner backend's texture has a view the shaders sample
    /// (the judge's answer; true without one).
    view: bool,
    tx: mpsc::Sender<Job<B>>,
}

impl<B: Backend + 'static> Drop for ProxyTexture<B> {
    fn drop(&mut self) {
        let id = self.id;
        // After the render thread exits there is nothing left to free.
        let _ = self.tx.send(Box::new(move |w: &mut Worker<B>| {
            w.textures.remove(&id);
        }));
    }
}

impl<B: Backend + 'static> std::fmt::Debug for ProxyTexture<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProxyTexture({})", self.id)
    }
}

/// [`Backend`] that queues `B`'s work to a render thread.
pub struct ThreadedBackend<B: Backend + 'static> {
    tx: mpsc::Sender<Job<B>>,
    join: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    bc: bool,
    sample_surfaces: bool,
    sample_depth: bool,
    next_surface: u32,
    next_texture: u64,
    next_ticket: u64,
    presents: u64,
    waits: Waits,
    /// The inner backend's draw outcomes, answered here (`None`: they are
    /// speculated as drawn).
    judge: Option<Box<dyn DrawJudge>>,
    /// Each live surface's format and kind (for the judge's views).
    surface_formats: HashMap<u32, (D3DFormat, bool)>,
    /// Depth-alias decodes seen: description, signs, whether it has a view.
    alias_views: Vec<(TextureDesc, [bool; 4], bool)>,
}

impl<B: Backend + 'static> ThreadedBackend<B> {
    /// Moves `inner` to a new thread named `d3d-render`.
    pub fn new(inner: B) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job<B>>();
        let shared = Arc::new(Shared::default());
        let bc = inner.bc_supported();
        let judge = inner.judge();
        let sample_surfaces = inner.can_sample_surfaces();
        let sample_depth = inner.can_sample_depth_surfaces();
        let s = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("d3d-render".into())
            .spawn(move || {
                let mut w = Worker {
                    inner,
                    surfaces: HashMap::new(),
                    textures: HashMap::new(),
                    tickets: HashMap::new(),
                    depth_words: None,
                    shared: s,
                    exit: false,
                    jitter: None,
                };
                while let Ok(job) = rx.recv() {
                    if let Some(x) = w.jitter.as_mut() {
                        *x ^= *x << 13;
                        *x ^= *x >> 7;
                        *x ^= *x << 17;
                        std::thread::sleep(std::time::Duration::from_micros(*x % 300));
                    }
                    let t = std::time::Instant::now();
                    job(&mut w);
                    let ns = t.elapsed().as_nanos() as u64;
                    w.shared.busy_ns.fetch_add(ns, Ordering::Relaxed);
                    BUSY_NS.fetch_add(ns, Ordering::Relaxed);
                    if w.exit {
                        break;
                    }
                }
                // Wake a guest waiting on a frame that will never finish.
                *w.shared.presented.lock().unwrap() = u64::MAX;
                w.shared.presented_cv.notify_all();
            })?;
        Ok(Self {
            tx,
            join: Some(join),
            shared,
            bc,
            sample_surfaces,
            sample_depth,
            next_surface: 1,
            next_texture: 1,
            next_ticket: 1,
            presents: 0,
            waits: Waits::default(),
            judge,
            surface_formats: HashMap::new(),
            alias_views: Vec::new(),
        })
    }

    /// Wrong speculations so far (each also stops the guest).
    pub fn misses(&self) -> u64 {
        self.shared.misses.load(Ordering::Relaxed)
    }

    /// Where the guest waited so far, and the render thread's busy time.
    pub fn waits(&self) -> Waits {
        Waits {
            busy: std::time::Duration::from_nanos(self.shared.busy_ns.load(Ordering::Relaxed)),
            ..self.waits
        }
    }

    /// Delays the render thread by a pseudo-random 0-300 us before every
    /// command from here on (seeded), so tests can show that its timing
    /// changes nothing the guest sees.
    pub fn with_jitter(self, seed: u64) -> Self {
        let seed = seed | 1;
        self.post(move |w| {
            w.jitter = Some(seed);
            Ok(())
        });
        self
    }

    /// Waits until the render thread has run everything queued so far.
    pub fn finish(&mut self) -> Result<(), String> {
        self.call(|_| Ok(()))
    }

    /// Runs `f` on the inner backend after everything queued, and waits for
    /// its answer (tests and reports: `GfxBackend::batch_stats`, say).
    pub fn with_inner<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut B) -> R + Send + 'static,
    ) -> Result<R, String> {
        self.call(move |w| Ok(f(&mut w.inner)))
    }

    /// [`with_inner`](Self::with_inner) with the inner backend's handle of
    /// `surface` (`GfxBackend::read_color` of the back buffer, say).
    pub fn with_surface<R: Send + 'static>(
        &mut self,
        surface: u32,
        f: impl FnOnce(&mut B, B::Surface) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        self.call(move |w| {
            let s = w.surface(surface)?;
            f(&mut w.inner, s)
        })
    }

    fn check(&self) -> Result<(), String> {
        match &*self.shared.error.lock().unwrap() {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    fn post(&self, f: impl FnOnce(&mut Worker<B>) -> Result<(), String> + Send + 'static) {
        if self.tx.send(Box::new(move |w: &mut Worker<B>| w.run(f))).is_err() {
            self.shared.latch("render thread: gone".into());
        }
    }

    /// Runs `f` after everything queued, and waits for its answer.
    fn call<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut Worker<B>) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        self.check()?;
        let t = std::time::Instant::now();
        let (rtx, rrx) = mpsc::sync_channel(1);
        self.tx
            .send(Box::new(move |w: &mut Worker<B>| {
                let _ = rtx.send(f(w));
            }))
            .map_err(|_| "render thread: gone".to_string())?;
        let r = rrx
            .recv()
            .map_err(|_| "render thread: gone".to_string())?;
        self.waits.reads += 1;
        self.waits.read_wait += t.elapsed();
        READS.fetch_add(1, Ordering::Relaxed);
        READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        // An error queued before this call comes first.
        self.check()?;
        r
    }

    fn new_surface(&mut self) -> u32 {
        let id = self.next_surface;
        self.next_surface += 1;
        id
    }

    fn new_texture(&mut self, view: bool) -> ProxyTexture<B> {
        let id = self.next_texture;
        self.next_texture += 1;
        ProxyTexture {
            id,
            view,
            tx: self.tx.clone(),
        }
    }

    /// The judge's view answer for a texture over surface `surface`.
    fn surface_view(&self, surface: u32) -> bool {
        match (&self.judge, self.surface_formats.get(&surface)) {
            (Some(j), Some(&(format, false))) => j.surface_view(format),
            (Some(_), _) => false,
            (None, _) => true,
        }
    }
}

impl<B: Backend + 'static> Drop for ThreadedBackend<B> {
    fn drop(&mut self) {
        if std::env::var_os("H2_RENDER_THREAD_STATS").is_some() {
            eprintln!("render thread: {:?}", self.waits());
        }
        let _ = self
            .tx
            .send(Box::new(|w: &mut Worker<B>| w.exit = true));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn expect_drawn(what: &str, r: DrawOutcome) -> Result<(), String> {
    match r {
        DrawOutcome::Drawn => Ok(()),
        other => Err(format!("{what}: {other:?}")),
    }
}

impl<B: Backend + 'static> Backend for ThreadedBackend<B> {
    type Surface = u32;
    type Texture = ProxyTexture<B>;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<u32, String> {
        self.check()?;
        let id = self.new_surface();
        self.surface_formats.insert(id, (format, depth));
        self.post(move |w| {
            let s = w.inner.create_surface(format, width, height, depth)?;
            w.surfaces.insert(id, s);
            Ok(())
        });
        Ok(id)
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<u32, String> {
        self.check()?;
        let id = self.new_surface();
        self.surface_formats.insert(id, (format, false));
        self.post(move |w| {
            let s = w.inner.create_texture_target(format, width, height)?;
            w.surfaces.insert(id, s);
            Ok(())
        });
        Ok(id)
    }

    fn destroy_surface(&mut self, surface: u32) -> Result<(), String> {
        self.check()?;
        self.surface_formats.remove(&surface);
        self.post(move |w| {
            let s = w.surface(surface)?;
            w.surfaces.remove(&surface);
            w.inner.destroy_surface(s)
        });
        Ok(())
    }

    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<ProxyTexture<B>, String> {
        self.check()?;
        let view = self.judge.as_ref().is_none_or(|j| j.upload_view(texture));
        let t = self.new_texture(view);
        let id = t.id;
        let texture = texture.clone();
        self.post(move |w| {
            let h = w.inner.upload_texture(&texture)?;
            w.textures.insert(id, h);
            Ok(())
        });
        Ok(t)
    }

    fn bc_supported(&self) -> bool {
        self.bc
    }

    fn clear(&mut self, color: u32, depth: Option<u32>, clear: Clear) -> Result<(), String> {
        self.check()?;
        self.post(move |w| {
            let (c, d) = (w.surface(color)?, w.depth(depth)?);
            w.inner.clear(c, d, clear)
        });
        Ok(())
    }

    fn draw(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: &ResolvedDraw,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> Result<DrawOutcome, String> {
        self.check()?;
        let ids = textures.map(|t| t.map(|t| t.id));
        // The outcome the inner backend will give, decided now from the
        // draw and the textures' views; speculated as drawn without a
        // judge. A skipped draw still goes to the render thread: the inner
        // backend submits a pass for another target before it skips.
        let judged = match self.judge.as_mut() {
            Some(j) => j.draw(draw, textures.map(|t| t.map(|t| t.view))),
            None => DrawOutcome::Drawn,
        };
        let expect = judged.clone();
        let draw = Box::new(draw.clone());
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let (c, d) = (w.surface(color)?, w.depth(depth)?);
            let mut tex: [Option<&B::Texture>; 4] = [None; 4];
            for (slot, id) in tex.iter_mut().zip(ids) {
                if let Some(id) = id {
                    *slot = Some(
                        w.textures
                            .get(&id)
                            .ok_or_else(|| format!("render thread: texture {id} is not live"))?,
                    );
                }
            }
            let r = w.inner.draw(c, d, &draw, tex)?;
            if r != expect {
                shared.miss(format!("{}: answered {expect:?}, the backend gave {r:?}", draw.entry));
            }
            Ok(())
        });
        Ok(judged)
    }

    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        self.present_with_gamma(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: u32,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.check()?;
        let gamma = gamma.map(|g| Box::new(*g));
        self.presents += 1;
        let n = self.presents;
        self.post(move |w| {
            let s = w.surface(surface)?;
            let r = w.inner.present_with_gamma(s, flags, gamma.as_deref());
            *w.shared.presented.lock().unwrap() = n;
            w.shared.presented_cv.notify_all();
            r?;
            w.land_pixel_counts()
        });
        // Frame-in-flight limit: wall time only, never logical time.
        let t = std::time::Instant::now();
        let mut done = self.shared.presented.lock().unwrap();
        if n.saturating_sub(*done) > MAX_FRAMES_IN_FLIGHT {
            self.waits.throttles += 1;
            while n.saturating_sub(*done) > MAX_FRAMES_IN_FLIGHT {
                done = self.shared.presented_cv.wait(done).unwrap();
            }
            self.waits.throttle_wait += t.elapsed();
            THROTTLES.fetch_add(1, Ordering::Relaxed);
            THROTTLE_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        Ok(())
    }

    fn clear_rects(
        &mut self,
        color: u32,
        depth: Option<u32>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        self.check()?;
        let clear = clear.clone();
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let (c, d) = (w.surface(color)?, w.depth(depth)?);
            let r = w.inner.clear_rects(c, d, &clear)?;
            if let Err(e) = expect_drawn("clear_rects", r) {
                shared.miss(e);
            }
            Ok(())
        });
        Ok(DrawOutcome::Drawn)
    }

    fn read_surface(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.call(move |w| {
            let s = w.surface(surface)?;
            w.inner.read_surface(s, format, width, height, layout)
        })
    }

    fn read_depth(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.call(move |w| {
            let s = w.surface(surface)?;
            w.inner.read_depth(s, format, width, height, layout)
        })
    }

    fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
        self.check()?;
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let (s, d) = (w.surface(src)?, w.surface(dst)?);
            if !w.inner.copy_surface(s, d)? {
                shared.miss("copy_surface returned false".into());
            }
            Ok(())
        });
        Ok(true)
    }

    fn write_surface(
        &mut self,
        surface: u32,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.check()?;
        let desc = desc.clone();
        let bytes = bytes.to_vec();
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let s = w.surface(surface)?;
            if !w.inner.write_surface(s, &desc, &bytes)? {
                shared.miss("write_surface returned false".into());
            }
            Ok(())
        });
        Ok(true)
    }

    fn snapshot_surface(
        &mut self,
        surface: u32,
        opaque: bool,
    ) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        let t = self.new_texture(self.surface_view(surface));
        let id = t.id;
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let s = w.surface(surface)?;
            match w.inner.snapshot_surface(s, opaque)? {
                Some(h) => {
                    w.textures.insert(id, h);
                }
                None => shared.miss("snapshot_surface returned None".into()),
            }
            Ok(())
        });
        Ok(Some(t))
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.check()?;
        self.post(|w| w.inner.clear_pixel_count());
        Ok(())
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.check()?;
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            match w.inner.report_pixel_count()? {
                Some(t) => {
                    w.tickets.insert(ticket, t);
                }
                None => shared.miss("report_pixel_count returned None".into()),
            }
            Ok(())
        });
        Ok(Some(ticket))
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        // Already back: the count the inner backend's read gives, with no
        // wait for the work queued since.
        self.check()?;
        if let Some(c) = self.shared.landed.lock().unwrap().remove(&ticket) {
            LANDED_COUNTS.fetch_add(1, Ordering::Relaxed);
            self.waits.landed_counts += 1;
            return Ok(c);
        }
        self.call(move |w| {
            // It may have landed after the check above.
            if let Some(c) = w.shared.landed.lock().unwrap().remove(&ticket) {
                return Ok(c);
            }
            let t = w
                .tickets
                .remove(&ticket)
                .ok_or_else(|| format!("pixel-count ticket {ticket} is not live"))?;
            w.inner.read_pixel_count(t)
        })
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.shared.landed.lock().unwrap().remove(&ticket);
        self.post(move |w| {
            w.shared.landed.lock().unwrap().remove(&ticket);
            if let Some(t) = w.tickets.remove(&ticket) {
                w.inner.drop_pixel_count(t);
            }
            Ok(())
        });
    }

    fn can_sample_surfaces(&self) -> bool {
        self.sample_surfaces
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        self.sample_depth
    }

    fn depth_level_texture(&mut self, surface: u32) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        // An RGBA8 texture: it has a view.
        let t = self.new_texture(true);
        let id = t.id;
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let s = w.surface(surface)?;
            match w.inner.depth_level_texture(s)? {
                Some(h) => {
                    w.textures.insert(id, h);
                }
                None => shared.miss("depth_level_texture returned None".into()),
            }
            Ok(())
        });
        Ok(Some(t))
    }

    fn defers_depth_aliases(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn depth_alias_texture(
        &mut self,
        depth: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        let view = match &self.judge {
            None => true,
            Some(j) => match self
                .alias_views
                .iter()
                .find(|(d, s, _)| d == desc && *s == options.signed)
            {
                Some(&(_, _, v)) => v,
                None => {
                    // The host format depends on the description only:
                    // decode zeros once to learn it.
                    let len = desc.layout().map_err(|e| e.to_string())?.total_size;
                    let v = gfx::texture::decode_texture(desc, &vec![0; len], None, options)
                        .is_ok_and(|t| j.upload_view(&t));
                    self.alias_views.push((desc.clone(), options.signed, v));
                    v
                }
            },
        };
        let t = self.new_texture(view);
        let id = t.id;
        let desc = desc.clone();
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            // One read-back per depth generation, as the device does it.
            if w
                .depth_words
                .as_ref()
                .is_none_or(|(s, g, _)| (*s, *g) != (depth, generation))
            {
                let s = w.surface(depth)?;
                let layout = GuestLayoutKind::Linear { pitch };
                let Some(bytes) = w.inner.read_depth(s, format, width, height, layout)? else {
                    shared.miss("read_depth returned None for a depth-alias texture".into());
                    return Ok(());
                };
                w.depth_words = Some((depth, generation, bytes));
            }
            let bytes = &w.depth_words.as_ref().expect("read above").2;
            let decoded = match bytes
                .get(offset..)
                .ok_or_else(|| format!("read_depth returned {} bytes, offset {offset}", bytes.len()))
                .and_then(|b| {
                    gfx::texture::decode_texture(&desc, b, None, options).map_err(|e| e.to_string())
                }) {
                Ok(d) => d,
                Err(e) => {
                    shared.miss(format!("depth-alias texture: {e}"));
                    return Ok(());
                }
            };
            let h = w.inner.upload_texture(&decoded)?;
            w.textures.insert(id, h);
            Ok(())
        });
        Ok(Some(t))
    }

    fn surface_texture(&mut self, surface: u32) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        if !self.sample_surfaces {
            return Ok(None);
        }
        let t = self.new_texture(self.surface_view(surface));
        let id = t.id;
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let s = w.surface(surface)?;
            match w.inner.surface_texture(s)? {
                Some(h) => {
                    w.textures.insert(id, h);
                }
                None => shared.miss("surface_texture returned None".into()),
            }
            Ok(())
        });
        Ok(Some(t))
    }
}
