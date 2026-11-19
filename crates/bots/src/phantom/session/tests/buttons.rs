//! Exercise the simulation gather hook over a real-sized synthetic slot.
//! Guest calls stand in for map-dependent queries (which seat, weapon, zoom).
use super::*;
use crate::{
    local::{Frame, PadSource},
    phantom::{Identity, Phantoms, machine_id},
    tuning::PhantomLatency,
};
use engine::entry::{self, GuestCall, Regs, Ret};
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
                ..Default::default()
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
            Box::pin(async move {
                ph.slot_action(cx, slot, |s| Ok(fixture_record(s.pad.gamepad)))
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
    record[0x3c..0x40].copy_from_slice(&0xe0020002u32.to_le_bytes());
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
    for (ping, delay) in [(0., 0), (80., 2)] {
        let (ph, mut g, slot) = game_with_latency(
            move |f| pads[(f.tick.unwrap() as usize - 100) % 3],
            PhantomLatency {
                ping_ms: Some(ping),
                jitter_ms: 0.,
            },
        );
        for tick in [100, 101, 102, 102, 103, 104, 105] {
            at_tick(&mut g, tick);
            g = gather(&ph, g, slot);
            let pad = if tick < 100 + delay {
                HostPadState::default()
            } else {
                pads[((tick - 100 - delay) % 3) as usize]
            };
            let local = fixture_record(pad.to_gamepad(&Default::default(), false));
            let mut got = [0; ACTION_SIZE];
            g.read(OUT, &mut got).unwrap();
            assert_eq!(got, local);
        }
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
fn aim_view_separates_intended_arrived_and_observed_aim() {
    let (ph, mut g, slot) = game_with_latency(
        |f| {
            if f.tick == Some(100) {
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
    at_tick(&mut g, 101);
    g = gather(&ph, g, slot);
    // Unit state may still lag the newly accepted angle.
    let next = ph.aim_view(&g, bot).unwrap();
    assert_eq!((next.stick, next.trigger), ([0., 0.], 0.));
    assert_eq!((next.arrived_stick, next.arrived_trigger), ([1., 0.], 1.));
    assert!((next.action_deg.unwrap()[0] - 1.0f32.to_degrees()).abs() < 0.001);
    assert_eq!(next.unit_forward, [1., 0., 0.]);
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
