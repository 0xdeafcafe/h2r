//! The lab protocol: one JSON object per line each way (docs/runtime/lab.md).
//!
//! A request is `{"cmd": "...", ...}`; the answer is `{"ok": true, ...}` or
//! `{"ok": false, "error": "..."}`. Everything a request names is checked
//! here before the host acts on it: the environment comes from allowlists,
//! `h2 play` flags from a list of known ones, and every name that becomes a
//! path (run ids, recordings, PGO profiles) is one plain component.
//! Branch sync and service updates act on the configured checkout. Pair
//! only with drivers trusted to build and run code as the host user.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Protocol version, in `status`.
pub const VERSION: u32 = 1;

/// Run-time environment a request may set. Paths the game writes (frame
/// stats CSV, input recording, profile and agent output directories) are
/// the host's to choose.
pub const RUN_ENV: &[&str] = &[
    "H2_FRAME_STATS",
    "H2_FRAME_STATS_FROM",
    "H2_BOOT_STOP_AT_PRESENT",
    "H2_PERF_FROM",
    "H2_PROFILE_HZ",
    "H2_HOST_PERF",
    "H2_MEM_SAMPLE",
    "H2_PIPELINE_CACHE",
    "H2_PREWARM_PROGRAMS",
    "H2_AUDIO_STATS",
    "H2_FPS",
    "H2_RENDER_SCALE",
    "H2_RESOLUTION",
    "H2_HUD_POSITION",
    "H2_LOD_DISTANCE",
    "H2_WATERMARK",
    "H2_MAPPACK",
    "H2_HALO1",
    "H2_FEATURES",
    "H2_LOCKSTEP_TICKS",
    "H2_ICONS",
    // Sim jobs: the warm Z: template and the bots' think mode, for A/B arms.
    "H2_SIM_CACHE_TEMPLATE",
    "H2_BOT_THREADS",
    "H2_BOT_PIPELINE",
];

/// The host's own variables a `--own-data` run keeps (the others are
/// dropped like any run): the data root and Live's directory, server,
/// database, web app and cloud-saves switch, which `h2 play` there reads.
pub const OWN_DATA_ENV: &[&str] = &[
    "H2_DATA",
    "H2_LIVE_DIR",
    "H2_LIVE_SERVER",
    "H2_LIVE_DB",
    "H2_LIVE_WEB",
    "H2_CLOUD_SAVES",
];

/// `H2_PIPELINE_CACHE` values a request may pass: `off`, or
/// [`PIPELINE_CACHE_FRESH`]. The cache's path is the host's to choose.
pub const PIPELINE_CACHE_VALUES: &[&str] = &["off", PIPELINE_CACHE_FRESH];

/// `H2_PIPELINE_CACHE=fresh`: the run starts from an empty pipeline cache
/// of its own, in its run directory (A/B against the shared one).
pub const PIPELINE_CACHE_FRESH: &str = "fresh";

/// Build environment: it picks the image, so it is part of the image cache
/// key and is set again on every run of that image (an `h2` whose H2_OPT
/// differs from the image's rebuilds it).
pub const BUILD_ENV: &[&str] = &[
    "H2_OPT",
    "H2_DEBUG",
    "H2_CPU",
    "H2_HOT",
    "H2_OPT_COLD",
    "H2_PASSES",
    "H2_PGO",
];

/// Cargo features a build may turn on.
pub const FEATURES: &[&str] = &["perf"];

/// `h2 play` flags a run may pass, and whether each takes a value. Flags
/// that name files or directories, or reach the network, are not here.
pub const PLAY_FLAGS: &[(&str, bool)] = &[
    ("--no-audio", false),
    ("--no-gamepads", false),
    ("--no-vsync", false),
    ("--unpaced", false),
    ("--fullscreen", false),
    ("--nearest", false),
    ("--no-watermark", false),
    ("--no-mappack", false),
    ("--no-halo1", false),
    ("--status", false),
    ("--offline", false),
    ("--fps", true),
    ("--aspect", true),
    ("--scaling", true),
    ("--icons", true),
    ("--slices", true),
    ("--transfers", true),
    ("--set", true),
];

pub const DIFFICULTIES: &[&str] = &["easy", "normal", "heroic", "legendary"];

/// What to build: a commit of the host's checkout, its features and build
/// environment. With no `rev` the host runs its own `h2` and image as they
/// are (`--current`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BuildSpec {
    pub rev: Option<String>,
    /// The branch `rev` was resolved from (`--branch`, or the driving
    /// checkout's current branch). Only shown to the player; absent for
    /// `--rev` and `--current`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub features: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RunReq {
    /// Human-readable window label. Never used as a path.
    pub name: String,
    pub level: Option<String>,
    pub difficulty: Option<String>,
    /// Stop after this many presents (`H2_BOOT_STOP_AT_PRESENT`).
    pub frames: Option<u64>,
    /// Frame stats report from this present (`H2_FRAME_STATS_FROM`).
    pub stats_from: Option<u64>,
    /// Frame stats on, written to the run's frames.csv. On unless false.
    pub stats: Option<bool>,
    /// Replay this recording's input (`H2_INPUT_REPLAY`).
    pub replay: Option<String>,
    /// Record input into this recording (`H2_INPUT_RECORD`).
    pub record: Option<String>,
    /// Record a film into the host's film library as `<name>.h2film`, and
    /// keep a copy in the run's directory. Absent is never sent, so a
    /// request without a film is the same bytes an older host expects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub film: Option<String>,
    /// Profile into the run's profile/ (`H2_PROFILE`, needs `perf`).
    pub perf: bool,
    pub env: BTreeMap<String, String>,
    /// Extra `h2 play` flags from [`PLAY_FLAGS`].
    pub args: Vec<String>,
    pub build: BuildSpec,
    /// An empty data directory for this run only, removed after it.
    pub fresh_data: bool,
    /// Play on the host Mac's own data root, Live accounts and settings, as
    /// `h2 play` there would (no `--data`, Live's variables kept). Absent
    /// when false, so an older host still reads the request. Refused with
    /// `fresh_data`.
    #[serde(skip_serializing_if = "is_false")]
    pub own_data: bool,
    /// Game child footprint cap in bytes. Absent selects the host default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_limit: Option<u64>,
    /// Free text kept in the run's metadata.
    pub note: Option<String>,
    /// Driven through the agent API (`h2 lab agent`) until `stop`: the game
    /// never stops by itself, so `frames` can't be set (docs/runtime/lab.md).
    pub serve: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One arm of a sim benchmark: a name (a path component, it names the
/// arm's step directories) and the `H2_FEATURES` value its runs get. No
/// `features` leaves the variable as the request's `env` has it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SimArm {
    pub name: String,
    pub features: Option<String>,
}

/// A sim benchmark job (`h2 lab sim`, docs/runtime/lab.md, "Sim
/// benchmarks"): `h2 sim run` or `h2 sim batch` of one uploaded file, once
/// per arm and repeat.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SimReq {
    /// Human-readable job label. Never used as a path.
    pub name: String,
    /// The entry file's key in `files`: a match file, or a batch file when
    /// `batch`.
    pub file: String,
    pub batch: bool,
    /// The entry file and the tuning files it names, by path relative to
    /// their common directory.
    pub files: BTreeMap<String, String>,
    /// The bots' data for the maps the match or batch names, as hex under
    /// `bots/<file>` (`.bots` and `.vis`, from the driving Mac's data root).
    /// The host puts them in the job's data root. Empty (an older client)
    /// runs on the host's own data.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, String>,
    pub arms: Vec<SimArm>,
    /// Runs of each arm, interleaved (`simbench::schedule`).
    pub repeat: u32,
    /// Matches at once in a batch. Absent: one at a time, since these are
    /// benchmarks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<usize>,
    /// Run environment from [`RUN_ENV`], set on every run before the arm's
    /// `H2_FEATURES`.
    pub env: BTreeMap<String, String>,
    pub build: BuildSpec,
    /// Footprint cap on the sim process, in bytes. Absent selects the host
    /// default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_limit: Option<u64>,
    pub note: Option<String>,
}

/// Most arms a sim benchmark compares.
pub const SIM_ARMS_MAX: usize = 8;
/// Most repeats of each arm.
pub const SIM_REPEAT_MAX: u32 = 100;
/// Most files, and most bytes of them, one sim request carries.
pub const SIM_FILES_MAX: usize = 64;
pub const SIM_BYTES_MAX: usize = 2 << 20;
/// Most files, and most bytes (before hex) of bot data one sim request
/// carries. Hex-encoded with the files it stays under `REQUEST_MAX`.
pub const SIM_DATA_FILES_MAX: usize = 64;
pub const SIM_DATA_MAX: usize = 60 << 20;

/// A recording's manifest (`recordings/<name>/manifest.json`). A recording
/// is a directory so a machine snapshot can join the input later
/// (`snapshot`, empty until agent API phase 5 exists).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Manifest {
    pub name: String,
    pub level: Option<String>,
    pub difficulty: Option<String>,
    /// Commit of the image it was recorded on, when known.
    pub commit: Option<String>,
    /// The input log, a file in the recording's directory.
    pub input: String,
    /// A snapshot to start the replay from; none can be made yet.
    pub snapshot: Option<String>,
    pub created: u64,
    /// The run that recorded it, when the host did.
    pub run: Option<String>,
}

/// The extension of a film file in the library.
pub const FILM_EXT: &str = "h2film";

/// The library file name for film `name` (`<name>.h2film`).
pub fn film_file(name: &str) -> String {
    format!("{name}.{FILM_EXT}")
}

/// A film name is a plain file stem: [`check_name`]'s rules, and no
/// extension of its own, so the file is exactly `<name>.h2film`.
pub fn check_film_name(s: &str) -> Result<()> {
    check_name("film", s)?;
    ensure!(
        !s.ends_with(&format!(".{FILM_EXT}")),
        "film {s:?} is a name without its .{FILM_EXT} extension"
    );
    Ok(())
}

/// The input log's file name inside a recording.
pub const INPUT_FILE: &str = "input.bin";

pub(super) const CURRENT_JOB: &str = "current";

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Update,
    Fetch {
        id: String,
    },
    Mem {
        id: String,
    },
    /// The log feed for one job ID or all current jobs, using transfer frames.
    Logs {
        id: String,
        #[serde(default)]
        follow: bool,
    },
    SyncBases {
        branch: String,
        commit: String,
    },
    Sync {
        branch: String,
        commit: String,
        bytes: u64,
    },
    Run(RunReq),
    /// A sim benchmark (`h2 sim` per arm and repeat, on the lab's image).
    Sim(SimReq),
    Build(BuildSpec),
    Wait {
        id: String,
        /// Seconds; none waits until the job ends.
        #[serde(default)]
        timeout: Option<f64>,
    },
    Stop {
        id: String,
    },
    Runs {
        #[serde(default)]
        limit: Option<usize>,
    },
    Recordings,
    /// One `h2 agent` call against run `id`'s game: `args` are what follow
    /// `h2 agent` on a command line (a method and its JSON, or a short
    /// form). The answer carries the call's `exit`, `stdout` and `stderr`.
    Agent {
        id: String,
        args: Vec<String>,
        /// Seconds the call may take (default [`AGENT_TIMEOUT`]).
        #[serde(default)]
        timeout: Option<f64>,
        /// Standard input for the call (an `h2 agent -` script).
        #[serde(default)]
        stdin: Option<String>,
    },
    /// Store a recording's input (hex) under `recordings/<name>`, or a
    /// PGO profile under `pgo/<name>` with `kind: "pgo"`.
    Put {
        name: String,
        #[serde(default)]
        kind: Option<String>,
        hex: String,
        #[serde(default)]
        level: Option<String>,
        #[serde(default)]
        difficulty: Option<String>,
        #[serde(default)]
        commit: Option<String>,
    },
}

/// Default seconds an `agent` call may take.
pub const AGENT_TIMEOUT: f64 = 300.0;

/// Most seconds an `agent` call may take.
pub const AGENT_TIMEOUT_MAX: f64 = 7200.0;

/// Largest `agent` stdin, in bytes.
pub const AGENT_STDIN_MAX: usize = 1 << 20;

/// Largest upload `put` takes, in bytes.
pub const PUT_MAX: usize = 64 << 20;

/// Largest git bundle a sync accepts, streamed to disk.
pub const BUNDLE_MAX: u64 = 4 << 30;

/// Bounds a control message before it can reach the JSON parser.
pub const REQUEST_MAX: u64 = (2 * PUT_MAX + (1 << 20)) as u64;

pub fn check_commit(commit: &str) -> Result<()> {
    ensure!(
        matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "commit must be a full git object id"
    );
    Ok(())
}

pub fn parse(line: &str) -> Result<Request> {
    let mut r: Request = serde_json::from_str(line)?;
    if let Request::Run(run) = &mut r {
        run.name = job_name(&run.name)?.into();
    }
    if let Request::Sim(sim) = &mut r {
        sim.name = job_name(&sim.name)?.into();
    }
    validate(&r)?;
    Ok(r)
}

pub fn validate(r: &Request) -> Result<()> {
    match r {
        Request::Status | Request::Update | Request::Recordings | Request::Runs { .. } => Ok(()),
        Request::Fetch { id } | Request::Mem { id } => check_job_id(id),
        Request::Logs { id, .. } => check_log_id(id),
        Request::SyncBases { branch, commit } | Request::Sync { branch, commit, .. } => {
            check_rev(branch)?;
            check_commit(commit)?;
            if let Request::Sync { bytes, .. } = r {
                ensure!(
                    *bytes <= BUNDLE_MAX,
                    "git bundle over {} GiB",
                    BUNDLE_MAX >> 30
                );
            }
            Ok(())
        }
        Request::Run(run) => validate_run(run),
        Request::Sim(sim) => validate_sim(sim),
        Request::Build(b) => validate_build(b),
        Request::Wait { id, timeout } => {
            check_name("id", id)?;
            if let Some(t) = timeout {
                ensure!(
                    t.is_finite() && *t >= 0.0,
                    "timeout must be a non-negative number of seconds"
                );
            }
            Ok(())
        }
        Request::Stop { id } => check_name("id", id),
        Request::Agent {
            id,
            args,
            timeout,
            stdin,
        } => {
            check_name("id", id)?;
            check_agent_args(args)?;
            if let Some(t) = timeout {
                ensure!(
                    t.is_finite() && *t > 0.0 && *t <= AGENT_TIMEOUT_MAX,
                    "timeout must be between 0 and {AGENT_TIMEOUT_MAX} seconds"
                );
            }
            ensure!(
                stdin.as_ref().is_none_or(|s| s.len() <= AGENT_STDIN_MAX),
                "stdin over {} KiB",
                AGENT_STDIN_MAX >> 10
            );
            Ok(())
        }
        Request::Put {
            name,
            kind,
            hex,
            level,
            difficulty,
            commit,
        } => {
            check_name("name", name)?;
            match kind.as_deref() {
                None | Some("recording") | Some("pgo") => {}
                Some(k) => bail!("unknown put kind {k:?} (recording or pgo)"),
            }
            ensure!(
                hex.len() / 2 <= PUT_MAX,
                "upload over {} MiB",
                PUT_MAX >> 20
            );
            if let Some(l) = level {
                check_level(l)?;
            }
            if let Some(d) = difficulty {
                check_difficulty(d)?;
            }
            if let Some(c) = commit {
                ensure!(
                    c.len() <= 64 && c.bytes().all(|b| b.is_ascii_hexdigit()),
                    "commit must be a hex id"
                );
            }
            Ok(())
        }
    }
}

pub(super) fn check_log_id(id: &str) -> Result<()> {
    if id == CURRENT_JOB {
        return Ok(());
    }
    check_job_id(id)
}

/// What follows `h2 agent` in an `agent` request. The host uses the run's
/// socket for calls, including `flight show`, which reads a bare file name
/// through the game's API. `raw-diff` only reads local files.
pub fn check_agent_args(args: &[String]) -> Result<()> {
    ensure!(
        !args.is_empty() && args.len() <= 64,
        "an agent call is a method and at most 63 more words"
    );
    for a in args {
        ensure!(
            a.len() <= 64 << 10 && !a.contains('\0'),
            "an agent argument is over 64 KiB or has a NUL"
        );
        ensure!(
            !matches!(
                a.as_str(),
                "--socket" | "--xbox" | "--timeout" | "-h" | "--help"
            ),
            "{a} is not passed to the host's h2 agent: the run's own socket is used (--timeout is an option of `h2 lab agent`)"
        );
    }
    ensure!(
        args[0] != "raw-diff",
        "`h2 agent {}` reads files on the machine it runs on; the lab doesn't forward it",
        args[0]
    );
    if let [domain, verb, show_args @ ..] = args
        && domain == "flight"
        && verb == "show"
    {
        crate::agent_flight::check_remote_show_args(show_args)?;
    }
    Ok(())
}

/// Labels may include spaces and Unicode text. Use Rust's printable character
/// tables to exclude controls, invisible formatting and line separators.
pub fn job_name(raw: &str) -> Result<&str> {
    let name = raw.trim();
    ensure!(!name.is_empty(), "lab game jobs require --name TEXT");
    ensure!(
        name.chars().count() <= 60
            && raw
                .chars()
                .all(|ch| { ch == ' ' || ch.is_ascii_graphic() || ch.escape_debug().count() == 1 }),
        "--name must be a printable label of at most 60 characters"
    );
    Ok(name)
}

pub fn window_title(id: &str, name: &str, commit: &str) -> String {
    format!("h2 · lab {id} · {name} · {commit:.7}")
}

pub fn validate_run(r: &RunReq) -> Result<()> {
    job_name(&r.name)?;
    ensure!(
        !(r.serve && r.frames.is_some()),
        "a --serve run never stops by itself; --frames can't be set (h2 lab stop ends it)"
    );
    ensure!(
        !(r.own_data && r.fresh_data),
        "--own-data plays on the host's own data; --fresh-data asks for an empty one"
    );
    if let Some(l) = &r.level {
        check_level(l)?;
    }
    if let Some(d) = &r.difficulty {
        ensure!(
            r.level.is_some() || r.replay.is_some(),
            "a difficulty needs a level"
        );
        check_difficulty(d)?;
    }
    if let Some(n) = &r.replay {
        check_name("replay", n)?;
    }
    if let Some(n) = &r.record {
        check_name("record", n)?;
    }
    if let Some(n) = &r.film {
        check_film_name(n)?;
    }
    ensure!(
        r.replay.is_none() || r.record.is_none(),
        "replay and record are exclusive"
    );
    for (k, v) in &r.env {
        ensure!(
            RUN_ENV.contains(&k.as_str()),
            "{k} is not a run variable the lab passes (allowed: {}; build variables go in build.env)",
            RUN_ENV.join(" ")
        );
        check_value(k, v)?;
        if k == "H2_MEM_SAMPLE" {
            mem_watch::parse_size(v).map_err(anyhow::Error::msg)?;
        }
        if k == "H2_PIPELINE_CACHE" {
            ensure!(
                PIPELINE_CACHE_VALUES.contains(&v.as_str()),
                "H2_PIPELINE_CACHE is one of {} (the cache's path is the host's to choose)",
                PIPELINE_CACHE_VALUES.join(" ")
            );
        }
        if k == "H2_PREWARM_PROGRAMS" {
            ensure!(
                v == "off" || v.parse::<usize>().is_ok(),
                "H2_PREWARM_PROGRAMS is a count or off"
            );
        }
    }
    check_play_args(&r.args)?;
    if let Some(n) = &r.note {
        ensure!(n.len() <= 1024, "note over 1024 bytes");
    }
    validate_build(&r.build)
}

/// One uploaded sim file's key: a relative `/` path of plain components,
/// so it stays inside the step's input directory.
pub fn check_sim_path(key: &str) -> Result<()> {
    ensure!(
        key.len() <= 200 && !key.is_empty(),
        "sim file {key:?} must be a relative path of at most 200 bytes"
    );
    for part in key.split('/') {
        check_name("sim file path component", part)?;
    }
    Ok(())
}

/// One uploaded bot data file's key: `bots/<name>.bots` or `.vis`, `name`
/// a plain component (`bots::cache::file_name` makes them).
pub fn check_sim_data_key(key: &str) -> Result<()> {
    let name = key
        .strip_prefix("bots/")
        .with_context(|| format!("sim data {key:?} must be bots/<file>"))?;
    ensure!(
        name.ends_with(".bots") || name.ends_with(".vis"),
        "sim data {key:?} must be a .bots or .vis file"
    );
    check_name("sim data file name", name)
}

pub fn validate_sim(s: &SimReq) -> Result<()> {
    job_name(&s.name)?;
    check_sim_path(&s.file)?;
    ensure!(
        s.files.contains_key(&s.file),
        "sim file {:?} is not among the uploaded files",
        s.file
    );
    ensure!(
        s.files.len() <= SIM_FILES_MAX,
        "more than {SIM_FILES_MAX} sim files"
    );
    for key in s.files.keys() {
        check_sim_path(key)?;
    }
    ensure!(
        s.files.values().map(String::len).sum::<usize>() <= SIM_BYTES_MAX,
        "sim files over {} MiB",
        SIM_BYTES_MAX >> 20
    );
    ensure!(
        s.data.len() <= SIM_DATA_FILES_MAX,
        "more than {SIM_DATA_FILES_MAX} sim data files"
    );
    for (key, hex) in &s.data {
        check_sim_data_key(key)?;
        ensure!(hex.len() % 2 == 0, "sim data {key:?} is not hex");
    }
    ensure!(
        s.data.values().map(|h| h.len() / 2).sum::<usize>() <= SIM_DATA_MAX,
        "sim data over {} MiB",
        SIM_DATA_MAX >> 20
    );
    ensure!(
        (1..=SIM_ARMS_MAX).contains(&s.arms.len()),
        "a sim benchmark has 1 to {SIM_ARMS_MAX} arms"
    );
    let mut seen = std::collections::BTreeSet::new();
    for arm in &s.arms {
        check_name("arm", &arm.name)?;
        ensure!(seen.insert(arm.name.as_str()), "arm {:?} is named twice", arm.name);
        if let Some(f) = &arm.features {
            check_value("H2_FEATURES", f)?;
        }
    }
    ensure!(
        (1..=SIM_REPEAT_MAX).contains(&s.repeat),
        "--repeat is 1 to {SIM_REPEAT_MAX}"
    );
    if let Some(p) = s.parallel {
        ensure!(s.batch, "--parallel applies to a batch file");
        ensure!((1..=64).contains(&p), "--parallel is 1 to 64");
    }
    for (k, v) in &s.env {
        ensure!(
            RUN_ENV.contains(&k.as_str()),
            "{k} is not a run variable the lab passes (allowed: {})",
            RUN_ENV.join(" ")
        );
        check_value(k, v)?;
    }
    if let Some(n) = &s.note {
        ensure!(n.len() <= 1024, "note over 1024 bytes");
    }
    validate_build(&s.build)
}

pub fn validate_build(b: &BuildSpec) -> Result<()> {
    if let Some(rev) = &b.rev {
        check_rev(rev)?;
    }
    if let Some(branch) = &b.branch {
        check_rev(branch)?;
        ensure!(b.rev.is_some(), "a build branch needs the rev it names");
    }
    for f in &b.features {
        ensure!(
            FEATURES.contains(&f.as_str()),
            "unknown feature {f:?} (allowed: {})",
            FEATURES.join(" ")
        );
    }
    for (k, v) in &b.env {
        ensure!(
            BUILD_ENV.contains(&k.as_str()),
            "{k} is not a build variable (allowed: {})",
            BUILD_ENV.join(" ")
        );
        check_value(k, v)?;
        if k == "H2_PGO" {
            pgo_name(v)?;
        }
    }
    ensure!(
        b.rev.is_some() || (b.features.is_empty() && b.env.is_empty()),
        "build features and variables need a branch or commit; --current runs the host's image as it is"
    );
    Ok(())
}

/// `H2_PGO`: `gen`, or `use:NAME` with NAME a profile `put` into `pgo/`.
/// Returns that NAME.
pub fn pgo_name(v: &str) -> Result<Option<&str>> {
    if v == "gen" || v.is_empty() || v == "0" || v == "off" {
        return Ok(None);
    }
    let Some(name) = v.strip_prefix("use:") else {
        bail!("H2_PGO must be gen or use:NAME (a profile put with `h2 lab put --pgo`)");
    };
    check_name("H2_PGO profile", name)?;
    Ok(Some(name))
}

/// The PGO profile name `env`'s `H2_PGO` uses, if any.
pub fn pgo_from(env: &BTreeMap<String, String>) -> Result<Option<&str>> {
    env.get("H2_PGO").map_or(Ok(None), |v| pgo_name(v))
}

pub(super) fn check_job_id(id: &str) -> Result<()> {
    check_name("id", id)?;
    ensure!(id.bytes().all(|b| b.is_ascii_digit()), "invalid job id");
    Ok(())
}

/// One plain path component: letters, digits, `.`, `_`, `-`; not starting
/// with a dot (so no `..`, no hidden files) and at most 64 bytes.
pub fn check_name(what: &str, s: &str) -> Result<()> {
    ensure!(
        !s.is_empty()
            && s.len() <= 64
            && !s.starts_with('.')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
        "{what} {s:?} must be 1-64 letters, digits, '.', '_' or '-', not starting with '.'"
    );
    Ok(())
}

/// A map file name from the disc (`03a_oldmombasa`, `ascension`).
pub fn check_level(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "level {s:?} must be a map name (letters, digits, '_')"
    );
    Ok(())
}

pub fn check_difficulty(s: &str) -> Result<()> {
    ensure!(
        DIFFICULTIES.contains(&s),
        "difficulty must be one of {}",
        DIFFICULTIES.join(", ")
    );
    Ok(())
}

/// A branch, tag or commit for `git rev-parse`: no option-like start, no
/// `..` ranges, no reflog or revision syntax beyond plain names.
pub fn check_rev(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty()
            && s.len() <= 200
            && !s.starts_with('-')
            && !s.starts_with('/')
            && !s.contains("..")
            && !s.contains("//")
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/')),
        "rev {s:?} must be a branch name or commit id"
    );
    Ok(())
}

fn check_value(k: &str, v: &str) -> Result<()> {
    ensure!(
        v.len() <= 256 && !v.contains('\0') && !v.contains('\n'),
        "{k}'s value must be one line of at most 256 bytes"
    );
    Ok(())
}

/// Checks `h2 play` flags against [`PLAY_FLAGS`]; a value is the next word.
pub fn check_play_args(args: &[String]) -> Result<()> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(&(_, takes)) = PLAY_FLAGS.iter().find(|(f, _)| *f == a.as_str()) else {
            bail!(
                "h2 play flag {a:?} is not one the lab passes (allowed: {})",
                PLAY_FLAGS
                    .iter()
                    .map(|(f, _)| *f)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        };
        if takes {
            let Some(v) = it.next() else {
                bail!("{a} needs a value")
            };
            ensure!(
                !v.starts_with('-') || v.parse::<f64>().is_ok(),
                "{a} needs a value, not {v:?}"
            );
            ensure!(
                v.len() <= 128 && !v.contains('\0') && !v.contains('\n'),
                "{a}'s value is too long"
            );
            if a == "--set" {
                ensure!(v.contains('='), "--set needs KEY=VALUE");
            }
        }
    }
    Ok(())
}

/// Hex payloads for recording and PGO uploads.
pub fn to_hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

pub fn from_hex(s: &str) -> Result<Vec<u8>> {
    ensure!(s.len() % 2 == 0, "odd hex length");
    let d = |c: u8| -> Result<u8> {
        Ok(match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => bail!("bad hex digit"),
        })
    };
    s.as_bytes()
        .chunks(2)
        .map(|p| Ok(d(p[0])? << 4 | d(p[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(json: &str) -> Result<Request> {
        parse(json)
    }

    #[test]
    fn names_round_trip_trimmed_and_only_printable() {
        let Request::Run(run) = parse(r#"{"cmd":"run","name":"  Café A/B  "}"#).unwrap() else {
            panic!()
        };
        assert_eq!(run.name, "Café A/B");
        assert_eq!(serde_json::to_value(run).unwrap()["name"], "Café A/B");
        assert!(
            parse(r#"{"cmd":"run"}"#)
                .unwrap_err()
                .to_string()
                .contains("--name")
        );
        for bad in [
            "",
            "  ",
            "line\nbreak",
            "\u{1b}",
            "bidi\u{202e}",
            &"é".repeat(61),
        ] {
            assert!(job_name(bad).is_err(), "{bad:?}");
        }
        assert!(job_name(&"é".repeat(60)).is_ok());
        assert_eq!(
            window_title("0007", "Café A/B", "0123456789abcdef"),
            "h2 · lab 0007 · Café A/B · 0123456"
        );
    }

    #[test]
    fn a_full_run_request_parses() {
        let r = run(
            r#"{"cmd":"run","name":"Fixture","level":"03a_oldmombasa","difficulty":"heroic","frames":900,
            "stats_from":300,"replay":"fight-1","perf":true,"env":{"H2_PROFILE_HZ":"2000"},
            "args":["--no-audio","--fps","60","--set","fov=90"],
            "build":{"rev":"lab/perf-work","features":["perf"],"env":{"H2_OPT":"2"}}}"#,
        )
        .unwrap();
        let Request::Run(r) = r else { panic!() };
        assert_eq!(r.frames, Some(900));
        assert_eq!(r.build.rev.as_deref(), Some("lab/perf-work"));
        assert_eq!(r.args.len(), 5);
    }

    #[test]
    fn short_requests_parse() {
        assert_eq!(run(r#"{"cmd":"status"}"#).unwrap(), Request::Status);
        assert_eq!(
            run(r#"{"cmd":"wait","id":"0003","timeout":2.5}"#).unwrap(),
            Request::Wait {
                id: "0003".into(),
                timeout: Some(2.5)
            }
        );
        assert!(run(r#"{"cmd":"shell","line":"rm -rf /"}"#).is_err());
        assert!(
            run(r#"{"cmd":"run","name":"Fixture","h2":"/bin/sh"}"#).is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn logs_request_accepts_only_job_ids_or_current() {
        assert_eq!(
            run(r#"{"cmd":"logs","id":"0003"}"#).unwrap(),
            Request::Logs {
                id: "0003".into(),
                follow: false,
            }
        );
        assert_eq!(
            run(r#"{"cmd":"logs","id":"current","follow":true}"#).unwrap(),
            Request::Logs {
                id: CURRENT_JOB.into(),
                follow: true,
            }
        );
        for bad in ["..", "../auth/pairing.json", "some-name", ""] {
            let request = serde_json::json!({"cmd": "logs", "id": bad});
            assert!(run(&request.to_string()).is_err(), "{request}");
        }
        assert!(run(r#"{"cmd":"logs","id":"current","path":"host.log"}"#).is_err());
    }

    #[test]
    fn run_env_is_an_allowlist() {
        for k in [
            "H2_FRAME_STATS_CSV",
            "H2_INPUT_REPLAY",
            "H2_PROFILE",
            "H2_AGENT",
            agent_proto::OUT_ENV,
            "PATH",
            "DYLD_INSERT_LIBRARIES",
            "H2_OPT",
        ] {
            let j = format!(r#"{{"cmd":"run","name":"Fixture","env":{{"{k}":"x"}}}}"#);
            assert!(run(&j).is_err(), "{k} must be refused");
        }
        assert!(
            run(r#"{"cmd":"run","name":"Fixture","env":{"H2_FRAME_STATS_FROM":"300"}}"#).is_ok()
        );
        assert!(run(r#"{"cmd":"run","name":"Fixture","env":{"H2_RENDER_SCALE":"4","H2_LOD_DISTANCE":"retail"}}"#).is_ok());
        assert!(run(r#"{"cmd":"run","name":"Fixture","env":{"H2_FPS":"6\n0"}}"#).is_err());
    }

    /// The pipeline cache can be turned off or started fresh, never pointed
    /// at a path: the game rewrites that file.
    #[test]
    fn pipeline_cache_takes_off_or_fresh() {
        let env = |k: &str, v: &str| {
            run(&format!(
                r#"{{"cmd":"run","name":"Fixture","env":{{"{k}":"{v}"}}}}"#
            ))
        };
        assert!(env("H2_PIPELINE_CACHE", "off").is_ok());
        assert!(env("H2_PIPELINE_CACHE", "fresh").is_ok());
        for bad in ["/Users/x/.zshrc", "../cache.jsonl", "pipelines.jsonl", ""] {
            assert!(env("H2_PIPELINE_CACHE", bad).is_err(), "{bad:?}");
        }
        assert!(env("H2_PREWARM_PROGRAMS", "256").is_ok());
        assert!(env("H2_PREWARM_PROGRAMS", "off").is_ok());
        assert!(env("H2_PREWARM_PROGRAMS", "lots").is_err());
    }

    #[test]
    fn build_env_is_an_allowlist_and_needs_a_rev() {
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"H2_OPT":"2","H2_PGO":"use:city.profdata"}}"#).is_ok());
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"CC":"evil"}}"#).is_err());
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"H2_PGO":"use:/tmp/x"}}"#).is_err());
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"H2_PGO":"use:../x"}}"#).is_err());
        assert!(run(r#"{"cmd":"build","rev":"master","features":["metal-debug"]}"#).is_err());
        assert!(
            run(r#"{"cmd":"run","name":"Fixture","build":{"env":{"H2_OPT":"2"}}}"#).is_err(),
            "no rev"
        );
    }

    #[test]
    fn names_never_escape_the_lab_root() {
        for bad in [
            "..",
            "../x",
            "a/b",
            "/etc",
            ".hidden",
            "",
            "a\0b",
            "a b",
            &"x".repeat(65),
        ] {
            assert!(check_name("n", bad).is_err(), "{bad:?}");
        }
        for good in ["fight-1", "0007", "city_heroic.v2"] {
            check_name("n", good).unwrap();
        }
        assert!(run(r#"{"cmd":"run","name":"Fixture","replay":"../../.ssh/id_ed25519"}"#).is_err());
        assert!(run(r#"{"cmd":"run","name":"Fixture","record":"a/b"}"#).is_err());
        assert!(run(r#"{"cmd":"wait","id":".."}"#).is_err());
        assert!(run(r#"{"cmd":"put","name":"../x","hex":""}"#).is_err());
    }

    #[test]
    fn revs_are_plain_names() {
        for bad in [
            "-c",
            "--upload-pack=x",
            "a..b",
            "HEAD@{1}",
            "a b",
            "/abs",
            "x~1",
            "x^",
        ] {
            assert!(check_rev(bad).is_err(), "{bad:?}");
        }
        for good in ["master", "lab/perf/city", "0123abcd", "v1.2"] {
            check_rev(good).unwrap();
        }
    }

    #[test]
    fn play_flags_are_known_ones_with_sane_values() {
        let a = |v: &[&str]| check_play_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        a(&["--no-audio", "--fps", "60", "--set", "fov=90"]).unwrap();
        assert!(a(&["--out", "/tmp/x"]).is_err());
        assert!(a(&["--live"]).is_err());
        assert!(a(&["--fps"]).is_err());
        assert!(a(&["--fps", "--no-audio"]).is_err());
        assert!(a(&["--set", "novalue"]).is_err());
        assert!(a(&["03a_oldmombasa"]).is_err());
    }

    #[test]
    fn agent_calls_use_the_runs_socket() {
        let ok = r#"{"cmd":"agent","id":"0003","args":["capture","600"],"timeout":90}"#;
        assert!(run(ok).is_ok());
        assert!(
            run(r#"{"cmd":"agent","id":"0003","args":["session.wait","{\"until\":{\"phase\":\"gameplay\"}}"]}"#)
                .is_ok()
        );
        assert!(
            run(r#"{"cmd":"agent","id":"0003","args":["flight","show","walk.jsonl","--field","physics.crouch"]}"#)
                .is_ok()
        );
        assert!(run(r#"{"cmd":"agent","id":"0003","args":["flight","dump","walk.jsonl"]}"#).is_ok());
        for bad in [
            r#"{"cmd":"agent","id":"0003","args":[]}"#,
            r#"{"cmd":"agent","id":"../x","args":["status"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["status","--socket","/tmp/x"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["status","--xbox"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["flight","show","/etc/passwd"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["flight","show","walk.jsonl","--field"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["raw-diff","a","b"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["status"],"timeout":0}"#,
            r#"{"cmd":"agent","id":"0003","args":["status"],"timeout":-1}"#,
        ] {
            assert!(run(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn own_data_round_trips_and_is_absent_for_older_hosts() {
        let old: RunReq = serde_json::from_str(r#"{"name":"Fixture"}"#).unwrap();
        assert!(!old.own_data);
        assert!(!serde_json::to_string(&old).unwrap().contains("own_data"));
        let own = run(r#"{"cmd":"run","name":"Fixture","own_data":true,"serve":true}"#);
        assert!(own.is_ok());
        let r = RunReq { name: "Fixture".into(), own_data: true, ..RunReq::default() };
        let back: RunReq = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        assert!(
            run(r#"{"cmd":"run","name":"Fixture","own_data":true,"fresh_data":true}"#).is_err()
        );
    }

    #[test]
    fn a_serve_run_has_no_frame_limit() {
        assert!(run(r#"{"cmd":"run","name":"Fixture","level":"x","serve":true}"#).is_ok());
        assert!(
            run(r#"{"cmd":"run","name":"Fixture","level":"x","serve":true,"frames":10}"#).is_err()
        );
    }

    #[test]
    fn levels_and_difficulties() {
        assert!(run(r#"{"cmd":"run","name":"Fixture","level":"03a_oldmombasa","difficulty":"legendary"}"#).is_ok());
        assert!(run(r#"{"cmd":"run","name":"Fixture","level":"../x"}"#).is_err());
        assert!(
            run(r#"{"cmd":"run","name":"Fixture","level":"x","difficulty":"impossible"}"#).is_err()
        );
        assert!(run(r#"{"cmd":"run","name":"Fixture","difficulty":"easy"}"#).is_err());
        assert!(run(r#"{"cmd":"run","name":"Fixture","replay":"a","record":"b"}"#).is_err());
    }

    #[test]
    fn a_film_name_is_a_plain_file_stem() {
        let film = |n: &str| run(&format!(r#"{{"cmd":"run","name":"Fixture","level":"x","film":"{n}"}}"#));
        assert!(film("coldstart-1").is_ok());
        assert!(film("a.b_c").is_ok());
        for bad in ["", ".hidden", "../x", "a/b", "a b", "x.h2film", &"a".repeat(65)] {
            assert!(film(bad).is_err(), "{bad:?}");
        }
        assert_eq!(film_file("coldstart-1"), "coldstart-1.h2film");
    }

    /// The game's own `--film` is the host's to place, so a request can't
    /// aim it at a path.
    #[test]
    fn film_is_not_a_play_flag() {
        assert!(check_play_args(&["--film".into(), "/tmp/x.h2film".into()]).is_err());
    }

    /// A request without a film serializes without the field, so an older
    /// host (which refuses unknown fields) still accepts it.
    #[test]
    fn a_request_without_a_film_omits_the_field() {
        let v = serde_json::to_value(RunReq::default()).unwrap();
        assert!(v.get("film").is_none());
        let v = serde_json::to_value(RunReq {
            film: Some("f".into()),
            ..RunReq::default()
        })
        .unwrap();
        assert_eq!(v["film"], "f");
    }


    fn sim_value() -> serde_json::Value {
        serde_json::json!({
            "cmd": "sim", "name": " Lockout A/B ", "file": "m.toml",
            "files": {"m.toml": "map = 1"},
            "arms": [
                {"name": "on", "features": "impl.sim.native"},
                {"name": "off", "features": "-impl.sim.native"},
            ],
            "repeat": 3,
        })
    }

    /// The request with `edit` applied, as the host would parse it.
    fn sim_with(edit: impl FnOnce(&mut serde_json::Value)) -> Result<Request> {
        let mut v = sim_value();
        edit(&mut v);
        parse(&v.to_string())
    }

    fn sim_error(edit: impl FnOnce(&mut serde_json::Value)) -> String {
        sim_with(edit).unwrap_err().to_string()
    }

    #[test]
    fn a_sim_request_parses_and_trims_its_name() {
        let Request::Sim(s) = sim_with(|_| {}).unwrap() else { panic!() };
        assert_eq!(s.name, "Lockout A/B");
        assert_eq!(s.arms[1].features.as_deref(), Some("-impl.sim.native"));
        assert_eq!(s.repeat, 3);
        assert!(!s.batch && s.parallel.is_none());
        // Unset options are not sent, so the wire form stays small.
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("parallel").is_none() && v.get("mem_limit").is_none());
    }

    #[test]
    fn a_sim_request_is_checked_before_a_job_exists() {
        use serde_json::json;
        assert!(sim_error(|v| v["repeat"] = json!(0)).contains("--repeat"));
        assert!(sim_error(|v| v["repeat"] = json!(101)).contains("--repeat"));
        assert!(sim_error(|v| v["parallel"] = json!(2)).contains("batch"));
        assert!(sim_error(|v| {
            v["batch"] = json!(true);
            v["parallel"] = json!(0);
        })
        .contains("--parallel"));
        assert!(sim_error(|v| v["env"] = json!({"H2_SIM": "/etc/x"})).contains("not a run variable"));
        assert!(sim_error(|v| v["build"] = json!({"features": ["x"]})).contains("unknown feature"));
        // Files are plain relative paths that exist in the upload.
        assert!(sim_error(|v| v["file"] = json!("../m.toml")).contains("must be 1-64"));
        assert!(sim_error(|v| v["file"] = json!("n.toml")).contains("not among"));
        assert!(
            sim_error(|v| v["files"] = json!({"m.toml": "x", "/etc/passwd": "y"}))
                .contains("must be 1-64")
        );
        assert!(
            sim_error(|v| v["files"] = json!({"m.toml": "x", "a/../b": "y"}))
                .contains("must be 1-64")
        );
        assert!(sim_error(|v| v["arms"][1]["name"] = json!("on")).contains("named twice"));
        assert!(sim_error(|v| v["arms"] = json!([])).contains("1 to 8 arms"));
        assert!(
            sim_error(|v| v["files"]["m.toml"] = json!("x".repeat(SIM_BYTES_MAX + 1)))
                .contains("MiB")
        );
        // The build's variables still need a rev, as for a run.
        assert!(
            sim_error(|v| v["build"] = json!({"env": {"H2_OPT": "2"}}))
                .contains("need a branch or commit")
        );
        sim_with(|v| {
            v["build"] = json!({"rev": "main", "env": {"H2_OPT": "2"}});
            v["env"] = json!({"H2_FEATURES": "x"});
        })
        .unwrap();
    }

    #[test]
    fn a_sim_requests_bot_data_is_plain_bots_files_under_a_cap() {
        use serde_json::json;
        let Request::Sim(s) = sim_with(|v| {
            v["data"] = json!({"bots/lockout.0.bots": "00ff", "bots/lockout.0.vis": "ab"});
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(s.data.len(), 2);
        for bad in [
            "lockout.0.bots",
            "bots/../x.bots",
            "bots/a/b.bots",
            "bots/lockout.0.exe",
            "/bots/x.bots",
            "bots/.hidden.bots",
        ] {
            let e = sim_error(|v| v["data"] = json!({ bad: "00" }));
            assert!(e.contains("sim data"), "{bad}: {e}");
        }
        assert!(sim_error(|v| v["data"] = json!({"bots/a.0.bots": "abc"})).contains("not hex"));
        let many: serde_json::Map<String, serde_json::Value> = (0..=SIM_DATA_FILES_MAX)
            .map(|i| (format!("bots/m{i}.0.bots"), json!("00")))
            .collect();
        assert!(sim_error(|v| v["data"] = many.into()).contains("sim data files"));
        let Request::Sim(mut s) = sim_with(|_| {}).unwrap() else { panic!() };
        s.data.insert("bots/a.0.bots".into(), "00".repeat(SIM_DATA_MAX + 1));
        let e = validate_sim(&s).unwrap_err().to_string();
        assert!(e.contains("sim data over 60 MiB"), "{e}");
    }
}
