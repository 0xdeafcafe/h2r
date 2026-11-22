//! `h2 lab host`: the lab's side on the machine that plays. One job at a
//! time (a build, or a run that may build first), each in `runs/<id>/` with
//! its log and metadata; later jobs wait in a FIFO. See docs/runtime/lab.md.

mod dedicated;
mod memory;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_proto::takeover::{Operation, Request as TakeoverRequest};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::proto::{self, BuildSpec, INPUT_FILE, Manifest, Request, RunReq, SimArm, SimReq};
use super::forensics::Summary;
use super::transfer::{Artifacts, Transfers};
use super::{build_cache, discovery, lan, service, simbench, takeover, transfer, watch};

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
    pub obj_cache_limit: u64,
    pub target_limit: u64,
    /// Always 1: the lab runs one build or game at a time. Kept so
    /// `--max-jobs 1` and status's `max_jobs` keep their meaning.
    pub max_jobs: usize,
    pub lan: Option<lan::Listen>,
    pub managed: bool,
    pub explicit_h2: bool,
    /// SIGTERM to SIGKILL on stop.
    pub grace: Duration,
    pub takeover: takeover::Config,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `run` or `build`.
    pub kind: String,
    /// queued, awaiting_consent, awaiting_quit, building, running, then
    /// done, failed, stopped or skipped.
    pub state: String,
    pub request: Value,
    pub commit: Option<String>,
    /// The build key stamped in `image/image.json`.
    pub image: Option<String>,
    /// The image was already built.
    pub cached: Option<bool>,
    pub build_env: BTreeMap<String, String>,
    pub level: Option<String>,
    pub created: u64,
    /// Persist phase times so file readers can see transitions between polls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_elapsed_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_elapsed_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub peak_footprint_bytes: Option<u64>,
    pub last_footprint_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) postmortem: Option<Summary>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// The frame stats report lines from the log.
    pub summary: Vec<String>,
    pub notes: Vec<String>,
    /// The game's agent API socket, for `h2 lab agent` (runs only).
    pub agent_socket: Option<String>,
    /// The `h2` that runs it, which also makes the agent calls.
    pub h2: Option<String>,
    /// The header ID of the film a `--film` run recorded: the ID
    /// `theater.play {"id"}` takes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub film_id: Option<String>,
    /// A dedicated host's XNet address and base port (`IPv4:port`), what a
    /// joiner on another Mac needs (`h2 lab run --dedicated`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_link: Option<String>,
}

pub(super) fn is_final(state: &str) -> bool {
    matches!(state, "done" | "failed" | "stopped" | "skipped")
}

struct Active {
    id: String,
    pgid: Option<i32>,
    memory_wake: Option<std::sync::mpsc::Sender<memory::Event>>,
    stop: bool,
}

#[derive(Default)]
struct Inner {
    active: Option<Active>,
    queue: VecDeque<Pending>,
    /// Held from admission of the first job until the FIFO is empty.
    activity: Option<File>,
    jobs: HashMap<String, Meta>,
}

struct Shared {
    cfg: HostConfig,
    listen_addr: Option<SocketAddr>,
    service_name: Option<String>,
    inner: Mutex<Inner>,
    cv: Condvar,
    transfers: Transfers,
    checkout: Mutex<()>,
    takeover: takeover::Gate,
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

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Serves the private socket and, when configured, the paired LAN listener.
pub fn serve(cfg: HostConfig) -> Result<()> {
    for d in ["runs", "recordings", "pgo", "scratch", "a"] {
        fs::create_dir_all(cfg.dir(d))
            .with_context(|| format!("creating {}", cfg.dir(d).display()))?;
    }
    fs::create_dir_all(&cfg.data)?;
    mem_watch::start(&cfg.data, Some(app::BuildInfo::baked().sha));
    let lan = cfg.lan.as_ref().map(open_lan).transpose()?;
    let authority = lan
        .as_ref()
        .map(|_| lan::Server::new(&cfg.root))
        .transpose()?;
    let listen_addr = lan.as_ref().map(|(addr, _, _)| *addr);
    let service_name = lan.as_ref().and_then(|(_, name, _)| name.clone());
    let listener = bind(&cfg.socket)?;
    let _advertisement = match (&service_name, listen_addr) {
        (Some(name), Some(addr)) => Some(discovery::advertise(name, addr.port())?),
        _ => None,
    };
    let takeover = takeover::Gate::new(cfg.takeover, &cfg.root, &cfg.h2);
    let shared = Arc::new(Shared {
        cfg,
        listen_addr,
        service_name,
        inner: Mutex::new(Inner::default()),
        cv: Condvar::new(),
        transfers: Transfers::default(),
        checkout: Mutex::new(()),
        takeover,
    });
    // Install/update may hold the activity lock across starting this service.
    // Verify an interrupted host has no children still writing job logs.
    {
        let lock = open_activity_lock(&shared.cfg.root)?;
        lock.lock()?;
        let unfinished: Vec<_> = disk_metas(&shared.cfg)
            .into_iter()
            .filter(|m| !is_final(&m.state))
            .collect();
        for job in &unfinished {
            build_cache::check_abandoned(&shared.cfg.dir("runs").join(&job.id))?;
        }
        build_cache::migrate(&shared.cfg)?;
        recover(&shared, unfinished);
    }
    eprintln!(
        "lab: listening on {} (root {}, repo {}, disc {})",
        shared.cfg.socket.display(),
        shared.cfg.root.display(),
        shared.cfg.repo.display(),
        shared.cfg.disc.display()
    );
    if let (Some((addr, name, listener)), Some(authority)) = (lan, authority) {
        eprintln!("lab: paired LAN listener on {addr} (Bonjour name {name:?})");
        let sh = Arc::clone(&shared);
        std::thread::spawn(move || serve_lan(sh, listener, authority));
    }
    if shared.cfg.managed {
        let sh = Arc::clone(&shared);
        std::thread::spawn(move || refresh_when_idle(&sh));
    }
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let sh = Arc::clone(&shared);
        std::thread::spawn(move || {
            let timeout = Some(Duration::from_secs(120));
            if conn.set_read_timeout(timeout).is_ok() && conn.set_write_timeout(timeout).is_ok() {
                let _ = connection(&sh, conn, "Local lab", |_| Ok(true));
            }
        });
    }
    Ok(())
}

fn open_lan(listen: &lan::Listen) -> Result<(SocketAddr, Option<String>, TcpListener)> {
    let listener = TcpListener::bind(listen.addr)
        .with_context(|| format!("binding LAN address {}", listen.addr))?;
    let addr = listener.local_addr()?;
    let name = match &listen.name {
        Some(name) => Some(name.clone()),
        None if cfg!(target_os = "macos") && !addr.ip().is_loopback() => {
            Some(discovery::default_name()?)
        }
        None => None,
    };
    ensure!(
        name.is_none() || !addr.ip().is_loopback(),
        "Bonjour advertisement requires a non-loopback --listen address"
    );
    ensure!(
        !name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("local")),
        "local is reserved for the Unix-socket target; choose another Bonjour name"
    );
    Ok((addr, name, listener))
}

fn serve_lan(sh: Arc<Shared>, listener: TcpListener, authority: lan::Server) {
    const MIN_DELAY: Duration = Duration::from_millis(50);
    const MAX_DELAY: Duration = Duration::from_secs(2);
    let authority = Arc::new(authority);
    let mut retry_delay = MIN_DELAY;
    loop {
        let (conn, peer) = match listener.accept() {
            Ok(connected) => connected,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                if retry_delay == MIN_DELAY {
                    eprintln!("lab: accepting LAN connections: {err}");
                }
                std::thread::sleep(retry_delay);
                retry_delay = (retry_delay * 2).min(MAX_DELAY);
                continue;
            }
        };
        retry_delay = MIN_DELAY;
        let Some(reservation) = authority.reserve_handshake(peer.ip()) else {
            continue;
        };
        let authority = Arc::clone(&authority);
        let sh = Arc::clone(&sh);
        std::thread::spawn(move || {
            let authenticated = authority.authenticate(conn);
            drop(reservation);
            if let Ok(Some(conn)) = authenticated {
                let requester = format!("Lab {}", peer.ip().to_canonical());
                let _ = connection(&sh, conn, &requester, |conn: &lan::Authenticated| {
                    conn.is_authorized()
                });
            }
        });
    }
}

fn open_activity_lock(root: &Path) -> Result<File> {
    fs::create_dir_all(root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("activity.lock"))?;
    Ok(lock)
}

fn try_activity_lock(root: &Path) -> Result<Option<File>> {
    let lock = open_activity_lock(root)?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}

/// Holds job admission closed across a local or remote service restart.
pub(super) fn activity_lock(root: &Path) -> Result<File> {
    try_activity_lock(root)?.context("busy: a lab job or host maintenance is active")
}

fn refresh_when_idle(sh: &Shared) {
    let cfg = &sh.cfg;
    let mut updates = service::AutoUpdate::default();
    loop {
        std::thread::sleep(Duration::from_secs(15));
        let result = (|| -> Result<()> {
            let Some(_lock) = try_activity_lock(&cfg.root)? else {
                return Ok(());
            };
            if updates.adopt_newer(cfg)? {
                sh.transfers.drain();
                // KeepAlive starts the stable copy. Hold the lock until we exit.
                std::process::exit(0);
            }
            Ok(())
        })();
        if let Err(err) = result {
            eprintln!("lab: checking for a newer host: {err:#}");
        }
    }
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
fn recover(sh: &Shared, unfinished: Vec<Meta>) {
    for mut job in unfinished {
        if job.state == "running" {
            job.postmortem = Some(Summary::read(&sh.cfg.dir("runs").join(&job.id).join("mem")));
        }
        job.state = "failed".into();
        job.error = Some("the host stopped while this job was pending or running".into());
        let _ = write_meta(&sh.cfg, &job);
    }
}

fn reply(w: &mut impl Write, result: Result<Value>) -> Result<()> {
    let value = match result {
        Ok(mut v) => {
            v["ok"] = Value::Bool(true);
            v
        }
        Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
    };
    writeln!(w, "{value}")?;
    w.flush()?;
    Ok(())
}

pub(super) fn local_status(path: &Path) -> Result<Value> {
    let mut socket = UnixStream::connect(path)?;
    let timeout = Some(Duration::from_secs(2));
    socket.set_read_timeout(timeout)?;
    socket.set_write_timeout(timeout)?;
    writeln!(socket, r#"{{"cmd":"status"}}"#)?;
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line)?;
    let value: Value = serde_json::from_str(&line)?;
    ensure!(value["ok"] == true, "lab host status failed");
    Ok(value)
}

fn connection<S: Read + Write>(
    sh: &Arc<Shared>,
    conn: S,
    requester: &str,
    authorized: impl Fn(&S) -> Result<bool>,
) -> Result<()> {
    let mut conn = BufReader::new(conn);
    let mut line = String::new();
    loop {
        // Put carries its bytes in the JSON line. Admit the transfer before
        // reading the rest of that line, without counting idle connections.
        if conn.fill_buf()?.is_empty() {
            return Ok(());
        }
        let transfer = match sh.transfers.begin() {
            Ok(transfer) => transfer,
            Err(error) => return reply(conn.get_mut(), Err(error)),
        };
        line.clear();
        (&mut conn)
            .take(proto::REQUEST_MAX + 1)
            .read_line(&mut line)?;
        ensure!(
            line.len() as u64 <= proto::REQUEST_MAX,
            "lab request too large"
        );
        ensure!(line.ends_with('\n'), "unterminated lab request");
        ensure!(authorized(conn.get_ref())?, "the lab pairing was revoked");
        let r = match proto::parse(&line) {
            Ok(req) => req,
            Err(e) => {
                reply(conn.get_mut(), Err(e))?;
                continue;
            }
        };
        match r {
            Request::Fetch { id } => {
                fetch(sh, &mut conn, &id, Artifacts::Run)?;
                return Ok(());
            }
            Request::Mem { id } => {
                fetch(sh, &mut conn, &id, Artifacts::Memory)?;
                return Ok(());
            }
            Request::Logs { id, follow } => {
                // A continuous feed must let an idle host restart.
                drop(transfer);
                return logs(sh, &mut conn, &id, follow, &authorized);
            }
            Request::Sync {
                branch,
                commit,
                bytes,
            } => {
                sync(sh, &mut conn, &branch, &commit, bytes, &authorized)?;
                return Ok(());
            }
            Request::Put { .. } => reply(conn.get_mut(), handle(sh, r, requester))?,
            Request::Update => {
                drop(transfer);
                let staged = (|| {
                    ensure!(
                        sh.cfg.managed,
                        "install the lab LaunchAgent before updating it"
                    );
                    let lock = activity_lock(&sh.cfg.root)?;
                    let _checkout = sh.checkout.lock().unwrap();
                    service::stage_update(&sh.cfg)?;
                    Ok(lock)
                })();
                match staged {
                    Ok(_lock) => {
                        sh.transfers.drain();
                        let _ = reply(conn.get_mut(), Ok(json!({ "restarting": true })));
                        if let Err(e) = service::restart() {
                            eprintln!("lab: kickstart failed, exiting for KeepAlive: {e:#}");
                        }
                        std::process::exit(0);
                    }
                    Err(e) => reply(conn.get_mut(), Err(e))?,
                }
            }
            _ => {
                drop(transfer);
                reply(conn.get_mut(), handle(sh, r, requester))?;
            }
        }
    }
}

fn fetch(sh: &Shared, conn: &mut BufReader<impl Read + Write>, id: &str, artifacts: Artifacts) -> Result<()> {
    let runs = sh.cfg.dir("runs");
    let child = (|| {
        ensure!(runs.join(id).join("meta.json").is_file(), "no job {id}");
        let member = artifacts.member(id);
        ensure!(runs.join(&member).is_dir(), "no {} artifacts for job {id}", artifacts.command());
        Command::new("tar")
            .arg("-C")
            .arg(&runs)
            .args(["-cf", "-", "--"])
            .arg(member)
            .stdout(Stdio::piped())
            .spawn()
            .context("starting host tar")
    })();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => return reply(conn.get_mut(), Err(e)),
    };
    let sent = reply(conn.get_mut(), Ok(json!({})))
        .and_then(|()| transfer::send_archive(child.stdout.take().unwrap(), conn.get_mut()));
    if let Err(e) = sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let result = child
        .wait()
        .map_err(anyhow::Error::from)
        .and_then(|status| {
            ensure!(status.success(), "host tar exited with {status}");
            Ok(json!({}))
        });
    reply(conn.get_mut(), result)
}

fn logs<S: Read + Write>(
    sh: &Shared,
    conn: &mut BufReader<S>,
    id: &str,
    follow: bool,
    authorized: &impl Fn(&S) -> Result<bool>,
) -> Result<()> {
    let id = (id != proto::CURRENT_JOB).then_some(id);
    let mut feed = match watch::Watch::new(&sh.cfg.root, id) {
        Ok(feed) => feed,
        Err(error) => return reply(conn.get_mut(), Err(error)),
    };
    reply(conn.get_mut(), Ok(json!({})))?;

    // Check revocation while idle and when a large tail takes multiple
    // frames to send. Heartbeats notice clients that close an idle feed.
    let mut last_write = Instant::now();
    let mut wire_failed = false;
    let result = (|| {
        loop {
            // Keep checking the driver even if there is nothing to send.
            ensure!(authorized(conn.get_ref())?, "the lab pairing was revoked");
            let mut checked_at = Instant::now();
            let finished = feed.poll(!follow, &mut |line| {
                if checked_at.elapsed() >= watch::POLL_INTERVAL {
                    ensure!(authorized(conn.get_ref())?, "the lab pairing was revoked");
                    checked_at = Instant::now();
                }
                let mut frame = line.as_bytes().to_vec();
                frame.push(b'\n');
                let sent = transfer::send_log_frame(conn.get_mut(), &frame);
                wire_failed = sent.is_err();
                last_write = Instant::now();
                sent
            })?;
            if finished || !follow {
                return Ok(json!({}));
            }
            if last_write.elapsed() >= Duration::from_secs(5) {
                let sent = transfer::send_log_frame(conn.get_mut(), transfer::LOG_HEARTBEAT);
                wire_failed = sent.is_err();
                sent?;
                last_write = Instant::now();
            }
            std::thread::sleep(watch::POLL_INTERVAL);
        }
    })();
    if wire_failed {
        return result.map(|_| ());
    }
    transfer::send_log_frame(conn.get_mut(), b"")?;
    reply(conn.get_mut(), result)
}

fn sync<S: Read + Write>(
    sh: &Shared,
    conn: &mut BufReader<S>,
    branch: &str,
    commit: &str,
    bytes: u64,
    authorized: &impl Fn(&S) -> Result<bool>,
) -> Result<()> {
    let prepared = (|| {
        let refs = transfer::branch_refs(branch)?;
        let bundle = if bytes == 0 {
            None
        } else {
            Some(transfer::TempFile::new(&sh.cfg.dir("scratch"))?)
        };
        Ok((refs, bundle))
    })();
    let ((source, dest), mut bundle) = match prepared {
        Ok(prepared) => prepared,
        Err(e) => return reply(conn.get_mut(), Err(e)),
    };
    reply(conn.get_mut(), Ok(json!({})))?;
    let result = (|| {
        if let Some(bundle) = &mut bundle {
            let actual = io::copy(&mut (&mut *conn).take(bytes), &mut bundle.file)?;
            ensure!(actual == bytes, "truncated git bundle");
        }
        // Jobs use immutable commits in detached worktrees. Serialize only
        // mutations to the checkout, and leave the upload outside this lock.
        let _checkout = sh.checkout.lock().unwrap();
        ensure!(authorized(conn.get_ref())?, "the lab pairing was revoked");
        let repo = &sh.cfg.repo;
        transfer::ensure_unchecked(repo, &dest)?;
        if let Some(bundle) = &bundle {
            transfer::import_bundle(repo, &bundle.path, &source, commit)?;
        }
        let out = transfer::git(repo)
            .args(["update-ref", &dest, commit])
            .output()?;
        ensure!(
            out.status.success(),
            "updating {dest}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(json!({ "commit": commit }))
    })();
    reply(conn.get_mut(), result)
}

fn handle(sh: &Arc<Shared>, r: Request, requester: &str) -> Result<Value> {
    let cfg = &sh.cfg;
    match r {
        Request::Status => {
            let memory = mem_watch::snapshot();
            let pressure = mem_watch::pressure();
            let inner = sh.inner.lock().unwrap();
            let active = inner.active.as_ref().and_then(|a| inner.jobs.get(&a.id));
            // One job at a time: `jobs_active` holds at most the active job.
            let jobs_active: Vec<_> = active.into_iter().collect();
            Ok(json!({
                "version": proto::VERSION,
                "build_version": env!("CARGO_PKG_VERSION"),
                "build_commit": app::BuildInfo::baked().sha,
                "pid": std::process::id(),
                "footprint_bytes": memory.footprint_bytes,
                "peak_footprint_bytes": memory.peak_bytes,
                "memory_pressure": pressure,
                "listen": sh.listen_addr,
                "name": sh.service_name,
                "root": cfg.root,
                "repo": cfg.repo,
                "disc": cfg.disc,
                "h2": cfg.h2,
                "active": active,
                "jobs_active": jobs_active,
                "max_jobs": cfg.max_jobs,
                "queued": inner.queue.iter().map(|p| &p.id).collect::<Vec<_>>(),
            }))
        }
        Request::SyncBases { branch, commit } => {
            let (_, dest) = transfer::branch_refs(&branch)?;
            let known = transfer::resolve_commit(&cfg.repo, &commit)?.is_some();
            let mut bases = Vec::new();
            for rev in [dest.as_str(), "HEAD"] {
                if let Some(base) = transfer::resolve_commit(&cfg.repo, rev)? {
                    if !bases.contains(&base) {
                        bases.push(base);
                    }
                }
            }
            Ok(json!({ "known": known, "bases": bases }))
        }
        Request::Fetch { .. }
        | Request::Mem { .. }
        | Request::Logs { .. }
        | Request::Sync { .. }
        | Request::Update => unreachable!(),
        Request::Run(run) => start(sh, Job::Run(run), requester),
        Request::Sim(sim) => start(sh, Job::Sim(sim), requester),
        Request::Build(b) => start(sh, Job::Build(b), requester),
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

struct Pending {
    id: String,
    job: Job,
    commit: Option<String>,
    requester: String,
}

enum Job {
    Run(RunReq),
    /// `h2 lab sim`: headless matches on the image, no game window.
    Sim(SimReq),
    Build(BuildSpec),
}

impl Job {
    fn spec(&self) -> &BuildSpec {
        match self {
            Job::Run(r) => &r.build,
            Job::Sim(s) => &s.build,
            Job::Build(b) => b,
        }
    }
}

fn start(sh: &Arc<Shared>, job: Job, requester: &str) -> Result<Value> {
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
        if let Some(d) = &r.dedicated {
            level = d.map.clone();
        }
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
        if let Some(name) = &r.film {
            ensure!(
                !film_library(cfg, "", r).join(proto::film_file(name)).exists(),
                "film {name} exists in the host's film library; pick another name"
            );
        }
    }
    let mut inner = sh.inner.lock().unwrap();
    if let Job::Run(run) = &job && let Some(name) = &run.record {
        ensure!(
            !inner.jobs.values().any(|m| !is_final(&m.state) && m.request["record"].as_str() == Some(name.as_str())),
            "recording {name} already has a pending job"
        );
    }
    if let Job::Run(run) = &job && let Some(name) = &run.film {
        ensure!(
            !inner.jobs.values().any(|m| !is_final(&m.state)
                && m.request["film"].as_str() == Some(name.as_str())
                && !m.request["fresh_data"].as_bool().unwrap_or(false)
                && !run.fresh_data),
            "film {name} already has a pending job"
        );
    }
    let lock = if inner.activity.is_none() {
        Some(activity_lock(&cfg.root)?)
    } else {
        None
    };
    let id = next_id(cfg, &inner);
    fs::create_dir_all(cfg.dir("runs").join(&id))?;
    let (kind, name, request) = match &job {
        Job::Run(r) => ("run", Some(r.name.clone()), serde_json::to_value(r)?),
        Job::Sim(s) => {
            // The uploaded files are in the run's `sim/*/in/`, not in every
            // metadata write and listing.
            let mut request = serde_json::to_value(s)?;
            request["files"] = json!(s.files.keys().collect::<Vec<_>>());
            ("sim", Some(s.name.clone()), request)
        }
        Job::Build(b) => ("build", None, serde_json::to_value(b)?),
    };
    let m = Meta {
        id: id.clone(),
        kind: kind.into(),
        name,
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
    inner.queue.push_back(Pending {
        id: id.clone(),
        job,
        commit,
        requester: requester.to_owned(),
    });
    if let Some(lock) = lock {
        inner.activity = Some(lock);
        let sh = Arc::clone(sh);
        std::thread::spawn(move || drain_queue(&sh));
    }
    drop(inner);
    sh.cv.notify_all();
    Ok(json!({ "id": id, "job": m }))
}

fn drain_queue(sh: &Arc<Shared>) {
    loop {
        let mut inner = sh.inner.lock().unwrap();
        if inner.queue.is_empty() {
            inner.activity = None;
            return;
        }
        if mem_watch::pressure() == Some(true) {
            drop(sh.cv.wait_timeout(inner, Duration::from_secs(2)).unwrap());
            continue;
        }
        let pending = inner.queue.pop_front().unwrap();
        inner.active = Some(Active {
            id: pending.id.clone(),
            pgid: None,
            stop: false,
            memory_wake: None,
        });
        drop(inner);
        execute(sh, &pending.id, pending.job, pending.commit, &pending.requester);
    }
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
        "{rev} is not a commit in {} (h2 lab sync imports branches as lab/<name>)",
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

    /// Whether a game the consent check found is this host's running job.
    fn owns_game(&self, socket: Option<&Path>, pid: Option<u32>) -> bool {
        let inner = self.inner.lock().unwrap();
        let group = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|&pid| pid > 0)
            .map(|pid| {
                // SAFETY: getpgid only queries the process group of a positive PID.
                unsafe { libc::getpgid(pid) }
            });
        inner.active.as_ref().is_some_and(|active| {
            inner.jobs.get(&active.id).is_some_and(|m| m.state == "running")
                && ((group.is_some() && group == active.pgid)
                    || socket.is_some_and(|path| {
                        path == agent_socket_path(&self.cfg, &active.id).as_path()
                    }))
        })
    }

    /// Runs one step of job `id` in its own process group, so stop reaches
    /// what it starts (cargo's rustc, the game's helpers) too.
    fn step(&self, id: &str, cmd: Command, log: &File) -> Result<std::process::ExitStatus> {
        self.step_with(id, cmd, log, |child| Ok(child.wait()?))
    }

    /// [`Self::step`], waiting for the child with `wait`.
    fn step_with(
        &self,
        id: &str,
        mut cmd: Command,
        log: &File,
        wait: impl FnOnce(&mut std::process::Child) -> Result<std::process::ExitStatus>,
    ) -> Result<std::process::ExitStatus> {
        cmd.stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?)
            .process_group(0);
        // Publish the process group before a stop can observe this spawn. A
        // stop between steps must prevent the next child from starting at all.
        let mut inner = self.inner.lock().unwrap();
        let Some(active) = inner.active.as_mut().filter(|a| a.id == id && !a.stop) else {
            return Err(Stopped.into());
        };
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {:?}", cmd.get_program()))?;
        let pgid = child.id() as i32;
        active.pgid = Some(pgid);
        if let Some(wake) = &active.memory_wake {
            let _ = wake.send(memory::Event::Step);
        }
        drop(inner);
        let status = wait(&mut child)?;
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

enum JobOutcome {
    Finished(Option<std::process::ExitStatus>),
    Skipped(&'static str),
}

fn execute(
    sh: &Arc<Shared>,
    id: &str,
    job: Job,
    commit: Option<String>,
    requester: &str,
) {
    let t = Instant::now();
    let (outcome, memory_error) = memory::supervise(sh, id, &job, || {
        run_job(sh, id, &job, commit.as_deref(), requester, t)
    });
    let (state, error) = match &outcome {
        Ok(JobOutcome::Finished(Some(st))) if st.success() => ("done", None),
        Ok(JobOutcome::Finished(Some(st))) => (
            "failed",
            Some(match (st.code(), st.signal()) {
                (Some(mem_watch::MEMORY_LIMIT_EXIT), _) => {
                    format!("memory limit: game watchdog exited; see runs/{id}/mem/")
                }
                (Some(c), _) => format!("exit {c}"),
                (_, Some(s)) => format!("signal {s}"),
                _ => "failed".into(),
            }),
        ),
        Ok(JobOutcome::Finished(None)) => ("done", None),
        Ok(JobOutcome::Skipped(_)) => ("skipped", None),
        Err(e) if e.is::<Stopped>() => ("stopped", None),
        Err(e) => ("failed", Some(format!("{e:#}"))),
    };
    let run_dir = sh.cfg.dir("runs").join(id);
    let summary = if matches!(job, Job::Sim(_)) {
        // The benchmark's table and hash verdict, not frame stats.
        fs::read_to_string(run_dir.join(simbench::SUMMARY_TEXT))
            .map(|t| t.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    } else {
        File::open(run_dir.join("run.log"))
            .and_then(|log| summarize(BufReader::new(log)))
            .unwrap_or_default()
    };
    let (status, skip_reason) = match outcome {
        Ok(JobOutcome::Finished(status)) => (status, None),
        Ok(JobOutcome::Skipped(reason)) => (None, Some(reason.to_owned())),
        Err(_) => (None, None),
    };
    // A stopped game exits through its SIGTERM handler or the signal.
    let state = if state == "failed" && sh.stop_requested(id) {
        "stopped"
    } else {
        state
    };
    let postmortem = (memory_error.is_some() || (state == "failed" && status.is_some()))
        .then(|| Summary::read(&run_dir.join("mem")));
    let _ = fs::remove_dir_all(sh.cfg.dir("scratch").join(id));
    let _ = fs::remove_file(agent_socket_path(&sh.cfg, id));
    let mut inner = sh.inner.lock().unwrap();
    if let Some(m) = inner.jobs.get_mut(id) {
        m.state = if memory_error.is_some() { "failed" } else { state }.into();
        m.error = memory_error.or(error);
        m.skip_reason = skip_reason;
        m.duration_s = Some(t.elapsed().as_secs_f64());
        m.exit_code = status.and_then(|s| s.code());
        m.signal = status.and_then(|s| s.signal());
        m.summary = summary;
        m.postmortem = postmortem;
        if write_meta(&sh.cfg, m).is_ok() {
            inner.jobs.remove(id);
        }
    }
    if inner.active.as_ref().is_some_and(|a| a.id == id) {
        inner.active = None;
    }
    drop(inner);
    sh.cv.notify_all();
}

/// A coarse estimate for the player. A run without a frame limit can
/// continue until the driver stops it.
fn takeover_request(
    job: &Job,
    commit: Option<&str>,
    requester: &str,
    builds_image: bool,
    timeout_seconds: u32,
) -> TakeoverRequest {
    const BUILD_SECONDS: u32 = 20 * 60;
    let build_seconds = if builds_image { BUILD_SECONDS } else { 0 };
    let (operation, estimate_seconds) = match job {
        Job::Build(_) => (Operation::Build, build_seconds),
        // A sim has no frame limit to estimate from.
        Job::Sim(_) => (Operation::Run, 0),
        Job::Run(run) => {
            let frames = run
                .env
                .get("H2_BOOT_STOP_AT_PRESENT")
                .and_then(|n| n.parse::<u64>().ok())
                .or(run.frames);
            let seconds = frames.map_or(0, |frames| {
                let fps = run.args.windows(2).rev().find_map(|pair| {
                    (pair[0] == "--fps").then_some(&pair[1])
                });
                let fps = fps
                    .or_else(|| run.env.get("H2_FPS"))
                    .and_then(|fps| fps.parse::<u64>().ok())
                    .filter(|&fps| fps > 0)
                    .unwrap_or(30);
                let run_seconds = frames.div_ceil(fps).saturating_add(30);
                u32::try_from(run_seconds).unwrap_or(u32::MAX).saturating_add(build_seconds)
            });
            (Operation::Run, seconds)
        }
    };
    let mut revision: String = commit.map_or_else(
        || "installed image".into(),
        |sha| sha.chars().take(12).collect(),
    );
    if matches!(job, Job::Run(run) if run.own_data) {
        revision.push_str(", using your own data");
    }
    TakeoverRequest {
        requester: requester.to_owned(),
        operation,
        revision,
        estimate_seconds,
        timeout_seconds,
    }
}

fn request_takeover(
    sh: &Shared,
    id: &str,
    request: TakeoverRequest,
) -> Result<Option<JobOutcome>> {
    let outcome = sh.takeover.admit(
        request,
        |phase| sh.update(id, |m| m.state = phase.into()),
        || sh.stop_requested(id),
        |socket, pid| sh.owns_game(socket, pid),
    );
    match outcome {
        _ if sh.stop_requested(id) => Err(Stopped.into()),
        takeover::Outcome::Proceed => Ok(None),
        takeover::Outcome::Skipped(reason) => Ok(Some(JobOutcome::Skipped(reason))),
        takeover::Outcome::Stopped => Err(Stopped.into()),
    }
}

fn run_job(
    sh: &Arc<Shared>,
    id: &str,
    job: &Job,
    commit: Option<&str>,
    requester: &str,
    started: Instant,
) -> Result<JobOutcome> {
    let cfg = &sh.cfg;
    let run_dir = cfg.dir("runs").join(id);
    let planned = commit.map(|c| plan_image(cfg, job.spec(), c)).transpose()?;
    let builds_image = planned.as_ref().is_some_and(|p| !p.cached);
    let consent = |builds_image| {
        request_takeover(
            sh,
            id,
            takeover_request(job, commit, requester, builds_image, cfg.takeover.timeout_seconds),
        )
    };
    if (builds_image || !matches!(job, Job::Build(_))) && let Some(skipped) = consent(builds_image)? {
        return Ok(skipped);
    }
    let image = match (commit, planned) {
        (Some(c), Some(planned)) => {
            sh.update(id, |m| {
                m.state = "building".into();
                m.build_elapsed_s = Some(started.elapsed().as_secs_f64());
            });
            let log = File::create(run_dir.join("build.log"))?;
            ensure_image(sh, id, job.spec(), c, planned, &log)?
        }
        _ => Image {
            h2: cfg.h2.clone(),
            out: cfg.out.clone(),
            env: BTreeMap::new(),
        },
    };
    if matches!(job, Job::Build(_)) {
        return Ok(JobOutcome::Finished(None));
    }
    // A player can open the game during a long build. Ask before starting
    // the run too if there was an uncached build in between.
    if builds_image && let Some(skipped) = consent(false)? {
        return Ok(skipped);
    }
    let run = match job {
        Job::Run(run) => run,
        Job::Sim(sim) => return run_sim(sh, id, sim, &image, started),
        Job::Build(_) => unreachable!("builds returned above"),
    };
    if let Some(d) = &run.dedicated {
        return dedicated::run(sh, id, run, d, &image, started);
    }
    let mut log = File::create(run_dir.join("run.log"))?;
    writeln!(log, "lab job {id} · {}", run.name)?;
    let cmd = play_command(cfg, id, run, &image, commit)?;
    let sock = agent_socket_path(cfg, id);
    sh.update(id, |m| {
        m.state = "running".into();
        m.run_elapsed_s = Some(started.elapsed().as_secs_f64());
        m.agent_socket = Some(sock.display().to_string());
        m.h2 = Some(image.h2.display().to_string());
    });
    let run_log = run_dir.join("run.log");
    let mut lingered = None;
    let stepped = sh.step_with(id, cmd, &log, |child| {
        let (st, outcome) = wait_game(child, &run_log, GAME_EXIT_GRACE, sh.cfg.grace)?;
        lingered = outcome;
        Ok(st)
    });
    // Keep the film however the game ended: `h2 lab stop` is the usual end
    // of a `--serve` recording, and a `--fresh-data` root is removed next.
    if run.film.is_some() {
        let (film_id, notes) = keep_film(cfg, id, run);
        sh.update(id, |m| {
            m.film_id = film_id;
            m.notes.extend(notes);
        });
    }
    let st = stepped?;
    if let Some(outcome) = lingered {
        let note = format!(
            "the game reported its outcome ({outcome}) but had not exited {} s later; \
             the lab stopped it",
            GAME_EXIT_GRACE.as_secs()
        );
        let _ = writeln!(log, "lab: {note}");
        sh.update(id, |m| m.notes.push(note));
    }
    if let Some(name) = &run.record {
        let input = cfg.dir("recordings").join(name).join(INPUT_FILE);
        if !input.is_file() {
            sh.update(id, |m| {
                m.notes
                    .push(format!("recording {name}: the game wrote no input log"))
            });
        }
    }
    Ok(JobOutcome::Finished(Some(st)))
}

/// A sim benchmark: `h2 sim run` (or `batch`) of the uploaded file once per
/// arm and repeat, one after another in the order [`simbench::schedule`]
/// gives, each in `runs/<id>/sim/<arm>-r<N>/`. Per-run data directories go
/// as each step ends; the reports, logs and the summary stay
/// (docs/runtime/lab.md, "Sim benchmarks").
fn run_sim(
    sh: &Arc<Shared>,
    id: &str,
    sim: &SimReq,
    image: &Image,
    started: Instant,
) -> Result<JobOutcome> {
    let cfg = &sh.cfg;
    let run_dir = cfg.dir("runs").join(id);
    let mut log = File::create(run_dir.join("run.log"))?;
    writeln!(log, "lab job {id} · {}", sim.name)?;
    simbench::check_entry(sim)?;
    // The driver's bot meshes, in a data root of the job's own.
    let job_data = run_dir.join("sim-data");
    let data = if sim.data.is_empty() {
        cfg.data.clone()
    } else {
        simbench::stage_data(&job_data, &cfg.data, &sim.data)?;
        job_data.clone()
    };
    let plan = simbench::schedule(sim.arms.len(), sim.repeat);
    sh.update(id, |m| {
        m.state = "running".into();
        m.run_elapsed_s = Some(started.elapsed().as_secs_f64());
        m.h2 = Some(image.h2.display().to_string());
    });
    let mut steps = Vec::new();
    let mut stopped = None;
    for (n, &(arm, repeat)) in plan.iter().enumerate() {
        let name = simbench::step_name(&sim.arms[arm].name, repeat);
        let dir = run_dir.join("sim").join(&name);
        writeln!(log, "lab: sim run {}/{}: {name}", n + 1, plan.len())?;
        let t = Instant::now();
        let ran = sim_command(cfg, id, sim, &sim.arms[arm], image, &data, &dir).and_then(|cmd| {
            let step_log = File::create(dir.join("sim.log"))?;
            sh.step(id, cmd, &step_log)
        });
        // The 1.3 GB data directories go whether or not the run finished.
        simbench::remove_run_data(&dir);
        let mut error = match ran {
            Err(e) if e.is::<Stopped>() => {
                stopped = Some(e);
                break;
            }
            Err(e) => Some(format!("{e:#}")),
            Ok(st) if !st.success() => Some(match (st.code(), st.signal()) {
                (Some(c), _) => format!("exit {c}"),
                (_, Some(s)) => format!("signal {s}"),
                _ => "failed".into(),
            }),
            Ok(_) => None,
        };
        let (reports, bad) = simbench::collect_reports(&dir);
        if error.is_none() && !bad.is_empty() {
            error = Some(format!("unreadable report: {}", bad.join("; ")));
        } else if error.is_none() && reports.is_empty() {
            error = Some("no report.json".into());
        }
        // A match without the nav mesh ran, but not as a bot match: the
        // bots stood still and its speed means nothing.
        if error.is_none() {
            let log_text = fs::read(dir.join("sim.log")).unwrap_or_default();
            error = simbench::nav_problem(&reports, &String::from_utf8_lossy(&log_text));
        }
        writeln!(
            log,
            "lab: sim run {name}: {} in {:.1} s ({} report(s))",
            error.as_deref().map_or("ok".to_owned(), |e| format!("failed, {e}")),
            t.elapsed().as_secs_f64(),
            reports.len()
        )?;
        steps.push(simbench::StepResult {
            arm,
            repeat,
            ok: error.is_none(),
            error,
            reports,
        });
    }
    let _ = fs::remove_dir_all(&job_data);
    let summary = simbench::summarize(sim, &steps);
    let text = summary.render();
    fs::write(run_dir.join(simbench::SUMMARY_TEXT), &text)?;
    fs::write(
        run_dir.join(simbench::SUMMARY_JSON),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    write!(log, "{text}")?;
    if let Some(e) = stopped {
        return Err(e);
    }
    let failed = steps.iter().filter(|s| !s.ok).count();
    if failed > 0 {
        let note = format!(
            "{failed} of {} sim runs failed (runs/{id}/sim/<arm>-r<N>/sim.log)",
            steps.len()
        );
        sh.update(id, |m| m.notes.push(note));
    }
    Ok(JobOutcome::Finished(Some(status_for_outcome(if failed == 0 {
        "Returned"
    } else {
        "Failed"
    }))))
}

/// `h2 sim run <file> --report <dir>/report.json` (or `h2 sim batch <file>`)
/// on the lab's image and data, with the arm's `H2_FEATURES`, cleaned of the
/// host's own `H2_*` variables like a game run. Writes the step's input
/// files first; the sim never builds, so the image must be the one the job
/// built (`H2_OPT` and the rest of the build environment come with it).
fn sim_command(
    cfg: &HostConfig,
    id: &str,
    sim: &SimReq,
    arm: &SimArm,
    image: &Image,
    data: &Path,
    dir: &Path,
) -> Result<Command> {
    let inputs = dir.join("in");
    simbench::write_inputs(&inputs, &sim.files)?;
    let mut cmd = Command::new(&image.h2);
    cmd.arg("sim")
        .arg(if sim.batch { "batch" } else { "run" })
        .arg(inputs.join(&sim.file))
        .arg("--disc")
        .arg(&cfg.disc);
    if let Some(out) = &image.out {
        cmd.arg("--out").arg(out);
    }
    cmd.arg("--data").arg(data);
    if sim.batch {
        // One at a time unless asked: these are benchmarks.
        cmd.arg("--parallel").arg(sim.parallel.unwrap_or(1).to_string());
    } else {
        cmd.arg("--report").arg(dir.join("report.json"));
    }
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cmd.env_remove(k);
        }
    }
    cmd.envs(&image.env).envs(&sim.env);
    if let Some(f) = &arm.features {
        cmd.env("H2_FEATURES", f);
    }
    cmd.env("H2_LAB_JOB_ID", id).env("H2_LAB_JOB_NAME", &sim.name);
    cmd.current_dir(dir);
    Ok(cmd)
}

/// How long a game may outlive its guest thread's exit before the lab stops
/// it. `h2 play` closes its window when a lab job's guest returns and exits
/// within its shutdown timeout; this is the backstop for a process that
/// still lingers.
const GAME_EXIT_GRACE: Duration = Duration::from_secs(60);

/// What the app shell logs when the guest thread ends
/// (`crates/apps/app/src/shell.rs`, `UserEvent::GuestExited`), followed by
/// the `GuestExit` in Debug form.
const GUEST_EXITED: &str = "app: guest thread exited: ";

/// The guest's outcome from one run log line, if it is the shell's
/// guest-exit line.
fn guest_exit_outcome(line: &str) -> Option<&str> {
    line.trim_end().strip_prefix(GUEST_EXITED)
}

/// The exit status a game that reported `outcome` would have had: success
/// for a guest that returned, exit 1 for one that failed or panicked.
fn status_for_outcome(outcome: &str) -> std::process::ExitStatus {
    // A wait status: the exit code sits in the second byte.
    std::process::ExitStatus::from_raw(if outcome == "Returned" { 0 } else { 1 << 8 })
}

/// Waits for the game `child` (its own process group leader) to exit,
/// watching `log` for the guest's outcome. A game still alive `grace` after
/// its guest thread exited is stopped (SIGTERM to the group, SIGKILL after
/// `kill_grace`) and finishes with the status that outcome implies; the
/// outcome is returned with it. A game that exits by itself returns its
/// own status and no outcome.
fn wait_game(
    child: &mut std::process::Child,
    log: &Path,
    grace: Duration,
    kill_grace: Duration,
) -> Result<(std::process::ExitStatus, Option<String>)> {
    let pgid = child.id() as i32;
    let mut reader = File::open(log).ok().map(BufReader::new);
    let mut line = Vec::new();
    let mut exited: Option<(String, Instant)> = None;
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok((st, None));
        }
        if exited.is_none()
            && let Some(r) = reader.as_mut()
        {
            // Whole lines only: a partial one waits for its newline.
            loop {
                match r.read_until(b'\n', &mut line) {
                    Ok(n) if n > 0 && line.ends_with(b"\n") => {
                        if let Some(o) = guest_exit_outcome(&String::from_utf8_lossy(&line)) {
                            exited = Some((o.to_owned(), Instant::now()));
                            break;
                        }
                        line.clear();
                    }
                    _ => break,
                }
            }
        }
        if let Some((outcome, at)) = &exited
            && at.elapsed() >= grace
        {
            // SAFETY: the game's own process group.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };
            let deadline = Instant::now() + kill_grace;
            while child.try_wait()?.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            // SAFETY: as above; the group may be gone already.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            child.wait()?;
            return Ok((status_for_outcome(outcome), Some(outcome.clone())));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
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
    // Earlier revisions also accept this flag. A current game supplies its
    // own baked SHA for --current, even when --h2 selects another binary.
    let revision = commit.unwrap_or_else(|| {
        if cfg.explicit_h2 {
            "unknown"
        } else {
            app::BuildInfo::baked().sha
        }
    });
    cmd.arg("--title")
        .arg(proto::window_title(id, &run.name, revision));
    if let Some(out) = &image.out {
        cmd.arg("--out").arg(out);
    }
    // `--own-data`: no `--data`, so the game takes its default data root
    // (and settings, Live accounts) like `h2 play` on this Mac.
    if let Some(data) = run_data_root(cfg, id, run) {
        if run.fresh_data {
            fs::create_dir_all(&data)?;
        }
        cmd.arg("--data").arg(data);
    }
    let mut level = run.level.clone();
    let mut difficulty = run.difficulty.clone();
    for (k, _) in std::env::vars_os() {
        let kept = |k: &str| run.own_data && proto::OWN_DATA_ENV.contains(&k);
        if k.to_str().is_some_and(|k| k.starts_with("H2_") && !kept(k)) {
            cmd.env_remove(k);
        }
    }
    cmd.envs(&image.env);
    if let Some(name) = &run.film {
        // The game records into the library its own `theater.play` reads
        // (the data root's films/, since H2_FILM_DIR is cleaned above).
        let dir = film_library(cfg, id, run);
        fs::create_dir_all(&dir)?;
        let path = dir.join(proto::film_file(name));
        ensure!(
            !path.exists(),
            "film {name} exists in the host's film library; pick another name"
        );
        cmd.arg("--film").arg(path);
    }
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
        fs::create_dir(&dir).with_context(|| format!("creating recording {name}"))?;
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
    // `fresh`: an empty pipeline cache of the run's own, beside its logs.
    if run.env.get("H2_PIPELINE_CACHE").map(String::as_str) == Some(proto::PIPELINE_CACHE_FRESH) {
        cmd.env("H2_PIPELINE_CACHE", run_dir.join("pipeline-cache.jsonl"));
    }
    // The host owns the agent socket, the output root and the memory
    // report directory (proto::RUN_ENV).
    let sock = agent_socket_path(cfg, id);
    if let Some(dir) = sock.parent() {
        fs::create_dir_all(dir)?;
    }
    let _ = fs::remove_file(&sock);
    cmd.env("H2_LAB_JOB_ID", id)
        .env("H2_LAB_JOB_NAME", &run.name)
        .env(agent_proto::ENV, &sock)
        .env(agent_proto::OUT_ENV, &run_dir)
        .env("H2_MEM_REPORT_DIR", run_dir.join("mem"));
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

/// The film library the run's game reads and records into: `films/` under
/// the data root it plays with (`h2 play --data`), which is the run's own
/// scratch root for `--fresh-data`.
fn film_library(cfg: &HostConfig, id: &str, run: &RunReq) -> PathBuf {
    run_data_root(cfg, id, run)
        .unwrap_or_else(content::store::default_data_root)
        .join("films")
}

/// The `--data` a run's game gets: the run's scratch root for
/// `--fresh-data`, none for `--own-data` (the game's default root), else the
/// lab's `data/`.
fn run_data_root(cfg: &HostConfig, id: &str, run: &RunReq) -> Option<PathBuf> {
    if run.own_data {
        None
    } else if run.fresh_data {
        Some(cfg.dir("scratch").join(id).join("data"))
    } else {
        Some(cfg.data.clone())
    }
}

/// Keeps the film a `--film` run recorded: a link or copy in `runs/<id>/`
/// (so `fetch` brings it back and a `--fresh-data` run's film survives its
/// scratch root), and its header ID read through the `film` crate. Returns
/// the ID and the notes for the job.
fn keep_film(cfg: &HostConfig, id: &str, run: &RunReq) -> (Option<String>, Vec<String>) {
    let Some(name) = &run.film else {
        return (None, vec![]);
    };
    let file = proto::film_file(name);
    let src = film_library(cfg, id, run).join(&file);
    let dest = cfg.dir("runs").join(id).join(&file);
    if !src.is_file() {
        return (None, vec![format!("film {name}: the game wrote no film")]);
    }
    if let Err(e) = fs::hard_link(&src, &dest).or_else(|_| fs::copy(&src, &dest).map(|_| ())) {
        return (None, vec![format!("film {name}: keeping a copy in the run failed: {e}")]);
    }
    match film::LibraryEntry::read(&dest) {
        Ok(e) => (
            Some(e.meta.id.clone()),
            vec![format!(
                "film {name}: id {} ({} ticks); h2 lab run --serve, then theater.play {{\"id\":\"{}\"}}",
                e.meta.id, e.meta.length_ticks, e.meta.id
            )],
        ),
        Err(e) => (None, vec![format!("film {name}: header unreadable: {e:#}")]),
    }
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
            "queued" | "awaiting_consent" | "awaiting_quit" | "building" => bail!(
                "job {id} is {} (h2 lab wait {id} --timeout 5, until it is running)",
                m.state
            ),
            "running" => {}
            "skipped" => bail!("run {id} was skipped: {}", m.skip_reason.as_deref().unwrap_or("?")),
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
    cmd.arg("agent");
    if let [domain, verb, rest @ ..] = args
        && domain == "flight"
        && verb == "show"
    {
        cmd.args(["flight", "show", "--remote"]).args(rest);
    } else {
        cmd.args(args);
    }
    cmd.arg("--socket")
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
    let _lock = activity_lock(&cfg.root)?;
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
    let mut log = File::create(cfg.dir("runs").join(&id).join("run.log"))?;
    writeln!(log, "lab job {id} · {}", run.name)?;
    let t = Instant::now();
    let st = cmd
        .status()
        .with_context(|| format!("starting {}", cfg.h2.display()))?;
    let m = Meta {
        id,
        kind: "run".into(),
        name: Some(run.name.clone()),
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

/// Resolve the cache key once so admission and the builder agree about
/// whether a job will start an expensive compile.
struct ImagePlan {
    image: Image,
    key: String,
    dir: PathBuf,
    features: Vec<String>,
    pgo_hash: String,
    cached: bool,
}

fn plan_image(cfg: &HostConfig, spec: &BuildSpec, commit: &str) -> Result<ImagePlan> {
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
    let dir = cfg.dir("image");
    build_cache::ensure_dir(&dir)?;
    let image = Image {
        h2: dir.join("h2"),
        out: Some(dir.clone()),
        env,
    };
    let cached = build_cache::matches_stamp(&dir, &key)?;
    Ok(ImagePlan { image, key, dir, features, pgo_hash, cached })
}

/// From the image cache when built before, else build a detached checkout
/// with the requested features and shared object cache.
fn ensure_image(
    sh: &Shared,
    id: &str,
    spec: &BuildSpec,
    commit: &str,
    plan: ImagePlan,
    log: &File,
) -> Result<Image> {
    let cfg = &sh.cfg;
    let ImagePlan { image, key, dir, features, pgo_hash, cached } = plan;
    let mut w = log.try_clone()?;
    if cached {
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
    let src = cfg.dir("src/current");
    for cache in ["target", "obj-cache"] {
        build_cache::ensure_dir(&cfg.dir(cache))?;
    }
    let stamp = dir.join("image.json");
    if stamp.try_exists()? {
        fs::remove_file(&stamp)?;
    }
    let build = (|| -> Result<()> {
        let git = build_cache::checkout_command(cfg, commit)?;
        let status = sh.step(id, git, log)?;
        ensure!(status.success(), "checking out {commit} failed (build.log)");

        let jobs = build_cache::workers().to_string();
        writeln!(w, "lab: cargo build at {commit}, {jobs} workers")?;
        let target = cfg.dir("target");
        let mut cargo = crate::follow::niced(&cfg.cargo);
        cargo.args(["build", "--release", "-p", "cli", "--bin", "h2"]);
        if !features.is_empty() {
            cargo.arg("--features").arg(features.join(","));
        }
        cargo
            .current_dir(&src)
            .env("CARGO_BUILD_JOBS", &jobs)
            .env("CARGO_TARGET_DIR", &target)
            .envs(&image.env);
        let st = sh.step(id, cargo, log)?;
        ensure!(st.success(), "cargo build failed (build.log)");
        fs::copy(target.join("release/h2"), dir.join("h2.tmp")).context("copying the built h2")?;
        fs::rename(dir.join("h2.tmp"), &image.h2)?;
        writeln!(w, "lab: h2 build into {}", dir.display())?;
        let mut build = crate::follow::niced(&image.h2);
        build.arg("build").arg(&cfg.disc).arg("--out").arg(&dir).arg("--jobs").arg(&jobs);
        for (k, _) in std::env::vars_os() {
            if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
                build.env_remove(k);
            }
        }
        build.envs(&image.env).env("H2_OBJ_CACHE", cfg.dir("obj-cache")).current_dir(&dir);
        let st = sh.step(id, build, log)?;
        ensure!(st.success(), "h2 build failed (build.log)");
        Ok(())
    })();
    let prune = build_cache::prune(cfg, &mut w);
    if let Err(err) = build {
        if let Err(prune) = prune {
            writeln!(w, "lab: cache cleanup failed: {prune:#}")?;
        }
        return Err(err);
    }
    prune?;
    let info = json!({ "key": key, "commit": commit, "features": features, "env": spec.env, "pgo_sha256": pgo_hash, "built": now() });
    let tmp = dir.join("image.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&info)?)?;
    fs::rename(tmp, stamp)?;
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
    if let Some(index) = inner.queue.iter().position(|pending| pending.id == id) {
        let m = inner.jobs.get_mut(id).context("missing queued job")?;
        let mut stopped = m.clone();
        stopped.state = "stopped".into();
        write_meta(&sh.cfg, &stopped)?;
        inner.jobs.remove(id);
        inner.queue.remove(index);
        sh.cv.notify_all();
        return Ok(json!({ "id": id, "stopping": true }));
    }
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
    load_meta(&cfg.dir("runs").join(id))?.with_context(|| format!("no job {id}"))
}

pub(super) fn load_meta(run_dir: &Path) -> Result<Option<Meta>> {
    let path = run_dir.join("meta.json");
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "{} is not a metadata file",
        path.display()
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let meta = serde_json::from_slice(&bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(meta))
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
fn summarize(mut log: impl BufRead) -> io::Result<Vec<String>> {
    // Logs may cover an entire soak run. Bound both the number of retained
    // lines and a single line, including output without any newlines.
    const LINE_LIMIT: u64 = 16 << 10;
    let mut out = Vec::new();
    let mut outcome = None;
    let mut looking_for_worst = false;
    let mut collecting_worst = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = (&mut log).take(LINE_LIMIT).read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        if n as u64 == LINE_LIMIT && !line.ends_with(b"\n") {
            log.skip_until(b'\n')?;
            collecting_worst = false;
            continue;
        }
        let Ok(text) = std::str::from_utf8(&line) else { continue };
        if text.starts_with("outcome:") {
            outcome = Some(text.trim().to_owned());
        }
        if text.starts_with("frame stats:") {
            out.clear();
            out.push(text.trim().to_owned());
            looking_for_worst = true;
            collecting_worst = false;
        } else if looking_for_worst && text.trim() == "worst frames:" {
            looking_for_worst = false;
            collecting_worst = true;
        } else if collecting_worst && text.trim_start().starts_with('#') {
            out.push(text.trim().to_owned());
            collecting_worst = out.len() < 4;
        } else {
            collecting_worst = false;
        }
    }
    out.extend(outcome);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `script` under `sh` in its own process group with its output
    /// in a fresh log, as `Shared::step` runs a game.
    fn game(name: &str, script: &str) -> (std::process::Child, PathBuf) {
        let log =
            std::env::temp_dir().join(format!("h2lab-wait-{name}-{}.log", std::process::id()));
        let file = File::create(&log).unwrap();
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .process_group(0)
            .spawn()
            .unwrap();
        (child, log)
    }

    fn config(root: &Path) -> HostConfig {
        HostConfig {
            root: root.to_owned(),
            socket: root.join("lab.sock"),
            repo: root.join("repo"),
            disc: root.join("halo2.iso"),
            h2: "/bin/true".into(),
            out: None,
            data: root.join("data"),
            cargo: "cargo".into(),
            obj_cache_limit: 0,
            target_limit: 0,
            max_jobs: 1,
            lan: None,
            managed: false,
            explicit_h2: true,
            grace: Duration::from_secs(1),
            takeover: takeover::Config::default(),
        }
    }

    #[test]
    fn own_data_runs_get_no_data_flag_and_keep_lives_variables() {
        let root = std::env::temp_dir().join(format!("h2lab-own-{}", std::process::id()));
        let cfg = config(&root);
        let image = Image { h2: "/bin/true".into(), out: None, env: BTreeMap::new() };
        let args = |run: &RunReq| -> Vec<String> {
            let cmd = play_command(&cfg, "0001", run, &image, None).unwrap();
            cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let has_data = |a: &[String]| a.iter().any(|x| x == "--data");
        let mut run = RunReq { name: "Play".into(), ..RunReq::default() };
        let plain = args(&run);
        let at = plain.iter().position(|x| x == "--data").unwrap();
        assert_eq!(plain[at + 1], root.join("data").to_string_lossy());
        run.fresh_data = true;
        let fresh = args(&run);
        assert!(fresh.iter().any(|x| x.ends_with("scratch/0001/data")));
        run.fresh_data = false;
        run.own_data = true;
        assert!(!has_data(&args(&run)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn own_data_runs_say_so_to_the_player_and_read_their_films() {
        let cfg = config(Path::new("/lab"));
        let mut run = RunReq::default();
        assert_eq!(run_data_root(&cfg, "0001", &run), Some("/lab/data".into()));
        run.fresh_data = true;
        assert_eq!(
            run_data_root(&cfg, "0001", &run),
            Some("/lab/scratch/0001/data".into())
        );
        run.fresh_data = false;
        run.own_data = true;
        assert_eq!(run_data_root(&cfg, "0001", &run), None);
        assert_eq!(
            film_library(&cfg, "0001", &run),
            content::store::default_data_root().join("films")
        );
        let ask = |run: &RunReq| {
            takeover_request(&Job::Run(run.clone()), None, "Lab", false, 60).revision
        };
        assert_eq!(ask(&run), "installed image, using your own data");
        run.own_data = false;
        assert_eq!(ask(&run), "installed image");
        for name in ["H2_DATA", "H2_LIVE_DIR", "H2_LIVE_SERVER"] {
            assert!(proto::OWN_DATA_ENV.contains(&name));
        }
        assert!(!proto::RUN_ENV.iter().any(|k| proto::OWN_DATA_ENV.contains(k)));
    }

    #[test]
    fn the_guest_exit_line_gives_the_outcome_and_its_status() {
        assert_eq!(
            guest_exit_outcome("app: guest thread exited: Returned\n"),
            Some("Returned")
        );
        assert_eq!(
            guest_exit_outcome("app: guest thread exited: Failed(\"boom\")"),
            Some("Failed(\"boom\")")
        );
        assert_eq!(guest_exit_outcome("outcome: SliceLimit"), None);
        assert!(status_for_outcome("Returned").success());
        assert_eq!(status_for_outcome("Panicked(\"x\")").code(), Some(1));
    }

    #[test]
    fn a_game_that_exits_by_itself_keeps_its_own_status() {
        let (mut child, log) = game("own", "echo 'app: guest thread exited: Returned'; exit 3");
        let (st, outcome) = wait_game(
            &mut child,
            &log,
            Duration::from_secs(30),
            Duration::from_secs(1),
        )
        .unwrap();
        let _ = fs::remove_file(&log);
        assert_eq!(st.code(), Some(3));
        assert_eq!(outcome, None);
    }

    #[test]
    fn a_game_that_lingers_after_its_guest_returned_is_stopped_with_that_outcome() {
        let t = Instant::now();
        let (mut child, log) = game(
            "linger",
            "echo 'frame stats: 300 frames'; echo 'app: guest thread exited: Returned'; exec sleep 60",
        );
        let (st, outcome) = wait_game(
            &mut child,
            &log,
            Duration::from_millis(100),
            Duration::from_secs(2),
        )
        .unwrap();
        let _ = fs::remove_file(&log);
        assert!(st.success(), "{st:?}");
        assert_eq!(outcome.as_deref(), Some("Returned"));
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    }

    #[test]
    fn a_lingering_game_that_ignores_sigterm_is_killed_and_keeps_a_failed_outcome() {
        let t = Instant::now();
        let (mut child, log) = game(
            "ignore",
            "trap '' TERM; echo 'app: guest thread exited: Failed(\"crash\")'; sleep 60",
        );
        let (st, outcome) = wait_game(
            &mut child,
            &log,
            Duration::from_millis(100),
            Duration::from_millis(300),
        )
        .unwrap();
        let _ = fs::remove_file(&log);
        assert_eq!(st.code(), Some(1));
        assert_eq!(outcome.as_deref(), Some("Failed(\"crash\")"));
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    }

    #[test]
    fn a_game_still_running_its_guest_is_not_stopped() {
        let (mut child, log) = game("running", "echo 'frame stats: 10 frames'; sleep 1");
        let (st, outcome) = wait_game(
            &mut child,
            &log,
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .unwrap();
        let _ = fs::remove_file(&log);
        assert!(st.success(), "{st:?}");
        assert_eq!(outcome, None);
    }

    #[test]
    fn the_summary_is_the_last_report_and_its_worst_frames() {
        let log = "entry 0x1, running\n\
            frame stats: 10 frames from 0, 30.00 fps; frame ms p50 33.0 p90 33.0 p99 40.0 max 41.0; over 33.3 ms: 2\n\
            frame stats: 900 frames from 300, 29.10 fps; frame ms p50 33.1 p90 34.0 p99 52.0 max 80.2; over 33.3 ms: 40\n  \
            mean ms/frame: cpu 10.00\n  mean per frame: draws 900.0\n  worst frames:\n    \
            #1 80.2 ms: cpu 50.0\n    #2 60.1 ms: cpu 40.0\n    #3 55.0 ms: cpu 30.0\n    #4 54.0 ms: cpu 30.0\n\
            DbgPrint: x\noutcome: stopped at present 1200\n";
        let s = summarize(log.as_bytes()).unwrap();
        assert_eq!(s.len(), 5, "{s:?}");
        assert!(s[0].starts_with("frame stats: 900 frames"));
        assert_eq!(s[3], "#3 55.0 ms: cpu 30.0");
        assert_eq!(s[4], "outcome: stopped at present 1200");
        assert!(summarize("nothing here".as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn oversized_log_lines_are_skipped_without_losing_later_reports() {
        let mut log = b"frame stats: ".to_vec();
        log.resize(1 << 20, b'x');
        log.extend_from_slice(b"\nframe stats: small\noutcome: complete");
        assert_eq!(summarize(log.as_slice()).unwrap(), ["frame stats: small", "outcome: complete"]);
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
