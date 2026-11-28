//! The three object walks (docs/engine/object-update.md, "Slice 1: walks
//! and dispatchers"; design docs/engine/sim-rewrite-objects.md section 2):
//! pre-physics `b6f10`, post-physics `b7060` and finalize `b7150`.
//!
//! The pre- and post-physics bodies are the per-function ports
//! (`sim_physics::objects`, which stay test oracles) taken into the body
//! table: the first pass uses the early-mover list, the second enumerates
//! live headers in slot order, re-reading the high-water mark and the
//! bitmap after every visited slot. Finalize scans the bitmap one slot at
//! a time, with the high-water mark and the bitmap read once per visited
//! slot. Callbacks can add, delete and relink objects during any pass.

use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use guest_layout::object::header;
use runtime::{CpuAbi, Guest};
use ::sim_objects::events::{self, SimEvent};
use ::sim_objects::walk;

const LIST_OWNER: u32 = 0x0051_07f0;
const PHASE_STATE: u32 = 0x004d_e2f4;
const LIST: u32 = 0x1d00;
const LIST_COUNT: u32 = 0x1d80;
const HEADER_FLAGS: u32 = 2;
const HEADER_KIND: u32 = 3;
const OBJECT_PTR: u32 = 8;
const PHYSICS_FLAGS: u32 = 0xc0;
const HEADER_BYTES: u32 = 12;

const PRE_OBJECT: u32 = 0x000b_c470;
const PRE_LIST_VISIT: u32 = 0x0010_9580;
const POST_OBJECT: u32 = 0x000b_c820;
const POST_LIST_VISIT: u32 = 0x0010_9660;
const DEFERRED_FIRE: u32 = 0x000f_d910;
const DELETE: u32 = 0x000b_83b0;
const DELETE_FINISH: u32 = 0x000b_8460;
const COLLECT: u32 = 0x000b_f380;

// Return PCs, in call order, of the list update, its follow-up, and the
// occupancy-walk update (plus the post phase's final callback).
pub(super) const PRE_SITES: [u32; 3] = [0xb6f6e, 0xb6f7f, 0xb7036];
pub(super) const POST_SITES: [u32; 4] = [0xb70ae, 0xb70b7, 0xb7142, 0xb70f6];
// After `fd910`, `bc470`, `bc820`, `b83b0` and `b8460`.
pub(super) const FINALIZE_SITES: [u32; 5] = [0xb71a0, 0xb7240, 0xb7250, 0xb7282, 0xb728a];

fn call_arg(c: &mut CpuAbi, g: &mut Guest, arg: u32, target: u32, site: u32) -> Result<Next> {
    push(c, g, arg)?;
    Ok(Next::Call { target, site })
}

fn set_phase(g: &mut Guest, state: u32, active: bool) -> Result<()> {
    g.set_u8(state.wrapping_add(3), u8::from(active))
}

fn current_objects(g: &Guest) -> Result<u32> {
    g.u32(points::OBJECTS.base)
}

fn header_offset(handle: u32) -> u32 {
    (handle & 0xffff) * HEADER_BYTES
}

#[derive(PartialEq, Eq)]
enum Scan {
    Hit,
    Negative,
    AtLimit,
    Exhausted,
}

// Skip zero runs a word at a time. Never look beyond the current word
// before proving that the previous one contained no candidate. Recreate
// the final slot's scratch registers even when no callback follows.
fn next_live(c: &mut CpuAbi, g: &Guest) -> Result<Scan> {
    c.eax = c.edi.wrapping_add(1);
    if (c.eax as i32) < 0 {
        logic(c, c.eax, 1 << 31);
        return Ok(Scan::Negative);
    }
    c.edi = g.u32(c.esi.wrapping_add(layout::HIGH_WATER))?;
    if c.eax as i32 >= c.edi as i32 {
        arithmetic(c, c.eax, c.edi, true);
        return Ok(Scan::AtLimit);
    }
    c.edx = g.u32(c.esi.wrapping_add(layout::BITMAP))?;
    let slot = walk::next_live(c.eax..c.edi, |at| {
        c.eax = at;
        c.ecx = at >> 5;
        g.u32(c.edx.wrapping_add(4 * c.ecx))
    })?;
    if let Some(slot) = slot {
        c.eax = slot;
        c.ebx = 1 << (slot & 31);
        // Both originals compare the selected slot to the NONE sentinel.
        arithmetic(c, slot, u32::MAX, true);
        return Ok(Scan::Hit);
    }
    c.ebx = 1 << ((c.edi - 1) & 31);
    c.eax = c.edi;
    arithmetic(c, c.eax, c.edi, true);
    Ok(Scan::Exhausted)
}

fn restore(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    Ok(())
}

/// `b6f10`. The tick's event buffer is emptied earlier, at `GAME_TICK`'s
/// entry (`damage::tick_starts`): damage lands before the walks run.
pub(super) fn pre(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == PRE_SITES[2] {
        return pre_slots(c, g);
    }
    if pc == points::OBJECTS_PRE_PHYSICS.base {
        push(c, g, c.ecx)?;
        c.eax = g.u32(LIST_OWNER)?;
        c.ecx = g.u32(PHASE_STATE)?;
        push(c, g, c.ebx)?;
        push(c, g, c.ebp)?;
        c.ebx = c.eax.wrapping_add(LIST);
        c.eax = g.u32(c.eax.wrapping_add(LIST_COUNT))?;
        push(c, g, c.esi)?;
        c.ebp = 0;
        push(c, g, c.edi)?;
        g.set_u32(c.esp.wrapping_add(16), c.eax)?;
        set_phase(g, c.ecx, true)?;
    } else if pc == PRE_SITES[0] || pc == PRE_SITES[1] {
        if pc == PRE_SITES[0] {
            let flags = g.u8(c.edi.wrapping_add(HEADER_FLAGS))?;
            c.eax = byte(c.eax, flags);
            if flags & 12 == 4 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.esi, PRE_LIST_VISIT, PRE_SITES[1]);
            }
        }
        c.ebp = c.ebp.wrapping_add(1);
        c.eax = g.u32(c.esp.wrapping_add(16))?;
    } else {
        return Err(unowned(pc));
    }

    while (c.ebp as i32) < c.eax as i32 {
        c.esi = g.u32(c.ebx.wrapping_add(c.ebp.wrapping_mul(4)))?;
        c.edx = (c.esi & 0xffff) * 3;
        c.eax = current_objects(g)?;
        c.ecx = g.u32(c.eax.wrapping_add(layout::DATA))?;
        c.edi = c.ecx.wrapping_add(4 * c.edx);
        let flags = g.u8(c.edi.wrapping_add(HEADER_FLAGS))?;
        c.eax = byte(c.eax, flags);
        if flags & 11 == 3 {
            logic(c, 0, 0x80);
            return call_arg(c, g, c.esi, PRE_OBJECT, PRE_SITES[0]);
        }
        c.eax = g.u32(c.esp.wrapping_add(16))?;
        c.ebp = c.ebp.wrapping_add(1);
    }
    c.esi = current_objects(g)?;
    c.edi = u32::MAX;
    pre_slots(c, g)
}

fn pre_slots(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        match next_live(c, g)? {
            Scan::Hit => {}
            exhausted => {
                let state = g.u32(PHASE_STATE)?;
                match exhausted {
                    Scan::Negative => c.eax = state,
                    Scan::AtLimit => c.ecx = state,
                    _ => c.edx = state,
                }
                restore(c, g)?;
                set_phase(g, state, false)?;
                c.ebx = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
        }
        let stride = g.u32(c.esi.wrapping_add(layout::ELEMENT_SIZE))?;
        let data = g.u32(c.esi.wrapping_add(layout::DATA))?;
        c.ecx = data.wrapping_add(c.eax.wrapping_mul(stride));
        c.edx = (u32::from(g.u16(c.ecx)?) << 16) | c.eax;
        let flags = g.u8(c.ecx.wrapping_add(HEADER_FLAGS))?;
        c.ecx = byte(c.ecx, flags);
        c.edi = c.eax;
        if flags & 11 == 3 {
            c.ecx = g.u32(current_objects(g)?.wrapping_add(layout::DATA))?;
            c.eax = g.u32(c.ecx.wrapping_add(header_offset(c.edx) + OBJECT_PTR))?;
            let attachment = g.u8(c.eax.wrapping_add(PHYSICS_FLAGS))? >> 3;
            c.ecx = byte(c.ecx, attachment);
            if attachment & 1 == 0 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.edx, PRE_OBJECT, PRE_SITES[2]);
            }
        }
    }
}

/// `b7060`.
pub(super) fn post(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == POST_SITES[3] {
        c.edx = g.u32(PHASE_STATE)?;
        restore(c, g)?;
        set_phase(g, c.edx, false)?;
        c.ebx = pop(c, g)?;
        return Ok(Next::Ret { pop: 0 });
    }
    if pc == POST_SITES[0] {
        c.ecx = c.edi;
        c.eax = c.esi;
        return Ok(Next::Call { target: POST_LIST_VISIT, site: POST_SITES[1] });
    }
    if pc != POST_SITES[2] {
        if pc == points::OBJECTS_POST_PHYSICS.base {
            c.eax = g.u32(LIST_OWNER)?;
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(c.eax.wrapping_add(LIST_COUNT))?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.ebx = c.eax.wrapping_add(LIST);
            c.eax = g.u32(PHASE_STATE)?;
            c.edi = 0;
            set_phase(g, c.eax, true)?;
        } else if pc == POST_SITES[1] {
            c.edi = c.edi.wrapping_add(1);
        } else {
            return Err(unowned(pc));
        }

        while (c.edi as i32) < c.ebp as i32 {
            c.esi = g.u32(c.ebx.wrapping_add(c.edi.wrapping_mul(4)))?;
            c.edx = current_objects(g)?;
            c.ecx = (c.esi & 0xffff) * 3;
            c.eax = g.u32(c.edx.wrapping_add(layout::DATA))?.wrapping_add(4 * c.ecx);
            let flags = g.u8(c.eax.wrapping_add(HEADER_FLAGS))?;
            c.eax = byte(c.eax, flags);
            if flags & 12 == 4 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.esi, POST_OBJECT, POST_SITES[0]);
            }
            c.edi = c.edi.wrapping_add(1);
        }
        c.esi = current_objects(g)?;
        c.edi = u32::MAX;
    }

    loop {
        if next_live(c, g)? != Scan::Hit {
            return Ok(Next::Call { target: DEFERRED_FIRE, site: POST_SITES[3] });
        }
        let stride = g.u32(c.esi.wrapping_add(layout::ELEMENT_SIZE))?;
        c.ebp = g.u32(c.esi.wrapping_add(layout::DATA))?;
        c.ecx = c.ebp.wrapping_add(c.eax.wrapping_mul(stride));
        c.edx = (u32::from(g.u16(c.ecx)?) << 16) | c.eax;
        c.edi = c.eax;
        let flags = g.u8(c.ecx.wrapping_add(HEADER_FLAGS))?;
        c.eax = byte(c.eax, flags);
        if flags & 12 == 4 {
            c.eax = g.u32(c.ecx.wrapping_add(OBJECT_PTR))?;
            let attachment = g.u8(c.eax.wrapping_add(PHYSICS_FLAGS))? >> 3;
            c.ecx = byte(c.ecx, attachment);
            if attachment & 1 == 0 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.edx, POST_OBJECT, POST_SITES[2]);
            }
        }
    }
}

/// `b7150`. Pass A clears DEFER on every live header (a store even when
/// it is clear) and runs both updates for each NEW, ACTIVE, AWAKE object
/// not marked DELETE, clearing NEW first; then `fd910`; pass B deletes
/// every DELETE object (`b83b0`, `b8460`, each `(handle, 1)`), recording
/// an `ObjectDeleted` event first; then the phase byte is cleared and it
/// tail-jumps to the collector `bf380`.
///
/// Registers: `ebp` holds the objects array (read once per pass); pass A
/// keeps the slot in `ebx`, the handle in `edi` and the header in `esi`;
/// pass B the slot in `edi` and the handle in `esi`.
pub(super) fn finalize(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECTS_FINALIZE.base => {
            c.eax = g.u32(PHASE_STATE)?;
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = current_objects(g)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            set_phase(g, c.eax, true)?;
            c.ebx = u32::MAX;
            finalize_new(c, g)
        }
        // After `bc470`: `test byte ptr [esi+2], 4`.
        0xb7240 => {
            let flags = g.u8(c.esi.wrapping_add(HEADER_FLAGS))?;
            logic(c, u32::from(flags & header::POST), 0x80);
            if flags & header::POST != 0 {
                return call_arg(c, g, c.edi, POST_OBJECT, FINALIZE_SITES[2]);
            }
            finalize_new(c, g)
        }
        0xb7250 => finalize_new(c, g),
        // After `fd910`: pass B.
        0xb71a0 => {
            c.ebp = current_objects(g)?;
            c.edi = u32::MAX;
            finalize_delete(c, g)
        }
        0xb7282 => {
            push(c, g, 1)?;
            call_arg(c, g, c.esi, DELETE_FINISH, FINALIZE_SITES[4])
        }
        0xb728a => finalize_delete(c, g),
        _ => Err(unowned(pc)),
    }
}

/// The scan at `b7166`/`b71b0`, from `eax` (one past the last slot) over
/// the array in `ebp`: `test eax, eax; jl`, the high-water mark into
/// `esi`, `cmp eax, esi; jge`, the bitmap into `edx`, then one slot at a
/// time (`ecx` the word index, `edi` the bit) until a live slot or the
/// high-water mark. True with `eax` the live slot. Each bitmap word is
/// read once, where retail reads it once per slot; the registers and
/// flags at the end are retail's.
fn scan(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    logic(c, c.eax, 1 << 31);
    if (c.eax as i32) < 0 {
        return Ok(false);
    }
    c.esi = g.u32(c.ebp.wrapping_add(layout::HIGH_WATER))?;
    arithmetic(c, c.eax, c.esi, true);
    if c.eax as i32 >= c.esi as i32 {
        return Ok(false);
    }
    c.edx = g.u32(c.ebp.wrapping_add(layout::BITMAP))?;
    // 0 <= eax < esi here.
    let high = c.esi;
    let slot = walk::next_live(c.eax..high, |at| g.u32(c.edx.wrapping_add(4 * (at >> 5))))?;
    if let Some(slot) = slot {
        c.eax = slot;
        c.ecx = slot >> 5;
        c.edi = 1 << (slot & 31);
        logic(c, c.edi, 1 << 31);
        return Ok(true);
    }
    let last = high - 1;
    c.eax = high;
    c.ecx = last >> 5;
    c.edi = 1 << (last & 31);
    arithmetic(c, high, high, true);
    Ok(false)
}

/// The live slots after `last` (one slot before the first to look at; all
/// ones for none) that `idle` passes over, up to the first it does not or
/// the end, with `idle`'s byte stored to each header's flags when `store`.
/// Returns the live slot just before the last idle one, which the caller
/// puts in its slot register so the register loop replays that last idle
/// slot and everything after it: the registers every transfer shows then
/// depend only on slots the loop ran, as in retail. `None` when fewer than
/// two slots were passed over. Pass A and pass B make no callout between
/// those slots and read nothing a callout could change, so the whole run
/// is read from the array once.
fn skip_idle(
    c: &CpuAbi,
    g: &mut Guest,
    last: u32,
    store: bool,
    idle: impl Fn(u8) -> Option<u8>,
) -> Result<Option<u32>> {
    let from = last.wrapping_add(1);
    let high = g.u32(c.ebp.wrapping_add(layout::HIGH_WATER))?;
    if (from as i32) < 0 || from as i32 >= high as i32 {
        return Ok(None);
    }
    let bitmap = g.u32(c.ebp.wrapping_add(layout::BITMAP))?;
    let stride = g.u32(c.ebp.wrapping_add(layout::ELEMENT_SIZE))?;
    let data = g.u32(c.ebp.wrapping_add(layout::DATA))?;
    let (mut at, mut passed, mut latest) = (from, None, None);
    while let Some(slot) = walk::next_live(at..high, |at| g.u32(bitmap.wrapping_add(4 * (at >> 5))))? {
        let flags_at = data.wrapping_add(slot.wrapping_mul(stride)).wrapping_add(HEADER_FLAGS);
        let Some(byte) = idle(g.u8(flags_at)?) else {
            break;
        };
        if store {
            g.set_u8(flags_at, byte)?;
        }
        passed = latest;
        latest = Some(slot);
        at = slot + 1;
    }
    Ok(passed)
}

/// Finalize pass A from `b7166`.
fn finalize_new(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let due = header::NEW | header::ACTIVE | header::AWAKE;
    let idle = |flags: u8| {
        let kept = flags & !header::DEFER;
        (kept & (due | header::DELETE) != due).then_some(kept)
    };
    if let Some(before) = skip_idle(c, g, c.ebx, true, idle)? {
        c.ebx = before;
    }
    loop {
        c.eax = c.ebx.wrapping_add(1);
        if !scan(c, g)? {
            return Ok(Next::Call { target: DEFERRED_FIRE, site: FINALIZE_SITES[0] });
        }
        arithmetic(c, c.eax, u32::MAX, true);
        if c.eax == u32::MAX {
            return Ok(Next::Call { target: DEFERRED_FIRE, site: FINALIZE_SITES[0] });
        }
        c.esi = g.u32(c.ebp.wrapping_add(layout::ELEMENT_SIZE))?;
        c.edi = g.u32(c.ebp.wrapping_add(layout::DATA))?;
        c.esi = c.esi.wrapping_mul(c.eax);
        let flags = g.u8(c.esi.wrapping_add(c.edi).wrapping_add(HEADER_FLAGS))?;
        c.esi = c.esi.wrapping_add(c.edi);
        c.edi = (u32::from(g.u16(c.esi)?) << 16) | c.eax;
        let dl = flags & !header::DEFER;
        c.edx = byte(c.edx, dl);
        c.ebx = c.eax;
        c.eax = byte(c.eax, dl);
        g.set_u8(c.esi.wrapping_add(HEADER_FLAGS), dl)?;
        let due = header::NEW | header::ACTIVE | header::AWAKE;
        if dl & (due | header::DELETE) != due {
            continue;
        }
        let al = dl & !header::NEW;
        c.eax = byte(c.eax, al);
        logic(c, u32::from(al), 0x80);
        push(c, g, c.edi)?;
        g.set_u8(c.esi.wrapping_add(HEADER_FLAGS), al)?;
        return Ok(Next::Call { target: PRE_OBJECT, site: FINALIZE_SITES[1] });
    }
}

/// Finalize pass B from `b71b0`.
fn finalize_delete(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let idle = |flags: u8| (flags & header::DELETE == 0).then_some(flags);
    if let Some(before) = skip_idle(c, g, c.edi, false, idle)? {
        c.edi = before;
    }
    loop {
        c.eax = c.edi.wrapping_add(1);
        if !scan(c, g)? {
            return finalize_end(c, g);
        }
        arithmetic(c, c.eax, u32::MAX, true);
        if c.eax == u32::MAX {
            return finalize_end(c, g);
        }
        c.ecx = g.u32(c.ebp.wrapping_add(layout::ELEMENT_SIZE))?;
        c.edx = g.u32(c.ebp.wrapping_add(layout::DATA))?;
        c.ecx = c.ecx.wrapping_mul(c.eax);
        c.esi = (u32::from(g.u16(c.ecx.wrapping_add(c.edx))?) << 16) | c.eax;
        c.ecx = c.ecx.wrapping_add(c.edx);
        c.edi = c.eax;
        let flags = g.u8(c.ecx.wrapping_add(HEADER_FLAGS))?;
        logic(c, u32::from(flags & header::DELETE), 0x80);
        if flags & header::DELETE == 0 {
            continue;
        }
        deleted(g, c.esi, c.ecx);
        push(c, g, 1)?;
        return call_arg(c, g, c.esi, DELETE, FINALIZE_SITES[3]);
    }
}

/// `b71dc`: clear the phase byte, restore, and leave for the collector.
fn finalize_end(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(PHASE_STATE)?;
    restore(c, g)?;
    set_phase(g, c.edx, false)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Tail { target: COLLECT })
}

/// The `ObjectDeleted` event for the object about to be deleted. Its
/// reads are not retail's, so a fault leaves the field unknown instead of
/// faulting the guest.
fn deleted(g: &Guest, id: u32, header: u32) {
    let kind = g.u8(header.wrapping_add(HEADER_KIND)).unwrap_or(u8::MAX);
    let definition = g
        .u32(header.wrapping_add(OBJECT_PTR))
        .and_then(|body| g.u32(body))
        .unwrap_or(u32::MAX);
    events::push(SimEvent::ObjectDeleted {
        id: guest_layout::Datum(id),
        kind,
        definition: guest_layout::Datum(definition),
    });
}
