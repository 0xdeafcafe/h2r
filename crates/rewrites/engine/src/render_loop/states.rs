//! The shader draw's render-state list, `0x0001a170` (docs/graphics/layer-draw.md,
//! "Shader draw begin"; the read is in docs/graphics/render-loop.md, "The
//! render-state list"), as a Port-grade state machine (see [`super`]). The
//! native world's walk runs it with [`Mode::SkipRedundant`]; the
//! differential test checks both modes against the lifted original.
//!
//! `0x00016b90` calls it once per draw with EBX the shader draw's context
//! and one stack argument: a word `count << 9 | first`. Entry `i` is the
//! four bytes at `[[ctx+0x20]+0x3c] + 4·(first+i)`: a state definition
//! (byte 0, an index into the 4-byte records at `0x00467020`: the D3D render
//! state at `+0`, the value's type at `+3`), the value's source (byte 1: 1 a
//! float parameter at `ctx+0x5c+4·n`, otherwise a colour function
//! `0x00131ed0(ctx+12·(n+0x16))`), and `n` (byte 3). By type the value is
//! the float converted with `fistp` (2), its bits (1), or the float times
//! the constant at `0x0045dc38`, `fistp`, as the top byte over the state's
//! low 24 bits (0); a colour replaces the low 24 bits. Then the value goes
//! to D3D as `SetRenderState` does inline:
//!
//! - states below `0x5c`: `SetRenderState_Simple` (`0x003f5cf0`, ECX the push
//!   header from `0x0043e6f0`, EDX the value), then the shadow
//!   `0x00407498 + 4·state`;
//! - below `0x88`: deferred: the state's dirty bit (`0x0043e4d0`) into
//!   `0x00407490`, and the shadow;
//! - `0x88`: dirty bit `0x4000` and `0x004076b8` (the body's copy to the
//!   device's `+0x790` only fed D3D; the port makes none);
//! - `0x89` to `0xa5`: the state's own stdcall setter (`setter`).
//!
//! There is no comparison with the shadow: a value D3D already holds is
//! pushed again. With [`Mode::SkipRedundant`] the port skips the Simple call
//! (and its shadow store, which would store the same value) when the shadow
//! already holds the value, except for the states that share a push header
//! (`SHARED_HEADER`). Deferred states, `0x88` and the setters run
//! as in the original. [`Mode::Exact`] is the original call for call.
//!
//! x87: the original converts with `fld` / `fmul` / `fistp` under the
//! guest's control word; the port does the same arithmetic on the exact
//! product (two 24-bit significands fit a double) with the control word's
//! precision and rounding, so it needs the FPU control word at entry. The
//! x87 registers themselves are not reproduced (the original leaves the
//! stack as it found it).

use super::{Regs, Step};
use crate::memory::{GuestMemory, MemFault};

pub const RENDER_STATES: u32 = 0x0001_a170;
/// `D3DDevice_SetRenderState_Simple`: ECX header, EDX value.
pub const SIMPLE: u32 = 0x003f_5cf0;
/// A colour from a shader function: cdecl, one argument, EAX the colour.
pub const COLOUR: u32 = 0x0013_1ed0;
/// D3D's render-state shadow, a dword per state.
pub const SHADOW: u32 = 0x0040_7498;
/// D3D's dirty mask for the deferred states.
pub const DIRTY: u32 = 0x0040_7490;
/// The guest D3D device pointer. Nothing here reads it; its last readers
/// are render-bridge's `native_world::opaque_material` uploads, which
/// still push to the guest device.
pub const DEVICE: u32 = 0x0040_7488;
/// State `0x88`'s mirror.
pub const STATE_88: u32 = 0x0040_76b8;
/// Push headers of the Simple states, dirty bits of the deferred ones.
pub const HEADERS: u32 = 0x0043_e6f0;
pub const MASKS: u32 = 0x0043_e4d0;
/// The state definitions entries index.
pub const DEFS: u32 = 0x0046_7020;
/// Type 0's scale (the float that turns 0..1 into 0..255).
pub const ALPHA_SCALE: u32 = 0x0045_dc38;
/// `ret 4`.
pub const RET_CLEANUP: u32 = 4;

/// The setters of states `0x89..=0xa5` (stdcall, one argument, D3D code).
pub const SETTERS: [u32; 29] = [
    0x003f_5c70,
    0x003f_5370,
    0x003f_5ba0,
    0x003f_5bf0,
    0x003f_5c10,
    0x003f_54a0,
    0x003f_54f0,
    0x003f_5590,
    0x003f_5640,
    0x003f_5450,
    0x003f_53d0,
    0x003f_56c0,
    0x003f_59b0,
    0x003f_5b40,
    0x003f_52f0,
    0x003f_5ac0,
    0x003f_5b00,
    0x003f_5a60,
    0x003f_5a90,
    0x003f_5330,
    0x003f_5730,
    0x003f_5b20,
    0x003f_57a0,
    0x003f_5830,
    0x003f_5870,
    0x003f_58e0,
    0x003f_5950,
    0x003f_5970,
    0x003f_5990,
];

/// Return addresses of the setter calls (states `0x89..=0xa5`) in the three
/// copies of the switch: the type-2 copy, the type-1 copy, and the one the
/// type-0 and colour values share; and in the first two, the register the
/// value is pushed from (0 EAX, 1 ECX, 2 EDX).
const SETTER_SITES: [(u32, u8, u32, u8, u32); 29] = [
    (0x1a281, 2, 0x1a579, 2, 0x1a92c),
    (0x1a298, 0, 0x1a590, 0, 0x1a93f),
    (0x1a2af, 1, 0x1a5a7, 1, 0x1a952),
    (0x1a2c6, 2, 0x1a5be, 2, 0x1a965),
    (0x1a2dd, 0, 0x1a5d5, 0, 0x1a978),
    (0x1a2f4, 1, 0x1a5ec, 1, 0x1a98b),
    (0x1a30b, 2, 0x1a603, 2, 0x1a99e),
    (0x1a322, 0, 0x1a61a, 0, 0x1a9b1),
    (0x1a339, 1, 0x1a631, 1, 0x1a9c4),
    (0x1a367, 0, 0x1a65f, 0, 0x1a9ea),
    (0x1a350, 2, 0x1a648, 2, 0x1a9d7),
    (0x1a37e, 1, 0x1a676, 1, 0x1a9fd),
    (0x1a395, 2, 0x1a68d, 2, 0x1aa10),
    (0x1a3ac, 0, 0x1a6a4, 0, 0x1aa23),
    (0x1a3c3, 1, 0x1a6bb, 1, 0x1aa36),
    (0x1a3da, 2, 0x1a6d2, 2, 0x1aa49),
    (0x1a3f1, 0, 0x1a6e9, 0, 0x1aa5c),
    (0x1a408, 1, 0x1a700, 1, 0x1aa6f),
    (0x1a41f, 2, 0x1a717, 2, 0x1aa82),
    (0x1a436, 0, 0x1a72e, 0, 0x1aa95),
    (0x1a44d, 1, 0x1a745, 1, 0x1aaa8),
    (0x1a464, 2, 0x1a75c, 2, 0x1aab8),
    (0x1a47b, 0, 0x1a773, 0, 0x1aac8),
    (0x1a492, 1, 0x1a78a, 1, 0x1aad8),
    (0x1a4a9, 2, 0x1a7a1, 2, 0x1aae8),
    (0x1a4c0, 0, 0x1a7b8, 0, 0x1aaf8),
    (0x1a4d7, 1, 0x1a7cf, 1, 0x1ab08),
    (0x1a4ee, 2, 0x1a7e6, 2, 0x1ab18),
    // 0xa5: every copy jumps to the one call at 0x1ab23.
    (0x1ab28, 0, 0x1ab28, 0, 0x1ab28),
];

/// The setter of `state`, if it has one.
pub fn setter(state: u32) -> Option<u32> {
    state
        .checked_sub(0x89)
        .and_then(|i| SETTERS.get(i as usize).copied())
}

/// Simple states whose push header is shared with other states (the
/// header table holds the same method for 0x36 and 0x54 to 0x5b): the GPU
/// register holds whichever of them was pushed last, so the shadow of one
/// says nothing about it. Never skipped. The differential test checks this
/// list against the image's table.
pub const SHARED_HEADER: [u32; 9] = [0x36, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b];

/// Bytes a callee pops.
pub fn cleanup(target: u32) -> u32 {
    if SETTERS.contains(&target) { 4 } else { 0 }
}

/// Whether the port pushes values D3D already holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Call for call the original.
    Exact,
    /// Skip `SetRenderState_Simple` when the shadow holds the value.
    SkipRedundant,
}

/// What one run did, for the counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// List entries applied.
    pub entries: u32,
    /// `SetRenderState_Simple` calls made.
    pub simple: u32,
    /// `SetRenderState_Simple` calls skipped (the shadow held the value).
    pub skipped: u32,
    /// States `0x5c..=0x88`, stored to the shadow and marked dirty.
    pub deferred: u32,
    /// Per-state setter calls (states `0x89..=0xa5`).
    pub setters: u32,
}

/// Which copy of the apply code a value goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Path {
    /// Type 2 (`fistp`), the value in the local at `[esp+0x10]`.
    Int,
    /// Type 1 (the float's bits), the value in `[esp+0xc]`.
    Bits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    /// After a Simple call: store the shadow (state, value).
    Simple {
        state: u32,
        value: u32,
    },
    Colour,
    /// After a setter: the next entry.
    Next,
}

/// `0x0001a170` in progress.
#[derive(Clone, Copy, Debug)]
pub struct RenderStates {
    at: At,
    entry_esp: u32,
    ret_to: u32,
    fcw: u16,
    mode: Mode,
    pub counts: Counts,
}

impl RenderStates {
    /// Enters at the first instruction: EBX the context, the list word's
    /// address on the stack, `fcw` the x87 control word.
    pub fn start(
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        fcw: u16,
        mode: Mode,
    ) -> Result<(RenderStates, Step), MemFault> {
        let mut s = RenderStates {
            at: At::Next,
            entry_esp: r.esp,
            ret_to: m.u32(r.esp)?,
            fcw,
            mode,
            counts: Counts::default(),
        };
        r.eax = m.u32(r.esp.wrapping_add(4))?;
        r.eax = m.u16(r.eax)? as u32;
        r.esp = r.esp.wrapping_sub(0x14);
        if r.eax & 0xfe00 == 0 {
            // jbe: straight to `add esp, 0x14; ret 4`.
            return Ok((s, s.leave(r)));
        }
        r.edx = m.u32(r.ebx.wrapping_add(0x20))?;
        r.edx = m.u32(r.edx.wrapping_add(0x3c))?;
        r.ecx = r.eax & 0x1ff;
        r.eax >>= 9;
        push(r, m, r.ebp)?;
        r.ebp = r.edx.wrapping_add(r.ecx.wrapping_mul(4));
        m.set_u32(r.esp.wrapping_add(0x10), 0)?;
        push(r, m, r.esi)?;
        push(r, m, r.edi)?;
        let step = s.entry(r, m)?;
        Ok((s, step))
    }

    /// Continues after the call [`Step::Call`] asked for returned.
    pub fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        match self.at {
            At::Simple { state, value } => {
                m.set_u32(SHADOW + 4 * state, value)?;
                self.next(r, m)
            }
            At::Colour => self.colour_returned(r, m),
            At::Next => self.next(r, m),
        }
    }

    fn call(&mut self, at: At, target: u32, site: u32) -> Result<Step, MemFault> {
        self.at = at;
        Ok(Step::Call {
            target,
            site,
            cleanup: cleanup(target),
        })
    }

    /// The loop body at `0x0001a1af` for the entry at EBP.
    fn entry(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        self.counts.entries += 1;
        let def = m.u8(r.ebp)? as u32;
        let source = m.u8(r.ebp.wrapping_add(1))?;
        r.eax = m.u8(r.ebp.wrapping_add(3))? as u32;
        r.edx = 0x4000;
        r.ecx = def.wrapping_mul(4).wrapping_add(DEFS);
        if source != 1 {
            // 0x0001a89c: a colour.
            r.eax = r.eax.wrapping_add(0x16).wrapping_mul(3);
            r.edx = r.ebx.wrapping_add(r.eax.wrapping_mul(4));
            push(r, m, r.edx)?;
            return self.call(At::Colour, COLOUR, 0x0001_a8ab);
        }
        r.xmm0 = m.u32(r.ebx.wrapping_add(r.eax.wrapping_mul(4)).wrapping_add(0x5c))? as u128;
        r.eax = m.u8(r.ecx.wrapping_add(3))? as u32;
        let local = r.esp.wrapping_add(0xc);
        m.set_u32(local, r.xmm0 as u32)?;
        let float = f32::from_bits(r.xmm0 as u32);
        match r.eax {
            2 => {
                r.eax = 0;
                let v = integer_value(float, self.fcw);
                m.set_u32(r.esp.wrapping_add(0x10), v)?;
                r.esi = m.u16(r.ecx)? as u32;
                self.apply_local(r, m, Path::Int)
            }
            1 => {
                r.eax = 0;
                r.esi = m.u16(r.ecx)? as u32;
                self.apply_local(r, m, Path::Bits)
            }
            0 => {
                // 0x0001a801
                r.xmm0 = m.u32(ALPHA_SCALE)? as u128;
                m.set_u32(r.esp.wrapping_add(0x1c), r.xmm0 as u32)?;
                m.set_u32(r.esp.wrapping_add(0x14), 0)?;
                let scale = f32::from_bits(r.xmm0 as u32);
                let v = alpha_value(float, scale, self.fcw);
                m.set_u32(r.esp.wrapping_add(0x14), v)?;
                r.edi = m.u16(r.ecx)? as u32;
                r.eax = v;
                let shadow = m.u32(SHADOW.wrapping_add(r.edi.wrapping_mul(4)))?;
                r.esi = ((shadow ^ r.eax) & 0x00ff_ffff) ^ r.eax;
                self.apply_merged(r, m)
            }
            _ => {
                // jne 0x0001ab28 with EAX = type - 2.
                r.eax = r.eax.wrapping_sub(2);
                self.next(r, m)
            }
        }
    }

    /// After the colour call (`0x0001a8ab`).
    fn colour_returned(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        r.edi = m.u16(r.ecx)? as u32;
        let at = SHADOW.wrapping_add(r.edi.wrapping_mul(4));
        r.esi = m.u32(at)?;
        r.ecx = m.u32(at)?;
        r.esi ^= r.eax;
        r.esi &= 0x00ff_ffff;
        r.esp = r.esp.wrapping_add(4);
        r.esi ^= r.ecx;
        if r.edi < 0x5c {
            return self.simple(r, m, r.edi, r.esi, 0x0001_a855);
        }
        if r.edi < 0x88 {
            // 0x0001a8da: ECX/EAX swapped against the other copies.
            r.ecx = m.u32(DIRTY)?;
            r.eax = m.u32(MASKS.wrapping_add(r.edi.wrapping_mul(4)))?;
            r.ecx |= r.eax;
            m.set_u32(DIRTY, r.ecx)?;
            m.set_u32(SHADOW.wrapping_add(r.edi.wrapping_mul(4)), r.esi)?;
            self.counts.deferred += 1;
            return self.next(r, m);
        }
        if r.edi == 0x88 {
            return self.state_88_tail(r, m, r.esi);
        }
        self.setter_merged(r, m)
    }

    /// Type 0 from `0x0001a842` (state EDI, value ESI).
    fn apply_merged(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        if r.edi < 0x5c {
            return self.simple(r, m, r.edi, r.esi, 0x0001_a855);
        }
        if r.edi < 0x88 {
            r.eax = m.u32(DIRTY)?;
            r.ecx = m.u32(MASKS.wrapping_add(r.edi.wrapping_mul(4)))?;
            r.eax |= r.ecx;
            m.set_u32(DIRTY, r.eax)?;
            m.set_u32(SHADOW.wrapping_add(r.edi.wrapping_mul(4)), r.esi)?;
            self.counts.deferred += 1;
            return self.next(r, m);
        }
        if r.edi == 0x88 {
            return self.state_88_tail(r, m, r.esi);
        }
        self.setter_merged(r, m)
    }

    /// `0x0001a909`: dirty bit 0x4000 and `0x004076b8`.
    fn state_88_tail(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        v: u32,
    ) -> Result<Step, MemFault> {
        let d = m.u32(DIRTY)? | 0x4000;
        m.set_u32(DIRTY, d)?;
        m.set_u32(STATE_88, v)?;
        self.next(r, m)
    }

    /// `0x0001a91e`: `push esi; call setter`.
    fn setter_merged(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        let Some(target) = setter(r.edi) else {
            return self.next(r, m);
        };
        let site = SETTER_SITES[(r.edi - 0x89) as usize].4;
        push(r, m, r.esi)?;
        self.counts.setters += 1;
        self.call(At::Next, target, site)
    }

    /// Types 2 and 1 (state ESI, value in a local).
    fn apply_local(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        copy: Path,
    ) -> Result<Step, MemFault> {
        let local = r
            .esp
            .wrapping_add(if copy == Path::Int { 0x10 } else { 0xc });
        let state = r.esi;
        if state < 0x5c {
            r.edi = m.u32(local)?;
            return self.simple(r, m, state, r.edi, 0x0001_a216);
        }
        if state < 0x88 {
            r.eax = m.u32(DIRTY)?;
            r.ecx = m.u32(MASKS.wrapping_add(state.wrapping_mul(4)))?;
            r.edx = m.u32(local)?;
            r.eax |= r.ecx;
            m.set_u32(DIRTY, r.eax)?;
            m.set_u32(SHADOW.wrapping_add(state.wrapping_mul(4)), r.edx)?;
            self.counts.deferred += 1;
            return self.next(r, m);
        }
        if state == 0x88 {
            r.eax = m.u32(local)?;
            let d = m.u32(DIRTY)? | r.edx;
            m.set_u32(DIRTY, d)?;
            m.set_u32(STATE_88, r.eax)?;
            return self.next(r, m);
        }
        let Some(target) = setter(state) else {
            return self.next(r, m);
        };
        let (s2, g2, s1, g1, _) = SETTER_SITES[(state - 0x89) as usize];
        let (site, reg) = if copy == Path::Int {
            (s2, g2)
        } else {
            (s1, g1)
        };
        let v = m.u32(local)?;
        match reg {
            0 => r.eax = v,
            1 => r.ecx = v,
            _ => r.edx = v,
        }
        push(r, m, v)?;
        self.counts.setters += 1;
        self.call(At::Next, target, site)
    }

    /// `SetRenderState_Simple(header, value)`, then the shadow; or, with
    /// [`Mode::SkipRedundant`] and the shadow already holding `value`,
    /// nothing.
    fn simple(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        state: u32,
        value: u32,
        site: u32,
    ) -> Result<Step, MemFault> {
        if self.mode == Mode::SkipRedundant
            && !SHARED_HEADER.contains(&state)
            && m.u32(SHADOW.wrapping_add(state.wrapping_mul(4)))? == value
        {
            self.counts.skipped += 1;
            return self.next(r, m);
        }
        r.ecx = m.u32(HEADERS.wrapping_add(state.wrapping_mul(4)))?;
        r.edx = value;
        self.counts.simple += 1;
        self.call(At::Simple { state, value }, SIMPLE, site)
    }

    /// `0x0001ab28`: the next entry, or the epilogue.
    fn next(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        r.edx = m.u32(r.esp.wrapping_add(0x24))?;
        r.ecx = m.u16(r.edx)? as u32;
        r.eax = m.u32(r.esp.wrapping_add(0x18))?;
        r.ebp = r.ebp.wrapping_add(4);
        r.eax = r.eax.wrapping_add(1);
        r.ecx >>= 9;
        m.set_u32(r.esp.wrapping_add(0x18), r.eax)?;
        if (r.eax as i32) < (r.ecx as i32) {
            return self.entry(r, m);
        }
        r.edi = pop(r, m)?;
        r.esi = pop(r, m)?;
        r.ebp = pop(r, m)?;
        Ok(self.leave(r))
    }

    fn leave(&self, r: &mut Regs) -> Step {
        r.esp = r.esp.wrapping_add(0x14);
        debug_assert_eq!(r.esp, self.entry_esp);
        r.esp = r.esp.wrapping_add(4 + RET_CLEANUP);
        Step::Return { to: self.ret_to }
    }
}

fn push(r: &mut Regs, m: &mut dyn GuestMemory, v: u32) -> Result<(), MemFault> {
    r.esp = r.esp.wrapping_sub(4);
    m.set_u32(r.esp, v)
}

fn pop(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let v = m.u32(r.esp)?;
    r.esp = r.esp.wrapping_add(4);
    Ok(v)
}

/// Integer parameter conversion used by the render and sampler binding lists.
pub fn integer_value(value: f32, fcw: u16) -> u32 {
    fistp(f64::from(value), fcw)
}

/// Scaled alpha parameter, in the high byte of a D3D colour word.
pub fn alpha_value(value: f32, scale: f32, fcw: u16) -> u32 {
    let product = round_precision(f64::from(value) * f64::from(scale), fcw);
    fistp(product, fcw) << 24
}

/// Rounds an integer-valued-or-not `x` by the control word's rounding
/// control.
fn round_rc(x: f64, fcw: u16) -> f64 {
    match (fcw >> 10) & 3 {
        0 => x.round_ties_even(),
        1 => x.floor(),
        2 => x.ceil(),
        _ => x.trunc(),
    }
}

/// An exact product rounded to the control word's precision (24, 53 or 64
/// bits; the exponent range is the x87's, so no overflow here).
fn round_precision(p: f64, fcw: u16) -> f64 {
    let bits = match (fcw >> 8) & 3 {
        0 => 24,
        // 53 and 64 bits hold a product of two floats exactly.
        _ => return p,
    };
    if p == 0.0 || !p.is_finite() {
        return p;
    }
    let e = p.abs().log2().floor() as i32;
    // Scale so the significand's bits are the integer part; guard against
    // log2 landing one off at a power-of-two boundary.
    let mut shift = bits - 1 - e;
    let mut q = p * 2f64.powi(shift);
    if q.abs() >= 2f64.powi(bits) {
        shift -= 1;
        q = p * 2f64.powi(shift);
    } else if q.abs() < 2f64.powi(bits - 1) {
        shift += 1;
        q = p * 2f64.powi(shift);
    }
    round_rc(q, fcw) * 2f64.powi(-shift)
}

/// `fistp m32`: rounded by RC; out of range or NaN gives the integer
/// indefinite.
fn fistp(x: f64, fcw: u16) -> u32 {
    if x.is_nan() {
        return 0x8000_0000;
    }
    let r = round_rc(x, fcw);
    if r < -2147483648.0 || r > 2147483647.0 {
        return 0x8000_0000;
    }
    r as i32 as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fistp_rounds_by_the_control_word() {
        assert_eq!(fistp(2.5, 0x037f), 2);
        assert_eq!(fistp(3.5, 0x037f), 4);
        assert_eq!(fistp(-2.5, 0x077f), -3i32 as u32);
        assert_eq!(fistp(2.1, 0x0b7f), 3);
        assert_eq!(fistp(-2.9, 0x0f7f), -2i32 as u32);
        assert_eq!(fistp(f64::NAN, 0x037f), 0x8000_0000);
        assert_eq!(fistp(3e9, 0x037f), 0x8000_0000);
        assert_eq!(fistp(-2147483648.0, 0x037f), 0x8000_0000);
    }

    #[test]
    fn single_precision_rounds_the_product() {
        // 24-bit precision: (1 + 2^-23) * (1 + 2^-23) = 1 + 2^-22 + 2^-46.
        let a = 1.0 + 2f64.powi(-23);
        assert_eq!(round_precision(a * a, 0x007f), 1.0 + 2f64.powi(-22));
        assert_eq!(
            round_precision(a * a, 0x087f),
            1.0 + 2f64.powi(-22) + 2f64.powi(-23)
        );
        assert_eq!(round_precision(a * a, 0x037f), a * a);
        assert_eq!(round_precision(255.0, 0x007f), 255.0);
    }
}
