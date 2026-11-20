//! `h2 lab host`: the lab's side on the machine that plays. One job at a
//! time (a build, or a run that may build first), each in `runs/<id>/` with
//! its log and metadata. See docs/runtime/lab.md.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::proto::{self, BuildSpec, INPUT_FILE, Manifest, Request, RunReq};

/// Where the host keeps everything, and what it runs.
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub root: PathBuf,
    pub socket: PathBuf,
    /// The checkout builds come from (git worktrees of its commits).
    pub repo: PathBuf,
    pub disc: PathBuf,
    /// The `h2` for `--current` runs (default: this one).
    pub h2: PathBuf,
    /// The image directory for `--current` runs (default: `h2 play`'s).
    pub out: Option<PathBuf>,
    /// The data root runs share (default: `<root>/data`).
    pub data: PathBuf,
    pub cargo: PathBuf,
    /// SIGTERM to SIGKILL on stop.
    pub grace: Duration,
}

impl HostConfig {
    pub fn dir(&self, sub: &str) -> PathBuf {
        self.root.join(sub)
    }
}

/// A job's metadata, `runs/<id>/meta.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub id: String,
    /// `run` or `build`.
    pub kind: String,
    /// queued, building, running, then done, failed or stopped.
    pub state: String,
    pub request: Value,
    pub commit: Option<String>,
    /// Image cache key (`images/<key>`), for builds by commit.
    pub image: Option<String>,
    /// The image was already built.
    pub cached: Option<bool>,
    pub build_env: BTreeMap<String, String>,
    pub level: Option<String>,
    pub created: u64,
    pub duration_s: Option<f64>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub error: Option<String>,
    /// The frame stats report lines from the log.
    pub summary: Vec<String>,
    pub notes: Vec<String>,
    /// The game's agent API socket, for `h2 lab agent` (runs only).
    pub agent_socket: Option<String>,
    /// The `h2` that runs it, which also makes the agent calls.
    pub h2: Option<String>,
}

fn is_final(state: &str) -> bool {
    matches!(state, "done" | "failed" | "stopped")
}

struct Active {
    id: String,
    pgid: Option<i32>,
    stop: bool,
}

#[derive(Default)]
struct Inner {
    active: Option<Active>,
    jobs: HashMap<String, Meta>,
}

struct Shared {
    cfg: HostConfig,
    inner: Mutex<Inner>,
    cv: Condvar,
}

/// A job ended because it was asked to stop.
#[derive(Debug)]
struct Stopped;
impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}
impl std::error::Error for Stopped {}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Serves the lab socket until the process ends.
pub fn serve(cfg: HostConfig) -> Result<()> {
    for d in ["runs", "recordings", "pgo", "images", "src", "scratch", "a"] {
        fs::create_dir_all(cfg.dir(d))
            .with_context(|| format!("creating {}", cfg.dir(d).display()))?;
    }
    fs::create_dir_all(&cfg.data)?;
    let listener = bind(&cfg.socket)?;
    // A stable path to this h2 for `ssh HOST <root>/h2 lab pipe`.
    let link = cfg.root.join("h2");
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(&link);
        let _ = std::os::unix::fs::symlink(exe, &link);
    }
    let shared = Arc::new(Shared {
        cfg,
        inner: Mutex::new(Inner::default()),
        cv: Condvar::new(),
    });
    recover(&shared);
    eprintln!(
        "lab: listening on {} (root {}, repo {}, disc {})",
        shared.cfg.socket.display(),
        shared.cfg.root.display(),
        shared.cfg.repo.display(),
        shared.cfg.disc.display()
    );
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let sh = Arc::clone(&shared);
        std::thread::spawn(move || {
            let _ = connection(&sh, conn);
        });
    }
    Ok(())
}

/// Binds `path` with mode 0600, replacing a stale socket but never a live
/// host's.
pub fn bind(path: &Path) -> Result<UnixListener> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("a lab host is already listening on {}", path.display());
        }
        fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    let l = UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// Jobs a previous host left unfinished are failed: nothing watches them now.
fn recover(sh: &Shared) {
    for m in disk_metas(&sh.cfg) {
        if !is_final(&m.state) {
            let mut m = m;
            m.state = "failed".into();
            m.error = Some("the host stopped while this job ran".into());
            let _ = write_meta(&sh.cfg, &m);
        }
    }
}

fn connection(sh: &Arc<Shared>, conn: UnixStream) -> Result<()> {
    let mut w = conn.try_clone()?;
    for line in BufReader::new(conn).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match proto::parse(&line).and_then(|r| handle(sh, r)) {
            Ok(mut v) => {
                v["ok"] = Value::Bool(true);
                v
            }
            Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
        };
        writeln!(w, "{reply}")?;
        w.flush()?;
    }
    Ok(())
}

fn handle(sh: &Arc<Shared>, r: Request) -> Result<Value> {
    let cfg = &sh.cfg;
    match r {
        Request::Status => {
            let inner = sh.inner.lock().unwrap();
            let active = inner.active.as_ref().and_then(|a| inner.jobs.get(&a.id));
            Ok(json!({
                "version": proto::VERSION,
                "root": cfg.root,
                "repo": cfg.repo,
                "disc": cfg.disc,
                "h2": cfg.h2,
                "active": active,
            }))
        }
        Request::Run(run) => start(sh, Job::Run(run)),
        Request::Build(b) => start(sh, Job::Build(b)),
        Request::Wait { id, timeout } => wait(sh, &id, timeout.map(Duration::from_secs_f64)),
        Request::Stop { id } => stop(sh, &id),
        Request::Agent {
            id,
            args,
            timeout,
            stdin,
        } => agent_call(sh, &id, &args, timeout, stdin),
        Request::Runs { limit } => {
            let mut all = disk_metas(cfg);
            let inner = sh.inner.lock().unwrap();
            for m in &mut all {
                if let Some(live) = inner.jobs.get(&m.id) {
                    *m = live.clone();
                }
            }
            let skip = limit.map_or(0, |n| all.len().saturating_sub(n));
            Ok(json!({ "runs": &all[skip..] }))
        }
        Request::Recordings => {
            let mut v = Vec::new();
            for e in fs::read_dir(cfg.dir("recordings"))?.flatten() {
                if let Ok(m) = read_manifest(&e.path()) {
                    v.push(m);
                }
            }
            v.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(json!({ "recordings": v }))
        }
        Request::Put {
            name,
            kind,
            hex,
            level,
            difficulty,
            commit,
        } => {
            let bytes = proto::from_hex(&hex)?;
            if kind.as_deref() == Some("pgo") {
                let p = cfg.dir("pgo").join(&name);
                ensure!(!p.exists(), "pgo/{name} exists; put it under another name");
                fs::write(&p, &bytes)?;
                return Ok(json!({ "path": p, "bytes": bytes.len() }));
            }
            let dir = cfg.dir("recordings").join(&name);
            ensure!(
                !dir.exists(),
                "recording {name} exists; put it under another name"
            );
            fs::create_dir(&dir)?;
            fs::write(dir.join(INPUT_FILE), &bytes)?;
            let m = Manifest {
                name: name.clone(),
                level,
                difficulty,
                commit,
                input: INPUT_FILE.into(),
                snapshot: None,
                created: now(),
                run: None,
            };
            fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&m)?)?;
            Ok(json!({ "recording": m, "bytes": bytes.len() }))
        }
    }
}

enum Job {
    Run(RunReq),
    Build(BuildSpec),
}

impl Job {
    fn spec(&self) -> &BuildSpec {
        match self {
            Job::Run(r) => &r.build,
            Job::Build(b) => b,
        }
    }
}

fn start(sh: &Arc<Shared>, job: Job) -> Result<Value> {
    let cfg = &sh.cfg;
    // Everything that can be refused is refused before a job exists.
    let commit = match &job.spec().rev {
        Some(rev) => Some(resolve_rev(&cfg.repo, rev)?),
        None => None,
    };
    if let Some(name) = proto::pgo_from(&job.spec().env)? {
        ensure!(
            cfg.dir("pgo").join(name).is_file(),
            "no PGO profile pgo/{name} (h2 lab put --pgo FILE)"
        );
    }
    let mut level = None;
    if let Job::Run(r) = &job {
        level = r.level.clone();
        if let Some(name) = &r.replay {
            let m = read_manifest(&cfg.dir("recordings").join(name))
                .with_context(|| format!("no recording {name} (h2 lab recordings)"))?;
            ensure!(
                m.snapshot.is_none(),
                "recording {name} starts from a snapshot, which this host can't restore yet"
            );
            level = level.or(m.level);
        }
        if let Some(name) = &r.record {
            ensure!(
                !cfg.dir("recordings").join(name).exists(),
                "recording {name} exists; pick another name"
            );
        }
        ensure!(
            level.is_some() || r.replay.is_none(),
            "replaying needs a level (the recording names none)"
        );
    }
    let mut inner = sh.inner.lock().unwrap();
    if let Some(a) = &inner.active {
        let state = inner.jobs.get(&a.id).map_or("busy", |m| m.state.as_str());
        bail!(
            "busy: job {} is {state}; one job at a time (h2 lab wait {0} or h2 lab stop {0})",
            a.id
        );
    }
    let id = next_id(cfg, &inner);
    fs::create_dir_all(cfg.dir("runs").join(&id))?;
    let (kind, request) = match &job {
        Job::Run(r) => ("run", serde_json::to_value(r)?),
        Job::Build(b) => ("build", serde_json::to_value(b)?),
    };
    let m = Meta {
        id: id.clone(),
        kind: kind.into(),
        state: "queued".into(),
        request,
        commit: commit.clone(),
        build_env: job.spec().env.clone(),
        level,
        created: now(),
        ..Meta::default()
    };
    write_meta(cfg, &m)?;
    inner.jobs.insert(id.clone(), m.clone());
    inner.active = Some(Active {
        id: id.clone(),
        pgid: None,
        stop: false,
    });
    drop(inner);
    let sh2 = Arc::clone(sh);
    let id2 = id.clone();
    std::thread::spawn(move || execute(&sh2, &id2, job, commit));
    Ok(json!({ "id": id, "job": m }))
}

fn next_id(cfg: &HostConfig, inner: &Inner) -> String {
    let on_disk = fs::read_dir(cfg.dir("runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()));
    let in_mem = inner.jobs.keys().filter_map(|k| k.parse::<u64>().ok());
    format!("{:04}", on_disk.chain(in_mem).max().map_or(1, |n| n + 1))
}

fn resolve_rev(repo: &Path, rev: &str) -> Result<String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .context("running git")?;
    ensure!(
        o.status.success(),
        "{rev} is not a commit in {} (h2 lab sync pushes branches as lab/<name>)",
        repo.display()
    );
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

impl Shared {
    fn update(&self, id: &str, f: impl FnOnce(&mut Meta)) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(m) = inner.jobs.get_mut(id) {
            f(m);
            let _ = write_meta(&self.cfg, m);
        }
        drop(inner);
        self.cv.notify_all();
    }

    fn stop_requested(&self, id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.active.as_ref().is_some_and(|a| a.id == id && a.stop)
    }

    /// Runs one step of job `id` in its own process group, so stop reaches
    /// what it starts (cargo's rustc, the game's helpers) too.
    fn step(&self, id: &str, mut cmd: Command, log: &File) -> Result<std::process::ExitStatus> {
        if self.stop_requested(id) {
            return Err(Stopped.into());
        }
        cmd.stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?)
            .process_group(0);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {:?}", cmd.get_program()))?;
        let pgid = child.id() as i32;
        let stop_now = {
            let mut inner = self.inner.lock().unwrap();
            match inner.active.as_mut() {
                Some(a) if a.id == id => {
                    a.pgid = Some(pgid);
                    a.stop
                }
                _ => false,
            }
        };
        if stop_now {
            // SAFETY: signalling the group we just made.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };
        }
        let status = child.wait()?;
        let stopped = {
            let mut inner = self.inner.lock().unwrap();
            match inner.active.as_mut() {
                Some(a) if a.id == id => {
                    a.pgid = None;
                    a.stop
                }
                _ => false,
            }
        };
        // Whatever the group left behind goes with it.
        // SAFETY: as above; the group may be gone already.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        if stopped {
            return Err(Stopped.into());
        }
        Ok(status)
    }
}

/// A built image: the `h2` to run it with and its environment.
struct Image {
    h2: PathBuf,
    out: Option<PathBuf>,
    env: BTreeMap<String, String>,
}

fn execute(sh: &Arc<Shared>, id: &str, job: Job, commit: Option<String>) {
    let t = Instant::now();
    let r = run_job(sh, id, &job, commit.as_deref());
    let (state, error) = match &r {
        Ok(Some(st)) if st.success() => ("done", None),
        Ok(Some(st)) => (
            "failed",
            Some(match (st.code(), st.signal()) {
                (Some(c), _) => format!("exit {c}"),
                (_, Some(s)) => format!("signal {s}"),
                _ => "failed".into(),
            }),
        ),
        Ok(None) => ("done", None),
        Err(e) if e.is::<Stopped>() => ("stopped", None),
        Err(e) => ("failed", Some(format!("{e:#}"))),
    };
    let run_dir = sh.cfg.dir("runs").join(id);
    let summary = fs::read_to_string(run_dir.join("run.log"))
        .map(|l| summarize(&l))
        .unwrap_or_default();
    let status = r.as_ref().ok().copied().flatten();
    // A stopped game exits through its SIGTERM handler or the signal.
    let state = if state == "failed" && sh.stop_requested(id) {
        "stopped"
    } else {
        state
    };
    let _ = fs::remove_dir_all(sh.cfg.dir("scratch").join(id));
    let _ = fs::remove_file(agent_socket_path(&sh.cfg, id));
    sh.update(id, |m| {
        m.state = state.into();
        m.error = error;
        m.duration_s = Some(t.elapsed().as_secs_f64());
        m.exit_code = status.and_then(|s| s.code());
        m.signal = status.and_then(|s| s.signal());
        m.summary = summary;
    });
    let mut inner = sh.inner.lock().unwrap();
    if inner.active.as_ref().is_some_and(|a| a.id == id) {
        inner.active = None;
    }
    drop(inner);
    sh.cv.notify_all();
}

/// The build (when the job names a commit) and the run; `None` for a build
/// job, else the game's exit status.
fn run_job(
    sh: &Arc<Shared>,
    id: &str,
    job: &Job,
    commit: Option<&str>,
) -> Result<Option<std::process::ExitStatus>> {
    let cfg = &sh.cfg;
    let run_dir = cfg.dir("runs").join(id);
    let image = match commit {
        Some(c) => {
            sh.update(id, |m| m.state = "building".into());
            let log = File::create(run_dir.join("build.log"))?;
            ensure_image(sh, id, job.spec(), c, &log)?
        }
        None => Image {
            h2: cfg.h2.clone(),
            out: cfg.out.clone(),
            env: BTreeMap::new(),
        },
    };
    let Job::Run(run) = job else { return Ok(None) };
    let log = File::create(run_dir.join("run.log"))?;
    let cmd = play_command(cfg, id, run, &image, commit)?;
    let sock = agent_socket_path(cfg, id);
    sh.update(id, |m| {
        m.state = "running".into();
        m.agent_socket = Some(sock.display().to_string());
        m.h2 = Some(image.h2.display().to_string());
    });
    let st = sh.step(id, cmd, &log)?;
    if let Some(name) = &run.record {
        let input = cfg.dir("recordings").join(name).join(INPUT_FILE);
        if !input.is_file() {
            sh.update(id, |m| {
                m.notes
                    .push(format!("recording {name}: the game wrote no input log"))
            });
        }
    }
    Ok(Some(st))
}

/// `h2 play` for `run` with `image`, its environment cleaned of the host's
/// own `H2_*` variables so a run means the same whatever terminal the host
/// started in. Writes the recording's manifest when it records.
fn play_command(
    cfg: &HostConfig,
    id: &str,
    run: &RunReq,
    image: &Image,
    commit: Option<&str>,
) -> Result<Command> {
    let run_dir = cfg.dir("runs").join(id);
    let mut cmd = Command::new(&image.h2);
    cmd.arg("play").arg(&cfg.disc);
    if let Some(out) = &image.out {
        cmd.arg("--out").arg(out);
    }
    let data = if run.fresh_data {
        let d = cfg.dir("scratch").join(id).join("data");
        fs::create_dir_all(&d)?;
        d
    } else {
        cfg.data.clone()
    };
    cmd.arg("--data").arg(data);
    let mut level = run.level.clone();
    let mut difficulty = run.difficulty.clone();
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cmd.env_remove(k);
        }
    }
    cmd.envs(&image.env);
    if run.stats != Some(false) {
        cmd.env("H2_FRAME_STATS", "1")
            .env("H2_FRAME_STATS_CSV", run_dir.join("frames.csv"));
    }
    if let Some(n) = run.frames {
        cmd.env("H2_BOOT_STOP_AT_PRESENT", n.to_string());
    }
    if let Some(n) = run.stats_from {
        cmd.env("H2_FRAME_STATS_FROM", n.to_string());
    }
    if let Some(name) = &run.replay {
        let dir = cfg.dir("recordings").join(name);
        let m = read_manifest(&dir)?;
        if level.is_none() {
            level = m.level.clone();
            difficulty = difficulty.or(m.difficulty.clone());
        }
        proto::check_name("input", &m.input)?;
        cmd.env("H2_INPUT_REPLAY", dir.join(&m.input));
    }
    if let Some(name) = &run.record {
        let dir = cfg.dir("recordings").join(name);
        fs::create_dir_all(&dir)?;
        let m = Manifest {
            name: name.clone(),
            level: level.clone(),
            difficulty: difficulty.clone(),
            commit: commit.map(Into::into),
            input: INPUT_FILE.into(),
            snapshot: None,
            created: now(),
            run: Some(id.into()),
        };
        fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&m)?)?;
        cmd.env("H2_INPUT_RECORD", dir.join(INPUT_FILE));
    }
    if run.perf {
        cmd.env("H2_PROFILE", run_dir.join("profile"));
        cmd.env("H2_PERF_FROM", "level");
    }
    cmd.envs(&run.env);
    // Every run answers the agent API, for `h2 lab agent`. After the
    // run's own variables, which can't name it (proto::RUN_ENV).
    let sock = agent_socket_path(cfg, id);
    if let Some(dir) = sock.parent() {
        fs::create_dir_all(dir)?;
    }
    let _ = fs::remove_file(&sock);
    cmd.env(agent_proto::ENV, &sock);
    if let Some(l) = &level {
        cmd.arg("--level").arg(l);
        if let Some(d) = &difficulty {
            cmd.arg("--difficulty").arg(d);
        }
    }
    cmd.args(&run.args);
    cmd.current_dir(&run_dir);
    Ok(cmd)
}

/// Where run `id`'s game listens for the agent API: `<root>/a/<id>.sock`,
/// outside `runs/<id>` so a fetch never tars a socket, and short because a
/// Unix socket path is limited to about 100 bytes. A root too deep for that
/// gets `/tmp/h2lab-<hash of root>-<id>.sock` instead.
pub fn agent_socket_path(cfg: &HostConfig, id: &str) -> PathBuf {
    let p = cfg.dir("a").join(format!("{id}.sock"));
    if p.as_os_str().len() <= 100 {
        return p;
    }
    let h = hex_sha(cfg.root.as_os_str().as_encoded_bytes());
    PathBuf::from(format!("/tmp/h2lab-{}-{id}.sock", &h[..8]))
}

/// `agent`: runs `h2 agent ARGS --socket <the run's socket>` and answers
/// with its exit code and output, so every `h2 agent` call works as it does
/// on this machine and nothing is special-cased here. The `h2` is the
/// run's own image's, so a short form that branch knows is understood.
fn agent_call(
    sh: &Arc<Shared>,
    id: &str,
    args: &[String],
    timeout: Option<f64>,
    stdin: Option<String>,
) -> Result<Value> {
    let secs = timeout.unwrap_or(proto::AGENT_TIMEOUT);
    // Up to two minutes for a just-started game to open its socket.
    let ready = Instant::now() + Duration::from_secs(120);
    let (sock, h2) = loop {
        let m = {
            let inner = sh.inner.lock().unwrap();
            inner.jobs.get(id).cloned()
        };
        let m = match m {
            Some(m) => m,
            None => read_meta(&sh.cfg, id)?,
        };
        ensure!(m.kind == "run", "job {id} is a {}, not a run", m.kind);
        match m.state.as_str() {
            "queued" | "building" => bail!(
                "job {id} is {} (h2 lab wait {id} --timeout 5, until it is running)",
                m.state
            ),
            "running" => {}
            s => bail!("run {id} is {s}: its game has exited"),
        }
        let Some(sock) = m.agent_socket.as_deref().map(PathBuf::from) else {
            bail!("run {id} has no agent socket recorded");
        };
        if sock.exists() {
            let h2 = m.h2.map_or_else(|| sh.cfg.h2.clone(), PathBuf::from);
            break (sock, h2);
        }
        ensure!(
            Instant::now() < ready,
            "run {id}'s game opened no agent socket at {} in 120 s (see runs/{id}/run.log)",
            sock.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut cmd = Command::new(&h2);
    cmd.arg("agent")
        .args(args)
        .arg("--socket")
        .arg(&sock)
        .arg("--timeout")
        .arg(secs.to_string());
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cmd.env_remove(k);
        }
    }
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting {}", h2.display()))?;
    if let (Some(text), Some(mut w)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = w.write_all(text.as_bytes());
        });
    }
    let out = drain(child.stdout.take().context("no stdout")?);
    let err = drain(child.stderr.take().context("no stderr")?);
    // The call has its own socket timeout; this is the backstop.
    let deadline = Instant::now() + Duration::from_secs_f64(secs) + Duration::from_secs(15);
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("h2 agent {} did not finish in {secs} s", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(json!({
        "exit": status.code().unwrap_or(1),
        "stdout": out.join().unwrap_or_default(),
        "stderr": err.join().unwrap_or_default(),
    }))
}

fn drain(r: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = r.take(AGENT_OUT_MAX).read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    })
}

/// Most output of one `agent` call kept, in bytes.
const AGENT_OUT_MAX: u64 = 64 << 20;

/// `h2 lab record` with no host running: the same `h2 play` as a host run
/// with the host's own image, in this terminal, kept in `runs/<id>`.
pub fn run_direct(cfg: &HostConfig, run: &RunReq) -> Result<std::process::ExitStatus> {
    for d in ["runs", "recordings"] {
        fs::create_dir_all(cfg.dir(d))?;
    }
    fs::create_dir_all(&cfg.data)?;
    if let Some(name) = &run.record {
        ensure!(
            !cfg.dir("recordings").join(name).exists(),
            "recording {name} exists; pick another name"
        );
    }
    let id = next_id(cfg, &Inner::default());
    fs::create_dir_all(cfg.dir("runs").join(&id))?;
    let image = Image {
        h2: cfg.h2.clone(),
        out: cfg.out.clone(),
        env: BTreeMap::new(),
    };
    let mut cmd = play_command(cfg, &id, run, &image, None)?;
    let t = Instant::now();
    let st = cmd
        .status()
        .with_context(|| format!("starting {}", cfg.h2.display()))?;
    let m = Meta {
        id,
        kind: "run".into(),
        state: if st.success() { "done" } else { "failed" }.into(),
        request: serde_json::to_value(run)?,
        level: run.level.clone(),
        created: now(),
        duration_s: Some(t.elapsed().as_secs_f64()),
        exit_code: st.code(),
        signal: st.signal(),
        ..Meta::default()
    };
    write_meta(cfg, &m)?;
    let _ = fs::remove_dir_all(cfg.dir("scratch").join(&m.id));
    Ok(st)
}

/// The image for `spec` at `commit`: from `images/<key>` when built before,
/// else built there: a detached worktree of the commit (`src/<commit>`),
/// `cargo build` of its `h2` into the shared `target/`, a copy of that `h2`,
/// and `h2 build` with the build environment and a shared object cache.
fn ensure_image(
    sh: &Shared,
    id: &str,
    spec: &BuildSpec,
    commit: &str,
    log: &File,
) -> Result<Image> {
    let cfg = &sh.cfg;
    let mut env = spec.env.clone();
    let mut pgo_hash = String::new();
    if let Some(name) = proto::pgo_from(&spec.env)? {
        let p = cfg.dir("pgo").join(name);
        pgo_hash = hex_sha(&fs::read(&p).with_context(|| format!("reading {}", p.display()))?);
        env.insert("H2_PGO".into(), format!("use:{}", p.display()));
    }
    let mut features = spec.features.clone();
    features.sort();
    features.dedup();
    let key = image_key(commit, &features, &spec.env, &pgo_hash);
    let dir = cfg.dir("images").join(&key);
    let image = Image {
        h2: dir.join("h2"),
        out: Some(dir.join("out")),
        env,
    };
    let mut w = log.try_clone()?;
    if dir.join("ready").is_file() {
        writeln!(w, "lab: image {key} for {commit} is built")?;
        sh.update(id, |m| {
            m.image = Some(key);
            m.cached = Some(true);
        });
        return Ok(image);
    }
    sh.update(id, |m| {
        m.image = Some(key.clone());
        m.cached = Some(false);
    });
    let src = cfg.dir("src").join(commit);
    if !src.join(".git").exists() {
        writeln!(w, "lab: worktree {} at {commit}", src.display())?;
        let _ = fs::remove_dir_all(&src);
        let _ = Command::new("git")
            .arg("-C")
            .arg(&cfg.repo)
            .args(["worktree", "prune"])
            .status();
        let mut git = Command::new("git");
        git.arg("-C")
            .arg(&cfg.repo)
            .args(["worktree", "add", "--detach"])
            .arg(&src)
            .arg(commit)
            // The builds need no Git LFS pictures; never fetch them.
            .env("GIT_LFS_SKIP_SMUDGE", "1");
        let st = sh.step(id, git, log)?;
        ensure!(st.success(), "git worktree add failed (build.log)");
    }
    writeln!(w, "lab: cargo build of h2 at {commit}")?;
    let target = cfg.dir("target");
    let mut cargo = Command::new(&cfg.cargo);
    cargo.args(["build", "--release", "-p", "cli", "--bin", "h2"]);
    if !features.is_empty() {
        cargo.arg("--features").arg(features.join(","));
    }
    cargo
        .current_dir(&src)
        .env("CARGO_TARGET_DIR", &target)
        .envs(&image.env);
    let st = sh.step(id, cargo, log)?;
    ensure!(st.success(), "cargo build failed (build.log)");
    fs::create_dir_all(&dir)?;
    fs::copy(target.join("release/h2"), &image.h2).context("copying the built h2")?;
    writeln!(w, "lab: h2 build into {}", dir.join("out").display())?;
    let mut build = Command::new(&image.h2);
    build
        .arg("build")
        .arg(&cfg.disc)
        .arg("--out")
        .arg(dir.join("out"));
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            build.env_remove(k);
        }
    }
    build
        .envs(&image.env)
        .env("H2_OBJ_CACHE", cfg.dir("obj-cache"))
        .current_dir(&dir);
    let st = sh.step(id, build, log)?;
    ensure!(st.success(), "h2 build failed (build.log)");
    let info = json!({ "commit": commit, "features": features, "env": spec.env, "pgo_sha256": pgo_hash, "built": now() });
    fs::write(dir.join("image.json"), serde_json::to_vec_pretty(&info)?)?;
    fs::write(dir.join("ready"), b"")?;
    Ok(image)
}

/// The image cache key: commit, features and build environment (with the
/// PGO profile's contents, not its name).
pub fn image_key(
    commit: &str,
    features: &[String],
    env: &BTreeMap<String, String>,
    pgo_hash: &str,
) -> String {
    let mut s = format!("commit {commit}\n");
    for f in features {
        s += &format!("feature {f}\n");
    }
    for (k, v) in env {
        s += &format!("env {k}={v}\n");
    }
    s += &format!("pgo {pgo_hash}\n");
    hex_sha(s.as_bytes())[..16].to_string()
}

fn hex_sha(b: &[u8]) -> String {
    proto::to_hex(&Sha256::digest(b))
}

fn wait(sh: &Shared, id: &str, timeout: Option<Duration>) -> Result<Value> {
    let deadline = timeout.map(|t| Instant::now() + t);
    let mut inner = sh.inner.lock().unwrap();
    loop {
        let m = match inner.jobs.get(id) {
            Some(m) => m.clone(),
            None => read_meta(&sh.cfg, id)?,
        };
        if is_final(&m.state) {
            return Ok(json!({ "job": m, "finished": true }));
        }
        let left = match deadline {
            Some(d) => match d.checked_duration_since(Instant::now()) {
                Some(l) if !l.is_zero() => l,
                _ => return Ok(json!({ "job": m, "finished": false })),
            },
            None => Duration::from_secs(3600),
        };
        inner = sh.cv.wait_timeout(inner, left).unwrap().0;
    }
}

fn stop(sh: &Arc<Shared>, id: &str) -> Result<Value> {
    let mut inner = sh.inner.lock().unwrap();
    let Some(a) = inner.active.as_mut().filter(|a| a.id == id) else {
        let m = inner
            .jobs
            .get(id)
            .cloned()
            .map_or_else(|| read_meta(&sh.cfg, id), Ok)?;
        bail!("job {id} is not running ({})", m.state);
    };
    a.stop = true;
    if let Some(pgid) = a.pgid {
        // SAFETY: the job's own process group.
        unsafe { libc::kill(-pgid, libc::SIGTERM) };
        let sh2 = Arc::clone(sh);
        let id2 = id.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(sh2.cfg.grace);
            let inner = sh2.inner.lock().unwrap();
            if let Some(Active {
                id, pgid: Some(p), ..
            }) = inner.active.as_ref()
            {
                if *id == id2 && *p == pgid {
                    // SAFETY: still the job's group.
                    unsafe { libc::kill(-pgid, libc::SIGKILL) };
                }
            }
        });
    }
    drop(inner);
    Ok(json!({ "id": id, "stopping": true }))
}

fn meta_path(cfg: &HostConfig, id: &str) -> PathBuf {
    cfg.dir("runs").join(id).join("meta.json")
}

fn write_meta(cfg: &HostConfig, m: &Meta) -> Result<()> {
    let p = meta_path(cfg, &m.id);
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

fn read_meta(cfg: &HostConfig, id: &str) -> Result<Meta> {
    let p = meta_path(cfg, id);
    let b = fs::read(&p).map_err(|_| anyhow!("no job {id}"))?;
    Ok(serde_json::from_slice(&b)?)
}

fn disk_metas(cfg: &HostConfig) -> Vec<Meta> {
    let mut v: Vec<Meta> = fs::read_dir(cfg.dir("runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| read_meta(cfg, e.file_name().to_str()?).ok())
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let b = fs::read(dir.join("manifest.json"))?;
    Ok(serde_json::from_slice(&b)?)
}

/// The run's frame stats report from its log: the last `frame stats:` line
/// and the three worst frames after it, plus the outcome line.
pub fn summarize(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().collect();
    let mut out = Vec::new();
    if let Some(i) = lines.iter().rposition(|l| l.starts_with("frame stats:")) {
        out.push(lines[i].trim().to_string());
        if let Some(w) = lines[i..].iter().position(|l| l.trim() == "worst frames:") {
            out.extend(
                lines[i + w + 1..]
                    .iter()
                    .take_while(|l| l.trim_start().starts_with('#'))
                    .take(3)
                    .map(|l| l.trim().to_string()),
            );
        }
    }
    if let Some(l) = lines.iter().rev().find(|l| l.starts_with("outcome:")) {
        out.push(l.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_is_the_last_report_and_its_worst_frames() {
        let log = "entry 0x1, running\n\
            frame stats: 10 frames from 0, 30.00 fps; frame ms p50 33.0 p90 33.0 p99 40.0 max 41.0; over 33.3 ms: 2\n\
            frame stats: 900 frames from 300, 29.10 fps; frame ms p50 33.1 p90 34.0 p99 52.0 max 80.2; over 33.3 ms: 40\n  \
            mean ms/frame: cpu 10.00\n  mean per frame: draws 900.0\n  worst frames:\n    \
            #1 80.2 ms: cpu 50.0\n    #2 60.1 ms: cpu 40.0\n    #3 55.0 ms: cpu 30.0\n    #4 54.0 ms: cpu 30.0\n\
            DbgPrint: x\noutcome: stopped at present 1200\n";
        let s = summarize(log);
        assert_eq!(s.len(), 5, "{s:?}");
        assert!(s[0].starts_with("frame stats: 900 frames"));
        assert_eq!(s[3], "#3 55.0 ms: cpu 30.0");
        assert_eq!(s[4], "outcome: stopped at present 1200");
        assert!(summarize("nothing here").is_empty());
    }

    #[test]
    fn the_image_key_covers_commit_features_and_env() {
        let env = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let base = image_key("abc", &[], &env(&[("H2_OPT", "2")]), "");
        assert_eq!(base, image_key("abc", &[], &env(&[("H2_OPT", "2")]), ""));
        assert_ne!(base, image_key("abd", &[], &env(&[("H2_OPT", "2")]), ""));
        assert_ne!(
            base,
            image_key("abc", &["perf".into()], &env(&[("H2_OPT", "2")]), "")
        );
        assert_ne!(base, image_key("abc", &[], &env(&[("H2_OPT", "3")]), ""));
        assert_ne!(
            base,
            image_key("abc", &[], &env(&[("H2_OPT", "2")]), "f00d")
        );
    }
}
