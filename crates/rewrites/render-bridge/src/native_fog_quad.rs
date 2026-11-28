//! `native-fog-quad`: the planar fog's screen quad (`0x00025d50`, view
//! modes 2 and 3) drawn by the native renderer, the guest's fan cut
//! (docs/graphics/retire-d3d-fog-quad.md). On with the native renderer.
//!
//! | Guest | Cut |
//! |---|---|
//! | the setup `0x00026520`, called by the fan `0x00034420` (return `0x0003452e`) of `0x00025d50` (return `0x00025d9d`) with the corners `0x00026710` | the quad from `engine::planar_fog` drawn by [`Host::draw`] (`native_render::fog_quad`) at this point in the frame; the setup returns as its gate returns when the density is not above 0: the depth word written, AL 0 |
//!
//! So the guest makes the path it makes with no fog to draw: the fan's
//! draw-state reset `0x00016b10` and rectangle, no setup state, no fan,
//! then `0x00025d50`'s depth clip control. No later draw can depend on the
//! state the setup leaves, as that path leaves none. The setup's body
//! never runs: a density not above 0 makes the gate's write in Rust too.
//!
//! The setup of view modes 1, 4 and 5, `0x00025e50`, is cut the same way
//! at its own entry ([`turn_other`]): its texture-shader fog
//! (`engine::planar_fog::other`) drawn by [`Host::draw_other`]
//! (`native_render::fog_quad::FogOtherDraw`), the depth word written, AL
//! 0, so its fan draws nothing. With `[0x0048578c]` 0 it asks the weather
//! plate `0x0003a5c0` first (`engine::fog_fans::weather_plate_placement`,
//! kind 1); when that returns true the strip draws the plate's block
//! (`FogOtherDraw::plate`) and the depth word is the plate's.
//!
//! What the cuts cannot draw is `unimplemented!("fog quad: <cause>")`: no
//! host, a pass the device does not place (`placed`, `other_placed`), a
//! fan that is not one quad (`fan_shape`, `fan_unfinite`), an operand the
//! SSE or x87 model refuses (`simd`, `x87`), an unreadable input
//! (`unread`), a ramp the texture cache would read from disc
//! (`texture_request: <cause>`, [`crate::texture_cache`]), and a quad
//! placed but not recorded ([`unported`]).

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::planar_fog::{self as pf, FogQuad};
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::fog_quad::{FogOtherDraw, FogQuadDraw};

pub const ID: &str = "native-fog-quad";

/// What draws the quad: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `draw` at this point in the frame. `false`: not placed,
    /// nothing recorded.
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool;
    /// Places the other setup's `draw` likewise.
    fn draw_other(&self, mem: &mut dyn GuestMemory, draw: &FogOtherDraw) -> bool;
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
pub fn on(_: &hooks::Switches) -> bool {
    true
}

/// A quad the cut cannot draw, by cause: the host's too, for a quad placed
/// but not recorded on the GPU.
pub fn unported(cause: &'static str) -> ! {
    unimplemented!("fog quad: {cause}")
}

/// What the setup call does.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// Not the quad's call: run the setup.
    Guest,
    /// The quad is placed (or there is none to draw) and the gate's write
    /// is made: return AL 0.
    Return,
}

/// Whether the setup whose stack is at `esp` is the fan's of `0x00025d50`
/// with its corners: the fan's frame above it holds the return to
/// `0x00025d50`, then the rectangle's address and the callback. The
/// rectangle's address, if so.
pub fn quad_call(mem: &dyn GuestMemory, esp: u32) -> Option<u32> {
    let fan = esp.wrapping_add(pf::SETUP_FRAME);
    let word = |a: u32| mem.u32(a).ok();
    (word(esp)? == pf::SETUP_RETURN && word(fan)? == pf::QUAD_RETURN && word(fan.wrapping_add(8))? == pf::CORNERS)
        .then(|| word(fan.wrapping_add(4)))
        .flatten()
}

fn cause(e: engine::light_constants::Error) -> &'static str {
    match e {
        engine::light_constants::Error::Simd => "simd",
        engine::light_constants::Error::X87(_) => "x87",
        _ => "unread",
    }
}

fn fan(q: &FogQuad) -> [[f32; 4]; 4] {
    q.positions.map(|p| p.map(f32::from_bits))
}

/// The setup call at `esp` under the call's MXCSR and x87 control word,
/// `host` the one drawing.
pub fn turn(mem: &mut dyn GuestMemory, esp: u32, mxcsr: u32, x87_control: u16, host: Option<&dyn Host>) -> Turn {
    let Some(rect_at) = quad_call(&*mem, esp) else {
        return Turn::Guest;
    };
    let draws = pf::draws(&*mem, mxcsr).unwrap_or_else(|e| unported(cause(e)));
    if draws {
        let quad = pf::quad(&*mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
        let host = host.unwrap_or_else(|| unported("host"));
        let draw = FogQuadDraw::from_fan(quad.combiner, fan(&quad)).unwrap_or_else(|shape| unported(shape.cause()));
        if !host.draw(mem, &draw) {
            unported("placed");
        }
    }
    if pf::gate_writes(mem).is_err() {
        unported("depth_write");
    }
    Turn::Return
}

/// The other setup's call at `esp` (`0x00025e50`, view modes 1, 4 and 5),
/// as [`turn`] is the planar quad's. A ramp not resident is the texture
/// cache's `0x00012310` (LOD 0) in Rust (`crate::texture_cache::header`);
/// a header of 0 binds no ramp, so the strip is not drawn that frame; the
/// depth word is written and the setup returns as when it draws.
pub fn turn_other(mem: &mut dyn GuestMemory, esp: u32, mxcsr: u32, x87_control: u16, host: Option<&dyn Host>) -> Turn {
    let Some(rect_at) = quad_call(&*mem, esp) else {
        return Turn::Guest;
    };
    // The plate writes its own depth word when it returns true.
    let (quad, plate) = match pf::other(mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e))) {
        pf::Other::Plate(q) => (q, true),
        pf::Other::Quad(q) => (q, false),
    };
    let host = host.unwrap_or_else(|| unported("host"));
    let positions = quad.positions.map(|p| p.map(f32::from_bits));
    let coordinates = quad.coordinates.map(|v| v.map(|c| c.map(f32::from_bits)));
    let draw = FogOtherDraw::from_fan(quad.combiner, positions, coordinates, quad.depth_test)
        .map(|d| FogOtherDraw { plate, ..d })
        .unwrap_or_else(|shape| unported(shape.cause()));
    // Stage 2 (`0x00025f45`): the plate samples stage 0 alone.
    let ramp = if plate {
        0
    } else {
        let ramp = native_render::fog_quad::ramp_texture(&|a| mem.u32(a).ok()).unwrap_or_else(|why| unported(why));
        match ramp {
            native_render::fog_quad::Ramp::Header(header) => header,
            native_render::fog_quad::Ramp::Streaming { image } => {
                crate::texture_cache::header(mem, image, 0.0).unwrap_or_else(|why| unported(why))
            }
        }
    };
    if (plate || ramp != 0) && !host.draw_other(mem, &FogOtherDraw { ramp, ..draw }) {
        unported("other_placed");
    }
    if !plate && pf::gate_writes(mem).is_err() {
        unported("depth_write");
    }
    Turn::Return
}

/// `0x00025d50`'s entry: its branch must read.
pub fn before_fog(mem: &dyn GuestMemory) {
    if pf::setup(mem).is_err() {
        unimplemented!("fog quad: unread_setup");
    }
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

type TurnFn = fn(&mut dyn GuestMemory, u32, u32, u16, Option<&dyn Host>) -> Turn;

fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    cut(cpu, ctx, addr, engine::points::PLANAR_FOG_SETUP.base, turn)
}

fn other_setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    cut(cpu, ctx, addr, engine::points::PLANAR_FOG_OTHER_SETUP.base, turn_other)
}

fn cut(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32, base: u32, turn: TurnFn) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if addr != base {
        return original(cpu, ctx, addr);
    }
    let rt = Arc::clone(ctx.runtime());
    let turn = {
        let mut mem = rt.memory();
        turn(&mut *mem, cpu.esp, cpu.mxcsr, cpu.x87.control, host().as_deref())
    };
    match turn {
        Turn::Guest => original(cpu, ctx, addr),
        Turn::Return => {
            cpu.eax &= !0xff;
            rt.memory().ret(cpu, 4)
        }
    }
}

pub struct NativeFogQuad;

impl Feature for NativeFogQuad {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The planar fog's screen quad drawn by the native renderer").doc("docs/graphics/retire-d3d-fog-quad.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Each call is drawn from the guest's memory of that call.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&engine::points::PLANAR_FOG, "native_fog_quad_branch", |o| before_fog(&*o.guest()))
            .context_only();
        b.raw().implement(&engine::points::PLANAR_FOG_SETUP, "native_fog_quad", setup_call).context_only();
        b.raw().implement(&engine::points::PLANAR_FOG_OTHER_SETUP, "native_fog_quad_other", other_setup_call).context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use engine::planar_fog::global;

    const ESP: u32 = 0x0050_0000;
    const RECT: u32 = 0x0050_1000;
    const MXCSR: u32 = 0x1f80;
    const CONTROL: u16 = 0x027f;

    /// A setup call from the fan of `0x00025d50` over 640x480, density
    /// 0.5.
    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, engine::screen_grid::constant::ONE, 1.0);
        f(&mut m, engine::screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, engine::screen_grid::constant::NEAR, 0.0625);
        f(&mut m, engine::screen_grid::constant::FAR, 1024.0);
        f(&mut m, global::SCALE, 255.0);
        f(&mut m, global::CLIP_FAR, 1024.0);
        f(&mut m, global::DENSITY, 0.5);
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, RECT + 4 * i as u32, v);
        }
        m.set_u32(ESP, pf::SETUP_RETURN).unwrap();
        m.set_u32(ESP + pf::SETUP_FRAME, pf::QUAD_RETURN).unwrap();
        m.set_u32(ESP + pf::SETUP_FRAME + 4, RECT).unwrap();
        m.set_u32(ESP + pf::SETUP_FRAME + 8, pf::CORNERS).unwrap();
        m
    }

    #[derive(Default)]
    struct Draws(Mutex<Vec<FogQuadDraw>>, bool, Mutex<Vec<FogOtherDraw>>);

    impl Host for Draws {
        fn draw(&self, _: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool {
            self.0.lock().unwrap().push(draw.clone());
            !self.1
        }
        fn draw_other(&self, _: &mut dyn GuestMemory, draw: &FogOtherDraw) -> bool {
            self.2.lock().unwrap().push(draw.clone());
            !self.1
        }
    }

    /// The addresses `turn` changed in the test memory.
    fn turned(m: &mut FlatMemory, host: Option<&dyn Host>) -> (Turn, Vec<u32>) {
        let before = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        let t = turn(m, ESP, MXCSR, CONTROL, host);
        let after = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        let changed = (0..before.len()).filter(|&i| before[i] != after[i]).map(|i| 0x0040_0000 + i as u32).collect();
        (t, changed)
    }

    #[test]
    fn the_first_call_is_cut_without_the_body() {
        let mut m = mem();
        let host = Draws::default();
        let (t, changed) = turned(&mut m, Some(&host));
        assert_eq!(t, Turn::Return);
        let drawn = host.0.lock().unwrap().clone();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].rect, [0.0, 0.0, 640.0, 480.0]);
        assert_eq!(drawn[0].w, 16_777_215.0);
        // The only write is the gate's: the depth word.
        assert!(!changed.is_empty());
        assert!(changed.iter().all(|&a| (global::DEPTH..global::DEPTH + 4).contains(&a)));
        assert_eq!(m.u32(global::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn no_density_makes_the_gates_write_and_draws_nothing() {
        let mut m = mem();
        m.set_f32(global::DENSITY, 0.0).unwrap();
        let host = Draws::default();
        let (t, changed) = turned(&mut m, Some(&host));
        assert_eq!(t, Turn::Return);
        assert!(host.0.lock().unwrap().is_empty());
        assert!(changed.iter().all(|&a| (global::DEPTH..global::DEPTH + 4).contains(&a)));
        assert_eq!(m.u32(global::DEPTH).unwrap(), 1024.0f32.to_bits());
        // With no density there is nothing to place, so no host is asked.
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, None), Turn::Return);
    }

    #[test]
    fn another_caller_runs_the_setup() {
        let mut m = mem();
        m.set_u32(ESP + pf::SETUP_FRAME + 8, 0x0002_54c0).unwrap();
        assert_eq!(quad_call(&m, ESP), None);
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, None), Turn::Guest);
    }

    #[test]
    #[should_panic(expected = "fog quad: placed")]
    fn a_pass_the_device_does_not_place_panics() {
        let mut m = mem();
        let refusing = Draws(Mutex::new(Vec::new()), true, Mutex::new(Vec::new()));
        turn(&mut m, ESP, MXCSR, CONTROL, Some(&refusing));
    }

    #[test]
    #[should_panic(expected = "fog quad: host")]
    fn no_host_panics() {
        let mut m = mem();
        turn(&mut m, ESP, MXCSR, CONTROL, None);
    }

    /// The ramp image [`other_mem`] binds, header [`RAMP_HEADER`].
    const RAMP_IMAGE: u32 = 0x0051_3000;
    const RAMP_HEADER: u32 = 0x0051_4000;

    /// The other setup's call, its plate word set and its ramp resident.
    fn other_mem() -> FlatMemory {
        use native_render::decal_material::{RASTERIZER, TAG_TABLE};
        let mut m = mem();
        m.set_f32(engine::planar_fog::other::PLATE, 0.5).unwrap();
        m.set_f32(engine::planar_fog::other::TINY, 0.0001).unwrap();
        let (raster, table, tag) = (0x0051_0000, 0x0051_1000, 0x0051_2000);
        for (a, v) in [
            (RASTERIZER, raster),
            (raster + 0x14, 0xe000_0002),
            (raster + 0x64, u32::MAX),
            (TAG_TABLE, table),
            (table + 2 * 16 + 8, tag),
            (tag + 0x44, 1),
            (tag + 0x48, RAMP_IMAGE),
            (RAMP_IMAGE + 0x70, 5),
            (RAMP_IMAGE + 0x50, RAMP_HEADER),
            (native_render::effect_material::RESIDENT_STAMP, 4),
        ] {
            m.set_u32(a, v).unwrap();
        }
        m
    }

    #[test]
    fn the_other_setup_is_cut_and_draws_its_strip() {
        let mut m = other_mem();
        let host = Draws::default();
        let before = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        assert_eq!(turn_other(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        let after = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        let drawn = host.2.lock().unwrap().clone();
        assert_eq!(drawn.len(), 1);
        assert!(drawn[0].depth_test);
        assert_eq!(drawn[0].strip[3].0, [640.0, 480.0]);
        assert_eq!(drawn[0].combiner[54], 0x2621);
        assert_eq!(drawn[0].ramp, RAMP_HEADER);
        let changed: Vec<u32> = (0..before.len()).filter(|&i| before[i] != after[i]).map(|i| 0x0040_0000 + i as u32).collect();
        assert!(changed.iter().all(|&a| (global::DEPTH..global::DEPTH + 4).contains(&a)));
    }

    /// [`other_mem`] with its ramp streamed (`+0x0f` bit 1) at the cache's
    /// stamp with no header, its level 0 in cache entry 0, loaded or not.
    fn streaming_mem(loaded: bool) -> FlatMemory {
        use crate::texture_cache as tc;
        let mut m = other_mem();
        for (a, v) in [
            (native_render::effect_material::RESIDENT_STAMP, 5),
            (RAMP_IMAGE + 0x50, 0),
            (RAMP_IMAGE + 0xc, 0x0200_0000),
            (RAMP_IMAGE + 0x58, 1.0f32.to_bits()),
            (RAMP_IMAGE + 0x1c, 0),
            (RAMP_IMAGE + 0x34, 0x100),
            (RAMP_IMAGE + 0x28, 0),
            (RAMP_IMAGE + 0x20, u32::MAX),
            (RAMP_IMAGE + 0x24, u32::MAX),
            (RAMP_IMAGE + 0x2c, u32::MAX),
            (RAMP_IMAGE + 0x30, u32::MAX),
            (tc::TEXTURES, 0x0051_6000),
            (0x0051_6044, TEXTURE_ENTRIES),
            (tc::TEXTURE_RESOURCES, 0x0051_7000),
            (0x0051_7064, 0x0051_7100),
            (0x0051_7144, 0x0051_7200),
        ] {
            m.set_u32(a, v).unwrap();
        }
        m.set_u8(TEXTURE_ENTRIES + 3, u8::from(loaded)).unwrap();
        m
    }

    const TEXTURE_ENTRIES: u32 = 0x0051_6100;

    #[test]
    fn a_streaming_ramp_takes_the_texture_caches_header() {
        let mut m = streaming_mem(true);
        let host = Draws::default();
        assert_eq!(turn_other(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        assert_eq!(host.2.lock().unwrap()[0].ramp, TEXTURE_ENTRIES + 0x14);
        // The cache holds its level 0 as `0x0012ccf0` does.
        assert_eq!(m.u32(RAMP_IMAGE + 0x50).unwrap(), TEXTURE_ENTRIES + 0x14);
    }

    #[test]
    fn a_ramp_default_of_0_draws_no_strip_and_writes_the_depth() {
        let mut m = streaming_mem(false);
        // Pinned under I/O load: the 2D default `[0x00485ae4]`, here 0.
        m.set_u32(crate::texture_cache::PIN_COUNT, 2).unwrap();
        m.set_u32(crate::texture_cache::IO_LOAD, 0x1a).unwrap();
        m.set_u32(global::DEPTH, 0).unwrap();
        let host = Draws::default();
        assert_eq!(turn_other(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        assert!(host.2.lock().unwrap().is_empty());
        assert_eq!(m.u32(global::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    #[should_panic(expected = "fog quad: texture_request: 0x0012ca6f")]
    fn a_ramp_read_to_wait_for_panics_with_its_cause() {
        let mut m = streaming_mem(false);
        turn_other(&mut m, ESP, MXCSR, CONTROL, Some(&Draws::default()));
    }

    #[test]
    fn the_other_setups_plate_draws_its_block_at_its_depth() {
        use engine::fog_fans::plate_global as p;
        use engine::planar_fog::other as o;
        let mut m = other_mem();
        for (a, v) in [
            (o::PLATE, 0.0),
            (o::NEAR_START, 0.25),
            (o::NEAR_END, 0.75),
            (o::DEPTH_SCALE, 1.0),
            (p::CAP_KIND1, 128.0),
            (p::RESCALE, 256.0),
            (p::FIXED, 65536.0),
            (p::UNFIXED, 1.0 / 65536.0),
            (p::HALF, 0.5),
            (p::LIMIT, 0.1),
            (p::KIND1_DIV, 2.0),
        ] {
            m.set_f32(a, v).unwrap();
        }
        let host = Draws::default();
        assert_eq!(turn_other(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        let drawn = host.2.lock().unwrap().clone();
        assert_eq!(drawn.len(), 1);
        assert!(drawn[0].plate && drawn[0].depth_test);
        assert_eq!(drawn[0].combiner[54], 1);
        // The plate's depth word, not the gate's clip far.
        assert_eq!(m.u32(global::DEPTH).unwrap(), 0.25f32.to_bits());
    }

    #[test]
    #[should_panic(expected = "fog quad: other_placed")]
    fn an_other_pass_the_device_does_not_place_panics() {
        let mut m = other_mem();
        let refusing = Draws(Mutex::new(Vec::new()), true, Mutex::new(Vec::new()));
        turn_other(&mut m, ESP, MXCSR, CONTROL, Some(&refusing));
    }

    #[test]
    fn every_mode_passes_the_branch() {
        let mut m = mem();
        for mode in [0, 1, 2, 3, 4, 5] {
            m.set_u32(global::MODE, mode).unwrap();
            before_fog(&m);
        }
    }

    /// The pass draws with the state the setup sets.
    #[test]
    fn the_native_raster_is_the_setups_state() {
        use native_render::fog_quad as nq;
        let state = |name: &str| pf::STATES.iter().find(|s| s.0 == name).map(|s| s.3).unwrap();
        assert_eq!(state("blend_source"), nq::BLEND_SOURCE);
        assert_eq!(state("blend_dest"), nq::BLEND_DEST);
        assert_eq!(state("blend_equation"), nq::BLEND_EQUATION);
        assert_eq!(state("blend_colour"), nq::BLEND_COLOUR);
        assert_eq!(state("z_enable"), nq::Z_ENABLE);
        assert_eq!(state("z_func"), nq::Z_FUNC);
        assert_eq!(pf::WORDS, nq::WORDS);
    }
}
