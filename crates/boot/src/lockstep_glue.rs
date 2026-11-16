//! The determinism harness in a boot run (docs/high-framerate.md, phase 1;
//! crates/lockstep, `xapi::tap`).
//!
//! * `H2_LOCKSTEP=FILE`: hash guest RAM at the end of every `game_tick` and
//!   at every present, and write the trace to FILE when the run ends.
//!   `H2_LOCKSTEP_TICKS=0` keeps only the present records, which are taken
//!   between slices and so leave the guest's run exactly as without the
//!   harness; the tick observer adds dispatch transfers (the same in every
//!   run that has it on). Compare two traces with `h2 lockstep A B`.
//! * `H2_INPUT_RECORD=FILE`: log every `XInputGetState` the guest made, its
//!   status and bytes, stamped with the tick it fell in.
//! * `H2_INPUT_REPLAY=FILE`: answer read *n* with FILE's read *n*.
//!
//! Two runs of the same image with the same replay (and the same flags)
//! must produce identical traces; anything else is a determinism bug, and
//! the comparison names the first tick and the RAM megabytes that differ.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};

/// What to record or replay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub trace: Option<PathBuf>,
    /// Hash at every tick too, not only at presents.
    pub ticks: bool,
    pub record: Option<PathBuf>,
    pub replay: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Config {
        let path = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Config {
            trace: path("H2_LOCKSTEP"),
            ticks: std::env::var("H2_LOCKSTEP_TICKS").map_or(true, |v| v != "0"),
            record: path("H2_INPUT_RECORD"),
            replay: path("H2_INPUT_REPLAY"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.trace.is_some() || self.record.is_some() || self.replay.is_some()
    }
}

/// The harness attached to one run.
pub struct Lockstep {
    config: Config,
    recorder: Option<Arc<lockstep::Recorder>>,
    input: Option<Arc<Mutex<xapi::tap::InputLog>>>,
    replay: Option<Arc<Mutex<xapi::tap::ReplayStats>>>,
    /// Why ticks are not hashed, when they were asked for and could not be.
    pub tick_note: Option<String>,
}

impl Lockstep {
    /// Sets up what `config` asks for: the tick observer (added to
    /// `registry`; it is hooked when the plan is installed), and an input tap
    /// on `xapi`. When the plan turns the observer off, say so with
    /// [`set_tick_note`](Self::set_tick_note).
    pub fn install<S: xapi::Storage>(
        registry: &mut feature::Registry,
        config: Config,
        hook: lockstep::TickHook,
        xapi: Option<&Mutex<xapi::Xapi<S>>>,
    ) -> Result<Lockstep> {
        if config.record.is_some() && config.replay.is_some() {
            bail!("H2_INPUT_RECORD and H2_INPUT_REPLAY are exclusive");
        }
        let mut tick_note = None;
        let recorder = config
            .trace
            .as_ref()
            .map(|_| lockstep::Recorder::new(hook.game_time));
        if let Some(r) = &recorder
            && config.ticks
        {
            match registry.add(&lockstep::TickObserver(Arc::clone(r))) {
                Ok(true) => {}
                Ok(false) => tick_note = Some("features: lockstep is switched off".to_owned()),
                Err(e) => tick_note = Some(format!("lockstep: {e}")),
            }
        }
        let mut input = None;
        let mut replay = None;
        if config.record.is_some() || config.replay.is_some() {
            let Some(x) = xapi else {
                bail!("input record or replay needs XAPI (it taps XInputGetState)")
            };
            let tap: Box<dyn xapi::tap::InputTap> = if let Some(path) = &config.replay {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let log = xapi::tap::InputLog::parse(&text)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                let (r, stats) = xapi::tap::Replayer::new(log);
                replay = Some(stats);
                Box::new(r)
            } else {
                let (mut r, log) = xapi::tap::Recorder::new();
                if let Some(rec) = &recorder {
                    r = r.with_ticks(rec.tick_counter());
                }
                input = Some(log);
                Box::new(r)
            };
            x.lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_input_tap(Some(tap));
        }
        Ok(Lockstep {
            config,
            recorder,
            input,
            replay,
            tick_note,
        })
    }

    /// Why ticks are not hashed, when the installed plan left the observer
    /// out.
    pub fn set_tick_note(&mut self, note: String) {
        self.tick_note = Some(note);
    }

    /// Between slices: a record for each present since the last call.
    pub fn after_slice(&self, rt: &runtime::Runtime, presents: u64) {
        if let Some(r) = &self.recorder {
            let clock = rt.guest_time();
            r.record_presents(presents, &rt.memory(), clock);
        }
    }

    /// The trace so far.
    pub fn trace(&self) -> Option<lockstep::Trace> {
        self.recorder.as_ref().map(|r| r.trace())
    }

    /// Writes the trace and the input log; returns a line for the report.
    pub fn finish(&self) -> Result<String> {
        let mut said = Vec::new();
        if let (Some(path), Some(t)) = (&self.config.trace, self.trace()) {
            std::fs::write(path, t.to_text())
                .with_context(|| format!("writing {}", path.display()))?;
            said.push(format!(
                "{} ticks, {} records -> {}",
                t.ticks(),
                t.records.len(),
                path.display()
            ));
            if let Some(n) = &self.tick_note {
                said.push(format!("ticks not hashed: {n}"));
            }
        }
        if let (Some(path), Some(log)) = (&self.config.record, &self.input) {
            let log = log.lock().unwrap_or_else(|e| e.into_inner());
            std::fs::write(path, log.to_text())
                .with_context(|| format!("writing {}", path.display()))?;
            said.push(format!(
                "{} input reads -> {}",
                log.reads.len(),
                path.display()
            ));
        }
        if let Some(s) = &self.replay {
            let s = *s.lock().unwrap_or_else(|e| e.into_inner());
            said.push(format!(
                "replayed {} reads ({} past the end, {} handle mismatches)",
                s.replayed, s.past_end, s.handle_mismatches
            ));
        }
        Ok(format!("lockstep: {}", said.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toy_guest_through_boot_s_harness_writes_a_trace_that_compares() {
        use lockstep::synthetic::{self, Toy};
        let dir = std::env::temp_dir().join(format!("boot-lockstep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |name: &str, lx: i16| {
            let mut toy = Toy::new();
            let config = Config {
                trace: Some(dir.join(name)),
                ticks: true,
                ..Default::default()
            };
            let mut features = feature::Registry::new(feature::Switches::defaults());
            let h =
                Lockstep::install::<xapi::VfsStorage>(&mut features, config, synthetic::HOOK, None)
                    .unwrap();
            let addrs = feature::Remap::new(&[(&feature::points::GAME_TICK, synthetic::TICK)]);
            features
                .build(&addrs)
                .unwrap()
                .install(&toy.runtime)
                .unwrap();
            for f in 0..20u32 {
                toy.set_input(&synthetic::packet(
                    f,
                    0,
                    if f == 2 { 255 } else { 0 },
                    if f == 9 { lx } else { 900 },
                ));
                toy.frame();
                h.after_slice(&toy.runtime, u64::from(f) + 1);
            }
            let line = h.finish().unwrap();
            assert!(line.contains("20 ticks, 40 records"), "{line}");
            lockstep::Trace::parse(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap()
        };
        let (a, b, c) = (run("a", 900), run("b", 900), run("c", -900));
        assert!(lockstep::compare(&a, &b).identical());
        assert!(matches!(
            lockstep::compare(&a, &c),
            lockstep::Verdict::Diverged { index: 18, .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn input_needs_xapi_and_record_excludes_replay() {
        let mut features = feature::Registry::new(feature::Switches::defaults());
        let both = Config {
            record: Some("a".into()),
            replay: Some("b".into()),
            ..Default::default()
        };
        assert!(
            Lockstep::install::<xapi::VfsStorage>(
                &mut features,
                both,
                lockstep::synthetic::HOOK,
                None
            )
            .is_err()
        );
        let rec = Config {
            record: Some("a".into()),
            ..Default::default()
        };
        assert!(
            Lockstep::install::<xapi::VfsStorage>(
                &mut features,
                rec,
                lockstep::synthetic::HOOK,
                None
            )
            .is_err()
        );
        assert!(!Config::default().enabled());
    }
}
