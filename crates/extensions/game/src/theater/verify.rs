//! A verification child owns its private data directory and machine. It
//! plays the film from its game start, so a range starts at an
//! authenticated tick.
use super::{Mode, Replay, Session, State};
use agent_proto::theater::{Divergence, VerificationReport, Verify};
use agent_proto::{RpcError, code};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum VerificationMode {
    /// Installed by the CLI player with its original disc, image and film.
    Player(Arc<dyn Fn(Verify) -> Result<VerificationReport> + Send + Sync>),
    /// Private result destination owned by the launching player.
    Worker { request: Verify, report: PathBuf },
}

pub(super) struct Verification {
    range: [u64; 2],
    report: PathBuf,
    counts: VerificationReport,
}

impl Verification {
    pub(super) fn open(mode: &Mode, length: u64) -> Result<Option<Self>> {
        let Mode::Replay(
            Replay {
                verification: Some(VerificationMode::Worker { request, report }),
                ..
            },
        ) = mode
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            range: request.bounds(length).map_err(anyhow::Error::msg)?,
            report: report.clone(),
            counts: Default::default(),
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
            Ok(serde_json::json!(report))
        });
    }

    pub fn verifying(&self) -> bool {
        self.lock().verify.is_some()
    }
}
