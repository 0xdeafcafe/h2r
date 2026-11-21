//! `h2 lab host`: the lab's side on the machine that plays. Each job keeps
//! its log and metadata in `runs/<id>/`. Jobs can overlap, with at most one
//! building an image at a time. See docs/runtime/lab.md.

use std::collections::{BTreeMap, HashMap};
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
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::proto::{self, BuildSpec, INPUT_FILE, Manifest, Request, RunReq};
use super::transfer::Transfers;
use super::{discovery, lan, service, takeover, transfer};

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
    /// Persistent data for serial runs and the seed for concurrent runs.
    pub data: PathBuf,
    pub cargo: PathBuf,
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
    /// `run` or `build`.
    pub kind: String,
    /// queued, awaiting_consent, awaiting_quit, building, running, then
    /// done, failed, stopped or skipped.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// The frame stats report lines from the log.
    pub summary: Vec<String>,
    pub notes: Vec<String>,
    /// The game's agent API socket, for `h2 lab agent` (runs only).
    pub agent_socket: Option<String>,
    /// The `h2` that runs it, which also makes the agent calls.
    pub h2: Option<String>,
}

fn is_final(state: &str) -> bool {
    matches!(state, "done" | "failed" | "stopped" | "skipped")
}

struct Active {
    pgid: Option<i32>,
    stop: bool,
    building: bool,
}

#[derive(Default)]
struct Inner {
    active: BTreeMap<String, Active>,
    /// Keep maintenance and other processes out until the last job finishes.
    activity_lock: Option<File>,
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Serves the private socket and, when configured, the paired LAN listener.
pub fn serve(cfg: HostConfig) -> Result<()> {
    for d in ["runs", "recordings", "pgo", "images", "src", "scratch", "a"] {
        fs::create_dir_all(cfg.dir(d))
            .with_context(|| format!("creating {}", cfg.dir(d).display()))?;
    }
    fs::create_dir_all(&cfg.data)?;
    recover(&cfg)?;
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
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("activity.lock"))
        .with_context(|| format!("opening the lab activity lock in {}", root.display()))
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
fn recover(cfg: &HostConfig) -> Result<()> {
    // A host on another socket can have the same root. Recover only while
    // no host is writing jobs there, including during a LaunchAgent restart.
    let lock = open_activity_lock(&cfg.root)?;
    lock.lock().context("locking the lab root for job recovery")?;
    for m in disk_metas(cfg) {
        if !is_final(&m.state) {
            let mut m = m;
            m.state = "failed".into();
            m.error = Some("the host stopped while this job ran".into());
            let _ = write_meta(cfg, &m);
        }
    }
    Ok(())
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
                fetch(sh, &mut conn, &id)?;
                return Ok(());
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

fn fetch(sh: &Shared, conn: &mut BufReader<impl Read + Write>, id: &str) -> Result<()> {
    let runs = sh.cfg.dir("runs");
    let child = (|| {
        ensure!(runs.join(id).join("meta.json").is_file(), "no job {id}");
        Command::new("tar")
            .arg("-C")
            .arg(&runs)
            .args(["-cf", "-", id])
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
            let inner = sh.inner.lock().unwrap();
            let jobs_active: Vec<_> = inner
                .active
                .keys()
                .filter_map(|id| inner.jobs.get(id))
                .collect();
            Ok(json!({
                "version": proto::VERSION,
                "build_version": env!("CARGO_PKG_VERSION"),
                "build_commit": app::BuildInfo::baked().sha,
                "pid": std::process::id(),
                "listen": sh.listen_addr,
                "name": sh.service_name,
                "root": cfg.root,
                "repo": cfg.repo,
                "disc": cfg.disc,
                "h2": cfg.h2,
                "active": jobs_active.first(),
                "jobs_active": jobs_active,
                "max_jobs": cfg.max_jobs,
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
        Request::Fetch { .. } | Request::Sync { .. } | Request::Update => unreachable!(),
        Request::Run(run) => start(sh, Job::Run(run), requester),
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

fn start(sh: &Arc<Shared>, job: Job, requester: &str) -> Result<Value> {
    let cfg = &sh.cfg;
    // Resolve image and recording inputs before admitting the job.
    let mut image = match &job.spec().rev {
        Some(rev) => Some(prepare_image(cfg, job.spec(), resolve_rev(&cfg.repo, rev)?)?),
        None => None,
    };
    let mut level = None;
    let mut recording_job = None;
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
            recording_job = m.run;
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
    if inner.active.len() >= cfg.max_jobs {
        if cfg.max_jobs == 1
            && let Some(id) = inner.active.keys().next()
        {
            let state = inner.jobs[id].state.as_str();
            bail!(
                "busy: job {id} is {state}; one job at a time (h2 lab wait {id} or h2 lab stop {id})"
            );
        }
        let ids = inner.active.keys().map(String::as_str).collect::<Vec<_>>();
        bail!(
            "busy: --max-jobs {} reached; active jobs: {}",
            cfg.max_jobs,
            ids.join(", ")
        );
    }
    if let Some(id) = recording_job
        && inner.active.contains_key(&id)
    {
        bail!("job {id} is still recording (h2 lab wait {id})");
    }
    let lock = if inner.active.is_empty() {
        Some(activity_lock(&cfg.root)?)
    } else {
        None
    };
    // Check the cache and reserve the build slot under the same mutex.
    let needs_build = image.as_mut().is_some_and(|image| {
        image.cached = image.dir.join("ready").is_file();
        !image.cached
    });
    if needs_build
        && let Some((id, _)) = inner.active.iter().find(|(_, active)| active.building)
    {
        bail!("busy: job {id} is building an image (h2 lab wait {id})");
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
        commit: image.as_ref().map(|image| image.commit.clone()),
        image: image.as_ref().map(|image| image.key.clone()),
        cached: image.as_ref().map(|image| image.cached),
        build_env: job.spec().env.clone(),
        level,
        created: now(),
        ..Meta::default()
    };
    write_meta(cfg, &m)?;
    inner.jobs.insert(id.clone(), m.clone());
    inner.active.insert(
        id.clone(),
        Active {
            pgid: None,
            stop: false,
            building: needs_build,
        },
    );
    if lock.is_some() {
        inner.activity_lock = lock;
    }
    drop(inner);
    let sh2 = Arc::clone(sh);
    let id2 = id.clone();
    let requester = requester.to_owned();
    std::thread::spawn(move || execute(&sh2, &id2, job, image, &requester));
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
        self.inner.lock().unwrap().active.get(id).is_some_and(|a| a.stop)
    }

    fn owns_game(&self, socket: Option<&Path>, pid: Option<u32>) -> bool {
        let inner = self.inner.lock().unwrap();
        let group = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|&pid| pid > 0)
            .map(|pid| {
                // SAFETY: getpgid only queries the process group of a positive PID.
                unsafe { libc::getpgid(pid) }
            });
        inner.active.iter().any(|(id, active)| {
            inner.jobs[id].state == "running"
                && ((group.is_some() && group == active.pgid)
                    || socket.is_some_and(|path| path == agent_socket_path(&self.cfg, id).as_path()))
        })
    }

    /// Runs one step of job `id` in its own process group, so stop reaches
    /// what it starts (cargo's rustc, the game's helpers) too.
    fn step(&self, id: &str, mut cmd: Command, log: &File) -> Result<std::process::ExitStatus> {
        cmd.stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?)
            .process_group(0);
        // Publish the process group before stop can inspect this job.
        let mut inner = self.inner.lock().unwrap();
        let active = inner.active.get_mut(id).expect("executing job is active");
        if active.stop {
            return Err(Stopped.into());
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {:?}", cmd.get_program()))?;
        let pgid = child.id() as i32;
        active.pgid = Some(pgid);
        drop(inner);

        let status = child.wait();
        let mut inner = self.inner.lock().unwrap();
        // Whatever the group left behind goes with it.
        // SAFETY: signalling the group we made. It may already be gone.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        let active = inner.active.get_mut(id).expect("executing job is active");
        active.pgid = None;
        if active.stop {
            return Err(Stopped.into());
        }
        Ok(status?)
    }
}

/// A built image: the `h2` to run it with and its environment.
struct Image {
    h2: PathBuf,
    out: Option<PathBuf>,
    env: BTreeMap<String, String>,
}

impl Image {
    fn command(&self, action: &str, disc: &Path) -> Command {
        let mut cmd = Command::new(&self.h2);
        cmd.arg(action).arg(disc);
        if let Some(out) = &self.out {
            cmd.arg("--out").arg(out);
        }
        for (k, _) in std::env::vars_os() {
            if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
                cmd.env_remove(k);
            }
        }
        cmd.envs(&self.env);
        cmd
    }
}

/// A revision's image and the inputs needed to populate its cache entry.
struct RevisionImage {
    commit: String,
    key: String,
    dir: PathBuf,
    features: Vec<String>,
    pgo_hash: String,
    image: Image,
    cached: bool,
}

enum JobOutcome {
    Finished(Option<std::process::ExitStatus>),
    Skipped(&'static str),
}

fn execute(
    sh: &Shared,
    id: &str,
    job: Job,
    image: Option<RevisionImage>,
    requester: &str,
) {
    let t = Instant::now();
    let outcome = run_job(sh, id, &job, image, requester);
    let (state, error) = match &outcome {
        Ok(JobOutcome::Finished(Some(st))) if st.success() => ("done", None),
        Ok(JobOutcome::Finished(Some(st))) => (
            "failed",
            Some(match (st.code(), st.signal()) {
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
    let summary = fs::read_to_string(run_dir.join("run.log"))
        .map(|l| summarize(&l))
        .unwrap_or_default();
    let (status, skip_reason) = match outcome {
        Ok(JobOutcome::Finished(status)) => (status, None),
        Ok(JobOutcome::Skipped(reason)) => (None, Some(reason.to_owned())),
        Err(_) => (None, None),
    };
    let _ = fs::remove_dir_all(sh.cfg.dir("scratch").join(id));
    let _ = fs::remove_file(agent_socket_path(&sh.cfg, id));
    let mut inner = sh.inner.lock().unwrap();
    let active = inner.active.remove(id).expect("executing job is active");
    let state = if state == "failed" && active.stop {
        "stopped"
    } else {
        state
    };
    if let Some(m) = inner.jobs.get_mut(id) {
        m.state = state.into();
        m.error = error;
        m.skip_reason = skip_reason;
        m.duration_s = Some(t.elapsed().as_secs_f64());
        m.exit_code = status.and_then(|s| s.code());
        m.signal = status.and_then(|s| s.signal());
        m.summary = summary;
        let _ = write_meta(&sh.cfg, m);
    }
    if inner.active.is_empty() {
        drop(inner.activity_lock.take());
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
    TakeoverRequest {
        requester: requester.to_owned(),
        operation,
        revision: commit.map_or_else(
            || "installed image".into(),
            |sha| sha.chars().take(12).collect(),
        ),
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
    sh: &Shared,
    id: &str,
    job: &Job,
    revision: Option<RevisionImage>,
    requester: &str,
) -> Result<JobOutcome> {
    let cfg = &sh.cfg;
    let run_dir = cfg.dir("runs").join(id);
    let commit = revision.as_ref().map(|image| image.commit.clone());
    let builds_image = revision.as_ref().is_some_and(|image| !image.cached);
    let consent = |builds_image| {
        request_takeover(
            sh,
            id,
            takeover_request(
                job,
                commit.as_deref(),
                requester,
                builds_image,
                cfg.takeover.timeout_seconds,
            ),
        )
    };
    if (builds_image || matches!(job, Job::Run(_))) && let Some(skipped) = consent(builds_image)? {
        return Ok(skipped);
    }
    let image = match revision {
        Some(revision) => {
            let mut log = File::create(run_dir.join("build.log"))?;
            if revision.cached {
                writeln!(
                    log,
                    "lab: image {} for {} is built",
                    revision.key, revision.commit
                )?;
            } else {
                sh.update(id, |m| m.state = "building".into());
                build_image(sh, id, job.spec(), &revision, &log)?;
                if matches!(job, Job::Run(_)) {
                    sh.update(id, |m| m.state = "queued".into());
                }
                let mut inner = sh.inner.lock().unwrap();
                let active = inner.active.get_mut(id).expect("executing job is active");
                active.building = false;
                if active.stop {
                    return Err(Stopped.into());
                }
            }
            revision.image
        }
        None => Image {
            h2: cfg.h2.clone(),
            out: cfg.out.clone(),
            env: BTreeMap::new(),
        },
    };
    let Job::Run(run) = job else { return Ok(JobOutcome::Finished(None)) };
    // A player can open the game during a long build. Ask before starting
    // the run too if there was an uncached build in between.
    if builds_image && let Some(skipped) = consent(false)? {
        return Ok(skipped);
    }
    let log = File::create(run_dir.join("run.log"))?;
    if cfg.max_jobs > 1 {
        // A stale image would make `play` build outside the reserved slot.
        let mut check = image.command("build", &cfg.disc);
        check.arg("--check").current_dir(&run_dir);
        let status = sh.step(id, check, &log)?;
        ensure!(
            status.success(),
            "--max-jobs > 1 requires a current image verified by h2 build --check (run.log)"
        );
    }
    let data = prepare_run_data(sh, id, run, &log)?;
    let cmd = play_command(cfg, id, run, &image, commit.as_deref(), &data)?;
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
    Ok(JobOutcome::Finished(Some(st)))
}

fn prepare_run_data(sh: &Shared, id: &str, run: &RunReq, log: &File) -> Result<PathBuf> {
    let cfg = &sh.cfg;
    if !run.fresh_data && cfg.max_jobs == 1 {
        return Ok(cfg.data.clone());
    }
    let scratch = cfg.dir("scratch").join(id);
    let data = scratch.join("data");
    if run.fresh_data {
        fs::create_dir_all(&data)?;
        return Ok(data);
    }

    fs::create_dir_all(&scratch)?;
    let seed = cfg.data.canonicalize()?;
    ensure!(
        !scratch.canonicalize()?.starts_with(&seed),
        "the lab scratch directory must be outside the --data seed"
    );
    let mut copy = Command::new("cp");
    if cfg!(target_os = "macos") {
        // macOS cp falls back to a copy when the filesystem cannot clone.
        copy.arg("-c");
    }
    copy.args(["-R", "-L"]).arg(&seed).arg(&data);
    let status = sh.step(id, copy, log)?;
    ensure!(status.success(), "copying the lab data seed failed (run.log)");
    Ok(data)
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
    data: &Path,
) -> Result<Command> {
    let run_dir = cfg.dir("runs").join(id);
    let mut cmd = image.command("play", &cfg.disc);
    cmd.arg("--data").arg(data);
    let mut level = run.level.clone();
    let mut difficulty = run.difficulty.clone();
    if cfg.max_jobs > 1 && image.env.get("H2_PGO").is_some_and(|pgo| pgo == "gen") {
        cmd.env("H2_PGO_OUT", run_dir.join("h2.profraw"));
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
    // The host owns the agent socket and the output root (proto::RUN_ENV).
    let sock = agent_socket_path(cfg, id);
    if let Some(dir) = sock.parent() {
        fs::create_dir_all(dir)?;
    }
    let _ = fs::remove_file(&sock);
    cmd.env(agent_proto::ENV, &sock)
        .env(agent_proto::OUT_ENV, &run_dir);
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
/// with its exit code and output. The run's own `h2` understands the short
/// forms of its image. `flight show` always reads from the game's socket.
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
    let data = if run.fresh_data {
        let data = cfg.dir("scratch").join(&id).join("data");
        fs::create_dir_all(&data)?;
        data
    } else {
        cfg.data.clone()
    };
    let mut cmd = play_command(cfg, &id, run, &image, None, &data)?;
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

fn prepare_image(cfg: &HostConfig, spec: &BuildSpec, commit: String) -> Result<RevisionImage> {
    let mut env = spec.env.clone();
    let mut pgo_hash = String::new();
    if let Some(name) = proto::pgo_from(&spec.env)? {
        let p = cfg.dir("pgo").join(name);
        ensure!(
            p.is_file(),
            "no PGO profile pgo/{name} (h2 lab put --pgo FILE)"
        );
        pgo_hash = hex_sha(&fs::read(&p).with_context(|| format!("reading {}", p.display()))?);
        env.insert("H2_PGO".into(), format!("use:{}", p.display()));
    }
    let mut features = spec.features.clone();
    features.sort();
    features.dedup();
    let key = image_key(&commit, &features, &spec.env, &pgo_hash);
    let dir = cfg.dir("images").join(&key);
    let image = Image {
        h2: dir.join("h2"),
        out: Some(dir.join("out")),
        env,
    };
    Ok(RevisionImage {
        commit,
        key,
        dir,
        features,
        pgo_hash,
        image,
        cached: false,
    })
}

/// Fill the reserved cache entry from a detached worktree, the shared
/// cargo target and the shared object cache. Publish `ready` last.
fn build_image(
    sh: &Shared,
    id: &str,
    spec: &BuildSpec,
    revision: &RevisionImage,
    log: &File,
) -> Result<()> {
    let cfg = &sh.cfg;
    let RevisionImage {
        commit,
        dir,
        features,
        pgo_hash,
        image,
        ..
    } = revision;
    let mut w = log.try_clone()?;
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
    fs::create_dir_all(dir)?;
    fs::copy(target.join("release/h2"), &image.h2).context("copying the built h2")?;
    writeln!(w, "lab: h2 build into {}", dir.join("out").display())?;
    let mut build = image.command("build", &cfg.disc);
    build
        .env("H2_OBJ_CACHE", cfg.dir("obj-cache"))
        .current_dir(dir);
    let st = sh.step(id, build, log)?;
    ensure!(st.success(), "h2 build failed (build.log)");
    let info = json!({ "commit": commit, "features": features, "env": spec.env, "pgo_sha256": pgo_hash, "built": now() });
    fs::write(dir.join("image.json"), serde_json::to_vec_pretty(&info)?)?;
    fs::write(dir.join("ready"), b"")?;
    Ok(())
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
    let Some(a) = inner.active.get_mut(id) else {
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
            if inner.active.get(&id2).and_then(|a| a.pgid) == Some(pgid) {
                // SAFETY: still the job's group.
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
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
