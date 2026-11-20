//! The in-process sampling profiler on demand (`profile.sample.*` through
//! the agent API, or `H2_SAMPLE_SECONDS` with `H2_PERF_FROM`): for a
//! device that can't symbolise its own samples, the Xbox app
//! (docs/runtime/xbox.md, "Profiling on the console"). Stopping writes a
//! module-relative dump (`perf::remote`) to `H2_SAMPLE_DIR` (the app's
//! LocalState) as `profile.h2prof`; `h2 profile remote` resolves it on the
//! Mac against the deployed `h2game.dll` and `h2uwp.exe`.
//!
//! Only with the `perf` feature (the Xbox app turns it on); without it the
//! methods answer that the build has no profiler.

use std::path::PathBuf;

use serde_json::{Value, json};

/// Where dumps go: `H2_SAMPLE_DIR`, else the temp directory.
pub const DIR_ENV: &str = "H2_SAMPLE_DIR";
/// `H2_SAMPLE_SECONDS=S`: sample the game thread for S seconds from
/// `H2_PERF_FROM` (a present count, or `level`), then write the dump.
pub const SECONDS_ENV: &str = "H2_SAMPLE_SECONDS";
/// The dump's file name in [`DIR_ENV`].
pub const FILE: &str = "profile.h2prof";

/// Sampling rate when none is asked for.
pub const DEFAULT_HZ: u32 = 1000;

/// The out directory from `H2_SAMPLE_DIR`.
pub fn out_dir() -> PathBuf {
    std::env::var_os(DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `profile.sample.start`'s params.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartParams {
    /// Samples a second (default 1000, at most 10000).
    #[serde(default)]
    pub hz: Option<u32>,
    /// Also sample the D3D render thread.
    #[serde(default)]
    pub render: bool,
    /// Stop by itself after this long (default: when asked).
    #[serde(default)]
    pub seconds: Option<f64>,
}

/// `profile.sample.stop`'s params.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopParams {
    /// Return the dump's text in the answer too (`dump`), so no file
    /// needs fetching.
    #[serde(default)]
    pub inline: bool,
}

#[cfg(feature = "perf")]
mod imp {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use perf::sample::{Sampler, ThreadTarget};
    use serde_json::{Value, json};

    use super::{StartParams, StopParams};

    struct Running {
        sampler: Sampler,
        since: Instant,
        hz: u32,
        until: Option<Instant>,
    }

    #[derive(Default)]
    struct State {
        game: Option<ThreadTarget>,
        running: Option<Running>,
        last: Option<Value>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);

    fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
        let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(State::default))
    }

    pub fn available() -> bool {
        perf::sample::SUPPORTED
    }

    pub fn set_game_thread() {
        let t = ThreadTarget::current("game");
        with(|s| s.game = Some(t));
    }

    pub fn start(p: StartParams) -> Result<Value, String> {
        if !perf::sample::SUPPORTED {
            return Err("this host has no sampling profiler".into());
        }
        with(|s| {
            if s.running.is_some() {
                return Err("already sampling; profile.sample.stop first".into());
            }
            let game = s.game.clone().ok_or("the game thread isn't known yet")?;
            let mut targets = vec![game];
            if p.render {
                targets.extend(
                    perf::profile::registered_threads()
                        .into_iter()
                        .filter(|t| t.name == "render"),
                );
            }
            let names: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
            let hz = p.hz.unwrap_or(super::DEFAULT_HZ).clamp(1, 10_000);
            let sampler = Sampler::start(targets, hz).map_err(|e| e.to_string())?;
            let since = Instant::now();
            let until = p
                .seconds
                .filter(|s| s.is_finite() && *s > 0.0)
                .map(|s| since + Duration::from_secs_f64(s.min(3600.0)));
            s.running = Some(Running {
                sampler,
                since,
                hz,
                until,
            });
            eprintln!(
                "sampling: {} at {hz} Hz{}",
                names.join(", "),
                until.map_or(String::new(), |u| format!(
                    " for {:.1} s",
                    (u - since).as_secs_f64()
                ))
            );
            Ok(json!({ "sampling": true, "hz": hz, "threads": names }))
        })
    }

    pub fn stop(p: StopParams) -> Result<Value, String> {
        let run = with(|s| s.running.take()).ok_or("not sampling; profile.sample.start first")?;
        let raw = run.sampler.finish();
        // The sampled threads run again: the loader lock is safe to take.
        let dump = perf::remote::Dump::from_raw(&raw, perf::remote::locate);
        let text = dump.write();
        let dir = super::out_dir();
        let file = dir.join(super::FILE);
        let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, &text));
        let mut v = json!({
            "file": file.display().to_string(),
            "samples": dump.samples(),
            "stacks": dump.stacks.len(),
            "modules": dump.modules.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
            "missed": dump.missed,
            "wall_ms": dump.wall.as_millis() as u64,
            "hz": run.hz,
            "mean_pause_us": dump.mean_pause_ns as f64 / 1000.0,
        });
        if let Err(e) = written {
            v["write_error"] = Value::from(format!("{}: {e}", file.display()));
        }
        eprintln!(
            "sampling: {} samples in {:.1} s ({} stacks) to {}{}",
            dump.samples(),
            run.since.elapsed().as_secs_f64(),
            dump.stacks.len(),
            file.display(),
            v.get("write_error")
                .map_or(String::new(), |e| format!(" FAILED: {e}"))
        );
        with(|s| s.last = Some(v.clone()));
        if p.inline {
            v["dump"] = Value::from(text);
        }
        Ok(v)
    }

    pub fn status() -> Value {
        with(|s| {
            json!({
                "available": perf::sample::SUPPORTED,
                "sampling": s.running.is_some(),
                "for_ms": s.running.as_ref().map(|r| r.since.elapsed().as_millis() as u64),
                "hz": s.running.as_ref().map(|r| r.hz),
                "last": s.last.clone(),
            })
        })
    }

    /// Between slices: stops a timed run that is due.
    pub fn tick() {
        let due = with(|s| {
            s.running
                .as_ref()
                .and_then(|r| r.until)
                .is_some_and(|u| Instant::now() >= u)
        });
        if due && let Err(e) = stop(StopParams::default()) {
            eprintln!("sampling: {e}");
        }
    }
}

#[cfg(not(feature = "perf"))]
mod imp {
    use serde_json::{Value, json};

    use super::{StartParams, StopParams};

    const NO: &str = "this build has no profiler (boot's perf feature is off)";

    pub fn available() -> bool {
        false
    }
    pub fn set_game_thread() {}
    pub fn start(_: StartParams) -> Result<Value, String> {
        Err(NO.into())
    }
    pub fn stop(_: StopParams) -> Result<Value, String> {
        Err(NO.into())
    }
    pub fn status() -> Value {
        json!({ "available": false, "sampling": false })
    }
    pub fn tick() {}
}

pub use imp::{available, set_game_thread, status, tick};

/// Starts sampling (the game thread, the render thread with `render`).
pub fn start(params: Value) -> Result<Value, String> {
    let p: StartParams = if params.is_null() {
        StartParams::default()
    } else {
        serde_json::from_value(params).map_err(|e| e.to_string())?
    };
    imp::start(p)
}

/// Stops sampling and writes the dump.
pub fn stop(params: Value) -> Result<Value, String> {
    let p: StopParams = if params.is_null() {
        StopParams::default()
    } else {
        serde_json::from_value(params).map_err(|e| e.to_string())?
    };
    imp::stop(p)
}

/// `H2_SAMPLE_SECONDS`, when set and a positive number.
pub fn seconds_from_env() -> Option<f64> {
    std::env::var(SECONDS_ENV)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s > 0.0)
}

/// Starts the timed run `H2_SAMPLE_SECONDS` asks for; called once, when
/// `H2_PERF_FROM`'s moment comes.
pub fn start_timed(seconds: f64) {
    match start(json!({ "seconds": seconds })) {
        Ok(_) => {}
        Err(e) => eprintln!("sampling: not sampling: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_are_checked() {
        assert!(serde_json::from_value::<StartParams>(json!({"hz": 500, "render": true})).is_ok());
        assert!(serde_json::from_value::<StartParams>(json!({"rate": 5})).is_err());
        assert!(
            serde_json::from_value::<StopParams>(json!({"inline": true}))
                .unwrap()
                .inline
        );
        assert!(stop(json!({"bogus": 1})).is_err());
    }

    #[cfg(all(feature = "perf", target_os = "macos"))]
    #[test]
    fn a_run_samples_this_thread_and_writes_a_dump() {
        let dir = std::env::temp_dir().join(format!("h2-sampling-{}", std::process::id()));
        // SAFETY: test-only; no other test reads H2_SAMPLE_DIR.
        unsafe { std::env::set_var(DIR_ENV, &dir) };
        // The "game thread" is a busy worker that registers itself.
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = std::sync::Arc::clone(&stop_flag);
        let (tx, rx) = std::sync::mpsc::channel();
        let w = std::thread::spawn(move || {
            set_game_thread();
            tx.send(()).unwrap();
            let mut x = 0u64;
            while !f.load(std::sync::atomic::Ordering::Relaxed) {
                x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(1));
            }
        });
        rx.recv().unwrap();
        assert!(stop(Value::Null).is_err(), "not started");
        start(json!({"hz": 1000})).unwrap();
        assert!(start(Value::Null).is_err(), "already running");
        assert_eq!(status()["sampling"], true);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let v = stop(json!({"inline": true})).unwrap();
        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        w.join().unwrap();
        assert!(v["samples"].as_u64().unwrap() >= 1, "{v}");
        let dump = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(v["dump"].as_str().unwrap(), dump);
        assert!(perf::remote::Dump::parse(&dump).unwrap().samples() >= 1);
        assert_eq!(status()["sampling"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
