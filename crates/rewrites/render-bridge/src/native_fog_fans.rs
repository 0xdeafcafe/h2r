//! `native-fog-fans`: the second fog's screen fans (`0x00025dc0`)
//! (docs/graphics/retire-d3d-fog-fans.md). On with the native renderer.
//!
//! | Fans | Here |
//! |---|---|
//! | `dl` alone: the composite, setup `0x000276d0` (return `0x0003452e`) of the fan returning to `0x00025e2d` with the corners `0x00027960` | the fan from `engine::fog_fans::composite` drawn by [`Host::composite`] (`native_render::fog_fans`) at this point in the frame, into the view's target and sampling target 3 by role; the setup writes the depth word and returns AL 0, so no fan reaches D3D |
//! | `cl`: the layers into target `0x13`, then their composite | `unimplemented!("fog fans: layers …")` at `0x00025dc0`'s entry |
//!
//! So the guest makes the path it makes with the gate shut, but for the
//! fan's draw-state reset `0x00016b10` and rectangle: no setup state, no
//! fan, then `0x00025dc0`'s tail. The setup's body never runs.
//!
//! What the cut cannot draw is `unimplemented!("fog fans: <cause>")`: no
//! host (`host`), a pass the device does not place (`placed`), a view
//! target other than 0 (`composite into target <n>`), a fan not finite
//! (`composite_unfinite`), an operand the SSE or x87 model refuses
//! (`simd`, `x87`), an unreadable input (`unread`), and a fan placed but
//! not recorded ([`unported`], the GPU's cause).

use std::sync::{Arc, Mutex};

use engine::fog_fans::{self as ff, Fans};
use engine::memory::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::fog_fans::CompositeDraw;

pub const ID: &str = "native-fog-fans";

/// One composite fan to place: the draw, and the surface headers it
/// draws into and samples at stage 0.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeFan {
    pub draw: CompositeDraw,
    pub dest: u32,
    pub source: u32,
}

/// What draws the fans: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `fan` at this point in the frame. `false`: not placed,
    /// nothing recorded.
    fn composite(&self, mem: &mut dyn GuestMemory, fan: &CompositeFan) -> bool;
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

/// `0x00025dc0`'s entry with `ecx` and `edx` under the call's MXCSR: the
/// layers panic, named; the composite is cut at its setup ([`turn`]).
pub fn before_fans(mem: &dyn GuestMemory, mxcsr: u32, ecx: u32, edx: u32) {
    match ff::fans(mem, mxcsr, ecx as u8, edx as u8) {
        Ok(Fans::None | Fans::Composite) => {}
        Ok(fans) => unported(fans.cause()),
        Err(e) => unported(cause(e)),
    }
}

/// The surface header of the view's target `n`: target 0's colour is the
/// view's own (`0x00014bc0`'s first case).
fn view_surface(mem: &dyn GuestMemory, n: i16) -> u32 {
    if n != 0 {
        unported(&format!("composite into target {n}"));
    }
    mem.u32(crate::frame_tail::TARGET0_COLOUR).unwrap_or_else(|_| unported("unread"))
}

/// What the setup call does.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// Not the composite's fan: run the setup.
    Guest,
    /// The fan is placed and the setup's write made: return AL 0.
    Return,
}

/// The composite setup's call at `esp` under the call's MXCSR and x87
/// control word, `host` the one drawing.
pub fn turn(mem: &mut dyn GuestMemory, esp: u32, mxcsr: u32, x87_control: u16, host: Option<&dyn Host>) -> Turn {
    let Some(rect_at) = ff::fan_call(&*mem, esp, ff::SECOND_RETURN, ff::setup::COMPOSITE_CORNERS) else {
        return Turn::Guest;
    };
    let c = ff::composite(&*mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
    let f = |v: [[u32; 4]; 4]| v.map(|p| p.map(f32::from_bits));
    let draw = CompositeDraw::from_fan(c.combiner, f(c.positions), f(c.coordinates)).unwrap_or_else(|r| unported(r.cause()));
    let source = crate::native_targets::surface(frame_types::TargetRole::Game(ff::COMPOSITE_SOURCE)).unwrap_or_else(|| unported("source"));
    let fan = CompositeFan { draw, dest: view_surface(&*mem, c.target), source };
    let host = host.unwrap_or_else(|| unported("host"));
    if !host.composite(mem, &fan) {
        unported("placed");
    }
    if ff::composite_writes(mem).is_err() {
        unported("depth_write");
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
    if addr != engine::points::FOG_COMPOSITE_SETUP.base {
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

pub struct NativeFogFans;

impl Feature for NativeFogFans {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The second fog's screen fans: the composite drawn by the native renderer, the layers a named panic")
            .doc("docs/graphics/retire-d3d-fog-fans.md")
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
        b.raw().implement(&engine::points::FOG_COMPOSITE_SETUP, "native_fog_fans_composite", setup_call).context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::fog_fans::{composite_global as cg, global};
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

    /// A composite setup call from the fan of `0x00025dc0` over 640x480
    /// into target 0.
    fn composite_mem() -> FlatMemory {
        let mut m = mem(0.5);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, engine::screen_grid::constant::ONE, 1.0);
        f(&mut m, engine::screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, engine::screen_grid::constant::NEAR, 0.0625);
        f(&mut m, engine::screen_grid::constant::FAR, 1024.0);
        f(&mut m, cg::SCALE, 255.0);
        f(&mut m, cg::CLIP_FAR, 1024.0);
        f(&mut m, cg::COLOUR, 1.0);
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, RECT + 4 * i as u32, v);
        }
        m.set_u32(crate::frame_tail::TARGET0_COLOUR, VIEW).unwrap();
        m.set_u32(ESP, ff::SETUP_RETURN).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME, ff::SECOND_RETURN).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME + 4, RECT).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME + 8, ff::setup::COMPOSITE_CORNERS).unwrap();
        m
    }

    #[derive(Default)]
    struct Drawn(Mutex<Vec<CompositeFan>>, bool);

    impl Host for Drawn {
        fn composite(&self, _: &mut dyn GuestMemory, fan: &CompositeFan) -> bool {
            self.0.lock().unwrap().push(fan.clone());
            !self.1
        }
    }

    #[test]
    fn a_call_with_no_fan_or_the_composite_runs() {
        before_fans(&mem(0.0), MXCSR, 0x01, 0x01);
        before_fans(&mem(0.5), MXCSR, 0xff00, 0xff00);
        before_fans(&mem(0.5), MXCSR, 0, 0x01);
    }

    #[test]
    #[should_panic(expected = "fog fans: layers")]
    fn the_layers_panic() {
        before_fans(&mem(0.5), MXCSR, 0x01, 0);
    }

    #[test]
    #[should_panic(expected = "fog fans: unread")]
    fn an_unread_gate_panics() {
        let m = FlatMemory::new(0x0010_0000, 0x10);
        before_fans(&m, MXCSR, 1, 0);
    }

    #[test]
    fn the_composite_is_cut_into_the_view_sampling_target_3() {
        let mut m = composite_mem();
        let host = Drawn::default();
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        let drawn = host.0.lock().unwrap().clone();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].dest, VIEW);
        let three = crate::native_targets::surface(frame_types::TargetRole::Game(3)).unwrap();
        assert_eq!(drawn[0].source, three);
        assert_eq!(crate::native_targets::role(drawn[0].source, 0), Some(frame_types::TargetRole::Game(3)));
        assert_eq!(drawn[0].draw.strip[3], ([640.0, 480.0], [640.0, 480.0, 0.0, 1.0]));
        assert_eq!(drawn[0].draw.combiner[43], 0x00ff_0000);
        assert_eq!(m.u32(cg::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn another_caller_runs_the_setup() {
        let mut m = composite_mem();
        m.set_u32(ESP + ff::SETUP_FRAME + 8, ff::setup::LAYERS_COMPOSITE_CORNERS).unwrap();
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, None), Turn::Guest);
    }

    #[test]
    #[should_panic(expected = "fog fans: placed")]
    fn a_pass_the_device_does_not_place_panics() {
        let mut m = composite_mem();
        turn(&mut m, ESP, MXCSR, CONTROL, Some(&Drawn(Mutex::new(Vec::new()), true)));
    }

    #[test]
    #[should_panic(expected = "fog fans: host")]
    fn no_host_panics() {
        let mut m = composite_mem();
        turn(&mut m, ESP, MXCSR, CONTROL, None);
    }

    #[test]
    #[should_panic(expected = "fog fans: composite into target 2")]
    fn another_view_target_panics() {
        let mut m = composite_mem();
        m.set_u16(cg::VIEW_TARGET, 2).unwrap();
        turn(&mut m, ESP, MXCSR, CONTROL, Some(&Drawn::default()));
    }
}
