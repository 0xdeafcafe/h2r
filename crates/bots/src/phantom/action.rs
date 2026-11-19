//! The default pad layout at the server's per-player action gather.
//! docs/game/bots.md §5.1 traces the offsets and guest helpers.
use std::collections::VecDeque;

use super::{
    Phantoms, handle_of,
    session::{self, at::*},
};
use crate::{
    arms, guest,
    human_input::TICK_MS,
    local::Frame,
    mind::feel::{Rng, reached},
    tuning::PhantomLatency,
};
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::view::{objects, players};
use input::{AnalogButton, Buttons, host::HostPadState};

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
pub(super) struct ButtonState {
    sample: Option<(DatumHandle, u32)>,
    accepted: bool,
    thresholds: [u8; 8],
    pressure: [u8; 8],
    x_ticks: u32,
    y_ticks: u32,
    zoom_ticks: u32,
    restore_zoom: Option<u8>,
    zoom_latched: bool,
    crouch_down: bool,
    crouched: bool,
    jump_down: bool,
    suppress_jump: bool,
    grenade_swap_down: bool,
    suppress_grenade: bool,
}

fn held(ticks: u32, down: bool) -> u32 {
    if down { ticks.saturating_add(1) } else { 0 }
}

// Analog button hysteresis in the XInput reader (0x124c53..0x124cc9).
fn pressure_down(threshold: &mut u8, previous: u8, pressure: u8) -> bool {
    *threshold = if previous > *threshold {
        (*threshold).max(pressure.saturating_sub(32))
    } else {
        (*threshold).min(pressure.saturating_add(64))
    };
    // The action is for this simulation sample. XInput published its count
    // for the previous pressure while it read this one.
    pressure > *threshold
}

struct Sample {
    player: DatumHandle,
    unit: DatumHandle,
    body: u32,
    biped: bool,
    player_flags: u16,
    tick: u32,
    dt: f32,
    pad: HostPadState,
}

impl ButtonState {
    async fn action(&mut self, cx: &Cx, sample: Sample) -> Result<session::Action, EntryError> {
        let Sample {
            player_flags,
            unit,
            body,
            pad,
            dt,
            ..
        } = sample;
        let unit = unit.raw();
        let g = pad.to_gamepad(&Default::default(), false);
        let mut down = [false; 8];
        for (i, &pressure) in g.analog.iter().enumerate() {
            down[i] = pressure_down(&mut self.thresholds[i], self.pressure[i], pressure);
        }
        self.pressure = g.analog;
        let [jump, melee, x, y, swap_grenade, light, lt, rt] = down;
        let (current, left, mut a) = cx.read(|m| -> Result<_, MemFault> {
            let aim = m.vec3(body + guest::UNIT_AIM)?;
            let current = m.u8(body + arms::UNIT_WEAPON_INDEX)?;
            let left = m.u8(body + arms::UNIT_LEFT_WEAPON_INDEX)?;
            let requested = m.u8(body + 0x216)?;
            let primary = if requested < 4 { requested } else { current };
            let mut a = session::motion_from_pad(&pad, aim, primary as i8, dt);
            a.weapon_set = m.u16(body + 0x214)?;
            let secondary = m.u8(body + 0x217)?;
            a.secondary = (secondary < 4).then_some(secondary);
            let zoom = m.u8(body + 0x241)?;
            a.zoom = (zoom < 4).then_some(zoom);
            Ok((current, left, a))
        })?;
        let dual = current < 4 && left < 4;

        if jump {
            self.suppress_jump |= cx.read(|m| {
                let state = m.u32(0x0051_0c50)?;
                Ok::<_, MemFault>(state != 0 && m.u8(state + 5)? != 0)
            })?;
            if !self.suppress_jump {
                a.flags |= 2;
            }
            if !self.jump_down {
                a.buttons |= 0x100;
            }
        }
        self.jump_down = jump;
        self.suppress_jump &= jump;
        let crouch = g.buttons.contains(Buttons::LEFT_THUMB);
        if crouch && !self.crouch_down {
            let (x, y) = pad.left_stick;
            self.crouched = x * x + y * y < 0.25
                || (sample.biped
                    && cx.call(GuestCall::new(0xe4050).eax(unit)).await?.eax as u8 != 0);
        }
        self.crouch_down = crouch;
        self.crouched &= crouch;
        if self.crouched {
            a.flags |= 1;
        }
        if melee {
            a.flags |= 0x30;
        }
        if light {
            a.flags |= 0x0210_0004;
        }
        if rt {
            a.flags |= 0x0001_0040;
        }
        if !lt {
            self.suppress_grenade = false;
        }
        if lt && dual {
            self.suppress_grenade = true;
        }
        if lt && (dual || !self.suppress_grenade) {
            a.flags |= 0x0022_4800;
            if !dual {
                a.flags |= 0x0400_0000;
            }
        }
        if g.buttons.contains(Buttons::DPAD_LEFT) {
            a.flags |= 0x80;
        }
        if g.buttons.contains(Buttons::DPAD_RIGHT) {
            a.flags |= 0x100;
        }

        let switch_weapon = self.y_ticks != 0 && !y && !dual && player_flags & 0x20 == 0;
        self.x_ticks = held(self.x_ticks, x);
        self.y_ticks = held(self.y_ticks, y);
        if x || y {
            let seconds = cx.read(|m| {
                let globals = m.u32(0x004e_034c)?;
                let control = m.u32(globals + 0xf4)?;
                m.f32(control + 0x7c)
            })?;
            let threshold_ms = (seconds * 1000.) as u32;
            for (ticks, mask) in [(self.x_ticks, 1), (self.y_ticks, 0x10)] {
                if ticks != 0 {
                    a.buttons |= mask;
                    if (ticks as f32 * dt * 1000.) as u32 >= threshold_ms {
                        a.buttons |= mask << 1;
                    }
                }
            }
        }
        if switch_weapon || a.weapon == -1 {
            a.weapon = cx
                .call(GuestCall::new(0xcdeb0).args([
                    unit,
                    u32::MAX,
                    i32::from(a.weapon) as u32,
                    u32::from(switch_weapon),
                ]))
                .await?
                .eax as i8;
        }

        // The objective weapon, when carried, takes precedence over a switch.
        let forced = cx.call(GuestCall::new(0xcfe40).ecx(unit)).await?.eax as u16;
        if forced < 4 && forced != u16::from(current) {
            a.weapon = forced as i8;
            a.secondary = None;
        }

        let grenade = cx.read(|m| m.u8(body + 0x23d))?;
        let mut counts = [0; 2];
        cx.read(|m| m.read(body + arms::UNIT_GRENADES, &mut counts))?;
        // 0x186cb9 scans both types starting at the current type (or 0
        // for NONE), preferring the other stocked type on a new press.
        let start = if grenade < 2 { grenade } else { 0 };
        let other = start ^ 1;
        let change = grenade > 1 || (swap_grenade && !self.grenade_swap_down);
        self.grenade_swap_down = swap_grenade;
        let order = if change {
            [other, start]
        } else {
            [start, other]
        };
        a.grenade = order.into_iter().find(|&g| counts[g as usize] as i8 > 0);

        a.zoom = self.zoom(cx, &sample, &g, current, a.zoom).await?;
        Ok(a)
    }

    async fn zoom(
        &mut self,
        cx: &Cx,
        sample: &Sample,
        g: &input::Gamepad,
        current: u8,
        mut zoom: Option<u8>,
    ) -> Result<Option<u8>, EntryError> {
        let unit = sample.unit.raw();
        let click = g.buttons.contains(Buttons::RIGHT_THUMB);
        let previous_ticks = self.zoom_ticks;
        self.zoom_ticks = held(previous_ticks, click);
        if click || zoom.is_some() {
            if cx.call(GuestCall::new(0xc85c0).eax(unit)).await?.eax as u8 == 0 {
                zoom = None;
                self.zoom_latched = false;
            } else if self.zoom_ticks == 1 {
                let mut allowed = true;
                if zoom.is_none() {
                    allowed = g.analog(AnalogButton::LeftTrigger) <= 127
                        && g.analog(AnalogButton::RightTrigger) <= 127;
                    let (x, y) = sample.pad.right_stick;
                    if allowed && current < 4 && x * x + y * y > 0.64 {
                        let weapon = cx.read(|m| {
                            m.u32(sample.body + arms::UNIT_WEAPONS + 4 * u32::from(current))
                        })?;
                        allowed = weapon == u32::MAX
                            || cx.call(GuestCall::new(0x100f70).eax(weapon)).await?.eax as u8 != 0;
                    }
                }
                if allowed {
                    self.restore_zoom = zoom;
                    let level = cx
                        .call(
                            GuestCall::new(0xc8960)
                                .eax(unit)
                                .ecx(zoom.map_or(u32::MAX, u32::from)),
                        )
                        .await?
                        .eax as u16;
                    zoom = (level < 4).then_some(level as u8);
                    self.zoom_latched = zoom.is_some();
                }
            } else if !click && self.zoom_latched && previous_ticks > 16 {
                zoom = self.restore_zoom;
            }
        }
        if !click {
            self.zoom_latched = false;
        }
        Ok(zoom)
    }
}

impl Phantoms {
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
    pub async fn slot_action(&self, cx: Cx, slot: u32) -> Result<Ret, EntryError> {
        let out = cx.arg(1)?;
        let input = cx.read(|m| -> Result<_, MemFault> {
            let Some(tick) = guest::game_tick(m) else {
                return Ok(None);
            };
            let index = m.u32(slot)?;
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
            let Some((kind, body)) = objects::get(m, unit)? else {
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
            let player_flags = player.flags(m)?;
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
            let sample = Sample {
                player,
                unit,
                body: body.addr,
                biped: kind == objects::BIPED,
                tick,
                dt,
                player_flags,
                pad,
            };
            Ok(Some((p.info.handle, state, Some(sample))))
        })?;
        let Some((handle, mut state, sample)) = input else {
            return Ok(Ret::new(0, 8));
        };
        if let Some(sample) = sample {
            let Sample {
                tick, player, unit, ..
            } = sample;
            let a = state.action(&cx, sample).await?;
            cx.write(|m| m.write(out, &a.bytes()))?;
            // The same target also supplies melee and sword lunges on ticks without X.
            cx.call(GuestCall::new(0x151110).args([player.raw(), out + 0x28]))
                .await?;
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
