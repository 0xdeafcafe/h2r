//! The default pad layout at the server's per-player action gather.
//! docs/game/bots.md §5.1 traces the offsets and guest helpers.
use std::collections::VecDeque;

use super::{
    Batch, Books, Phantoms, handle_of,
    session::{self, at::*},
};
use crate::{
    guest,
    human_input::TICK_MS,
    mind::feel::{Rng, reached},
    pad_source::Frame,
    think::{self, Report, lock_source},
    tuning::PhantomLatency,
};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::{objects, players};
use input::host::HostPadState;

/// Buffered controller reports on the remote action path. Build the guest
/// action on arrival so its absolute aim and weapon fields use current
/// host state. Stick positions coalesce. Each trigger transition needs
/// a sample of its own for the retail pressure reader.
pub(super) struct Uplink {
    rng: Rng,
    in_flight: VecDeque<(u32, HostPadState)>,
    trigger_changes: [VecDeque<(u32, f32)>; 2],
    sent: HostPadState,
    arrived: HostPadState,
    last_tick: Option<u32>,
    gated: bool,
    /// Time already spent waiting for this report's pipelined think.
    think_ticks: u32,
}

impl Uplink {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x7068_616e_746f_6d73),
            in_flight: VecDeque::new(),
            trigger_changes: Default::default(),
            sent: HostPadState::default(),
            arrived: HostPadState::default(),
            last_tick: None,
            gated: false,
            think_ticks: 0,
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
            .unwrap_or(0.0)
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
            let delay = ((one_way.max(0.0) / TICK_MS).ceil() as u32).saturating_sub(self.think_ticks);
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

#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Source of the accepted record, retained when the one-tick fallback
    /// repeats it. The player index is the simulation slot, not the peer ID.
    published_pad: Option<(u8, input::Gamepad)>,
}

/// Replay still runs slot validation and bridges at most one missed sample.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct FilmGather {
    sample: Option<(u32, u32)>,
    publication: Publication,
    #[serde(default)]
    published_pad: Option<(u8, [u8; input::Gamepad::SIZE])>,
}
impl GatherState {
    pub(super) fn for_film(self) -> FilmGather {
        FilmGather {
            sample: self.sample.map(|(unit, tick)| (unit.raw(), tick)),
            publication: self.publication,
            published_pad: self.published_pad.map(|(seat, pad)| (seat, pad.to_bytes())),
        }
    }
}
impl FilmGather {
    pub(super) fn restore(&self) -> Result<GatherState, String> {
        if self
            .published_pad
            .is_some_and(|(seat, _)| u32::from(seat) >= session::MAX)
        {
            return Err("invalid film phantom input seat".into());
        }
        Ok(GatherState {
            sample: self.sample.map(|(unit, tick)| (DatumHandle(unit), tick)),
            publication: self.publication,
            published_pad: self
                .published_pad
                .map(|(seat, bytes)| (seat, input::Gamepad::from_bytes(&bytes))),
        })
    }
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
    /// The XINPUT_GAMEPAD that produced each accepted phantom action in the
    /// current simulation update (after the emulated network delay).
    pub fn gathered_pads(&self, tick: u32) -> [Option<input::Gamepad>; session::MAX as usize] {
        let mut pads = [None; session::MAX as usize];
        let books = self.lock();
        if books.hosted_game.is_some() {
            for p in &books.list {
                let state = &p.buttons;
                if p.made
                    && state.sample.is_some_and(|(_, t)| t == tick)
                    && state.publication != Publication::Missing
                    && let Some((seat, pad)) = state.published_pad
                {
                    pads[usize::from(seat)] = Some(pad);
                }
            }
        }
        pads
    }

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
            let Some(seat) = seat_at(m, slot)? else {
                return Ok(None);
            };
            let mut books = self.lock();
            let books = &mut *books;
            let Some(session) = books.hosted_game else {
                return Ok(None);
            };
            // Only a joined phantom with a live slot and unit reaches its pad source.
            let Some(i) = books
                .list
                .iter()
                .position(|p| p.made && p.info.machine == seat.mac)
            else {
                return Ok(None);
            };
            let unit = seat.unit;
            if !guest::round::playing(m).unwrap_or(false) {
                // Drop queued input and any repeat of this seat's previous
                // action while retail holds the round closed.
                books.list[i].invalidate_input();
                return Ok(None);
            }
            let mut state = books.list[i].buttons;
            if state.sample == Some((unit, tick)) {
                return Ok((m.u32(slot + SLOT_STAMP)? == tick
                    && state.publication != Publication::Missing)
                    .then_some((books.list[i].info.handle, state, None)));
            }
            if state.sample.is_some() && state.sample != Some((unit, tick.wrapping_sub(1))) {
                books.list[i].invalidate_input();
                state = books.list[i].buttons;
            }
            let time = m.u32(guest_layout::points::GAME_TIME.base)?;
            let dt = m.f32(time + 4)?;
            let rate = m.u16(time + guest::GAME_TIME_RATE)?;
            let player_addr = seat.player_addr;
            let player = DatumHandle::new(players::Player::at(player_addr).salt(m)?, seat.index);
            let match_fading = match books.opening_fade {
                Some((when, fading)) if tick == when => fading,
                Some((when, false)) if reached(tick, when) => false,
                _ => guest::match_opening_fading(m, tick, rate)?,
            };
            books.opening_fade = Some((tick, match_fading));
            let retiring = !books.thinker.ready_for_game();
            let gated = retiring || match_fading || guest::phantom_spawn_fading(m, player_addr, rate)?;
            let mode = books.thinker.mode();
            let pad = if retiring {
                HostPadState::default()
            } else if let Some(report) = books.list[i].uplink.report_at(tick) {
                report
            } else {
                // Keep observing and planning while the pad is gated
                // (docs/game/bots.md §5.4).
                // Every phantom sampled at this tick reads one object scan.
                let mut ground = std::mem::take(&mut books.ground);
                let key = u64::from(session) << 32 | u64::from(tick);
                let report = crate::arms::share_ground(&mut ground, key, || {
                    if mode.batched() {
                        books.batch_pad(m, session, tick, slot, i)
                    } else {
                        Ok(books.list[i].serial_pad(m, tick))
                    }
                });
                books.ground = ground;
                let report = report?;
                let p = &mut books.list[i];
                p.shot_range = report.shot_range;
                p.uplink.think_ticks = report.think_ticks;
                p.uplink.sample_with_gate(tick, report.pad, report.latency, gated)
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
            Ok(Some((books.list[i].info.handle, state, Some((sample, gated)))))
        })?;
        let Some((handle, mut state, sample)) = input else {
            return Ok(Ret::new(0, 8));
        };
        if let Some((sample, gated)) = sample {
            let (tick, unit) = (sample.tick, sample.unit);
            let candidate = if let Some(bytes) = build(sample).await {
                // The builder's guest writes are discarded, so its control
                // update's clear of the facing flag is made here on the
                // live biped (docs/game/bots.md §5.1, "The facing release").
                if cx.write(|m| release_facing(m, unit))? {
                    self.lock().maintenance.facing_releases += 1;
                }
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
                    if publication == Publication::Built {
                        // index was checked against session::MAX above.
                        state.published_pad =
                            Some((sample.player.index() as u8, sample.pad.gamepad));
                    }
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

/// What a remote action slot says about its player, when the gather
/// would sample a pad for it: the slot's player is listed, has the
/// slot's machine id and a live unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotSeat {
    index: u32,
    player_addr: u32,
    mac: [u8; 6],
    unit: DatumHandle,
}

/// Clears the facing-set bit of the biped `unit`, as retail's control
/// update does for a local player once it has reseeded the look from the
/// unit's aim (`0x0018700b`). While the bit stays set, every build reseeds
/// the look from the aim and drops the look stick. Returns whether it was
/// set.
fn release_facing(m: &mut dyn GuestMemory, unit: DatumHandle) -> Result<bool, MemFault> {
    let Some((kind, body)) = objects::get(m, unit)? else {
        return Ok(false);
    };
    if kind != objects::BIPED {
        return Ok(false);
    }
    let at = body.addr + guest::BIPED_FLAGS;
    let flags = m.u8(at)?;
    if flags & guest::BIPED_FACING_SET == 0 {
        return Ok(false);
    }
    m.set_u8(at, flags & !guest::BIPED_FACING_SET)?;
    Ok(true)
}

fn seat_at(m: &dyn GuestMemory, slot: u32) -> Result<Option<SlotSeat>, MemFault> {
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
    Ok(Some(SlotSeat {
        index,
        player_addr: player.addr,
        mac,
        unit,
    }))
}

impl super::Phantom {
    /// The whole sample at this phantom's own gather ([`think::Mode::Serial`]).
    fn serial_pad(&mut self, m: &dyn GuestMemory, tick: u32) -> Report {
        let _timer = think::GameThreadTimer::start();
        self.reset_source_if_due();
        let mut source = lock_source(&self.source);
        let pad = source.pad(&Frame {
            frame: self.samples,
            tick,
            machine: self.info.machine,
            guest: m,
        });
        self.samples = self.samples.saturating_add(1);
        Report::from_source(pad, &**source)
    }
}

impl Books {
    /// Phantom `caller`'s pad from this tick's batch, sampling the batch
    /// at the tick's first gather ([`think::Mode::Batched`]).
    ///
    /// The batch holds every phantom whose slot the gather would sample
    /// now: the checks of [`Phantoms::slot_action`] against each phantom's
    /// own slot, an interrupted stream reset as there. Seats go in slot
    /// order, the order the gather visits them.
    fn batch_pad(
        &mut self,
        m: &dyn GuestMemory,
        session: u32,
        tick: u32,
        slot: u32,
        caller: usize,
    ) -> Result<Report, MemFault> {
        let handle = self.list[caller].info.handle;
        let current = self
            .batch
            .as_ref()
            .is_some_and(|b| b.session == session && b.tick == tick);
        if !current {
            let mut order = vec![(slot, caller)];
            for (j, p) in self.list.iter_mut().enumerate() {
                if j == caller || !p.made {
                    continue;
                }
                let Some(at) = session::slot_of(m, p.info.machine)? else {
                    continue;
                };
                if m.u32(at + 8)? != 4 {
                    continue;
                }
                let Some(seat) = seat_at(m, at)? else {
                    continue;
                };
                if seat.mac != p.info.machine
                    || p.buttons.sample == Some((seat.unit, tick))
                    || p.uplink.report_at(tick).is_some()
                {
                    continue;
                }
                if p.buttons.sample.is_some()
                    && p.buttons.sample != Some((seat.unit, tick.wrapping_sub(1)))
                {
                    p.invalidate_input();
                }
                order.push((at, j));
            }
            order.sort_unstable();
            let frames: Vec<u64> = order
                .iter()
                .map(|&(_, j)| {
                    let p = &mut self.list[j];
                    p.reset_source_if_due();
                    let frame = p.samples;
                    p.samples = p.samples.saturating_add(1);
                    frame
                })
                .collect();
            let seats: Vec<think::Seat<'_>> = order
                .iter()
                .zip(frames)
                .map(|(&(_, j), frame)| {
                    let p = &self.list[j];
                    think::Seat {
                        key: p.info.handle.0,
                        generation: p.generation,
                        source: &p.source,
                        frame,
                        machine: p.info.machine,
                    }
                })
                .collect();
            let reports = self.thinker.sample_reports(tick, m, &seats);
            let handles = order.iter().map(|&(_, j)| self.list[j].info.handle);
            self.batch = Some(Batch {
                session,
                tick,
                pads: handles.zip(reports).collect(),
            });
        }
        let batched = self
            .batch
            .as_ref()
            .and_then(|b| b.pads.iter().find(|(h, _)| *h == handle))
            .map(|&(_, pad)| pad);
        Ok(match batched {
            Some(pad) => pad,
            // Not seen by the batch's checks: sampled whole, as serial.
            None => self.list[caller].serial_pad(m, tick),
        })
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

    type Pads = Vec<Vec<HostPadState>>;

    /// Pads through each phantom's uplink as the gather sends them, gated
    /// until the opening fade clears at [`OPEN`]: what the brains gave,
    /// what arrived, and how many thinks each bot had.
    fn through_the_fade(mode: think::Mode) -> (Pads, Pads, Vec<usize>) {
        use crate::think::{Thinker, tests::fixture};
        let latency = PhantomLatency {
            ping_ms: Some(2.0 * TICK_MS),
            jitter_ms: 0.0,
        };
        let w = fixture::world(7);
        let mut thinker = Thinker::on_radio(mode, w.board);
        let g = guest_layout::testing::FakeGuest::new();
        let mut uplinks: Vec<Uplink> = (0..fixture::BOTS)
            .map(|i| Uplink::new(100 + u64::from(i)))
            .collect();
        let (mut thought, mut arrived) = (Vec::new(), Vec::new());
        for tick in 0..TICKS {
            let reports = thinker.sample_reports(tick, &g, &fixture::seats(&w, tick));
            arrived.push(
                reports.iter()
                    .zip(&mut uplinks)
                    .map(|(report, u)| {
                        u.think_ticks = report.think_ticks;
                        u.sample_with_gate(tick, report.pad, latency, tick < OPEN)
                    })
                    .collect::<Vec<_>>(),
            );
            thought.push(reports.into_iter().map(|report| report.pad).collect());
        }
        thinker.settle();
        let thinks = w
            .logs
            .iter()
            .map(|l| l.lock().unwrap_or_else(|e| e.into_inner()).len())
            .collect();
        (thought, arrived, thinks)
    }

    const OPEN: u32 = 56;
    const TICKS: u32 = 90;

    #[test]
    fn pipelined_bots_wait_for_the_black_screen_and_keep_their_latency() {
        let (serial, serial_arrived, serial_thinks) = through_the_fade(think::Mode::Serial);
        let (piped, piped_arrived, piped_thinks) = through_the_fade(think::Mode::Batched {
            threads: 3,
            pipeline: true,
        });
        // The brains observe and plan on every tick of the fade.
        assert_eq!(serial_thinks, vec![TICKS as usize; serial_thinks.len()]);
        assert_eq!(piped_thinks, serial_thinks);
        let idle = vec![HostPadState::default(); serial[0].len()];
        for tick in 0..OPEN as usize {
            assert_eq!(serial_arrived[tick], idle, "tick {tick}");
            assert_eq!(piped_arrived[tick], idle, "tick {tick}");
        }
        // The first clear tick sends the newest pad at once; pipelined, that
        // pad was thought a tick earlier.
        let open = OPEN as usize;
        assert_eq!(serial_arrived[open], serial[open]);
        assert_eq!(piped_arrived[open], piped[open]);
        assert_eq!(piped[open], serial[open - 1]);
        // Then the tick in the brain comes off the wire: the host gets the
        // same input on the same tick either way.
        for tick in open + 1..TICKS as usize {
            assert_eq!(piped_arrived[tick], serial_arrived[tick], "tick {tick}");
        }
        assert!(
            serial_arrived[open..]
                .iter()
                .flatten()
                .any(|p| *p != HostPadState::default())
        );
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
    }
}
