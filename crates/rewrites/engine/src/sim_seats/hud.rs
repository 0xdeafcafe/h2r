//! One mapped user's simulation HUD update. The seat record (0x6c bytes)
//! stays in the arena. Its widget description and two weapon views live in
//! the original caller frame across the display and sound callbacks.
use guest_layout::points;
use crate::sim_native::{self, Memory, plan::segment};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

pub const ENTRY: u32 = 0x0013_a838;
const END: u32 = 0x0013_aa27;
const USER_PREPARED: u32 = 0x13a85c;
const HUD_GATHERED: u32 = 0x13a868;
const WIDGETS_UPDATED: u32 = 0x13a874;
const USER_TESTED: u32 = 0x13a8f6;
const PLAYER_FOUND: u32 = 0x13a904;
const FIRST_WEAPON_DONE: u32 = 0x13a9f9;
const SECOND_WEAPON_DONE: u32 = 0x13aa20;
pub const SITES: &[u32] = &[
    USER_PREPARED, HUD_GATHERED, WIDGETS_UPDATED, USER_TESTED, PLAYER_FOUND,
    FIRST_WEAPON_DONE, SECOND_WEAPON_DONE,
];

const HUD_SEATS: u32 = 0x0051_0c4c;
const SEAT_BYTES: u32 = 0x6c;
const FRAME_BYTES: u32 = 0x274;
const NONE: u32 = u32::MAX;
const FULL_SCALE: u32 = 0x0045_dbc0;
const FADE_SPEED: u32 = 0x0045_dc0c;
const SHIELD_HOLD: u32 = 0x0045_dbbc;
const SHIELD_CHARGING: u32 = 0x0045_dbcc;

type Result<T> = std::result::Result<T, GuestFault>;

/// The view fade approaches the target recorded by the widget update.
/// Unordered comparisons and MXCSR follow the guest on all branches.
fn update_fade(c: &mut CpuAbi, g: &mut impl Memory) -> Result<()> {
    let enabled = g.u8(c.ebp.wrapping_sub(0x144))?;
    sim_native::sub_flags(c, enabled.into(), 0, 8);
    c.edx = g.u32(points::GAME_TIME.base)?;
    sse::load(c, 3, g.u32(FULL_SCALE)?);
    sse::zero(c, 2);
    if enabled == 0 {
        return Ok(());
    }

    let visible = g.u8(c.ebp.wrapping_sub(0x141))?;
    sim_native::sub_flags(c, visible.into(), 0, 8);
    sse::movaps(c, 1, if visible != 0 { 3 } else { 2 });
    let fade = c.edi.wrapping_add(0x28);
    sse::load(c, 0, g.u32(fade)?);
    sse::ucomiss(c, sse::lane0(c, 0), sse::lane0(c, 1));
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    if !sse::parity(c) {
        return Ok(());
    }
    sse::comiss(c, sse::lane0(c, 1), g.u32(fade)?);
    let rising = !sse::below_or_equal(c);
    c.eax = i32::from(g.u16(c.edx.wrapping_add(2))? as i16) as u32;
    sse::load(c, 0, g.u32(FADE_SPEED)?);
    sse::cvtsi2ss(c, 4, c.eax);
    sse::ss_reg(c, Ss::Div, 0, 4);
    if rising {
        let next = sse::ss(c, Ss::Add, 0, g.u32(fade)?);
        sse::comiss(c, next, sse::lane0(c, 3));
        if !sse::below_or_equal(c) {
            sse::movaps(c, 0, 3);
        }
        g.set_u32(fade, sse::lane0(c, 0))
    } else {
        sse::load(c, 1, g.u32(fade)?);
        let next = sse::ss_reg(c, Ss::Sub, 1, 0);
        sse::comiss(c, next, sse::lane0(c, 2));
        if sse::below_or_equal(c) {
            sse::movaps(c, 1, 2);
        }
        g.set_u32(fade, sse::lane0(c, 1))
    }
}

/// The three history words are displayed shield strength, change age and
/// last tick. A decrease holds the previous strength for the tag's delay.
/// A rising shield switches the age to the guest's charging indicator.
fn update_shield(c: &mut CpuAbi, g: &mut impl Memory) -> Result<()> {
    let displayed = c.edi;
    let age = displayed.wrapping_add(4);
    let last_tick = displayed.wrapping_add(8);
    let tick = c.edx.wrapping_add(8);
    let timestep = c.edx.wrapping_add(4);
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.ecx = g.u32(c.ecx.wrapping_add(guest_layout::datum::layout::DATA))?;
    sse::load(c, 0, g.u32(displayed)?);
    c.eax = (c.eax & c.ebx).wrapping_mul(3);
    c.eax = g.u32(c.ecx.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(8))?;
    let shield = c.eax.wrapping_add(0xf0);
    sse::comiss(c, sse::lane0(c, 0), g.u32(shield)?);
    if !sse::below_or_equal(c) {
        sse::comiss(c, sse::lane0(c, 2), g.u32(age)?);
        let restart = if !sse::below_or_equal(c) {
            true
        } else {
            sse::load(c, 0, g.u32(age)?);
            sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 3));
            !sse::below_or_equal(c)
        };
        if restart {
            c.ecx = g.u32(tick)?;
            g.set_u32(last_tick, c.ecx)?;
        }
        c.ecx = g.u32(last_tick)?;
        c.ebx = g.u32(tick)?;
        sse::load(c, 1, g.u32(SHIELD_HOLD)?);
        c.ebx = arithmetic(c, c.ebx, c.ecx, true);
        sse::cvtsi2ss(c, 0, c.ebx);
        let delta = sse::ss(c, Ss::Mul, 0, g.u32(timestep)?);
        sse::comiss(c, sse::lane0(c, 1), delta);
        if !sse::below_or_equal(c) {
            g.set_u32(age, sse::lane0(c, 2))?;
            return Ok(());
        }
        c.eax = g.u32(shield)?;
        g.set_u32(displayed, c.eax)?;
        c.eax = arithmetic(c, g.u32(tick)?, c.ecx, true);
    } else {
        sse::load(c, 0, g.u32(shield)?);
        c.eax = g.u32(shield)?;
        sse::comiss(c, sse::lane0(c, 0), g.u32(displayed)?);
        g.set_u32(displayed, c.eax)?;
        if !sse::below_or_equal(c) {
            sse::load(c, 0, g.u32(SHIELD_CHARGING)?);
            g.set_u32(age, sse::lane0(c, 0))?;
            c.eax = g.u32(tick)?;
            return g.set_u32(last_tick, c.eax);
        }
        sse::load(c, 0, g.u32(age)?);
        sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2));
        if sse::below_or_equal(c) {
            c.eax = g.u32(tick)?;
            return g.set_u32(last_tick, c.eax);
        }
        c.eax = arithmetic(c, g.u32(tick)?, g.u32(last_tick)?, true);
    }
    sse::cvtsi2ss(c, 0, c.eax);
    sse::ss(c, Ss::Mul, 0, g.u32(timestep)?);
    let elapsed = sse::ss(c, Ss::Add, 0, g.u32(age)?);
    g.set_u32(age, elapsed)?;
    c.eax = g.u32(tick)?;
    g.set_u32(last_tick, c.eax)
}

/// Offsets in the transient HUD view and seat record for its two weapons.
struct WeaponSlot {
    handle: u32,
    view: u32,
    state: u32,
    resume: u32,
}

const WEAPONS: [WeaponSlot; 2] = [
    WeaponSlot { handle: 0x268, view: 0x1ec, state: 0x18, resume: FIRST_WEAPON_DONE },
    WeaponSlot { handle: 0x264, view: 0x1b4, state: 0x20, resume: SECOND_WEAPON_DONE },
];

fn weapons(c: &mut CpuAbi, g: &mut impl Memory, first: usize) -> sim_native::Result {
    for slot in &WEAPONS[first..] {
        c.ebx = g.u32(c.ebp.wrapping_sub(slot.handle))?;
        arithmetic(c, c.ebx, NONE, true);
        if c.ebx != NONE {
            c.eax = c.ebp.wrapping_sub(FRAME_BYTES);
            push(c, g, c.eax)?;
            push(c, g, c.esi)?;
            c.edi = arithmetic(c, c.edi, slot.state, false);
            c.esi = c.ebp.wrapping_sub(slot.view);
            return g.call(c, 0x13aa27, slot.resume);
        }
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = c.ebp;
    c.ebp = pop(c, g)?;
    g.ret(c, 4)
}

fn step(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> sim_native::Result {
    super::leaf::at_site(c, g, pc);
    match pc {
        ENTRY => {
            push(c, g, c.ebp)?;
            c.ebp = c.esp;
            c.esp = arithmetic(c, c.esp, FRAME_BYTES, true);
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            c.esi = g.u32(c.ebp.wrapping_add(8))?;
            push(c, g, c.edi)?;
            let offset = c.esi.wrapping_mul(SEAT_BYTES);
            c.edi = arithmetic(c, offset, g.u32(HUD_SEATS)?, false);
            c.eax = c.esi;
            g.set_u32(c.ebp.wrapping_sub(4), c.edi)?;
            g.call(c, 0x2003dc, USER_PREPARED)
        }
        USER_PREPARED => {
            push(c, g, c.esi)?;
            c.eax = c.ebp.wrapping_sub(FRAME_BYTES);
            g.call(c, 0x13992a, HUD_GATHERED)
        }
        HUD_GATHERED => {
            push(c, g, c.esi)?;
            c.ebx = c.ebp.wrapping_sub(FRAME_BYTES);
            g.call(c, 0x13a720, WIDGETS_UPDATED)
        }
        WIDGETS_UPDATED => {
            update_fade(c, g)?;
            c.ecx = c.esi;
            super::leaf::call(c, g, super::leaf::Leaf::UserTest, USER_TESTED, |c, g| step(c, g, USER_TESTED))
        }
        USER_TESTED => {
            logic(c, c.eax & 0xff, 0x80);
            c.ebx = 0xffff;
            if c.eax as u8 != 0 {
                return super::leaf::call(c, g, super::leaf::Leaf::UserPlayer, PLAYER_FOUND, |c, g| step(c, g, PLAYER_FOUND));
            }
            c.eax = NONE;
            weapons(c, g, 0)
        }
        PLAYER_FOUND => {
            c.ecx = g.u32(points::PLAYERS.base)?;
            c.ecx = g.u32(c.ecx.wrapping_add(guest_layout::datum::layout::DATA))?;
            let player = (c.eax & c.ebx).wrapping_mul(0x21c);
            c.eax = g.u32(c.ecx.wrapping_add(player).wrapping_add(0x2c))?;
            if c.eax != NONE {
                update_shield(c, g)?;
            }
            weapons(c, g, 0)
        }
        FIRST_WEAPON_DONE => {
            c.edi = g.u32(c.ebp.wrapping_sub(4))?;
            c.esi = g.u32(c.ebp.wrapping_add(8))?;
            weapons(c, g, 1)
        }
        SECOND_WEAPON_DONE => weapons(c, g, 2),
        _ => unreachable!("checked HUD selector"),
    }
}

pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<Option<TransferAbi>> {
    segment::<16, _>(c, g, pc, ENTRY..END, SITES, |c, g, pc| step(c, g, pc))
}

#[cfg(test)]
mod tests;
