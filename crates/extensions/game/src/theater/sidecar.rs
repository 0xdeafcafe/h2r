//! The mounted snapshot machine supplies the build key. Only the viewer's
//! library owns the cache; the replay's private guest disk supplies its deltas.
use super::*;
use film::manifest::Manifest;
use film::sidecar::SidecarWriter;
use snapshot::BuildKey;
use std::path::Path;

#[derive(Clone)]
pub struct ReplayCache {
    pub films_dir: PathBuf,
    pub cap_bytes: u64,
    /// Digest of the delta already applied to this child's private disk.
    pub disk_digest: String,
}

/// Ask the launcher for a fresh disk and process before executing guest code.
/// Carry the measured build so the launcher can find the same disk delta.
#[derive(Debug)]
pub struct RetrySeek(pub (String, String));

impl RetrySeek {
    pub(super) fn new(build: &BuildKey) -> Self {
        Self((build.stamp.clone(), build.switches.clone()))
    }
}

impl std::fmt::Display for RetrySeek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("restarting seek with the mounted build's guest disk")
    }
}

impl std::error::Error for RetrySeek {}

impl State {
    pub(super) fn seek_info(&self, replay: &Replay) -> Option<agent_proto::theater::SeekInfo> {
        use agent_proto::theater::{SeekInfo, SeekSource};
        use film::blam::KeyframeSource;
        if replay.seek_tick == 0 {
            return None;
        }
        let source = match self.engine.as_ref().and_then(|e| e.track.keyframe_source()) {
            Some(KeyframeSource::Sidecar) => SeekSource::Sidecar,
            Some(KeyframeSource::Embedded) => SeekSource::Keyframe,
            None => SeekSource::Boot,
        };
        Some(SeekInfo {
            source,
            target: replay.seek_tick,
            from_tick: self.presentation.seek_from,
            warm: false,
            restore_ms: self.engine.as_ref().and_then(|e| e.restore_ms),
            remaining_ticks: replay.seek_tick.saturating_sub(self.tick),
        })
    }
}

impl Session {
    /// Called once, after installing the hook plan and before the first slice.
    pub fn mount_sidecar(
        &self,
        films_dir: &Path,
        cap: u64,
        build: &BuildKey,
        guest_disk_root: &Path,
    ) -> Result<()> {
        let mut state = self.lock();
        let State {
            header,
            mode,
            engine,
            presentation,
            ..
        } = &mut *state;
        let prune = || {
            if let Err(e) = film::sidecar::prune(films_dir, cap) {
                eprintln!("theater: could not enforce the keyframe cache limit: {e:#}");
            }
        };
        match mode {
            Mode::Record(rec) => {
                if header.meta.source != Source::Local {
                    return Ok(());
                }
                prune();
                if cap > 0 {
                    match SidecarWriter::new(
                        films_dir,
                        &header.meta.id,
                        &build.stamp,
                        &build.switches,
                        cap,
                    ) {
                        Ok(writer) => rec.sidecar = Some(writer),
                        Err(e) => eprintln!("theater: local keyframe cache unavailable: {e:#}"),
                    }
                }
            }
            Mode::Replay(tape, replay) => {
                let Some(engine) = engine else { return Ok(()) };
                if cap > 0
                    && let Err(e) =
                        engine
                            .track
                            .attach_sidecar(films_dir, &build.stamp, &build.switches)
                {
                    eprintln!("theater: could not open sidecar: {e:#}");
                }
                prune();
                ensure!(
                    cap > 0 || !replay.unpaced,
                    "Prepare requires a nonzero Theater cache cap"
                );
                if replay.unpaced {
                    engine.track.verify_sidecar()?;
                }
                engine.keyframe = engine.track.seek(replay.seek_tick)?;
                if let Some(cache) = &replay.cache {
                    let delta = engine
                        .keyframe
                        .as_ref()
                        .map_or(&[][..], |k| k.disk_delta.as_slice());
                    if film::xbe_digest(delta) != cache.disk_digest {
                        return Err(RetrySeek::new(build).into());
                    }
                }
                presentation.snapshot_build = Some((build.stamp.clone(), build.switches.clone()));
                if cap > 0 {
                    match SidecarWriter::resume(
                        films_dir,
                        &mut tape.reader,
                        &build.stamp,
                        &build.switches,
                        cap,
                    ) {
                        Ok(writer) => {
                            let data = tape.reader.machine_section().unique(film::DATA)?.clone();
                            let manifest: Manifest = tape.reader.read(&data)?;
                            engine.disk =
                                Some(Arc::new(Mutex::new(manifest.disk_tracker(guest_disk_root))));
                            engine.sidecar = Some(writer);
                        }
                        Err(e) if replay.unpaced => {
                            return Err(e.context("opening Prepare sidecar"));
                        }
                        Err(e) => eprintln!("theater: playback keyframe cache unavailable: {e:#}"),
                    }
                }
            }
        }
        Ok(())
    }
}
