//! A game-engine event's post and show, seat bodies over the retail frame
//! (docs/engine/sim-rewrite-sketches.md, "Seats and rules (phase 7): kernels"). The respawn
//! gate calls `19eb90` from its own frame for a host's message or an event
//! naming no player; every other caller reaches them too.
//!
//! | Function | Point | Body | Guest calls |
//! |---|---|---|---|
//! | `19eb90 (event: eax)` | `GAME_EVENT_POST` | nothing on a client (mode 4); else the send, then the show | `a7c50`, `19eb30` (seats) |
//! | `a7c50 (event: edx)` | `GAME_EVENT_SEND` | on a host (mode 5): the event copied to the frame, its three player words cut to 16 bits, then the message | `b5a70 (7, 0, 0, 24h, copy, [4cedf0])`, the network message |
//! | `19eb30 (event: edi)` | `GAME_EVENT_SHOW` | the event's player's announcements, or every player's when it names none | `19de80` (seat), the player iterator `19f240` |
//! | `19de80 (event: eax, player: esi)` | `GAME_EVENT_SUBJECTS` | the row by the event's subjects in order (the player as `+0xc`, `+0x14`, its team as `+0x10`, `+0x18`, anyone), announced | `19df10` (seat), `19e890` (seat) |
//! | `19df10 (player: eax, event: edi, kind)` | `GAME_EVENT_ROW` | the respawn kernel's [`crate::sim_seats::respawn::announcement`] | none |
//!
//! At every call and the return the registers, flags and frame are
//! retail's, but `19df10`'s: it calls nothing, writes nothing, and its
//! callers test `eax` before anything else, so it returns `eax`, `ecx`
//! and `edx` as retail does and leaves the arithmetic flags undefined.
//! The bodies decline only on the float environment
//! ([`crate::sim_native::plan`]); the seat installer panics on it by name.

use crate::sim_native::{Memory, plan::segment, sub_flags};
use crate::sim_physics::{ARITH, arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

type Result<T> = std::result::Result<T, GuestFault>;
type Boundary = std::result::Result<Option<TransferAbi>, GuestFault>;

const MODE: u32 = 0x004e_6948;
const PLAYERS: u32 = 0x004e_8c24;
const POST_TAG: u32 = 0x004c_edf0;
const STRIDE: u32 = 0x21c;
const NONE: u32 = u32::MAX;

const SEND: u32 = 0x000a_7c50;
const SHOW: u32 = 0x0019_eb30;
const SUBJECTS: u32 = 0x0019_de80;
const ROW: u32 = 0x0019_df10;
const ANNOUNCE: u32 = 0x0019_e890;
const MESSAGE: u32 = 0x000b_5a70;
const NEXT_PLAYER: u32 = 0x0019_f240;

#[inline]
fn at(p: u32, n: u32) -> u32 {
    p.wrapping_add(n)
}

// `19eb90`: the post.

pub const POST: u32 = 0x0019_eb90;
const POST_SENT: u32 = 0x0019_eba5;
const POST_SHOWN: u32 = 0x0019_ebaa;
pub const POST_SITES: &[u32] = &[POST_SENT, POST_SHOWN];

fn post_select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    match pc {
        POST => {
            push(c, g, c.edi)?;
            c.edi = c.eax;
            c.eax = g.u32(MODE)?;
            let mode = g.u8(at(c.eax, 0xc))?;
            sub_flags(c, u32::from(mode), 4, 8);
            if mode == 4 {
                c.edi = pop(c, g)?;
                return g.ret(c, 0);
            }
            c.edx = c.edi;
            g.call(c, SEND, POST_SENT)
        }
        POST_SENT => g.call(c, SHOW, POST_SHOWN),
        POST_SHOWN => {
            c.edi = pop(c, g)?;
            g.ret(c, 0)
        }
        _ => unreachable!("undeclared event post continuation {pc:#x}"),
    }
}

/// One `19eb90` segment.
pub fn post(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    segment::<8, _>(c, g, pc, POST..0x0019_ebac, POST_SITES, |c, p, pc| post_select(c, p, pc))
}

// `a7c50`: the send.

const SEND_DONE: u32 = 0x000a_7ccc;
pub const SEND_SITES: &[u32] = &[SEND_DONE];
const SEND_FRAME: u32 = 0x24;

/// `add esp, 24h; ret` at `a7ccc`.
fn send_leave(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.esp = arithmetic(c, c.esp, SEND_FRAME, false);
    g.ret(c, 0)
}

/// `cmp edx, eax; je; and edx, ecx; mov [esp+off], edx` for one player
/// word, `eax` -1 and `ecx` `ffff`.
fn cut(c: &mut CpuAbi, g: &mut impl Memory, off: u32) -> Result<()> {
    c.edx = g.u32(at(c.esp, off))?;
    arithmetic(c, c.edx, c.eax, true);
    if c.edx != c.eax {
        c.edx &= c.ecx;
        logic(c, c.edx, 1 << 31);
        g.set_u32(at(c.esp, off), c.edx)?;
    }
    Ok(())
}

fn send_select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    match pc {
        SEND => {
            c.eax = g.u32(MODE)?;
            c.eax = g.u8(at(c.eax, 0xc))? as i8 as i32 as u32;
            c.esp = arithmetic(c, c.esp, SEND_FRAME, true);
            arithmetic(c, c.eax, 4, true);
            if (c.eax as i32) < 4 {
                return send_leave(c, g);
            }
            arithmetic(c, c.eax, 5, true);
            if c.eax as i32 > 5 {
                return send_leave(c, g);
            }
            c.eax = arithmetic(c, c.eax, 2, true);
            if c.eax == 0 {
                return send_leave(c, g);
            }
            c.eax = arithmetic(c, c.eax, 2, true);
            if c.eax == 0 {
                return send_leave(c, g);
            }
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.esi = c.edx;
            c.ecx = 9;
            c.edi = at(c.esp, 8);
            // `rep movsd`, in the direction flag's direction.
            let step = if c.eflags & (1 << 10) == 0 { 4 } else { 4u32.wrapping_neg() };
            while c.ecx != 0 {
                let word = g.u32(c.esi)?;
                g.set_u32(c.edi, word)?;
                c.esi = at(c.esi, step);
                c.edi = at(c.edi, step);
                c.ecx -= 1;
            }
            c.edx = g.u32(at(c.esp, 0x10))?;
            c.eax = NONE;
            arithmetic(c, c.edx, c.eax, true);
            let same = c.edx == c.eax;
            c.edi = pop(c, g)?;
            c.ecx = 0xffff;
            c.esi = pop(c, g)?;
            if !same {
                c.edx &= c.ecx;
                logic(c, c.edx, 1 << 31);
                g.set_u32(at(c.esp, 8), c.edx)?;
            }
            cut(c, g, 0xc)?;
            cut(c, g, 0x14)?;
            c.ecx = g.u32(POST_TAG)?;
            push(c, g, c.ecx)?;
            c.edx = at(c.esp, 4);
            for word in [c.edx, 0x24, 0, 0, 7] {
                push(c, g, word)?;
            }
            g.call(c, MESSAGE, SEND_DONE)
        }
        SEND_DONE => send_leave(c, g),
        _ => unreachable!("undeclared event send continuation {pc:#x}"),
    }
}

/// One `a7c50` segment.
pub fn send(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    segment::<24, _>(c, g, pc, SEND..0x000a_7cd0, SEND_SITES, |c, p, pc| send_select(c, p, pc))
}

// `19eb30`: the show.

const FIRST_PLAYER: u32 = 0x0019_eb59;
const PLAYER_SHOWN: u32 = 0x0019_eb6b;
const NEXT_DONE: u32 = 0x0019_eb74;
const SHOWN: u32 = 0x0019_eb84;
pub const SHOW_SITES: &[u32] = &[FIRST_PLAYER, PLAYER_SHOWN, NEXT_DONE, SHOWN];

/// `pop esi; add esp, 10h; ret`.
fn show_leave(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.esi = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x10, false);
    g.ret(c, 0)
}

/// `19eb60`: the iterator's player announced.
fn show_player(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    c.esi = g.u32(at(c.esp, 0xc))?;
    c.eax = c.edi;
    g.call(c, SUBJECTS, PLAYER_SHOWN)
}

fn show_select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    match pc {
        SHOW => {
            c.esp = arithmetic(c, c.esp, 0x10, true);
            push(c, g, c.esi)?;
            c.esi = g.u32(at(c.edi, 8))?;
            c.eax = NONE;
            logic(c, c.eax, 1 << 31);
            arithmetic(c, c.esi, c.eax, true);
            if c.esi != c.eax {
                c.eax = c.edi;
                return g.call(c, SUBJECTS, SHOWN);
            }
            c.ecx = g.u32(PLAYERS)?;
            c.esi = at(c.esp, 4);
            g.set_u32(at(c.esp, 8), c.ecx)?;
            g.set_u32(at(c.esp, 0x10), c.eax)?;
            g.set_u32(at(c.esp, 0xc), c.eax)?;
            g.call(c, NEXT_PLAYER, FIRST_PLAYER)
        }
        FIRST_PLAYER | NEXT_DONE => {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax & 0xff == 0 {
                return show_leave(c, g);
            }
            show_player(c, g)
        }
        PLAYER_SHOWN => {
            c.esi = at(c.esp, 4);
            g.call(c, NEXT_PLAYER, NEXT_DONE)
        }
        SHOWN => show_leave(c, g),
        _ => unreachable!("undeclared event show continuation {pc:#x}"),
    }
}

/// One `19eb30` segment.
pub fn show(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    segment::<8, _>(c, g, pc, SHOW..0x0019_eb89, SHOW_SITES, |c, p, pc| show_select(c, p, pc))
}

// `19de80`: the subjects.

const AS_PLAYER: u32 = 0x0019_deac;
const AS_SECOND: u32 = 0x0019_debe;
const AS_TEAM: u32 = 0x0019_ded7;
const AS_SECOND_TEAM: u32 = 0x0019_dee9;
const AS_ANYONE: u32 = 0x0019_def6;
const ANNOUNCED: u32 = 0x0019_df04;
pub const SUBJECTS_SITES: &[u32] = &[AS_PLAYER, AS_SECOND, AS_TEAM, AS_SECOND_TEAM, AS_ANYONE, ANNOUNCED];

/// `push kind; mov eax, esi; call 19df10`.
fn row(c: &mut CpuAbi, g: &mut impl Memory, kind: u32, site: u32) -> Result<TransferAbi> {
    push(c, g, kind)?;
    c.eax = c.esi;
    g.call(c, ROW, site)
}

/// `pop edi; pop ebx; ret` at `19df04`.
fn subjects_leave(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 0)
}

/// `test eax, eax; jne 19defa`: the row found is announced.
fn found(c: &mut CpuAbi) -> bool {
    logic(c, c.eax, 1 << 31);
    c.eax != 0
}

/// `19defa`: `mov ecx, eax; push edi; mov eax, esi; call 19e890`.
fn announce(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    c.ecx = c.eax;
    push(c, g, c.edi)?;
    c.eax = c.esi;
    g.call(c, ANNOUNCE, ANNOUNCED)
}

/// `19deb0`: the player as the second subject.
fn as_second(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    let second = g.u32(at(c.edi, 0x14))?;
    arithmetic(c, c.esi, second, true);
    if c.esi == second {
        return row(c, g, 2, AS_SECOND);
    }
    as_team(c, g)
}

/// `19dec2`: the player's team as the first team subject.
fn as_team(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    c.ebx = g.u8(at(c.ebx, 0xc0))? as i8 as i32 as u32;
    let team = g.u32(at(c.edi, 0x10))?;
    arithmetic(c, c.ebx, team, true);
    if c.ebx == team {
        return row(c, g, 1, AS_TEAM);
    }
    as_second_team(c, g)
}

/// `19dedb`: the team as the second team subject.
fn as_second_team(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    let team = g.u32(at(c.edi, 0x18))?;
    arithmetic(c, c.ebx, team, true);
    if c.ebx == team {
        return row(c, g, 3, AS_SECOND_TEAM);
    }
    row(c, g, 4, AS_ANYONE)
}

fn subjects_select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    match pc {
        SUBJECTS => {
            push(c, g, c.ebx)?;
            c.ebx = (c.esi & 0xffff).wrapping_mul(STRIDE);
            push(c, g, c.edi)?;
            c.edi = c.eax;
            c.eax = g.u32(PLAYERS)?;
            c.ecx = g.u32(at(c.eax, 0x44))?;
            c.eax = g.u32(at(c.edi, 0xc))?;
            c.ebx = arithmetic(c, c.ebx, c.ecx, false);
            arithmetic(c, c.esi, c.eax, true);
            if c.esi == c.eax {
                return row(c, g, 0, AS_PLAYER);
            }
            as_second(c, g)
        }
        AS_PLAYER => if found(c) { announce(c, g) } else { as_second(c, g) },
        AS_SECOND => if found(c) { announce(c, g) } else { as_team(c, g) },
        AS_TEAM => if found(c) { announce(c, g) } else { as_second_team(c, g) },
        AS_SECOND_TEAM => if found(c) { announce(c, g) } else { row(c, g, 4, AS_ANYONE) },
        AS_ANYONE => if found(c) { announce(c, g) } else { subjects_leave(c, g) },
        ANNOUNCED => subjects_leave(c, g),
        _ => unreachable!("undeclared event subjects continuation {pc:#x}"),
    }
}

/// One `19de80` segment.
pub fn subjects(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    segment::<8, _>(c, g, pc, SUBJECTS..0x0019_df07, SUBJECTS_SITES, |c, p, pc| subjects_select(c, p, pc))
}

// `19df10`: the row.

/// `19df10 (player: eax, event: edi, kind)`, `ret 4`, whole: the kernel's
/// answer in `eax`, `ecx` and `edx`, the argument popped.
pub fn row_answer(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    if pc != ROW {
        return crate::sim_native::decline(pc, crate::sim_native::Decline::Boundary);
    }
    if !crate::sim_native::supported(c) {
        return crate::sim_native::decline(pc, crate::sim_native::Decline::Float);
    }
    let m = crate::sim_seats::respawn::Mem::new(g);
    let kind = g.u32(at(c.esp, 4))?;
    let r = crate::sim_seats::respawn::announcement(&m, c.eax, c.edi, kind)?;
    let target = g.u32(c.esp)?;
    c.eax = r.eax;
    c.ecx = r.ecx;
    c.edx = r.edx;
    c.esp = at(c.esp, 8);
    c.eflags_defined &= !ARITH;
    Ok(Some(TransferAbi::ret(target)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::abi;
    use runtime::guest::Permissions;

    const MODE_RECORD: u32 = 0x0060_0000;
    const HEADER: u32 = 0x0060_1000;
    const PLAYER_DATA: u32 = 0x0060_2000;
    const EVENT: u32 = 0x0060_4000;
    const STACK: u32 = 0x007f_0000;
    const RETURN: u32 = 0x0012_3456;

    /// A host (mode 5), an event naming player 1 (salted), the second
    /// subject none and the third player 2.
    fn world(mode: u8) -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        // The bodies' text, `a7c50` and `19de80..19ebac`: a segment reads it.
        g.map_zeroed(0x000a_7000, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(0x0019_d000, 0x2000, Permissions::RW).unwrap();
        g.map_zeroed(0x004c_0000, 0x0004_0000, Permissions::RW).unwrap();
        g.map_zeroed(MODE_RECORD, 0x0001_0000, Permissions::RW).unwrap();
        g.map_zeroed(STACK - 0x2000, 0x3000, Permissions::RW).unwrap();
        g.set_u32(MODE, MODE_RECORD).unwrap();
        g.set_u8(MODE_RECORD + 0xc, mode).unwrap();
        g.set_u32(PLAYERS, HEADER).unwrap();
        g.set_u32(HEADER + 0x44, PLAYER_DATA).unwrap();
        g.set_u32(POST_TAG, 0x0bad_7a90).unwrap();
        for i in 0..9 {
            g.set_u32(EVENT + 4 * i, 0x1000 + i).unwrap();
        }
        g.set_u32(EVENT + 8, 0xe001_0001).unwrap();
        g.set_u32(EVENT + 0xc, NONE).unwrap();
        g.set_u32(EVENT + 0x14, 0xe002_0002).unwrap();
        g.set_u32(STACK, RETURN).unwrap();
        let cpu = CpuAbi { esp: STACK, esi: 0x5151, edi: 0xd1d1, mxcsr: 0x1f80, ..Default::default() };
        let mut cpu = cpu;
        cpu.x87.control = 0x027f;
        cpu.x87.tag = 0xffff;
        (cpu, g)
    }

    /// The callee's `ret pop`.
    fn back(cpu: &mut CpuAbi, pop: u32) {
        cpu.esp += 4 + pop;
        cpu.eflags_defined = 0;
    }

    fn step(body: fn(&mut CpuAbi, &mut Guest, u32) -> Boundary, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> TransferAbi {
        body(cpu, g, pc)
            .unwrap()
            .unwrap_or_else(|| panic!("the body answers: declined {:?}", crate::sim_native::plan::take_decline()))
    }

    #[test]
    fn a_client_posts_nothing() {
        let (mut cpu, mut g) = world(4);
        cpu.eax = EVENT;
        let t = step(post, &mut cpu, &mut g, POST);
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.edi), (abi::RETURN, RETURN, STACK + 4, 0xd1d1));
    }

    #[test]
    fn a_host_sends_then_shows() {
        let (mut cpu, mut g) = world(5);
        cpu.eax = EVENT;
        let t = step(post, &mut cpu, &mut g, POST);
        assert_eq!((t.kind, t.target_pc, t.resume_pc), (abi::CALL, SEND, POST_SENT));
        assert_eq!((cpu.edx, cpu.edi, g.u32(cpu.esp + 4).unwrap()), (EVENT, EVENT, 0xd1d1));
        back(&mut cpu, 0);
        let t = step(post, &mut cpu, &mut g, POST_SENT);
        assert_eq!((t.target_pc, t.resume_pc, cpu.edi), (SHOW, POST_SHOWN, EVENT));
        back(&mut cpu, 0);
        let t = step(post, &mut cpu, &mut g, POST_SHOWN);
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.edi), (abi::RETURN, RETURN, STACK + 4, 0xd1d1));
    }

    #[test]
    fn the_send_copies_the_event_and_cuts_its_players() {
        let (mut cpu, mut g) = world(5);
        cpu.edx = EVENT;
        let t = step(send, &mut cpu, &mut g, SEND);
        assert_eq!((t.target_pc, t.resume_pc), (MESSAGE, SEND_DONE));
        let frame = STACK - SEND_FRAME;
        // `b5a70 (7, 0, 0, 24h, copy, [4cedf0])`.
        let args: Vec<u32> = (1..7).map(|i| g.u32(cpu.esp + 4 * i).unwrap()).collect();
        assert_eq!(args, [7, 0, 0, 0x24, frame, 0x0bad_7a90]);
        let copy: Vec<u32> = (0..9).map(|i| g.u32(frame + 4 * i).unwrap()).collect();
        let mut want: Vec<u32> = (0..9).map(|i| g.u32(EVENT + 4 * i).unwrap()).collect();
        want[2] = 1;
        want[5] = 2;
        assert_eq!(copy, want, "the player words cut to 16 bits, none kept");
        assert_eq!((cpu.esi, cpu.edi), (0x5151, 0xd1d1));
        back(&mut cpu, 0);
        cpu.esp += 0x18;
        let t = step(send, &mut cpu, &mut g, SEND_DONE);
        assert_eq!((t.kind, t.target_pc, cpu.esp), (abi::RETURN, RETURN, STACK + 4));
    }

    #[test]
    fn a_single_player_or_a_solo_game_sends_nothing() {
        for mode in [3, 6] {
            let (mut cpu, mut g) = world(mode);
            cpu.edx = EVENT;
            let t = step(send, &mut cpu, &mut g, SEND);
            assert_eq!((t.kind, t.target_pc, cpu.esp), (abi::RETURN, RETURN, STACK + 4), "mode {mode}");
        }
    }

    #[test]
    fn the_show_names_the_event_player_or_walks_every_player() {
        let (mut cpu, mut g) = world(5);
        cpu.edi = EVENT;
        let t = step(show, &mut cpu, &mut g, SHOW);
        assert_eq!((t.target_pc, t.resume_pc, cpu.eax, cpu.esi), (SUBJECTS, SHOWN, EVENT, 0xe001_0001));
        back(&mut cpu, 0);
        let t = step(show, &mut cpu, &mut g, SHOWN);
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.esi), (abi::RETURN, RETURN, STACK + 4, 0x5151));

        let (mut cpu, mut g) = world(5);
        g.set_u32(EVENT + 8, NONE).unwrap();
        cpu.edi = EVENT;
        let t = step(show, &mut cpu, &mut g, SHOW);
        assert_eq!((t.target_pc, t.resume_pc), (NEXT_PLAYER, FIRST_PLAYER));
        let iter = cpu.esi;
        assert_eq!((g.u32(iter + 4).unwrap(), g.u32(iter + 8).unwrap(), g.u32(iter + 0xc).unwrap()), (HEADER, NONE, NONE));
        back(&mut cpu, 0);
        // The iterator found player 3.
        g.set_u32(iter + 8, 0xe003_0003).unwrap();
        cpu.eax = 1;
        let t = step(show, &mut cpu, &mut g, FIRST_PLAYER);
        assert_eq!((t.target_pc, t.resume_pc, cpu.eax, cpu.esi), (SUBJECTS, PLAYER_SHOWN, EVENT, 0xe003_0003));
        back(&mut cpu, 0);
        let t = step(show, &mut cpu, &mut g, PLAYER_SHOWN);
        assert_eq!((t.target_pc, t.resume_pc, cpu.esi), (NEXT_PLAYER, NEXT_DONE, iter));
        back(&mut cpu, 0);
        cpu.eax = 0;
        let t = step(show, &mut cpu, &mut g, NEXT_DONE);
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.esi), (abi::RETURN, RETURN, STACK + 4, 0x5151));
    }

    #[test]
    fn the_subjects_go_player_second_team_second_team_anyone() {
        let (mut cpu, mut g) = world(5);
        // Player 2 on team 1; the event's subjects: player 1, player 2, team 1.
        g.set_u32(EVENT + 0xc, 0xe001_0001).unwrap();
        g.set_u32(EVENT + 0x10, 1).unwrap();
        g.set_u32(EVENT + 0x18, NONE).unwrap();
        g.set_u8(PLAYER_DATA + 2 * STRIDE + 0xc0, 1).unwrap();
        cpu.eax = EVENT;
        cpu.esi = 0xe002_0002;
        let t = step(subjects, &mut cpu, &mut g, SUBJECTS);
        // Not `+0xc`: the second subject, kind 2.
        assert_eq!((t.target_pc, t.resume_pc, cpu.eax, g.u32(cpu.esp + 4).unwrap()), (ROW, AS_SECOND, 0xe002_0002, 2));
        back(&mut cpu, 4);
        cpu.eax = 0;
        let t = step(subjects, &mut cpu, &mut g, AS_SECOND);
        assert_eq!((t.target_pc, t.resume_pc, g.u32(cpu.esp + 4).unwrap()), (ROW, AS_TEAM, 1));
        back(&mut cpu, 4);
        cpu.eax = 0x0060_9000;
        let t = step(subjects, &mut cpu, &mut g, AS_TEAM);
        assert_eq!((t.target_pc, t.resume_pc, cpu.eax, cpu.ecx), (ANNOUNCE, ANNOUNCED, 0xe002_0002, 0x0060_9000));
        assert_eq!(g.u32(cpu.esp + 4).unwrap(), EVENT);
        back(&mut cpu, 4);
        let t = step(subjects, &mut cpu, &mut g, ANNOUNCED);
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.edi), (abi::RETURN, RETURN, STACK + 4, 0xd1d1));
    }
}
