//! A process-local watchdog shared by h2, the editor and the lab daemon.
mod diagnostics;
mod gauge;
mod limits;
mod sample;
#[cfg(test)]
mod tests;

pub use diagnostics::diagnostics;
pub use gauge::{Counter, Gauge};
pub use limits::parse_size;
#[cfg(target_os = "macos")]
pub use sample::process_rusage;
pub use sample::{footprint, pressure};

use limits::{History, Limits, SAMPLES, Trigger};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MEMORY_LIMIT_EXIT: i32 = 86;
const PERIOD: Duration = Duration::from_secs(2);
type Probes = BTreeMap<&'static str, Box<dyn Fn() -> u64 + Send + Sync>>;
static PROBES: LazyLock<Mutex<Probes>> = LazyLock::new(Mutex::default);
static WATCH: OnceLock<Watch> = OnceLock::new();
static START: Once = Once::new();
static SERIAL: AtomicU64 = AtomicU64::new(0);
// One client can request OS diagnostics at a time. Automatic hard reports
// always have their own slot even if a socket thread is waiting for vmmap.
static MANUAL: Mutex<()> = Mutex::new(());

/// Register once at startup. Reads should only load a counter, with no I/O.
pub fn register_probe(name: &'static str, read: impl Fn() -> u64 + Send + Sync + 'static) {
    PROBES.lock().unwrap().insert(name, Box::new(read));
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub footprint_bytes: u64,
    pub peak_bytes: u64,
    pub soft: u64,
    pub hard: u64,
    pub growth_bytes_per_min: f64,
    pub samples: Vec<(u64, u64)>,
    pub probes: BTreeMap<String, u64>,
    pub reports: Vec<PathBuf>,
}

struct Watch {
    since: Instant,
    dir: PathBuf,
    limits: Limits,
    commit: Option<String>,
    history: Mutex<History>,
    reports: Mutex<VecDeque<PathBuf>>,
}

impl Watch {
    fn elapsed_ms(&self) -> u64 {
        self.since.elapsed().as_millis() as u64
    }

    fn snapshot(&self, count: usize) -> Snapshot {
        let mut snapshot = {
            let history = self.history.lock().unwrap();
            Snapshot {
                footprint_bytes: history.samples.back().map_or(0, |s| s.1),
                peak_bytes: history.peak,
                soft: self.limits.soft,
                hard: self.limits.hard,
                growth_bytes_per_min: history.growth,
                samples: history.samples.iter().skip(history.samples.len().saturating_sub(count))
                    .copied().collect(),
                ..Snapshot::default()
            }
        };
        snapshot.probes = PROBES.lock().unwrap().iter()
            .map(|(&name, read)| (name.to_owned(), read())).collect();
        snapshot.reports = self.reports.lock().unwrap().iter().cloned().collect();
        snapshot
    }
}

pub fn snapshot() -> Snapshot {
    WATCH.get().map_or_else(Snapshot::default, |watch| watch.snapshot(60))
}

/// Defaults relative to the same data root the caller selected for the game.
pub fn report_dir(data_dir: &Path) -> PathBuf {
    std::env::var_os("H2_MEM_REPORT_DIR").filter(|p| !p.is_empty()).map_or_else(
        || data_dir.join("logs/mem"), PathBuf::from)
}

/// Idempotent, including when play enters the common run path.
pub fn start(data_dir: &Path, commit: Option<&str>) {
    START.call_once(|| {
        let watch = WATCH.get_or_init(|| Watch {
            since: Instant::now(), dir: report_dir(data_dir),
            limits: Limits::from_env(), commit: commit.filter(|s| !s.is_empty() && *s != "unknown")
                .or(option_env!("H2_BUILD_SHA")).map(str::to_owned),
            history: Mutex::default(), reports: Mutex::default(),
        });
        thread::Builder::new().name("memory-watch".into()).spawn(move || monitor(watch))
            .expect("starting process memory watchdog");
    });
}

fn monitor(watch: &'static Watch) {
    let pid = std::process::id();
    let mut failed = false;
    loop {
        let sample = footprint(pid);
        if sample.is_none() && !failed {
            eprintln!("memory watchdog: cannot read footprint for pid {pid}");
        }
        failed = sample.is_none();
        let trigger = watch.history.lock().unwrap().poll(watch.limits,
            || watch.elapsed_ms(), || sample);
        if let Some(trigger) = trigger {
            let bytes = sample.unwrap();
            if trigger == Trigger::Hard {
                match write_report(watch, trigger.label()) {
                    Ok(path) => eprintln!("memory limit: {bytes} over {}; report {}", watch.limits.hard, path.display()),
                    Err(e) => eprintln!("memory limit: {bytes} over {}; report failed in {}: {e}", watch.limits.hard, watch.dir.display()),
                }
                std::process::exit(MEMORY_LIMIT_EXIT);
            }
            // Sampling and hard decisions continue while the OS tools run.
            let _ = thread::Builder::new().name("memory-report".into()).spawn(move || {
                match write_report(watch, trigger.label()) {
                    Ok(path) => eprintln!("memory watchdog: {}, {bytes} bytes; report {}", trigger.label(), path.display()),
                    Err(e) => eprintln!("memory watchdog: {}, {bytes} bytes; report failed: {e}", trigger.label()),
                }
            }).inspect_err(|e| eprintln!("memory watchdog: could not start report writer: {e}"));
        }
        thread::sleep(PERIOD);
    }
}

#[derive(Serialize)]
struct Report {
    pid: u32,
    argv: Vec<String>,
    build_commit: Option<String>,
    uptime_ms: u64,
    unix_ms: u64,
    reason: String,
    job_id: Option<String>,
    job_name: Option<String>,
    #[serde(flatten)]
    memory: Snapshot,
    diagnostics: BTreeMap<String, String>,
}

fn write_report(watch: &Watch, reason: &str) -> io::Result<PathBuf> {
    let when = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let pid = std::process::id();
    let path = watch.dir.join(format!("mem-{pid}-{}-{}.json", when.as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)));
    let mut report = Report {
        pid, argv: std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect(),
        build_commit: watch.commit.clone(), uptime_ms: watch.elapsed_ms(),
        unix_ms: when.as_millis() as u64, reason: reason.to_owned(),
        job_id: std::env::var("H2_LAB_JOB_ID").ok(), job_name: std::env::var("H2_LAB_JOB_NAME").ok(),
        memory: watch.snapshot(SAMPLES), diagnostics: BTreeMap::new(),
    };
    fs::create_dir_all(&watch.dir)?;
    // Leave a complete report if the lab kills us during OS diagnostics.
    save(&path, &report)?;
    let mut reports = watch.reports.lock().unwrap();
    if reports.len() == 60 { reports.pop_front(); }
    reports.push_back(path.clone());
    drop(reports);
    report.diagnostics = diagnostics(pid);
    if !report.diagnostics.is_empty() {
        save(&path, &report)?;
    }
    Ok(path)
}

fn save(path: &Path, report: &Report) -> io::Result<()> {
    use std::io::{BufWriter, Write};
    let temp = path.with_extension("partial");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut writer = BufWriter::new(options.open(&temp)?);
    let result = (|| {
        serde_json::to_writer_pretty(&mut writer, report)?;
        writer.flush()?;
        drop(writer);
        fs::rename(&temp, path)
    })();
    if result.is_err() { let _ = fs::remove_file(temp); }
    result
}

/// Write a report on demand, leaving automatic enforcement available.
pub fn report() -> io::Result<PathBuf> {
    let watch = WATCH.get().ok_or_else(|| io::Error::other("watchdog has not started"))?;
    let _guard = MANUAL.try_lock().map_err(|_| io::Error::new(io::ErrorKind::WouldBlock,
        "another memory report is in progress"))?;
    write_report(watch, "requested")
}
