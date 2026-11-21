//! Engine commands are sampled at the simulation-update envelope. A frame is
//! committed when GAME_TICK returns. A seekable snapshot is taken later, at
//! a scheduler boundary, and carries both the logical tick and the HLE cursors.
use super::{Mode, Recording, Session};
use crate::snapshot_glue::BootMachine;
use anyhow::{Context, Result};
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

#[derive(Default)]
pub(super) struct CapturePoints {
    next_attempt: u64,
    ordinal: u64,
    last_reason: Option<String>,
    ram_anchor: Option<RamAnchor>,
}

impl CapturePoints {
    fn due(&self, tick: u64) -> bool {
        tick >= self.next_attempt.max(CAPTURE_TICKS) && tick % CAPTURE_TICKS < CAPTURE_RETRY_TICKS
    }

    fn retry(&mut self, tick: u64) {
        self.next_attempt = tick.saturating_add(1);
    }

    fn mark(&mut self, tick: u64) -> u64 {
        self.ordinal += 1;
        self.next_attempt = tick.saturating_add(CAPTURE_TICKS - tick % CAPTURE_TICKS);
        self.ordinal
    }

    fn deferred(&mut self, tick: u64, why: String) {
        if self.last_reason.as_deref() != Some(why.as_str()) {
            eprintln!("theater: capture at tick {tick} deferred: {why}");
            self.last_reason = Some(why);
        }
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
            Ok(()) => self.last_reason = None,
            Err(error) => self.deferred(tick, format!("{error:#}")),
        }
    }
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
        let inputs = self.frame.actions.take().unwrap_or_default();
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
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                KEYFRAME.set(self.0);
            }
        }
        let _guard = Restore(KEYFRAME.replace(true));
        work()
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
            && matches!(&s.mode, Mode::Record(r) if r.frame.actions.is_none()
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
            if rec.match_over
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
        self.with_keyframe(|| {
            if let Err(error) = self.capture_ready(machine) {
                if let Mode::Record(rec) = &mut self.lock().mode {
                    rec.frame.points.deferred(tick, error.to_string());
                }
                return;
            }
            let mut out = machine.film_snapshot_writer(tick);
            let Some(device) = machine.capture_film_device(&mut out) else {
                return;
            };

            let (capture, mark_offset, calls) = {
                let mut s = self.lock();
                let calls = s.calls;
                if s.complete || s.failure.is_some() {
                    return;
                }
                let Mode::Record(rec) = &mut s.mode else {
                    return;
                };
                let Some(film) = &mut rec.out else { return };
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
                        return;
                    }
                };
                let points = &mut rec.frame.points;
                let ordinal = points.mark(tick);
                if let Err(error) = device {
                    points.deferred(tick, error);
                    return;
                }
                let Some(capture) = points.snapshot(
                    ordinal,
                    tick,
                    rec.meta.descriptor().match_start_tick,
                    rec.sidecar.as_ref(),
                    Some(&rec.disk),
                ) else {
                    return;
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
            }
        });
    }
}
