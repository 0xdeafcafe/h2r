//! Engine commands are sampled at the simulation-update envelope. A frame is
//! committed when GAME_TICK returns. A seekable snapshot is taken later, at
//! a scheduler boundary, and carries both the logical tick and the HLE cursors.
use super::{Mode, Recording, Session};
use crate::snapshot_glue::BootMachine;
use anyhow::{Context, Result, ensure};
use film::{
    blam::{Event, Keyframe, SeatInput, Tick},
    manifest::DiskTracker,
    sidecar::SidecarWriter,
};
use std::{
    cell::Cell,
    sync::{Arc, Mutex},
};

thread_local! {
    static KEYFRAME: Cell<bool> = const { Cell::new(false) };
}

/// Frame-only policies may serialise state owned by the film's tape. An
/// ordinary agent snapshot has no tape with which to restore that state.
pub fn in_keyframe() -> bool {
    KEYFRAME.get()
}

/// Runs `work` as a keyframe's save or load: the parts serialise what
/// only a whole-machine keyframe may ([`in_keyframe`]). The bounce
/// search's fork is one with no film (docs/runtime/bounce-search.md, "The
/// fork").
pub(crate) fn keyframe_scope<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            KEYFRAME.set(self.0);
        }
    }
    let _guard = Restore(KEYFRAME.replace(true));
    work()
}

const CAPTURE_TICKS: u64 = 150;
const CAPTURE_RETRY_TICKS: u64 = 30;
const POINTS_PER_KEYFRAME: u64 = 6;
const DELTAS_PER_ANCHOR: u8 = 7;

#[derive(Clone)]
struct RamAnchor {
    tick: u64,
    name: String,
    bytes: Arc<Vec<u8>>,
    deltas: u8,
}

impl RamAnchor {
    fn base(&self) -> snapshot::Base<'_> {
        snapshot::Base {
            name: &self.name,
            ram: &self.bytes,
            dirty: None,
        }
    }
}

#[derive(Default)]
pub(super) struct Capture {
    pub(super) actions: Option<Vec<SeatInput>>,
    pub(super) events: Vec<Event>,
    paused: bool,
    points: CapturePoints,
}

impl Capture {
    /// After a game's film is cut: free its RAM anchor
    /// ([`CapturePoints::release_anchor`]).
    pub(super) fn next_game(&mut self) {
        self.points.release_anchor();
    }
}

#[derive(Default)]
pub(super) struct CapturePoints {
    next_attempt: u64,
    ordinal: u64,
    last_reason: Option<String>,
    /// The reason set of the last printed deferral line
    /// ([`reason_set`]).
    printed: Option<String>,
    /// Deferred attempts since a capture last landed.
    deferrals: u64,
    ram_anchor: Option<RamAnchor>,
}

impl CapturePoints {
    fn due(&self, tick: u64) -> bool {
        tick >= self.next_attempt.max(CAPTURE_TICKS) && tick % CAPTURE_TICKS < CAPTURE_RETRY_TICKS
    }

    fn retry(&mut self, tick: u64) {
        self.next_attempt = tick.saturating_add(1);
    }

    /// Free the cut game's RAM anchor. The next game's first snapshot
    /// starts a new group; the capture ordinals carry on with the tape.
    /// The next game's captures also start with a clean deferral slate:
    /// the last game's line must not mute this one's.
    pub(super) fn release_anchor(&mut self) {
        self.ram_anchor = None;
        self.printed = None;
        self.deferrals = 0;
        self.last_reason = None;
    }

    fn mark(&mut self, tick: u64) -> u64 {
        self.ordinal += 1;
        self.next_attempt = tick.saturating_add(CAPTURE_TICKS - tick % CAPTURE_TICKS);
        self.ordinal
    }

    /// Record a deferred attempt and return the line to print, if this
    /// reason set has not printed since the last capture landed
    /// ([`CapturePoints::captured`]). [`Session::try_capture_point`]
    /// prints it.
    fn deferred(&mut self, tick: u64, why: String) -> Option<String> {
        self.deferrals += 1;
        self.last_reason = Some(why.clone());
        let set = reason_set(&why);
        (self.printed.as_deref() != Some(set.as_str())).then(|| {
            self.printed = Some(set);
            format!("theater: capture at tick {tick} deferred: {why}")
        })
    }

    /// A capture landed, a barrier with or without a keyframe: close the
    /// deferral gap. Returns the line to print when one was open.
    fn captured(&mut self) -> Option<String> {
        let line = (self.deferrals != 0)
            .then(|| format!("theater: captures resumed after {} deferrals", self.deferrals));
        self.deferrals = 0;
        self.printed = None;
        self.last_reason = None;
        line
    }

    pub(super) fn snapshot(
        &self,
        ordinal: u64,
        tick: u64,
        match_start: Option<u64>,
        sidecar: Option<&SidecarWriter>,
        disk: Option<&Arc<Mutex<DiskTracker>>>,
    ) -> Option<SnapshotCapture> {
        if ordinal == 0 || !ordinal.is_multiple_of(POINTS_PER_KEYFRAME) || sidecar?.contains(tick) {
            return None;
        }
        // A lobby anchor shares little RAM with the loaded map. Start a
        // new group on the first full snapshot in the match.
        let anchor = self
            .ram_anchor
            .as_ref()
            .filter(|a| a.deltas < DELTAS_PER_ANCHOR && a.tick >= match_start.unwrap_or(0))
            .cloned();
        Some(SnapshotCapture {
            disk: Arc::clone(disk?),
            anchor,
        })
    }

    pub(super) fn publish(
        &mut self,
        tick: u64,
        sidecar: Option<&mut SidecarWriter>,
        mark_offset: u64,
        capture: Result<CapturedSnapshot>,
    ) {
        let Some(sidecar) = sidecar else { return };
        let result = capture.and_then(|capture| {
            sidecar.append(&capture.frame, capture.anchor_tick, mark_offset)?;
            if let Some(anchor) = &mut self.ram_anchor
                && capture.anchor_tick == Some(anchor.tick)
            {
                anchor.deltas += 1;
            } else {
                self.ram_anchor = capture.new_anchor;
            }
            Ok(())
        });
        match result {
            Ok(()) => self.captured(),
            Err(error) => self.deferred(tick, format!("{error:#}")),
        }
        .map(|line| eprintln!("{line}"));
    }
}

/// The deferral line is keyed by the set of reasons, not their order: the
/// kernel reports its blockers in whatever order its threads answer, so
/// one deferral alternates strings tick to tick.
fn reason_set(why: &str) -> String {
    let mut reasons: Vec<&str> = why.split(", ").collect();
    reasons.sort_unstable();
    reasons.dedup();
    reasons.join(", ")
}

pub(super) struct SnapshotCapture {
    disk: Arc<Mutex<DiskTracker>>,
    anchor: Option<RamAnchor>,
}

pub(super) struct CapturePosition {
    pub tick: u64,
    pub slices: u64,
    pub calls: u64,
    pub lobby_boundary: u64,
}

pub(super) struct CapturedSnapshot {
    frame: Keyframe,
    anchor_tick: Option<u64>,
    new_anchor: Option<RamAnchor>,
}

impl SnapshotCapture {
    /// Continues the marked D3D capture, using the section already in
    /// `out`. Called in keyframe scope and without the session lock.
    pub(super) fn save(
        self,
        session: &Session,
        machine: &mut BootMachine<'_>,
        out: snapshot::SnapshotWriter,
        at: CapturePosition,
    ) -> Result<CapturedSnapshot> {
        session.capture_ready(machine)?;
        vfs::flush_pending_metadata();
        let disk_delta = self
            .disk
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capture()?;
        let base = self.anchor.as_ref().map(RamAnchor::base);
        let bytes = machine
            .save_after_film_device(out, base.as_ref())
            .map_err(anyhow::Error::msg)?;
        session.capture_ready(machine)?;
        let new_anchor = if self.anchor.is_none() && machine.rt.is_some() {
            // Derive the immutable base from the bytes actually saved,
            // including the device's render-target writebacks.
            let saved = snapshot::Snapshot::open(&bytes, machine.shelf.build())?;
            Some(RamAnchor {
                tick: at.tick,
                name: saved.header.name.clone(),
                bytes: Arc::new(snapshot::guest::standalone_ram(&saved)?),
                deltas: 0,
            })
        } else {
            None
        };
        Ok(CapturedSnapshot {
            frame: Keyframe {
                tick: at.tick,
                slices: at.slices,
                calls: at.calls,
                lobby_boundary: at.lobby_boundary,
                snapshot: bytes,
                disk_delta,
                ram_anchor: None,
            },
            anchor_tick: self.anchor.map(|anchor| anchor.tick),
            new_anchor,
        })
    }
}

impl Recording {
    /// Whether a game is on the tape: captures belong to games
    /// (docs/game/film-format.md, "Recording every game"). Outside one
    /// (main menu, lobby, pregame, Theater, settings) no keyframe or
    /// snapshot is attempted, so the tape takes no capture material. The
    /// descriptor's match start appears when a game starts and is cleared
    /// when its film is cut or it is left ([`CapturePoints::release_anchor`]).
    pub(super) fn in_game(&self) -> bool {
        self.meta.descriptor().match_start_tick.is_some()
    }

    pub(super) fn post_tick(
        &mut self,
        guest: &runtime::Guest,
        tick: u64,
        post_hash: Option<u64>,
    ) -> Result<()> {
        let mut events = std::mem::take(&mut self.frame.events);
        events.extend(
            self.meta
                .observe(guest, tick, |source| self.unified.dropped(source))?,
        );
        if self.meta.take_cut_left() {
            // The left game's film is cut at the next slice's check, as a
            // game's end is (docs/game/film-format.md, "One film per game").
            self.match_over = true;
        }
        let inputs = self.frame.actions.take().unwrap_or_default();
        if let Some(film) = &self.film {
            use film::file::{RECORD_JOIN, RECORD_LEAVE, TICK_DIGEST, TICK_EVENTS};
            let mut seats = [None; film::columns::SEATS];
            for input in &inputs {
                if let Some(slot) = seats.get_mut(usize::from(input.seat)) {
                    *slot = input.action;
                }
            }
            let flags = if post_hash.is_some() { TICK_DIGEST } else { 0 }
                | if events.is_empty() { 0 } else { TICK_EVENTS };
            let mut film = film.lock().unwrap_or_else(|e| e.into_inner());
            film.push_tick(flags, post_hash.unwrap_or(0), &seats)?;
            for event in &events {
                match event {
                    Event::Join { player } => {
                        film.seat_record(player.seat, RECORD_JOIN, &[player.seat, player.kind as u8])?
                    }
                    Event::Leave { seat } | Event::Quit { seat } => {
                        film.seat_record(*seat, RECORD_LEAVE, &[*seat])?
                    }
                    _ => {}
                }
            }
        }
        let out = self.out.as_mut().context("engine tape is closed")?;
        let match_start = self.meta.descriptor().match_start_tick;
        if match_start == Some(tick) {
            self.unified.marker(&self.meta.descriptor().map.name);
        }
        let clock = self.unified.tick_clock();
        if let Some(clock) = clock {
            self.unified.objectives(guest, clock)?;
        }
        let records = self
            .unified
            .take_records(self.track.begins_chunk(), self.track.needs_pose_baseline())?;
        self.track.push_clocked(
            out,
            Tick {
                tick,
                inputs,
                events,
                closing_events: Vec::new(),
                post_hash,
                forge: Vec::new(),
                // The Firefight host log's entries applied this tick.
                firefight: crate::firefight::film::take(),
            },
            records,
            clock,
        )?;
        Ok(())
    }
}

impl Session {
    pub(super) fn record_actions(&self, inputs: Vec<SeatInput>) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        if let Mode::Record(r) = &mut s.mode
            && r.frame.actions.replace(inputs).is_some()
        {
            let tick = s.tick + 1;
            s.fail_at(
                tick,
                "two authoritative action batches arrived before a tick completed".into(),
            );
        }
    }

    /// The campaign timer stops ticking while paused. Sample the pause
    /// latch between scheduler slices so both edges reach the resumed tick.
    pub fn observe_pause(&self, guest: &runtime::Guest) {
        let Ok(time) = guest.u32(crate::telemetry::layout::GAME_TIME_PTR) else {
            return;
        };
        if time == 0 || guest.u8(time).ok() != Some(1) {
            return;
        }
        let Some(paused) = time.checked_add(1).and_then(|p| guest.u8(p).ok()) else {
            return;
        };
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        let paused = paused != 0;
        if let Mode::Record(r) = &mut s.mode {
            if r.frame.paused != paused {
                r.frame.events.push(Event::Pause { paused });
                r.frame.paused = paused;
            }
        } else if let Some(engine) = &mut s.engine {
            engine.observe_pause(paused);
        }
    }

    pub(super) fn with_keyframe<T>(&self, work: impl FnOnce() -> T) -> T {
        keyframe_scope(work)
    }

    pub(super) fn capture_ready(&self, machine: &mut BootMachine<'_>) -> Result<()> {
        let blocked = agent::SnapshotMachine::blockers(machine);
        anyhow::ensure!(blocked.is_empty(), "{}", blocked.join(", "));
        if self.video_menu.get().is_some()
            && machine
                .rt
                .is_some_and(|rt| super::video_menu::picker_in_stack(&rt.memory()))
        {
            anyhow::bail!("Video resolution picker is open");
        }
        Ok(())
    }

    pub fn capture_point_due(&self) -> bool {
        let s = self.lock();
        !s.complete
            && s.failure.is_none()
            && matches!(&s.mode, Mode::Record(r) if r.in_game() && r.frame.actions.is_none()
                && r.frame.events.is_empty() && r.frame.points.due(s.tick) && !r.match_over)
    }

    pub fn try_capture_point(
        &self,
        slices: u64,
        lobby_boundary: u64,
        machine: &mut BootMachine<'_>,
    ) {
        let tick = {
            let mut s = self.lock();
            let tick = s.tick;
            if s.complete || s.failure.is_some() {
                return;
            }
            let Mode::Record(rec) = &mut s.mode else {
                return;
            };
            if !rec.in_game()
                || rec.match_over
                || rec.frame.actions.is_some()
                || !rec.frame.events.is_empty()
                || !rec.frame.points.due(tick)
            {
                return;
            }
            rec.frame.points.retry(tick);
            tick
        };

        // Snapshot policies can consult the session through a feature or tap.
        // Run the device and the rest of the save outside the session lock.
        let started = std::time::Instant::now();
        let result: Result<()> = self.with_keyframe(|| {
            if let Err(error) = self.capture_ready(machine) {
                if let Mode::Record(rec) = &mut self.lock().mode
                    && let Some(line) = rec.frame.points.deferred(tick, error.to_string())
                {
                    eprintln!("{line}");
                }
                return Err(error);
            }
            let mut out = machine.film_snapshot_writer(tick);
            let device = machine.capture_film_device(&mut out)
                .context("capture requires a D3D device")?;

            let (capture, mark_offset, calls) = {
                let mut s = self.lock();
                let calls = s.calls;
                ensure!(!s.complete && s.failure.is_none(), "recording ended during capture");
                let Mode::Record(rec) = &mut s.mode else {
                    anyhow::bail!("recording ended during capture");
                };
                // The film file reaches this tick before the mark that says
                // the tape can end here.
                if let Some(actions) = &rec.film {
                    actions.lock().unwrap_or_else(|e| e.into_inner()).checkpoint()?;
                }
                let film = rec.out.as_mut().context("capture requires an open film")?;
                let mark = film::tape::Mark::D3dBarrier {
                    slices,
                    tick,
                    saved: device.is_ok(),
                };
                // Every mark can serve as the tape cursor for a replay-built
                // sidecar. Finish the call batch before writing the mark.
                let offset = rec
                    .batch
                    .flush(film)
                    .and_then(|()| film.chunk(film::MARK, &mark))
                    .and_then(|()| {
                        film.last_chunk_offset()
                            .context("missing capture mark offset")
                    });
                let mark_offset = match offset {
                    Ok(offset) => offset,
                    Err(error) => {
                        s.fail(format!("writing D3D capture boundary: {error:#}"));
                        return Err(error);
                    }
                };
                let points = &mut rec.frame.points;
                let ordinal = points.mark(tick);
                if let Err(error) = device {
                    if let Some(line) = points.deferred(tick, error.clone()) {
                        eprintln!("{line}");
                    }
                    anyhow::bail!("{error}");
                }
                let Some(capture) = points.snapshot(
                    ordinal,
                    tick,
                    rec.meta.descriptor().match_start_tick,
                    rec.sidecar.as_ref(),
                    Some(&rec.disk),
                ) else {
                    // A barrier without a keyframe still closes a deferral
                    // gap: the attempt landed.
                    if let Some(line) = points.captured() {
                        eprintln!("{line}");
                    }
                    return Ok(());
                };
                (capture, mark_offset, calls)
            };

            let capture = capture.save(
                self,
                machine,
                out,
                CapturePosition {
                    tick,
                    slices,
                    calls,
                    lobby_boundary,
                },
            );
            let mut s = self.lock();
            if !s.complete
                && s.failure.is_none()
                && let Mode::Record(rec) = &mut s.mode
            {
                rec.frame
                    .points
                    .publish(tick, rec.sidecar.as_mut(), mark_offset, capture);
                if let Some(reason) = &rec.frame.points.last_reason {
                    anyhow::bail!("{reason}");
                }
            }
            Ok(())
        });
        if let Some(run) = &mut self.lock().measurements {
            run.capture(tick, slices, started, result.err().map(|error| error.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's repeating line: the kernel names its two continuations
    /// in alternating order, so the joined blocker string changes every
    /// tick while the deferral stays the same.
    const CONTINUATIONS: [&str; 2] = [
        "kernel: thread 2: continuation feature: after hooks -> 0xa0fa30bc",
        "kernel: thread 2: continuation feature: after hooks -> 0xa0fc1000",
    ];

    #[test]
    fn a_deferral_prints_once_per_reason_set_until_a_capture_lands() {
        let mut points = CapturePoints::default();
        let menus = |a: usize, b: usize| {
            [
                CONTINUATIONS[a],
                CONTINUATIONS[b],
                "features: pause-rows: not audited for snapshots",
                "features: pause-mp-rows: not audited for snapshots",
            ]
            .join(", ")
        };
        let first = menus(0, 1);
        assert_eq!(
            points.deferred(3463, first.clone()),
            Some(format!("theater: capture at tick 3463 deferred: {first}"))
        );
        // The same four reasons in the other order stay quiet.
        for tick in [3464, 3465, 3466] {
            assert!(points.deferred(tick, menus(1, 0)).is_none());
        }
        // A new reason joins the set: one line for it, then quiet again.
        let grown = format!("{first}, d3d: push-buffer busy");
        assert!(points
            .deferred(3467, grown.clone())
            .is_some_and(|l| l.ends_with("d3d: push-buffer busy")));
        assert!(points.deferred(3468, grown).is_none());

        // The capture lands: one resume line naming the gap, then nothing.
        assert_eq!(
            points.captured(),
            Some("theater: captures resumed after 6 deferrals".to_string())
        );
        assert!(points.captured().is_none());
        // The same deferral prints again after the resume.
        assert!(points.deferred(4000, menus(0, 1)).is_some());
    }

    #[test]
    fn a_new_game_starts_a_clean_deferral_slate() {
        let mut points = CapturePoints::default();
        let why = [CONTINUATIONS[0], "features: pause-rows: not audited for snapshots"].join(", ");
        assert!(points.deferred(300, why.clone()).is_some());
        points.release_anchor();
        // The cut game's line does not mute the next game's.
        assert_eq!(
            points.deferred(5000, why.clone()),
            Some(format!("theater: capture at tick 5000 deferred: {why}"))
        );
        assert_eq!(
            points.captured(),
            Some("theater: captures resumed after 1 deferrals".to_string())
        );
        assert!(points.last_reason.is_none());
    }
}
