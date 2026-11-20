//! One cursor for the engine update currently being consumed. HLE calls are
//! ordered independently inside that tick. A machine keyframe restarts both
//! cursors before the scheduler executes the first restored instruction.
use super::*;
use crate::snapshot_glue::{BootMachine, check_hash, load_parts_with_base};
use film::blam::{Keyframe, Tick, TrackReader};

pub(super) struct EngineReplay {
    pub(super) track: TrackReader<File>,
    pub(super) map_vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    pending: Option<Tick>,
    keyframe: Option<Keyframe>,
    saw_actions: bool,
    last_pause: Option<bool>,
    pause_edges: Vec<bool>,
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
            map_vfs: None,
            pending: None,
            keyframe,
            saw_actions: false,
            last_pause: Some(false),
            pause_edges: Vec::new(),
        })
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
    ) -> Result<()> {
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
        self.pending = None;
        self.saw_actions = false;
        Ok(())
    }
}

impl Session {
    pub(super) fn check_replay_tick(s: &mut State, guest: &runtime::Guest, post_hash: Option<u64>) {
        let tick = s.tick;
        if let Some(engine) = &mut s.engine
            && let Err(e) = engine.completed(guest, tick, post_hash)
        {
            s.fail(e.to_string());
        }
    }

    pub fn replay_device_barrier(
        &self,
        slices: u64,
        host: &crate::snapshot_glue::Host<'_>,
        shelf: &crate::snapshot_glue::Shelf,
    ) -> Result<()> {
        let (tick, expected) = {
            let mut state = self.lock();
            let tick = state.tick;
            let Mode::Replay(tape, _) = &mut state.mode else {
                return Ok(());
            };
            let Some(expected) = tape.d3d_barrier(slices, tick)? else {
                return Ok(());
            };
            (tick, expected)
        };
        let device = host.d3d.context("film D3D capture requires a device")?;
        // Capture only the D3D section and discard it. Its device operations
        // (including the ones performed by an interrupted save) still take
        // place, just as they did in the recording. Do not copy guest RAM.
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
        Ok(())
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
        if reached_target {
            s.seeking = None;
            s.paused = pause;
        }
        Ok(frame.slices)
    }
}
