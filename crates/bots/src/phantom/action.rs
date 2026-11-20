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
    pad_source::Frame,
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
/// host state. Stick positions coalesce. Each trigger transition needs
/// a sample of its own for the retail pressure reader.
pub(super) struct Uplink {
    rng: Rng,
    default_ping_ms: f32,
    in_flight: VecDeque<(u32, HostPadState)>,
    trigger_changes: [VecDeque<(u32, f32)>; 2],
    sent: HostPadState,
    arrived: HostPadState,
    last_tick: Option<u32>,
    gated: bool,
}

impl Uplink {
    pub(super) fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x7068_616e_746f_6d73);
        Self {
            default_ping_ms: 40.0 + 40.0 * rng.unit(),
            rng,
            in_flight: VecDeque::new(),
            trigger_changes: Default::default(),
            sent: HostPadState::default(),
            arrived: HostPadState::default(),
            last_tick: None,
            gated: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.in_flight.clear();
        for changes in &mut self.trigger_changes {
            changes.clear();
        }
        self.sent = HostPadState::default();
        self.arrived = HostPadState::default();
        self.last_tick = None;
        self.gated = false;
    }

    fn report_at(&self, tick: u32) -> Option<HostPadState> {
        (self.last_tick == Some(tick)).then_some(self.arrived)
    }

    fn sample_with_gate(
        &mut self,
        tick: u32,
        pad: HostPadState,
        latency: PhantomLatency,
        gated: bool,
    ) -> HostPadState {
        // Drop any pre-fade input and publish a fresh sample when the gate
        // opens. The normal uplink delay resumes on the following tick.
        let latency = if gated || self.gated {
            PhantomLatency {
                ping_ms: Some(0.0),
                jitter_ms: 0.0,
            }
        } else {
            latency
        };
        self.gated = gated;
        let pad = if gated { HostPadState::default() } else { pad };
        self.sample(tick, pad, latency)
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
        self.sent = pad;
        if ping == 0.0 {
            self.in_flight.clear();
            for changes in &mut self.trigger_changes {
                changes.clear();
            }
            self.arrived = pad;
        } else {
            let one_way = ping * 0.5 + jitter * self.rng.signed();
            let delay = (one_way.max(0.0) / TICK_MS).ceil() as u32;
            let due = tick.wrapping_add(delay);
            let left = arrived_trigger(
                &mut self.trigger_changes[0],
                tick,
                due,
                pad.left_trigger,
                self.arrived.left_trigger,
            );
            let right = arrived_trigger(
                &mut self.trigger_changes[1],
                tick,
                due,
                pad.right_trigger,
                self.arrived.right_trigger,
            );
            self.in_flight.push_back((due, pad));
            if let Some(i) = self
                .in_flight
                .iter()
                .rposition(|&(due, _)| reached(tick, due))
            {
                self.arrived = self.in_flight[i].1;
                self.in_flight.drain(..=i);
            }
            self.arrived.left_trigger = left;
            self.arrived.right_trigger = right;
        }
        self.last_tick = Some(tick);
        self.arrived
    }
}

// A release can reach the host on the same tick as its press or even
// overtake it. Deliver pressure changes in send order, one per trigger
// per tick. Steady pressure adds nothing to the queue.
fn arrived_trigger(
    changes: &mut VecDeque<(u32, f32)>,
    tick: u32,
    due: u32,
    sent: f32,
    held: f32,
) -> f32 {
    if changes.back().map_or(held, |&(_, value)| value) != sent {
        changes.push_back((due, sent));
    }
    if changes.front().is_some_and(|&(due, _)| reached(tick, due)) {
        return changes.pop_front().unwrap().1;
    }
    held
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Publication {
    #[default]
    Missing,
    Built,
    Replayed,
}

#[derive(Clone, Copy, Default)]
pub(super) struct GatherState {
    sample: Option<(DatumHandle, u32)>,
    publication: Publication,
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
        let sent = p.uplink.sent;
        let accepted =
            p.buttons.publication != Publication::Missing && m.u32(slot + SLOT_STAMP).ok()? == tick;
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
    pub async fn slot_action<F>(
        &self,
        cx: Cx,
        slot: u32,
        build: impl FnOnce(SeatInput) -> F,
    ) -> Result<Ret, EntryError>
    where
        F: std::future::Future<Output = Option<[u8; ACTION_SIZE]>>,
    {
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
            let super::Books {
                hosted_game,
                opening_fade,
                list,
                ..
            } = &mut *books;
            if hosted_game.is_none() {
                return Ok(None);
            }
            // Only a joined phantom with a live slot and unit reaches its pad source.
            let Some(p) = list.iter_mut().find(|p| p.made && p.info.machine == mac) else {
                return Ok(None);
            };
            let mut state = p.buttons;
            if state.sample == Some((unit, tick)) {
                return Ok((m.u32(slot + SLOT_STAMP)? == tick
                    && state.publication != Publication::Missing)
                    .then_some((p.info.handle, state, None)));
            }
            if state.sample.is_some() && state.sample != Some((unit, tick.wrapping_sub(1))) {
                p.reset_input();
                state = p.buttons;
            }
            let time = m.u32(engine::points::GAME_TIME.base)?;
            let dt = m.f32(time + 4)?;
            let rate = m.u16(time + guest::GAME_TIME_RATE)?;
            let player_addr = player.addr;
            let player = DatumHandle::new(player.salt(m)?, index);
            let match_fading = match *opening_fade {
                Some((when, fading)) if tick == when => fading,
                Some((when, false)) if reached(tick, when) => false,
                _ => guest::match_opening_fading(m, tick, rate)?,
            };
            *opening_fade = Some((tick, match_fading));
            let gated = match_fading || guest::phantom_spawn_fading(m, player_addr, rate)?;
            let pad = if let Some(report) = p.uplink.report_at(tick) {
                report
            } else {
                let latency = p.source.phantom_latency();
                // Keep observing and planning while the pad is gated
                // (docs/game/bots.md §5.4).
                let pad = p.source.pad(&Frame {
                    frame: p.samples,
                    tick,
                    machine: mac,
                    guest: m,
                });
                p.samples = p.samples.saturating_add(1);
                p.uplink.sample_with_gate(tick, pad, latency, gated)
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
            Ok(Some((p.info.handle, state, Some((sample, gated)))))
        })?;
        let Some((handle, mut state, sample)) = input else {
            return Ok(Ret::new(0, 8));
        };
        if let Some((sample, gated)) = sample {
            let (tick, unit) = (sample.tick, sample.unit);
            let candidate = if let Some(bytes) = build(sample).await {
                Some((bytes, Publication::Built))
            } else if !gated
                && state.publication == Publication::Built
                && cx.read(|m| m.u32(slot + SLOT_STAMP))? == tick.wrapping_sub(1)
            {
                // A gap or unit change cleared the state above. Bridge one
                // missed tick without holding fire or a stale target indefinitely.
                let mut bytes = [0; ACTION_SIZE];
                cx.read(|m| m.read(slot + SLOT_ACTION, &mut bytes))?;
                Some((bytes, Publication::Replayed))
            } else {
                None
            };
            state.publication = Publication::Missing;
            if let Some((bytes, publication)) = candidate {
                cx.write(|m| m.write(out, &bytes))?;
                if cx.call(GuestCall::new(0x14d0a0).ecx(out)).await?.eax as u8 != 0 {
                    cx.write(|m| -> Result<(), MemFault> {
                        let mut bytes = [0; ACTION_SIZE];
                        m.read(out, &mut bytes)?;
                        m.write(slot + SLOT_ACTION, &bytes)?;
                        m.set_u32(slot + SLOT_STAMP, tick)
                    })?;
                    state.publication = publication;
                }
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
            if state.publication == Publication::Missing {
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
    fn coalesced_reports_preserve_fire_edges_and_take_the_latest_stick() {
        // Two ticks for the press, one for the release: both are due
        // at tick zero across the game-clock wrap.
        let mut u = Uplink::new(17);
        let send = |u: &mut Uplink, tick, ping, right, left, x| {
            u.sample(
                tick,
                HostPadState {
                    right_trigger: right,
                    left_trigger: left,
                    right_stick: (x, 0.),
                    ..Default::default()
                },
                PhantomLatency {
                    ping_ms: Some(ping),
                    jitter_ms: 0.,
                },
            )
        };
        let start = u32::MAX - 1;
        assert_eq!(send(&mut u, start, 100., 1., 0., 0.5).right_trigger, 0.);
        assert_eq!(
            send(&mut u, u32::MAX, 40., 0., 0.75, -0.5).right_trigger,
            0.
        );
        let arrived = send(&mut u, 0, 40., 0., 0.75, 0.);
        assert_eq!(
            (
                arrived.right_trigger,
                arrived.left_trigger,
                arrived.right_stick.0
            ),
            (1., 0.75, -0.5)
        );
        assert_eq!(u.sent.right_trigger, 0.);
        let arrived = send(&mut u, 1, 40., 0., 0.75, 0.);
        assert_eq!((arrived.right_trigger, arrived.left_trigger), (0., 0.75));

        // A delayed press for a despawned unit must never fire on respawn.
        send(&mut u, 2, 100., 1., 0., 0.);
        u.clear();
        assert_eq!(send(&mut u, 4, 40., 0., 0., 0.).right_trigger, 0.);
    }

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
        // Every requested pulse reaches retail even when jitter moves its
        // press and release onto the same sample.
        assert!(
            seen.chunks_exact(6)
                .all(|ticks| ticks.iter().any(|&down| down))
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
