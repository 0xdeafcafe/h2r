//! The water end `0x0003c3c0` in Rust with the native renderer
//! (docs/graphics/retire-d3d-display.md, "The water end"). No guest body
//! runs.
//!
//! The late passes call it after layer 6 (`0x0002c132`): `(1, al)` with
//! the setup wrapper `0x0003ba90`'s AL when the water gate (bit 4 of
//! `0x004b9fe0`) is clear, `(0, 0)` when it is set. `ret 8`, AL 1.
//!
//! The guest's gates (Code, `0x0003c3c0..0x0003c3f6`), each a return with
//! no work ([`decide`]):
//!
//! | Gate | Read |
//! |---|---|
//! | no water | the active BSP `[0x004e0348]`'s definition count `+0x214` at or below 0 |
//! | still | its first definition `[bsp + 0x218]`'s `+0x7c` equal (`ucomiss`) to `[0x0045dbd8]`, 0.0 |
//! | not drawn | the first argument's byte 0 |
//!
//! Past them is the ocean (`0x0003c3ff..0x0003c632`), [`ocean::run`]:
//! Rust makes the grid (`engine::ocean_grid`), the gates (the occlusion
//! query over the native flare query's count pass, [`ocean::gate`]; the
//! texture block's residency, [`ocean::cache`]), the shader lookup, the
//! copy `0x0003bea0` into target `0x16`, the alpha clear `0x0003c515`,
//! the draw (the ocean's pass from the map over the frame's record,
//! [`OceanHost`]) and the texture stamps, then returns as the body does.
//! No guest helper is called. The sky pass into target `0x15` is not
//! made. A step it cannot make is `unimplemented!("water ocean:
//! <cause>")`, or `water ocean copy: <cause>` for the copies; a gate word
//! that does not read is `water end: unread`.

use std::sync::{Arc, RwLock};

use engine::memory::{GuestMemory, MemFault};

pub mod ocean;

/// The active BSP's pointer.
pub const BSP: u32 = 0x004e_0348;
/// The BSP's water definitions: count (signed) and pointer.
pub const WATER_COUNT: u32 = 0x214;
pub const WATER_DEFINITIONS: u32 = 0x218;
/// The word of the first definition the teardown tests against [`ZERO`].
pub const DEFINITION_GATE: u32 = 0x7c;
/// The guest's 0.0.
pub const ZERO: u32 = 0x0045_dbd8;
/// The call's stack arguments: the draw byte and the clear byte.
pub const ARG_DRAW: u32 = 4;
pub const ARG_CLEAR: u32 = 8;
/// Bytes of argument `ret 8` pops.
pub const ARG_BYTES: u32 = 8;

/// What the call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The BSP has no water definitions.
    NoWater,
    /// The first definition's gate word is 0.0.
    Still,
    /// The call's draw byte is 0.
    NotDrawn,
    /// The guest would draw the ocean of `definition`, clearing the bound
    /// target's alpha before it when `clear`.
    Ocean { definition: u32, clear: bool },
}

/// One call's reads: what it does, and the BSP and count the guest leaves
/// in EAX and ECX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub end: End,
    pub bsp: u32,
    pub count: u32,
}

impl Call {
    /// EAX and ECX at the return: `mov al, 1` over the BSP pointer, the
    /// count in ECX.
    pub fn registers(&self) -> (u32, u32) {
        ((self.bsp & !0xff) | 1, self.count)
    }
}

/// The gates for a call with bytes `draw` and `clear`, in the guest's order.
pub fn decide(mem: &dyn GuestMemory, draw: u8, clear: u8) -> Result<Call, MemFault> {
    let bsp = mem.u32(BSP)?;
    let count = mem.u32(bsp.wrapping_add(WATER_COUNT))?;
    let call = |end| Call { end, bsp, count };
    if count as i32 <= 0 {
        return Ok(call(End::NoWater));
    }
    let definition = mem.u32(bsp.wrapping_add(WATER_DEFINITIONS))?;
    // `ucomiss; lahf; test ah, 0x44; jnp`: equal and ordered returns.
    if mem.f32(definition.wrapping_add(DEFINITION_GATE))? == mem.f32(ZERO)? {
        return Ok(call(End::Still));
    }
    if draw == 0 {
        return Ok(call(End::NotDrawn));
    }
    Ok(call(End::Ocean { definition, clear: clear != 0 }))
}

/// A call the native renderer cannot make: no guest body runs instead.
pub fn unported(cause: &str) -> ! {
    crate::native_targets::unhandled("water end", cause)
}

/// The call's gates, or a named panic: `unread`.
pub fn water_end(mem: &dyn GuestMemory, draw: u8, clear: u8) -> Call {
    decide(mem, draw, clear).unwrap_or_else(|_| unported("unread"))
}

/// An ocean step the native renderer cannot make: no guest body runs
/// instead.
pub fn ocean_unported(cause: &str) -> ! {
    match cause.strip_prefix("copy_") {
        Some(copy) => unimplemented!("water ocean copy: {copy}"),
        None => unimplemented!("water ocean: {cause}"),
    }
}

pub static WATER_END: crate::native_targets::Arm = crate::native_targets::Arm::new("water_end");
pub static WATER_OCEAN: crate::native_targets::Arm = crate::native_targets::Arm::new("water_ocean");

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    WATER_END.counters().into_iter().chain(WATER_OCEAN.counters()).collect()
}

/// The guest stack below the call's return address the clear stages its
/// words in: 64 bytes at `esp - SCRATCH`.
const SCRATCH: u32 = 0x50;

/// The ocean's draw on the device: boot's glue (`native_ocean_glue`).
pub trait OceanHost: Send + Sync {
    /// Draws `draw` on the bound target, its quads as the scene holds
    /// them; `Err` names what it could not make.
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &ocean::Draw, quads: &frame_scene::late::WaterComposite) -> Result<(), ocean::Cause>;
}

static OCEAN_HOST: RwLock<Option<Arc<dyn OceanHost>>> = RwLock::new(None);

pub fn set_ocean_host(host: Arc<dyn OceanHost>) {
    *OCEAN_HOST.write().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn ocean_host() -> Option<Arc<dyn OceanHost>> {
    OCEAN_HOST.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The ocean's device: the native targets' host for the clear and the
/// copy, the scene for the record and [`OceanHost`] for the draw.
struct Live {
    host: Arc<dyn crate::native_targets::Host>,
    scratch: u32,
    mxcsr: u32,
    x87_control: u16,
}

impl ocean::Device for Live {
    fn host(&self) -> &dyn crate::native_targets::Host {
        &*self.host
    }

    fn scratch(&self) -> u32 {
        self.scratch
    }

    fn draw(&mut self, mem: &mut dyn GuestMemory, draw: &ocean::Draw) -> Result<(), ocean::Cause> {
        let quads = crate::scene_step::set_ocean(&*mem, draw.ocean, draw.indices, draw.count).ok_or("quads")?;
        ocean_host().ok_or("host")?.draw(mem, draw, &quads)
    }

    fn visible(&mut self, mem: &mut dyn GuestMemory, definition: u32) -> Result<bool, ocean::Cause> {
        crate::native_flare_query::with_host(|host, state| ocean::gate::visible(mem, host, state, definition)).unwrap_or(Err("flare_host"))
    }

    fn fp(&mut self) -> (&mut u32, u16) {
        (&mut self.mxcsr, self.x87_control)
    }
}

fn end_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    WATER_END.call();
    let esp = cpu.esp;
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let arg = |mem: &dyn GuestMemory, at: u32| mem.u8(esp.wrapping_add(at)).unwrap_or_else(|_| unported("unread"));
    let (draw, clear) = (arg(&*mem, ARG_DRAW), arg(&*mem, ARG_CLEAR));
    let call = water_end(&*mem, draw, clear);
    (cpu.eax, cpu.ecx) = match call.end {
        End::Ocean { definition, clear } => {
            WATER_OCEAN.call();
            let host = crate::native_targets::host().unwrap_or_else(|| ocean_unported("host"));
            let mut device = Live { host, scratch: esp.wrapping_sub(SCRATCH), mxcsr: cpu.mxcsr, x87_control: cpu.x87.control };
            let ended = ocean::run(&mut *mem, &mut device, definition, clear, cpu.ecx).unwrap_or_else(|cause| ocean_unported(cause));
            cpu.mxcsr = device.mxcsr;
            (ended.eax, ended.ecx)
        }
        _ => call.registers(),
    };
    drop(mem);
    ctx.memory().ret(cpu, ARG_BYTES)
}

/// Places the water end's implementation (`native-water`'s feature).
pub fn place(b: &mut hooks::Builder<'_>) {
    b.raw().implement(&engine::points::WATER_TEARDOWN, "native_water_end", end_call).context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    const AT: u32 = 0x0050_0000;
    const DEFINITIONS: u32 = 0x0051_0000;

    fn mem(count: i32, gate: f32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(BSP, AT).unwrap();
        m.set_u32(AT + WATER_COUNT, count as u32).unwrap();
        m.set_u32(AT + WATER_DEFINITIONS, DEFINITIONS).unwrap();
        m.set_f32(DEFINITIONS + DEFINITION_GATE, gate).unwrap();
        m.set_f32(ZERO, 0.0).unwrap();
        m
    }

    fn end(m: &FlatMemory, draw: u8, clear: u8) -> End {
        decide(m, draw, clear).unwrap().end
    }

    /// The gates return with no work, in the guest's order, whatever the
    /// arguments.
    #[test]
    fn the_gates_end_with_no_work() {
        for count in [0, -1, i32::MIN] {
            assert_eq!(end(&mem(count, 1.0), 1, 1), End::NoWater, "{count}");
        }
        for gate in [0.0, -0.0] {
            assert_eq!(end(&mem(1, gate), 1, 1), End::Still, "{gate}");
        }
        assert_eq!(end(&mem(1, 1.0), 0, 1), End::NotDrawn);
        // The late passes' call with the water gate set.
        assert_eq!(water_end(&mem(1, 1.0), 0, 0).registers(), (AT | 1, 1));
    }

    /// A non-zero gate word, NaN included (`ucomiss` unordered), with the
    /// draw byte set is the ocean; the clear byte rides along.
    #[test]
    fn a_drawn_definition_is_the_ocean() {
        for gate in [1.0, -0.5, f32::NAN] {
            for clear in [0, 1] {
                assert_eq!(end(&mem(2, gate), 1, clear), End::Ocean { definition: DEFINITIONS, clear: clear != 0 });
            }
        }
    }

    /// EAX keeps the BSP pointer's high bytes under AL 1, ECX the count.
    #[test]
    fn the_return_leaves_the_guests_registers() {
        let mut m = mem(0, 0.0);
        m.set_u32(BSP, 0x0050_12ab).unwrap();
        m.set_u32(0x0050_12ab + WATER_COUNT, u32::MAX).unwrap();
        assert_eq!(water_end(&m, 1, 1).registers(), (0x0050_1201, u32::MAX));
    }

    #[test]
    #[should_panic(expected = "water ocean: shader_group")]
    fn an_unported_ocean_step_is_named() {
        ocean_unported("shader_group");
    }

    #[test]
    #[should_panic(expected = "water ocean copy: pass")]
    fn an_unported_copy_is_named() {
        ocean_unported("copy_pass");
    }

    #[test]
    #[should_panic(expected = "water end: unread")]
    fn an_unread_bsp_panics() {
        let mut m = mem(1, 1.0);
        m.set_u32(BSP, 0x0010_0000).unwrap();
        water_end(&m, 1, 0);
    }
}
