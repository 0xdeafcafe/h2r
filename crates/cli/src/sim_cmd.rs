//! `h2 sim`: bot matches without rendering (docs/runtime/sim.md).
//!
//! - `run <match>`: one match in this process (`H2_SIM`), its report
//!   written at the end.
//! - `batch <batch>`: many matches, each a child `h2 sim run` with its own
//!   data directory, log, report and agent socket, a few at a time; then
//!   the A/B comparison (`compare.json`, `compare.txt`). With `--fork`, one
//!   seed run per map saves a snapshot at spawn and every run loads it,
//!   falling back to its own boot when the snapshot is refused.
//! - `compare <dir>`: the comparison of a batch's results again.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const USAGE: &str = "usage:
  h2 sim run <match.toml> [--disc ISO] [--out DIR] [--data DIR] [--report FILE]
                          [--socket PATH] [--tier auto|fast|clang] [--pace PACE]
                          [--upload [--as GAMERTAG] [--server URL]]
  h2 sim batch <batch.toml> [--dry-run] [--fork] [--parallel N] [--keep-data]
                          [--disc ISO] [--out DIR] [--data DIR] [--tier ...] [--pace PACE]
                          [--upload [--as GAMERTAG] [--server URL]]
  h2 sim compare <results dir>
  h2 sim audit-merge OUTPUT INPUT [INPUT ...]

A match file names the map, the game type, the limits, the seed and up to 16
bots (one to four local pads, remaining seats phantom peers) with their skill, team and tuning (docs/runtime/sim.md, \"Match file\").
`run` boots headless with no rendering, no keyboard and the match's bots,
and writes a JSON report (--report, default $TMPDIR/h2sim/<match>.report.json).
It never builds: build the image first with `h2 build` (same H2_OPT).
--socket turns the agent API on (match.status, match.report, match.stop).
--upload sends a private recording and report to Live's Clanker Training.
Choose a stored admin account with --as, or leave it off if there is only one.
--data defaults to h2's data root; its bots/ must hold the map's nav mesh
(`h2 bots nav <map>`): sim mode never builds one.

`batch` runs maps x game_types x seeds (x arms with mode = \"separate\") as child
processes, --parallel at once (default: half the cores), each with its own
data directory under <out>/runs/<id>/ (removed after unless --keep-data;
its bots/ linked to --data's), and writes <out>/compare.json and
compare.txt. With mode = \"seat_swap\" each map/seed plays twice, once with
each seat layout, and compare.json includes their paired differences.
--dry-run lists the runs. For all-local rosters, --fork boots each map once, saves a
snapshot at spawn (state.save), and has every run load it, or boot on its
own when the snapshot is refused (docs/runtime/sim.md, \"Forking from spawn\").";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("run") => run_one(it.collect()),
        Some("batch") => batch(it.collect()),
        Some("compare") => {
            let dir = it.next().context(USAGE)?;
            let c = compare_dir(Path::new(&dir))?;
            print!("{}", c.table());
            Ok(())
        }
        Some("audit-merge") => {
            let output = it.next().context(USAGE)?;
            let mut report = boot::host_audit::Report::default();
            for input in it {
                let file = File::open(&input).with_context(|| format!("opening {input}"))?;
                let run = serde_json::from_reader(std::io::BufReader::new(file))
                    .with_context(|| format!("reading {input}"))?;
                report.merge(run)?;
            }
            anyhow::ensure!(report.runs != 0, "no completed audit runs");
            report.write(Path::new(&output))
        }
        _ => bail!("{USAGE}"),
    }
}

/// What `run` and `batch` share: where the disc, the image and the data are.
#[derive(Clone, Debug, Default, PartialEq)]
struct Common {
    disc: Option<PathBuf>,
    out: Option<PathBuf>,
    data: Option<PathBuf>,
    tier: Option<String>,
    upload: bool,
    as_tag: Option<String>,
    server: Option<String>,
    pace: Option<sim::clock::Pace>,
}

impl Common {
    /// Takes `a` (and its value) if it is one of these.
    fn take(&mut self, a: &str, val: &mut dyn FnMut() -> Result<String>) -> Result<bool> {
        match a {
            "--disc" => self.disc = Some(PathBuf::from(val()?)),
            "--out" => self.out = Some(PathBuf::from(val()?)),
            "--data" => self.data = Some(PathBuf::from(val()?)),
            "--tier" => self.tier = Some(val()?),
            "--upload" => self.upload = true,
            "--as" => self.as_tag = Some(val()?),
            "--server" => self.server = Some(val()?),
            "--pace" => {
                self.pace = Some(sim::clock::Pace::parse(&val()?).map_err(anyhow::Error::msg)?)
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn disc(&self) -> Result<PathBuf> {
        self.disc
            .clone()
            .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
            .context("no disc: --disc or H2_DISC")
    }

    fn out(&self) -> PathBuf {
        self.out
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("h2boot"))
    }

    fn data(&self) -> PathBuf {
        self.data
            .clone()
            .unwrap_or_else(content::store::default_data_root)
    }

    /// The flags again, for a child.
    fn args(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (flag, p) in [("--disc", &self.disc), ("--out", &self.out)] {
            if let Some(p) = p {
                v.push(flag.to_string());
                v.push(p.display().to_string());
            }
        }
        if let Some(t) = &self.tier {
            v.push("--tier".into());
            v.push(t.clone());
        }
        if let Some(pace) = self.pace {
            v.extend(["--pace".into(), pace.label()]);
        }
        v
    }

    fn upload_account(&self) -> Result<Option<(String, boot::telemetry::TelemetryAccount)>> {
        if !self.upload {
            if self.as_tag.is_some() || self.server.is_some() {
                bail!("--as and --server require --upload");
            }
            return Ok(None);
        }
        let accounts = crate::live_accounts::load(&crate::live_accounts::accounts_path()?)?;
        let mut choices = accounts.iter().filter(|account| {
            self.as_tag
                .as_ref()
                .is_none_or(|tag| account.gamertag.eq_ignore_ascii_case(tag))
                && self.server.as_ref().is_none_or(|server| {
                    account.server.trim_end_matches('/') == server.trim_end_matches('/')
                })
        });
        let entry = choices
            .next()
            .context("no stored Live account matches the upload options")?;
        if choices.next().is_some() {
            bail!(
                "multiple stored Live accounts match; choose one with --as GAMERTAG and --server URL"
            );
        }
        let base = self.server.as_ref().unwrap_or(&entry.server);
        if !live_proto::bearer_allowed(base) {
            bail!("sim upload requires HTTPS or a loopback URL");
        }
        Ok(Some((
            base.to_owned(),
            boot::telemetry::TelemetryAccount {
                xuid: entry.xuid.to_string(),
                gamertag: entry.gamertag.clone(),
                key: entry.key_pair()?.private_jwk(),
            },
        )))
    }
}

const LIVE_SIM_SLOTS: usize = 2;
const BATCH_ID_ENV: &str = "H2_SIM_BATCH_ID";
const BATCH_LOCKS_ENV: &str = "H2_SIM_BATCH_LOCKS";

/// Run in the child so the OS releases the slot even if the sim crashes.
fn live_slot(dir: &Path) -> Result<Option<File>> {
    for slot in 0..LIVE_SIM_SLOTS {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(format!("live-{slot}.lock")))?;
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    Ok(None)
}

/// `h2 sim run`.
fn run_one(args: Vec<String>) -> Result<()> {
    let mut common = Common::default();
    let mut report = None;
    let mut socket = None;
    let mut file = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        if common.take(&a, &mut val)? {
            continue;
        }
        match a.as_str() {
            "--report" => report = Some(PathBuf::from(val()?)),
            "--socket" => socket = Some(PathBuf::from(val()?)),
            "-h" | "--help" => bail!("{USAGE}"),
            _ if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let file = file.context(USAGE)?;
    let file = std::path::absolute(&file)?;
    // Checked here, with every problem listed, before anything boots.
    let mut m =
        sim::config::load_match(&file).map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    if let Some(pace) = common.pace {
        m.pace = pace;
        m.config.pace = pace.label();
    }
    let account = common.upload_account()?;
    let report = match report {
        Some(r) => r,
        None => default_report(&file),
    };
    if let Some(dir) = report.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let disc = common.disc()?;
    let out = common.out();
    let data = common.data();
    let xbe = boot::read_xbe(&disc)?;
    use boot::tier::Choice;
    let choice = match &common.tier {
        Some(t) => Choice::parse(t)?,
        None if std::env::var_os("H2_TIER").is_none() => Choice::Clang,
        None => Choice::from_env()?,
    };
    let plan = boot::tier::plan(&xbe, &out, choice);
    if plan.build {
        bail!(
            "h2 sim: {} isn't built for this image and this h2; `h2 build{}` first (sim never builds)",
            plan.dylib.display(),
            if plan.tier == boot::tier::Tier::Fast {
                " --tier fast"
            } else {
                ""
            }
        );
    }
    let lib = boot::tier::prepare(&xbe, &disc, &out, choice, 1, false)?.dylib;
    let mut slot = None;
    let upload = if let Some((base_url, account)) = account {
        use boot::telemetry::{TelemetryConfig, TelemetrySinkConfig};
        let batch_id = std::env::var(BATCH_ID_ENV).ok();
        let deferred_lock = if let Some(dir) = std::env::var_os(BATCH_LOCKS_ENV).map(PathBuf::from)
        {
            slot = live_slot(&dir)?;
            slot.is_none().then(|| dir.join("upload.lock"))
        } else {
            None
        };
        let game = live_proto::game::SimGame {
            run_id: live_proto::sign::random_id(),
            batch_id,
            seed: m.config.seed.to_string(),
            gametype: m.game_type.name().into(),
            pace: m.pace.label(),
            bots: m.resolved_json()["bot"]
                .as_array()
                .context("resolved sim bots are missing")?
                .clone(),
            deferred: deferred_lock.is_some(),
        };
        eprintln!(
            "sim: Live run {} ({})",
            game.run_id,
            if game.deferred {
                "upload after match"
            } else {
                "stream live"
            }
        );
        Some(TelemetryConfig {
            enabled: true,
            sink: TelemetrySinkConfig::Simulation {
                base_url,
                account,
                game,
                deferred_lock,
            },
        })
    } else {
        None
    };
    // SAFETY: set before the run starts any thread; boot reads them once.
    unsafe {
        std::env::set_var("H2_BOTS_NAV_AUTO", "0");
        std::env::remove_var(bots::ENV);
        std::env::remove_var("H2_LEVEL");
        if let Some(s) = &socket {
            std::env::set_var(agent_proto::ENV, s);
        } else {
            std::env::remove_var(agent_proto::ENV);
        }
    }
    eprintln!(
        "h2 sim: {} on {} ({} bots, {}), report {}",
        file.display(),
        m.config.map,
        m.bots.len(),
        m.game_type.name(),
        report.display()
    );
    let matched = boot::sim_glue::Sim::new(file, m, Some(report.clone()));
    let r = boot::run_sim(
        &xbe,
        &lib,
        &boot::RunOpts {
            disc,
            data_root: data,
            max_slices: u64::MAX,
            transfers_per_slice: 50_000_000,
            stopgaps: true,
            trace: None,
            d3d: true,
            bink: true,
            dsound: true,
            xapi: true,
            movies: boot::movies_glue::Movies::Off,
        },
        matched,
        upload,
    );
    drop(slot);
    vfs::flush_pending_metadata();
    if let Some(s) = &socket {
        let _ = std::fs::remove_file(s);
    }
    let r = r?;
    println!("stop: {}", r.stop);
    if !report.exists() {
        bail!(
            "h2 sim: no report at {} (the run stopped before the match: {})",
            report.display(),
            r.stop
        );
    }
    println!("report: {}", report.display());
    if std::env::var_os(boot::host_audit::MODULE_ENV).is_some_and(|v| !v.is_empty()) {
        let report: sim::metrics::Report =
            serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(report)?))?;
        anyhow::ensure!(matches!(report.result.end, sim::metrics::End::TickLimit | sim::metrics::End::GameOver),
            "host module did not finish the match: {}", r.stop);
    }
    Ok(())
}

/// `$TMPDIR/h2sim/<match stem>.report.json`.
fn default_report(file: &Path) -> PathBuf {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("match");
    std::env::temp_dir()
        .join("h2sim")
        .join(format!("{}.report.json", sim::config::safe(stem)))
}

/// A short socket path for run `n` (Unix sockets allow about 100 bytes).
fn socket_for(n: usize) -> PathBuf {
    PathBuf::from(format!("/tmp/h2sim-{}-{n}.sock", std::process::id()))
}

/// The snapshot a map's runs fork from: `a-z 0-9 . _ -`, at most 64.
fn snapshot_name(map: &str, game_type: &str) -> String {
    let clean = |s: &str| -> String {
        s.to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let mut n = format!("sim.{}.{}.spawn", clean(map), clean(game_type));
    n.truncate(64);
    n
}

/// One run's files.
struct RunDir {
    dir: PathBuf,
}

impl RunDir {
    fn new(out: &Path, id: &str) -> RunDir {
        RunDir {
            dir: out.join("runs").join(id),
        }
    }
    fn matched(&self) -> PathBuf {
        self.dir.join("match.toml")
    }
    fn report(&self) -> PathBuf {
        self.dir.join("report.json")
    }
    fn audit(&self) -> PathBuf {
        self.dir.join("audit.json")
    }
    fn log(&self) -> PathBuf {
        self.dir.join("log.txt")
    }
    fn data(&self) -> PathBuf {
        self.dir.join("data")
    }
}

/// A run's own data directory, its `bots/` linked to the shared one.
fn make_data(run: &RunDir, shared: &Path) -> Result<()> {
    let data = run.data();
    std::fs::create_dir_all(&data)?;
    let bots = shared.join("bots");
    let link = data.join("bots");
    if bots.is_dir() && !link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&bots, &link)
            .with_context(|| format!("linking {}", link.display()))?;
    }
    Ok(())
}

/// Isolate the selected Map Pack offer with the rest of the run's data.
fn stage_map_pack(run: &RunDir, shared: &Path, map: &str) -> Result<()> {
    let Some((_, _, file)) = boot::map_bases::map_pack_file(shared, map) else {
        return Ok(());
    };
    let dir = file.parent().context("Map Pack map has no offer directory")?;
    let relative = dir.strip_prefix(shared)?;
    let target = run.data().join(relative);
    std::fs::create_dir_all(&target)?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// The child's arguments for one `h2 sim run`.
fn child_args(common: &Common, run: &RunDir, socket: Option<&Path>) -> Vec<String> {
    let mut v = vec![
        "sim".to_string(),
        "run".into(),
        run.matched().display().to_string(),
        "--report".into(),
        run.report().display().to_string(),
        "--data".into(),
        run.data().display().to_string(),
    ];
    v.extend(common.args());
    if let Some(s) = socket {
        v.push("--socket".into());
        v.push(s.display().to_string());
    }
    v
}

fn spawn(
    common: &Common,
    run: &RunDir,
    socket: Option<&Path>,
    snapshots: &Path,
    batch: Option<(&str, &Path)>,
    audit: bool,
) -> Result<Child> {
    let log = std::fs::File::create(run.log())?;
    let err = log.try_clone()?;
    let mut child = Command::new(std::env::current_exe()?);
    child
        .args(child_args(common, run, socket))
        .env_remove(BATCH_ID_ENV)
        .env_remove(BATCH_LOCKS_ENV)
        .env(snapshot::store::ENV, snapshots);
    child.env_remove(boot::host_audit::ENV);
    if audit {
        child.env(boot::host_audit::ENV, run.audit());
    }
    if let Some((id, locks)) = batch {
        child
            .arg("--upload")
            .env(BATCH_ID_ENV, id)
            .env(BATCH_LOCKS_ENV, locks);
        for (flag, value) in [("--as", &common.as_tag), ("--server", &common.server)] {
            if let Some(value) = value {
                child.args([flag, value]);
            }
        }
    }
    child
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .spawn()
        .context("starting h2 sim run")
}

/// Connects to a child's agent socket once it is up (or the child is gone).
fn connect(child: &mut Child, socket: &Path, within: Duration) -> Result<agent_proto::Client> {
    let t = Instant::now();
    loop {
        if let Ok(c) = agent_proto::Client::connect(socket) {
            c.set_timeout(Some(Duration::from_secs(300)))?;
            return Ok(c);
        }
        if child.try_wait()?.is_some() {
            bail!("the run exited before its agent socket was up");
        }
        if t.elapsed() > within {
            bail!("no agent socket at {} after {:?}", socket.display(), within);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Waits until `match.status` says the match plays, with at least `ticks`.
fn wait_playing(
    c: &mut agent_proto::Client,
    child: &mut Child,
    ticks: u64,
    within: Duration,
) -> Result<()> {
    let t = Instant::now();
    loop {
        let s = c.call(agent_proto::method::MATCH_STATUS, Value::Null)?;
        match s["phase"].as_str() {
            Some("playing") if s["ticks"].as_u64().unwrap_or(0) >= ticks => return Ok(()),
            Some("over") => bail!("the match ended before it could be forked ({})", s["end"]),
            _ => {}
        }
        if child.try_wait()?.is_some() {
            bail!("the run exited while waiting for the match");
        }
        if t.elapsed() > within {
            bail!("the match did not start within {within:?}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The seed run of `--fork`: boot the map's first run, wait for the match,
/// save it, stop. `Err`: no snapshot (the runs boot on their own).
fn seed_snapshot(
    common: &Common,
    run: &RunDir,
    name: &str,
    snapshots: &Path,
    n: usize,
) -> Result<()> {
    let socket = socket_for(n);
    let mut child = spawn(common, run, Some(&socket), snapshots, None, false)?;
    let r = (|| -> Result<()> {
        let mut c = connect(&mut child, &socket, Duration::from_secs(600))?;
        wait_playing(&mut c, &mut child, 30, Duration::from_secs(900))?;
        c.call(
            agent_proto::state::method::SAVE,
            json!({ "name": name, "replace": true, "max_ticks": 300 }),
        )
        .map_err(|e| anyhow::anyhow!("state.save: {e}"))?;
        let _ = c.call(agent_proto::method::MATCH_STOP, Value::Null);
        Ok(())
    })();
    if r.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = std::fs::remove_file(&socket);
    r
}

/// One batch run: the child, and with a snapshot, its load once the match
/// plays. A refused load is logged; the run goes on from its own boot.
fn run_child(
    common: &Common,
    run: &RunDir,
    fork: Option<&str>,
    snapshots: &Path,
    n: usize,
    batch: Option<(&str, &Path)>,
    audit: bool,
) -> Result<String> {
    let socket = fork.map(|_| socket_for(n));
    let mut child = spawn(common, run, socket.as_deref(), snapshots, batch, audit)?;
    let mut how = "boot".to_string();
    if let (Some(name), Some(socket)) = (fork, socket.as_deref()) {
        let loaded = (|| -> Result<()> {
            let mut c = connect(&mut child, socket, Duration::from_secs(600))?;
            wait_playing(&mut c, &mut child, 1, Duration::from_secs(900))?;
            c.call(
                agent_proto::state::method::LOAD,
                json!({ "name": name, "max_ticks": 300 }),
            )
            .map_err(|e| anyhow::anyhow!("state.load: {e}"))?;
            Ok(())
        })();
        how = match loaded {
            Ok(()) => format!("forked from {name}"),
            Err(e) => format!("own boot ({e:#})"),
        };
    }
    let status = child.wait()?;
    if let Some(s) = socket {
        let _ = std::fs::remove_file(s);
    }
    if !status.success() {
        bail!("sim child exited {status}; see {}", run.log().display());
    }
    Ok(how)
}

/// `h2 sim batch`.
fn batch(args: Vec<String>) -> Result<()> {
    let mut common = Common::default();
    let mut file = None;
    let mut dry = false;
    let mut fork = false;
    let mut keep = false;
    let mut parallel = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        if common.take(&a, &mut val)? {
            continue;
        }
        match a.as_str() {
            "--dry-run" => dry = true,
            "--fork" => fork = true,
            "--keep-data" => keep = true,
            "--parallel" => parallel = Some(val()?.parse::<usize>()?),
            "-h" | "--help" => bail!("{USAGE}"),
            _ if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let file = std::path::absolute(file.context(USAGE)?)?;
    let b =
        sim::config::load_batch(&file).map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    let fork = fork || b.config.fork;
    let audit = b.config.audit;
    if fork && boot::sim_glue::use_light_host()? {
        bail!("light audio snapshots are unavailable; set H2_FEATURES=-host-light for a full headless fork");
    }
    if audit && fork {
        bail!("audit requires a fresh boot per run");
    }
    if fork
        && b.runs.iter().any(|r| {
            r.config
                .bots
                .iter()
                .any(|b| b.kind == sim::config::SeatKind::Phantom)
        })
    {
        bail!(
            "distributed runs need a fresh System Link session per game; remove --fork or fork = true"
        );
    }
    let parallel = parallel
        .or(b.config.parallel)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)))
        .max(1);
    let arms: Vec<String> = b.arms.iter().map(|a| a.name.clone()).collect();
    println!(
        "{} runs, {} at a time, arms {}, into {}{}",
        b.runs.len(),
        parallel,
        arms.join(" vs "),
        b.out.display(),
        if fork { ", forked from spawn" } else { "" }
    );
    for r in &b.runs {
        let bots: Vec<String> = r
            .config
            .bots
            .iter()
            .map(|x| {
                format!(
                    "{}:{:?}:{}",
                    x.name.as_deref().unwrap_or("bot"),
                    x.kind,
                    x.arm.as_deref().unwrap_or("-")
                )
            })
            .collect();
        println!(
            "  {:<28} {} seed {} [{}]",
            r.id,
            r.map,
            r.seed,
            bots.join(" ")
        );
    }
    if dry {
        return Ok(());
    }
    let upload = common.upload_account()?.is_some();
    // Checked before the children start: a missing image fails once.
    let disc = common.disc()?;
    let xbe = boot::read_xbe(&disc)?;
    let choice = match &common.tier {
        Some(t) => boot::tier::Choice::parse(t)?,
        None => boot::tier::Choice::Clang,
    };
    if boot::tier::plan(&xbe, &common.out(), choice).build {
        bail!("h2 sim batch: the image isn't built; `h2 build` first (sim never builds)");
    }
    let shared = common.data();
    std::fs::create_dir_all(&b.out)?;
    let snapshots = b.out.join("snapshots");
    let batch_id = upload.then(live_proto::sign::random_id);
    let locks = b.out.join(".live-locks");
    if let Some(id) = &batch_id {
        std::fs::create_dir_all(&locks)?;
        println!("Live batch {id}");
    }
    let batch = batch_id.as_deref().map(|id| (id, locks.as_path()));
    std::fs::write(
        b.out.join("batch.json"),
        serde_json::to_string_pretty(&json!({
            "batch": file.display().to_string(),
            "live_batch_id": batch_id,
            "arms": arms,
            "mode": b.mode.name(),
            "fork": fork,
            "runs": b.runs.iter().map(|r| &r.id).collect::<Vec<_>>(),
        }))?,
    )?;
    for r in &b.runs {
        let rd = RunDir::new(&b.out, &r.id);
        std::fs::create_dir_all(&rd.dir)?;
        std::fs::write(rd.matched(), toml::to_string(&r.config)?)?;
        let _ = std::fs::remove_file(rd.report());
        let _ = std::fs::remove_file(rd.audit());
    }
    common.disc = Some(disc);
    // --fork: one seed run per map and game type, first.
    let mut forks = std::collections::BTreeMap::new();
    if fork {
        for (n, r) in b.runs.iter().enumerate() {
            let name = snapshot_name(&r.map, &r.config.game_type);
            if forks.contains_key(&name) {
                continue;
            }
            let seed = RunDir::new(&b.out, &format!("seed-{}", sim::config::safe(&name)));
            std::fs::create_dir_all(&seed.dir)?;
            std::fs::write(seed.matched(), toml::to_string(&r.config)?)?;
            make_data(&seed, &shared)?;
            stage_map_pack(&seed, &shared, &r.map)?;
            let got = seed_snapshot(&common, &seed, &name, &snapshots, 10_000 + n);
            match &got {
                Ok(()) => println!("fork: {name} saved"),
                Err(e) => println!("fork: {name}: {e:#}; its runs boot on their own"),
            }
            if !keep {
                let _ = std::fs::remove_dir_all(seed.data());
            }
            forks.insert(name, got.is_ok());
        }
    }
    let next = AtomicUsize::new(0);
    let lines = Mutex::new(Vec::new());
    let failed = AtomicUsize::new(0);
    let t = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..parallel.min(b.runs.len().max(1)) {
            s.spawn(|| {
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some(r) = b.runs.get(n) else { break };
                    let rd = RunDir::new(&b.out, &r.id);
                    let name = snapshot_name(&r.map, &r.config.game_type);
                    let from = (fork && forks.get(&name) == Some(&true)).then_some(name.as_str());
                    let how = make_data(&rd, &shared)
                        .and_then(|()| stage_map_pack(&rd, &shared, &r.map))
                        .and_then(|()| run_child(&common, &rd, from, &snapshots, n, batch, audit))
                        .unwrap_or_else(|e| {
                            failed.fetch_add(1, Ordering::Relaxed);
                            format!("failed: {e:#}")
                        });
                    if !keep {
                        let _ = std::fs::remove_dir_all(rd.data());
                    }
                    let got = if rd.report().exists() {
                        "report"
                    } else {
                        "NO REPORT"
                    };
                    let line = format!("{:<28} {got}  {how}", r.id);
                    println!("[{}/{}] {line}", n + 1, b.runs.len());
                    lines.lock().unwrap_or_else(|e| e.into_inner()).push(line);
                }
            });
        }
    });
    if audit {
        let mut merged = boot::host_audit::Report::default();
        let mut missing = Vec::new();
        for r in &b.runs {
            let run = RunDir::new(&b.out, &r.id);
            let complete = std::fs::File::open(run.report()).ok()
                .and_then(|f| serde_json::from_reader::<_, sim::metrics::Report>(std::io::BufReader::new(f)).ok())
                .is_some_and(|r| r.ticks > 0 && matches!(r.result.end, sim::metrics::End::TickLimit | sim::metrics::End::GameOver));
            if !complete {
                missing.push(&r.id);
                continue;
            }
            let path = run.audit();
            match std::fs::File::open(&path) {
                Ok(file) => merged.merge(serde_json::from_reader(std::io::BufReader::new(file))?)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(&r.id),
                Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
            }
        }
        merged.missing_runs.extend(missing.iter().map(|id| (*id).clone()));
        merged.write(&b.out.join("audit.json"))?;
        std::fs::write(
            b.out.join("audit-missing.json"),
            serde_json::to_vec_pretty(&missing)?,
        )?;
        std::fs::write(
            b.out.join("audit-equipment-gaps.json"),
            serde_json::to_vec_pretty(&merged.equipment_gaps())?,
        )?;
        println!("audit: {} runs, {} missing reports", merged.runs, missing.len());
    }
    let c = compare_dir(&b.out)?;
    let table = c.table();
    std::fs::write(
        b.out.join("compare.json"),
        serde_json::to_string_pretty(&c)?,
    )?;
    std::fs::write(b.out.join("compare.txt"), &table)?;
    println!(
        "\n{} runs in {:.0} s\n",
        b.runs.len(),
        t.elapsed().as_secs_f64()
    );
    print!("{table}");
    println!("\n{}", b.out.join("compare.json").display());
    if b.mode == sim::config::Mode::SeatSwap
        && c.seat_swap.as_ref().map_or(0, |s| s.pairs) * 2 != b.runs.len()
    {
        bail!("seat comparison is incomplete; see compare.json and the run logs");
    }
    if failed.load(Ordering::Relaxed) > 0 {
        bail!(
            "{} sim runs failed; see the run logs",
            failed.load(Ordering::Relaxed)
        );
    }
    Ok(())
}

/// Reads `<dir>/runs/*/report.json` and compares them, arms in
/// `batch.json`'s order when it is there.
fn compare_dir(dir: &Path) -> Result<sim::compare::Comparison> {
    let manifest = std::fs::read_to_string(dir.join("batch.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_default();
    let arms = serde_json::from_value::<Vec<String>>(manifest["arms"].clone()).unwrap_or_default();
    let runs = dir.join("runs");
    let mut ids: Vec<String> = match serde_json::from_value(manifest["runs"].clone()) {
        Ok(planned) => planned,
        Err(_) => std::fs::read_dir(&runs)
            .with_context(|| format!("reading {}", runs.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|id| !id.starts_with("seed-"))
            .collect(),
    };
    ids.sort();
    let mut reports = Vec::new();
    let mut missing = Vec::new();
    for id in ids {
        let path = runs.join(&id).join("report.json");
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<sim::metrics::Report>(&t).ok())
        {
            Some(r) => reports.push((id, r)),
            None => missing.push(id),
        }
    }
    Ok(sim::compare::compare(&arms, &reports, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names_are_store_names() {
        assert_eq!(
            snapshot_name("lockout", "slayer"),
            "sim.lockout.slayer.spawn"
        );
        assert_eq!(snapshot_name("My Map", "CTF"), "sim.my_map.ctf.spawn");
        assert!(snapshot_name(&"x".repeat(100), "slayer").len() <= 64);
    }

    #[test]
    fn sockets_are_short() {
        assert!(socket_for(99_999).display().to_string().len() < 100);
    }

    #[test]
    fn a_child_gets_its_own_files() {
        let common = Common {
            disc: Some("/d/halo2.iso".into()),
            out: Some("/o".into()),
            data: Some("/shared".into()),
            tier: Some("fast".into()),
            ..Default::default()
        };
        let rd = RunDir::new(Path::new("/res"), "lockout-s1");
        let a = child_args(&common, &rd, Some(Path::new("/tmp/s.sock")));
        assert_eq!(
            a,
            [
                "sim",
                "run",
                "/res/runs/lockout-s1/match.toml",
                "--report",
                "/res/runs/lockout-s1/report.json",
                "--data",
                "/res/runs/lockout-s1/data",
                "--disc",
                "/d/halo2.iso",
                "--out",
                "/o",
                "--tier",
                "fast",
                "--socket",
                "/tmp/s.sock"
            ]
        );
        // The shared data directory is never the child's.
        assert!(!a.iter().any(|x| x == "/shared"));
    }

    #[test]
    fn the_report_defaults_under_tmp() {
        let r = default_report(Path::new("/x/lockout 2v2.toml"));
        assert!(
            r.ends_with("h2sim/lockout_2v2.report.json"),
            "{}",
            r.display()
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-sim-cmd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn report(arm_a: &str, arm_b: &str, kills_b: i32) -> sim::metrics::Report {
        let p = |index: u8, arm: &str, kills: i32| sim::metrics::PlayerReport {
            index,
            name: format!("b{index}"),
            bot: true,
            port: Some(index),
            arm: Some(arm.into()),
            kills,
            ..Default::default()
        };
        sim::metrics::Report {
            format: sim::metrics::FORMAT.into(),
            match_: Value::Null,
            result: sim::metrics::MatchResult {
                end: sim::metrics::End::TickLimit,
                winner: Some(sim::metrics::Winner::Player(1)),
                teams: vec![],
            },
            ticks: 10,
            game_seconds: 0.3,
            wall: Default::default(),
            players: vec![p(0, arm_a, 1), p(1, arm_b, kills_b)],
            kills: vec![],
            nav: "loaded".into(),
            notes: vec![],
        }
    }

    #[test]
    fn sim_upload_caps_the_live_slots_and_reuses_them() {
        let dir = scratch("live-slots");
        let first = live_slot(&dir).unwrap().unwrap();
        let second = live_slot(&dir).unwrap().unwrap();
        assert!(live_slot(&dir).unwrap().is_none());
        drop(first);
        assert!(live_slot(&dir).unwrap().is_some());
        drop(second);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compare_reads_a_results_directory() {
        let dir = scratch("compare");
        std::fs::write(dir.join("batch.json"), r#"{"arms": ["base", "aggr"]}"#).unwrap();
        for (id, k) in [("lockout-s1", 3), ("lockout-s2", 5)] {
            let d = dir.join("runs").join(id);
            std::fs::create_dir_all(&d).unwrap();
            let text = serde_json::to_string(&report("base", "aggr", k)).unwrap();
            std::fs::write(d.join("report.json"), text).unwrap();
        }
        // A run with no report, and a seed run, which is not compared.
        std::fs::create_dir_all(dir.join("runs/lockout-s3")).unwrap();
        std::fs::create_dir_all(dir.join("runs/seed-sim.lockout.slayer.spawn")).unwrap();
        let c = compare_dir(&dir).unwrap();
        assert_eq!(c.runs, 2);
        assert_eq!(c.arms, ["base", "aggr"]);
        assert_eq!(c.missing, ["lockout-s3"]);
        assert_eq!(c.diff["kills"].diff, 3.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_data_links_the_shared_nav_meshes() {
        let dir = scratch("data");
        let shared = dir.join("shared");
        std::fs::create_dir_all(shared.join("bots")).unwrap();
        let rd = RunDir::new(&dir, "r1");
        make_data(&rd, &shared).unwrap();
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(rd.data().join("bots")).unwrap(),
            shared.join("bots")
        );
        // Again: no error for the link already there.
        make_data(&rd, &shared).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
