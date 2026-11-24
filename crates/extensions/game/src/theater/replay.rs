//! One cursor for the engine update currently being consumed. HLE calls are
//! ordered independently inside that tick. A machine keyframe restarts both
//! cursors before the scheduler executes the first restored instruction.
use super::*;
use super::record::{CapturePoints, CapturePosition};
use crate::snapshot_glue::{BootMachine, check_hash, load_parts_with_base};
use film::blam::{Keyframe, Tick, TrackReader};

pub(super) struct EngineReplay {
    pub(super) track: TrackReader<File>,
    pub(super) sidecar: Option<film::sidecar::SidecarWriter>,
    pub(super) disk: Option<Arc<Mutex<film::manifest::DiskTracker>>>,
    capture_points: CapturePoints,
    pub(super) map_vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    pub(super) pending: Option<Tick>,
    pub(super) keyframe: Option<Keyframe>,
    pub(super) saw_actions: bool,
    pub(super) last_pause: Option<bool>,
    pub(super) pause_edges: Vec<bool>,
}

impl EngineReplay {
    pub(super) fn open(replay: &Replay, image: film::image::ImageMatch) -> Result<Self> {
        let mut track = TrackReader::open_with_film(&replay.path)?;
        if image == film::image::ImageMatch::FromBoot {
            track.disable_embedded_keyframes();
        }
        Ok(Self {
            track,
            map_vfs: None,
            pending: None,
            keyframe: None,
            sidecar: None,
            disk: None,
            capture_points: CapturePoints::default(),
            saw_actions: false,
            last_pause: Some(false),
            pause_edges: Vec::new(),
        })
    }

    pub(super) fn file_candidate(&self, target: u64) -> Option<film::blam::KeyframeInfo> {
        self.track.keyframes().into_iter().rev().find(|key| key.tick <= target)
    }

    pub(super) fn seek_source(&self, target: u64) -> agent_proto::theater::SeekSource {
        use agent_proto::theater::SeekSource;
        use film::blam::KeyframeSource;
        match self.file_candidate(target).map(|key| key.source) {
            Some(KeyframeSource::Embedded) => SeekSource::Keyframe,
            Some(KeyframeSource::Sidecar) => SeekSource::Sidecar,
            None => SeekSource::Boot,
        }
    }

    /// Peek at the upcoming tick. A menu/cinematic tick can reach the
    /// post-tick observer without passing through the player update.
    pub(super) fn next_tick(&mut self) -> Result<&Tick> {
        if self.pending.is_none() {
            self.pending = self.track.step()?;
        }
        self.pending
            .as_ref()
            .context("past end of the film input stream")
    }

    pub(super) fn observe_pause(&mut self, paused: bool) {
        if self
            .last_pause
            .replace(paused)
            .is_some_and(|previous| previous != paused)
        {
            self.pause_edges.push(paused);
        }
    }

    pub(super) fn note_actions(&mut self) -> Result<()> {
        ensure!(
            !self.saw_actions,
            "two action updates in the same film tick"
        );
        self.saw_actions = true;
        Ok(())
    }

    fn completed(
        &mut self,
        guest: &runtime::Guest,
        tick: u64,
        gameplay: Option<u64>,
    ) -> Result<bool> {
        metadata::verify_start(guest, tick, &self.track.descriptor, self.map_vfs.as_ref())?;
        let applied = self.saw_actions;
        let observed = std::mem::take(&mut self.pause_edges);
        let frame = self.next_tick()?;
        let recorded = frame.events.iter().filter_map(|event| match event {
            film::blam::Event::Pause { paused } => Some(*paused),
            _ => None,
        });
        ensure!(recorded.eq(observed), "campaign pause transitions differ");
        ensure!(
            frame.tick == tick,
            "expected recorded tick {}, got {tick}",
            frame.tick
        );
        ensure!(
            applied || !frame.inputs.iter().any(|input| input.action.is_some()),
            "simulation skipped the recorded player update"
        );
        if let Some(expected) = frame.post_hash {
            ensure!(
                gameplay == Some(expected),
                "expected gameplay {expected:016x}, observed {gameplay:?}"
            );
        }
        metadata::verify_events(guest, &frame.events)?;
        let checked_digest = frame.post_hash.is_some();
        self.pending = None;
        self.saw_actions = false;
        // The next tick's Firefight entries, before its pre-tick applies
        // them (`firefight::film`). Past the film's end there is none; a
        // block that cannot be read fails at the next tick's own read.
        if let Ok(next) = self.next_tick() {
            crate::firefight::film::deliver(&next.firefight);
        }
        Ok(checked_digest)
    }
}

impl Session {
    pub(super) fn check_replay_tick(s: &mut State, guest: &runtime::Guest, post_hash: Option<u64>) {
        let tick = s.tick;
        if let Some(engine) = &mut s.engine {
            match engine.completed(guest, tick, post_hash) {
                Ok(true) => { if let Some(v) = &mut s.verify { v.digest(tick); } }
                Ok(false) => {}
                Err(e) => s.fail(e.to_string()),
            }
        }
    }

    pub fn replay_capture_point<'a>(
        &self,
        slices: u64,
        lobby_boundary: impl FnOnce() -> u64,
        machine: impl FnOnce() -> BootMachine<'a>,
    ) -> Result<Option<(u64, snapshot::SnapshotWriter)>> {
        let (tick, expected) = {
            let mut state = self.lock();
            if state.restart.is_some() {
                return Ok(None);
            }
            let tick = state.tick;
            let Mode::Replay(tape, _) = &mut state.mode else {
                return Ok(None);
            };
            let Some(expected) = tape.d3d_barrier(slices, tick)? else {
                return Ok(None);
            };
            (tick, expected)
        };
        let mut machine = machine();
        self.with_keyframe(|| {
            let mut out = machine.film_snapshot_writer(tick);
            let saved = machine
                .capture_film_device(&mut out)
                .context("film D3D capture requires a device")?;
            ensure!(
                saved.is_ok() == expected,
                "first divergence at tick {tick}: D3D capture at slice {slices} expected success={expected}, observed {saved:?}"
            );
            if expected {
                self.verify_capture_hash(slices, tick, || (machine.hash)())?;
            }
            let ring_enabled = self.lock().ring_status.cap > 0;
            let ring = expected && (ring_enabled || self.verify_ring_at(tick));
            let sidecar = (|| {
                let s = self.lock();
                let Mode::Replay(tape, _) = &s.mode else { return None };
                let point = tape.sidecar_capture_point()?;
                let engine = s.engine.as_ref()?;
                let capture = engine.capture_points.snapshot(
                    point.ordinal,
                    tick,
                    engine.track.descriptor.match_start_tick,
                    engine.sidecar.as_ref(),
                    engine.disk.as_ref(),
                )?;
                Some((capture, point.offset, s.calls))
            })();
            let Some((capture, mark_offset, calls)) = sidecar else {
                return Ok(ring.then_some((tick, out)));
            };
            let ring_snapshot = ring.then(|| (tick, out.clone()));
            let capture = capture.save(
                self,
                &mut machine,
                out,
                CapturePosition {
                    tick,
                    slices,
                    calls,
                    lobby_boundary: lobby_boundary(),
                },
            );
            let mut s = self.lock();
            if s.failure.is_none() && let Some(engine) = &mut s.engine {
                engine.capture_points.publish(
                    tick,
                    engine.sidecar.as_mut(),
                    mark_offset,
                    capture,
                );
            }
            Ok(ring_snapshot)
        })
    }

    pub fn take_keyframe(&self) -> Option<Keyframe> {
        self.lock().engine.as_mut()?.keyframe.take()
    }

    /// Caller discards this process if any part of the load fails.
    pub fn restore_keyframe(
        &self,
        frame: Keyframe,
        machine: &mut BootMachine<'_>,
        agent: Option<&crate::agent_glue::AgentGlue>,
    ) -> Result<u64> {
        let result = self.load_keyframe(frame, machine, agent);
        if let Err(error) = &result
            && let Some(engine) = &mut self.lock().engine
            && engine.track.sidecar_mark_offset().is_some()
        {
            engine.sidecar.take();
            engine.track.invalidate_sidecar()?;
            eprintln!("theater: discarding sidecar after failed restore: {error:#}");
            return Err(RetrySeek::new(machine.shelf.build()).into());
        }
        result
    }

    fn load_keyframe(
        &self,
        frame: Keyframe,
        machine: &mut BootMachine<'_>,
        agent: Option<&crate::agent_glue::AgentGlue>,
    ) -> Result<u64> {
        let started = std::time::Instant::now();
        let snap = snapshot::Snapshot::open(&frame.snapshot, machine.shelf.build())?;
        ensure!(
            snap.header.tick == frame.tick,
            "machine keyframe tick disagrees with the input track"
        );
        // The feature and XAPI loaders can call back into the film tap. Do
        // not hold the film lock while restoring them.
        self.with_keyframe(|| -> Result<()> {
            let blockers = machine.film_load_blockers();
            ensure!(
                blockers.is_empty(),
                "cannot restore film keyframe: {}",
                blockers.join(", ")
            );
            let full_ram = frame
                .ram_anchor
                .as_ref()
                .map(|(tick, bytes)| -> Result<_> {
                    let anchor = snapshot::Snapshot::open(bytes, machine.shelf.build())?;
                    let name = format!("film-{tick}");
                    ensure!(
                        anchor.header.tick == *tick && anchor.header.name == name,
                        "machine RAM anchor disagrees with its index"
                    );
                    Ok((name, snapshot::guest::standalone_ram(&anchor)?))
                })
                .transpose()?;
            let base = full_ram.as_ref().map(|(name, bytes)| snapshot::Base {
                name,
                ram: bytes,
                dirty: None,
            });
            load_parts_with_base(&mut machine.parts, &snap, None, base.as_ref())
                .map_err(anyhow::Error::msg)?;
            let ram = check_hash(&snap, &(machine.hash)());
            ensure!(
                ram.checked && ram.equal,
                "film keyframe RAM mismatch at tick {}",
                frame.tick
            );
            Ok(())
        })?;
        if let Some(agent) = agent {
            agent
                .agent
                .restore_film_lobby_boundary(frame.lobby_boundary);
        } else {
            ensure!(
                frame.lobby_boundary == 0,
                "keyframe requires the lobby executor"
            );
        }
        let mut s = self.lock();
        let engine = s.engine.as_mut().context("missing film input track")?;
        // The first slice samples the saved latch itself. Pending pause
        // edges never straddle a keyframe.
        engine.pending = None;
        engine.saw_actions = false;
        engine.pause_edges.clear();
        engine.last_pause = None;
        if engine
            .track
            .descriptor
            .match_start_tick
            .is_some_and(|start| start <= frame.tick)
        {
            metadata::verify_restored_map(&engine.track.descriptor, engine.map_vfs.as_ref())?;
        }
        let embedded = engine.track.keyframe_offset();
        let sidecar = engine.track.sidecar_cursor();
        let Mode::Replay(tape, replay) = &mut s.mode else {
            bail!("keyframe restore requires playback")
        };
        ensure!(
            tape.end.is_some_and(|(end, _, _)| frame.slices <= end),
            "keyframe slice is past the end of the film"
        );
        if let Some(cursor) = sidecar {
            tape.resume_sidecar_at(cursor, &frame)?;
        } else {
            let offset = embedded.context("selected keyframe is absent from the input track")?;
            tape.resume_keyframe(offset, frame.tick, frame.calls, frame.lobby_boundary)?;
        }
        let reached_target = replay.seek_tick == frame.tick;
        s.tick = frame.tick;
        s.calls = frame.calls;
        if let Some(seek) = &mut s.seek {
            seek.restore_ms = Some(started.elapsed().as_millis() as u64);
        }
        if reached_target {
            s.seeking = None;
        }
        Ok(frame.slices)
    }
}
