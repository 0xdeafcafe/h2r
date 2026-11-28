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
//! What the cut cannot draw is `unimplemented!("fog quad: <cause>")`: no
//! host, a pass the device does not place (`placed`), a fan that is not
//! one quad (`fan_shape`, `fan_unfinite`), an operand the SSE or x87
//! model refuses (`simd`, `x87`), an unreadable input (`unread`), a quad
//! placed but not recorded ([`unported`]), and the fan of the setup
//! `0x00025e50` (modes 1, 4 and 5, `other_setup`), which has no native
//! stage.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::planar_fog::{self as pf, FogQuad, Setup};
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::fog_quad::FogQuadDraw;

pub const ID: &str = "native-fog-quad";

/// What draws the quad: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `draw` at this point in the frame. `false`: not placed,
    /// nothing recorded.
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool;
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

/// `0x00025d50`'s entry: the setup `0x00025e50` of modes 1, 4 and 5 has
/// no native stage.
pub fn before_fog(mem: &dyn GuestMemory) {
    match pf::setup(mem) {
        Ok(Setup::Other) => unimplemented!("fog quad: other_setup (0x00025e50, view modes 1, 4, 5)"),
        Ok(_) => {}
        Err(_) => unimplemented!("fog quad: unread_setup"),
    }
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if addr != engine::points::PLANAR_FOG_SETUP.base {
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
    struct Draws(Mutex<Vec<FogQuadDraw>>, bool);

    impl Host for Draws {
        fn draw(&self, _: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool {
            self.0.lock().unwrap().push(draw.clone());
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
        let refusing = Draws(Mutex::new(Vec::new()), true);
        turn(&mut m, ESP, MXCSR, CONTROL, Some(&refusing));
    }

    #[test]
    #[should_panic(expected = "fog quad: host")]
    fn no_host_panics() {
        let mut m = mem();
        turn(&mut m, ESP, MXCSR, CONTROL, None);
    }

    #[test]
    #[should_panic(expected = "fog quad: other_setup")]
    fn the_other_setup_panics() {
        let mut m = mem();
        m.set_u32(global::MODE, 4).unwrap();
        before_fog(&m);
    }

    #[test]
    fn modes_2_and_3_pass_the_branch() {
        let mut m = mem();
        for mode in [0, 2, 3] {
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
