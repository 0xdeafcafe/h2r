//! A verification child owns its disk and machine. It replays from boot so
//! even a range beginning between keyframes starts at an authenticated cursor.
use super::{Mode, Replay, Session, State};
use agent_proto::theater::{Divergence, VerificationReport, Verify};
use agent_proto::{RpcError, code};
use anyhow::{Context, Result, ensure};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum VerificationMode {
    /// Installed by the CLI player with its original disc, image and film.
    Player(Arc<dyn Fn(Verify) -> Result<VerificationReport> + Send + Sync>),
    /// Private result destination owned by the launching player.
    Worker {
        request: Verify,
        report: PathBuf,
        /// Private sequence of (slice, tick, whole-RAM digest) triples shared
        /// only by the two passes of a ring verification request.
        baseline: Option<PathBuf>,
    },
}

pub(super) struct Verification {
    range: [u64; 2],
    ring: bool,
    report: PathBuf,
    counts: VerificationReport,
    baseline: Option<Baseline>,
}

// Fixed-size, streamed local records. The reference pass performs the
// recorded D3D work and hashes RAM at exactly the same point as the reader.
enum Baseline {
    Write(File),
    Read {
        file: File,
        left: u64,
        expected: Option<(u64, u64)>,
    },
}

impl Baseline {
    fn read(file: File) -> Result<Self> {
        let bytes = file.metadata()?.len();
        ensure!(bytes % 24 == 0, "incomplete verification reference");
        Ok(Self::Read {
            file,
            left: bytes / 24,
            expected: None,
        })
    }

    fn barrier(&mut self, slice: u64, tick: u64, hash: u64) -> Result<bool> {
        match self {
            Self::Write(file) => {
                for word in [slice, tick, hash] {
                    file.write_all(&word.to_le_bytes())?;
                }
                Ok(false)
            }
            Self::Read {
                file,
                left,
                expected,
            } => {
                ensure!(expected.is_none(), "previous ring capture was skipped");
                ensure!(
                    *left > 0,
                    "extra ring capture at slice {slice}, tick {tick}"
                );
                let mut words = [0; 3];
                for word in &mut words {
                    let mut bytes = [0; 8];
                    file.read_exact(&mut bytes)?;
                    *word = u64::from_le_bytes(bytes);
                }
                *left -= 1;
                let [ref_slice, ref_tick, ref_hash] = words;
                ensure!(
                    (slice, tick) == (ref_slice, ref_tick),
                    "capture point differs from linear replay at slice {ref_slice}, tick {ref_tick}"
                );
                ensure!(
                    hash == ref_hash,
                    "RAM before capture differs from linear replay at tick {tick}"
                );
                *expected = Some((tick, ref_hash));
                Ok(true)
            }
        }
    }

    fn captured(&mut self, tick: u64, hash: u64) -> Result<()> {
        let Self::Read { expected, .. } = self else {
            anyhow::bail!("ring pass needs a reference")
        };
        ensure!(
            expected.take() == Some((tick, hash)),
            "RAM after capture differs from linear replay at tick {tick}"
        );
        Ok(())
    }

    fn complete(&self) -> bool {
        match self {
            Self::Write(_) => true,
            Self::Read { left, expected, .. } => *left == 0 && expected.is_none(),
        }
    }
}

impl Verification {
    pub(super) fn open(mode: &Mode, length: u64) -> Result<Option<Self>> {
        let Mode::Replay(
            _,
            Replay {
                verification:
                    Some(VerificationMode::Worker {
                        request,
                        report,
                        baseline,
                    }),
                ..
            },
        ) = mode
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            range: request.bounds(length).map_err(anyhow::Error::msg)?,
            ring: request.ring,
            report: report.clone(),
            counts: Default::default(),
            baseline: baseline
                .as_ref()
                .map(|path| {
                    if request.ring {
                        Baseline::read(File::open(path)?)
                    } else {
                        Ok(Baseline::Write(File::create_new(path)?))
                    }
                })
                .transpose()?,
        }))
    }

    pub(super) fn contains(&self, tick: u64) -> bool {
        (self.range[0]..=self.range[1]).contains(&tick)
    }

    pub(super) fn tick(&mut self, tick: u64) {
        self.counts.ticks += u64::from(self.contains(tick));
    }

    pub(super) fn digest(&mut self, tick: u64) {
        self.counts.digests_checked += u64::from(self.contains(tick));
    }

    pub(super) fn arena(&mut self, tick: u64) {
        self.counts.arena_hashes_checked += u64::from(self.contains(tick));
    }

    pub(super) fn failed(&mut self, tick: u64, kind: &str) {
        self.counts
            .first_divergence
            .get_or_insert_with(|| Divergence {
                tick,
                kind: kind.to_owned(),
            });
    }

    fn save(&self) -> Result<()> {
        std::fs::write(&self.report, serde_json::to_vec(&self.counts)?)
            .context("writing Theater verification result")
    }
}

impl State {
    /// A full-film verification includes the end-of-tape checks after the
    /// final gameplay tick. An interior range stops at the next boundary.
    pub(super) fn verify_range_ended(&self) -> bool {
        self.verify
            .as_ref()
            .is_some_and(|v| v.range[1] < self.header.meta.length_ticks && self.tick >= v.range[1])
    }

    pub(super) fn verify_end_of_reference(&mut self) {
        if let Some(v) = &mut self.verify
            && v.baseline.as_ref().is_some_and(|b| !b.complete())
        {
            self.fail("missing capture point from linear replay".into());
        }
    }

    pub(super) fn save_verify_result(&self) -> Result<()> {
        if let Some(v) = &self.verify {
            v.save()?;
        }
        Ok(())
    }
}

impl Session {
    pub(super) fn add_verify_method(self: &Arc<Self>, agent: &agent::Agent) {
        let session = self.clone();
        let running = Mutex::new(());
        agent.add_connection_read(agent_proto::theater::VERIFY, move |params| {
            let request: Verify = super::controls::params(params)?;
            let launch = {
                let s = session.lock();
                request
                    .bounds(s.header.meta.length_ticks)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
                let Mode::Replay(
                    _,
                    Replay {
                        verification: Some(VerificationMode::Player(launch)),
                        ..
                    },
                ) = &s.mode
                else {
                    return Err(RpcError::new(
                        code::UNAVAILABLE,
                        "no verification player launcher",
                    ));
                };
                launch.clone()
            };
            let _job = running
                .try_lock()
                .map_err(|_| RpcError::new(code::BUSY, "a verification player is running"))?;
            let report = launch(request)
                .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("{e:#}")))?;
            if request.ring && report.first_divergence.is_some() {
                session.reject_verified_ring();
            }
            Ok(serde_json::json!(report))
        });
    }

    pub fn verifying(&self) -> bool {
        self.lock().verify.is_some()
    }

    pub fn verify_capture_hash(
        &self,
        slice: u64,
        tick: u64,
        hash: impl FnOnce() -> lockstep::RamHash,
    ) -> Result<()> {
        let mut s = self.lock();
        if let Some(v) = &mut s.verify
            && v.contains(tick)
            && let Some(reference) = &mut v.baseline
            && reference.barrier(slice, tick, hash().total)?
        {
            v.counts.ram_hashes_checked += 1;
        }
        Ok(())
    }

    pub fn verify_captured_hash(
        &self,
        tick: u64,
        hash: impl FnOnce() -> lockstep::RamHash,
    ) -> Result<()> {
        let mut s = self.lock();
        if let Some(v) = &mut s.verify
            && v.contains(tick)
            && let Some(reference) = &mut v.baseline
        {
            reference.captured(tick, hash().total)?;
            v.counts.ram_hashes_checked += 1;
        }
        Ok(())
    }

    pub fn verify_ring_at(&self, tick: u64) -> bool {
        self.lock()
            .verify
            .as_ref()
            .is_some_and(|v| v.ring && v.contains(tick))
    }

    pub fn verified_ring_restore(&self, tick: u64, result: Result<()>) {
        let mut s = self.lock();
        let Some(v) = &mut s.verify else { return };
        match result {
            Ok(()) if v.contains(tick) => v.counts.ram_hashes_checked += 1,
            Ok(()) => {}
            Err(e) => s.fail_at(tick, format!("verifying ring restore: {e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_reference_detects_a_changed_capture_and_missing_or_extra_boundaries() {
        let path =
            std::env::temp_dir().join(format!("ring-reference-{}", live_proto::sign::random_id()));
        let mut reference = Baseline::Write(File::create_new(&path).unwrap());
        assert!(!reference.barrier(51, 30, 7).unwrap());
        assert!(!reference.barrier(73, 60, 9).unwrap());
        drop(reference);
        let open = || Baseline::read(File::open(&path).unwrap()).unwrap();
        let mut good = open();
        assert!(good.barrier(51, 30, 7).unwrap());
        good.captured(30, 7).unwrap();
        assert!(!good.complete());
        assert!(good.barrier(73, 60, 9).unwrap());
        good.captured(60, 9).unwrap();
        assert!(good.complete());
        assert!(good.barrier(90, 71, 2).is_err());
        assert!(open().barrier(51, 30, 8).is_err());
        assert!(open().barrier(52, 30, 7).is_err());
        let mut changed = open();
        assert!(changed.barrier(51, 30, 7).unwrap());
        assert!(changed.captured(30, 8).is_err());
        drop((good, changed));
        std::fs::remove_file(path).unwrap();
    }
}
