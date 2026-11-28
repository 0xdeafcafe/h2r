//! `native-flare-draw`: the flare draw pass's glows made in Rust
//! (native-only plan, `nor-8g`; docs/graphics/d3d-visibility.md, "The
//! glow in Rust"). On with the native renderer.
//!
//! | Guest | Cut |
//! |---|---|
//! | `0x00046ea0(eax datum, ecx &position)`, called for each late-list entry at `0x0002e1c5` | the glow from [`scene_extract::flares::glow`], drawn by [`Host::draw`] (`native_render::flare_draw_gpu`) at this point in the frame, or nothing where the guest culls it |
//!
//! The draw pass `0x0002df00` is the last call of the post `0x00013870`
//! ([`POST_CALLS`]), so the glows are the view's last draws before the
//! overlay. The pass's own setup and its walker call (the reflections)
//! stay the guest's; only the late loop's calls are cut. The hook is an
//! `implement` at the glow's entry; a call from anywhere but the late
//! loop runs the guest body.
//!
//! Every late-loop call is the cut's: the guest body never runs for it. A
//! glow the cut cannot plan or place draws nothing and is
//! `scene_dropped_flare_glow_<cause>` (`host`, `unread`, `table`,
//! `placed`), as is a glow placed but not drawn ([`dropped`]). The body's
//! frame reads ([`mark`], [`guest_glow`], [`same`]) stay as the test
//! oracle of [`plan`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::flare_draw_gpu::GlowDraw;
use scene_extract::GuestRead;
use scene_extract::flares::{self as fl, GlowConstants, LateGlow, Lens};

pub const ID: &str = "native-flare-draw";

/// The late loop's return from the glow (`0x0002e1c5`'s call).
pub const LATE_RETURN: u32 = 0x0002_e1ca;

/// The post `0x00013870`'s draw calls, in order: (call site, callee).
/// The flare draw pass is the last.
pub const POST_CALLS: [(u32, u32); 3] = [
    (0x0001_39b5, 0x0002_14f0), // the colour glow
    (0x0001_3b52, 0x0002_d140), // the screen effects
    (0x0001_3b7e, FLARE_DRAW),  // the flare draw pass
];
pub const FLARE_DRAW: u32 = 0x0002_df00;

/// The glow's frame below its entry ESP (`sub esp, 0x44`, four pushes):
/// where its locals sit after it returns.
pub const FRAME: u32 = 0x54;
/// Locals of that frame: the ring loop's count (`[esp+0x20]`, 16 down to
/// 0 once every ring is drawn), the composite weight (`[esp+0x10]`) and
/// the rectangle's corners.
pub const RING_COUNT: u32 = 0x20;
pub const WEIGHT: u32 = 0x10;
pub const CORNERS: [u32; 4] = [0x34, 0x3c, 0x38, 0x40];
/// Written to the ring count before a checked body: a culled glow leaves it.
const UNDRAWN: u32 = u32::MAX;

/// What draws the glows: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `draw`'s passes at this point in the frame. `false`: not
    /// placed, nothing recorded.
    fn draw(&self, mem: &mut dyn GuestMemory, draw: GlowDraw) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call runs the guest body.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The glow call's arm and its counts.
pub struct Arm {
    calls: AtomicU64,
    cut: AtomicU64,
    dropped: Mutex<BTreeMap<&'static str, u64>>,
}

impl Arm {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            cut: AtomicU64::new(0),
            dropped: Mutex::new(BTreeMap::new()),
        }
    }

    fn counters(&self) -> Vec<(String, u64)> {
        let mut out = vec![
            ("flare_glow_calls".to_owned(), self.calls.load(Ordering::Relaxed)),
            ("flare_glow_cut".to_owned(), self.cut.load(Ordering::Relaxed)),
        ];
        let named = |map: &Mutex<BTreeMap<&'static str, u64>>, prefix: &str| -> Vec<(String, u64)> {
            map.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(c, n)| (format!("{prefix}{c}"), *n)).collect()
        };
        out.extend(named(&self.dropped, "scene_dropped_flare_glow_"));
        out
    }
}

pub static GLOW: Arm = Arm::new();

/// A placed glow that did not draw for `cause` (the host's, at record
/// time): the frame has no glow there.
pub fn dropped(cause: &'static str) {
    *GLOW.dropped.lock().unwrap_or_else(|e| e.into_inner()).entry(cause).or_default() += 1;
    timeline::native::Stage::Flares.count(timeline::native::Metric::SceneDropped, 1);
}

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    GLOW.counters()
}

/// The guest's memory as `scene_extract` reads it.
struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The glow `0x00046ea0` draws for `datum` at the position `at` points
/// to; `None` where it draws nothing. `Err` names why the glow is dropped.
pub fn plan(mem: &dyn GuestMemory, datum: u32, at: u32) -> Result<Option<GlowDraw>, &'static str> {
    let ro = Ro(mem);
    let lens = Lens::read(&ro).ok_or("unread")?;
    let k = GlowConstants::read(&ro).ok_or("unread")?;
    let f = |a: u32| ro.read_f32(a).ok_or("unread");
    let late = LateGlow { datum, position: [f(at)?, f(at.wrapping_add(4))?, f(at.wrapping_add(8))?] };
    let glow = fl::glow(&lens, &k, &ro, &late).map_err(|u| match u {
        fl::Unmade::Unread => "unread",
        fl::Unmade::Table => "table",
    })?;
    Ok(glow.map(|glow| GlowDraw { glow, rings: glow.rings(&k), source: k.source }))
}

/// Marks the frame of a body about to run from `esp`, so a culled glow
/// reads as undrawn ([`guest_glow`]).
pub fn mark(mem: &mut dyn GuestMemory, esp: u32) -> Result<(), &'static str> {
    mem.set_u32(esp.wrapping_sub(FRAME).wrapping_add(RING_COUNT), UNDRAWN).map_err(|_| "unwritable")
}

/// What the body that ran from `esp` drew: its rectangle and weight, or
/// `None` where it drew no rings.
pub fn guest_glow(mem: &dyn GuestMemory, esp: u32) -> Result<Option<([f32; 4], f32)>, &'static str> {
    let b = esp.wrapping_sub(FRAME);
    let r = |o: u32| mem.u32(b.wrapping_add(o)).map_err(|_| "unread");
    if r(RING_COUNT)? != 0 {
        return Ok(None);
    }
    let mut rect = [0.0; 4];
    for (v, o) in rect.iter_mut().zip(CORNERS) {
        *v = f32::from_bits(r(o)?);
    }
    Ok(Some((rect, f32::from_bits(r(WEIGHT)?))))
}

/// Whether the cut's glow is the body's, bit for bit.
pub fn same(cut: Option<&GlowDraw>, guest: Option<([f32; 4], f32)>) -> bool {
    match (cut, guest) {
        (None, None) => true,
        (Some(c), Some((rect, weight))) => {
            c.glow.rect.map(f32::to_bits) == rect.map(f32::to_bits) && c.glow.weight.to_bits() == weight.to_bits()
        }
        _ => false,
    }
}

/// Whether the native flare draw is placed: with the native renderer on,
/// unless `native-flare-draw=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    switches.feature(ID, true)
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn glow_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let esp = cpu.esp;
    let caller = rt.memory().u32(esp).ok();
    if caller != Some(LATE_RETURN) {
        return original(cpu, ctx, addr);
    }
    let arm = &GLOW;
    arm.calls.fetch_add(1, Ordering::Relaxed);
    let drawn = host().ok_or("host").and_then(|host| {
        let mut mem = rt.memory();
        match plan(&*mem, cpu.eax, cpu.ecx)? {
            Some(draw) if !host.draw(&mut *mem, draw) => Err("placed"),
            _ => Ok(()),
        }
    });
    match drawn {
        Ok(()) => {
            arm.cut.fetch_add(1, Ordering::Relaxed);
        }
        Err(cause) => dropped(cause),
    }
    ctx.memory().ret(cpu, 0)
}

pub struct NativeFlareDraw;

impl Feature for NativeFlareDraw {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The flare draw pass's glows made in Rust").doc("docs/graphics/d3d-visibility.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The glow keeps no state across frames: each call is drawn from
        // the guest's memory of that call.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&engine::points::FLARE_GLOW, "native_flare_glow", glow_call).context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use frame_scene::ViewSlot;

    /// The draw pass is the post's last call, and the post is the view's
    /// last slot: the glows draw over everything the view draws.
    #[test]
    fn the_flare_draw_is_the_last_post_pass() {
        assert_eq!(POST_CALLS.last().map(|c| c.1), Some(FLARE_DRAW));
        assert!(POST_CALLS.windows(2).all(|w| w[0].0 < w[1].0), "in call-site order");
        let last = [
            ViewSlot::OpaqueBase,
            ViewSlot::Late,
            ViewSlot::Transparents,
            ViewSlot::FirstPerson,
            ViewSlot::FlareQuery,
            ViewSlot::Layer17,
            ViewSlot::Post,
        ]
        .into_iter()
        .max();
        assert_eq!(last, Some(ViewSlot::Post));
        assert_eq!(ViewSlot::Post.number(), Some(13));
        assert!(ViewSlot::Post < ViewSlot::Hud, "before the overlay");
    }

    const TAGS_TABLE: u32 = 0x0060_0000;
    const TAG: u32 = 0x0060_1000;
    const POSITION: u32 = 0x0060_2000;

    /// A 640x480 view with an identity camera at the origin looking down
    /// +z, and a glow ten units ahead (lens tag 7, radius 0.01).
    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        for (a, v) in [
            (fl::k::ONE, 1.0),
            (fl::k::HALF, 0.5),
            (fl::k::TWO, 2.0),
            (fl::k::EPSILON, 1e-4),
            (fl::WORLD_SCALE, 1.0),
            (fl::FAR, 1000.0),
            (fl::glow_k::COS, 0.5),
            (fl::glow_k::COS_SCALE, 2.0),
            (fl::glow_k::HALF, 8.0),
            (fl::glow_k::RING[0], 1.0),
            (fl::glow_k::RING[1], 2.0),
            (fl::glow_k::RING_START, 1.0),
            (fl::glow_k::EPSILON, 1e-4),
            (fl::glow_k::SOURCE[1], 64.0),
            (fl::CAMERA_VIEW_FORWARD + 8, 1.0),
        ] {
            f(&mut m, a, v);
        }
        for (i, v) in [(0, 1.0), (4, 1.0), (8, 1.0)] {
            f(&mut m, fl::VIEW + 4 * i, v);
        }
        for (i, v) in [(0, 1.0), (5, 1.0), (10, 1.0), (11, 1.0)] {
            f(&mut m, fl::PROJECTION + 4 * i, v);
        }
        m.set_u32(fl::VIEWPORT, 0).unwrap();
        m.set_u32(fl::VIEWPORT + 4, 480 | 640 << 16).unwrap();
        m.set_u32(fl::TAGS, TAGS_TABLE).unwrap();
        m.set_u32(TAGS_TABLE + 16 * 7 + 8, TAG).unwrap();
        f(&mut m, TAG + 0x10, 0.01);
        f(&mut m, POSITION + 8, 10.0);
        m
    }

    struct Refusing(Mutex<Vec<GlowDraw>>);

    /// Places the pass; at record time it does not draw, as the glue's
    /// pass refuses a target it cannot copy.
    impl Host for Refusing {
        fn draw(&self, _: &mut dyn GuestMemory, draw: GlowDraw) -> bool {
            self.0.lock().unwrap().push(draw);
            dropped("colour");
            true
        }
    }

    fn count(name: &str) -> u64 {
        counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
    }

    /// A glow the cut placed but the GPU did not draw is counted under
    /// its cause; the call is still the cut's, so the guest body does not
    /// draw it either.
    #[test]
    fn an_undrawn_glow_counts_scene_dropped_flare_glow() {
        let mut m = mem();
        let host = Refusing(Mutex::new(Vec::new()));
        let before = count("scene_dropped_flare_glow_colour");
        let draw = plan(&m, 7, POSITION).unwrap().expect("ahead: drawn");
        assert_eq!(draw.glow.rect, [312.0, 232.0, 328.0, 248.0]);
        assert_eq!(draw.rings[1].rect, [311.0, 231.0, 329.0, 249.0]);
        assert_eq!(draw.source, [0.0, 64.0]);
        assert!(host.draw(&mut m, draw));
        assert!(count("scene_dropped_flare_glow_colour") > before);
        assert_eq!(host.0.lock().unwrap().len(), 1);
    }

    /// The oracle reads the body's rectangle and weight from its frame
    /// below the entry ESP; a culled body leaves the mark.
    #[test]
    fn a_checked_glow_reads_the_bodys_frame() {
        let mut m = mem();
        let esp = 0x0050_0000;
        let draw = plan(&m, 7, POSITION).unwrap();
        mark(&mut m, esp).unwrap();
        assert_eq!(guest_glow(&m, esp), Ok(None), "culled: the mark stays");
        assert!(!same(draw.as_ref(), None));
        let b = esp - FRAME;
        for (o, v) in CORNERS.iter().zip([312.0f32, 232.0, 328.0, 248.0]) {
            m.set_u32(b + o, v.to_bits()).unwrap();
        }
        m.set_u32(b + WEIGHT, 1.0f32.to_bits()).unwrap();
        m.set_u32(b + RING_COUNT, 0).unwrap();
        let guest = guest_glow(&m, esp).unwrap();
        assert_eq!(guest, Some(([312.0, 232.0, 328.0, 248.0], 1.0)));
        assert!(same(draw.as_ref(), guest));
        m.set_u32(b + WEIGHT, 0.5f32.to_bits()).unwrap();
        assert!(!same(draw.as_ref(), guest_glow(&m, esp).unwrap()), "another weight differs");
    }
}
