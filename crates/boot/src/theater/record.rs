//! Engine commands are sampled at the simulation-update envelope. A frame is
//! committed when GAME_TICK returns. A seekable snapshot is taken later, at
//! a scheduler boundary, and carries both the logical tick and the HLE cursors.
use super::{Mode, Recording, Session};
use anyhow::{Context, Result};
use film::blam::{Event, Keyframe, SeatInput, Tick};
use std::cell::Cell;

thread_local! {
    static KEYFRAME: Cell<bool> = const { Cell::new(false) };
}

/// Frame-only policies may serialise state owned by the film's tape. An
/// ordinary agent snapshot has no tape with which to restore that state.
pub(crate) fn in_keyframe() -> bool {
    KEYFRAME.get()
}

const KEYFRAME_SECONDS: u64 = 30;
const DELTAS_PER_ANCHOR: u8 = 7;

#[derive(Clone)]
struct RamAnchor {
    tick: u64,
    name: String,
    bytes: std::sync::Arc<Vec<u8>>,
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
    next_attempt: u64,
    interval: u64,
    last_reason: Option<String>,
    ram_anchor: Option<RamAnchor>,
}

impl Recording {
    pub(super) fn post_tick(
        &mut self,
        guest: &runtime::Guest,
        tick: u64,
        post_hash: Option<u64>,
    ) -> Result<()> {
        let mut events = std::mem::take(&mut self.frame.events);
        events.extend(self.meta.observe(guest, tick, |source| self.unified.dropped(source))?);
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
        let rate = guest
            .u32(crate::telemetry::layout::GAME_GLOBALS_PTR)
            .ok()
            .filter(|p| *p != 0)
            .and_then(|p| {
                p.checked_add(
                    crate::telemetry::layout::OPTIONS + crate::telemetry::layout::OPTIONS_TICK_RATE,
                )
            })
            .and_then(|p| guest.u16(p).ok())
            .filter(|rate| (1..=240).contains(rate))
            .unwrap_or(30);
        self.frame.interval = KEYFRAME_SECONDS * u64::from(rate);
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
    pub(crate) fn observe_pause(&self, guest: &runtime::Guest) {
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

    pub(crate) fn keyframe_due(&self) -> bool {
        let s = self.lock();
        !s.complete
            && s.failure.is_none()
            && s.tick > 0
            && matches!(&s.mode, Mode::Record(r) if r.frame.actions.is_none()
                && r.frame.events.is_empty() && s.tick >= r.frame.next_attempt && !r.match_over)
    }

    pub(crate) fn try_keyframe(
        &self,
        slices: u64,
        lobby_boundary: u64,
        machine: &mut crate::snapshot_glue::BootMachine<'_>,
    ) {
        let (tick, calls, anchor, disk) = {
            let mut s = self.lock();
            let (tick, calls) = (s.tick, s.calls);
            let Mode::Record(rec) = &mut s.mode else {
                return;
            };
            rec.frame.next_attempt = tick.saturating_add(1);
            // A lobby anchor shares little RAM with the loaded map. Start a
            // new group at the first successful snapshot inside the match.
            let match_start = rec.meta.descriptor().match_start_tick.unwrap_or(0);
            let anchor = rec
                .frame
                .ram_anchor
                .as_ref()
                .filter(|a| a.deltas < DELTAS_PER_ANCHOR && a.tick >= match_start)
                .cloned();
            (tick, calls, anchor, std::sync::Arc::clone(&rec.disk))
        };

        // Feature policies may inspect this Session themselves. Never hold its
        // mutex while asking the snapshot machine to inspect/save the parts.
        let mut d3d_barrier = None;
        let mut new_anchor = None;
        let capture = self.with_keyframe(|| -> Result<(Vec<u8>, Vec<u8>)> {
            let ready = |m: &mut crate::snapshot_glue::BootMachine<'_>| -> Result<()> {
                let blocked = agent::SnapshotMachine::blockers(m);
                anyhow::ensure!(blocked.is_empty(), "{}", blocked.join(", "));
                Ok(())
            };
            ready(machine)?;
            vfs::flush_pending_metadata();
            let disk_delta = disk.lock().unwrap_or_else(|e| e.into_inner()).capture()?;
            let header = snapshot::Header {
                build: machine.shelf.build().clone(),
                name: format!("film-{tick}"),
                tick,
                presents: machine.presents,
                notes: vec![],
            };
            let bytes = crate::snapshot_glue::save_parts_observing(
                &mut machine.parts,
                header,
                &*machine.hash,
                anchor.as_ref().map(RamAnchor::base).as_ref(),
                |part, saved| {
                    if part == "d3d" {
                        d3d_barrier = Some(saved);
                    }
                },
            )
            .map_err(anyhow::Error::msg)?;
            ready(machine)?;
            if anchor.is_none() && machine.rt.is_some() {
                // Derive the immutable base from the bytes that actually
                // reached the file, including D3D capture's RAM writebacks.
                let saved = snapshot::Snapshot::open(&bytes, machine.shelf.build())?;
                new_anchor = Some(RamAnchor {
                    tick,
                    name: saved.header.name.clone(),
                    bytes: std::sync::Arc::new(snapshot::guest::standalone_ram(&saved)?),
                    deltas: 0,
                });
            }
            Ok((bytes, disk_delta))
        });

        let mut s = self.lock();
        let Mode::Record(rec) = &mut s.mode else {
            return;
        };
        if let Some(saved) = d3d_barrier {
            let Some(out) = &mut rec.out else { return };
            let mark = film::tape::Mark::D3dBarrier {
                slices,
                tick,
                saved,
            };
            if let Err(error) = out.chunk(film::MARK, &mark) {
                s.fail(format!("writing D3D capture boundary: {error:#}"));
                return;
            }
        }
        match capture {
            Ok((snapshot, disk_delta)) => {
                let Some(out) = &mut rec.out else { return };
                let frame = Keyframe {
                    tick,
                    slices,
                    calls,
                    lobby_boundary,
                    snapshot,
                    disk_delta,
                    ram_anchor: None,
                };
                // Make the physical cursor in the outer file a call boundary.
                if let Err(error) = rec.batch.flush(out) {
                    s.fail(format!("flushing film calls: {error:#}"));
                    return;
                }
                rec.frame.next_attempt = tick.saturating_add(rec.frame.interval);
                match rec
                    .track
                    .keyframe_against(out, &frame, anchor.as_ref().map(|a| a.tick))
                {
                    Ok(()) => {
                        if let Some(full) = new_anchor {
                            rec.frame.ram_anchor = Some(full);
                        } else if let Some(existing) = &mut rec.frame.ram_anchor {
                            existing.deltas += 1;
                        }
                        rec.frame.last_reason = None;
                    }
                    Err(error) if error.is::<film::ChunkTooLarge>() => {
                        eprintln!("theater: skipped keyframe at tick {tick}: {error}");
                    }
                    Err(error) => s.fail(format!("writing keyframe: {error:#}")),
                }
            }
            Err(error) => {
                let why = error.to_string();
                // Quiescence may change on the next tick. After one second
                // of attempts, defer until the next interval.
                let interval = rec.frame.interval.max(1);
                if tick % interval >= interval / KEYFRAME_SECONDS {
                    rec.frame.next_attempt = tick.saturating_add(interval - tick % interval);
                }
                if rec.frame.last_reason.is_none() {
                    eprintln!("theater: keyframe at tick {tick} deferred: {why}");
                    rec.frame.last_reason = Some(why);
                }
            }
        }
    }
}
