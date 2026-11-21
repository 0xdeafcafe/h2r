//! One cursor for the engine update currently being consumed. HLE calls are
//! ordered independently inside that tick. A machine keyframe restarts both
//! cursors before the scheduler executes the first restored instruction.
use super::*;
use crate::snapshot_glue::{BootMachine, check_hash, load_parts_with_base};
use film::blam::{Keyframe, Tick, TrackReader};

pub(super) struct EngineReplay {
    pub(super) track: TrackReader<File>,
    pub(super) native_keys: bool,
    pub(super) map_vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    pub(super) pending: Option<Tick>,
    keyframe: Option<Keyframe>,
    pub(super) saw_actions: bool,
    pub(super) last_pause: Option<bool>,
    pub(super) pause_edges: Vec<bool>,
}

impl EngineReplay {
    pub(super) fn open(replay: &Replay, image: film::image::ImageMatch) -> Result<Self> {
        let mut track = TrackReader::open(&replay.path)?;
        let keyframe = match image {
            film::image::ImageMatch::Native => track.seek(replay.seek_tick)?,
            film::image::ImageMatch::FromBoot => None,
        };
        Ok(Self {
            track,
            native_keys: image == film::image::ImageMatch::Native,
            map_vfs: None,
            pending: None,
            keyframe,
            saw_actions: false,
            last_pause: Some(false),
            pause_edges: Vec::new(),
        })
    }

    pub(super) fn file_candidate(&self, target: u64) -> Option<film::blam::KeyframeInfo> {
        use film::blam::KeyframeSource;
        self.track.keyframes().into_iter().rev().find(|key| {
            key.tick <= target && (self.native_keys || key.source == KeyframeSource::Sidecar)
        })
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

    pub fn replay_device_barrier(
        &self,
        slices: u64,
        host: &crate::snapshot_glue::Host<'_>,
        shelf: &crate::snapshot_glue::Shelf,
    ) -> Result<Option<(u64, snapshot::SnapshotWriter)>> {
        let (tick, expected) = {
            let mut state = self.lock();
            let tick = state.tick;
            let Mode::Replay(tape, _) = &mut state.mode else {
                return Ok(None);
            };
            let Some(expected) = tape.d3d_barrier(slices, tick)? else {
                return Ok(None);
            };
            (tick, expected)
        };
        let device = host.d3d.context("film D3D capture requires a device")?;
        // The recorder observed this one device save, including its RAM writebacks.
        // Pass the same section on to the ring when this is a capture point.
        let mut scratch = snapshot::SnapshotWriter::new(snapshot::Header {
            build: shelf.build().clone(),
            name: String::new(),
            tick,
            presents: 0,
            notes: vec![],
        });
        let saved = device.save_film(host.rt, &mut scratch);
        ensure!(
            saved.is_ok() == expected,
            "first divergence at tick {tick}: D3D capture at slice {slices} expected success={expected}, observed {saved:?}"
        );
        if expected {
            self.verify_capture_hash(slices, tick, || lockstep::hash_guest(&host.rt.memory()))?;
        }
        let enabled = self.lock().ring_status.cap > 0;
        Ok((expected && (enabled || self.verify_ring_at(tick))).then_some((tick, scratch)))
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
        let offset = engine
            .track
            .keyframe_offset()
            .context("selected keyframe is absent from the input track")?;
        let Mode::Replay(tape, replay) = &mut s.mode else {
            bail!("keyframe restore requires playback")
        };
        ensure!(
            tape.end.is_some_and(|(end, _, _)| frame.slices <= end),
            "keyframe slice is past the end of the film"
        );
        tape.resume_keyframe(offset, frame.tick, frame.calls, frame.lobby_boundary)?;
        let reached_target = replay.seek_tick == frame.tick;
        let pause = replay.paused_at_target;
        s.tick = frame.tick;
        s.calls = frame.calls;
        if let Some(seek) = &mut s.seek {
            seek.restore_ms = Some(started.elapsed().as_millis() as u64);
        }
        if reached_target {
            s.seeking = None;
            s.paused = pause;
        }
        Ok(frame.slices)
    }
}
