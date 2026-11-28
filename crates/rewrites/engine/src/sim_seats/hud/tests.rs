//! The seat record's fade and shield histories on synthetic guest memory
//! (no image): the fade walking toward the visible target and latching at
//! it, a shield drop held for the tag's delay, a rising shield switching
//! to the charging indicator, and every path leaving the frame the entry
//! built. The lifted `13a838` is the oracle in
//! `sim_seats_differential::hud_seat_update`.
use super::*;
use runtime::Guest;
use runtime::guest::Permissions;

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const SEAT: u32 = 0x0050_0000;
const TIME: u32 = 0x0050_1000;
const HEADER: u32 = 0x0060_4000;
const DATA: u32 = 0x0060_5000;
const PLAYER: u32 = 0x0060_6000;
const BODY: u32 = 0x0061_0000;

/// One seat with the given history and the unit it reports to. The fade
/// and the shield update read the seat through `edi` and the game time
/// through `edx`, as the entry leaves them.
fn seat(displayed: f32, age: f32, last_tick: u32, shield: f32, tick: u32, step: f32) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let w = |g: &mut Guest, a: u32, v: f32| g.set_u32(a, v.to_bits()).unwrap();
    w(&mut g, FULL_SCALE, 1.0);
    w(&mut g, FADE_SPEED, 0.5);
    w(&mut g, SHIELD_HOLD, 1.0);
    w(&mut g, SHIELD_CHARGING, 2.0);
    w(&mut g, SEAT, displayed);
    w(&mut g, SEAT + 4, age);
    g.set_u32(SEAT + 8, last_tick).unwrap();
    w(&mut g, SEAT + 0x28, 0.0);
    w(&mut g, BODY + 0xf0, shield);
    g.set_u32(TIME + 4, step.to_bits()).unwrap();
    g.set_u32(TIME + 8, tick).unwrap();
    g.set_u16(TIME + 2, 2).unwrap();
    word(&mut g, points::GAME_TIME.base, TIME);
    word(&mut g, points::OBJECTS.base, HEADER);
    word(&mut g, HEADER + guest_layout::datum::layout::DATA, DATA);
    word(&mut g, DATA + 2 * 12 + 8, BODY);
    word(&mut g, points::PLAYERS.base, PLAYER);
    word(&mut g, PLAYER + guest_layout::datum::layout::DATA, DATA);
    let mut c = CpuAbi {
        // The player the seat reports: mask 2 against ebx 3.
        eax: 0x0003_0002,
        ebx: 0x0000_0003,
        ecx: 0x1234_5678,
        edx: TIME,
        esi: 0x5151,
        edi: SEAT,
        eflags: 0x202,
        mxcsr: 0x1f80,
        ..CpuAbi::default()
    };
    // The fade update leaves lane 2 zeroed and lane 3 at full scale for
    // the shield update's holds and restarts.
    sse::zero(&mut c, 2);
    sse::load(&mut c, 3, 1.0f32.to_bits());
    (c, g)
}

fn word(g: &mut Guest, a: u32, v: u32) {
    g.set_u32(a, v).unwrap();
}

fn bits(g: &Guest, a: u32) -> f32 {
    f32::from_bits(g.u32(a).unwrap())
}

#[test]
fn the_fade_rises_by_the_frame_speed_and_latches_at_full() {
    // The widget update enables the seat and the player is visible.
    let (mut c, mut g) = seat(0.9, 0.2, 10, 0.5, 20, 0.5);
    g.set_u8(STACK - 0x144, 1).unwrap();
    g.set_u8(STACK - 0x141, 1).unwrap();
    c.ebp = STACK;
    g.set_u32(SEAT + 0x28, 0.9f32.to_bits()).unwrap();
    update_fade(&mut c, &mut g).unwrap();
    // 0.5 speed over the 2-frame count, clamped at 1.0.
    assert_eq!(bits(&g, SEAT + 0x28), 1.0);

    // Already at the target: no change, and the disabled seat neither.
    g.set_u32(SEAT + 0x28, 1.0f32.to_bits()).unwrap();
    update_fade(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT + 0x28), 1.0);
    g.set_u8(STACK - 0x144, 0).unwrap();
    g.set_u32(SEAT + 0x28, 0.25f32.to_bits()).unwrap();
    update_fade(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT + 0x28), 0.25);
}

#[test]
fn the_fade_falls_when_the_player_is_not_visible() {
    let (mut c, mut g) = seat(0.9, 0.2, 10, 0.5, 20, 0.5);
    g.set_u8(STACK - 0x144, 1).unwrap();
    g.set_u8(STACK - 0x141, 0).unwrap();
    c.ebp = STACK;
    g.set_u32(SEAT + 0x28, 1.0f32.to_bits()).unwrap();
    // Hidden, the target is zero: each update falls by 0.5 / 2 frames.
    update_fade(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT + 0x28), 0.75);
    update_fade(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT + 0x28), 0.5);
}

#[test]
fn a_shield_drop_is_held_until_the_tag_delay_passes() {
    // Ten ticks at half a second each: past the one-second hold.
    let (mut c, mut g) = seat(0.9, 0.2, 10, 0.5, 20, 0.5);
    update_shield(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT), 0.5);
    // (20 - 10) * 0.5 seconds of age added to the 0.2 it had.
    assert!((bits(&g, SEAT + 4) - 5.2).abs() < 1e-5);
    assert_eq!(g.u32(SEAT + 8).unwrap(), 20);

    // Inside the delay the old value holds and the age restarts; the
    // last tick stays where the restart left it.
    let (mut c, mut g) = seat(0.9, 0.2, 19, 0.5, 20, 0.5);
    update_shield(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT), 0.9);
    assert_eq!(bits(&g, SEAT + 4), 0.0);
    assert_eq!(g.u32(SEAT + 8).unwrap(), 19);
}

#[test]
fn a_rising_shield_switches_to_the_charging_indicator() {
    let (mut c, mut g) = seat(0.4, 1.5, 10, 0.9, 20, 0.5);
    update_shield(&mut c, &mut g).unwrap();
    assert_eq!(bits(&g, SEAT), 0.9);
    assert_eq!(bits(&g, SEAT + 4), 2.0);
    assert_eq!(g.u32(SEAT + 8).unwrap(), 20);

    // An equal shield with a positive age keeps accumulating.
    let (mut c, mut g) = seat(0.4, 0.25, 10, 0.4, 20, 0.5);
    update_shield(&mut c, &mut g).unwrap();
    assert!((bits(&g, SEAT + 4) - 5.25).abs() < 1e-5);
    assert_eq!(g.u32(SEAT + 8).unwrap(), 20);
}

#[test]
fn an_unregistered_selector_leaves_the_frame_untouched() {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let mut c = CpuAbi {
        esp: STACK,
        edi: SEAT,
        eflags: 0x202,
        mxcsr: 0x1f80,
        ..CpuAbi::default()
    };
    g.set_u32(STACK, 0x9000_0000).unwrap();
    let before = c;
    assert_eq!(run(&mut c, &mut g, ENTRY + 4).unwrap(), None);
    assert_eq!(c, before);
    assert_eq!(g.u32(STACK).unwrap(), 0x9000_0000);
}

#[test]
fn the_entry_prepares_the_seat_frame_and_calls_the_user_prepare() {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    g.set_u32(HUD_SEATS, SEAT).unwrap();
    let mut c = CpuAbi {
        esp: STACK,
        esi: 2,
        eflags: 0x202,
        mxcsr: 0x1f80,
        ..CpuAbi::default()
    };
    g.set_u32(STACK, 0x9000_0000).unwrap();
    g.set_u32(STACK + 4, 2).unwrap();
    let t = run(&mut c, &mut g, ENTRY).unwrap().unwrap();
    assert_eq!((t.target_pc, t.resume_pc), (0x0020_03dc, USER_PREPARED));
    // The seat record's index, scaled and based, saved below the frame.
    assert_eq!(g.u32(c.ebp - 4).unwrap(), SEAT + 2 * SEAT_BYTES);
    assert_eq!(c.esi, 2);
}
