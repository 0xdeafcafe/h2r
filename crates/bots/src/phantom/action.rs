//! The default pad layout at the server's per-player action gather.
//! docs/game/bots.md §5.1 traces the offsets and guest helpers.
use std::collections::VecDeque;

use super::{
    Phantoms, handle_of,
    session::{self, at::*},
};
use crate::{
    guest,
    human_input::TICK_MS,
    local::Frame,
    mind::feel::{Rng, reached},
    tuning::PhantomLatency,
};
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::view::{objects, players};
use input::host::HostPadState;

/// Buffered controller reports on the remote action path. Build the guest
/// action on arrival so its absolute aim and weapon fields use current
/// host state. A newer arrival supersedes any older queued reports.
pub(super) struct Uplink {
    rng: Rng,
    default_ping_ms: f32,
    in_flight: VecDeque<(u32, HostPadState)>,
    arrived: HostPadState,
    last_tick: Option<u32>,
}

impl Uplink {
    pub(super) fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x7068_616e_746f_6d73);
        Self {
            default_ping_ms: 40.0 + 40.0 * rng.unit(),
            rng,
            in_flight: VecDeque::new(),
            arrived: HostPadState::default(),
            last_tick: None,
        }
    }

    pub(super) fn clear(&mut self) {
        self.in_flight.clear();
        self.arrived = HostPadState::default();
        self.last_tick = None;
    }

    fn report_at(&self, tick: u32) -> Option<HostPadState> {
        (self.last_tick == Some(tick)).then_some(self.arrived)
    }

    fn sample(&mut self, tick: u32, pad: HostPadState, latency: PhantomLatency) -> HostPadState {
        let ping = latency
            .ping_ms
            .filter(|p| p.is_finite())
            .unwrap_or(self.default_ping_ms)
            .clamp(0.0, 1000.0);
        let jitter = if latency.jitter_ms.is_finite() {
            latency.jitter_ms.clamp(0.0, 50.0)
        } else {
            PhantomLatency::default().jitter_ms
        };
        let delay = if ping > 0.0 {
            let one_way = ping * 0.5 + jitter * self.rng.signed();
            (one_way.max(0.0) / TICK_MS).ceil() as u32
        } else {
            0
        };
        self.in_flight.push_back((tick.wrapping_add(delay), pad));
        if let Some(i) = self
            .in_flight
            .iter()
            .rposition(|&(due, _)| reached(tick, due))
        {
            self.arrived = self.in_flight[i].1;
            self.in_flight.drain(..=i);
        }
        self.last_tick = Some(tick);
        self.arrived
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct GatherState {
    sample: Option<(DatumHandle, u32)>,
    accepted: bool,
}

/// The arrived XInput report and the identity for one retail action build.
#[derive(Clone, Copy)]
pub struct SeatInput {
    pub player: DatumHandle,
    pub player_addr: u32,
    pub unit: DatumHandle,
    pub tick: u32,
    pub dt: f32,
    pub pad: input::State,
    pub reset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimView {
    pub tick: u32,
    pub stick: [f32; 2],
    pub arrived_stick: [f32; 2],
    pub trigger: f32,
    pub arrived_trigger: f32,
    pub action_deg: Option<[f32; 2]>,
    pub fire_primary: bool,
    pub assist_strength: [f32; 2],
    pub unit_forward: [f32; 3],
    pub assist_player: Option<u16>,
}

impl Phantoms {
    /// Read the last controller sample, the validated slot record and the
    /// current unit at one agent boundary. A respawn invalidates the sample.
    pub fn aim_view(&self, m: &dyn GuestMemory, handle: super::PhantomHandle) -> Option<AimView> {
        let books = self.lock();
        let p = books.list.iter().find(|p| p.info.handle == handle)?;
        books.hosted_game?;
        let (unit, tick) = p.buttons.sample?;
        let slot = session::slot_of(m, p.info.machine).ok()??;
        let index = i32::try_from(m.u32(slot).ok()?).ok()?;
        let player = players::array(m).ok()??.get_absolute(m, index).ok()??;
        if !p.made
            || !player.listed(m).ok()?
            || player.machine(m).ok()? != p.info.machine
            || player.unit(m).ok()? != Some(unit)
        {
            return None;
        }
        let (_, body) = objects::get(m, unit).ok()??;
        let observed = m.vec3(body.addr + guest::UNIT_AIM).ok()?;
        let sent = p
            .uplink
            .in_flight
            .back()
            .map_or(p.uplink.arrived, |&(_, pad)| pad);
        let accepted = p.buttons.accepted && m.u32(slot + SLOT_STAMP).ok()? == tick;
        let record = slot + SLOT_ACTION;
        let (action_deg, fire_primary, assist_strength, target) = if accepted {
            (
                Some([
                    m.f32(record + 4).ok()?.to_degrees(),
                    m.f32(record + 8).ok()?.to_degrees(),
                ]),
                m.u32(record).ok()? & 0x40 != 0,
                [m.f32(record + 0x50).ok()?, m.f32(record + 0x54).ok()?],
                DatumHandle::from_raw(m.u32(record + 0x3c).ok()?),
            )
        } else {
            (None, false, [0.; 2], None)
        };
        Some(AimView {
            tick,
            stick: sent.right_stick.into(),
            arrived_stick: p.uplink.arrived.right_stick.into(),
            trigger: sent.right_trigger,
            arrived_trigger: p.uplink.arrived.right_trigger,
            action_deg,
            fire_primary,
            assist_strength,
            unit_forward: observed,
            assist_player: target.map(|h| h.index()),
        })
    }

    /// Only a remote slot of a running match this console hosts can be answered.
    pub fn owns_action_slot(&self, m: &dyn GuestMemory, slot: u32) -> bool {
        if slot == 0 || m.u32(slot + 8) != Ok(4) {
            return false;
        }
        let mut mac = [0; 6];
        if m.read(slot + SLOT_MAC, &mut mac).is_err() || handle_of(mac).is_none() {
            return false;
        }
        let b = self.lock();
        b.hosted_game.is_some() && b.list.iter().any(|p| p.made && p.info.machine == mac)
    }

    /// 0x84990(slot, out), only for a slot matched by owns_action_slot.
    pub async fn slot_action(
        &self,
        cx: Cx,
        slot: u32,
        build: impl FnOnce(SeatInput) -> Result<[u8; ACTION_SIZE], EntryError>,
    ) -> Result<Ret, EntryError> {
        let out = cx.arg(1)?;
        let input = cx.read(|m| -> Result<_, MemFault> {
            let Some(tick) = guest::game_tick(m) else {
                return Ok(None);
            };
            let index = m.u32(slot)?;
            if index >= session::MAX {
                return Ok(None);
            }
            let Some(players) = players::array(m)? else {
                return Ok(None);
            };
            let Some(player) = players.get_absolute(m, index as i32)? else {
                return Ok(None);
            };
            let mac = player.machine(m)?;
            let mut slot_mac = [0; 6];
            m.read(slot + SLOT_MAC, &mut slot_mac)?;
            if slot_mac != mac || !player.listed(m)? {
                return Ok(None);
            }
            let Some(unit) = player.unit(m)? else {
                return Ok(None);
            };
            let Some(_) = objects::get(m, unit)? else {
                return Ok(None);
            };
            let mut books = self.lock();
            if books.hosted_game.is_none() {
                return Ok(None);
            }
            let Some(p) = books
                .list
                .iter_mut()
                .find(|p| p.made && p.info.machine == mac)
            else {
                return Ok(None);
            };
            let mut state = p.buttons;
            if state.sample == Some((unit, tick)) {
                return Ok(
                    (m.u32(slot + SLOT_STAMP)? == tick && state.accepted).then_some((
                        p.info.handle,
                        state,
                        None,
                    )),
                );
            }
            if state.sample.is_some() && state.sample != Some((unit, tick.wrapping_sub(1))) {
                p.reset_input();
                state = p.buttons;
            }
            let dt = m.f32(m.u32(engine::points::GAME_TIME.base)? + 4)?;
            let player_addr = player.addr;
            let player = DatumHandle::new(player.salt(m)?, index);
            let pad = if let Some(report) = p.uplink.report_at(tick) {
                report
            } else {
                let latency = p.source.phantom_latency();
                let pad = p.source.pad(&Frame {
                    frame: p.samples,
                    tick: Some(tick),
                    port: 4,
                    machine: Some(mac),
                    guest: m,
                    joining: false,
                });
                p.samples = p.samples.saturating_add(1);
                p.uplink.sample(tick, pad, latency)
            };
            let sample = SeatInput {
                player,
                player_addr,
                unit,
                tick,
                dt,
                reset: state.sample.is_none(),
                pad: input::State {
                    packet_number: tick,
                    gamepad: pad.to_gamepad(&Default::default(), false),
                },
            };
            Ok(Some((p.info.handle, state, Some(sample))))
        })?;
        let Some((handle, mut state, sample)) = input else {
            return Ok(Ret::new(0, 8));
        };
        if let Some(sample) = sample {
            let (tick, unit) = (sample.tick, sample.unit);
            let bytes = build(sample)?;
            cx.write(|m| m.write(out, &bytes))?;
            state.accepted = cx.call(GuestCall::new(0x14d0a0).ecx(out)).await?.eax as u8 != 0;
            if state.accepted {
                cx.write(|m| -> Result<(), MemFault> {
                    let mut bytes = [0; ACTION_SIZE];
                    m.read(out, &mut bytes)?;
                    m.write(slot + SLOT_ACTION, &bytes)?;
                    m.set_u32(slot + SLOT_STAMP, tick)
                })?;
            }
            state.sample = Some((unit, tick));
            if let Some(p) = self
                .lock()
                .list
                .iter_mut()
                .find(|p| p.info.handle == handle)
            {
                p.buttons = state;
            }
            if !state.accepted {
                return Ok(Ret::new(0, 8));
            }
        } else {
            cx.write(|m| {
                let mut bytes = [0; ACTION_SIZE];
                m.read(slot + SLOT_ACTION, &mut bytes)?;
                m.write(out, &bytes)
            })?;
        }
        Ok(Ret::new(1, 8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phantom_ping_and_jitter_replay_per_identity() {
        let report = HostPadState {
            right_trigger: 1.0,
            ..Default::default()
        };
        let latency = PhantomLatency {
            ping_ms: Some(2.0 * TICK_MS),
            ..Default::default()
        };
        let run = || {
            let mut uplink = Uplink::new(17);
            (0..300)
                .map(|tick| {
                    uplink
                        .sample(
                            tick,
                            if tick % 6 == 0 {
                                report
                            } else {
                                HostPadState::default()
                            },
                            latency,
                        )
                        .right_trigger
                        > 0.0
                })
                .collect::<Vec<_>>()
        };
        let seen = run();
        assert_eq!(seen, run());
        assert!(
            seen.iter()
                .enumerate()
                .all(|(t, &down)| !down || matches!(t % 6, 1 | 2))
        );
        assert!(seen.iter().skip(1).step_by(6).any(|&down| down));
        assert!(seen.iter().skip(2).step_by(6).any(|&down| down));

        let pings = (0..100)
            .map(|i| Uplink::new(i).default_ping_ms)
            .collect::<Vec<_>>();
        assert!(pings.iter().all(|p| (40.0..80.0).contains(p)));
        assert!(pings.iter().any(|&p| p < 50.0));
        assert!(pings.iter().any(|&p| p > 70.0));
    }
}
