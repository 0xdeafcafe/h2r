//! `H2_SNAPSHOT_PROBE`: how often could this run have been snapshotted,
//! and what stopped it (docs/runtime/snapshots.md, "Quiescence probe").
//! Read-only; saves nothing.
//!
//! A `snapshot_probe` feature counts game ticks and, by default, asks the
//! runtime to end the slice right after each one (`request_preempt`), as
//! `state.save` would. Between slices, at most once per tick, the boot
//! loop asks [`kernel::Machine::quiescent`], every feature's snapshot
//! policy and the host subsystems, and feeds the answers to a
//! [`kernel::quiesce::Tally`]. Every line it prints starts with
//! `snapshot-probe:`.
//!
//! - `H2_SNAPSHOT_PROBE=1`: check at the end of the slice the tick ended.
//! - `H2_SNAPSHOT_PROBE=slice`: don't end the slice early; check at the
//!   first slice boundary after a tick, wherever the schedule puts it.
//!
//! Ending slices early changes the schedule, so a probed run is not
//! comparable with an unprobed one under lockstep.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use kernel::quiesce::{Blocker, PROBE_PREFIX, Tally};

pub const ENV: &str = "H2_SNAPSHOT_PROBE";

/// What `H2_SNAPSHOT_PROBE` asks for; `None` is off.
pub fn mode_from(value: Option<&str>) -> Result<Option<bool>, String> {
    match value.map(str::trim) {
        None | Some("" | "0" | "off") => Ok(None),
        Some("1" | "on" | "tick") => Ok(Some(true)),
        Some("slice") => Ok(Some(false)),
        Some(other) => Err(format!("{ENV}={other}: want 1, tick, slice or 0")),
    }
}

struct TickCounter {
    ticks: Arc<AtomicU64>,
    preempt: bool,
}

impl feature::Feature for TickCounter {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("snapshot_probe", "Snapshot quiescence probe")
            .doc("docs/runtime/snapshots.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let ticks = Arc::clone(&self.ticks);
        let preempt = self.preempt;
        b.frame().on_tick("count", move |o| {
            ticks.fetch_add(1, Ordering::Relaxed);
            if preempt {
                o.runtime.request_preempt(runtime::preempt::REQUEST);
            }
        });
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}

pub struct Probe {
    ticks: Arc<AtomicU64>,
    checked: u64,
    tally: Tally,
    /// Host subsystems installed outside the feature plan, none audited yet.
    host: Vec<&'static str>,
}

impl Probe {
    /// Install the tick counter when `H2_SNAPSHOT_PROBE` asks for it.
    pub fn from_env(features: &mut feature::Registry) -> anyhow::Result<Option<Self>> {
        let value = std::env::var(ENV).ok();
        let Some(preempt) = mode_from(value.as_deref()).map_err(anyhow::Error::msg)? else {
            return Ok(None);
        };
        let ticks = Arc::new(AtomicU64::new(0));
        let on = features
            .add(&TickCounter {
                ticks: Arc::clone(&ticks),
                preempt,
            })
            .map_err(|e| anyhow::anyhow!("snapshot probe: {e}"))?;
        if !on {
            eprintln!("{PROBE_PREFIX} off: the snapshot_probe feature is switched off");
            return Ok(None);
        }
        eprintln!(
            "{PROBE_PREFIX} on, checking {} each game tick",
            if preempt {
                "at the end of the slice"
            } else {
                "at the first slice boundary after"
            }
        );
        Ok(Some(Self {
            ticks,
            checked: 0,
            tally: Tally::default(),
            host: Vec::new(),
        }))
    }

    /// Host subsystems running outside the feature plan (d3d, dsound,
    /// xapi, ...). Each counts as an unaudited blocker until it reports
    /// its own state.
    pub fn set_host(&mut self, host: Vec<&'static str>) {
        self.host = host;
    }

    /// Between slices: check once if a tick ended since the last check.
    pub fn after_slice(&mut self, m: &kernel::Machine, plan: &feature::Plan) {
        let tick = self.ticks.load(Ordering::Relaxed);
        if tick == self.checked {
            return;
        }
        self.checked = tick;
        let mut blockers = m.quiescent().blockers;
        blockers.extend(
            plan.snapshot_blockers()
                .into_iter()
                .map(|(id, why)| Blocker::Host {
                    owner: format!("feature {id}"),
                    why,
                }),
        );
        blockers.extend(self.host.iter().map(|name| Blocker::Host {
            owner: format!("host {name}"),
            why: "not audited for snapshots".into(),
        }));
        for line in self
            .tally
            .observe(tick, &blockers, &|target| plan.describe_target(target))
        {
            eprintln!("{line}");
        }
    }

    /// The closing summary.
    pub fn finish(&self) {
        for line in self.tally.summary(10) {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(mode_from(None), Ok(None));
        assert_eq!(mode_from(Some("0")), Ok(None));
        assert_eq!(mode_from(Some("1")), Ok(Some(true)));
        assert_eq!(mode_from(Some("slice")), Ok(Some(false)));
        assert!(mode_from(Some("yes please")).is_err());
    }
}
