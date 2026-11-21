//! Scheduler boundary between the recorded device save and the next bot/input
//! poll. Restores rewind both that boundary and the film's logical clocks.
use super::replay_cursor::ReplayCursor;
use super::warm::Warm;
use super::*;
use crate::agent_glue::AgentGlue;
use crate::snapshot_glue::BootMachine;
use std::path::Path;
use std::time::Instant;

#[derive(Clone)]
pub(super) struct Boundary {
    cursor: ReplayCursor,
    slices: u64,
    lobby: u64,
    audio_idle: u64,
}

#[derive(Default)]
pub(super) struct RingStatus {
    pub(super) ticks: Vec<u64>,
    pub(super) bytes: usize,
    pub(super) cap: usize,
}

impl RingStatus {
    pub(super) fn from_warm(warm: Option<&Warm<Boundary>>) -> Self {
        warm.map_or_else(Self::default, |warm| Self {
            ticks: warm.ticks(),
            bytes: warm.bytes(),
            cap: warm.cap(),
        })
    }
}

pub enum SliceAction {
    Run,
    Stop,
    Seek(u64),
}

pub(super) fn open(
    mode: &mut Mode,
    root: &Path,
    presentation: &Presentation,
    version: u32,
) -> Result<Option<Warm<Boundary>>> {
    let Mode::Replay(tape, _) = mode else {
        return Ok(None);
    };
    let Some(cap) = crate::setting_defs::theater_ring_cap(None) else {
        return Ok(None);
    };
    if presentation.warm_disabled || version < 2 {
        return Ok(None);
    }
    let entry = tape.reader.machine_section().unique(film::DATA)?.clone();
    let manifest: film::manifest::Manifest = tape.reader.read(&entry)?;
    Ok(Some(Warm::new(cap, manifest.disk_tracker(root))))
}

impl State {
    pub(super) fn can_redraw_from_ring(&self, tick: u64) -> bool {
        !self.presentation.warm_disabled
            && self.ring_status.ticks.first().is_some_and(|t| *t <= tick)
    }

    pub(super) fn finish_warm_tick(&mut self, tick: u64) {
        if self.warm_check.is_some_and(|(target, _)| tick == target) {
            self.warm_check = None;
        }
    }

    pub(super) fn retry_cold(&mut self, target: u64, paused: bool) {
        self.presentation.warm_disabled = true;
        self.warm_check = None;
        self.ring_status = RingStatus::default();
        self.restart = Some(target);
        self.seeking = None;
        self.paused = paused;
        self.failure = None;
    }
}

impl Session {
    fn restore_boundary(
        &self,
        boundary: Boundary,
        agent: Option<&AgentGlue>,
    ) -> Result<(u64, u64)> {
        if let Some(agent) = agent {
            agent.agent.restore_film_lobby_boundary(boundary.lobby);
        } else {
            ensure!(
                boundary.lobby == 0,
                "ring entry requires the lobby executor"
            );
        }
        self.restore_replay_cursor(&boundary.cursor)?;
        Ok((boundary.slices, boundary.audio_idle))
    }

    pub fn capture_ring(
        &self,
        tick: u64,
        slices: u64,
        audio_idle: u64,
        barrier: snapshot::SnapshotWriter,
        machine: &mut BootMachine<'_>,
        agent: Option<&AgentGlue>,
    ) {
        let verify = self.verify_ring_at(tick);
        let mut warm = self.warm.lock().unwrap_or_else(|e| e.into_inner());
        let result = self.with_keyframe(|| -> Result<()> {
            let warm = warm.as_mut().context("ring capture is disabled")?;
            let cursor = self.capture_replay_cursor()?;
            let cost = cursor.bytes() + 3 * size_of::<u64>();
            let boundary = Boundary {
                cursor,
                slices,
                audio_idle,
                lobby: agent.map_or(0, |a| a.agent.film_lobby_boundary()),
            };
            let captured = warm.capture(machine, tick, boundary, cost, barrier)?;
            if verify {
                ensure!(captured, "ring capture refused at tick {tick}");
                self.verify_captured_hash(tick, || (machine.hash)())?;
                let frame = {
                    let mut state = self.lock();
                    let engine = state.engine.as_mut().context("missing film track")?;
                    if engine.native_keys {
                        engine.track.read_keyframe_at(tick)?
                    } else {
                        None
                    }
                };
                if let Some(frame) = frame {
                    ensure!(
                        warm.matches_recorded_ram(tick, &frame, machine.shelf.build())?,
                        "ring capture disagrees with recorded RAM at tick {tick}"
                    );
                    self.verified_ring_restore(tick, Ok(()));
                }
                let point = warm
                    .restore(machine, tick)?
                    .context("ring restore refused")?;
                self.restore_boundary(point, agent)?;
            }
            Ok(())
        });
        self.lock().ring_status = RingStatus::from_warm(warm.as_ref());
        if verify {
            self.verified_ring_restore(tick, result);
        } else if let Err(error) = result {
            eprintln!("theater: skipping ring capture at tick {tick}: {error:#}");
        }
    }

    /// None hands the target to the parent. Any partially loaded machine is
    /// discarded without another guest instruction.
    pub fn try_warm_seek(
        &self,
        target: u64,
        machine: &mut BootMachine<'_>,
        agent: Option<&AgentGlue>,
    ) -> Option<(u64, u64)> {
        let mut slot = self.warm.lock().unwrap_or_else(|e| e.into_inner());
        let warm = slot.as_mut()?;
        let (from_tick, paused) = {
            let state = self.lock();
            if state.presentation.warm_disabled || state.ring_status.cap == 0 {
                return None;
            }
            (
                state.tick,
                state.warm_check.map_or(state.paused, |(_, p)| p),
            )
        };
        let start = Instant::now();
        let ring_tick = warm.closest(target);
        let file = self.lock().engine.as_ref()?.file_candidate(target);
        let use_ring = ring_tick.is_some() && ring_tick >= file.map(|key| key.tick);
        let source = match (use_ring, file.map(|key| key.source)) {
            (true, _) => agent_proto::theater::SeekSource::Ring,
            (_, Some(film::blam::KeyframeSource::Embedded)) => {
                agent_proto::theater::SeekSource::Keyframe
            }
            (_, Some(film::blam::KeyframeSource::Sidecar)) => {
                agent_proto::theater::SeekSource::Sidecar
            }
            (_, None) => return None,
        };
        let loaded = self.with_keyframe(|| -> Result<Option<(u64, u64)>> {
            if let Some(tick) = ring_tick.filter(|_| use_ring) {
                return warm
                    .restore(machine, tick)?
                    .map(|point| self.restore_boundary(point, agent))
                    .transpose();
            }
            let frame = self
                .lock()
                .engine
                .as_mut()
                .context("missing film track")?
                .track
                .seek(target)?
                .context("missing selected keyframe")?;
            if !warm.disk_matches(machine, &frame.disk_delta)? {
                return Ok(None);
            }
            warm.clear(machine);
            Ok(Some((self.restore_keyframe(frame, machine, agent)?, 0)))
        });
        let mut state = self.lock();
        state.ring_status = RingStatus::from_warm(Some(warm));
        match loaded {
            Ok(Some(point)) => {
                let tick = state.tick;
                if let Mode::Replay(_, replay) = &mut state.mode {
                    replay.seek_tick = target;
                    replay.paused_at_target = paused;
                }
                state.seek = Some(SeekProgress {
                    from_tick,
                    target,
                    source,
                    warm: true,
                    restore_ms: Some(start.elapsed().as_millis() as u64),
                });
                state.restart = None;
                state.failure = None;
                state.complete = false;
                state.seeking = (tick < target).then_some(target);
                state.paused = tick == target && paused;
                state.warm_check = (tick < target).then_some((target, paused));
                Some(point)
            }
            Ok(None) => None,
            Err(error) => {
                eprintln!("theater: cold retry at tick {target}: {error:#}");
                state.retry_cold(target, paused);
                None
            }
        }
    }

    /// A failed independent ring run invalidates this viewer's acceleration
    /// too. Discard any possibly perturbed machine through the ordinary handoff.
    pub(super) fn reject_verified_ring(&self) {
        // Serialize this connection-thread reply with any scheduler-thread
        // load/capture; they publish their transport state before unlocking.
        let warm = self.warm.lock().unwrap_or_else(|e| e.into_inner());
        if warm.is_some() {
            let mut state = self.lock();
            let (target, paused) = state.warm_check.unwrap_or_else(|| {
                (state.restart.unwrap_or(state.visible_tick()), state.visible_pause())
            });
            state.retry_cold(target, paused);
        }
    }

    pub fn write_cold_restart(&self, target: u64) -> Result<()> {
        let mut state = self.lock();
        state.cancel_camera_redraw();
        let Mode::Replay(_, replay) = &state.mode else {
            bail!("cold restart requires a player")
        };
        let mut view = state.presentation.clone();
        if state.header.version >= 2 {
            replay.camera.save(&mut view);
        }
        let paused = state.warm_check.map_or(state.paused, |(_, p)| p);
        std::fs::write(
            &replay.restart,
            serde_json::to_vec(&(target, state.speed, paused, view))?,
        )?;
        Ok(())
    }

    pub fn barrier_failed(&self, error: anyhow::Error) -> Result<()> {
        let tick = self.lock().tick;
        if self.verifying() {
            self.verified_ring_restore(tick, Err(error));
            Ok(())
        } else {
            let mut state = self.lock();
            if let Some((target, paused)) = state.warm_check {
                state.retry_cold(target, paused);
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}
