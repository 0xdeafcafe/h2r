//! Exercise the simulation gather hook over a real-sized synthetic slot.
//! Guest calls stand in for map-dependent queries (which seat, weapon, zoom).
use super::*;
use crate::{
    local::Frame,
    phantom::{Identity, Phantoms, machine_id},
};
use engine::entry::{self, GuestCall, Regs, Ret};
use input::host::HostButton;

const STACK: u32 = 0x781000;
const OUT: u32 = 0x782000;
const UNIT: u32 = BODY + 0x400;

fn game(
    source: impl FnMut(&Frame<'_>) -> HostPadState + Send + 'static,
) -> (Phantoms, FakeGuest, u32) {
    let ph = Phantoms::default();
    let h = ph
        .add(
            Identity {
                gamertag: "Clanker".into(),
                xuid: 1,
                ..Default::default()
            },
            None,
            Box::new(source),
        )
        .unwrap();
    let mut g = hosted();
    ph.tick(&mut g, 0).unwrap();
    game_makes(&mut g, 1, machine_id(h), [1., 0., 0.]);
    ph.tick(&mut g, 1).unwrap();
    // Three simulation ticks to the long-hold threshold.
    g.put_u32(0x4e034c, 0x790000)
        .put_u32(0x7900f4, 0x791000)
        .put_f32(0x79107c, 0.1)
        .put_u8(UNIT + 0x23d, 0)
        .put(UNIT + 0x23e, &[2, 1]);
    let slot = slot_of(&g, machine_id(h)).unwrap().unwrap();
    assert!(ph.owns_action_slot(&g, slot));
    (ph, g, slot)
}

fn at_tick(g: &mut FakeGuest, tick: u32) {
    g.put_u32(TIME + guest::GAME_TIME_TICK, tick);
}

fn pad(button: HostButton) -> HostPadState {
    let mut pad = HostPadState::default();
    pad.buttons.set(button, true);
    pad
}

fn call(
    ph: &Phantoms,
    mut g: FakeGuest,
    slot: u32,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> entry::testing::Run {
    g.map(OUT, ACTION_SIZE as u32)
        .put_u32(STACK + 4, slot)
        .put_u32(STACK + 8, OUT);
    let ph = ph.clone();
    entry::testing::run(
        &move |cx| {
            let ph = ph.clone();
            Box::pin(async move { ph.slot_action(cx, slot).await })
        },
        g,
        Regs {
            esp: STACK,
            ..Default::default()
        },
        callee,
    )
}

fn guest_query(q: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
    let unit = DatumHandle::new(0xb001, 1).raw();
    let eax = match q.target {
        0xcfe40 => {
            assert_eq!(q.ecx, Some(unit));
            u32::MAX
        }
        0xcdeb0 => {
            assert_eq!(q.args, [unit, u32::MAX, 1, 1]);
            3
        }
        0xc85c0 => {
            assert_eq!(q.eax, Some(unit));
            1
        }
        0xc8960 => {
            assert_eq!(q.eax, Some(unit));
            assert_eq!(q.ecx, Some(u32::MAX));
            0
        }
        0x151110 => {
            assert_eq!(q.args, [DatumHandle::new(0xe001, 1).raw(), OUT + 0x28]);
            m.set_u16(OUT + 0x28, 5).unwrap();
            m.set_u16(OUT + 0x2a, 2).unwrap();
            m.set_u32(OUT + 0x2c, 0xb007_0007).unwrap();
            0
        }
        0x14d0a0 => {
            assert_eq!(q.ecx, Some(OUT));
            1
        }
        _ => panic!("unexpected guest call {q:?}"),
    };
    Regs {
        eax,
        ..Default::default()
    }
}

pub(super) fn gather(ph: &Phantoms, g: FakeGuest, slot: u32) -> FakeGuest {
    let run = call(ph, g, slot, guest_query);
    assert_eq!(run.result, Ok(Ret::new(1, 8)));
    let g = run.guest;
    let mut stored = [0; ACTION_SIZE];
    let mut output = [0; ACTION_SIZE];
    g.read(slot + SLOT_ACTION, &mut stored).unwrap();
    g.read(OUT, &mut output).unwrap();
    assert_eq!(stored, output);
    assert_eq!(
        g.u32(slot + SLOT_STAMP).unwrap(),
        guest::game_tick(&g).unwrap()
    );
    g
}

#[test]
fn each_default_button_reaches_the_remote_record() {
    use HostButton::*;
    for (button, flags, buttons, grenade, zoom) in [
        (South, 2, 0x100, 0, u16::MAX),
        (LeftStick, 1, 0, 0, u16::MAX),
        (East, 0x30, 0, 0, u16::MAX),
        (West, 0, 1, 0, u16::MAX),
        (North, 0, 0x10, 0, u16::MAX),
        (RightBumper, 0, 0, 1, u16::MAX),
        (LeftBumper, 0x0210_0004, 0, 0, u16::MAX),
        (RightStick, 0, 0, 0, 0),
        (DpadLeft, 0x80, 0, 0, u16::MAX),
        (DpadRight, 0x100, 0, 0, u16::MAX),
    ] {
        let (ph, g, slot) = game(move |_| pad(button));
        let g = gather(&ph, g, slot);
        assert_eq!(
            (g.u32(OUT).unwrap(), g.u16(OUT + 0x1c).unwrap()),
            (flags, buttons),
            "{button:?}"
        );
        assert_eq!(
            (g.u16(OUT + 0x22).unwrap(), g.u16(OUT + 0x24).unwrap()),
            (grenade, zoom)
        );
        assert_eq!(g.u16(OUT + 0x1e).unwrap(), 0x1234);
    }
}

#[test]
fn held_x_finds_the_seat_and_one_tick_taps_survive_repeated_reads() {
    let (ph, mut g, slot) = game(|f| match f.tick.unwrap() {
        100..=103 | 105 => pad(HostButton::West),
        _ => HostPadState::default(),
    });
    for (tick, buttons) in [
        (100, 1),
        (101, 1),
        (102, 3),
        (103, 3),
        (104, 0),
        (105, 1),
        (106, 0),
    ] {
        at_tick(&mut g, tick);
        g = gather(&ph, g, slot);
        assert_eq!(g.u16(OUT + 0x1c).unwrap(), buttons);
        assert_eq!(g.u16(OUT + 0x2a).unwrap(), 2);
        assert_eq!(g.u32(OUT + 0x2c).unwrap(), 0xb007_0007);
        let again = call(&ph, g, slot, |_, _| panic!("sampled twice in one tick"));
        assert_eq!(again.result, Ok(Ret::new(1, 8)));
        g = again.guest;
        assert_eq!(g.u16(OUT + 0x1c).unwrap(), buttons);
    }
}

#[test]
fn weapon_switch_is_on_y_release_unless_a_held_interaction_used_it() {
    for consumed in [false, true] {
        let (ph, mut g, slot) = game(|f| {
            if f.tick.unwrap() <= 102 {
                pad(HostButton::North)
            } else {
                HostPadState::default()
            }
        });
        for tick in 100..104 {
            at_tick(&mut g, tick);
            if consumed && tick == 103 {
                g.put_u16(PLAYERS_DATA + 0x21c + 2, 0x20);
            }
            g = gather(&ph, g, slot);
            let expected = match tick {
                100 | 101 => 0x10,
                102 => 0x30,
                _ => 0,
            };
            assert_eq!(g.u16(OUT + 0x1c).unwrap(), expected);
            assert_eq!(
                g.u8(OUT + 0x20).unwrap(),
                if tick == 103 && !consumed { 3 } else { 1 }
            );
        }
    }
}

#[test]
fn full_trigger_pulls_and_release_after_dual_wielding() {
    let (ph, mut g, slot) = game(|f| HostPadState {
        right_trigger: 0.75,
        left_trigger: if f.tick == Some(103) { 0.0 } else { 1.0 },
        ..Default::default()
    });
    for (tick, flags) in [
        (100, 0x0422_4800),
        (101, 0x0022_4800),
        (102, 0),
        (103, 0),
        (104, 0x0422_4800),
    ] {
        at_tick(&mut g, tick);
        let left = if tick == 101 { 2 } else { 0xff };
        g.put_u8(UNIT + 0x213, left).put_u8(UNIT + 0x217, left);
        g = gather(&ph, g, slot);
        assert_eq!(g.u32(OUT).unwrap(), flags | 0x10040);
        assert_eq!(g.u8(OUT + 0x21).unwrap(), left);
        assert_eq!(g.f32(OUT + 0x14).unwrap(), 0.75);
        assert_eq!(
            g.f32(OUT + 0x18).unwrap(),
            if tick == 103 { 0. } else { 1. }
        );
    }
}

#[test]
fn a_held_zoom_restores_on_release_and_a_short_tap_stays_zoomed() {
    for held_ticks in [1, 16, 17] {
        let (ph, mut g, slot) = game(move |f| {
            if f.tick.unwrap() < 100 + held_ticks {
                pad(HostButton::RightStick)
            } else {
                Default::default()
            }
        });
        for i in 0..=held_ticks {
            at_tick(&mut g, 100 + i);
            g = gather(&ph, g, slot);
            let zoom = g.u16(OUT + 0x24).unwrap();
            let expected = if i == held_ticks && held_ticks > 16 {
                u16::MAX
            } else {
                0
            };
            assert_eq!(zoom, expected);
            g.put_u8(UNIT + 0x241, zoom as u8);
        }
    }
}

#[test]
fn grenade_selection_changes_only_on_a_new_press_and_requires_stock() {
    let (ph, mut g, slot) = game(|f| {
        if f.tick != Some(102) {
            pad(HostButton::RightBumper)
        } else {
            Default::default()
        }
    });
    for (tick, grenade) in [
        (100, 1),
        (101, 1),
        (102, 1),
        (103, 0),
        (104, 1),
        (105, u16::MAX),
    ] {
        at_tick(&mut g, tick);
        if tick >= 104 {
            g.put_u8(UNIT + 0x23e, 0);
        }
        if tick == 105 {
            g.put_u8(UNIT + 0x23f, 0);
        }
        g = gather(&ph, g, slot);
        assert_eq!(g.u16(OUT + 0x22).unwrap(), grenade);
        g.put_u8(UNIT + 0x23d, grenade as u8);
    }
}

#[test]
fn only_valid_phantom_records_are_published() {
    let (ph, mut g, slot) = game(|_| pad(HostButton::South));
    assert!(!ph.owns_action_slot(&g, SIM + SIM_SLOTS));
    g.put_u32(slot + 8, 1);
    assert!(!ph.owns_action_slot(&g, slot));
    g.put_u32(slot + 8, 4);
    let run = call(&ph, g, slot, |q, m| {
        if q.target == 0x14d0a0 {
            Regs::default()
        } else {
            guest_query(q, m)
        }
    });
    assert_eq!(run.result, Ok(Ret::new(0, 8)));
    assert_eq!(run.guest.u32(slot + SLOT_STAMP).unwrap(), u32::MAX);
}

#[test]
fn a_frame_indexed_source_advances_only_when_the_simulation_samples_it() {
    use crate::local::{PadSource, Recorded};
    let mut recording = Recorded::new(vec![
        pad(HostButton::South),
        HostPadState::default(),
        pad(HostButton::South),
    ]);
    let (ph, mut g, slot) = game(move |f| recording.pad(f));
    for (render, tick, jump) in [
        (19, 100, true),
        (44, 100, true),
        (44, 101, false),
        (44, 102, true),
        (45, 0, false),
    ] {
        at_tick(&mut g, tick);
        ph.tick(&mut g, render).unwrap();
        g = gather(&ph, g, slot);
        assert_eq!(g.u32(OUT).unwrap() & 2 != 0, jump);
        assert_eq!(g.u16(OUT + 0x1c).unwrap() & 0x100 != 0, jump);
    }
}

#[test]
fn no_grenade_selection_scans_both_types_and_weapon_slots_stay_distinct() {
    let (ph, mut g, slot) = game(|f| {
        if f.frame == 0 {
            pad(HostButton::North)
        } else {
            Default::default()
        }
    });
    g.put_u8(UNIT + 0x23d, 0xff).put_u8(UNIT + 0x217, 3);
    g = gather(&ph, g, slot);
    assert_eq!(g.u16(OUT + 0x22).unwrap(), 1);
    assert_eq!(g.u8(OUT + 0x21).unwrap(), 3);
    at_tick(&mut g, 101);
    g = gather(&ph, g, slot);
    assert_eq!(g.u8(OUT + 0x20).unwrap(), 3);
    assert_eq!(g.u8(OUT + 0x21).unwrap(), 0xff);
}

#[test]
fn physical_pressure_uses_the_previous_samples_hysteresis() {
    let pressures = [255u8, 110, 100, 79, 74, 120, 160, 105, 90];
    let (ph, mut g, slot) = game(move |f| {
        let mut pressures_now = [0.; 6];
        pressures_now[1] = f32::from(pressures[f.frame as usize]) / 255.;
        HostPadState {
            analog_buttons: Some(pressures_now),
            ..Default::default()
        }
    });
    for (i, down) in [true, true, true, true, false, true, true, false, false]
        .into_iter()
        .enumerate()
    {
        at_tick(&mut g, 100 + i as u32);
        g = gather(&ph, g, slot);
        assert_eq!(
            g.u32(OUT).unwrap(),
            if down { 0x30 } else { 0 },
            "pressure {}",
            pressures[i]
        );
    }
}

#[test]
fn moving_crouch_checks_only_bipeds_and_stays_latched_until_release() {
    for kind in [0, 1] {
        let (ph, mut g, slot) = game(|f| {
            if f.frame < 2 {
                HostPadState {
                    left_stick: (1., 0.),
                    ..pad(HostButton::LeftStick)
                }
            } else {
                Default::default()
            }
        });
        g.put_u8(OBJECTS_DATA + 0xc + 3, kind);
        for i in 0..3 {
            at_tick(&mut g, 100 + i);
            let run = call(&ph, g, slot, |q, m| {
                if q.target == 0xe4050 {
                    assert_eq!((kind, i), (0, 0));
                    Regs {
                        eax: 1,
                        ..Default::default()
                    }
                } else {
                    guest_query(q, m)
                }
            });
            assert_eq!(run.result, Ok(Ret::new(1, 8)));
            g = run.guest;
            assert_eq!(g.u32(OUT).unwrap() & 1 != 0, kind == 0 && i < 2);
        }
    }
}
