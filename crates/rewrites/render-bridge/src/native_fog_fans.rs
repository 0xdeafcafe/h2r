//! `native-fog-fans`: the second fog's screen fans (`0x00025dc0`) drawn
//! by the native renderer (docs/graphics/retire-d3d-fog-fans.md). On with
//! the native renderer.
//!
//! Each fan's setup, which the fan `0x00034420` calls through EDI (return
//! `0x0003452e`), is cut ([`turn`]) when its frame holds `0x00025dc0`'s
//! fan call and the fan's corners callback:
//!
//! | Setup (corners, fan's return) | Here |
//! |---|---|
//! | layers `0x00026880` (`0x00026ca0`, `0x00025e04`) | `engine::fog_fans::layers` into target `0x13`, the fog patch's bitmap at stages 0..3 |
//! | layers' composite `0x00026e50` (`0x00027520`, `0x00025e2d`) | the set branch, `engine::fog_fans::layers_composite`; the clear branch's weather plate `0x0003a5c0` panics |
//! | composite `0x000276d0` (`0x00027960`, `0x00025e2d`) | `engine::fog_fans::composite` |
//!
//! [`Host::fan`] (`native_render::fog_fans`) draws the fan at this point in
//! the frame on the frame graph's surfaces by role (the targets' surface
//! headers); the setup makes the writes a later read sees (the depth
//! word) and returns AL 0, so no fan reaches D3D. The guest makes the path
//! it makes with the gate shut, but for the fan's draw-state reset
//! `0x00016b10` and rectangle. The setups' bodies never run.
//!
//! What the cut cannot draw is `unimplemented!("fog fans: <cause>")`: no
//! host (`host`), a pass the device does not place (`placed`), the
//! host's (a bitmap not loaded, a stage or copy it cannot make), a view
//! target other than 0 (`composite into target <n>`), the weather plate
//! (`layers composite weather plate (0x0003a5c0)`), a fan not finite
//! (`fan_unfinite`), an operand the SSE or x87 model refuses (`simd`,
//! `x87`), an unreadable input (`unread`), and a fan placed but not
//! recorded ([`unported`], the GPU's cause).

use std::sync::{Arc, Mutex};

use engine::fog_fans::{self as ff, LayersDest};
use engine::memory::GuestMemory;
use frame_types::TargetRole;
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::fog_fans::{Coords, FanDraw, FanRaster, FanStage};

pub const ID: &str = "native-fog-fans";

/// The rasterizer data element `[0x00485a80]`, and the bitmap datum the
/// layers' composite binds at stage 2 (`[[0x00485a80] + 0x14]`, image 0).
pub const RASTERIZER: u32 = 0x0048_5a80;
pub const RASTERIZER_BITMAP: u32 = 0x14;

/// What a stage samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The surface at a header: a game target's (by role) or the view's.
    Surface(u32),
    /// Image `image` of the bitmap `datum`.
    Bitmap { datum: u32, image: u32 },
}

/// One fan to place: the draw, the surface header it draws into, and what
/// each of its stages samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub draw: FanDraw,
    pub dest: u32,
    pub sources: [Option<Source>; 4],
}

/// What draws the fans: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `fan` at this point in the frame. `Err` names why it was not
    /// placed (nothing recorded).
    fn fan(&self, mem: &mut dyn GuestMemory, fan: &Fan) -> Result<(), &'static str>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot).
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the cut is placed: with the native renderer.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// A fan the native renderer cannot draw, by cause.
pub fn unported(cause: &str) -> ! {
    unimplemented!("fog fans: {cause}")
}

fn cause(e: engine::light_constants::Error) -> &'static str {
    match e {
        engine::light_constants::Error::Simd => "simd",
        engine::light_constants::Error::X87(_) => "x87",
        _ => "unread",
    }
}

/// `0x00025dc0`'s entry with `ecx` and `edx` under the call's MXCSR: a
/// gate the SSE model refuses or that does not read panics; the fans are
/// cut at their setups ([`turn`]).
pub fn before_fans(mem: &dyn GuestMemory, mxcsr: u32, ecx: u32, edx: u32) {
    if let Err(e) = ff::fans(mem, mxcsr, ecx as u8, edx as u8) {
        unported(cause(e));
    }
}

/// A game target's surface header, by role.
fn game(n: u8) -> u32 {
    crate::native_targets::surface(TargetRole::Game(n)).unwrap_or_else(|| unported("target"))
}

/// The surface header of the view's target `n`: target 0's colour is the
/// view's own (`0x00014bc0`'s first case).
fn view_surface(mem: &dyn GuestMemory, n: i16) -> u32 {
    if n != 0 {
        unported(&format!("composite into target {n}"));
    }
    mem.u32(crate::frame_tail::TARGET0_COLOUR).unwrap_or_else(|_| unported("unread"))
}

/// What a setup call does.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// Not `0x00025dc0`'s fan: run the setup.
    Guest,
    /// The fan is placed and the setup's writes made: return AL 0.
    Return,
}

/// Which of the three setups a cut is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setup {
    Layers,
    LayersComposite,
    Composite,
}

impl Setup {
    pub fn of(entry: u32) -> Option<Setup> {
        Some(match entry {
            ff::setup::LAYERS => Setup::Layers,
            ff::setup::LAYERS_COMPOSITE => Setup::LayersComposite,
            ff::setup::COMPOSITE => Setup::Composite,
            _ => return None,
        })
    }

    /// The fan's return and corners callback that make a call this
    /// setup's of `0x00025dc0`.
    fn frame(self) -> (u32, u32) {
        match self {
            Setup::Layers => (ff::LAYERS_RETURN, ff::setup::LAYERS_CORNERS),
            Setup::LayersComposite => (ff::SECOND_RETURN, ff::setup::LAYERS_COMPOSITE_CORNERS),
            Setup::Composite => (ff::SECOND_RETURN, ff::setup::COMPOSITE_CORNERS),
        }
    }
}

fn floats<const N: usize>(v: [[u32; 4]; N]) -> [[f32; 4]; N] {
    v.map(|p| p.map(f32::from_bits))
}

fn refused(r: native_render::fog_fans::Refusal) -> ! {
    unported(r.cause())
}

const PIXELS: Option<FanStage> = Some(FanStage { coords: Coords::Pixels, wrap: false });

/// The fan of `setup` whose rectangle is at `rect_at`; `mem` takes the
/// setup's and its callback's writes.
pub fn fan(mem: &mut dyn GuestMemory, setup: Setup, rect_at: u32, mxcsr: u32, x87_control: u16) -> Fan {
    match setup {
        Setup::Composite => {
            let c = ff::composite(&*mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
            let draw = FanDraw::composite(c.combiner, floats(c.positions), floats(c.coordinates)).unwrap_or_else(|r| refused(r));
            if ff::composite_writes(mem).is_err() {
                unported("depth_write");
            }
            Fan { draw, dest: view_surface(&*mem, c.target), sources: [Some(Source::Surface(game(ff::COMPOSITE_SOURCE))), None, None, None] }
        }
        Setup::Layers => {
            let l = ff::layers(mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
            let stage = Some(FanStage { coords: Coords::Unit, wrap: true });
            let draw = FanDraw::from_fan(l.combiner, floats(l.positions), l.coordinates.map(floats), [stage; 4], FanRaster::Opaque)
                .unwrap_or_else(|r| refused(r));
            let sources = l.images.map(|image| Some(Source::Bitmap { datum: l.bitmap, image: u32::from(image) }));
            Fan { draw, dest: game(ff::LAYERS_TARGET), sources }
        }
        Setup::LayersComposite => {
            if ff::weather_plate(&*mem).unwrap_or_else(|_| unported("unread")) {
                unported("layers composite weather plate (0x0003a5c0)");
            }
            let c = ff::layers_composite(mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
            let bitmap = mem
                .u32(RASTERIZER)
                .and_then(|e| mem.u32(e.wrapping_add(RASTERIZER_BITMAP)))
                .unwrap_or_else(|_| unported("unread"));
            let unit = Some(FanStage { coords: Coords::Unit, wrap: false });
            let raster = if c.blue_only() { FanRaster::BlueOnly } else { FanRaster::Constant };
            let draw = FanDraw::from_fan(c.combiner, floats(c.positions), c.coordinates.map(floats), [PIXELS, None, unit, PIXELS], raster)
                .unwrap_or_else(|r| refused(r));
            let dest = match c.dest {
                LayersDest::View(n) => view_surface(&*mem, n),
                LayersDest::Target3 => game(ff::COMPOSITE_SOURCE),
            };
            let sources = [
                Some(Source::Surface(game(ff::COMPOSITE_SOURCE))),
                None,
                Some(Source::Bitmap { datum: bitmap, image: 0 }),
                Some(Source::Surface(game(ff::LAYERS_TARGET))),
            ];
            Fan { draw, dest, sources }
        }
    }
}

/// The setup `setup`'s call at `esp` under the call's MXCSR and x87
/// control word, `host` the one drawing.
pub fn turn(mem: &mut dyn GuestMemory, setup: Setup, esp: u32, mxcsr: u32, x87_control: u16, host: Option<&dyn Host>) -> Turn {
    let (fan_return, corners) = setup.frame();
    let Some(rect_at) = ff::fan_call(&*mem, esp, fan_return, corners) else {
        return Turn::Guest;
    };
    let fan = fan(mem, setup, rect_at, mxcsr, x87_control);
    let host = host.unwrap_or_else(|| unported("host"));
    if let Err(why) = host.fan(mem, &fan) {
        unported(why);
    }
    Turn::Return
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let Some(setup) = Setup::of(addr) else {
        return original(cpu, ctx, addr);
    };
    let rt = Arc::clone(ctx.runtime());
    let turn = {
        let mut mem = rt.memory();
        turn(&mut *mem, setup, cpu.esp, cpu.mxcsr, cpu.x87.control, host().as_deref())
    };
    match turn {
        Turn::Guest => original(cpu, ctx, addr),
        Turn::Return => {
            cpu.eax &= !0xff;
            rt.memory().ret(cpu, 4)
        }
    }
}

pub struct NativeFogFans;

impl Feature for NativeFogFans {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The second fog's screen fans drawn by the native renderer").doc("docs/graphics/retire-d3d-fog-fans.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Each call is drawn from the guest's memory of that call.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&engine::points::FOG_FANS, "native_fog_fans", |o| {
                before_fans(&*o.guest(), o.cpu.mxcsr, o.cpu.ecx, o.cpu.edx)
            })
            .context_only();
        for (point, name) in [
            (&engine::points::FOG_LAYERS_SETUP, "native_fog_fans_layers"),
            (&engine::points::FOG_LAYERS_COMPOSITE_SETUP, "native_fog_fans_layers_composite"),
            (&engine::points::FOG_COMPOSITE_SETUP, "native_fog_fans_composite"),
        ] {
            b.raw().implement(point, name, setup_call).context_only();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::fog_fans::{composite_global as cg, global, layers_global as lg};
    use engine::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;
    const CONTROL: u16 = 0x027f;
    const ESP: u32 = 0x0050_0000;
    const RECT: u32 = 0x0050_1000;
    const VIEW: u32 = 0x0050_2000;

    fn mem(amount: f32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(global::DATUM, 0xe1a3_0012).unwrap();
        m.set_f32(global::AMOUNT, amount).unwrap();
        m.set_f32(global::ZERO, 0.0).unwrap();
        m
    }

    /// A setup call of `setup` from the fan of `0x00025dc0` over 640x480
    /// into target 0, with a fog patch whose bitmap is `0xabcd0001`.
    fn setup_mem(setup: Setup) -> FlatMemory {
        let mut m = mem(0.5);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, engine::screen_grid::constant::ONE, 1.0);
        f(&mut m, engine::screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, engine::screen_grid::constant::NEAR, 0.0625);
        f(&mut m, engine::screen_grid::constant::FAR, 1024.0);
        f(&mut m, cg::SCALE, 255.0);
        f(&mut m, cg::CLIP_FAR, 1024.0);
        f(&mut m, cg::COLOUR, 1.0);
        f(&mut m, lg::TWO, 2.0);
        f(&mut m, lg::SPAN_FAR, 1024.0);
        f(&mut m, lg::SPAN_MIN, 1.0);
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, RECT + 4 * i as u32, v);
        }
        m.set_u32(lg::TAG_TABLE, 0x0050_3000).unwrap();
        m.set_u32(0x0050_3000 + (0x12 << 4) + 8, 0x0050_4000).unwrap();
        m.set_u32(0x0050_4000 + lg::BITMAP_DATUM, 0xabcd_0001).unwrap();
        m.set_u32(RASTERIZER, 0x0050_5000).unwrap();
        m.set_u32(0x0050_5000 + RASTERIZER_BITMAP, 0xdcba_0002).unwrap();
        m.set_u8(lg::BRANCH, 1).unwrap();
        m.set_u8(global::FLAG_DL, 1).unwrap();
        m.set_u32(crate::frame_tail::TARGET0_COLOUR, VIEW).unwrap();
        let (fan_return, corners) = setup.frame();
        m.set_u32(ESP, ff::SETUP_RETURN).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME, fan_return).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME + 4, RECT).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME + 8, corners).unwrap();
        m
    }

    #[derive(Default)]
    struct Drawn(Mutex<Vec<Fan>>, Option<&'static str>);

    impl Host for Drawn {
        fn fan(&self, _: &mut dyn GuestMemory, fan: &Fan) -> Result<(), &'static str> {
            self.0.lock().unwrap().push(fan.clone());
            self.1.map_or(Ok(()), Err)
        }
    }

    fn drawn(setup: Setup, m: &mut FlatMemory) -> Fan {
        let host = Drawn::default();
        assert_eq!(turn(m, setup, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        let mut fans = host.0.lock().unwrap().clone();
        assert_eq!(fans.len(), 1);
        fans.remove(0)
    }

    #[test]
    fn every_gate_and_flag_runs_to_the_setups() {
        for (cl, dl) in [(1, 1), (1, 0), (0, 1), (0, 0)] {
            before_fans(&mem(0.5), MXCSR, cl, dl);
            before_fans(&mem(0.0), MXCSR, cl, dl);
        }
    }

    #[test]
    #[should_panic(expected = "fog fans: unread")]
    fn an_unread_gate_panics() {
        let m = FlatMemory::new(0x0010_0000, 0x10);
        before_fans(&m, MXCSR, 1, 0);
    }

    #[test]
    fn the_setups_are_named_by_their_entries() {
        assert_eq!(Setup::of(0x0002_6880), Some(Setup::Layers));
        assert_eq!(Setup::of(0x0002_6e50), Some(Setup::LayersComposite));
        assert_eq!(Setup::of(0x0002_76d0), Some(Setup::Composite));
        assert_eq!(Setup::of(0x0002_6520), None);
        assert_eq!(engine::points::FOG_LAYERS_SETUP.base, ff::setup::LAYERS);
        assert_eq!(engine::points::FOG_LAYERS_COMPOSITE_SETUP.base, ff::setup::LAYERS_COMPOSITE);
        assert_eq!(engine::points::FOG_COMPOSITE_SETUP.base, ff::setup::COMPOSITE);
    }

    #[test]
    fn the_composite_is_cut_into_the_view_sampling_target_3() {
        let mut m = setup_mem(Setup::Composite);
        let fan = drawn(Setup::Composite, &mut m);
        assert_eq!(fan.dest, VIEW);
        assert_eq!(fan.sources[0], Some(Source::Surface(game(3))));
        assert_eq!(crate::native_targets::role(game(3), 0), Some(TargetRole::Game(3)));
        assert_eq!(fan.draw.strip[3].0, [640.0, 480.0]);
        assert_eq!(fan.draw.strip[3].1[0], [640.0, 480.0, 0.0, 1.0]);
        assert_eq!(fan.draw.combiner[43], 0x00ff_0000);
        assert_eq!(fan.draw.raster, FanRaster::Constant);
        assert_eq!(m.u32(cg::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn the_layers_are_cut_into_target_0x13_from_the_patchs_bitmap() {
        let mut m = setup_mem(Setup::Layers);
        for n in 0..4u32 {
            m.set_u16(lg::LAYERS + lg::LAYER_STRIDE * n, 7 + n as u16).unwrap();
        }
        let fan = drawn(Setup::Layers, &mut m);
        assert_eq!(fan.dest, game(0x13));
        assert_eq!(crate::native_targets::role(fan.dest, 0), Some(TargetRole::Game(0x13)));
        assert_eq!(fan.sources[2], Some(Source::Bitmap { datum: 0xabcd_0001, image: 9 }));
        assert_eq!(fan.draw.raster, FanRaster::Opaque);
        assert!(fan.draw.stages.iter().all(|s| *s == Some(FanStage { coords: Coords::Unit, wrap: true })));
        assert_eq!(fan.draw.combiner[54], 0x8421);
    }

    #[test]
    fn the_layers_composite_samples_target_3_the_rasterizer_bitmap_and_0x13() {
        let mut m = setup_mem(Setup::LayersComposite);
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.dest, VIEW);
        assert_eq!(
            fan.sources,
            [
                Some(Source::Surface(game(3))),
                None,
                Some(Source::Bitmap { datum: 0xdcba_0002, image: 0 }),
                Some(Source::Surface(game(0x13))),
            ]
        );
        assert_eq!(fan.draw.raster, FanRaster::Constant);
        assert_eq!(fan.draw.stages[1], None);
        // `dl` clear: into target 3, blue alone.
        m.set_u8(global::FLAG_DL, 0).unwrap();
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.dest, game(3));
        assert_eq!(fan.draw.raster, FanRaster::BlueOnly);
    }

    #[test]
    #[should_panic(expected = "fog fans: layers composite weather plate")]
    fn the_weather_plate_panics() {
        let mut m = setup_mem(Setup::LayersComposite);
        m.set_u8(lg::BRANCH, 0).unwrap();
        turn(&mut m, Setup::LayersComposite, ESP, MXCSR, CONTROL, Some(&Drawn::default()));
    }

    #[test]
    fn another_caller_runs_the_setup() {
        let mut m = setup_mem(Setup::Composite);
        m.set_u32(ESP + ff::SETUP_FRAME + 8, ff::setup::LAYERS_COMPOSITE_CORNERS).unwrap();
        assert_eq!(turn(&mut m, Setup::Composite, ESP, MXCSR, CONTROL, None), Turn::Guest);
        let mut m = setup_mem(Setup::Layers);
        m.set_u32(ESP + ff::SETUP_FRAME, ff::SECOND_RETURN).unwrap();
        assert_eq!(turn(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, None), Turn::Guest);
    }

    #[test]
    #[should_panic(expected = "fog fans: bitmap")]
    fn a_fan_the_host_refuses_panics_with_its_cause() {
        let mut m = setup_mem(Setup::Layers);
        turn(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, Some(&Drawn(Mutex::new(Vec::new()), Some("bitmap"))));
    }

    #[test]
    #[should_panic(expected = "fog fans: host")]
    fn no_host_panics() {
        let mut m = setup_mem(Setup::Composite);
        turn(&mut m, Setup::Composite, ESP, MXCSR, CONTROL, None);
    }

    #[test]
    #[should_panic(expected = "fog fans: composite into target 2")]
    fn another_view_target_panics() {
        let mut m = setup_mem(Setup::Composite);
        m.set_u16(cg::VIEW_TARGET, 2).unwrap();
        turn(&mut m, Setup::Composite, ESP, MXCSR, CONTROL, Some(&Drawn::default()));
    }
}
