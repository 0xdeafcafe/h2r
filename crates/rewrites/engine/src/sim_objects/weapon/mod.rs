//! The weapon update `fdfb0` (slice 9 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 9 body"): the `weap`
//! descriptor's slot `+40`. The type dispatcher `108c60` runs the item
//! update `10b4d0` first, then this.
//!
//! **`fdfb0 (object: stack 0) -> al`; `ret 4`.** One body, written in three
//! files by state group, each a run of retail labels:
//!
//! - [`state`] `fdfb0..fe5ff`: the frame, the weapon-wide state (the
//!   owner, the animation block, the timers and charge at body
//!   `+17c..+19e`, the heat at `+180..+188`, the effect at `+244`), and the
//!   loop over the definition's `+2c0` entries (`fdd50` each).
//! - [`triggers`] `fe5ff..fefee`: the loop over the definition's triggers
//!   (`+2c8` count, `+2cc` records of `40` bytes), body state `0c` bytes
//!   each at `+20c`, a state machine of eight states through the jump
//!   tables at `ff57c`, `ff59c`, `ff5b4` and `ff5c4`.
//! - [`barrels`] `fefee..ff576`: the loop over the definition's barrels
//!   (`+2d0` count, `+2d4` records of `ec` bytes), body state `34` bytes
//!   each at `+1a4`, four states through the table at `ff5d4`, the x87
//!   sequences, and the return.
//!
//! **Frame.** `sub esp, 38`, then `ebx`, `ebp`, `esi`, `edi` pushed. With
//! `S` the `esp` after the four pushes: `[S+13]` the result byte, `[S+14]`
//! to `[S+22]` per-trigger and per-barrel flag bytes and the out bytes the
//! callees fill, `[S+24]` the object's header offset (index × `c`),
//! `[S+28]` the trigger or barrel index (a float scratch before the
//! trigger loop), `[S+2c]` the definition, `[S+30]` the owner's handle or
//! none (a float scratch in the barrel loop), `[S+34]` the body pointer
//! as read at entry (the barrel loop indexes it even after a callee moved
//! the body, as retail does), `[S+38]` the value `d1490` answered in
//! `xmm0`, `[S+3c..+48]` out words, and the object at `[S+4c]`.
//!
//! Registers across calls: `ebp` the body until the barrel loop, then the
//! barrel definition; `ebx` the result byte (`bl`) in the state group, the
//! trigger offset (index × `c`) in the trigger loop and the object in the
//! barrel loop; `esi` the trigger definition, then the barrel state;
//! `edi` the objects global's header, then the definition, then the
//! object or the barrel index. Every value is where retail keeps it, so the
//! lifted original can take over at any site and the reverse.
//!
//! Exit: `al` the result byte over the last callee's `eax`; `ecx` and
//! `edx` as the barrel loop left them; flags from `add esp, 38`.

mod barrels;
mod state;
mod triggers;

use super::sse;
use super::{Cx, Next, Result, unowned};
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, status};
use runtime::{CpuAbi, Guest};
use x87::abi as fp;

const NONE: u32 = u32::MAX;

/// Where each state group starts.
const TRIGGERS: u32 = 0xfe5ff;
const BARRELS: u32 = 0xfefee;

/// binary32 `1.0` and `0.0` in the image.
const ONE: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;

/// Every return address of `fdfb0`'s calls, address order.
pub(super) const WEAPON_SITES: [u32; 77] = [
    0xfdffc, 0xfe022, 0xfe067, 0xfe09b, 0xfe0ab, 0xfe0fe, 0xfe115, 0xfe128, 0xfe142, 0xfe1be, 0xfe1d8, 0xfe274,
    0xfe332, 0xfe33e, 0xfe36a, 0xfe436, 0xfe475, 0xfe4f1, 0xfe504, 0xfe50d, 0xfe5eb, 0xfe6d0, 0xfe737, 0xfe76f,
    0xfe7b0, 0xfe7fd, 0xfe846, 0xfe87c, 0xfe89c, 0xfe8ac, 0xfe8e3, 0xfe970, 0xfe980, 0xfe9b0, 0xfe9f1, 0xfea36,
    0xfea6f, 0xfea7f, 0xfea8d, 0xfeace, 0xfeb14, 0xfeb4c, 0xfeb5c, 0xfeb71, 0xfeb7d, 0xfeba5, 0xfebb0, 0xfebef,
    0xfec0f, 0xfec23, 0xfecd0, 0xfeced, 0xfed19, 0xfed35, 0xfed7c, 0xfedc4, 0xfedd8, 0xfee06, 0xfee64, 0xfee99,
    0xfeeaa, 0xfeeef, 0xfef72, 0xfef90, 0xfefb0, 0xff152, 0xff17f, 0xff196, 0xff1b0, 0xff1c2, 0xff1d4, 0xff1ec,
    0xff213, 0xff224, 0xff2f7, 0xff39f, 0xff40f,
];

/// Where a group's arm goes: another label of the body, or out.
enum Step {
    Go(u32),
    Out(Next),
}

type R = Result<Step>;

/// `fdfb0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn weapon_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::WEAPON_UPDATE.base && WEAPON_SITES.binary_search(&pc).is_err() {
        return Err(unowned(pc));
    }
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let mut pc = pc;
    loop {
        let step = if pc < TRIGGERS {
            state::step(c, g, pc)?
        } else if pc < BARRELS {
            triggers::step(c, g, pc)?
        } else {
            barrels::step(c, g, pc)?
        };
        match step {
            Step::Go(next) => pc = next,
            Step::Out(next) => return Ok(next),
        }
    }
}

fn go(pc: u32) -> R {
    Ok(Step::Go(pc))
}

fn call(target: u32, site: u32) -> R {
    Ok(Step::Out(Next::Call { target, site }))
}

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

/// `[esp + off]`.
fn slot(c: &CpuAbi, g: &Guest, off: u32) -> Result<u32> {
    g.u32(at(c.esp, off))
}

fn set_slot(c: &CpuAbi, g: &mut Guest, off: u32, v: u32) -> Result<()> {
    g.set_u32(at(c.esp, off), v)
}

/// `byte [esp + off]`.
fn slot8(c: &CpuAbi, g: &Guest, off: u32) -> Result<u8> {
    g.u8(at(c.esp, off))
}

fn set_slot8(c: &CpuAbi, g: &mut Guest, off: u32, v: u8) -> Result<()> {
    g.set_u8(at(c.esp, off), v)
}

/// `push word`.
fn push(c: &mut CpuAbi, g: &mut Guest, word: u32) -> Result<()> {
    crate::sim_physics::push(c, g, word)
}

/// The low word of `reg` replaced (`mov r16, m16`).
fn word(reg: u32, v: u16) -> u32 {
    (reg & 0xffff_0000) | u32::from(v)
}

/// `test r8, r8` (or `test r8, imm` on the masked value): true when
/// non-zero.
fn test8(c: &mut CpuAbi, v: u32) -> bool {
    let v = v & 0xff;
    logic(c, v, 0x80);
    v != 0
}

/// `test r32, r32`.
fn test32(c: &mut CpuAbi, v: u32) -> bool {
    logic(c, v, 1 << 31);
    v != 0
}

/// `test byte [addr], k`.
fn test_m8(c: &mut CpuAbi, g: &Guest, addr: u32, k: u8) -> Result<bool> {
    Ok(test8(c, u32::from(g.u8(addr)? & k)))
}

/// `cmp a32, b32`: true when equal.
fn cmp32(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    arithmetic(c, a, b, true);
    a == b
}

/// `cmp a8, b8`: true when equal.
fn cmp8(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    let (a, b) = (a & 0xff, b & 0xff);
    let v = a.wrapping_sub(b) & 0xff;
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x80) | cf | ((a ^ b ^ v) & 0x10) | of;
    c.eflags_defined |= ARITH;
    a == b
}

/// `cmp word [addr], k`: true when equal.
fn cmp_m16(c: &mut CpuAbi, g: &Guest, addr: u32, k: u32) -> Result<bool> {
    let v = u32::from(g.u16(addr)?);
    super::alu::cmp16(c, v, k);
    Ok(v == k & 0xffff)
}

/// `jl`: SF not equal to OF.
fn less(c: &CpuAbi) -> bool {
    (c.eflags & 0x80 != 0) != (c.eflags & 0x800 != 0)
}

/// `jle`.
fn less_or_equal(c: &CpuAbi) -> bool {
    super::alu::less_or_equal(c)
}

/// `ja`: CF and ZF clear.
fn above(c: &CpuAbi) -> bool {
    c.eflags & 0x41 == 0
}

/// `or`/`and dword [addr], k` (`f` the operation): the store, with its
/// flags.
fn rmw32(c: &mut CpuAbi, g: &mut Guest, addr: u32, f: impl FnOnce(u32) -> u32) -> Result<u32> {
    let v = f(g.u32(addr)?);
    g.set_u32(addr, v)?;
    logic(c, v, 1 << 31);
    Ok(v)
}

/// `or`/`and byte [addr], k`.
fn rmw8(c: &mut CpuAbi, g: &mut Guest, addr: u32, f: impl FnOnce(u8) -> u8) -> Result<u8> {
    super::alu::rmw8(c, g, addr, f)
}

/// `lahf; test ah, 44h`: whether the `jp` after it jumps (the compare
/// before was unordered or not equal).
fn lahf_jp(c: &mut CpuAbi) -> bool {
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    sse::parity(c)
}

/// Store lane 0 of `xmm r` at `addr` (`movss [addr], xmm`).
fn store_ss(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

/// The guest's x87 state as the `x87` crate's.
fn x87_in(c: &CpuAbi) -> fp::X87Abi {
    fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        opcode: c.x87.opcode,
        instruction_pointer: c.x87.instruction_pointer,
        data_pointer: c.x87.data_pointer,
        instruction_selector: c.x87.instruction_selector,
        data_selector: c.x87.data_selector,
    }
}

/// Write `x` back, with the last-instruction registers of the x87
/// instruction at `pc` and the last memory operand `data`.
fn x87_out(c: &mut CpuAbi, g: &Guest, x: &fp::X87Abi, pc: u32, data: u32) -> Result<()> {
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = pc;
    c.x87.instruction_selector = 8;
    // FOP: the low three bits of the opcode byte and the ModR/M byte.
    c.x87.opcode = g.u16(pc)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = data;
    c.x87.data_selector = 16;
    Ok(())
}

/// An x87 operation's status: every exception is masked where bodies run
/// (`super::supported`), so anything but a commit is a fault.
fn fp_ok(code: u32, pc: u32) -> Result<()> {
    if code & fp::status::MASK == fp::status::OK { Ok(()) } else { Err(unowned(pc)) }
}

/// `fld qword [addr]`.
fn fld_f64(x: &mut fp::X87Abi, g: &Guest, addr: u32, pc: u32) -> Result<()> {
    let bits = u64::from(g.u32(addr)?) | (u64::from(g.u32(at(addr, 4))?) << 32);
    fp_ok(fp::ld_f64(x, bits), pc)
}

/// `fld dword [addr]`.
fn fld_f32(x: &mut fp::X87Abi, g: &Guest, addr: u32, pc: u32) -> Result<()> {
    fp_ok(fp::ld_f32(x, g.u32(addr)?), pc)
}

/// `fstp dword [addr]`.
fn fstp_f32(x: &mut fp::X87Abi, g: &mut Guest, addr: u32, pc: u32) -> Result<()> {
    let (code, store) = fp::st_f32(x, 0);
    fp_ok(code, pc)?;
    g.set_u32(addr, store.value)?;
    fp::store_commit(x, store.status, true);
    Ok(())
}

/// `fcomip st, st(1); fstp st(0)`: EFLAGS as `fcomip` leaves them (ZF, PF,
/// CF; OF, SF and AF cleared).
fn fcomip_fstp(c: &mut CpuAbi, x: &mut fp::X87Abi, pc: u32) -> Result<()> {
    let f = fp::cmp(x, 0, fp::src_kind::ST, 1, 1, true);
    fp_ok(f, pc)?;
    c.eflags = (c.eflags & !ARITH) | (f & 0x45);
    c.eflags_defined |= ARITH;
    fp_ok(fp::st_st(x, 0, true), pc)
}

/// The body pointer re-read through the header: `[[objects]+44] +
/// [esp+24] + 8`.
fn body_now(c: &CpuAbi, g: &Guest) -> Result<u32> {
    let data = g.u32(at(g.u32(points::OBJECTS.base)?, guest_layout::datum::layout::DATA))?;
    g.u32(at(at(data, slot(c, g, 0x24)?), 8))
}

/// `al` into the low byte of `eax`.
fn set_al(c: &mut CpuAbi, v: u8) {
    c.eax = byte(c.eax, v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_are_sorted_and_split_by_group() {
        assert!(WEAPON_SITES.windows(2).all(|w| w[0] < w[1]));
        assert!(WEAPON_SITES.iter().all(|&s| s > points::WEAPON_UPDATE.base && s < 0xff576));
        // Each group owns the sites in its range.
        let (state, rest): (Vec<u32>, Vec<u32>) = WEAPON_SITES.iter().partition(|&&s| s < TRIGGERS);
        let triggers = rest.iter().filter(|&&s| s < BARRELS).count();
        assert_eq!((state.len(), triggers, rest.len() - triggers), (21, 44, 12));
    }

    #[test]
    fn byte_compares_see_the_low_byte() {
        let mut c = CpuAbi::default();
        assert!(cmp8(&mut c, 0x1207, 7));
        assert_eq!(c.eflags & 0x41, 0x40);
        assert!(!cmp8(&mut c, 0, 1));
        assert_eq!(c.eflags & 0x81, 0x81);
        assert!(less(&c));
        assert!(!test8(&mut c, 0x100));
        assert!(test8(&mut c, 0x80));
        assert_eq!(c.eflags & 0x80, 0x80);
        // `test` clears CF, so `ja` after it is "non-zero".
        assert!(above(&c));
        assert!(!test8(&mut c, 0));
        assert!(!above(&c));
    }
}
