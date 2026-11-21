//! The two inputs to `render.parity` meet by guest present ordinal. One
//! comes from the D3D display surface, the other from a fresh target drawn
//! from the sealed scene that produced that present. The GPU callback may
//! arrive before or after the private draw.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use agent::compare::{Rgba, compare};
use agent::render::ParityRequest;
use agent_proto::{ParityReply, RpcError, code};

/// Zero while idle, ARMED until the game view binds the request, then its
/// present ordinal. Guest presents are numbered from 1.
const ARMED: u64 = u64::MAX;
static TARGET: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<State> = Mutex::new(State { installed: false, session: 0, sequence: 0, job: None });

struct State {
    installed: bool,
    session: u64,
    sequence: u64,
    job: Option<Job>,
}

struct Job {
    client: ParityRequest,
    pair: Option<Pair>,
}

struct Pair {
    present: u64,
    tick: u32,
    half: Option<(Side, Rgba)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Reference,
    Native,
}

type Images = (Rgba, Rgba);

impl Pair {
    fn push(&mut self, present: u64, side: Side, r: Result<Rgba, String>) -> Option<Result<Images, String>> {
        if self.present != present {
            return None;
        }
        let image = match r {
            Ok(image) => image,
            Err(e) => return Some(Err(format!("{side:?} parity image at present {present}: {e}"))),
        };
        match self.half.take() {
            None => {
                self.half = Some((side, image));
                None
            }
            Some((first, _)) if first == side => Some(Err(format!(
                "duplicate {side:?} parity image at present {present}"
            ))),
            Some((Side::Reference, reference)) => Some(Ok((reference, image))),
            Some((Side::Native, native)) => Some(Ok((image, native))),
        }
    }
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn unavailable(message: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, message)
}

/// Register for the lifetime of the renderer that has both paths.
pub fn install() {
    let mut s = state();
    s.session = s.session.wrapping_add(1);
    s.installed = true;
    drop(s);
    agent::render::set_parity_source(Some(arm));
}

/// Disarm any outstanding comparisons before the GPU and scene identity
/// go away. A late completion is then discarded.
pub fn uninstall() {
    agent::render::set_parity_source(None);
    let mut s = state();
    s.installed = false;
    TARGET.store(0, Ordering::Release);
    if let Some(job) = s.job.take() {
        job.client.finish(Err(unavailable("parity renderer stopped")));
    }
}

/// An explicit map or snapshot timeline change invalidates both halves.
pub fn reset(reason: &'static str) {
    let mut s = state();
    TARGET.store(0, Ordering::Release);
    if let Some(job) = s.job.take() {
        job.client.finish(Err(unavailable(reason)));
    }
}

fn arm(client: ParityRequest) {
    let mut s = state();
    if !s.installed {
        client.finish(Err(unavailable("parity renderer stopped")));
        return;
    }
    s.sequence = s.sequence.wrapping_add(1);
    if let Some(previous) = s.job.replace(Job { client, pair: None }) {
        previous.client.finish(Err(RpcError::new(code::INTERRUPTED, "a later render.parity replaced this request")));
    }
    TARGET.store(ARMED, Ordering::Release);
}

/// The guest draw path uses this predicate before admitting a typed
/// replacement. A parity reference contains the original guest draws.
pub fn armed() -> bool {
    waiting_for(ARMED)
}

// Idle paths read one atomic. While a request is active its receiver's
// lifetime also matters: an agent timeout or disconnection must release
// any retained image and stop suppressing typed stage admissions.
fn waiting_for(target: u64) -> bool {
    if TARGET.load(Ordering::Acquire) != target {
        return false;
    }
    let mut s = state();
    if TARGET.load(Ordering::Relaxed) != target {
        return false;
    }
    if s.job.as_ref().is_some_and(|j| j.client.waiting()) {
        return true;
    }
    s.job = None;
    TARGET.store(0, Ordering::Release);
    false
}

/// The replay sink calls this before it forwards the matching present
/// to the reference backend. The tick comes from the scene builder.
pub fn bind(present: u64, tick: u32) {
    if !armed() {
        return;
    }
    let mut s = state();
    let Some(job) = s.job.as_mut() else { return };
    if job.pair.is_some() {
        return;
    }
    if !job.client.waiting() {
        s.job = None;
        TARGET.store(0, Ordering::Release);
        return;
    }
    job.pair = Some(Pair { present, tick, half: None });
    TARGET.store(present, Ordering::Release);
}

/// Fail the armed game frame if the renderer cannot reconstruct its
/// inputs. Substituting a later frame would hide a parity failure.
pub fn refuse(reason: impl Into<String>) {
    if !armed() {
        return;
    }
    let mut s = state();
    if s.job.as_ref().is_some_and(|j| j.pair.is_none())
        && let Some(job) = s.job.take()
    {
        TARGET.store(0, Ordering::Release);
        job.client.finish(Err(unavailable(reason)));
    }
}

pub fn wanted(present: u64) -> bool {
    present != 0 && present != ARMED && waiting_for(present)
}

/// On the render thread, in the D3D display's ordinary guest present
/// hook. Its store holds pre-gamma pixels at the actual display scale.
pub fn reference_observer() -> impl Fn(u64, &gfx::GfxContext, &gfx::surface::SurfaceStore, gfx::surface::SurfaceId, d3d::Lighting) + Send {
    let session = state().session;
    move |present, ctx, store, id, lighting| {
        if !wanted(present) {
            return;
        }
        let sequence = {
            let s = state();
            if !s.installed || s.session != session || TARGET.load(Ordering::Relaxed) != present {
                return;
            }
            s.sequence
        };
        // The legacy E3 display scales shadow maps and sharpens their lookup.
        // The private graph's L3 E3 passes only measure work. A Retail baseline
        // is the policy both renderers can reproduce for this gate.
        if lighting != d3d::Lighting::Retail {
            complete(present, Side::Reference, Err("parity requires Retail display lighting".into()), Some(sequence));
            return;
        }
        let image = crate::d3d_glue::read_rgba(ctx, store, id)
            .map(|(pixels, width, height)| Rgba { width, height, pixels })
            .map_err(|e| e.to_string());
        complete(present, Side::Reference, image, Some(sequence));
    }
}

/// The ordinary backend refused the reference present. Discard its
/// partially completed comparison and surface the failure.
pub fn reference_error(present: u64) {
    if wanted(present) {
        complete(present, Side::Reference, Err("the D3D present failed".to_owned()), None);
    }
}

/// The private draw worker, after rendering the same sealed scene.
pub fn native(present: u64, image: Result<Rgba, String>) {
    complete(present, Side::Native, image, None);
}

fn complete(present: u64, side: Side, image: Result<Rgba, String>, sequence: Option<u64>) {
    complete_with(present, side, image, sequence, compare_pair);
}

fn compare_pair(tick: u32, (reference, native): Images) -> Result<ParityReply, RpcError> {
    compare(&reference, &native, false)
        .map(|(compare, _)| ParityReply { tick: Some(u64::from(tick)), compare })
        .map_err(|e| unavailable(e.to_string()))
}

fn complete_with(
    present: u64,
    side: Side,
    image: Result<Rgba, String>,
    sequence: Option<u64>,
    compare: impl FnOnce(u32, Images) -> Result<ParityReply, RpcError>,
) {
    if !wanted(present) {
        return;
    }
    let (sequence, tick, images) = {
        let mut s = state();
        if TARGET.load(Ordering::Relaxed) != present || sequence.is_some_and(|seq| seq != s.sequence) {
            return;
        }
        let Some(pair) = s.job.as_mut().and_then(|job| job.pair.as_mut()) else { return };
        let Some(images) = pair.push(present, side, image) else { return };
        let tick = pair.tick;
        TARGET.store(0, Ordering::Release);
        (s.sequence, tick, images)
    };

    // Keep the client reachable for interruption or reset while the
    // renderer walks the pixels without holding the state lock.
    let result = images.map_err(unavailable).and_then(|images| compare(tick, images));
    let mut s = state();
    if s.sequence == sequence && let Some(job) = s.job.take() {
        job.client.finish(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> Pair {
        Pair { present: 101, tick: 47, half: None }
    }

    fn image(red: u8) -> Rgba {
        Rgba::filled(2, 1, [red, 0, 0, 255])
    }

    #[test]
    fn parity_pairs_only_one_present_in_either_completion_order() {
        for first in [Side::Native, Side::Reference] {
            let mut frame = pair();
            let (a, b, second) = match first {
                Side::Native => (8, 5, Side::Reference),
                Side::Reference => (5, 8, Side::Native),
            };
            assert!(frame.push(100, second, Ok(image(0))).is_none());
            assert!(frame.push(101, first, Ok(image(a))).is_none());
            assert!(frame.push(102, second, Err("stale".into())).is_none());
            let (reference, native) = frame.push(101, second, Ok(image(b))).unwrap().unwrap();
            assert_eq!((reference.get(0, 0)[0], native.get(0, 0)[0]), (5, 8));
            let result = compare(&reference, &native, false).unwrap().0;
            assert_eq!((result.differing, result.max_channel), (2, 3));
        }
    }

    // RPC tests install a renderer in the process-wide agent registry.
    static RPC_RENDERER: Mutex<()> = Mutex::new(());

    struct Installed;
    impl Installed {
        fn new() -> Self {
            install();
            Self
        }
    }
    impl Drop for Installed {
        fn drop(&mut self) { uninstall(); }
    }

    #[test]
    fn replacement_and_timeline_reset_interrupt_a_comparison_in_progress() {
        use agent::render::{CAPTURE, HOSTED};
        use serde_json::{Value, json};
        use std::sync::mpsc;

        struct Reader;
        impl agent::Reader for Reader {
            fn status(&mut self) -> agent::GameStatus { agent::GameStatus::default() }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> { Ok(Vec::new()) }
        }

        let _serial = RPC_RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let _installed = Installed::new();
        let agent = agent::Agent::new();
        agent::render::add_frame_methods(&agent, &CAPTURE, &HOSTED);
        let (tx, replies) = mpsc::sync_channel(4);
        let conn = agent.connect(tx);
        let request = |id: u32| {
            agent.submit(conn, &json!({
                "jsonrpc":"2.0", "id":id, "method":"render.parity", "params":{"arm":true}
            }).to_string());
            agent.on_tick(&mut Reader);
        };
        let start_frame = || {
            bind(101, 47);
            native(101, Ok(image(8)));
        };
        let reply = || -> Value {
            agent.on_tick(&mut Reader);
            serde_json::from_str(&replies.try_recv().unwrap()).unwrap()
        };

        request(1);
        start_frame();
        complete_with(101, Side::Reference, Ok(image(5)), None, |tick, images| {
            request(2);
            // Reuse the exact same tick and present during the first pixel
            // walk. Only the request sequence distinguishes the two clients.
            start_frame();
            compare_pair(tick, images)
        });
        let replaced = reply();
        assert_eq!(replaced["id"], 1);
        assert_eq!(replaced["error"]["code"], code::INTERRUPTED);
        assert!(replies.try_recv().is_err());

        complete_with(101, Side::Reference, Ok(image(5)), None, |tick, images| {
            reset("test timeline reset");
            compare_pair(tick, images)
        });
        let reset = reply();
        assert_eq!(reset["id"], 2);
        assert_eq!(reset["error"]["code"], code::UNAVAILABLE);
        assert_eq!(reset["error"]["message"], "test timeline reset");
        assert!(replies.try_recv().is_err());

        request(3);
        bind(102, 48);
        reference_error(102);
        let unshown = reply();
        assert_eq!(unshown["id"], 3);
        assert_eq!(unshown["error"]["code"], code::UNAVAILABLE);
        request(4);
        bind(103, 49);
        complete(103, Side::Reference, Ok(image(7)), None);
        native(103, Ok(image(7)));
        let shown = reply();
        assert_eq!(shown["id"], 4);
        assert_eq!(shown["result"]["tick"], 49);
        assert_eq!(shown["result"]["max_channel"], 0);
    }

    #[test]
    fn parity_rpc_compares_the_display_surface_to_a_fresh_gpu_draw() {
        use std::sync::mpsc;
        use agent::render::{CAPTURE, HOSTED};
        use d3d::backend::Backend;
        use d3d::scene_sink::ReplayOp;
        use frame_scene::{SceneBuilder, SceneId, Slot, SlotAt, Stamp};
        use gfx::{Clear, D3DFormat, GfxContext};
        use serde_json::{Value, json};

        struct Reader(u64);
        impl agent::Reader for Reader {
            fn status(&mut self) -> agent::GameStatus {
                agent::GameStatus { tick: Some(self.0), ..Default::default() }
            }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(Vec::new())
            }
        }

        let ctx = match GfxContext::headless() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("skipping GPU parity test: {e}");
                return;
            }
        };
        let _serial = RPC_RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let _installed = Installed::new();
        let agent = agent::Agent::new();
        agent::render::add_frame_methods(&agent, &CAPTURE, &HOSTED);
        let (tx, replies) = mpsc::sync_channel(4);
        let conn = agent.connect(tx);

        let mut display = d3d::GfxBackend::new(crate::d3d_glue::share_context(&ctx));
        display.observe_guest_present(reference_observer());
        let mut private = d3d::GfxBackend::new(ctx);
        let (frames, replayed) = mpsc::channel();
        let mut sink = d3d::SceneSink::new(display, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
            .with_replay(move |frame| {
                bind(frame.present, 40 + frame.present as u32);
                frames.send(frame).unwrap();
            });
        let target = sink.create_surface(D3DFormat::LinA8R8G8B8, 8, 4, false).unwrap();
        for n in 1..=3 {
            let request = json!({"jsonrpc":"2.0","id":n,"method":"render.parity","params":{"arm":true}});
            agent.submit(conn, &request.to_string());
            agent.on_tick(&mut Reader(40 + n));
            assert!(armed());

            let clear = Clear { color: Some([1., 0., 0., 1.]), ..Clear::NONE };
            sink.inner_mut().set_hard_shadows(n == 3);
            sink.clear(target, None, clear).unwrap();
            sink.present(target, 0).unwrap();
            let mut frame = replayed.try_recv().unwrap();
            assert_eq!(frame.present, n);
            if n == 3 {
                assert!(!wanted(n));
                agent.on_tick(&mut Reader(41 + n));
                let reply: Value = serde_json::from_str(&replies.try_recv().unwrap()).unwrap();
                assert_eq!(reply["error"]["code"], code::UNAVAILABLE);
                assert!(reply["error"]["message"].as_str().unwrap().contains("Retail display lighting"));
                continue;
            }
            assert!(wanted(n));
            assert!(replies.try_recv().is_err(), "the private draw still owes its pixels");
            if n == 2 {
                // An intentionally damaged private command demonstrates the
                // reference was actually read from the separate GPU target.
                let ReplayOp::Clear { clear, .. } = &mut frame.ops.first_mut().unwrap().1 else {
                    panic!("expected the synthetic clear");
                };
                clear.color = Some([0., 0., 1., 1.]);
            }
            let mut builder = SceneBuilder::default();
            builder.begin(SceneId { frame: n, tick: 40 + n as u32, ..Default::default() });
            let scene = builder.seal(Stamp::default()).unwrap();
            let drawn = native_render::frame_graph::draw_private(
                &mut private, &scene, &frame, 1, d3d::Lighting::Retail,
            ).unwrap();
            native(n, Ok(Rgba { width: drawn.size[0], height: drawn.size[1], pixels: drawn.rgba }));
            agent.on_tick(&mut Reader(41 + n));
            let reply: Value = serde_json::from_str(&replies.try_recv().unwrap()).unwrap();
            let result = &reply["result"];
            assert_eq!(result["tick"], 40 + n);
            assert_eq!(result["size"], json!([8, 4]));
            assert_eq!(result["max_channel"], if n == 1 { 0 } else { 255 });
            assert_eq!(result["differing"], if n == 1 { 0 } else { 32 });
        }
    }

    #[test]
    fn parity_reference_pins_an_inherited_target_before_the_original_swap() {
        use d3d::{Backend, GfxBackend, SceneSink, ThreadedBackend};
        use engine::memory::{FlatMemory, GuestMemory};
        use frame_scene::{Slot, SlotAt, mailbox::Mode};
        use gfx::{Clear, D3DFormat, GfxContext};
        use native_render::camera::RENDER_CAMERA;
        use render_bridge::scene_step;
        use serde_json::{Value, json};
        use std::{sync::mpsc, time::Duration};

        let ctx = match GfxContext::headless() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("skipping GPU parity input test: {e}");
                return;
            }
        };
        let _serial = RPC_RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let private = GfxBackend::new(crate::d3d_glue::share_context(&ctx));
        let service = crate::frame_owner::Service::new(private, Mode::Locked).unwrap();
        let mut display = GfxBackend::new(ctx);
        display.observe_guest_present(service.reference_observer());

        // Bound parity frames use the original renderer. The owner must
        // still receive their sealed pair to snapshot inherited pixels.
        let owner = crate::frame_cutover::owner(Box::new(|| panic!("expected a reference Swap")));
        let display = ThreadedBackend::new(display).unwrap().with_frame_owner(owner);
        let mut sink = SceneSink::new(display, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
            .with_replay(crate::frame_cutover::record(&service))
            .with_guest_reference_request(armed);
        let format = D3DFormat::LinA8R8G8B8;
        let src = sink.create_surface(format, 8, 4, false).unwrap();
        let dst = sink.create_surface(format, 8, 4, false).unwrap();
        // The source has real GPU contents, unknown to the scene's CPU
        // seed tracker. The private draw cannot synthesize them itself.
        sink.inner_mut().with_surface(src, |gpu, id| {
            gpu.clear(id, None, Clear { color: Some([0., 1., 1., 1.]), ..Clear::NONE })
        }).unwrap();
        let mut mem = FlatMemory::new(RENDER_CAMERA, 0x48);
        for (offset, value) in [(0xc, 1.), (0x20, 1.), (0x28, 1.), (0x40, 0.1), (0x44, 20.)] {
            mem.set_f32(RENDER_CAMERA + offset, value).unwrap();
        }
        mem.set_u16(RENDER_CAMERA + 0x34, 4).unwrap();
        mem.set_u16(RENDER_CAMERA + 0x36, 8).unwrap();

        struct Reader;
        impl agent::Reader for Reader {
            fn status(&mut self) -> agent::GameStatus { agent::GameStatus::default() }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> { Ok(Vec::new()) }
        }
        let agent = agent::Agent::new();
        agent::render::add_frame_methods(&agent, &agent::render::CAPTURE, &agent::render::HOSTED);
        let (tx, rx) = mpsc::sync_channel(1);
        let conn = agent.connect(tx);
        let request = json!({"jsonrpc":"2.0","id":1,"method":"render.parity","params":{"arm":true}});
        agent.submit(conn, &request.to_string());
        agent.on_tick(&mut Reader);
        assert!(armed());

        scene_step::begin(&mem);
        assert!(scene_step::open_view(&mem).is_some());
        assert!(sink.copy_surface(src, dst).unwrap());
        sink.present(dst, 0).unwrap();
        sink.inner_mut().finish().unwrap();
        let (_, replay) = crate::frame_owner::pair_for_present(1).expect("sealed game view");
        assert_eq!(replay.frozen_target_seeds().unwrap().len(), 1);
        // Drive the existing agent request while the private GPU worker
        // finishes. Each pump is a host tick, independent of the scene.
        let reply = (0..200).find_map(|_| {
            agent.on_tick(&mut Reader);
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(response) => Some(response),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(e) => panic!("parity reply channel: {e}"),
            }
        }).expect("parity reply for the inherited input");
        let json: Value = serde_json::from_str(&reply).unwrap();
        let result = &json["result"];
        assert_eq!(result["size"], json!([8, 4]), "{json}");
        assert_eq!(result["differing"], 0, "{json}");
        assert_eq!(result["max_channel"], 0, "{json}");

        // Both explicit lighting requests can still redraw the same
        // source after the guest destroys every original surface.
        sink.destroy_surface(src).unwrap();
        sink.destroy_surface(dst).unwrap();
        sink.inner_mut().finish().unwrap();
        check_lighting_overrides();
    }

    fn check_lighting_overrides() {
        use agent::render::{CAPTURE, private_graph};
        use agent_proto::{CaptureLighting, FrameCaptureParams};
        use std::time::{Duration, Instant};

        let capture = |lighting, tick| {
            let params = FrameCaptureParams { lighting: Some(lighting), tick, pass: None };
            let seq = CAPTURE.arm(&params, &std::env::temp_dir()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            let reply = loop {
                if let Some(done) = CAPTURE.finish(seq) {
                    break done.expect("private capture of the sealed inputs");
                }
                assert!(Instant::now() < deadline, "private capture timed out");
                std::thread::sleep(Duration::from_millis(5));
            };
            let path = std::path::Path::new(&reply.path);
            let image = agent::compare::read_png(path);
            std::fs::remove_file(path).unwrap();
            (reply, image.unwrap())
        };
        let (retail, a) = capture(CaptureLighting::Retail, None);
        let (e3, b) = capture(CaptureLighting::E3, Some(retail.tick));
        assert_eq!(retail.present, 1);
        assert_eq!((retail.tick, retail.present, &retail.scene_hash), (e3.tick, e3.present, &e3.scene_hash));
        assert!(e3.scene_hash.is_some());
        assert_ne!(retail.path, e3.path);
        assert_eq!(retail.lighting, Some(CaptureLighting::Retail));
        assert_eq!(e3.lighting, Some(CaptureLighting::E3));
        assert_eq!(a.pixels, [0, 255, 255, 255].repeat(32));
        assert_eq!(compare(&a, &b, false).unwrap().0.max_channel, 0);

        let graph = private_graph::last_stats().expect("completed private graph");
        assert_eq!(graph.present, e3.present);
        assert_eq!(Some(graph.scene_hash), e3.scene_hash);
        assert_eq!(graph.lighting, CaptureLighting::E3);
        assert!(graph.e3.unwrap().passes.iter().all(|p| p.draws == 0 && p.gpu_ns.is_none()));
    }

    #[test]
    fn parity_refuses_missing_or_duplicate_images() {
        let mut frame = pair();
        assert!(frame.push(101, Side::Native, Err("incomplete targets".into())).unwrap()
            .unwrap_err().contains("incomplete targets"));
        assert!(frame.push(101, Side::Reference, Ok(image(9))).is_none());
        assert!(frame.push(101, Side::Reference, Ok(image(9))).unwrap()
            .unwrap_err().contains("duplicate"));
    }
}
