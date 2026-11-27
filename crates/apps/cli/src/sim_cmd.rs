//! `h2 sim`: bot matches without rendering (docs/runtime/sim.md).
//!
//! - `run <match>`: one match in this process (`H2_SIM`), its report
//!   written at the end.
//! - `batch <batch>`: many matches on local child processes or dedicated
//!   hosts, a few at a time, then the A/B comparison (`compare.json`,
//!   `compare.txt`).
//! - `compare <dir>`: the comparison of a batch's results again.
//! - `sweep <sweep>`: a batch per candidate bounce tuning, ranked
//!   (`sweep.json`, `sweep.csv`, `sweep.md`).
//! - `harvest <dir>`: the proven spots and ghost runs a superbounce
//!   batch's runs kept, merged into the shared bots directory.
//! - `drops <file>`: seam drops, the targets split over match processes,
//!   then the pooled report (`drops.json`, `drops.md`).
//! - `bounces <file>`: the forked bounce search, the sites split over
//!   match processes, then the ranked report (`bounces.json`,
//!   `bounces.md`, `bounces.verified`).
//! - `diff <a.json> <b.json>`: the first differing state of two reports.
//! - `spatial <trace or dir>...`: the measures of spatial traces
//!   (`sim_bench::spatial::measure`).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

mod cache_template;
mod cloud;

const USAGE: &str = "usage:
  h2 sim run <match.toml> [--disc ISO] [--out DIR] [--data DIR] [--report FILE]
                          [--socket PATH] [--tier auto|bridge|fast|clang] [--pace PACE]
                          [--upload [--as GAMERTAG] [--server URL]]
  h2 sim store build <map|all> --disc ISO --out DIR --reads FILE
  h2 sim batch <batch.toml> [--provider local] [--dry-run] [--keep-data]
                          [--max-parallel N | --parallel N] [--budget-usd X]
                          [--disc ISO] [--out DIR] [--data DIR] [--tier ...] [--pace PACE]
                          [--upload [--as GAMERTAG] [--server URL]]
  h2 sim batch <batch.toml> --provider cloud [--live URL] [--token T] [--as GAMERTAG]
                          [--max-parallel N] [--budget-usd X] [--dry-run]
  h2 sim compare <results dir>
  h2 sim sweep <sweep.toml> [--dry-run] [batch flags: --parallel N --disc ISO --out DIR ...]
  h2 sim harvest <results dir> [--data DIR] [--dry-run]
  h2 sim train <train.toml> [--map NAME]... [--budget N] [--status | --dry-run]
                          [--reopen] [--no-merge] [--keep-data] [--parallel N]
                          [--disc ISO] [--out DIR] [--data DIR] [--tier ...] [--pace PACE]
  h2 sim drops <drops.toml> [--parallel N] [--dry-run] [--merge] [--keep-data]
                          [--disc ISO] [--out DIR] [--data DIR] [--tier ...] [--pace PACE]
  h2 sim bounces <bounces.toml> [--parallel N] [--dry-run] [--merge] [--keep-data]
                          [--disc ISO] [--out DIR] [--data DIR] [--tier ...] [--pace PACE]
  h2 sim diff <report a.json> <report b.json> [--sound]
  h2 sim spatial <spatial.bin | run dir | results dir>...
  h2 sim audit-merge OUTPUT INPUT [INPUT ...]
  h2 sim audit-coverage OUTPUT AUDIT [AUDIT ...]

A match file names the map, the game type, the limits, the seed and up to 15
phantom bots with their skill, team and tuning. The host occupies the sixteenth
peer slot (docs/runtime/sim.md, \"Match file\").
`run` boots headless with no rendering, no keyboard and the match's bots,
and writes a JSON report (--report, default $TMPDIR/h2sim/<match>.report.json).
It never builds: build the image first with `h2 build` (same H2_OPT).
--socket turns the agent API on (match.status, match.report, match.stop).
--upload sends a private recording and report to Live's Clanker Training.
Choose a stored admin account with --as, or leave it off if there is only one.
--data defaults to h2's data root; its bots/ must hold the map's nav mesh
(`h2 bots nav <map>`): sim mode never builds one.

`batch` runs maps x game_types x skills x seeds (x arms with mode = \"separate\") as
child processes, --parallel at once (default: half the cores), each with its own
data directory under <out>/runs/<id>/ (removed after unless --keep-data;
its bots/ linked to --data's), and writes <out>/compare.json and
compare.txt. With mode = \"arm_swap\" each plays twice with the arms exchanged, so both
arms play every seat. skills = [...] adds a run per skill with every bot
at it, and a table per skill.
--provider local is the default. --max-parallel also works for local batches.
--provider cloud runs unpaced on dedicated hosts through Live.
--dry-run lists the runs.
A run whose match has [rules] script = \"superbounce\" plays on its own copy of each map's
verified and ghosts files, kept in <out>/runs/<id>/harvest/{start,end}
(docs/runtime/sim.md, \"Bounce practice\").

`sweep` plays the sweep file's batch once per candidate bounce tuning, each in
<out>/candidates/<id>/, and ranks them by bounce rate, then launch height
(docs/runtime/sim.md, \"Sweeping the bounce tuning\"). Flags after the file
go to each `h2 sim batch`.
`harvest` merges what each run of a results directory proved and the ghost
runs it kept into --data's bots/ (--dry-run: what it would add).
`train` plays the train file's match as `shards` runs a map, each searching the
bounce knobs of its shard's sites on its own copy of the map's .learned, then
merges the runs' files into --data's bots/ (--no-merge: left in the runs'
harvest/end for `harvest`) and writes <out>/train.json and train.md
(docs/runtime/sim.md, \"Training the bounce sites\"). --budget and --map go over
the file's values; --status prints the table from the running runs' files;
--dry-run lists the runs and each shard's sites; --reopen first turns the
dead and blocked sites back to open.
`drops` drops every seat of a match file with a [drops] table onto points of
candidate collision edges (targets = \"scan\": every bounce candidate of the map's
.spots in --data's bots/), the pair word held at -3, after a control drop on a
known seam; the targets are split over --parallel match processes (default: half
the cores), each on its own copy of the map's verified file. It writes
<out>/drops.json and drops.md, and exits with an error if a run's control did not
bounce. --merge merges the proven targets into --data's bots/ (else `h2 sim
harvest <out>`); --dry-run lists the runs (docs/runtime/sim.md, \"Seam drops\").";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("run") => run_one(it.collect()),
        Some("host-run") => run_host_child(it.collect()),
        Some("store") => crate::sim_store_cmd::run(it.collect()),
        Some("batch") => batch(it.collect()),
        Some("sweep") => sweep(it.collect()),
        Some("harvest") => harvest(it.collect()),
        Some("train") => train(it.collect()),
        Some("drops") => drops(it.collect()),
        Some("bounces") => bounces(it.collect()),
        Some("compare") => {
            let dir = it.next().context(USAGE)?;
            let c = compare_dir(Path::new(&dir))?;
            print!("{}", c.table());
            Ok(())
        }
        Some("spatial") => spatial(it.collect()),
        Some("diff") => {
            let a = it.next().context(USAGE)?;
            let b = it.next().context(USAGE)?;
            let option = it.next();
            anyhow::ensure!(option.as_deref().is_none_or(|s| s == "--sound") && it.next().is_none(), "{USAGE}");
            let read = |path: &str| -> Result<sim_bench::metrics::Report> {
                let file = File::open(path).with_context(|| format!("opening {path}"))?;
                serde_json::from_reader(std::io::BufReader::new(file))
                    .with_context(|| format!("reading {path}"))
            };
            let (a, b) = (read(&a)?, read(&b)?);
            if option.is_some() {
                let result = sim_bench::diff::sound_diff(&a, &b).map_err(anyhow::Error::msg)?;
                if !matches!(result, sim_bench::diff::Verdict::Identical { .. }) {
                    bail!("{}", result.render().trim_end());
                }
            } else {
                print!("{}", sim_bench::diff::diff(&a, &b).render());
            }
            Ok(())
        }
        Some("audit-coverage") => {
            let output = it.next().context(USAGE)?;
            let mut report = boot::host_audit::Report::default();
            for input in it {
                let file = File::open(&input).with_context(|| format!("opening {input}"))?;
                let run: boot::host_audit::Report =
                    serde_json::from_reader(std::io::BufReader::new(file))
                        .with_context(|| format!("reading {input}"))?;
                run.coverage().with_context(|| format!("checking {input}"))?;
                report.merge(run)?;
            }
            report.coverage()?.write(Path::new(&output))
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
    pace: Option<sim_bench::clock::Pace>,
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
                self.pace = Some(sim_bench::clock::Pace::parse(&val()?).map_err(anyhow::Error::msg)?)
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

/// What `h2 sim run` sets for the bot think mode (`bots::think::Mode`),
/// as the values of `H2_BOT_THREADS` and `H2_BOT_PIPELINE`; `None` leaves
/// the variable alone. Per variable, in order: the environment as it is
/// (`env_*`, `Some` when set at all), the match's `bot_threads` and
/// `bot_pipeline`, then the default, serial: on the quiet laptop four
/// phantoms think 9% slower batched than serial in one match
/// (docs/perf/notes/sim-10x.md), and batched pads equal serial pads
/// (docs/game/bots.md §5.5).
fn think_env(
    config: &sim_bench::config::MatchConfig,
    env_threads: Option<&str>,
    env_pipeline: Option<&str>,
) -> (Option<String>, Option<String>) {
    let threads = if env_threads.is_some() {
        None
    } else {
        config.bot_threads.as_ref().map(|t| t.env_value())
    };
    let pipeline = if env_pipeline.is_some() {
        None
    } else {
        config
            .bot_pipeline
            .map(|on| if on { "1" } else { "0" }.to_string())
    };
    (threads, pipeline)
}

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

fn plan_sim_module(xbe: &[u8], out: &Path, choice: boot::tier::Choice) -> Result<boot::tier::Plan> {
    Ok(if boot::sim_glue::use_light_host()? {
        boot::tier::plan_light(xbe, out, choice)
    } else {
        boot::tier::plan(xbe, out, choice)
    })
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
    let mut m = sim_bench::config::load_match(&file)
        .map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
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
    let plan = plan_sim_module(&xbe, &out, choice)?;
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
    let slim = plan.tier == boot::tier::Tier::Host;
    let lib = plan.dylib;
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
        let game = sim_game(
            &m,
            live_proto::sign::random_id(),
            batch_id,
            deferred_lock.is_some(),
        )?;
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
        // Bot map data read on the game thread (game::bot_glue::MAP_INLINE_ENV).
        std::env::set_var("H2_BOTS_MAP_INLINE", "1");
        // Bot think mode (bots::think): explicit variables win.
        let (threads, pipeline) = think_env(
            &m.config,
            std::env::var(bots::think::THREADS_ENV).ok().as_deref(),
            std::env::var(bots::think::PIPELINE_ENV).ok().as_deref(),
        );
        if let Some(v) = threads {
            std::env::set_var(bots::think::THREADS_ENV, v);
        }
        if let Some(v) = pipeline {
            std::env::set_var(bots::think::PIPELINE_ENV, v);
        }
        std::env::remove_var("H2_LEVEL");
        if let Some(s) = &socket {
            std::env::set_var(agent_proto::ENV, s);
        } else {
            std::env::remove_var(agent_proto::ENV);
        }
    }
    eprintln!(
        "h2 sim: bot thinking: H2_BOT_THREADS={} H2_BOT_PIPELINE={}",
        std::env::var(bots::think::THREADS_ENV).unwrap_or_else(|_| "(serial)".into()),
        std::env::var(bots::think::PIPELINE_ENV).unwrap_or_else(|_| "(off)".into())
    );
    eprintln!(
        "h2 sim: {} on {} ({} bots, {}), report {}",
        file.display(),
        m.config.map,
        m.bots.len(),
        m.game_type_name(),
        report.display()
    );
    // A Firefight kit package named as the map is staged (a built-in kit)
    // or required (any other) on the data directory the run uses; one that
    // is not there stops the run, named.
    #[cfg(feature = "desktop")]
    crate::firefight_kits::ensure_map(&disc, &data, &m.config.map)?;
    let matched = boot::sim_glue::Sim::new(file, m, Some(report.clone()));
    let r = boot::run_sim(&xbe, &lib, &sim_options(disc, data), matched, upload);
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
    if slim || std::env::var_os(boot::host_audit::MODULE_ENV).is_some_and(|v| !v.is_empty()) {
        let report: sim_bench::metrics::Report =
            serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(report)?))?;
        anyhow::ensure!(matches!(report.result.end, sim_bench::metrics::End::TickLimit | sim_bench::metrics::End::GameOver),
            "host module did not finish the match: {}", r.stop);
    }
    Ok(())
}

fn sim_game(
    matched: &sim_bench::config::Match,
    run_id: String,
    batch_id: Option<String>,
    deferred: bool,
) -> Result<live_proto::game::SimGame> {
    Ok(live_proto::game::SimGame {
        run_id,
        batch_id,
        seed: matched.config.seed.to_string(),
        gametype: matched.game_type_name().into(),
        pace: matched.pace.label(),
        bots: matched.resolved_json()["bot"]
            .as_array()
            .context("resolved sim bots are missing")?
            .clone(),
        deferred,
    })
}

/// Internal child entry point of `h2 host run`. The supervisor provisions
/// the data and supplies the lease and credential in the child's environment.
fn run_host_child(args: Vec<String>) -> Result<()> {
    let [disc, image, data]: [String; 3] = args
        .try_into()
        .map_err(|_| anyhow::anyhow!("the sim host child needs its disc, image and data paths"))?;
    let live = std::env::var("H2_HOST_LIVE_URL").context("missing host Live URL")?;
    let token = std::env::var("H2_HOST_TOKEN").context("missing host credential")?;
    let lease: live_proto::hosts::HostLease = serde_json::from_str(
        &std::env::var("H2_HOST_LEASE_JSON").context("missing sim host lease")?,
    )
    .context("decoding sim host lease")?;
    let job = cloud::host_match(&live, &token, &lease)?;
    let matched = sim_bench::config::resolve(job.matched, &|_| {
        Err("a hosted sim requires inline bot tuning".into())
    })
    .map_err(anyhow::Error::msg)?;
    if matched.pace != sim_bench::clock::Pace::Unpaced
        || !matched.config.map.eq_ignore_ascii_case(&lease.map)
        || !matched.bots.iter().map(|bot| &bot.name).eq(&lease.bots)
    {
        bail!("the sim match does not match its unpaced host lease");
    }
    let game = sim_game(&matched, job.run_id, Some(job.batch_id), false)?;
    let upload = boot::telemetry::TelemetryConfig::for_host(live, token, Some(game))
        .map_err(anyhow::Error::msg)?;
    let disc = PathBuf::from(disc);
    let data_root = PathBuf::from(data);
    let xbe = boot::read_xbe(&disc)?;
    let module = boot::tier::clang_dylib(Path::new(&image));
    std::fs::create_dir_all(&data_root)?;
    let path = data_root.join("sim-match.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&matched.config)?)?;
    let report = data_root.join("sim-report.json");
    let sim = boot::sim_glue::Sim::new(path, matched, Some(report));
    let result = boot::run_sim(&xbe, &module, &sim_options(disc, data_root), sim, Some(upload));
    vfs::flush_pending_metadata();
    result?;
    Ok(())
}

fn sim_options(disc: PathBuf, data_root: PathBuf) -> boot::RunOpts {
    boot::RunOpts {
        disc,
        data_root,
        max_slices: u64::MAX,
        transfers_per_slice: 50_000_000,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        xapi: true,
        movies: boot::movies_glue::Movies::Off,
    }
}

/// `h2 sim spatial`: the measures of the traces named, each a
/// `spatial.bin`, a run's directory or a batch's results directory
/// (every `runs/*/spatial.bin`), together (docs/runtime/sim.md, "The
/// spatial trace").
fn spatial(args: Vec<String>) -> Result<()> {
    anyhow::ensure!(!args.is_empty() && !args.iter().any(|a| a.starts_with('-')), "{USAGE}");
    let mut files = Vec::new();
    for a in &args {
        files.extend(spatial_files(Path::new(a))?);
    }
    anyhow::ensure!(!files.is_empty(), "no spatial trace under {}", args.join(", "));
    let traces = files
        .iter()
        .map(|f| sim_bench::spatial::Trace::read(f).map_err(anyhow::Error::msg))
        .collect::<Result<Vec<_>>>()?;
    print!("{}", sim_bench::spatial::measure(&traces).render());
    Ok(())
}

/// The trace files `path` names: itself, its `spatial.bin`, or its
/// `runs/*/spatial.bin`.
fn spatial_files(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    let own = path.join(sim_bench::spatial::FILE_NAME);
    if own.is_file() {
        return Ok(vec![own]);
    }
    let runs = path.join("runs");
    let mut found = Vec::new();
    if runs.is_dir() {
        for entry in std::fs::read_dir(&runs).with_context(|| format!("reading {}", runs.display()))? {
            let f = entry?.path().join(sim_bench::spatial::FILE_NAME);
            if f.is_file() {
                found.push(f);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// `$TMPDIR/h2sim/<match stem>.report.json`.
fn default_report(file: &Path) -> PathBuf {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("match");
    std::env::temp_dir()
        .join("h2sim")
        .join(format!("{}.report.json", sim_bench::config::safe(stem)))
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
    /// A superbounce run's verified and ghosts files as it began.
    fn harvest_start(&self) -> PathBuf {
        self.dir.join("harvest").join("start")
    }
    /// ... and as it ended.
    fn harvest_end(&self) -> PathBuf {
        self.dir.join("harvest").join("end")
    }
}

/// The files a bounce run keeps of its own: verified, ghosts and (a
/// training run's) learned. A run writes its `.learned` on its own copy
/// whatever `bots::harvest::run_owned` says.
fn bounce_owned(name: &str) -> bool {
    bots::harvest::run_owned(name) || name.ends_with(".learned")
}

/// A superbounce run's data: like [`make_data`]'s, but its `bots/` is a
/// directory of its own, each shared file linked but the maps' verified
/// and ghosts files, which are copied in and to `harvest/start`. The run
/// proves spots and keeps ghost runs on its copy only: no run sees
/// another's, and the runs of a sweep start alike.
fn make_bounce_data(run: &RunDir, shared: &Path) -> Result<()> {
    let own = run.data().join("bots");
    let start = run.harvest_start();
    let _ = std::fs::remove_dir_all(run.dir.join("harvest"));
    // Afresh: what a kept run left (links and copies only; a link is
    // removed, not followed).
    if own
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        std::fs::remove_file(&own)?;
    } else {
        let _ = std::fs::remove_dir_all(&own);
    }
    std::fs::create_dir_all(&own)?;
    std::fs::create_dir_all(&start)?;
    let from = shared.join("bots");
    let entries = match std::fs::read_dir(&from) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", from.display())),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let target = own.join(&name);
        let owned = name.to_str().is_some_and(bounce_owned);
        if owned && entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("copying {}", entry.path().display()))?;
            std::fs::copy(entry.path(), start.join(&name))?;
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(entry.path(), &target)
                .with_context(|| format!("linking {}", target.display()))?;
        }
    }
    Ok(())
}

/// Keeps a superbounce run's verified and ghosts files in
/// `harvest/end`, before its data goes.
fn keep_bounce_files(run: &RunDir) -> Result<usize> {
    let end = run.harvest_end();
    std::fs::create_dir_all(&end)?;
    let own = run.data().join("bots");
    let mut kept = 0;
    let Ok(entries) = std::fs::read_dir(&own) else {
        return Ok(0);
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_some_and(bounce_owned) && entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), end.join(&name))?;
            kept += 1;
        }
    }
    Ok(kept)
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
fn child_args(common: &Common, run: &RunDir) -> Vec<String> {
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
    v
}

fn spawn(
    common: &Common,
    run: &RunDir,
    batch: Option<(&str, &Path)>,
    audit: bool,
    cache_seed: &[PathBuf],
) -> Result<Child> {
    let log = std::fs::File::create(run.log())?;
    let err = log.try_clone()?;
    let mut child = Command::new(std::env::current_exe()?);
    child
        .args(child_args(common, run))
        .env_remove(BATCH_ID_ENV)
        .env_remove(BATCH_LOCKS_ENV);
    child.env_remove(boot::host_audit::ENV);
    // N:'s slots from the template, cloned in after the child's boot formats
    // N: (docs/runtime/cache-slots.md, "Sim batches: a warm N:").
    child.env_remove(vfs::CACHE_SEED_ENV);
    if !cache_seed.is_empty() {
        child.env(vfs::CACHE_SEED_ENV, std::env::join_paths(cache_seed)?);
    }
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

/// `h2 sim batch`.
fn batch(args: Vec<String>) -> Result<()> {
    let mut common = Common::default();
    let mut file = None;
    let mut dry = false;
    let mut keep = false;
    let mut parallel = None;
    let mut max_parallel = None;
    let mut cloud_provider = false;
    let mut live = None;
    let mut token = None;
    let mut budget = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        if common.take(&a, &mut val)? {
            continue;
        }
        match a.as_str() {
            "--dry-run" => dry = true,
            "--fork" => {
                bail!("phantom bots require a fresh System Link session per run. Remove --fork")
            }
            "--keep-data" => keep = true,
            "--parallel" => parallel = Some(val()?.parse::<usize>()?),
            "--max-parallel" => {
                let value = val()?.parse::<std::num::NonZeroUsize>()?;
                max_parallel = Some(value.get());
            }
            "--provider" => {
                cloud_provider = match val()?.as_str() {
                    "local" => false,
                    "cloud" => true,
                    _ => bail!("--provider expects local or cloud"),
                };
            }
            "--live" => live = Some(val()?),
            "--token" => token = Some(val()?),
            "--budget-usd" => {
                let usd = val()?.parse::<f64>()?;
                budget = Some(sim_bench::batch::budget_microusd(usd).map_err(anyhow::Error::msg)?);
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let file = std::path::absolute(file.context(USAGE)?)?;
    let mut b = sim_bench::config::load_batch(&file)
        .map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    let audit = b.config.audit;
    if cloud_provider {
        if audit {
            bail!("audit requires local sim batches");
        }
        if keep
            || common.disc.is_some()
            || common.out.is_some()
            || common.data.is_some()
            || common.tier.is_some()
            || common.upload
            || common.server.is_some()
        {
            bail!("cloud batches use Live's host data and upload automatically; remove local-only options");
        }
        if common.pace.is_some_and(|pace| pace != sim_bench::clock::Pace::Unpaced) {
            bail!("cloud batches run unpaced");
        }
        for r in &mut b.runs {
            r.config.pace = sim_bench::clock::Pace::Unpaced.label();
        }
    } else if live.is_some() || token.is_some() {
        bail!("--live and --token require --provider cloud");
    }
    let parallel = max_parallel
        .or(parallel)
        .or(b.config.parallel)
        .unwrap_or_else(|| {
            if cloud_provider {
                sim_bench::batch::DEFAULT_MAX_PARALLEL
            } else {
                std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1))
            }
        })
        .max(1);
    let arms: Vec<String> = b.arms.iter().map(|a| a.name.clone()).collect();
    println!(
        "{} runs, {} at a time, arms {}, into {}",
        b.runs.len(),
        parallel,
        arms.join(" vs "),
        b.out.display(),
    );
    for r in &b.runs {
        let bots: Vec<String> = r
            .config
            .bots
            .iter()
            .map(|x| {
                format!(
                    "{}:{}",
                    x.name.as_deref().unwrap_or("bot"),
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
    if cloud_provider {
        let client = cloud::Client::open(live, token, common.as_tag.as_deref())?;
        prepare_output(&b, &file, &arms, Some(client.batch_id()))?;
        println!("Live batch {}", client.batch_id());
        let started = Instant::now();
        let failed = client.run(&b, parallel, budget);
        return finish_batch(&b, failed, started);
    }
    let upload = common.upload_account()?.is_some();
    let plan = check_image(&mut common, "batch")?;
    anyhow::ensure!(!audit || plan.tier != boot::tier::Tier::Host,
        "sim batch audit requires a full generated module");
    std::fs::create_dir_all(&b.out)?;
    let batch_id = upload.then(live_proto::sign::random_id);
    let locks = b.out.join(".live-locks");
    if let Some(id) = &batch_id {
        std::fs::create_dir_all(&locks)?;
        println!("Live batch {id}");
    }
    let batch = batch_id.as_deref().map(|id| (id, locks.as_path()));
    prepare_output(&b, &file, &arms, batch_id.as_deref())?;
    let disc = common.disc()?;
    let t = Instant::now();
    let failed = play_runs(
        &common,
        &b,
        (&plan.dylib, &disc),
        batch,
        audit,
        keep,
        parallel,
    );
    if audit {
        let mut merged = boot::host_audit::Report::default();
        let mut missing = Vec::new();
        for r in &b.runs {
            let run = RunDir::new(&b.out, &r.id);
            let complete = std::fs::File::open(run.report()).ok()
                .and_then(|f| serde_json::from_reader::<_, sim_bench::metrics::Report>(std::io::BufReader::new(f)).ok())
                .is_some_and(|r| r.ticks > 0 && matches!(r.result.end, sim_bench::metrics::End::TickLimit | sim_bench::metrics::End::GameOver));
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
    finish_batch(&b, failed, t)
}

/// Checks the image is built, before any child starts (a missing image
/// fails once), and fixes the disc in `common` for the children.
fn check_image(common: &mut Common, what: &str) -> Result<boot::tier::Plan> {
    let disc = common.disc()?;
    let xbe = boot::read_xbe(&disc)?;
    let choice = match &common.tier {
        Some(t) => boot::tier::Choice::parse(t)?,
        None => boot::tier::Choice::Clang,
    };
    let plan = plan_sim_module(&xbe, &common.out(), choice)?;
    if plan.build {
        bail!("h2 sim {what}: the image isn't built; `h2 build` first (sim never builds)");
    }
    common.disc = Some(disc);
    Ok(plan)
}

/// Plays a batch's runs, `parallel` at a time, each on its own data
/// directory (a bounce run on its own proofs, kept in `harvest/end`), Z:'s
/// map cache cloned from the template of the image `(dylib, disc)`.
/// How many failed.
fn play_runs(
    common: &Common,
    b: &sim_bench::config::Batch,
    (dylib, disc): (&Path, &Path),
    batch: Option<(&str, &Path)>,
    audit: bool,
    keep: bool,
    parallel: usize,
) -> usize {
    let shared = common.data();
    // Z:'s and N:'s map cache from the image's template
    // (docs/runtime/cache-slots.md, "Sim batches: a warm Z:" and "Sim
    // batches: a warm N:").
    let template = cache_template::Template::for_batch(&common.out(), dylib, disc);
    let gate = cache_template::Gate::default();
    let next = AtomicUsize::new(0);
    let lines = Mutex::new(Vec::new());
    let failed = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..parallel.min(b.runs.len().max(1)) {
            s.spawn(|| {
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some(r) = b.runs.get(n) else { break };
                    let rd = RunDir::new(&b.out, &r.id);
                    let bounce =
                        r.config.plays_superbounce()
                            || r.config.drops.is_some()
                            || r.config.bounces.is_some();
                    let data = if bounce {
                        make_bounce_data(&rd, &shared)
                    } else {
                        make_data(&rd, &shared)
                    };
                    let mut cache = String::new();
                    let result = data
                        .and_then(|()| stage_map_pack(&rd, &shared, &r.map))
                        .and_then(|()| {
                            let start =
                                cache_template::start(template.as_ref(), &gate, &rd.data(), &r.map);
                            cache = start.label();
                            let status = spawn(common, &rd, batch, audit, start.seed())?.wait()?;
                            if let (Some(parts), Some(t)) = (start.saves(), &template)
                                && status.success()
                            {
                                save_template(t, &rd.data(), parts);
                            }
                            // Dropping the start lets runs waiting for the
                            // template go.
                            drop(start);
                            if !status.success() {
                                bail!("sim child exited {status}. See {}", rd.log().display());
                            }
                            Ok(())
                        });
                    let how = match result {
                        Ok(()) => "boot".into(),
                        Err(e) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            format!("failed: {e:#}")
                        }
                    };
                    if bounce && let Err(e) = keep_bounce_files(&rd) {
                        eprintln!("{}: keeping the bounce files: {e:#}", r.id);
                    }
                    if !keep {
                        let _ = std::fs::remove_dir_all(rd.data());
                    }
                    let got = if rd.report().exists() {
                        "report"
                    } else {
                        "NO REPORT"
                    };
                    let line = format!("{:<28} {got}  {how}{cache}", r.id);
                    println!("[{}/{}] {line}", n + 1, b.runs.len());
                    lines.lock().unwrap_or_else(|e| e.into_inner()).push(line);
                }
            });
        }
    });
    failed.load(Ordering::Relaxed)
}

/// `h2 sim sweep`: the sweep file's batch once per candidate, then the
/// ranking (docs/runtime/sim.md, "Sweeping the bounce tuning").
fn sweep(args: Vec<String>) -> Result<()> {
    use sim_bench::sweep::{self as sw, Method};
    let mut file = None;
    let mut dry = false;
    let mut pass = Vec::new();
    for a in args {
        match a.as_str() {
            "--dry-run" => dry = true,
            "-h" | "--help" => bail!("{USAGE}"),
            _ if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            _ => pass.push(a),
        }
    }
    let file = std::path::absolute(file.context(USAGE)?)?;
    let s = sw::load_sweep(&file).map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    let checked = sim_bench::config::load_batch(&s.batch)
        .map_err(|e| anyhow::anyhow!("{}:\n{e}", s.batch.display()))?;
    anyhow::ensure!(
        !checked.runs.is_empty()
            && checked
                .runs
                .iter()
                .all(|r| r.config.plays_superbounce()),
        "{}: a sweep's batch plays the bounce practice: its match needs [rules] script = \"superbounce\"",
        s.batch.display()
    );
    let orig = sw::read_batch_config(&s.batch).map_err(anyhow::Error::msg)?;
    let orig_dir = s.batch.parent().unwrap_or(Path::new(".")).to_path_buf();
    std::fs::create_dir_all(&s.out)?;
    println!(
        "sweep {} ({}), {} runs a candidate, into {}",
        file.display(),
        s.method.name(),
        checked.runs.len(),
        s.out.display()
    );
    let mut scored: Vec<sw::Scored> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut played: Vec<sw::Candidate> = Vec::new();
    let play = |c: &sw::Candidate,
                scored: &mut Vec<sw::Scored>,
                missing: &mut Vec<String>|
     -> Result<()> {
        let dir = s.out.join("candidates").join(&c.id);
        std::fs::create_dir_all(&dir)?;
        let batch_file = dir.join("batch.toml");
        std::fs::write(&batch_file, toml::to_string(&sw::candidate_batch(&orig, &orig_dir, c))?)?;
        let key = if c.key.is_empty() { "built-in" } else { &c.key };
        println!("\n{} {key}", c.id);
        if dry {
            return Ok(());
        }
        let status = Command::new(std::env::current_exe()?)
            .args([
                "sim".to_string(),
                "batch".into(),
                batch_file.display().to_string(),
            ])
            .args(&pass)
            .stdin(Stdio::null())
            .status()
            .context("starting h2 sim batch")?;
        if !status.success() {
            eprintln!(
                "{}: the batch exited {status}; scoring the runs it finished",
                c.id
            );
        }
        match compare_dir(&dir).ok().and_then(|cmp| sw::score(c, &cmp)) {
            Some(sc) => scored.push(sc),
            None => missing.push(c.id.clone()),
        }
        Ok(())
    };
    match s.method {
        Method::Grid | Method::Random => {
            let cands = if s.method == Method::Grid {
                s.grid()
            } else {
                s.random()
            };
            for c in &cands {
                play(c, &mut scored, &mut missing)?;
            }
        }
        Method::Coordinate => {
            let base = s.base_candidate();
            play(&base, &mut scored, &mut missing)?;
            played.push(base);
            let mut best = s.base.clone();
            for round in 0..s.rounds() {
                for axis in 0..s.axes.len() {
                    let cands = sw::fresh(s.step(&best, axis, 0), &played, played.len());
                    for c in &cands {
                        play(c, &mut scored, &mut missing)?;
                        played.push(c.clone());
                    }
                    if let Some(lead) = sw::leader(&scored, s.min_attempts) {
                        best = lead.bounce;
                    }
                }
                if dry {
                    println!("\n(--dry-run: later steps start from the best, unknown yet)");
                    break;
                }
                println!("\nround {} done: best so far {}", round + 1, sw::key(&best));
            }
        }
    }
    if dry {
        return Ok(());
    }
    let result = sw::SweepResult::new(s.method, s.min_attempts, scored, missing);
    std::fs::write(
        s.out.join("sweep.json"),
        serde_json::to_string_pretty(&result)?,
    )?;
    std::fs::write(s.out.join("sweep.csv"), sw::csv(&result))?;
    let md = sw::markdown(&result, 20);
    std::fs::write(s.out.join("sweep.md"), &md)?;
    print!("\n{md}");
    println!("\n{}", s.out.join("sweep.json").display());
    Ok(())
}

/// What `h2 sim train` was asked.
#[derive(Clone, Debug, Default, PartialEq)]
struct TrainArgs {
    file: Option<PathBuf>,
    common: Common,
    over: sim_bench::train::Overrides,
    status: bool,
    dry: bool,
    reopen: bool,
    no_merge: bool,
    keep: bool,
    parallel: Option<usize>,
}

fn parse_train_args(args: Vec<String>) -> Result<TrainArgs> {
    let mut t = TrainArgs::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        if t.common.take(&a, &mut val)? {
            continue;
        }
        match a.as_str() {
            "--map" => t.over.maps.push(val()?),
            "--budget" => t.over.budget = Some(val()?.parse::<u32>().context("--budget: a count")?),
            "--status" => t.status = true,
            "--dry-run" => t.dry = true,
            "--reopen" => t.reopen = true,
            "--no-merge" => t.no_merge = true,
            "--keep-data" => t.keep = true,
            "--parallel" | "--max-parallel" => {
                t.parallel = Some(val()?.parse::<std::num::NonZeroUsize>()?.get())
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ if t.file.is_none() && !a.starts_with('-') => t.file = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    anyhow::ensure!(t.file.is_some(), "{USAGE}");
    anyhow::ensure!(
        !(t.status && (t.dry || t.reopen || t.no_merge)),
        "--status only reads: it takes no --dry-run, --reopen or --no-merge"
    );
    Ok(t)
}

/// The text of `dir`'s `<map>.learned`, if there is one (the file's name
/// is the map's, any case).
fn learned_in(dir: &Path, map: &str) -> Option<(PathBuf, String)> {
    let safe = sim_bench::config::safe(map);
    for e in std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let path = e.path();
        let name = path.file_name()?.to_str()?;
        let Some(stem) = name.strip_suffix(".learned") else {
            continue;
        };
        if stem.eq_ignore_ascii_case(map) || stem.eq_ignore_ascii_case(&safe) {
            return Some((path.clone(), std::fs::read_to_string(&path).ok()?));
        }
    }
    None
}

/// A run's site lines as they stand: its own data while it plays, what it
/// kept when it ended, what it began with, or the shared file before it
/// began. Only the sites of the run's shard.
fn run_rows(
    rd: &RunDir,
    run: &sim_bench::train::TrainRun,
    shards: u32,
    shared: &Path,
) -> Vec<sim_bench::train::SiteRow> {
    let dirs = [
        rd.data().join("bots"),
        rd.harvest_end(),
        rd.harvest_start(),
        shared.join("bots"),
    ];
    let Some((path, text)) = dirs.iter().find_map(|d| learned_in(d, &run.map)) else {
        return Vec::new();
    };
    match sim_bench::train::parse_rows(&text) {
        Ok(rows) => sim_bench::train::shard_rows(&rows, run.shard, shards, &|stand| {
            bots::learn::spot_id(&run.map, stand)
        }),
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            Vec::new()
        }
    }
}

/// The batch's report from the runs' files.
fn train_report(plan: &sim_bench::train::Plan, shared: &Path) -> sim_bench::train::TrainReport {
    let maps = plan
        .maps()
        .into_iter()
        .map(|map| {
            let rows: Vec<_> = plan
                .runs
                .iter()
                .filter(|r| r.map == map)
                .flat_map(|r| {
                    run_rows(&RunDir::new(&plan.batch.out, &r.id), r, plan.shards, shared)
                })
                .collect();
            (map, rows)
        })
        .collect();
    sim_bench::train::report(maps)
}

/// The bounce sites a map's scan found, as a dry run lists them.
fn scanned_sites(shared: &Path, map: &str) -> Option<Vec<[f32; 3]>> {
    let bytes = std::fs::read(bots::cache::dir(shared).join(bots::spots::file_name(map))).ok()?;
    let (spots, _) = bots::spots::from_bytes(&bytes)?;
    Some(
        spots
            .iter()
            .filter(|s| bots::spots::Site::from_spot(s).is_some())
            .map(|s| s.stand)
            .collect(),
    )
}

/// `--reopen`: the dead and blocked lines of each map's shared
/// `.learned` back to open. How many lines.
fn reopen_learned(shared: &Path, maps: &[String]) -> Result<usize> {
    let mut total = 0;
    for map in maps {
        let Some((path, text)) = learned_in(&bots::cache::dir(shared), map) else {
            continue;
        };
        let (text, n) = sim_bench::train::reopen(&text);
        if n > 0 {
            let tmp = path.with_extension("learned.tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path)?;
            total += n;
        }
    }
    Ok(total)
}

/// `h2 sim train`: the train file's runs, then the report
/// (docs/runtime/sim.md, "Training the bounce sites").
fn train(args: Vec<String>) -> Result<()> {
    let mut t = parse_train_args(args)?;
    let file = std::path::absolute(t.file.take().context(USAGE)?)?;
    let plan = sim_bench::train::load(&file, &t.over)
        .map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    let shared = t.common.data();
    let out = &plan.batch.out;
    if t.status {
        print!(
            "{}",
            sim_bench::train::markdown(&train_report(&plan, &shared))
        );
        return Ok(());
    }
    let parallel = t
        .parallel
        .or(plan.batch.config.parallel)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)))
        .max(1);
    println!(
        "{} runs ({} shards a map), budget {} tries a site, seed {}, {} at a time, into {}",
        plan.runs.len(),
        plan.shards,
        plan.budget,
        plan.seed,
        parallel,
        out.display(),
    );
    for r in &plan.runs {
        println!("  {:<28} {} shard {}/{}", r.id, r.map, r.shard, plan.shards);
    }
    if t.dry {
        for map in plan.maps() {
            match scanned_sites(&shared, &map) {
                None => println!(
                    "\n{map}: no spots file in {} yet",
                    bots::cache::dir(&shared).display()
                ),
                Some(sites) => {
                    println!("\n{map}: {} bounce sites", sites.len());
                    for shard in 0..plan.shards {
                        let mine: Vec<_> = sites
                            .iter()
                            .filter(|s| {
                                sim_bench::train::owns(
                                    bots::learn::spot_id(&map, **s),
                                    shard,
                                    plan.shards,
                                )
                            })
                            .collect();
                        println!("  shard {shard}: {} sites", mine.len());
                        for s in mine {
                            println!("    ({:.3}, {:.3}, {:.3})", s[0], s[1], s[2]);
                        }
                    }
                }
            }
        }
        return Ok(());
    }
    // Checked before anything is written: a missing image fails once.
    let image = check_image(&mut t.common, "train")?;
    let disc = t.common.disc()?;
    if t.reopen {
        let n = reopen_learned(&shared, &plan.maps())?;
        println!(
            "reopened {n} dead and blocked sites in {}",
            bots::cache::dir(&shared).display()
        );
    }
    prepare_output(&plan.batch, &file, &[], None)?;
    let started = Instant::now();
    let failed = play_runs(
        &t.common,
        &plan.batch,
        (&image.dylib, &disc),
        None,
        false,
        t.keep,
        parallel,
    );
    let report = train_report(&plan, &shared);
    std::fs::write(
        out.join("train.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    let md = sim_bench::train::markdown(&report);
    std::fs::write(out.join("train.md"), &md)?;
    println!(
        "\n{} runs in {:.0} s\n",
        plan.runs.len(),
        started.elapsed().as_secs_f64()
    );
    print!("{md}");
    let kept: Vec<PathBuf> = plan
        .runs
        .iter()
        .map(|r| RunDir::new(out, &r.id).dir)
        .filter(|d| d.join("harvest").join("end").is_dir())
        .collect();
    if t.no_merge {
        println!(
            "\nnot merged (--no-merge): `h2 sim harvest {}`",
            out.display()
        );
    } else if kept.is_empty() {
        println!("\nno run kept its files: nothing to merge");
    } else {
        println!();
        harvest_into(&kept, &bots::cache::dir(&shared), true)?;
    }
    println!("\n{}", out.join("train.json").display());
    if failed > 0 {
        bail!("{failed} sim runs failed; see the run logs");
    }
    Ok(())
}

/// What `h2 sim drops` was asked.
#[derive(Clone, Debug, Default, PartialEq)]
struct DropsArgs {
    file: Option<PathBuf>,
    common: Common,
    dry: bool,
    merge: bool,
    keep: bool,
    parallel: Option<usize>,
}

fn parse_drops_args(args: Vec<String>) -> Result<DropsArgs> {
    let mut d = DropsArgs::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        if d.common.take(&a, &mut val)? {
            continue;
        }
        match a.as_str() {
            "--dry-run" => d.dry = true,
            "--merge" => d.merge = true,
            "--keep-data" => d.keep = true,
            "--parallel" | "--max-parallel" => {
                d.parallel = Some(val()?.parse::<std::num::NonZeroUsize>()?.get())
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ if d.file.is_none() && !a.starts_with('-') => d.file = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    anyhow::ensure!(d.file.is_some(), "{USAGE}");
    Ok(d)
}

/// The bounce candidates of `map`'s scan (`<map>.spots` in the shared
/// bots directory) as explicit targets, in the scan's order (best first),
/// at most `max`.
fn scan_targets(
    shared: &Path,
    map: &str,
    max: Option<u32>,
) -> Result<Vec<sim_bench::drops::TargetSpec>> {
    let path = bots::cache::dir(shared).join(bots::spots::file_name(map));
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "{}: no spot scan for {map}; `h2 bots nav {map}` writes it",
            path.display()
        )
    })?;
    let (spots, _) = bots::spots::from_bytes(&bytes)
        .with_context(|| format!("{}: not a spots file this build reads", path.display()))?;
    Ok(spots
        .iter()
        .filter(|s| bots::spots::Site::from_spot(s).is_some())
        .take(max.map_or(usize::MAX, |m| m as usize))
        .enumerate()
        .map(|(i, s)| sim_bench::drops::TargetSpec {
            id: Some(format!("s{i:03}-{}", s.shape.name())),
            shape: Some(s.shape.name().into()),
            edge: Some(s.edge),
            point: None,
            drift: (s.drift != [0.0, 0.0]).then_some(s.drift),
            stand: Some(s.stand),
        })
        .collect())
}

/// `h2 sim drops`: the drops file's targets split over match processes,
/// then the pooled report (docs/runtime/sim.md, "Seam drops").
fn drops(args: Vec<String>) -> Result<()> {
    use sim_bench::drops::{Target, Targets, drops_per_target, shard_targets};
    let mut a = parse_drops_args(args)?;
    let file = std::path::absolute(a.file.take().context(USAGE)?)?;
    let m = sim_bench::config::load_match(&file)
        .map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    let Some(table) = m.config.drops.clone() else {
        bail!("{}: no [drops] table", file.display());
    };
    let map = m.config.map.clone();
    let settings = table
        .settings(&map)
        .map_err(|p| anyhow::anyhow!("{}:\n{}", file.display(), p.join("\n")))?;
    let shared = a.common.data();
    let specs = match &table.targets {
        Targets::List(l) => l.clone(),
        Targets::Word(_) => scan_targets(&shared, &map, table.max_targets)?,
    };
    anyhow::ensure!(!specs.is_empty(), "{map}: no bounce targets to drop on");
    let resolved = specs
        .iter()
        .enumerate()
        .map(|(i, t)| Target::from_spec(i, t))
        .collect::<std::result::Result<Vec<Target>, String>>()
        .map_err(anyhow::Error::msg)?;
    let weights: Vec<usize> = resolved
        .iter()
        .map(|t| drops_per_target(t, &settings))
        .collect();
    let parallel = a
        .parallel
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)))
        .max(1);
    let shards = shard_targets(&specs, &weights, parallel);
    let n = shards.len();
    let base = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("drops");
    let out = match &table.out {
        Some(o) => base.join(o),
        None => base.join("results").join(sim_bench::config::safe(stem)),
    };
    // Each run's match: the file's, its bots' tunings inline, its share
    // of the targets explicit.
    let bots: Vec<sim_bench::config::BotSpec> = m
        .bots
        .iter()
        .map(|b| sim_bench::config::BotSpec {
            name: Some(b.name.clone()),
            skill: Some(b.skill.name().into()),
            team: b.team,
            tuning: Some(b.params.clone()),
            ..sim_bench::config::BotSpec::default()
        })
        .collect();
    let control = settings.control.as_ref().map_or(0, |c| c.heights.len());
    let mut runs = Vec::new();
    let mut counts = Vec::new();
    for (i, targets) in shards.into_iter().enumerate() {
        let mut config = m.config.clone();
        config.bots = bots.clone();
        let count: usize = targets
            .iter()
            .filter_map(|t| specs.iter().position(|s| s == t))
            .map(|k| weights[k])
            .sum();
        counts.push((targets.len(), count + control));
        if let Some(d) = config.drops.as_mut() {
            d.targets = Targets::List(targets);
            d.shard = i as u32;
            d.shards = n as u32;
            d.max_targets = None;
            d.out = None;
            d.map = None;
        }
        runs.push(sim_bench::config::Run {
            id: format!("{}-drops{i}", sim_bench::config::safe(&map)),
            map: map.clone(),
            seed: config.seed,
            config,
        });
    }
    let batch = sim_bench::config::Batch {
        config: sim_bench::config::BatchConfig {
            out: out.clone(),
            match_file: file.clone(),
            ..sim_bench::config::BatchConfig::default()
        },
        out: out.clone(),
        mode: sim_bench::config::Mode::default(),
        arms: Vec::new(),
        runs,
    };
    println!(
        "{} targets on {map}, {} drops ({} a run for the control), {} runs of {} seats, into {}",
        specs.len(),
        weights.iter().sum::<usize>() + control * n,
        control,
        n,
        settings.seats,
        out.display()
    );
    for (r, (t, c)) in batch.runs.iter().zip(&counts) {
        println!("  {:<28} {t} targets, {c} drops", r.id);
    }
    if a.dry {
        return Ok(());
    }
    let image = check_image(&mut a.common, "drops")?;
    let disc = a.common.disc()?;
    prepare_output(&batch, &file, &[], None)?;
    let started = Instant::now();
    let failed = play_runs(
        &a.common,
        &batch,
        (&image.dylib, &disc),
        None,
        false,
        a.keep,
        n,
    );
    let mut blocks = Vec::new();
    let mut missing = Vec::new();
    for r in &batch.runs {
        let path = RunDir::new(&out, &r.id).report();
        let block = File::open(&path)
            .ok()
            .and_then(|f| {
                serde_json::from_reader::<_, sim_bench::metrics::Report>(std::io::BufReader::new(f))
                    .ok()
            })
            .and_then(|rep| rep.drops);
        match block {
            Some(b) => blocks.push(b),
            None => missing.push(r.id.clone()),
        }
    }
    let mut report = sim_bench::drops::summarize(&blocks);
    if report.map.is_empty() {
        report.map = map.clone();
    }
    for id in &missing {
        report.notes.push(format!("{id}: no report with a drops block"));
    }
    std::fs::write(out.join("drops.json"), serde_json::to_string_pretty(&report)?)?;
    let md = sim_bench::drops::markdown(&report);
    std::fs::write(out.join("drops.md"), &md)?;
    println!(
        "\n{} runs in {:.0} s\n",
        batch.runs.len(),
        started.elapsed().as_secs_f64()
    );
    print!("{md}");
    let kept: Vec<PathBuf> = batch
        .runs
        .iter()
        .map(|r| RunDir::new(&out, &r.id).dir)
        .filter(|d| d.join("harvest").join("end").is_dir())
        .collect();
    if a.merge && !kept.is_empty() {
        println!();
        harvest_into(&kept, &bots::cache::dir(&shared), true)?;
    } else if !kept.is_empty() {
        println!(
            "\nthe proven targets are in each run's verified file: `h2 sim harvest {}`",
            out.display()
        );
    }
    println!("\n{}", out.join("drops.json").display());
    if !report.errors.is_empty() {
        bail!("{}", report.errors.join("\n"));
    }
    if failed > 0 {
        bail!("{failed} sim runs failed; see the run logs");
    }
    Ok(())
}

/// `h2 sim bounces`: the search file's sites split over match processes,
/// then the ranked report (docs/runtime/bounce-search.md).
fn bounces(args: Vec<String>) -> Result<()> {
    use sim_bench::bounces::Site;
    use sim_bench::drops::{Target, Targets, shard_targets};
    let mut a = parse_drops_args(args)?;
    let file = std::path::absolute(a.file.take().context(USAGE)?)?;
    let m = sim_bench::config::load_match(&file)
        .map_err(|e| anyhow::anyhow!("{}:\n{e}", file.display()))?;
    let Some(table) = m.config.bounces.clone() else {
        bail!("{}: no [bounces] table", file.display());
    };
    let map = m.config.map.clone();
    let settings = table
        .settings()
        .map_err(|p| anyhow::anyhow!("{}:\n{}", file.display(), p.join("\n")))?;
    let shared = a.common.data();
    let specs = match &table.targets {
        Targets::List(l) => l.clone(),
        Targets::Word(_) => scan_targets(&shared, &map, table.max_targets)?,
    };
    anyhow::ensure!(!specs.is_empty(), "{map}: no bounce targets to search");
    let resolved = specs
        .iter()
        .enumerate()
        .map(|(i, t)| Target::from_spec(i, t))
        .collect::<std::result::Result<Vec<Target>, String>>()
        .map_err(anyhow::Error::msg)?;
    let per_site = settings.trials_per_site();
    // A target's weight: its sites' trials.
    let weights: Vec<usize> = resolved
        .iter()
        .map(|t| Site::of(std::slice::from_ref(t), settings.along).len() * per_site)
        .collect();
    let sites: usize = weights.iter().map(|w| w / per_site.max(1)).sum();
    let parallel = a
        .parallel
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)))
        .max(1);
    let shards = shard_targets(&specs, &weights, parallel);
    let n = shards.len();
    let base = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("bounces");
    let out = match &table.out {
        Some(o) => base.join(o),
        None => base.join("results").join(sim_bench::config::safe(stem)),
    };
    // Each run's match: the file's, its bots' tunings inline, its share
    // of the targets explicit.
    let bots: Vec<sim_bench::config::BotSpec> = m
        .bots
        .iter()
        .map(|b| sim_bench::config::BotSpec {
            name: Some(b.name.clone()),
            skill: Some(b.skill.name().into()),
            team: b.team,
            tuning: Some(b.params.clone()),
            ..sim_bench::config::BotSpec::default()
        })
        .collect();
    let mut runs = Vec::new();
    let mut counts = Vec::new();
    for (i, targets) in shards.into_iter().enumerate() {
        let mut config = m.config.clone();
        config.bots = bots.clone();
        let count: usize = targets
            .iter()
            .filter_map(|t| specs.iter().position(|s| s == t))
            .map(|k| weights[k])
            .sum();
        counts.push((targets.len(), count));
        if let Some(b) = config.bounces.as_mut() {
            b.targets = Targets::List(targets);
            b.shard = i as u32;
            b.shards = n as u32;
            b.max_targets = None;
            b.out = None;
            b.map = None;
        }
        runs.push(sim_bench::config::Run {
            id: format!("{}-bounces{i}", sim_bench::config::safe(&map)),
            map: map.clone(),
            seed: config.seed,
            config,
        });
    }
    let batch = sim_bench::config::Batch {
        config: sim_bench::config::BatchConfig {
            out: out.clone(),
            match_file: file.clone(),
            ..sim_bench::config::BatchConfig::default()
        },
        out: out.clone(),
        mode: sim_bench::config::Mode::default(),
        arms: Vec::new(),
        runs,
    };
    println!(
        "{} targets on {map}, {sites} sites, at most {} trials ({per_site} a site on {} variants), {} runs of {} seats, into {}",
        specs.len(),
        weights.iter().sum::<usize>(),
        settings.variants.len(),
        n,
        settings.seats,
        out.display()
    );
    for (r, (t, c)) in batch.runs.iter().zip(&counts) {
        println!("  {:<28} {t} targets, at most {c} trials", r.id);
    }
    if a.dry {
        return Ok(());
    }
    let image = check_image(&mut a.common, "bounces")?;
    let disc = a.common.disc()?;
    prepare_output(&batch, &file, &[], None)?;
    let started = Instant::now();
    let failed = play_runs(
        &a.common,
        &batch,
        (&image.dylib, &disc),
        None,
        false,
        a.keep,
        n,
    );
    let mut blocks = Vec::new();
    let mut missing = Vec::new();
    for r in &batch.runs {
        let path = RunDir::new(&out, &r.id).report();
        let block = File::open(&path)
            .ok()
            .and_then(|f| {
                serde_json::from_reader::<_, sim_bench::metrics::Report>(std::io::BufReader::new(f))
                    .ok()
            })
            .and_then(|rep| rep.bounces);
        match block {
            Some(b) => blocks.push(b),
            None => missing.push(r.id.clone()),
        }
    }
    let mut report = sim_bench::bounces::summarize(&blocks);
    if report.map.is_empty() {
        report.map = map.clone();
    }
    for id in &missing {
        report.notes.push(format!("{id}: no report with a bounces block"));
    }
    std::fs::write(out.join("bounces.json"), serde_json::to_string_pretty(&report)?)?;
    let md = sim_bench::bounces::markdown(&report);
    std::fs::write(out.join("bounces.md"), &md)?;
    let proven = sim_bench::bounces::proven(&report);
    std::fs::write(
        out.join("bounces.verified"),
        sim_bench::bounces::verified_text(&proven),
    )?;
    println!(
        "\n{} runs in {:.0} s\n",
        batch.runs.len(),
        started.elapsed().as_secs_f64()
    );
    print!("{md}");
    let kept: Vec<PathBuf> = batch
        .runs
        .iter()
        .map(|r| RunDir::new(&out, &r.id).dir)
        .filter(|d| d.join("harvest").join("end").is_dir())
        .collect();
    if a.merge && !kept.is_empty() {
        println!();
        harvest_into(&kept, &bots::cache::dir(&shared), true)?;
    } else if !kept.is_empty() {
        println!(
            "\nthe provable sites are in each run's verified file: `h2 sim harvest {}`",
            out.display()
        );
    }
    println!("\n{}", out.join("bounces.json").display());
    if !report.errors.is_empty() {
        bail!("{}", report.errors.join("\n"));
    }
    if failed > 0 {
        bail!("{failed} sim runs failed; see the run logs");
    }
    Ok(())
}

/// `h2 sim harvest`: every run's `harvest/` of a results directory
/// merged into the shared bots directory.
fn harvest(args: Vec<String>) -> Result<()> {
    let mut dir = None;
    let mut data = None;
    let mut dry = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => dry = true,
            "--data" => data = Some(PathBuf::from(it.next().context("--data needs a value")?)),
            "-h" | "--help" => bail!("{USAGE}"),
            _ if dir.is_none() && !a.starts_with('-') => dir = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let dir = dir.context(USAGE)?;
    let into = bots::cache::dir(&data.unwrap_or_else(content::store::default_data_root));
    let runs = harvest_runs(&dir)?;
    anyhow::ensure!(
        !runs.is_empty(),
        "{}: no run kept bounce files (runs/*/harvest/end)",
        dir.display()
    );
    harvest_into(&runs, &into, !dry)?;
    Ok(())
}

/// Merges each run's `harvest/` into `into`, printing a line a map and a
/// total (`write` false: what it would add).
fn harvest_into(runs: &[PathBuf], into: &Path, write: bool) -> Result<()> {
    let mut total = (0, 0, 0);
    for run in runs {
        let rd = RunDir { dir: run.clone() };
        let got = bots::harvest::harvest_run(&rd.harvest_start(), &rd.harvest_end(), into, write)
            .map_err(anyhow::Error::msg)?;
        for h in got {
            println!(
                "  {:<40} {:<12} {} verified ({} proven), {} ghost runs",
                run.display(),
                h.map,
                h.verified,
                h.proven,
                h.ghosts
            );
            total.0 += h.verified;
            total.1 += h.proven;
            total.2 += h.ghosts;
        }
    }
    println!(
        "{} {} verified ({} proven) and {} ghost runs into {}",
        if write { "merged" } else { "would merge" },
        total.0,
        total.1,
        total.2,
        into.display()
    );
    Ok(())
}

/// The run directories under `dir` that kept bounce files: a batch's
/// `runs/*`, or a sweep's `candidates/*/runs/*`, in name order.
fn harvest_runs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut batches = vec![dir.to_path_buf()];
    if let Ok(c) = std::fs::read_dir(dir.join("candidates")) {
        batches.extend(
            c.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir()),
        );
    }
    for b in batches {
        let Ok(runs) = std::fs::read_dir(b.join("runs")) else {
            continue;
        };
        out.extend(
            runs.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.join("harvest").join("end").is_dir()),
        );
    }
    out.sort();
    Ok(out)
}

/// Saves the parts of the template a finished producer run made.
fn save_template(t: &cache_template::Template, data: &Path, parts: &[cache_template::Part]) {
    use cache_template::Saved;
    for part in parts {
        let what = part.label();
        match t.save(data, part) {
            Ok(Saved::Saved { files, cloned }) => println!(
                "map cache: saved {what} ({files} files) in template {} ({})",
                t.key(),
                if cloned { "cloned" } else { "copied, no clone" }
            ),
            Ok(Saved::Exists) => {}
            Ok(Saved::Nothing) => {
                println!("map cache: the run left no whole {what}; runs copy it from the disc")
            }
            Err(e) => println!("map cache: {what} not saved in template {}: {e}", t.key()),
        }
    }
}

fn prepare_output(
    b: &sim_bench::config::Batch,
    file: &Path,
    arms: &[String],
    batch_id: Option<&str>,
) -> Result<()> {
    std::fs::create_dir_all(&b.out)?;
    std::fs::write(
        b.out.join("batch.json"),
        serde_json::to_string_pretty(&json!({
            "batch": file.display().to_string(),
            "live_batch_id": batch_id,
            "arms": arms,
            "mode": b.mode.name(),
            "runs": b.runs.iter().map(|r| &r.id).collect::<Vec<_>>(),
        }))?,
    )?;
    for r in &b.runs {
        let rd = RunDir::new(&b.out, &r.id);
        std::fs::create_dir_all(&rd.dir)?;
        std::fs::write(rd.matched(), toml::to_string(&r.config)?)?;
        let _ = std::fs::remove_file(rd.audit());
        match std::fs::remove_file(rd.report()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("clearing {}", rd.report().display()));
            }
        }
    }
    Ok(())
}

fn finish_batch(b: &sim_bench::config::Batch, failed: usize, started: Instant) -> Result<()> {
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
        started.elapsed().as_secs_f64()
    );
    print!("{table}");
    println!("\n{}", b.out.join("compare.json").display());
    if failed > 0 {
        bail!("{failed} sim runs failed; see the run logs");
    }
    Ok(())
}

/// Reads `<dir>/runs/*/report.json` and compares them, arms in
/// `batch.json`'s order when it is there.
fn compare_dir(dir: &Path) -> Result<sim_bench::compare::Comparison> {
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
            .and_then(|t| serde_json::from_str::<sim_bench::metrics::Report>(&t).ok())
        {
            Some(r) => reports.push((id, r)),
            None => missing.push(id),
        }
    }
    Ok(sim_bench::compare::compare(&arms, &reports, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let a = child_args(&common, &rd);
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
                "fast"
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

    fn report(arm_a: &str, arm_b: &str, kills_b: i32) -> sim_bench::metrics::Report {
        let p = |index: u8, arm: &str, kills: i32| sim_bench::metrics::PlayerReport {
            index,
            name: format!("b{index}"),
            bot: true,
            seat: Some(index),
            arm: Some(arm.into()),
            kills,
            ..Default::default()
        };
        sim_bench::metrics::Report {
            format: sim_bench::metrics::FORMAT.into(),
            match_: Value::Null,
            result: sim_bench::metrics::MatchResult {
                end: sim_bench::metrics::End::TickLimit,
                winner: Some(sim_bench::metrics::Winner::Player(1)),
                teams: vec![],
            },
            ticks: 10,
            state_hashes_v1: Vec::new(),
            state_components_v1: None,
            state_digest_v1: None,
            game_seconds: 0.3,
            wall: Default::default(),
            players: vec![p(0, arm_a, 1), p(1, arm_b, kills_b)],
            kills: vec![],
            nav: "loaded".into(),
            dispatch_per_tick: None,
            flat: None,
            guest_census: None,
            tick_phases: None,
            notes: vec![],
            bounce: None,
            drops: None,
            bounces: None,
            firefight: None,
            rounds: vec![],
        }
    }

    #[test]
    fn think_mode_precedence() {
        use sim_bench::config::{BotThreads, MatchConfig};
        let cfg = |threads: Option<BotThreads>, pipeline: Option<bool>| MatchConfig {
            bot_threads: threads,
            bot_pipeline: pipeline,
            ..Default::default()
        };
        let none = cfg(None, None);
        let s = |v: &str| Some(v.to_string());
        // The default leaves both alone: serial.
        assert_eq!(think_env(&none, None, None), (None, None));
        // The match's keys beat the default.
        let keyed = cfg(Some(BotThreads::Count(2)), Some(true));
        assert_eq!(think_env(&keyed, None, None), (s("2"), s("1")));
        let off = cfg(Some(BotThreads::Word("off".into())), Some(false));
        assert_eq!(think_env(&off, None, None), (s("off"), s("0")));
        // The environment beats the match and the default, each variable alone.
        assert_eq!(think_env(&keyed, Some("0"), None), (None, s("1")));
        assert_eq!(think_env(&keyed, None, Some("0")), (s("2"), None));
        assert_eq!(think_env(&none, Some("0"), None), (None, None));
        assert_eq!(think_env(&keyed, Some(""), Some("")), (None, None));
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
    fn spatial_finds_the_traces_of_a_results_directory() {
        let dir = scratch("spatial");
        let trace = sim_bench::spatial::Trace::default();
        for id in ["lockout-s2", "lockout-s1"] {
            let d = dir.join("runs").join(id);
            std::fs::create_dir_all(&d).unwrap();
            trace.write(&d.join("spatial.bin")).unwrap();
        }
        // A run with no trace.
        std::fs::create_dir_all(dir.join("runs/lockout-s3")).unwrap();
        let found = spatial_files(&dir).unwrap();
        assert_eq!(
            found,
            [
                dir.join("runs/lockout-s1/spatial.bin"),
                dir.join("runs/lockout-s2/spatial.bin")
            ]
        );
        // A run's directory is its own trace; a file is itself.
        let one = dir.join("runs/lockout-s1");
        assert_eq!(spatial_files(&one).unwrap(), [one.join("spatial.bin")]);
        assert_eq!(spatial_files(&found[0]).unwrap(), [found[0].clone()]);
        assert!(spatial_files(&dir.join("runs/lockout-s3")).unwrap().is_empty());
        spatial(vec![dir.display().to_string()]).unwrap();
        assert!(spatial(vec![dir.join("runs/lockout-s3").display().to_string()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_and_local_reports_produce_the_same_comparison() {
        let local = scratch("provider-local");
        let hosted = scratch("provider-cloud");
        for dir in [&local, &hosted] {
            std::fs::write(
                dir.join("batch.json"),
                r#"{"arms":["base","aggr"],"runs":["s1","s2","s3"]}"#,
            )
            .unwrap();
        }
        for (id, kills) in [("s1", 3), ("s2", 5)] {
            let local_run = RunDir::new(&local, id);
            let hosted_run = RunDir::new(&hosted, id);
            std::fs::create_dir_all(&local_run.dir).unwrap();
            std::fs::create_dir_all(&hosted_run.dir).unwrap();
            let mut raw = serde_json::to_value(report("base", "aggr", kills)).unwrap();
            raw["match"] = json!({"map": "lockout", "seed": 1, "game_type": "slayer"});
            raw["determinism"] = json!({"hash": "aabb"});
            std::fs::write(local_run.report(), serde_json::to_vec(&raw).unwrap()).unwrap();
            cloud::save_report(&hosted_run, &raw).unwrap();
            let saved: Value = serde_json::from_slice(&std::fs::read(hosted_run.report()).unwrap())
                .unwrap();
            assert_eq!(saved, raw);
        }
        let comparison = compare_dir(&hosted).unwrap();
        assert_eq!(comparison, compare_dir(&local).unwrap());
        assert_eq!(comparison.missing, ["s3"]);
        assert_eq!(comparison.diff["kills"].diff, 3.0);
        std::fs::remove_dir_all(local).unwrap();
        std::fs::remove_dir_all(hosted).unwrap();
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

    #[test]
    fn a_bounce_run_plays_on_its_own_proofs_and_keeps_them() {
        let dir = scratch("bounce-data");
        let shared = dir.join("shared");
        let bots_dir = shared.join("bots");
        std::fs::create_dir_all(&bots_dir).unwrap();
        std::fs::write(bots_dir.join("ascension.0.bots"), "nav").unwrap();
        std::fs::write(bots_dir.join("ascension.spots"), "spots").unwrap();
        std::fs::write(bots_dir.join("ascension.verified"), "shared proofs").unwrap();
        let rd = RunDir::new(&dir, "ascension-s1");
        make_bounce_data(&rd, &shared).unwrap();
        let own = rd.data().join("bots");
        assert!(!own.symlink_metadata().unwrap().file_type().is_symlink());
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(own.join("ascension.0.bots")).unwrap(),
            bots_dir.join("ascension.0.bots"),
            "the nav mesh is shared"
        );
        let copied = own.join("ascension.verified");
        assert!(!copied.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(rd.harvest_start().join("ascension.verified")).unwrap(), "shared proofs");
        // The run proves a spot and keeps a ghost: the shared files stay.
        std::fs::write(&copied, "run proofs").unwrap();
        std::fs::write(own.join("ascension.ghosts"), "ghosts").unwrap();
        assert_eq!(keep_bounce_files(&rd).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(bots_dir.join("ascension.verified")).unwrap(), "shared proofs");
        assert!(!bots_dir.join("ascension.ghosts").exists());
        assert_eq!(std::fs::read_to_string(rd.harvest_end().join("ascension.verified")).unwrap(), "run proofs");
        assert_eq!(harvest_runs(&dir).unwrap(), vec![rd.dir.clone()]);
        // A sweep's candidates' runs are found too.
        let c = RunDir::new(&dir.join("candidates").join("c001"), "lockout-s2");
        std::fs::create_dir_all(c.harvest_end()).unwrap();
        assert_eq!(harvest_runs(&dir).unwrap().len(), 2);
        // Again: the run starts from the shared files once more.
        make_bounce_data(&rd, &shared).unwrap();
        assert!(!rd.harvest_end().exists());
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "shared proofs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn args(a: &[&str]) -> Result<TrainArgs> {
        parse_train_args(a.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn train_arguments_parse() {
        let t = args(&[
            "t.toml",
            "--map",
            "ascension",
            "--map",
            "lockout",
            "--budget",
            "200",
            "--reopen",
            "--no-merge",
            "--parallel",
            "3",
            "--disc",
            "/d.iso",
            "--data",
            "/shared",
            "--keep-data",
        ])
        .unwrap();
        assert_eq!(t.file, Some(PathBuf::from("t.toml")));
        assert_eq!(t.over.maps, ["ascension", "lockout"]);
        assert_eq!(t.over.budget, Some(200));
        assert!(t.reopen && t.no_merge && t.keep && !t.status && !t.dry);
        assert_eq!(t.parallel, Some(3));
        assert_eq!(t.common.disc, Some(PathBuf::from("/d.iso")));
        assert_eq!(t.common.data, Some(PathBuf::from("/shared")));
        let t = args(&["--status", "t.toml"]).unwrap();
        assert!(t.status && t.over.maps.is_empty() && t.over.budget.is_none());
        assert!(args(&["t.toml", "--dry-run"]).unwrap().dry);
    }

    #[test]
    fn train_arguments_are_checked() {
        assert!(args(&[]).is_err(), "a train file is needed");
        assert!(args(&["t.toml", "--budget"]).is_err());
        assert!(args(&["t.toml", "--budget", "many"]).is_err());
        assert!(args(&["t.toml", "--parallel", "0"]).is_err());
        assert!(args(&["t.toml", "--bogus"]).is_err());
        assert!(args(&["t.toml", "other.toml"]).is_err());
        let e = args(&["t.toml", "--status", "--reopen"])
            .unwrap_err()
            .to_string();
        assert!(e.contains("--status only reads"), "{e}");
    }

    #[test]
    fn a_bounce_run_owns_its_learned_file() {
        assert!(bounce_owned("ascension.learned"));
        assert!(bounce_owned("ascension.verified") && bounce_owned("ascension.ghosts"));
        assert!(!bounce_owned("ascension.0.bots") && !bounce_owned("ascension.spots"));
    }

    #[test]
    fn a_learned_file_is_found_by_its_map() {
        let dir = scratch("learned-in");
        std::fs::write(dir.join("Ascension.learned"), "# x\n").unwrap();
        std::fs::write(dir.join("lockout.verified"), "# y\n").unwrap();
        assert_eq!(learned_in(&dir, "ascension").unwrap().1, "# x\n");
        assert!(learned_in(&dir, "lockout").is_none());
        assert!(learned_in(&dir.join("none"), "ascension").is_none());
    }

    #[test]
    fn reopen_rewrites_the_shared_file_once() {
        let shared = scratch("reopen");
        let bots_dir = shared.join("bots");
        std::fs::create_dir_all(&bots_dir).unwrap();
        let text = "# superbounce learned v1\nbounce dead 1.000 2.000 3.000 tries=9 hits=0 why=budget stage=compass\nbounce learned 4.000 5.000 6.000 tries=9 hits=7\n";
        std::fs::write(bots_dir.join("m.learned"), text).unwrap();
        assert_eq!(reopen_learned(&shared, &["m".into()]).unwrap(), 1);
        let after = std::fs::read_to_string(bots_dir.join("m.learned")).unwrap();
        assert!(
            after.contains("bounce open 1.000 2.000 3.000 tries=9 hits=0 stage=arms"),
            "{after}"
        );
        assert!(after.contains("bounce learned 4.000 5.000 6.000 tries=9 hits=7"));
        assert_eq!(
            reopen_learned(&shared, &["m".into(), "other".into()]).unwrap(),
            0
        );
    }
}
