//! `boot/menu-hold`: boot the game to the main menu, then hold it for N
//! frames and report wall-clock frames per second.
//!
//! The boot driver lives on the `boot` branch (crates/apps/boot), which this
//! crate must not depend on. It plugs in through [`BootDriver`] and
//! [`set_boot_driver`]; until something registers a factory the scenario
//! is recorded as skipped. Frames are counted with [`timeline::frames`],
//! which D3D's Swap bumps, so a driver only has to run the machine.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::bench::{BenchCtx, Better, Measurement, Prepared, Scenario};

/// What the boot scenario needs from a boot driver.
pub trait BootDriver {
    /// Run until the main menu is up. Called once, untimed.
    fn boot_to_menu(&mut self) -> Result<()>;
    /// Run one scheduling step (a few guest slices plus input pumping).
    /// `Ok(false)` means the machine stopped and cannot go on.
    fn step(&mut self) -> Result<bool>;
}

/// Where the boot driver finds the game.
#[derive(Debug, Clone)]
pub struct BootArgs {
    /// The disc image (`$H2_DISC`, `--disc`).
    pub disc: PathBuf,
    /// Scratch for the build and the data partitions.
    pub work: PathBuf,
}

type Factory = fn(&BootArgs) -> Result<Box<dyn BootDriver>>;

static FACTORY: Mutex<Option<Factory>> = Mutex::new(None);

/// Register the boot driver. The `boot` crate, or the CLI built with it,
/// calls this once at startup.
pub fn set_boot_driver(f: Factory) {
    *FACTORY.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

pub fn boot_driver_available() -> bool {
    FACTORY.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Step `driver` until `frames` more frames have been presented. Returns
/// the wall time it took. Fails if the machine stops or `timeout` passes.
pub fn hold(driver: &mut dyn BootDriver, frames: u32, timeout: Duration) -> Result<Duration> {
    let start_frames = timeline::frames();
    let t = Instant::now();
    while timeline::frames() - start_frames < frames as u64 {
        if !driver.step()? {
            bail!(
                "the machine stopped after {} of {frames} frames",
                timeline::frames() - start_frames
            );
        }
        if t.elapsed() > timeout {
            bail!(
                "only {} of {frames} frames in {timeout:?}",
                timeline::frames() - start_frames
            );
        }
    }
    Ok(t.elapsed())
}

pub struct MenuHold {
    disc: Option<PathBuf>,
    frames: u32,
    driver: Option<Box<dyn BootDriver>>,
}

impl MenuHold {
    pub fn new(disc: Option<PathBuf>) -> Self {
        MenuHold {
            disc,
            frames: 120,
            driver: None,
        }
    }
}

impl Scenario for MenuHold {
    fn name(&self) -> &str {
        "boot/menu-hold"
    }
    fn description(&self) -> &str {
        "boot to the main menu, then hold it for N frames (wall fps)"
    }
    fn unit(&self) -> &str {
        "fps"
    }
    fn better(&self) -> Better {
        Better::Higher
    }
    fn prepare(&mut self, ctx: &BenchCtx) -> Result<Prepared> {
        let Some(factory) = *FACTORY.lock().unwrap_or_else(|e| e.into_inner()) else {
            return Ok(Prepared::Skip(
                "no boot driver in this build (see docs/perf/profiling.md, \"Boot hook\")".into(),
            ));
        };
        let Some(disc) = self
            .disc
            .clone()
            .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        else {
            return Ok(Prepared::Skip("no disc: pass --disc or set H2_DISC".into()));
        };
        self.frames = ctx.frames.max(1);
        let mut d = factory(&BootArgs {
            disc,
            work: ctx.work.join("boot"),
        })?;
        d.boot_to_menu().context("booting to the menu")?;
        self.driver = Some(d);
        Ok(Prepared::Ready)
    }
    fn measure(&mut self) -> Result<Measurement> {
        let d = self.driver.as_mut().context("not prepared")?;
        let wall = hold(d.as_mut(), self.frames, Duration::from_secs(600))?;
        let fps = self.frames as f64 / wall.as_secs_f64();
        Ok(Measurement::new(fps)
            .with("frames", self.frames as f64)
            .with(
                "ms_per_frame",
                wall.as_secs_f64() * 1000.0 / self.frames as f64,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presents a frame every third step, stops after `limit` steps.
    struct Fake {
        steps: u32,
        limit: u32,
    }
    impl BootDriver for Fake {
        fn boot_to_menu(&mut self) -> Result<()> {
            Ok(())
        }
        fn step(&mut self) -> Result<bool> {
            self.steps += 1;
            if self.steps.is_multiple_of(3) {
                timeline::frame();
            }
            Ok(self.steps < self.limit)
        }
    }

    #[test]
    fn hold_counts_presented_frames() {
        let mut f = Fake {
            steps: 0,
            limit: 1000,
        };
        hold(&mut f, 5, Duration::from_secs(10)).unwrap();
        // Other tests may present frames concurrently; never fewer steps.
        assert!(f.steps <= 15, "{}", f.steps);
        let mut f = Fake { steps: 0, limit: 4 };
        assert!(hold(&mut f, 100, Duration::from_secs(10)).is_err());
    }

    #[test]
    fn without_a_driver_the_scenario_skips() {
        if boot_driver_available() {
            return;
        }
        let mut s = MenuHold::new(None);
        let ctx = BenchCtx {
            work: std::env::temp_dir(),
            quick: true,
            frames: 1,
        };
        assert!(matches!(s.prepare(&ctx).unwrap(), Prepared::Skip(_)));
    }
}
