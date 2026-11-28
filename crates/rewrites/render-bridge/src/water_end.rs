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
//! Past them the guest draws the ocean (`0x0003c3ff..0x0003c632`): the
//! grid's setup `0x0003ca90`, its visibility `0x0003c270` and texture
//! `0x0012de70`, the shader's copies (`0x0003bea0` into target `0x16`,
//! `0x0003bd00` into target `0x15`), the alpha clear `0x0003c515` when the
//! second argument's byte is set, the draw state and the indexed quads
//! `0x0001c8a0(8, [geometry + 0x24], [geometry + 0x20])`, then the
//! texture stamps `0x0003bcb0` of the shader's bitmaps. The native
//! renderer has no ocean stage yet, so that call is
//! `unimplemented!("water end: ocean")`; a word that does not read is
//! `water end: unread`.

use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};

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

/// The call's EAX and ECX, or a named panic: `ocean`, `unread`.
pub fn water_end(mem: &dyn GuestMemory, draw: u8, clear: u8) -> (u32, u32) {
    let call = decide(mem, draw, clear).unwrap_or_else(|_| unported("unread"));
    match call.end {
        End::Ocean { .. } => unported("ocean"),
        End::NoWater | End::Still | End::NotDrawn => call.registers(),
    }
}

pub static WATER_END: crate::native_targets::Arm = crate::native_targets::Arm::new("water_end");

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    WATER_END.counters()
}

fn end_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    WATER_END.call();
    let esp = cpu.esp;
    let rt = Arc::clone(ctx.runtime());
    let (eax, ecx) = {
        let mem = rt.memory();
        let arg = |at: u32| mem.u8(esp.wrapping_add(at)).unwrap_or_else(|_| unported("unread"));
        let (draw, clear) = (arg(ARG_DRAW), arg(ARG_CLEAR));
        water_end(&*mem, draw, clear)
    };
    cpu.eax = eax;
    cpu.ecx = ecx;
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
        assert_eq!(water_end(&mem(1, 1.0), 0, 0), (AT | 1, 1));
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
        assert_eq!(water_end(&m, 1, 1), (0x0050_1201, u32::MAX));
    }

    #[test]
    #[should_panic(expected = "water end: ocean")]
    fn the_ocean_panics() {
        water_end(&mem(1, 1.0), 1, 1);
    }

    #[test]
    #[should_panic(expected = "water end: unread")]
    fn an_unread_bsp_panics() {
        let mut m = mem(1, 1.0);
        m.set_u32(BSP, 0x0010_0000).unwrap();
        water_end(&m, 1, 0);
    }
}
