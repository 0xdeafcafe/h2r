//! The whole Halo 1 import: every campaign level and every multiplayer map
//! (both sandboxes) of a Halo: Combat Evolved disc, converted onto the
//! Halo 2 disc's maps and written into the data root the game reads
//! (docs/halo1/campaign.md "Importing", docs/halo1/multiplayer.md
//! "Importing").
//!
//! Both sandboxes and both campaign asset variants are converted levels
//! in the tag store. Campaign manifests, cards and HUD data stay under
//! `<data>/campaigns/halo1`; the multiplayer list metadata lives in the store.
//! The import stamp records the source and the outcome of each job.
//!
//! `h2 play`, `h2 run`, `h2 serve` and `h2 play --follow` run it before
//! booting when `halo1.iso` is next to the Halo 2 disc: the `halo1`
//! package's `convert` item, through [`package::Halo1`] and
//! `crate::package_prepare`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use h1convert::convert::{Sandbox, known, map_name, structure_mopp};
use h1map::H1Map;
use h1map::disc::{CAMPAIGN_LEVELS, Disc, map_stem};
use sha2::{Digest, Sha256};
use xiso::Xiso;
use workspace::converted::{
    halo1_campaign_stem, halo1_metadata, halo1_mp_stem, marker_path, store_converted,
};

use crate::h1_cmd;
use crate::safe_out::OutDir;

/// The converters' output version. **Bump it whenever a change to the
/// Halo 1 converters (h1convert, h1campaign, h1objects, h1ai, h1scripts,
/// h1sounds, h1bitmaps, h1models, h1sky, h1text, or the map builder they
/// use) changes the maps they write**: every import stamped with an older
/// version is converted again at the next `h2 play`, so `--follow` users
/// get the new maps after the merge.
pub const CONVERTER_VERSION: u32 = 103;

/// The stamp's file, in `<data>/campaigns/halo1/`.
pub const STAMP_FILE: &str = "import.stamp";
/// The Halo 1 disc's name beside the Halo 2 disc.
pub const AUTO_NAME: &str = "halo1.iso";
/// The campaign's folder under `<data>/campaigns`.
const CAMPAIGN_KEY: &str = h1campaign::manifest::KEY;
/// The sandboxes every multiplayer map is converted in.
const SANDBOXES: [Sandbox; 2] = [Sandbox::Halo2, Sandbox::Halo1];

/// One map to convert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job<'a> {
    /// A campaign level, `a10` .. `d40`.
    Campaign(&'a str),
    /// A campaign level's Halo 2-assets variant (docs/halo1/campaign.md
    /// §5).
    CampaignHalo2(&'a str),
    /// A multiplayer map in one sandbox.
    Multiplayer(&'a str, Sandbox),
}

impl Job<'_> {
    /// Its key in the stamp: `a10`, `bloodgulch/h2`.
    pub fn key(&self) -> String {
        match self {
            Job::Campaign(l) => l.to_string(),
            Job::CampaignHalo2(l) => format!("{l}/h2"),
            Job::Multiplayer(m, s) => format!("{m}/{}", s.name()),
        }
    }

    /// The Halo 1 map it converts.
    pub fn source(&self) -> &str {
        match self {
            Job::Campaign(l) | Job::CampaignHalo2(l) => l,
            Job::Multiplayer(m, _) => m,
        }
    }

    /// What its line calls it.
    pub fn label(&self) -> String {
        match self {
            Job::Campaign(l) => format!("campaign {l}"),
            Job::CampaignHalo2(l) => format!("campaign {l} (Halo 2 assets)"),
            Job::Multiplayer(m, s) => format!(
                "multiplayer {m} ({} sandbox)",
                if *s == Sandbox::Halo1 {
                    "Halo 1"
                } else {
                    "Halo 2"
                }
            ),
        }
    }

    pub fn stem(&self) -> String {
        match self {
            Job::Campaign(l) => halo1_campaign_stem(l, "h1"),
            Job::CampaignHalo2(l) => halo1_campaign_stem(l, "h2"),
            Job::Multiplayer(m, sandbox) => halo1_mp_stem(&map_name(m), sandbox.name()),
        }
    }

    /// The store marker written when the level is complete.
    pub fn output(&self, data: &Path) -> PathBuf {
        marker_path(&boot::store_status::root(data), &self.stem())
    }
}

/// `<data>/campaigns/halo1`.
pub fn campaign_dir(data: &Path) -> PathBuf {
    data.join("campaigns").join(CAMPAIGN_KEY)
}

/// The jobs for a disc whose `maps/` holds `stems`: each campaign level it
/// has, in play order, then each multiplayer map the converter knows, in
/// its table's order, in both sandboxes, then each level's Halo 2-assets
/// variant (the default assets are Halo 1's, so those come first).
pub fn jobs(stems: &[String]) -> Vec<Job<'static>> {
    let has = |n: &str| stems.iter().any(|s| s.eq_ignore_ascii_case(n));
    let mut out: Vec<Job> = CAMPAIGN_LEVELS
        .iter()
        .filter(|l| has(l))
        .map(|l| Job::Campaign(l))
        .collect();
    for (m, _, _) in h1convert::convert::MAPS.iter().filter(|(m, _, _)| has(m)) {
        out.extend(SANDBOXES.map(|s| Job::Multiplayer(m, s)));
    }
    out.extend(
        CAMPAIGN_LEVELS
            .iter()
            .filter(|l| has(l))
            .map(|l| Job::CampaignHalo2(l)),
    );
    out
}

/// What was converted from which disc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub converter: u32,
    /// The Halo 1 disc's SHA-256, hex.
    pub sha256: String,
    /// Its size and modification time (seconds), so an unchanged disc is
    /// not hashed again.
    pub size: u64,
    pub mtime: u64,
    /// By job key: `Ok` when written, else why it failed.
    pub done: BTreeMap<String, Result<(), String>>,
}

impl Stamp {
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "# h2 import halo1 (docs/halo1/campaign.md \"Importing\"). Delete this file, or\n\
             # run `h2 import halo1 --force`, to convert everything again.\n\
             converter={}\nsha256={}\nsize={}\nmtime={}\n",
            self.converter, self.sha256, self.size, self.mtime
        );
        for (k, r) in &self.done {
            match r {
                Ok(()) => s += &format!("map {k} ok\n"),
                Err(e) => s += &format!("map {k} failed {}\n", e.replace('\n', " ")),
            }
        }
        s
    }

    /// The stamp in `text`, or `None` when it isn't one.
    pub fn parse(text: &str) -> Option<Self> {
        let mut st = Stamp::default();
        let mut seen = 0;
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        {
            if let Some(rest) = line.strip_prefix("map ") {
                let (key, rest) = rest.split_once(' ')?;
                let r = if rest == "ok" {
                    Ok(())
                } else {
                    Err(rest.strip_prefix("failed ").unwrap_or(rest).to_owned())
                };
                st.done.insert(key.to_owned(), r);
                continue;
            }
            let (k, v) = line.split_once('=')?;
            match k {
                "converter" => st.converter = v.parse().ok()?,
                "sha256" => st.sha256 = v.to_owned(),
                "size" => st.size = v.parse().ok()?,
                "mtime" => st.mtime = v.parse().ok()?,
                _ => return None,
            }
            seen += 1;
        }
        (seen == 4).then_some(st)
    }

    /// The stamp in `<data>/campaigns/halo1/`, if there is a readable one.
    pub fn read(data: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(campaign_dir(data).join(STAMP_FILE)).ok()?)
    }

    fn write(&self, data: &Path) -> Result<()> {
        let dir = campaign_dir(data);
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".{STAMP_FILE}.tmp-{}", std::process::id()));
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, dir.join(STAMP_FILE))?;
        Ok(())
    }
}

/// A disc's size and modification time.
fn size_mtime(iso: &Path) -> Result<(u64, u64)> {
    let m = std::fs::metadata(iso).with_context(|| format!("reading {}", iso.display()))?;
    let mtime = m
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok((m.len(), mtime))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h1map::disc::hex(&h.finalize()))
}

/// The stamp to carry on from for this disc and this converter: `old`'s
/// outcomes when both are the same, else none. The disc is hashed only
/// when its size or modification time differ from `old`'s.
pub fn fresh_stamp(
    old: Option<&Stamp>,
    size: u64,
    mtime: u64,
    hash: impl FnOnce() -> Result<String>,
) -> Result<Stamp> {
    let sha256 = match old {
        Some(o) if o.size == size && o.mtime == mtime && !o.sha256.is_empty() => o.sha256.clone(),
        _ => hash()?,
    };
    let done = match old {
        Some(o) if o.converter == CONVERTER_VERSION && o.sha256 == sha256 => o.done.clone(),
        _ => BTreeMap::new(),
    };
    Ok(Stamp {
        converter: CONVERTER_VERSION,
        sha256,
        size,
        mtime,
        done,
    })
}

/// The jobs still to do: not in the stamp, or written once but whose map
/// file is gone. A map that failed with this converter and this disc is
/// not tried again (`--force` does).
pub fn pending<'a>(jobs: &[Job<'a>], stamp: &Stamp, data: &Path) -> Vec<Job<'a>> {
    jobs.iter()
        .filter(|j| match stamp.done.get(&j.key()) {
            None => true,
            Some(Ok(())) => !j.output(data).is_file(),
            Some(Err(_)) => false,
        })
        .copied()
        .collect()
}

/// The order [`import`] starts `todo` in: the campaign levels first (the
/// longest jobs, so the short ones fill in at the end), then the
/// multiplayer maps; within each, every map's first job before any map's
/// second, so a map's second job starts once its first has read it (a
/// level's two variants and a map's two sandboxes share the read), and
/// the biggest maps (`size`, its file on the disc) first.
pub fn schedule<'a>(todo: &[Job<'a>], size: impl Fn(&str) -> u64) -> Vec<Job<'a>> {
    let round = |j: &Job| {
        let i = todo.iter().position(|t| t == j).unwrap_or(0);
        todo[..i].iter().filter(|t| t.source() == j.source()).count()
    };
    let mut out = todo.to_vec();
    out.sort_by_key(|j| {
        (
            matches!(j, Job::Multiplayer(..)),
            round(j),
            std::cmp::Reverse(size(j.source())),
        )
    });
    out
}

/// Converts one map and writes it. Called from several threads at once
/// ([`run_jobs`]), one job each.
pub trait Converter: Sync {
    fn convert(&self, job: &Job) -> Result<()>;
}

/// What a run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub converted: usize,
    pub failed: usize,
    /// Already done before this run.
    pub skipped: usize,
}

/// One job's time: the whole of it, and its phases
/// ([`h1map::phase`]), each phase's own time.
#[derive(Clone, Debug)]
pub struct JobTime {
    pub key: String,
    pub total: Duration,
    pub phases: h1map::phase::Phases,
    /// What the conversion noted (its retries).
    pub notes: Vec<String>,
}

/// How many jobs [`import`] runs at once: `H2_IMPORT_JOBS` when set, else
/// one per core, and no more than one per [`JOB_MEMORY`] of the machine's
/// memory.
pub fn workers() -> usize {
    workers_with(
        std::env::var("H2_IMPORT_JOBS").ok().as_deref(),
        std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory_bytes(),
    )
}

/// The most one job holds at its peak (a campaign level: its base, the
/// level, the tag set and the map built from it), rounded up.
pub const JOB_MEMORY: u64 = 3 << 30;

fn workers_with(env: Option<&str>, cores: usize, memory: Option<u64>) -> usize {
    if let Some(n) = env.and_then(|v| v.trim().parse::<usize>().ok()) {
        return n.max(1);
    }
    let by_memory = memory.map_or(usize::MAX, |m| (m / JOB_MEMORY) as usize);
    cores.min(by_memory).max(1)
}

/// The machine's memory, where the system says.
fn memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `hw.memsize` is a 64-bit integer; `v` and `len` describe
        // a buffer that size.
        let r = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut v).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (r == 0 && v > 0).then_some(v)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // Windows requires dwLength before it writes the result.
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = text
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        Some(kb * 1024)
    }
}

/// Runs `todo` on `workers` threads, taking jobs in order; a line each
/// through `say` as each finishes; a failure (or a panic) in one is
/// recorded and the rest go on. The stamp is saved after each. A failed job
/// leaves no playable map: the one an older converter wrote under `data` is
/// withdrawn ([`withdraw`]) and the line says so. Returns each converted
/// job's time too.
pub fn run_jobs(
    todo: &[Job],
    data: &Path,
    workers: usize,
    stamp: &mut Stamp,
    conv: &dyn Converter,
    save: &mut dyn FnMut(&Stamp) -> Result<()>,
    say: &mut dyn FnMut(String),
) -> (Summary, Vec<JobTime>) {
    h1map::phase::set_cpu_clock(thread_cpu);
    let mut sum = Summary::default();
    let mut times = Vec::new();
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        for _ in 0..workers.clamp(1, todo.len().max(1)) {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = todo.get(i) else { break };
                    let t = Instant::now();
                    let (r, rec) = h1map::phase::record(|| {
                        catch_unwind(AssertUnwindSafe(|| conv.convert(job)))
                            .unwrap_or_else(|p| Err(anyhow!("panicked: {}", panic_text(&p))))
                    });
                    let time = JobTime {
                        key: job.key(),
                        total: t.elapsed(),
                        phases: rec.phases,
                        notes: rec.notes,
                    };
                    if tx.send((job, r, time)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (n, (job, r, time)) in rx.iter().enumerate() {
            let n = format!("[{}/{}]", n + 1, todo.len());
            match &r {
                Ok(()) => {
                    sum.converted += 1;
                    say(format!(
                        "{n} {}: done ({:.1} s)",
                        job.label(),
                        time.total.as_secs_f32()
                    ));
                    times.push(time);
                }
                Err(e) => {
                    sum.failed += 1;
                    say(format!("{n} {}: failed: {e:#}", job.label()));
                    match withdraw(job, data) {
                        Ok(true) => say(format!(
                            "{n} {}: the map an older converter wrote is removed, so it is not offered",
                            job.label()
                        )),
                        Ok(false) => {}
                        Err(w) => say(format!(
                            "{n} {}: could not remove the map an older converter wrote: {w:#}",
                            job.label()
                        )),
                    }
                }
            }
            stamp
                .done
                .insert(job.key(), r.map_err(|e| format!("{e:#}")));
            if let Err(e) = save(stamp) {
                say(format!("could not save the import stamp: {e:#}"));
            }
        }
    });
    (sum, times)
}

/// Takes a failed job's earlier map out of play: its store marker becomes
/// the `.converting` ownership record `store_converted` writes first, so
/// the level is no longer listed or booted (the game finds converted levels
/// by their marker) and the next conversion still owns the stem. For a
/// multiplayer map the other sandbox stays; a map with neither is not in
/// the multiplayer list (`new_maps::stored_halo1` skips a variant with no
/// verified level). Whether a marker was there.
fn withdraw(job: &Job, data: &Path) -> Result<bool> {
    let marker = job.output(data);
    if !marker.is_file() {
        return Ok(false);
    }
    let pending = marker.with_extension("converting");
    std::fs::rename(&marker, &pending)
        .with_context(|| format!("{} to {}", marker.display(), pending.display()))?;
    Ok(true)
}

/// The timing report's file, in `<data>/campaigns/halo1/`.
pub const TIMING_FILE: &str = "import-timing.txt";

/// The calling thread's CPU time, for [`h1map::phase`].
fn thread_cpu() -> Duration {
    Duration::from_nanos(timeline::host::thread_cpu_ns().unwrap_or_default())
}

/// The process's CPU time and peak memory so far, for the report.
#[cfg(unix)]
fn usage_text() -> String {
    // SAFETY: `ru` is a valid rusage to write.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } != 0 {
        return String::new();
    }
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    // Bytes on macOS, KiB elsewhere.
    let rss = if cfg!(target_os = "macos") {
        ru.ru_maxrss as f64
    } else {
        ru.ru_maxrss as f64 * 1024.0
    };
    format!(
        "\nprocess: {:.1} s user, {:.1} s system, peak resident {:.2} GB, {} page reclaims\n",
        t(ru.ru_utime),
        t(ru.ru_stime),
        rss / 1e9,
        ru.ru_minflt
    )
}

#[cfg(not(unix))]
fn usage_text() -> String {
    timeline::host::process_cpu_ns().map_or_else(String::new, |ns| {
        format!("\nprocess: {:.1} s CPU\n", Duration::from_nanos(ns).as_secs_f64())
    })
}

/// The timing report: the run's wall time and workers, each phase's time
/// over every job (most CPU first), then each job's. Wall time stretches
/// on a loaded machine; a phase's CPU time (its thread's) does not.
pub fn timing_text(wall: Duration, workers: usize, times: &[JobTime]) -> String {
    let secs = |d: &Duration| d.as_secs_f64();
    let mut all: h1map::phase::Phases = Vec::new();
    for &(name, w, c) in times.iter().flat_map(|t| t.phases.iter()) {
        match all.iter_mut().find(|p| p.0 == name) {
            Some(p) => {
                p.1 += w;
                p.2 += c;
            }
            None => all.push((name, w, c)),
        }
    }
    all.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    let jobs: Duration = times.iter().map(|t| t.total).sum();
    let cpu: Duration = all.iter().map(|p| p.2).sum();
    let mut s = format!(
        "# h2 import halo1: where the time went (converter version {CONVERTER_VERSION})\n\
         wall {:.1} s, {workers} workers, {} jobs, {:.1} s of job time, {:.1} s of it on a CPU\n",
        secs(&wall),
        times.len(),
        secs(&jobs),
        secs(&cpu),
    );
    s += "\nphases over every job (each phase's own time):\n";
    s += &format!("  {:<24} {:>8} {:>8} {:>6}\n", "", "wall", "cpu", "cpu%");
    for (n, w, c) in &all {
        let pct = if cpu.is_zero() {
            0.0
        } else {
            100.0 * secs(c) / secs(&cpu)
        };
        s += &format!("  {n:<24} {:>8.2} {:>8.2} {pct:>5.1}%\n", secs(w), secs(c));
    }
    s += "\njobs (wall; each phase's CPU time):\n";
    for t in times {
        let mut phases = t.phases.clone();
        phases.sort_by_key(|p| std::cmp::Reverse(p.2));
        let ph: Vec<String> = phases
            .iter()
            .filter(|p| p.2.as_millis() >= 50)
            .map(|(n, _, c)| format!("{n} {:.2}", secs(c)))
            .collect();
        s += &format!(
            "  {:<24} {:>7.2} s  {}\n",
            t.key,
            secs(&t.total),
            ph.join(", ")
        );
        for n in &t.notes {
            s += &format!("  {:<24}            {n}\n", "");
        }
    }
    s
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// Bytes several jobs read.
struct Shared(Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A Halo 1 map shared by the jobs that convert it.
#[derive(Default)]
struct H1Entry {
    /// Its jobs not yet done.
    jobs: usize,
    map: Arc<OnceLock<Result<Arc<(content::Sha256, H1Map)>, String>>>,
    /// What a level's two variants share beyond the map (its BSPs).
    campaign: Arc<h1campaign::Shared>,
}

/// The real converter: maps from the Halo 1 disc, bases from the Halo 2
/// disc, written under the data root. Shared by the workers: what more
/// than one job reads (the scripts' tables, the donors, `ui.map`, the
/// Halo 2 bases) is read once, by the first job that needs it.
struct DiscConverter {
    h1_path: PathBuf,
    iso: Mutex<Xiso<BufReader<File>>>,
    disc: Disc,
    h2: PathBuf,
    data: PathBuf,
    scripting: OnceLock<Result<Option<h1campaign::Scripting>, String>>,
    donors: OnceLock<Result<h1campaign::donor::Donors, String>>,
    /// The disc's `ui.map` (the maps' cards).
    card_ui: OnceLock<Option<H1Map>>,
    /// The Halo 2 maps' bytes by name, each read once.
    bases: Mutex<BTreeMap<String, Arc<OnceLock<Result<Arc<Vec<u8>>, String>>>>>,
    /// The Halo 1 maps read, by name, while a job still to finish
    /// converts it (a level's two variants, a map's two sandboxes): each
    /// is read and inflated once.
    h1_maps: Mutex<BTreeMap<String, H1Entry>>,
    /// Held while a job writes: the packages' and the campaign's
    /// manifests are read, merged and written again.
    write: Mutex<()>,
}

impl DiscConverter {
    fn new(
        h1_path: PathBuf,
        iso: Xiso<BufReader<File>>,
        disc: Disc,
        h2: PathBuf,
        data: PathBuf,
    ) -> Self {
        DiscConverter {
            h1_path,
            iso: Mutex::new(iso),
            disc,
            h2,
            data,
            scripting: OnceLock::new(),
            donors: OnceLock::new(),
            card_ui: OnceLock::new(),
            bases: Mutex::new(BTreeMap::new()),
            h1_maps: Mutex::new(BTreeMap::new()),
            write: Mutex::new(()),
        }
    }

    /// Keeps each Halo 1 map `todo` converts once read until its last job
    /// there is done.
    fn expect(&self, todo: &[Job]) {
        let mut maps = self.h1_maps.lock().unwrap_or_else(|e| e.into_inner());
        for j in todo {
            maps.entry(j.source().to_owned()).or_default().jobs += 1;
        }
    }

    /// `job` is done with its Halo 1 map: let it go after the last.
    fn release(&self, job: &Job) {
        let mut maps = self.h1_maps.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = maps.get_mut(job.source()) {
            e.jobs = e.jobs.saturating_sub(1);
            if e.jobs == 0 {
                maps.remove(job.source());
            }
        }
    }

    /// A Halo 2 map from the Halo 2 disc, opened on the bytes the jobs
    /// share (a conversion reads its base, never writes it).
    fn h2_base(&self, name: &str) -> Result<map::Map<Cursor<Shared>>> {
        let bytes = self.h2_base_file(name)?;
        h1map::phase::time("open halo 2 base", || {
            map::Map::from_reader(Cursor::new(Shared(bytes)))
                .with_context(|| format!("reading {name}.map"))
        })
    }

    /// A Halo 2 base map's file, read once.
    fn h2_base_file(&self, name: &str) -> Result<Arc<Vec<u8>>> {
        let cell = self
            .bases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(name.to_owned())
            .or_default()
            .clone();
        let bytes = cell
            .get_or_init(|| {
                h1map::phase::time("read halo 2 base", || {
                    h1_cmd::read_h2_map(&self.h2, name)
                        .map(Arc::new)
                        .map_err(|e| format!("{e:#}"))
                })
            })
            .as_ref()
            .map_err(|e| anyhow!("{e}"))?;
        Ok(bytes.clone())
    }

    /// A multiplayer job's map, converted and built as the import writes
    /// it (fresh: nothing of the base pinned), with its preparation.
    fn multiplayer(
        &self,
        name: &str,
        sandbox: Sandbox,
    ) -> Result<(h1convert::convert::Prepared, h1convert::fresh::Fresh)> {
        let source = self.h1_map(name)?;
        let (sha, h1) = (source.0, &source.1);
        let base_name = known(name).map_or("beavercreek", |k| k.1);
        let mut base = self.h2_base(base_name)?;
        let p = h1map::phase::time("convert", || {
            h1_cmd::prepare_mp_with(h1, sha, &mut base, base_name, sandbox)
        })?;
        let mopp = h1map::phase::time("mopp", || structure_mopp(&p.collision))?;
        let f = h1map::phase::time("build map", || p.build_fresh(&mut base, &mopp))?;
        Ok((p, f))
    }

    /// A Halo 1 map off the disc, read once while [`Self::expect`]ed jobs
    /// still need it.
    fn h1_map(&self, name: &str) -> Result<Arc<(content::Sha256, H1Map)>> {
        let cell = self
            .h1_maps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .map(|e| e.map.clone());
        let Some(cell) = cell else {
            return Ok(Arc::new(self.read(name)?));
        };
        cell.get_or_init(|| self.read(name).map(Arc::new).map_err(|e| format!("{e:#}")))
            .clone()
            .map_err(|e| anyhow!("{e}"))
    }

    /// A Halo 1 map off the disc, inflated, and its file's SHA-256 (the
    /// file itself let go).
    fn read(&self, name: &str) -> Result<(content::Sha256, H1Map)> {
        let e = self
            .disc
            .map(name)
            .with_context(|| format!("no {name}.map on the disc"))?
            .clone();
        let bytes = h1map::phase::time("read halo 1 map", || {
            self.iso.lock().unwrap_or_else(|e| e.into_inner()).read(&e)
        })?;
        let m = h1map::phase::time("open halo 1 map", || {
            H1Map::from_bytes(&bytes).with_context(|| e.path.clone())
        })?;
        Ok((content::Sha256::of(&bytes), m))
    }

    fn card_ui(&self) -> Option<&H1Map> {
        self.card_ui
            .get_or_init(|| self.read("ui").ok().map(|(_, m)| m))
            .as_ref()
    }

    /// A campaign level converted as the import writes it, with `assets`.
    fn campaign(
        &self,
        level: &str,
        assets: h1campaign::h2assets::Assets,
    ) -> Result<h1campaign::Converted> {
        let source = self.h1_map(level)?;
        let h1 = &source.1;
        let scripting = self.scripting.get_or_init(|| {
            h1map::phase::time("scripts' tables", || {
                h1_cmd::campaign_scripting(&self.h1_path, &self.h2).map_err(|e| format!("{e:#}"))
            })
        });
        let scripting = match scripting {
            Ok(s) => s.as_ref(),
            Err(e) => bail!("the scripts' tables: {e}"),
        };
        let donors = self
            .donors
            .get_or_init(|| {
                h1map::phase::time("read donors", || {
                    h1_cmd::campaign_donors(&self.h2).map_err(|e| format!("{e:#}"))
                })
            })
            .as_ref()
            .map_err(|e| anyhow!("{e}"))?;
        let base_name = h1campaign::base_for(level);
        let base_file = self.h2_base_file(base_name)?;
        let mut base = self.h2_base(base_name)?;
        let shared = self
            .h1_maps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(level)
            .map(|e| e.campaign.clone());
        let opts = h1campaign::Options {
            assets,
            shared: shared.as_deref(),
            base_file: Some(&base_file),
            ..h1_cmd::campaign_options(scripting, Some(donors))
        };
        let done = h1map::phase::time("convert", || h1campaign::convert(h1, &mut base, &opts))?;
        for r in &done.report.retries {
            h1map::phase::note(|| format!("converted again: {r}"));
        }
        Ok(done)
    }

    fn out(&self, root: PathBuf) -> Result<OutDir> {
        // Ours to replace: whatever is there came from an import or a
        // convert of the same map.
        Ok(OutDir::new(root, true)?.protecting(&self.h1_path))
    }

    fn lock_write(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl workspace::BaseMaps for DiscConverter {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.h2_base_file(name)
            .map(|bytes| bytes.as_ref().clone())
            .map_err(|e| format!("{e:#}"))
    }
}

impl Converter for DiscConverter {
    fn convert(&self, job: &Job) -> Result<()> {
        // Its Halo 1 map let go however the job ends, a panic included.
        struct Release<'c, 'j>(&'c DiscConverter, &'j Job<'j>);
        impl Drop for Release<'_, '_> {
            fn drop(&mut self) {
                self.0.release(self.1);
            }
        }
        let _release = Release(self, job);
        self.convert_job(job)
    }
}

impl DiscConverter {
    fn store(&self, job: &Job, bytes: &[u8]) -> Result<()> {
        store_converted(
            &boot::store_status::root(&self.data),
            &job.stem(),
            bytes,
            self,
            "halo1",
        )
        .map_err(anyhow::Error::msg)?;
        Ok(())
    }

    fn convert_job(&self, job: &Job) -> Result<()> {
        match *job {
            Job::CampaignHalo2(level) => {
                let done = self.campaign(level, h1campaign::h2assets::Assets::Halo2)?;
                let _w = self.lock_write();
                self.store(job, &done.bytes)?;
            }
            Job::Campaign(level) => {
                let done = self.campaign(level, h1campaign::h2assets::Assets::Halo1)?;
                // Its picture for the level select (boot::campaign_cards).
                let card = self
                    .card_ui()
                    .and_then(|ui| h1convert::ui::campaign_card(ui, level).ok().flatten());
                let dir = campaign_dir(&self.data);
                let _w = self.lock_write();
                h1map::phase::time("write", || -> Result<()> {
                    h1_cmd::write_campaign_metadata(
                        &self.out(dir.clone())?,
                        &dir,
                        level,
                        &done.report.scenario.hud_swaps,
                        done.report.scenario.hud_sensor,
                    )?;
                    if let Some(c) = card {
                        let mut png = Vec::new();
                        assets::png::write_rgba(&mut png, c.width, c.height, &c.rgba)?;
                        let name = format!("{}.png", level.to_ascii_lowercase());
                        self.out(dir.clone())?
                            .write(&[boot::campaign_cards::CARDS_DIR.to_owned(), name], &png)?;
                    }
                    self.store(job, &done.bytes)
                })?;
            }
            Job::Multiplayer(name, sandbox) => {
                let (p, f) = self.multiplayer(name, sandbox)?;
                let done = h1convert::convert::Converted {
                    manifest: p.manifest_fresh(&f),
                    name: f.name,
                    bytes: f.bytes,
                };
                let card = self
                    .card_ui()
                    .and_then(|ui| h1convert::ui::mp_card(ui, name).ok().flatten());
                let root = halo1_metadata(&boot::store_status::root(&self.data));
                let _w = self.lock_write();
                h1map::phase::time("write", || {
                    h1_cmd::write_mp_metadata(
                        &self.out(root.clone())?,
                        &root,
                        sandbox,
                        &done,
                        card.as_ref(),
                    )?;
                    self.store(job, &done.bytes)
                })?;
            }
        }
        Ok(())
    }
}

/// What [`import`] is asked to do.
pub struct Request<'a> {
    pub h1_iso: &'a Path,
    pub h2_disc: &'a Path,
    pub data: &'a Path,
    /// Convert everything again, whatever the stamp says.
    pub force: bool,
    /// Say what would be converted; write nothing.
    pub dry_run: bool,
}

/// The import: opens both discs, works out what is missing, converts it.
/// `Err` only when nothing could start (a disc unreadable); a map that
/// fails is a line and a count.
pub fn import(r: &Request, say: &mut dyn FnMut(String)) -> Result<Summary> {
    crate::safe_out::protect(r.h1_iso);
    crate::safe_out::protect(r.h2_disc);
    let f = File::open(r.h1_iso).with_context(|| format!("opening {}", r.h1_iso.display()))?;
    let mut iso = Xiso::open(BufReader::new(f)).with_context(|| r.h1_iso.display().to_string())?;
    let disc = Disc::open(&mut iso)?;
    if !r.h2_disc.is_file() {
        bail!("no Halo 2 disc at {} (give --disc)", r.h2_disc.display());
    }
    let stems: Vec<String> = disc.maps.iter().map(|e| map_stem(e).to_owned()).collect();
    let all = jobs(&stems);
    if all.is_empty() {
        bail!(
            "{} has no Halo 1 campaign or multiplayer maps",
            r.h1_iso.display()
        );
    }
    let (size, mtime) = size_mtime(r.h1_iso)?;
    // With `--force` too: its outcomes are dropped below, but an
    // unchanged disc's hash is still good.
    let old = Stamp::read(r.data);
    if old
        .as_ref()
        .is_none_or(|o| o.size != size || o.mtime != mtime)
    {
        say(format!(
            "checking {} ({:.1} GB)",
            r.h1_iso.display(),
            size as f64 / 1e9
        ));
    }
    let mut stamp = fresh_stamp(old.as_ref(), size, mtime, || sha256_file(r.h1_iso))?;
    if r.force {
        stamp.done.clear();
    }
    let todo = pending(&all, &stamp, r.data);
    let skipped = all.len() - todo.len();
    if r.dry_run {
        for j in &todo {
            say(format!(
                "would convert {} to {}",
                j.label(),
                j.output(r.data).display()
            ));
        }
        return Ok(Summary {
            skipped,
            ..Summary::default()
        });
    }
    if todo.is_empty() {
        return Ok(Summary {
            skipped,
            ..Summary::default()
        });
    }
    say(format!(
        "converting {} of {} maps from {} into {} (converter version {CONVERTER_VERSION})",
        todo.len(),
        all.len(),
        r.h1_iso.display(),
        r.data.display()
    ));
    let conv = DiscConverter::new(
        r.h1_iso.to_path_buf(),
        iso,
        disc,
        r.h2_disc.to_path_buf(),
        r.data.to_path_buf(),
    );
    let data = r.data.to_path_buf();
    let workers = workers();
    let wall = Instant::now();
    let order = schedule(&todo, |m| conv.disc.map(m).map_or(0, |e| u64::from(e.size)));
    conv.expect(&order);
    let (mut sum, times) = run_jobs(
        &order,
        r.data,
        workers,
        &mut stamp,
        &conv,
        &mut |s| s.write(&data),
        say,
    );
    let report = timing_text(wall.elapsed(), workers, &times) + &usage_text();
    let path = campaign_dir(r.data).join(TIMING_FILE);
    match std::fs::write(&path, report) {
        Ok(()) => say(format!(
            "{:.1} s with {workers} workers; where the time went: {}",
            wall.elapsed().as_secs_f32(),
            path.display()
        )),
        Err(e) => say(format!("could not write {}: {e}", path.display())),
    }
    sum.skipped = skipped;
    Ok(sum)
}

// ---------------------------------------------------------------------------
// Automatic import before the game starts (`package::Halo1`).

/// Whether the automatic import runs: not with `--no-halo1`, and not with
/// `H2_HALO1` set to `0`, `false`, `no` or `off`.
pub fn auto_enabled(no_flag: bool) -> bool {
    auto_enabled_with(no_flag, std::env::var("H2_HALO1").ok().as_deref())
}

fn auto_enabled_with(no_flag: bool, env: Option<&str>) -> bool {
    let off = env.is_some_and(|v| {
        ["0", "false", "no", "off"].contains(&v.trim().to_ascii_lowercase().as_str())
    });
    !no_flag && !off
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Whether `path` is a Halo 1 disc: reads its directory and XBE header.
fn is_halo1(path: &Path) -> bool {
    let Ok(f) = File::open(path) else {
        return false;
    };
    let Ok(mut iso) = Xiso::open(BufReader::new(f)) else {
        return false;
    };
    Disc::open(&mut iso).is_ok()
}

/// The Halo 1 disc beside the game disc: `halo1.iso` (any case), else the
/// first other `.iso` (by name) that is a Halo 1 disc. The map pack's
/// `mappack.iso` is never opened.
pub fn discover(game_disc: &Path) -> Option<PathBuf> {
    let dir = match game_disc.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut isos: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
        .filter(|p| p.is_file() && !same_file(p, game_disc))
        .collect();
    isos.sort();
    let named = |p: &PathBuf, n: &str| p.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n));
    if let Some(i) = isos.iter().position(|p| named(p, AUTO_NAME)) {
        return Some(isos.swap_remove(i));
    }
    isos.into_iter()
        .filter(|p| !named(p, "mappack.iso"))
        .find(|p| is_halo1(p))
}

#[path = "h1_import_package.rs"]
pub mod package;

#[cfg(test)]
#[path = "h1_import_tests.rs"]
mod tests;
