//! Join the scene builder's output to the device stream from the same
//! begin/seal interval. A renderer holds both by value. Private work runs
//! on its render thread even while the guest is paused.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::thread::{self, JoinHandle, Thread};

use agent::compare::Rgba;
use agent::render::{PrivateCaptureScene, set_private_capture_source};
use agent_proto::CaptureLighting;
use d3d::scene_sink::ReplayFrame;
use frame_scene::mailbox::{self, Mode, Publisher};
use frame_scene::{Cut, FrameScene, SceneId, ViewKind};
use native_render::frame_graph;
use render_bridge::scene_step::{self, SealObserver};

type Identity = (u64, SceneId);

/// Exactly one swap can supply a game-view frame. For instance, after a
/// load it is possible to see the same guest frame and tick a second time.
#[derive(Default)]
struct Pending<T> {
    item: Option<(Identity, Result<T, &'static str>)>,
}

impl<T> Pending<T> {
    fn put(&mut self, id: Identity, value: T) {
        let duplicate = self.item.as_ref().is_some_and(|(key, _)| *key == id);
        let item = if duplicate {
            Err("multiple presents in the sealed scene")
        } else {
            Ok(value)
        };
        self.item = Some((id, item));
    }

    fn take(&mut self, id: Identity) -> Result<T, &'static str> {
        match self.item.take() {
            Some((got, item)) if got == id => item,
            _ => Err("no present in the sealed scene"),
        }
    }
}

struct Inputs {
    scene: Arc<FrameScene>,
    replay: Arc<ReplayFrame>,
    // A pair of captures keeps the scale of the first draw of this scene.
    scale: OnceLock<u32>,
    hash: OnceLock<u64>,
}

impl Inputs {
    fn pixels(
        &self,
        backend: &mut d3d::GfxBackend,
        scale: u32,
        lighting: d3d::Lighting,
        observed: bool,
    ) -> Result<Rgba, String> {
        let drawn = if observed {
            let mut probe = crate::render_probe::PrivateFrame::begin(
                &self.scene,
                &self.replay,
                self.replay.present,
                lighting,
            );
            let frame = frame_graph::draw_private_with_observer(
                backend,
                &self.scene,
                &self.replay,
                scale,
                lighting,
                Some(&mut probe),
            )?;
            probe.completed(&frame);
            frame
        } else {
            frame_graph::draw_private(backend, &self.scene, &self.replay, scale, lighting)?
        };
        self.hash.get_or_init(|| drawn.scene_hash);
        let mut pixels = drawn.rgba;
        // Frame captures, like the displayed X8 back buffer, are opaque.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Ok(Rgba {
            width: drawn.size[0],
            height: drawn.size[1],
            pixels,
        })
    }
}

#[derive(Clone)]
struct Entry {
    generation: u64,
    id: SceneId,
    inputs: Result<Arc<Inputs>, String>,
}

#[derive(Default)]
struct Scenes {
    pending: Pending<ReplayFrame>,
    // The two latest scenes taken by the render thread. Explicit capture requests also pin
    // their first successful selection across the second request.
    recent: VecDeque<Entry>,
    // A canonical transaction pins this on the game thread at Swap,
    // before the worker starts the owning draw.
    published: Option<Entry>,
    generation: u64,
}

impl Scenes {
    fn reset(&mut self) {
        self.pending.item = None;
        self.recent.clear();
        self.published = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn retain(&mut self, entry: Entry) {
        if entry.generation != self.generation {
            return;
        }
        if self.recent.len() == 2 {
            self.recent.pop_front();
        }
        self.recent.push_back(entry);
    }

    fn select(&self, tick: Option<u64>) -> Result<Option<&Arc<Inputs>>, String> {
        let Some(newest) = self.recent.back() else {
            return Ok(None);
        };
        let entry = match tick {
            None => newest,
            Some(t) if t > u64::from(newest.id.tick) => return Ok(None),
            Some(t) => self
                .recent
                .iter()
                .rev()
                .find(|e| u64::from(e.id.tick) == t)
                .ok_or_else(|| format!("sealed scene tick {t} was skipped or has expired"))?,
        };
        entry.inputs.as_ref().map(Some).map_err(Clone::clone)
    }
}

enum Job {
    Png {
        frame: Arc<Inputs>,
        lighting: d3d::Lighting,
        path: PathBuf,
        done: mpsc::Sender<Result<(), String>>,
    },
    Parity(Arc<Inputs>),
}

struct RenderJob {
    generation: u64,
    work: Job,
}

impl RenderJob {
    fn run(self, gpu: &mut d3d::GfxBackend, current: u64, scale: &dyn Fn() -> u32) {
        let draw = |gpu: &mut d3d::GfxBackend, frame: &Inputs, lighting, observed| {
            if current != self.generation {
                return Err("scene timeline changed".to_owned());
            }
            frame.pixels(gpu, *frame.scale.get_or_init(scale), lighting, observed)
        };
        match self.work {
            Job::Png {
                frame,
                lighting,
                path,
                done,
            } => {
                let result = draw(gpu, &frame, lighting, true).and_then(|rgba| {
                    agent::compare::write_png(&path, &rgba).map_err(|e| e.to_string())
                });
                let _ = done.send(result);
            }
            Job::Parity(frame) => {
                let result = draw(gpu, &frame, d3d::Lighting::Retail, false);
                crate::frame_parity::native(frame.replay.present, result);
            }
        }
    }
}

struct Shared {
    scenes: Arc<Mutex<Scenes>>,
    frames: Publisher<Entry>,
    jobs: mpsc::SyncSender<RenderJob>,
    worker: Thread,
    stopping: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Weak<Shared>> = Mutex::new(Weak::new());

fn active() -> Option<Arc<Shared>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).upgrade()
}

fn sealed(sequence: u64, scene: Arc<FrameScene>) {
    let Some(shared) = active() else { return };
    let mut scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
    let pending = scenes.pending.take((sequence, scene.id));
    if scenes.published.as_ref().is_some_and(|prev| {
        prev.id.map_epoch != scene.id.map_epoch
            || prev.id.tick > scene.id.tick
            || scene.cut.contains(Cut::REVERT)
    }) {
        scenes.reset();
        set_private_capture_source(Some(capture));
    }
    let present = pending.as_ref().ok().map(|f| f.present);
    let inputs = pending.map_err(str::to_owned).and_then(|mut replay| {
        if !scene.views.iter().any(|v| v.key.kind == ViewKind::Game) {
            return Err("sealed scene has no game view".to_owned());
        }
        if !replay.refusals.is_empty() {
            return Err(replay.refusals.join(", "));
        }
        replay.classify_cameras(&scene);
        Ok(Arc::new(Inputs {
            scene: Arc::clone(&scene),
            replay: Arc::new(replay),
            scale: OnceLock::new(),
            hash: OnceLock::new(),
        }))
    });
    let generation = scenes.generation;
    let entry = Entry {
        id: scene.id,
        generation,
        inputs: inputs.clone(),
    };
    scenes.published = Some(entry.clone());
    drop(scenes);

    if let Some(present) = present.filter(|&n| crate::frame_parity::wanted(n)) {
        let queued = inputs
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|frame| shared.enqueue(generation, Job::Parity(Arc::clone(frame))));
        if let Err(e) = queued {
            crate::frame_parity::native(present, Err(e));
        }
    }
    // Only locked publication may wait. Never hold the builder or
    // capture-state lock while the render thread makes room.
    if shared.frames.publish(entry).is_ok() {
        shared.worker.unpark();
    }
}

fn reset() {
    if let Some(shared) = active() {
        shared
            .scenes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset();
        set_private_capture_source(Some(capture));
        crate::frame_parity::reset("scene timeline reset");
    }
}

struct Selection {
    frame: Arc<Inputs>,
    generation: u64,
    owner: Weak<Shared>,
}

impl PrivateCaptureScene for Selection {
    fn tick(&self) -> u64 {
        u64::from(self.frame.scene.id.tick)
    }
    fn present(&self) -> u64 {
        self.frame.replay.present
    }
    fn scene_hash(&self) -> u64 {
        *self
            .frame
            .hash
            .get_or_init(|| frame_scene::summary_hash(&self.frame.scene))
    }
    fn draw(
        &self,
        lighting: CaptureLighting,
        path: PathBuf,
    ) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let owner = self.owner.upgrade().ok_or("frame renderer stopped")?;
        let lighting = match lighting {
            CaptureLighting::Retail => d3d::Lighting::Retail,
            CaptureLighting::E3 => d3d::Lighting::E3,
        };
        let (done, result) = mpsc::channel();
        owner.enqueue(
            self.generation,
            Job::Png {
                frame: Arc::clone(&self.frame),
                lighting,
                path,
                done,
            },
        )?;
        Ok(result)
    }
}

fn capture(tick: Option<u64>) -> Result<Option<Arc<dyn PrivateCaptureScene>>, String> {
    let shared = active().ok_or("frame renderer stopped")?;
    let scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
    Ok(scenes.select(tick)?.map(|frame| {
        Arc::new(Selection {
            frame: Arc::clone(frame),
            generation: scenes.generation,
            owner: Arc::downgrade(&shared),
        }) as Arc<dyn PrivateCaptureScene>
    }))
}

/// Pin the seal associated with this exact guest swap for a transaction
/// on the display renderer. A miss leaves the queued commands in order.
pub fn pair_for_present(present: u64) -> Option<(Arc<FrameScene>, Arc<ReplayFrame>)> {
    // The parity comparison's reference must execute guest commands on
    // the original backend, including on a device eligible for cutover.
    if crate::frame_parity::wanted(present) {
        return None;
    }
    let shared = active()?;
    let scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
    let frame = scenes.published.as_ref()?.inputs.as_ref().ok()?;
    if frame.replay.present != present {
        return None;
    }
    Some((Arc::clone(&frame.scene), Arc::clone(&frame.replay)))
}

/// No image may be returned for a Swap that the reference failed to
/// submit, even if its present number was already reserved in the sink.
pub fn present_failed(present: u64) {
    if let Some(shared) = active() {
        let mut scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, slot)) = &mut scenes.pending.item
            && slot.as_ref().is_ok_and(|frame| frame.present == present)
        {
            *slot = Err("reference present failed");
        }
        if scenes.published.as_ref().is_some_and(|entry| {
            entry
                .inputs
                .as_ref()
                .is_ok_and(|f| f.replay.present == present)
        }) {
            scenes.reset();
            set_private_capture_source(Some(capture));
        }
    }
    crate::frame_parity::reference_error(present);
}

/// Camera use belongs to a particular sealed scene, including its map epoch.
pub fn camera_counts(id: SceneId, view: u8) -> Option<[u32; 3]> {
    let shared = active()?;
    let scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
    let entry = scenes
        .published
        .iter()
        .chain(scenes.recent.iter().rev())
        .find(|e| e.id == id)?;
    Some(entry.inputs.as_ref().ok()?.replay.camera_counts(view))
}

impl Shared {
    fn enqueue(&self, generation: u64, work: Job) -> Result<(), String> {
        self.jobs
            .try_send(RenderJob { generation, work })
            .map_err(|err| match err {
                mpsc::TrySendError::Disconnected(_) => "frame renderer stopped".to_owned(),
                mpsc::TrySendError::Full(_) => "frame renderer capture queue is full".to_owned(),
            })?;
        self.worker.unpark();
        Ok(())
    }
}

/// Render resources live on one thread. The original device retains only a
/// weak callback, so teardown also works when a guest hook holds the device.
pub struct Service {
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl Service {
    pub fn new(
        mut backend: d3d::GfxBackend,
        mode: Mode,
        current_scale: Box<dyn Fn() -> u32 + Send + Sync>,
    ) -> std::io::Result<Self> {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if active.upgrade().is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "a frame renderer is already installed",
            ));
        }
        let (tx, rx) = mailbox::channel::<Entry>(mode);
        let (jobs, requests) = mpsc::sync_channel::<RenderJob>(2);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let scenes = Arc::new(Mutex::new(Scenes::default()));
        let retained = Arc::clone(&scenes);
        let join = thread::Builder::new()
            .name("scene-render".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let request = requests.try_recv().ok();
                    let entry = rx.try_take();
                    if request.is_none() && entry.is_none() {
                        thread::park();
                        continue;
                    }
                    if let Some(entry) = entry {
                        if let Ok(frame) = &entry.inputs {
                            frame.scale.get_or_init(&current_scale);
                        }
                        retained
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .retain(entry);
                    }
                    if let Some(request) = request {
                        let generation = retained
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .generation;
                        request.run(&mut backend, generation, &current_scale);
                    }
                }
            })?;
        let worker = join.thread().clone();
        let shared = Arc::new(Shared {
            scenes,
            frames: tx,
            jobs,
            worker,
            stopping,
        });
        *active = Arc::downgrade(&shared);
        drop(active);
        scene_step::set_seal_observer(Some(SealObserver { sealed, reset }));
        set_private_capture_source(Some(capture));
        crate::frame_parity::install();
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    /// Only this installation's display backend may supply a reference
    /// image. Late presents from an earlier device do not enter a new pair.
    pub fn reference_observer(
        &self,
    ) -> impl FnMut(u64, &gfx::GfxContext, &gfx::surface::SurfaceStore, gfx::surface::SurfaceId)
    + Send
    + 'static
    + use<> {
        let weak = Arc::downgrade(&self.shared);
        let observe = crate::frame_parity::reference_observer();
        move |present, ctx, store, id| {
            if crate::frame_parity::wanted(present)
                && weak
                    .upgrade()
                    .is_some_and(|s| !s.stopping.load(Ordering::Acquire))
            {
                observe(present, ctx, store, id);
            }
        }
    }

    pub fn recorder(&self) -> impl FnMut(ReplayFrame) + Send + 'static + use<> {
        let weak = Arc::downgrade(&self.shared);
        move |replay| {
            let Some(shared) = weak.upgrade() else { return };
            let id = scene_step::open_identity();
            let mut scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(id) = id {
                if crate::frame_parity::armed() {
                    if !replay.refusals.is_empty() {
                        crate::frame_parity::refuse(replay.refusals.join(", "));
                    } else if replay.guest_reference_complete() {
                        crate::frame_parity::bind(replay.present, id.1.tick);
                    }
                }
                if let Some((previous, Ok(frame))) = &scenes.pending.item
                    && *previous == id
                    && crate::frame_parity::wanted(frame.present)
                {
                    crate::frame_parity::native(
                        frame.present,
                        Err("multiple presents in the sealed scene".to_owned()),
                    );
                }
                scenes.pending.put(id, replay);
            } else {
                scenes.pending.item = None;
            }
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        scene_step::set_seal_observer(None);
        set_private_capture_source(None);
        crate::frame_parity::uninstall();
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.worker.unpark();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Weak::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_present_matches_only_its_own_begin_and_seal() {
        let id = SceneId {
            map_epoch: 1,
            frame: 92,
            tick: 45,
        };
        let mut pending = Pending::default();
        pending.put((1, id), 101);
        assert!(
            pending.take((2, id)).is_err(),
            "a repeated guest identity is insufficient"
        );
        pending.put((2, id), 102);
        pending.put((2, id), 103);
        assert_eq!(
            pending.take((2, id)),
            Err("multiple presents in the sealed scene")
        );
        pending.put((3, id), 104);
        assert_eq!(pending.take((3, id)), Ok(104));
        assert!(pending.take((3, id)).is_err());
    }

    fn entry(tick: u32, generation: u64) -> Entry {
        let id = SceneId {
            map_epoch: 1,
            frame: u64::from(tick),
            tick,
        };
        let scene = FrameScene {
            id,
            ..Default::default()
        };
        Entry {
            generation,
            id,
            inputs: Ok(Arc::new(Inputs {
                scene: Arc::new(scene),
                replay: Arc::new(ReplayFrame {
                    present: u64::from(tick) + 10,
                    ..Default::default()
                }),
                scale: OnceLock::new(),
                hash: OnceLock::new(),
            })),
        }
    }

    #[test]
    fn capture_uses_exact_taken_ticks_and_drops_old_timeline_work() {
        let mut scenes = Scenes::default();
        for tick in [10, 12, 14] {
            scenes.retain(entry(tick, 0));
        }
        let capture = |tick| {
            scenes
                .select(tick)
                .map(|f| f.map(|inputs| inputs.replay.present))
        };
        assert_eq!(capture(Some(12)), Ok(Some(22)));
        assert_eq!(capture(None), Ok(Some(24)));
        assert_eq!(capture(Some(15)), Ok(None));
        assert!(capture(Some(10)).is_err());
        assert!(capture(Some(13)).is_err());
        let old = entry(18, 0);
        scenes.reset();
        scenes.retain(old);
        assert!(scenes.select(None).unwrap().is_none());
        scenes.retain(entry(8, 1));
        assert_eq!(scenes.select(None).unwrap().unwrap().scene.id.tick, 8);
    }

    #[test]
    fn unavailable_scene_reports_its_own_reason() {
        let mut scenes = Scenes::default();
        scenes.retain(entry(7, 0));
        let next = Entry {
            inputs: Err("missing texture".to_owned()),
            ..entry(8, 0)
        };
        scenes.retain(next);
        assert_eq!(
            scenes.select(Some(8)).err().as_deref(),
            Some("missing texture")
        );
        assert_eq!(
            scenes.select(None).err().as_deref(),
            Some("missing texture")
        );
    }
}
