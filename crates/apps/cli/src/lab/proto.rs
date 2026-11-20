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

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Protocol version, in `status`.
pub const VERSION: u32 = 1;

/// Run-time environment a request may set. Paths the game writes (frame
/// stats CSV, input recording, profile directory) are the host's to choose.
pub const RUN_ENV: &[&str] = &[
    "H2_FRAME_STATS",
    "H2_FRAME_STATS_FROM",
    "H2_BOOT_STOP_AT_PRESENT",
    "H2_PERF_FROM",
    "H2_PROFILE_HZ",
    "H2_HOST_PERF",
    "H2_AUDIO_STATS",
    "H2_FPS",
    "H2_RENDER_SCALE",
    "H2_WATERMARK",
    "H2_MAPPACK",
    "H2_HALO1",
    "H2_FEATURES",
    "H2_LOCKSTEP_TICKS",
    "H2_ICONS",
];

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
    pub features: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RunReq {
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
    /// Profile into the run's profile/ (`H2_PROFILE`, needs `perf`).
    pub perf: bool,
    pub env: BTreeMap<String, String>,
    /// Extra `h2 play` flags from [`PLAY_FLAGS`].
    pub args: Vec<String>,
    pub build: BuildSpec,
    /// An empty data directory for this run only, removed after it.
    pub fresh_data: bool,
    /// Free text kept in the run's metadata.
    pub note: Option<String>,
    /// Driven through the agent API (`h2 lab agent`) until `stop`: the game
    /// never stops by itself, so `frames` can't be set (docs/runtime/lab.md).
    pub serve: bool,
}

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

/// The input log's file name inside a recording.
pub const INPUT_FILE: &str = "input.bin";

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Update,
    Fetch {
        id: String,
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
    let r: Request = serde_json::from_str(line)?;
    validate(&r)?;
    Ok(r)
}

pub fn validate(r: &Request) -> Result<()> {
    match r {
        Request::Status | Request::Update | Request::Recordings | Request::Runs { .. } => Ok(()),
        Request::Fetch { id } => {
            check_name("id", id)?;
            ensure!(id.bytes().all(|b| b.is_ascii_digit()), "invalid job id");
            Ok(())
        }
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

/// What follows `h2 agent` in an `agent` request. Any call goes through
/// (the host does not know the methods); what is refused is naming a
/// different socket or console, which the run's own socket replaces, and
/// the two forms that read files on the host instead of talking to a game.
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
        !matches!(args[0].as_str(), "flight" | "raw-diff"),
        "`h2 agent {}` reads files on the machine it runs on; the lab doesn't forward it",
        args[0]
    );
    Ok(())
}

pub fn validate_run(r: &RunReq) -> Result<()> {
    ensure!(
        !(r.serve && r.frames.is_some()),
        "a --serve run never stops by itself; --frames can't be set (h2 lab stop ends it)"
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
    }
    check_play_args(&r.args)?;
    if let Some(n) = &r.note {
        ensure!(n.len() <= 1024, "note over 1024 bytes");
    }
    validate_build(&r.build)
}

pub fn validate_build(b: &BuildSpec) -> Result<()> {
    if let Some(rev) = &b.rev {
        check_rev(rev)?;
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
    fn a_full_run_request_parses() {
        let r = run(
            r#"{"cmd":"run","level":"03a_oldmombasa","difficulty":"heroic","frames":900,
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
            run(r#"{"cmd":"run","h2":"/bin/sh"}"#).is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn run_env_is_an_allowlist() {
        for k in [
            "H2_FRAME_STATS_CSV",
            "H2_INPUT_REPLAY",
            "H2_PROFILE",
            "H2_AGENT",
            "PATH",
            "DYLD_INSERT_LIBRARIES",
            "H2_OPT",
        ] {
            let j = format!(r#"{{"cmd":"run","env":{{"{k}":"x"}}}}"#);
            assert!(run(&j).is_err(), "{k} must be refused");
        }
        assert!(run(r#"{"cmd":"run","env":{"H2_FRAME_STATS_FROM":"300"}}"#).is_ok());
        assert!(run(r#"{"cmd":"run","env":{"H2_FPS":"6\n0"}}"#).is_err());
    }

    #[test]
    fn build_env_is_an_allowlist_and_needs_a_rev() {
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"H2_OPT":"2","H2_PGO":"use:city.profdata"}}"#).is_ok());
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"CC":"evil"}}"#).is_err());
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"H2_PGO":"use:/tmp/x"}}"#).is_err());
        assert!(run(r#"{"cmd":"build","rev":"master","env":{"H2_PGO":"use:../x"}}"#).is_err());
        assert!(run(r#"{"cmd":"build","rev":"master","features":["metal-debug"]}"#).is_err());
        assert!(
            run(r#"{"cmd":"run","build":{"env":{"H2_OPT":"2"}}}"#).is_err(),
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
        assert!(run(r#"{"cmd":"run","replay":"../../.ssh/id_ed25519"}"#).is_err());
        assert!(run(r#"{"cmd":"run","record":"a/b"}"#).is_err());
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
    fn agent_calls_pass_through_but_not_sockets_or_file_readers() {
        let ok = r#"{"cmd":"agent","id":"0003","args":["capture","600"],"timeout":90}"#;
        assert!(run(ok).is_ok());
        assert!(
            run(r#"{"cmd":"agent","id":"0003","args":["session.wait","{\"until\":{\"phase\":\"gameplay\"}}"]}"#)
                .is_ok()
        );
        for bad in [
            r#"{"cmd":"agent","id":"0003","args":[]}"#,
            r#"{"cmd":"agent","id":"../x","args":["status"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["status","--socket","/tmp/x"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["status","--xbox"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["flight","show","/etc/passwd"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["raw-diff","a","b"]}"#,
            r#"{"cmd":"agent","id":"0003","args":["status"],"timeout":0}"#,
            r#"{"cmd":"agent","id":"0003","args":["status"],"timeout":-1}"#,
        ] {
            assert!(run(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_serve_run_has_no_frame_limit() {
        assert!(run(r#"{"cmd":"run","level":"x","serve":true}"#).is_ok());
        assert!(run(r#"{"cmd":"run","level":"x","serve":true,"frames":10}"#).is_err());
    }

    #[test]
    fn levels_and_difficulties() {
        assert!(run(r#"{"cmd":"run","level":"03a_oldmombasa","difficulty":"legendary"}"#).is_ok());
        assert!(run(r#"{"cmd":"run","level":"../x"}"#).is_err());
        assert!(run(r#"{"cmd":"run","level":"x","difficulty":"impossible"}"#).is_err());
        assert!(run(r#"{"cmd":"run","difficulty":"easy"}"#).is_err());
        assert!(run(r#"{"cmd":"run","replay":"a","record":"b"}"#).is_err());
    }
}
