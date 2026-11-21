//! Join the scene builder's output to the device stream from the same
//! begin/seal interval. A renderer holds both by value. Private work runs
//! on its render thread even while the guest is paused.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::thread::{self, JoinHandle, Thread};

use agent::compare::Rgba;
use agent::render::{PrivateCaptureScene, PrivateCaptureWrite, set_private_capture_source};
use agent_proto::{CaptureLighting, FrameCaptureParams, RpcError, code};
use d3d::scene_sink::ReplayFrame;
use frame_scene::mailbox::{self, Mode, Publisher};
use frame_scene::{Cut, FrameScene, SceneId, ViewKind};
use native_render::frame_graph;
use render_bridge::scene_step::{self, SealObserver};

type Identity = (u64, SceneId);

/// Exactly one swap can supply a game-view frame. For instance, after a
/// load it is possible to see the same guest frame and tick a second time.
struct Pending<T> {
    item: Option<(Identity, Result<T, &'static str>)>,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Self { item: None }
    }
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

/// Metadata from the physical display surface of this Swap. The scaled
/// backend may present from its reference, or adopt a new scale immediately
/// afterwards. Reading the global scale at either scene seal or take can
/// therefore give a different answer from the surface that was shown.
#[derive(Default)]
struct DisplaySize {
    extent: OnceLock<[u32; 2]>,
    failed: AtomicBool,
}

impl DisplaySize {
    fn ready(&self) -> bool {
        self.extent.get().is_some() || self.failed.load(Ordering::Acquire)
    }

    fn scale(&self, guest: Option<frame_scene::AuxTarget>) -> Result<u32, String> {
        if self.failed.load(Ordering::Acquire) {
            return Err("reference present failed".into());
        }
        let &[width, height] = self
            .extent
            .get()
            .ok_or("display size unavailable for this Swap")?;
        let guest = guest.ok_or("no guest present target descriptor")?;
        let scale = width
            .checked_div(guest.width)
            .ok_or("guest present target has zero width")?;
        if !(1..=d3d::MAX_SCALE).contains(&scale)
            || guest.width.checked_mul(scale) != Some(width)
            || guest.height == 0
            || guest.height.checked_mul(scale) != Some(height)
        {
            return Err(format!(
                "display target {width}x{height} is not an integer scale of {}x{}",
                guest.width, guest.height
            ));
        }
        Ok(scale)
    }
}

struct Collected {
    replay: ReplayFrame,
    display: Arc<DisplaySize>,
}

struct Inputs {
    scene: Arc<FrameScene>,
    replay: Arc<ReplayFrame>,
    display: Arc<DisplaySize>,
    hash: OnceLock<u64>,
}

impl Inputs {
    fn pixels(
        &self,
        backend: &mut d3d::GfxBackend,
        scale: u32,
        lighting: d3d::Lighting,
        capture: Option<&PrivateCaptureWrite>,
    ) -> Result<Rgba, String> {
        let drawn = if let Some(request) = capture {
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
            if !request.waiting() {
                // A replacement can arrive during readback. Dropping the
                // observer releases its arming for the outstanding capture.
                return Err("private capture was interrupted".into());
            }
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
    pending: Pending<Collected>,
    // Weak so expired, omitted or superseded frames retain no GPU inputs.
    displays: BTreeMap<u64, Weak<DisplaySize>>,
    parity_wait: Option<Arc<Inputs>>,
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
        self.displays.clear();
        self.parity_wait = None;
        self.recent.clear();
        self.published = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn track(&mut self, frame: &Collected) {
        self.displays.retain(|_, sample| sample.strong_count() != 0);
        self.displays
            .insert(frame.replay.present, Arc::downgrade(&frame.display));
    }

    fn ready_parity(&mut self) -> Option<(u64, Arc<Inputs>)> {
        if self.parity_wait.as_ref()?.display.ready() {
            Some((self.generation, self.parity_wait.take()?))
        } else {
            None
        }
    }

    fn newest(&self) -> Option<&Entry> {
        self.published.as_ref().or_else(|| self.recent.back())
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

    fn select(&self, tick: Option<u64>) -> Result<Option<&Arc<Inputs>>, RpcError> {
        let Some(newest) = self.newest() else {
            return Ok(None);
        };
        let entry = match tick {
            None => newest,
            Some(t) if t == u64::from(newest.id.tick) => newest,
            Some(t)
                if t.saturating_sub(u64::from(newest.id.tick)) > FrameCaptureParams::MAX_AHEAD =>
            {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    format!(
                        "tick {t} is more than {} ticks ahead of sealed tick {}",
                        FrameCaptureParams::MAX_AHEAD,
                        newest.id.tick,
                    ),
                ));
            }
            Some(t) if t > u64::from(newest.id.tick) => return Ok(None),
            Some(t) => self
                .recent
                .iter()
                .rev()
                .find(|e| u64::from(e.id.tick) == t)
                .ok_or_else(|| {
                    RpcError::new(
                        code::UNAVAILABLE,
                        format!("sealed scene tick {t} was skipped or has expired"),
                    )
                })?,
        };
        entry
            .inputs
            .as_ref()
            .map(Some)
            .map_err(|e| RpcError::new(code::UNAVAILABLE, e.clone()))
    }
}

enum Job {
    Png {
        frame: Arc<Inputs>,
        lighting: d3d::Lighting,
        output: PrivateCaptureWrite,
    },
    Parity(Arc<Inputs>),
}

struct RenderJob {
    generation: u64,
    work: Job,
}

impl RenderJob {
    fn run(self, gpu: &mut d3d::GfxBackend, current: u64) {
        let draw = |gpu: &mut d3d::GfxBackend,
                    frame: &Inputs,
                    lighting,
                    capture: Option<&PrivateCaptureWrite>| {
            if current != self.generation {
                return Err("scene timeline changed".to_owned());
            }
            frame.pixels(
                gpu,
                frame.display.scale(frame.replay.colour)?,
                lighting,
                capture,
            )
        };
        match self.work {
            Job::Png {
                frame,
                lighting,
                output,
            } => {
                if output.waiting() {
                    let image = draw(gpu, &frame, lighting, Some(&output));
                    output.finish(image);
                }
            }
            Job::Parity(frame) => {
                if !crate::frame_parity::wanted(frame.replay.present) {
                    return;
                }
                let result = draw(gpu, &frame, d3d::Lighting::Retail, None);
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

fn has_game_view(scene: &FrameScene, replay: &ReplayFrame) -> bool {
    // An empty view never visits a camera-extraction hook. Its actual
    // entry and fully carried overlay are sufficient to replay that Swap.
    scene.views.iter().any(|v| v.key.kind == ViewKind::Game)
        || (replay.entered_game_view && replay.ops.iter().all(|(at, _)| !at.slot.in_game_view()))
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
        if let Ok(frame) = &pending {
            scenes.track(frame);
        }
        set_private_capture_source(Some(capture));
        crate::frame_parity::reset("scene timeline changed");
    }
    let present = pending.as_ref().ok().map(|f| f.replay.present);
    let inputs = pending.map_err(str::to_owned).and_then(
        |Collected {
             mut replay,
             display,
         }| {
            if !has_game_view(&scene, &replay) {
                return Err("sealed scene has no game view".to_owned());
            }
            if !replay.refusals.is_empty() {
                return Err(replay.refusals.join(", "));
            }
            replay.classify_cameras(&scene);
            Ok(Arc::new(Inputs {
                scene: Arc::clone(&scene),
                replay: Arc::new(replay),
                display,
                hash: OnceLock::new(),
            }))
        },
    );
    let generation = scenes.generation;
    let entry = Entry {
        id: scene.id,
        generation,
        inputs: inputs.clone(),
    };
    scenes.published = Some(entry.clone());
    if let Some(present) = present.filter(|&n| crate::frame_parity::wanted(n)) {
        match inputs {
            Ok(frame) => scenes.parity_wait = Some(frame),
            Err(reason) => crate::frame_parity::native(present, Err(reason)),
        }
    }
    let parity = scenes.ready_parity();
    drop(scenes);
    shared.submit_parity(parity);
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
    fn ready(&self) -> bool {
        self.frame.display.ready()
    }

    fn draw(&self, lighting: CaptureLighting, output: PrivateCaptureWrite) -> Result<(), String> {
        let owner = self.owner.upgrade().ok_or("frame renderer stopped")?;
        let lighting = match lighting {
            CaptureLighting::Retail => d3d::Lighting::Retail,
            CaptureLighting::E3 => d3d::Lighting::E3,
        };
        owner.enqueue(
            self.generation,
            Job::Png {
                frame: Arc::clone(&self.frame),
                lighting,
                output,
            },
        )
    }
}

fn capture(tick: Option<u64>) -> Result<Option<Arc<dyn PrivateCaptureScene>>, RpcError> {
    let shared =
        active().ok_or_else(|| RpcError::new(code::UNAVAILABLE, "frame renderer stopped"))?;
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
    // A reference submission needs the same pair as an adopted frame so
    // the worker can freeze its incoming pixels before the original draw.
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
        let known = if let Some(display) = scenes.displays.get(&present).and_then(Weak::upgrade) {
            display.failed.store(true, Ordering::Release);
            true
        } else {
            false
        };
        if let Some((_, slot)) = &mut scenes.pending.item
            && slot
                .as_ref()
                .is_ok_and(|frame| frame.replay.present == present)
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
        }
        if known {
            // Invalidates an RPC pin even if newer frames have since been published.
            set_private_capture_source(Some(capture));
        }
    }
    crate::frame_parity::reference_error(present);
}

/// Publication of scenes to the private renderer is independent of the display queue.
pub(crate) fn private_mailbox_stats() -> Option<mailbox::Stats> {
    Some(active()?.frames.stats())
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
    fn submit_parity(&self, request: Option<(u64, Arc<Inputs>)>) {
        if let Some((generation, frame)) = request {
            let present = frame.replay.present;
            if let Err(error) = self.enqueue(generation, Job::Parity(frame)) {
                crate::frame_parity::native(present, Err(error));
            }
        }
    }

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
    pub fn new(mut backend: d3d::GfxBackend, mode: Mode) -> std::io::Result<Self> {
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
                        request.run(&mut backend, generation);
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
    ) -> impl FnMut(
        u64,
        &gfx::GfxContext,
        &gfx::surface::SurfaceStore,
        gfx::surface::SurfaceId,
        d3d::Lighting,
    )
    + Send
    + 'static
    + use<> {
        let weak = Arc::downgrade(&self.shared);
        let observe = crate::frame_parity::reference_observer();
        move |present, ctx, store, id, lighting| {
            let Some(shared) = weak.upgrade() else { return };
            if shared.stopping.load(Ordering::Acquire) {
                return;
            }
            let parity = {
                let mut scenes = shared.scenes.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(display) = scenes.displays.get(&present).and_then(Weak::upgrade) {
                    match store.get(id) {
                        Ok(surface) => {
                            let desc = surface.desc();
                            let _ = display.extent.set([desc.width, desc.height]);
                        }
                        Err(_) => display.failed.store(true, Ordering::Release),
                    }
                }
                scenes.ready_parity()
            };
            shared.submit_parity(parity);
            if crate::frame_parity::wanted(present) {
                observe(present, ctx, store, id, lighting);
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
                if replay.has_game_view() && crate::frame_parity::armed() {
                    if !replay.refusals.is_empty() {
                        crate::frame_parity::refuse(replay.refusals.join(", "));
                    } else if replay.guest_reference_complete() {
                        crate::frame_parity::bind(replay.present, id.1.tick);
                    }
                }
                if let Some((previous, Ok(frame))) = &scenes.pending.item
                    && *previous == id
                    && crate::frame_parity::wanted(frame.replay.present)
                {
                    crate::frame_parity::native(
                        frame.replay.present,
                        Err("multiple presents in the sealed scene".to_owned()),
                    );
                }
                let frame = Collected {
                    replay,
                    display: Arc::new(DisplaySize::default()),
                };
                scenes.track(&frame);
                scenes.pending.put(id, frame);
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
    fn an_empty_game_view_needs_its_entry_mark_to_replay_without_an_extracted_camera() {
        use d3d::scene_sink::ReplayOp;
        use frame_scene::{Slot, SlotAt, TargetRef};

        let scene = FrameScene::default();
        let mut replay = ReplayFrame::default();
        assert!(!has_game_view(&scene, &replay));
        replay.entered_game_view = true;
        assert!(has_game_view(&scene, &replay));
        replay.ops.push((SlotAt { slot: Slot::Overlay, view: 0 }, ReplayOp::Clear {
            colour: TargetRef::SceneColour, depth: TargetRef::None, clear: gfx::Clear::NONE,
        }));
        assert!(has_game_view(&scene, &replay));
        replay.ops[0].0.slot = Slot::Opaque;
        assert!(!has_game_view(&scene, &replay));
    }

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
                display: Arc::new(DisplaySize::default()),
                hash: OnceLock::new(),
            })),
        }
    }

    #[test]
    fn captures_use_the_actual_display_size_of_their_own_swap() {
        let guest = Some(frame_scene::AuxTarget {
            width: 8,
            height: 4,
            ..Default::default()
        });
        let mut scenes = Scenes::default();
        let a = entry(2, 0);
        let b = entry(3, 0);
        let first = Arc::clone(a.inputs.as_ref().unwrap());
        let second = Arc::clone(b.inputs.as_ref().unwrap());
        scenes.retain(a);
        scenes.published = Some(b);

        assert_eq!(
            scenes.select(None).unwrap().unwrap().scene.id.tick,
            3,
            "newest seal can precede mailbox take"
        );
        scenes.parity_wait = Some(Arc::clone(&first));
        assert!(scenes.ready_parity().is_none());
        second.display.extent.set([32, 16]).unwrap();
        assert_eq!(second.display.scale(guest), Ok(4));
        assert!(
            scenes.ready_parity().is_none(),
            "the later Swap is not the armed reference"
        );

        first.display.extent.set([8, 4]).unwrap();
        let (_, reference) = scenes.ready_parity().unwrap();
        assert_eq!(reference.replay.present, first.replay.present);
        assert_eq!(
            reference.display.scale(guest),
            Ok(1),
            "a presented reference can be 1x"
        );
        assert!(
            scenes.ready_parity().is_none(),
            "only schedule the draw once"
        );
        first.display.failed.store(true, Ordering::Release);
        assert!(reference.display.scale(guest).is_err());
        let anisotropic = DisplaySize::default();
        anisotropic.extent.set([16, 4]).unwrap();
        assert!(anisotropic.scale(guest).is_err());
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
        assert_eq!(capture(Some(14 + FrameCaptureParams::MAX_AHEAD)), Ok(None));
        assert_eq!(
            capture(Some(15 + FrameCaptureParams::MAX_AHEAD))
                .unwrap_err()
                .code,
            code::INVALID_PARAMS
        );
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
            scenes.select(Some(8)).err().map(|e| e.message).as_deref(),
            Some("missing texture")
        );
        assert_eq!(
            scenes.select(None).err().map(|e| e.message).as_deref(),
            Some("missing texture")
        );
    }
}
