//! Exercise the simulation gather hook over a real-sized synthetic slot.
//! Guest calls stand in for map-dependent queries (which seat, weapon, zoom).
use super::*;
use crate::{
    pad_source::{Frame, PadSource},
    phantom::{Identity, Phantoms, machine_id},
    tuning::PhantomLatency,
};
use guest_layout::entry::{self, GuestCall, Regs, Ret};
use input::host::HostButton;

const STACK: u32 = 0x781000;
const OUT: u32 = 0x782000;

struct TimedSource<F>(F, PhantomLatency);

impl<F: FnMut(&Frame<'_>) -> HostPadState + Send> PadSource for TimedSource<F> {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        self.0(f)
    }

    fn phantom_latency(&self) -> PhantomLatency {
        self.1
    }
}

fn game(
    source: impl FnMut(&Frame<'_>) -> HostPadState + Send + 'static,
) -> (Phantoms, FakeGuest, u32) {
    game_with_latency(
        source,
        PhantomLatency {
            ping_ms: Some(0.0),
            jitter_ms: 0.0,
        },
    )
}

fn game_with_latency(
    source: impl FnMut(&Frame<'_>) -> HostPadState + Send + 'static,
    latency: PhantomLatency,
) -> (Phantoms, FakeGuest, u32) {
    let ph = Phantoms::default();
    let h = ph
        .add(
            Identity {
                gamertag: "Clanker".into(),
                xuid: 1,
            },
            None,
            Box::new(TimedSource(source, latency)),
        )
        .unwrap();
    let mut g = hosted();
    ph.tick(&mut g, 0).unwrap();
    game_makes(&mut g, 1, machine_id(h), [1., 0., 0.]);
    ph.tick(&mut g, 1).unwrap();
    let slot = slot_of(&g, machine_id(h)).unwrap().unwrap();
    assert!(ph.owns_action_slot(&g, slot));
    (ph, g, slot)
}

/// Hold the old game's think while the render hook exits and the next
/// simulation gathers its first action. Its result would be full fire.
#[test]
fn a_running_think_neither_holds_the_shell_nor_fires_into_the_next_match() {
    use std::sync::mpsc;
    use std::time::Duration;

    struct Held {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl PadSource for Held {
        fn pad(&mut self, _: &Frame<'_>) -> HostPadState {
            unreachable!("batched input")
        }
        fn observe(&mut self, _: &Frame<'_>) -> crate::pad_source::Sampled {
            crate::pad_source::Sampled::Think
        }
        fn think(&mut self) -> HostPadState {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            HostPadState { right_trigger: 1.0, ..Default::default() }
        }
        fn conclude(&mut self) {
            panic!("the retired decision must not be published");
        }
    }

    let ph = Phantoms::default();
    let board = Box::leak(Box::new(std::sync::Mutex::new(Default::default())));
    ph.lock().thinker = crate::think::Thinker::on_radio(
        crate::think::Mode::Batched { threads: 1, pipeline: true }, board,
    );
    let (entered, started) = mpsc::channel();
    let (finish, release) = mpsc::channel();
    let h = ph.add(
        Identity { gamertag: "Gated".into(), xuid: 1 },
        None, Box::new(Held { entered, release }),
    ).unwrap();
    let start = move |ph: &Phantoms| {
        let mut g = hosted();
        ph.tick(&mut g, 0).unwrap();
        game_makes(&mut g, 1, machine_id(h), [1., 0., 0.]);
        ph.tick(&mut g, 1).unwrap();
        let slot = slot_of(&g, machine_id(h)).unwrap().unwrap();
        (g, slot)
    };
    let (g, slot) = start(&ph);
    let mut g = gather(&ph, g, slot);
    started.recv_timeout(Duration::from_secs(5)).unwrap();

    let (tx, rx) = mpsc::channel();
    let transition = std::thread::spawn(move || {
        g.put_u32(points::GAME_TIME.base, 0);
        ph.tick(&mut g, 2).unwrap();
        let (g, slot) = start(&ph);
        let g = gather(&ph, g, slot);
        tx.send(g.f32(slot + SLOT_ACTION + 0x14).unwrap()).unwrap();
        ph
    });
    let fire = rx.recv_timeout(Duration::from_secs(5));
    finish.send(()).unwrap();
    let _ph = transition.join().unwrap();
    assert_eq!(fire.unwrap(), 0.0);
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
    g: FakeGuest,
    slot: u32,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> entry::testing::Run {
    call_with_builder(ph, g, slot, |s| Some(fixture_record(s.pad.gamepad)), callee)
}

fn call_with_builder(
    ph: &Phantoms,
    mut g: FakeGuest,
    slot: u32,
    build: fn(crate::phantom::SeatInput) -> Option<[u8; ACTION_SIZE]>,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> entry::testing::Run {
    g.map(OUT, ACTION_SIZE as u32)
        .put_u32(STACK + 4, slot)
        .put_u32(STACK + 8, OUT);
    let ph = ph.clone();
    entry::testing::run(
        &move |cx| {
            let ph = ph.clone();
            Box::pin(async move {
                ph.slot_action(cx, slot, |s| std::future::ready(build(s)))
                    .await
            })
        },
        g,
        Regs {
            esp: STACK,
            ..Default::default()
        },
        callee,
    )
}

// An opaque local-builder result for a pad. Distinct angle, target and
// fire fields catch reconstruction of the record on the remote transport.
fn fixture_record(pad: input::Gamepad) -> [u8; ACTION_SIZE] {
    let mut record = [0xa5; ACTION_SIZE];
    let rt = pad.analog[input::AnalogButton::RightTrigger as usize];
    record[0..4].copy_from_slice(&(if rt > 0 { 0x10040u32 } else { 0 }).to_le_bytes());
    record[4..8].copy_from_slice(&(f32::from(pad.thumb_rx) / 32767.).to_le_bytes());
    record[8..12].copy_from_slice(&(f32::from(pad.thumb_ry) / 32767.).to_le_bytes());
    record[0x14..0x18].copy_from_slice(&(f32::from(rt) / 255.).to_le_bytes());
    let (object, player, strengths) = if rt > 0 {
        (0xb0030003u32, 0xe0020002u32, [0.75f32, 0.125])
    } else {
        (u32::MAX, u32::MAX, [0., 0.])
    };
    record[0x34..0x38].copy_from_slice(&object.to_le_bytes());
    record[0x3c..0x40].copy_from_slice(&player.to_le_bytes());
    record[0x50..0x54].copy_from_slice(&strengths[0].to_le_bytes());
    record[0x54..0x58].copy_from_slice(&strengths[1].to_le_bytes());
    record
}
fn guest_query(q: &GuestCall, _: &mut dyn GuestMemory) -> Regs {
    assert_eq!((q.target, q.ecx), (0x14d0a0, Some(OUT)));
    Regs {
        eax: 1,
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

fn active_pad() -> HostPadState {
    let mut pad = pad(HostButton::South);
    pad.buttons.set(HostButton::Start, true);
    pad.left_stick = (0.75, -0.5);
    pad.right_stick = (-0.75, 0.5);
    pad.left_trigger = 0.75;
    pad.right_trigger = 1.0;
    pad
}

#[test]
fn match_opening_waits_for_host_opacity_and_uses_the_first_open_sample() {
    const PLAYER_GLOBALS: u32 = 0x0077_0000;
    const GAME_ENGINE: u32 = 0x0077_1000;
    let (ph, mut g, slot) = game_with_latency(
        |f| {
            let tick = f.tick;
            assert_eq!(
                f.frame,
                u64::from(tick - 54),
                "the brain sampled during the fade"
            );
            if tick < 57 {
                pad(HostButton::East)
            } else {
                active_pad()
            }
        },
        PhantomLatency {
            ping_ms: Some(80.0),
            jitter_ms: 0.0,
        },
    );
    guest::round::tests::live_round(&mut g, GAME_ENGINE, PLAYER_GLOBALS);
    g.put_u32(
            PLAYER_GLOBALS + guest::LOCAL_MAP,
            DatumHandle::new(0xe000, 0).raw(),
        );
    for user in 1..guest::LOCAL_USER_COUNT {
        g.put_u32(PLAYER_GLOBALS + guest::LOCAL_MAP + 4 * user, u32::MAX);
    }
    for (tick, opacity) in [(54, 0.5), (55, 0.25), (56, 0.125), (57, 0.0)] {
        at_tick(&mut g, tick);
        g.put_f32(GAME_ENGINE + guest::LOCAL_BLACK_OPACITY, opacity);
        let run = call_with_builder(
            &ph,
            g,
            slot,
            |sample| {
                let expected = if sample.tick == 57 {
                    active_pad().to_gamepad(&Default::default(), false)
                } else {
                    input::Gamepad::default()
                };
                assert_eq!(sample.pad.gamepad, expected);
                Some(fixture_record(sample.pad.gamepad))
            },
            guest_query,
        );
        assert_eq!(run.result, Ok(Ret::new(1, 8)));
        let expected = if tick == 57 {
            active_pad().to_gamepad(&Default::default(), false)
        } else {
            input::Gamepad::default()
        };
        assert_eq!(ph.gathered_pads(tick)[1], Some(expected));
        g = run.guest;
    }

    // Seek into active play. The local human's later fade must still allow
    // retail to bridge a single missed phantom build with the accepted action.
    let saved = serde_json::to_vec(&ph.film_roster()).unwrap();
    let restored = Phantoms::default();
    restored
        .restore_film_roster(&serde_json::from_slice(&saved).unwrap())
        .unwrap();
    at_tick(&mut g, 58);
    g.put_f32(GAME_ENGINE + guest::LOCAL_BLACK_OPACITY, 1.0);
    let run = call_with_builder(&restored, g, slot, |_| None, guest_query);
    assert_eq!(run.result, Ok(Ret::new(1, 8)));
    assert_eq!(run.guest.f32(OUT + 0x14).unwrap(), 1.0);
    assert_eq!(restored.gathered_pads(58)[1], ph.gathered_pads(57)[1]);
}

#[test]
fn dedicated_match_follows_the_retail_opening_clock() {
    let (ph, mut g, slot) = game(|_| active_pad());
    for (tick, trigger) in [(55, 0.0), (56, 1.0)] {
        at_tick(&mut g, tick);
        g = gather(&ph, g, slot);
        assert_eq!(g.f32(OUT + 0x14).unwrap(), trigger);
    }
}

#[test]
fn a_phantom_respawn_gates_only_its_own_pad_until_the_fade_ends() {
    let (ph, mut g, slot) = game(|_| active_pad());
    let other = ph
        .add(
            Identity {
                gamertag: "Clanker Two".into(),
                xuid: 2,
            },
            None,
            Box::new(TimedSource(
                |_: &Frame<'_>| active_pad(),
                PhantomLatency {
                    ping_ms: Some(0.0),
                    jitter_ms: 0.0,
                },
            )),
        )
        .unwrap();
    ph.tick(&mut g, 2).unwrap();
    game_makes(&mut g, 2, machine_id(other), [1., 0., 0.]);
    ph.tick(&mut g, 3).unwrap();
    let other_slot = slot_of(&g, machine_id(other)).unwrap().unwrap();
    g = gather(&ph, g, slot);
    assert_eq!(g.f32(OUT + 0x14).unwrap(), 1.0);
    g = gather(&ph, g, other_slot);
    assert_eq!(g.f32(OUT + 0x14).unwrap(), 1.0);

    let player = PLAYERS_DATA + 0x21c;
    let unit = DatumHandle::new(0xb101, 1);
    g.put_u32(player + 0x2c, unit.raw())
        .put_u16(OBJECTS_DATA + 0xc, unit.salt());
    for age in 1..=10 {
        at_tick(&mut g, 100 + age);
        g.put_u32(player + guest::PLAYER_SPAWN_GRACE, 30 - age);
        let run = call_with_builder(
            &ph,
            g,
            slot,
            |sample| {
                let expected = if sample.tick == 110 {
                    active_pad().to_gamepad(&Default::default(), false)
                } else {
                    input::Gamepad::default()
                };
                assert_eq!(sample.pad.gamepad, expected);
                Some(fixture_record(sample.pad.gamepad))
            },
            guest_query,
        );
        assert_eq!(run.result, Ok(Ret::new(1, 8)));
        g = gather(&ph, run.guest, other_slot);
        assert_eq!(g.f32(OUT + 0x14).unwrap(), 1.0);
        let active = active_pad().to_gamepad(&Default::default(), false);
        let pads = ph.gathered_pads(100 + age);
        assert_eq!(
            pads[1],
            Some(if age < 10 {
                input::Gamepad::default()
            } else {
                active
            })
        );
        assert_eq!(pads[2], Some(active));
    }
}

#[test]
fn phantom_acquires_an_enemy_on_the_tick_it_clears_cover_and_stops_at_cover() {
    use crate::{
        Brain, arms,
        cache::MapData,
        los::Walls,
        mind::{Doing, Skill},
        tuning::{AimModel, Tuning},
    };
    use guest_layout::tags::{entry_layout, index_layout};
    use guest_layout::view::objects;
    use std::sync::Arc;

    // A tiny move across the wall's edge used to reuse a blocked ray
    // until the next visibility phase. Both endpoints are in gun range.
    let wall = [[2., -3., -1.], [2., 0., -1.], [2., 0., 3.], [2., -3., 3.]];
    let map = Arc::new(MapData {
        walls: Walls::from_polygons([wall.as_slice()]),
        ..Default::default()
    });
    // The acquisition, not the start-of-match warm-up.
    let mut brain = Brain::new(Skill::Legendary, 17)
        .with_tuning(Tuning {
            perception_ticks: Some(0),
            motor_ticks: Some(0),
            button_sigma_ms: Some(0.0),
            aim_model: Some(AimModel::Code),
            burst: Some(false),
            ..Default::default()
        })
        .without_warmup();
    // This fixture has zero transport and controller delay so the action
    // carries the current brain report. The skill reaction still applies.
    let (ph, mut g, slot) = game(move |frame| {
        assert_ne!(frame.machine, [0; 6]);
        let tick = frame.tick;
        let pad = brain.pad_in_map(frame, Some(&map));
        assert_eq!(
            brain.mind.doing == Doing::Fight(2),
            (101..116).contains(&tick),
            "phantom chose a target on tick {tick}"
        );
        pad
    });

    game_makes(&mut g, 2, MAC_B, [-1., 0., 0.]);
    let own = BODY + 0x400;
    let enemy = BODY + 0x800;
    g.put_f32(enemy + 0x64, 4.)
        .put_f32(enemy + 0x6c, 0.2)
        .put_f32(own + 0xec, 1.)
        .put_f32(own + 0xf0, 1.);

    // A full battery weapon in the phantom's current weapon slot.
    const INDEX: u32 = 0x0076_0000;
    const ENTRIES: u32 = INDEX + 0x1000;
    const WEAPON_TAG: u32 = ENTRIES + 0x1000;
    let def = DatumHandle::new(0xa003, 0).raw();
    let weapon = DatumHandle::new(0xb003, 3);
    let header = OBJECTS_DATA + 3 * 0xc;
    let body = BODY + 3 * 0x400;
    g.put_u32(points::TAG_INDEX.base, INDEX)
        .put_u32(INDEX + index_layout::TAG_COUNT, 1)
        .put_u32(points::TAG_ENTRIES.base, ENTRIES)
        .put_u32(ENTRIES + entry_layout::GROUP, u32::from_be_bytes(*b"weap"))
        .put_u32(ENTRIES + entry_layout::DATUM, def)
        .put_u32(ENTRIES + entry_layout::ADDRESS, WEAPON_TAG)
        .map(WEAPON_TAG, 0x400)
        .put_u16(header, weapon.salt())
        .put_u8(header + 3, objects::WEAPON)
        .put_u32(header + 8, body)
        .put_u32(body, def)
        .put_u32(body + 0x14, DatumHandle::new(0xb001, 1).raw())
        .put_u8(own + arms::UNIT_WEAPON_INDEX, 0)
        .put_u32(own + arms::UNIT_WEAPONS, weapon.raw());
    assert!(arms::read(&g, own).unwrap().held.unwrap().ready());

    let mut fired = false;
    for tick in 100..=116 {
        let exposed = (101..116).contains(&tick);
        at_tick(&mut g, tick);
        g.put_f32(enemy + 0x68, if exposed { 0.04 } else { -0.04 });
        g = gather(&ph, g, slot);
        let trigger = g.f32(OUT + 0x14).unwrap();
        if exposed {
            fired |= trigger > 0.;
        } else {
            assert_eq!(trigger, 0., "phantom fired into cover on tick {tick}");
        }
    }
    assert!(fired, "the phantom must fire while the enemy is exposed");
}

#[test]
fn missed_build_revalidates_one_previous_action_then_resumes_fresh_input() {
    let (ph, g, slot) = game(|f| HostPadState {
        right_trigger: if f.tick == 100 { 1. } else { 0. },
        ..Default::default()
    });
    let mut g = gather(&ph, g, slot);
    let first_pad = ph.gathered_pads(100)[1].expect("accepted action source");
    let mut previous = [0; ACTION_SIZE];
    g.read(OUT, &mut previous).unwrap();
    at_tick(&mut g, 101);
    let run = call_with_builder(
        &ph,
        g,
        slot,
        |_| None,
        |q, m| {
            let mut replay = [0; ACTION_SIZE];
            m.read(OUT, &mut replay).unwrap();
            assert_eq!(replay, previous);
            m.set_f32(OUT + 0x50, 0.25).unwrap();
            guest_query(q, m)
        },
    );
    assert_eq!(run.result, Ok(Ret::new(1, 8)));
    assert_eq!(run.guest.u32(slot + SLOT_STAMP).unwrap(), 101);
    assert_eq!(ph.gathered_pads(101)[1], Some(first_pad));
    let repeat = call_with_builder(
        &ph,
        run.guest,
        slot,
        |_| panic!("rebuilt"),
        |_, _| panic!("revalidated"),
    );
    assert_eq!(repeat.result, Ok(Ret::new(1, 8)));
    assert_eq!(repeat.guest.f32(OUT + 0x50).unwrap(), 0.25);
    let mut g = repeat.guest;
    at_tick(&mut g, 102);
    let missed = call_with_builder(&ph, g, slot, |_| None, |_, _| panic!("replayed twice"));
    assert_eq!(missed.result, Ok(Ret::new(0, 8)));
    assert_eq!(missed.guest.u32(slot + SLOT_STAMP).unwrap(), 101);
    assert!(ph.gathered_pads(102).iter().all(Option::is_none));
    let repeat = call_with_builder(
        &ph,
        missed.guest,
        slot,
        |_| panic!("retried in tick"),
        |_, _| panic!("validated missing input"),
    );
    assert_eq!(repeat.result, Ok(Ret::new(0, 8)));
    let mut g = repeat.guest;
    at_tick(&mut g, 103);
    let g = gather(&ph, g, slot);
    assert_eq!(g.u32(OUT).unwrap() & 0x40, 0);
}

#[test]
fn rejection_of_a_replayed_action_leaves_the_slot_stamp_untouched() {
    let (ph, g, slot) = game(|_| HostPadState::default());
    let mut g = gather(&ph, g, slot);
    at_tick(&mut g, 101);
    let run = call_with_builder(
        &ph,
        g,
        slot,
        |_| None,
        |q, m| {
            let _ = guest_query(q, m);
            m.set_u32(OUT, 0x1234).unwrap();
            Regs::default()
        },
    );
    assert_eq!(run.result, Ok(Ret::new(0, 8)));
    assert_eq!(run.guest.u32(slot + SLOT_STAMP).unwrap(), 100);
    assert_eq!(run.guest.u32(slot + SLOT_ACTION).unwrap(), 0);
}

#[test]
fn missed_build_ignores_absent_stale_and_respawned_records() {
    for case in ["first", "lost stamp", "respawn", "tick gap"] {
        let (ph, mut g, slot) = game(|_| HostPadState::default());
        if case != "first" {
            g = gather(&ph, g, slot);
        }
        at_tick(&mut g, if case == "tick gap" { 103 } else { 101 });
        if case == "respawn" {
            let unit = DatumHandle::new(0xb101, 1);
            g.put_u32(PLAYERS_DATA + 0x21c + 0x2c, unit.raw());
            g.put_u16(OBJECTS_DATA + 0xc, unit.salt());
        }
        if case == "lost stamp" {
            g.put_u32(slot + SLOT_STAMP, 97);
        }
        let stamp = g.u32(slot + SLOT_STAMP).unwrap();
        let run = call_with_builder(
            &ph,
            g,
            slot,
            |_| None,
            |_, _| panic!("validated stale action"),
        );
        assert_eq!(run.result, Ok(Ret::new(0, 8)), "{case}");
        assert_eq!(run.guest.u32(slot + SLOT_STAMP).unwrap(), stamp);
    }
}

#[test]
fn arrived_pad_publishes_the_whole_local_action_once_per_tick() {
    let pads = [
        HostPadState {
            right_stick: (0.25, -0.75),
            right_trigger: 1.,
            ..Default::default()
        },
        HostPadState {
            right_stick: (-0.625, 0.5),
            right_trigger: 0.,
            ..Default::default()
        },
        HostPadState::default(),
    ];
    // A scripted source already supplies this tick's report in pipeline
    // mode too, so it must pay its full configured wire delay.
    let pipeline = crate::think::Mode::Batched { threads: 0, pipeline: true };
    for (ping, delay, mode) in [
        (0., 0, crate::think::Mode::Serial),
        (80., 2, crate::think::Mode::Serial),
        (80., 2, pipeline),
    ] {
        let (ph, mut g, slot) = game_with_latency(
            move |f| pads[(f.tick as usize - 100) % 3],
            PhantomLatency {
                ping_ms: Some(ping),
                jitter_ms: 0.,
            },
        );
        ph.lock().thinker = crate::think::Thinker::on_radio(mode, Box::leak(Box::default()));
        for tick in [100, 101, 102, 102, 103, 104, 105] {
            at_tick(&mut g, tick);
            g = gather(&ph, g, slot);
            let pad = if tick < 100 + delay {
                HostPadState::default()
            } else {
                pads[((tick - 100 - delay) % 3) as usize]
            };
            let arrived = pad.to_gamepad(&Default::default(), false);
            assert_eq!(ph.gathered_pads(tick)[1], Some(arrived));
            let local = fixture_record(arrived);
            let mut got = [0; ACTION_SIZE];
            g.read(OUT, &mut got).unwrap();
            assert_eq!(got, local);
        }
    }
}

#[test]
fn assist_tracks_the_validated_action_across_repeated_gather_and_target_loss() {
    let (ph, mut g, slot) = game(|f| HostPadState {
        right_trigger: if f.tick == 100 { 1. } else { 0. },
        ..Default::default()
    });
    let handle = ph.list()[0].handle;
    let run = call(&ph, g, slot, |q, m| {
        let accepted = guest_query(q, m);
        // Only the validated output may cross into the stored action.
        m.set_f32(OUT + 0x50, 0.625).unwrap();
        accepted
    });
    assert_eq!(run.result, Ok(Ret::new(1, 8)));
    g = run.guest;
    g.put_u32(OUT + 0x34, 0);
    let repeat = call(&ph, g, slot, |_, _| panic!("validated twice"));
    assert_eq!(repeat.result, Ok(Ret::new(1, 8)));
    g = repeat.guest;
    assert_eq!(g.u32(OUT + 0x34).unwrap(), 0xb0030003);
    let aim = ph.aim_view(&g, handle).unwrap();
    assert_eq!(aim.assist_player, Some(2));
    assert_eq!(aim.assist_strength, [0.625, 0.125]);
    at_tick(&mut g, 101);
    g = gather(&ph, g, slot);
    let aim = ph.aim_view(&g, handle).unwrap();
    assert_eq!(g.u32(OUT + 0x34).unwrap(), u32::MAX);
    assert_eq!(aim.assist_player, None);
    assert_eq!(aim.assist_strength, [0., 0.]);
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
fn aim_view_separates_intended_arrived_and_observed_aim() {
    let (ph, mut g, slot) = game_with_latency(
        |f| {
            if f.tick == 100 {
                HostPadState {
                    right_stick: (1., 0.),
                    right_trigger: 1.,
                    ..Default::default()
                }
            } else {
                HostPadState::default()
            }
        },
        PhantomLatency {
            ping_ms: Some(40.),
            jitter_ms: 0.,
        },
    );
    let bot = ph.list()[0].handle;
    assert!(ph.aim_view(&g, bot).is_none());
    g = gather(&ph, g, slot);
    let first = ph.aim_view(&g, bot).unwrap();
    assert_eq!(first.tick, 100);
    assert_eq!((first.stick, first.trigger), ([1., 0.], 1.));
    assert_eq!((first.arrived_stick, first.arrived_trigger), ([0., 0.], 0.));
    assert_eq!(first.action_deg, Some([0., 0.]));
    assert_eq!(
        (first.assist_player, first.assist_strength),
        (None, [0., 0.])
    );
    at_tick(&mut g, 101);
    g = gather(&ph, g, slot);
    // Unit state may still lag the newly accepted angle.
    let next = ph.aim_view(&g, bot).unwrap();
    assert_eq!((next.stick, next.trigger), ([0., 0.], 0.));
    assert_eq!((next.arrived_stick, next.arrived_trigger), ([1., 0.], 1.));
    assert!((next.action_deg.unwrap()[0] - 1.0f32.to_degrees()).abs() < 0.001);
    assert_eq!(next.unit_forward, [1., 0., 0.]);
    assert_eq!(
        (next.assist_player, next.assist_strength),
        (Some(2), [0.75, 0.125])
    );
    // Rejected input has no action, even with a prior stamped record.
    at_tick(&mut g, 102);
    let run = call(&ph, g, slot, |q, m| {
        if q.target == 0x14d0a0 {
            Regs::default()
        } else {
            guest_query(q, m)
        }
    });
    assert_eq!(run.result, Ok(Ret::new(0, 8)));
    let mut g = run.guest;
    assert_eq!(ph.aim_view(&g, bot).unwrap().action_deg, None);
    g.put_u32(PLAYERS_DATA + 0x21c + 0x2c, u32::MAX);
    assert!(ph.aim_view(&g, bot).is_none());
}

#[test]
fn a_phantom_cannot_sample_or_replay_an_action_while_the_round_is_closed() {
    use guest::round::tests::{close, open};

    let (ph, mut g, slot) = game_with_latency(
        |f| {
            assert!(matches!(f.tick, 101 | 104), "sampled while closed");
            HostPadState {
                right_trigger: 1.0,
                ..Default::default()
            }
        },
        PhantomLatency {
            ping_ms: Some(40.0),
            jitter_ms: 0.0,
        },
    );
    close(&mut g, ENGINE);
    let run = call_with_builder(
        &ph,
        g,
        slot,
        |_| panic!("built an action while closed"),
        guest_query,
    );
    assert_eq!(run.result, Ok(Ret::new(0, 8)));
    let mut g = run.guest;

    open(&mut g, ENGINE);
    at_tick(&mut g, 101);
    g = gather(&ph, g, slot);
    assert_eq!(g.f32(OUT + 0x14).unwrap(), 0.0, "the shot is in flight");

    close(&mut g, ENGINE);
    at_tick(&mut g, 102);
    let run = call_with_builder(
        &ph,
        g,
        slot,
        |_| panic!("replayed an action while closed"),
        guest_query,
    );
    assert_eq!(run.result, Ok(Ret::new(0, 8)));
    let mut g = run.guest;
    open(&mut g, ENGINE);
    at_tick(&mut g, 104);
    g = gather(&ph, g, slot);
    assert_eq!(g.f32(OUT + 0x14).unwrap(), 0.0, "old round's shot was flushed");
}

/// Retail's control update clears a local biped's facing-set flag after it
/// reseeds the look from the aim; the builder's copy of that write is
/// discarded, so the action path makes it. Left set (as after a melee
/// lunge), every build would reseed the look and drop the right stick.
#[test]
fn a_built_action_releases_the_bipeds_facing_flag() {
    let (ph, mut g, slot) = game(|_| HostPadState {
        right_stick: (1.0, 0.0),
        ..Default::default()
    });
    let flags = BODY + 0x400 + guest::BIPED_FLAGS;
    // Bit 0 (airborne) belongs to the biped update and stays.
    g.put_u8(flags, guest::BIPED_FACING_SET | 1);
    let mut g = gather(&ph, g, slot);
    assert_eq!(g.u8(flags).unwrap(), 1, "facing flag released, others kept");
    assert_eq!(ph.maintenance().facing_releases, 1);

    // Already clear: nothing to release or count.
    at_tick(&mut g, 101);
    let mut g = gather(&ph, g, slot);
    assert_eq!(ph.maintenance().facing_releases, 1);

    // A missed build ran no control update, so the flag stays for the next.
    g.put_u8(flags, guest::BIPED_FACING_SET);
    at_tick(&mut g, 103);
    let missed = call_with_builder(&ph, g, slot, |_| None, |_, _| panic!("no action to validate"));
    assert_eq!(missed.result, Ok(Ret::new(0, 8)));
    assert_eq!(missed.guest.u8(flags).unwrap(), guest::BIPED_FACING_SET);
    assert_eq!(ph.maintenance().facing_releases, 1);

    // The next build releases it.
    let mut g = missed.guest;
    at_tick(&mut g, 104);
    let g = gather(&ph, g, slot);
    assert_eq!(g.u8(flags).unwrap(), 0);
    assert_eq!(ph.maintenance().facing_releases, 2);
}
