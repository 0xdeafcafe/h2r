//! A local player's event announcement `19e890` (`GAME_EVENT_ANNOUNCE`,
//! `player: eax, row: ecx, event: stack 0`, `ret 4`), a seat body over the
//! retail frame (docs/engine/sim-rewrite-sketches.md, "The respawn gate").
//! The respawn countdown reaches it for a local player's death event.
//!
//! | Step | Guest | Body |
//! |---|---|---|
//! | no local user (`+0x28` is `-1`), or announcements quiet (`[[4e6948] + 0x1128]`) for all but type 0 kind `0x13` | out | the same |
//! | the row's first text (`+0xc`, not 0 or `-1`) | `1a0180` (the string), `19e0f0` (formatted with the event) | the calls |
//! | row flag bit 0 | the second text (`+0x1c`), formatted, then `24caac` (both lines) | the calls |
//! | else | `24cbee` (one line) | the call |
//! | the row's timed text (`+0x14`, not 0 or `-1`) | `1a0180`, `19e0f0`, then `24cc73` for `[row + 0x18]` seconds (`cvtsi2ss`, `mulss` by the tick rate, `fld`/`fistp`) | the calls; the product and its `fistp` in Rust |
//! | always, once past the gates | the announcer `19e770 (event)` | the call |
//!
//! At every call and the return the registers, flags, x87 state and frame
//! are retail's. The string, format, HUD and sound callees are guest
//! calls. The body declines only on the float environment
//! ([`crate::sim_native::plan`]); the seat installer panics on it by name.

use crate::sim_native::{Memory, plan::segment, sub_flags};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use x87::abi as fp;

pub const ENTRY: u32 = 0x0019_e890;
const END: u32 = 0x0019_ea57;

const TEXT_DONE: u32 = 0x0019_e93f;
const FORMAT_DONE: u32 = 0x0019_e958;
const SECOND_TEXT_DONE: u32 = 0x0019_e977;
const SECOND_FORMAT_DONE: u32 = 0x0019_e98d;
const LINES_DONE: u32 = 0x0019_e9ae;
const LINE_DONE: u32 = 0x0019_e9c5;
const TIMED_TEXT_DONE: u32 = 0x0019_e9ef;
const TIMED_FORMAT_DONE: u32 = 0x0019_ea05;
const TIMED_DONE: u32 = 0x0019_ea44;
const SOUND_DONE: u32 = 0x0019_ea4a;
pub const SITES: &[u32] = &[
    TEXT_DONE, FORMAT_DONE, SECOND_TEXT_DONE, SECOND_FORMAT_DONE, LINES_DONE, LINE_DONE,
    TIMED_TEXT_DONE, TIMED_FORMAT_DONE, TIMED_DONE, SOUND_DONE,
];

const STRING: u32 = 0x001a_0180;
const FORMAT: u32 = 0x0019_e0f0;
const TWO_LINES: u32 = 0x0024_caac;
const ONE_LINE: u32 = 0x0024_cbee;
const TIMED_LINE: u32 = 0x0024_cc73;
const SOUND: u32 = 0x0019_e770;

const SETTINGS: u32 = 0x004e_034c;
const PLAYERS: u32 = 0x004e_8c24;
const TAGS: u32 = 0x004e_3b44;
const MODE: u32 = 0x004e_6948;
const TICKS: u32 = 0x0051_0c54;
const STRIDE: u32 = 0x21c;
const FRAME: u32 = 0x80c;
const NONE: u32 = u32::MAX;
/// `fistp dword [esp+18h]` at `19ea27` (`db 5c 24 18`): its FOP.
const FISTP_AT: u32 = 0x0019_ea27;
const FISTP_OP: u16 = 0x35c;

type Result<T> = std::result::Result<T, GuestFault>;

#[inline]
fn at(p: u32, n: u32) -> u32 {
    p.wrapping_add(n)
}

fn movsx16(g: &impl Memory, a: u32) -> Result<u32> {
    Ok(g.u16(a)? as i16 as i32 as u32)
}

fn float_fault(at: u32) -> GuestFault {
    GuestFault {
        addr: at,
        len: 4,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// `pop edi` at `19ea4a`, then [`leave`].
fn leave_edi(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    leave(c, g)
}

/// `pop esi; pop ebp; pop ebx; add esp, 80ch; ret 4` at `19ea4b`.
fn leave(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    g.ret(c, 4)
}

fn entry(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    c.esp = arithmetic(c, c.esp, FRAME, true);
    c.edx = g.u32(SETTINGS)?;
    push(c, g, c.ebx)?;
    c.eax &= 0xffff;
    push(c, g, c.ebp)?;
    c.eax = c.eax.wrapping_mul(STRIDE);
    c.ebp = g.u32(at(c.esp, 0x818))?;
    push(c, g, c.esi)?;
    c.esi = c.ecx;
    c.ecx = g.u32(PLAYERS)?;
    c.ebx = g.u32(at(c.ecx, 0x44))?;
    c.ecx = g.u32(at(c.edx, 0x16c))?;
    c.edx = g.u32(TAGS)?;
    c.ecx &= 0xffff;
    c.eax = arithmetic(c, c.eax, c.ebx, false);
    c.ecx <<= 4;
    let local = g.u16(at(c.eax, 0x28))?;
    sub_flags(c, u32::from(local), 0xffff, 16);
    c.ecx = g.u32(at(at(c.ecx, c.edx), 8))?;
    c.ebx = g.u32(at(c.ecx, 0xc))?;
    g.set_u32(at(c.esp, 0x10), c.eax)?;
    if local == 0xffff {
        return leave(c, g);
    }
    c.edx = g.u32(MODE)?;
    let quiet = g.u8(at(c.edx, 0x1128))?;
    c.eax = (c.eax & !0xff) | u32::from(quiet);
    logic(c, u32::from(quiet), 0x80);
    if quiet != 0 {
        let kind = g.u32(c.ebp)?;
        arithmetic(c, kind, 0, true);
        if kind != 0 {
            return leave(c, g);
        }
        let kind = g.u32(at(c.ebp, 4))?;
        arithmetic(c, kind, 0x13, true);
        if kind != 0x13 {
            return leave(c, g);
        }
    }
    push(c, g, c.edi)?;
    c.edi = g.u32(at(c.esi, 0xc))?;
    if text_absent(c).is_some() {
        return timed(c, g);
    }
    // `lea eax, [esp+21Ch]; push eax`, the string's buffer.
    c.eax = at(c.esp, 0x21c);
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.ebx, 0x8c))?;
    g.set_u16(at(c.esp, 0x220), 0)?;
    g.call(c, STRING, TEXT_DONE)
}

/// `cmp edi, -1; je; test edi, edi; je`: `Some` when the text is absent.
fn text_absent(c: &mut CpuAbi) -> Option<()> {
    arithmetic(c, c.edi, NONE, true);
    if c.edi == NONE {
        return Some(());
    }
    logic(c, c.edi, 1 << 31);
    (c.edi == 0).then_some(())
}

/// `19e9c5`: the timed text, or straight to the announcer.
fn timed(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    c.edi = g.u32(at(c.esi, 0x14))?;
    if text_absent(c).is_some() {
        return sound(c, g);
    }
    c.edx = g.u32(at(c.esi, 0x18))?;
    c.eax = at(c.esp, 0x1c);
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.ebx, 0x8c))?;
    g.set_u32(at(c.esp, 0x14), c.edx)?;
    g.set_u16(at(c.esp, 0x20), 0)?;
    g.call(c, STRING, TIMED_TEXT_DONE)
}

/// `19ea44`: `push ebp; call 19e770`.
fn sound(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    push(c, g, c.ebp)?;
    g.call(c, SOUND, SOUND_DONE)
}

/// `push ebp; lea eax, [esp+eax_at]; mov edx, 100h; lea ecx, [esp+ecx_at];
/// call 19e0f0`.
fn format(c: &mut CpuAbi, g: &mut impl Memory, eax_at: u32, ecx_at: u32, site: u32) -> Result<TransferAbi> {
    push(c, g, c.ebp)?;
    c.eax = at(c.esp, eax_at);
    c.edx = 0x100;
    c.ecx = at(c.esp, ecx_at);
    g.call(c, FORMAT, site)
}

/// `19ea05..19ea2b`: the seconds `[row + 0x18]` times the tick rate, as
/// `cvtsi2ss`, `mulss`, `movss`, `fld` and `fistp` leave them.
fn timed_ticks(c: &mut CpuAbi, g: &mut impl Memory) -> Result<u32> {
    c.ecx = g.u32(TICKS)?;
    c.edx = movsx16(g, at(c.ecx, 2))?;
    sse::cvtsi2ss(c, 1, g.u32(at(c.esp, 0x10))?);
    sse::cvtsi2ss(c, 0, c.edx);
    let product = sse::ss_reg(c, Ss::Mul, 0, 1);
    g.set_u32(at(c.esp, 0x10), product)?;
    let mut x = fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        opcode: c.x87.opcode,
        instruction_pointer: c.x87.instruction_pointer,
        data_pointer: c.x87.data_pointer,
        instruction_selector: c.x87.instruction_selector,
        data_selector: c.x87.data_selector,
    };
    // `supported` masks every x87 exception with none pending, so neither
    // step can ask for delivery.
    if fp::ld_f32(&mut x, product) & fp::status::MASK != fp::status::OK {
        return Err(float_fault(FISTP_AT));
    }
    let (code, store) = fp::st_i32(&x, 0);
    if code & fp::status::MASK != fp::status::OK {
        return Err(float_fault(FISTP_AT));
    }
    let dst = at(c.esp, 0x18);
    g.set_u32(dst, store.value as u32)?;
    fp::store_commit(&mut x, store.status, true);
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = FISTP_AT;
    c.x87.opcode = FISTP_OP;
    c.x87.data_pointer = dst;
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    Ok(store.value as u32)
}

fn select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    match pc {
        ENTRY => entry(c, g),
        TEXT_DONE => format(c, g, 0x420, 0x220, FORMAT_DONE),
        FORMAT_DONE => {
            let flags = g.u8(c.esi)? & 1;
            logic(c, u32::from(flags), 0x80);
            if flags != 0 {
                c.edi = g.u32(at(c.esi, 0x1c))?;
                c.eax = g.u32(at(c.ebx, 0x8c))?;
                c.ecx = at(c.esp, 0x1c);
                push(c, g, c.ecx)?;
                g.set_u16(at(c.esp, 0x20), 0)?;
                return g.call(c, STRING, SECOND_TEXT_DONE);
            }
            c.ecx = g.u32(at(c.esp, 0x14))?;
            c.eax = at(c.esp, 0x41c);
            push(c, g, c.eax)?;
            c.eax = movsx16(g, at(c.ecx, 0x28))?;
            g.call(c, ONE_LINE, LINE_DONE)
        }
        SECOND_TEXT_DONE => format(c, g, 0x620, 0x20, SECOND_FORMAT_DONE),
        SECOND_FORMAT_DONE => {
            c.edx = g.u32(at(c.ebp, 0x1c))?;
            push(c, g, c.edx)?;
            c.edx = g.u32(at(c.esp, 0x18))?;
            c.eax = at(c.esp, 0x620);
            push(c, g, c.eax)?;
            c.eax = movsx16(g, at(c.edx, 0x28))?;
            c.ecx = at(c.esp, 0x424);
            push(c, g, c.ecx)?;
            g.call(c, TWO_LINES, LINES_DONE)
        }
        LINES_DONE | LINE_DONE => timed(c, g),
        TIMED_TEXT_DONE => format(c, g, 0x220, 0x20, TIMED_FORMAT_DONE),
        TIMED_FORMAT_DONE => {
            c.eax = timed_ticks(c, g)?;
            c.ecx = g.u32(at(c.esp, 0x14))?;
            push(c, g, c.eax)?;
            c.eax = movsx16(g, at(c.ecx, 0x28))?;
            c.ecx = at(c.esp, 0x220);
            g.call(c, TIMED_LINE, TIMED_DONE)
        }
        TIMED_DONE => sound(c, g),
        SOUND_DONE => leave_edi(c, g),
        _ => unreachable!("undeclared announcement continuation {pc:#x}"),
    }
}

/// One announcement segment.
pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    segment::<16, _>(c, g, pc, ENTRY..END, SITES, |c, p, pc| select(c, p, pc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::abi;
    use runtime::guest::Permissions;

    const HEADER: u32 = 0x0060_0000;
    const PLAYER_DATA: u32 = 0x0060_0100;
    const TAG_ROWS: u32 = 0x0060_2000;
    const DEFINITION: u32 = 0x0060_3000;
    const GLOBALS: u32 = 0x0060_4000;
    const MODE_RECORD: u32 = 0x0060_5000;
    const TICK_RECORD: u32 = 0x0060_6000;
    const ROW: u32 = 0x0060_7000;
    const EVENT: u32 = 0x0060_8000;
    const STACK: u32 = 0x007f_0000;
    const RETURN: u32 = 0x0012_3456;
    const SAVED: [u32; 4] = [0xb0b0_0001, 0xb0b0_0002, 0xb0b0_0003, 0xb0b0_0004];

    /// Player 1 with local user 0, a row with a first text and no flags,
    /// announcements not quiet.
    fn world() -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0019_0000, 0x0001_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x004e_0000, 0x0004_0000, Permissions::RW).unwrap();
        g.map_zeroed(HEADER, 0x0001_0000, Permissions::RW).unwrap();
        g.map_zeroed(STACK - 0x2000, 0x3000, Permissions::RW).unwrap();
        g.set_u32(PLAYERS, HEADER).unwrap();
        g.set_u32(HEADER + 0x44, PLAYER_DATA).unwrap();
        g.set_u16(PLAYER_DATA + STRIDE + 0x28, 0).unwrap();
        g.set_u32(SETTINGS, GLOBALS).unwrap();
        g.set_u32(GLOBALS + 0x16c, 0xe000_0002).unwrap();
        g.set_u32(TAGS, TAG_ROWS).unwrap();
        g.set_u32(TAG_ROWS + 2 * 16 + 8, DEFINITION).unwrap();
        g.set_u32(DEFINITION + 0xc, DEFINITION + 0x100).unwrap();
        g.set_u32(DEFINITION + 0x100 + 0x8c, 0x77).unwrap();
        g.set_u32(MODE, MODE_RECORD).unwrap();
        g.set_u32(TICKS, TICK_RECORD).unwrap();
        g.set_u16(TICK_RECORD + 2, 30).unwrap();
        g.set_u32(ROW + 0xc, 0x0042_0001).unwrap();
        g.set_u32(ROW + 0x14, 0).unwrap();
        g.set_u32(STACK, RETURN).unwrap();
        g.set_u32(STACK + 4, EVENT).unwrap();
        let mut cpu = CpuAbi {
            eax: 0xe000_0001,
            ecx: ROW,
            esp: STACK,
            ebx: SAVED[0],
            ebp: SAVED[1],
            esi: SAVED[2],
            edi: SAVED[3],
            mxcsr: 0x1f80,
            ..Default::default()
        };
        cpu.x87.control = 0x027f;
        cpu.x87.tag = 0xffff;
        (cpu, g)
    }

    /// The callee's `ret pop`.
    fn back(cpu: &mut CpuAbi, pop: u32) {
        cpu.esp += 4 + pop;
        cpu.eflags_defined = 0;
    }

    fn step(cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> TransferAbi {
        run(cpu, g, pc).unwrap().expect("the body answers")
    }

    #[test]
    fn one_line_then_the_announcer_then_the_return() {
        let (mut cpu, mut g) = world();
        let t = step(&mut cpu, &mut g, ENTRY);
        assert_eq!((t.kind, t.target_pc, t.resume_pc), (abi::CALL, STRING, TEXT_DONE));
        // `edi` the string id, `eax` the definition's `+0x8c`, the buffer
        // pushed and its first character cleared.
        let frame = STACK - 0x80c - 16;
        assert_eq!((cpu.edi, cpu.eax, cpu.esp), (0x0042_0001, 0x77, frame - 8));
        assert_eq!(g.u32(cpu.esp + 4).unwrap(), frame + 0x21c);
        assert_eq!(g.u32(frame + 0x14).unwrap(), PLAYER_DATA + STRIDE);
        back(&mut cpu, 4);
        let t = step(&mut cpu, &mut g, TEXT_DONE);
        assert_eq!((t.target_pc, t.resume_pc), (FORMAT, FORMAT_DONE));
        assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (frame + 0x41c, frame + 0x21c, 0x100));
        assert_eq!(g.u32(cpu.esp + 4).unwrap(), EVENT);
        back(&mut cpu, 4);
        let t = step(&mut cpu, &mut g, FORMAT_DONE);
        assert_eq!((t.target_pc, t.resume_pc), (ONE_LINE, LINE_DONE));
        assert_eq!((cpu.eax, g.u32(cpu.esp + 4).unwrap()), (0, frame + 0x41c));
        back(&mut cpu, 4);
        let t = step(&mut cpu, &mut g, LINE_DONE);
        assert_eq!((t.target_pc, t.resume_pc, g.u32(cpu.esp + 4).unwrap()), (SOUND, SOUND_DONE, EVENT));
        back(&mut cpu, 4);
        let t = step(&mut cpu, &mut g, SOUND_DONE);
        assert_eq!((t.kind, t.target_pc, cpu.esp), (abi::RETURN, RETURN, STACK + 8));
        assert_eq!([cpu.ebx, cpu.ebp, cpu.esi, cpu.edi], SAVED);
    }

    #[test]
    fn the_timed_line_counts_its_seconds_in_ticks() {
        let (mut cpu, mut g) = world();
        g.set_u32(ROW + 0xc, 0).unwrap();
        g.set_u32(ROW + 0x14, 0x0042_0002).unwrap();
        g.set_u32(ROW + 0x18, 3).unwrap();
        let t = step(&mut cpu, &mut g, ENTRY);
        assert_eq!((t.target_pc, t.resume_pc, cpu.edi), (STRING, TIMED_TEXT_DONE, 0x0042_0002));
        back(&mut cpu, 4);
        assert_eq!(step(&mut cpu, &mut g, TIMED_TEXT_DONE).target_pc, FORMAT);
        back(&mut cpu, 4);
        let t = step(&mut cpu, &mut g, TIMED_FORMAT_DONE);
        assert_eq!((t.target_pc, t.resume_pc), (TIMED_LINE, TIMED_DONE));
        assert_eq!(g.u32(cpu.esp + 4).unwrap(), 90, "3 seconds at 30 ticks");
        assert_eq!(sse::lane0(&cpu, 0), 90.0f32.to_bits());
        assert_eq!((cpu.x87.instruction_pointer, cpu.x87.opcode, cpu.x87.tag), (FISTP_AT, FISTP_OP, 0xffff));
    }

    #[test]
    fn no_local_user_or_quiet_announcements_return_at_once() {
        let (mut cpu, mut g) = world();
        g.set_u16(PLAYER_DATA + STRIDE + 0x28, 0xffff).unwrap();
        let t = step(&mut cpu, &mut g, ENTRY);
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.ebp), (abi::RETURN, RETURN, STACK + 8, SAVED[1]));

        let (mut cpu, mut g) = world();
        g.set_u8(MODE_RECORD + 0x1128, 1).unwrap();
        g.set_u32(EVENT + 4, 0x14).unwrap();
        let t = step(&mut cpu, &mut g, ENTRY);
        assert_eq!((t.kind, t.target_pc), (abi::RETURN, RETURN));
        // Type 0 kind 0x13 is announced even then.
        let (mut cpu, mut g) = world();
        g.set_u8(MODE_RECORD + 0x1128, 1).unwrap();
        g.set_u32(EVENT + 4, 0x13).unwrap();
        assert_eq!(step(&mut cpu, &mut g, ENTRY).target_pc, STRING);
    }
}
