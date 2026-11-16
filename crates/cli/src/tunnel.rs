//! `--tunnel`: a Live server reachable from the internet through Cloudflare
//! Tunnel (`cloudflared`), without TLS certificates or port forwarding
//! (docs/live-hosting.md).
//!
//! The server keeps listening on loopback; cloudflared connects out to
//! Cloudflare and forwards each request (and WebSocket) to it.
//!
//! - Quick tunnel (`--tunnel`): `cloudflared tunnel --url <origin>`, no
//!   Cloudflare account; it prints a fresh `https://*.trycloudflare.com`
//!   URL each run, which [`quick_url`] finds in its log.
//! - Named tunnel (`--tunnel-name NAME [--hostname HOST]`, on `h2 play`
//!   also `--live-tunnel-name NAME --live-tunnel-url https://HOST`): the URL
//!   stays `https://HOST`. With a host name it is set up first when missing
//!   ([`provision`]: `cloudflared tunnel login`, `create`, `route dns`)
//!   unless `--no-tunnel-setup`.
//!
//! cloudflared is supervised: if it exits it is started again with backoff
//! (a quick tunnel then gets a new URL, reported through the callback), and
//! it is stopped when the [`Tunnel`] is dropped.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

/// How to install cloudflared, for the error when it is missing.
pub const INSTALL_HINT: &str = "install it with `brew install cloudflared` (or see \
     https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/), \
     or point $H2_CLOUDFLARED at it";

/// How long a start may take before it counts as failed.
pub const START_TIMEOUT: Duration = Duration::from_secs(45);
/// Lines of cloudflared's output kept for error messages.
const TAIL: usize = 12;
/// Restart backoff: first wait, cap, and how long a run must last to reset it.
const BACKOFF_FIRST: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const BACKOFF_RESET: Duration = Duration::from_secs(60);

/// Which tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// A throwaway `*.trycloudflare.com` URL, new each run.
    Quick,
    /// `cloudflared tunnel run NAME`, at `https://HOST` when given.
    Named {
        name: String,
        hostname: Option<String>,
    },
}

impl Mode {
    /// From `--tunnel`, `--tunnel-name` and `--hostname`.
    pub fn from_flags(
        quick: bool,
        name: Option<String>,
        hostname: Option<String>,
    ) -> Result<Option<Mode>> {
        if let Some(h) = &hostname {
            valid_hostname(h)?;
        }
        match (quick, name, hostname) {
            (true, Some(_), _) => {
                bail!("--tunnel is a quick tunnel; --tunnel-name runs a named one")
            }
            (true, None, Some(_)) => bail!(
                "--hostname goes with --tunnel-name (a quick tunnel's host name is chosen by Cloudflare)"
            ),
            (true, None, None) => Ok(Some(Mode::Quick)),
            (false, Some(name), hostname) => {
                if name.is_empty() || name.starts_with('-') {
                    bail!("--tunnel-name wants the tunnel's name or UUID");
                }
                Ok(Some(Mode::Named { name, hostname }))
            }
            (false, None, Some(_)) => bail!("--hostname goes with --tunnel-name"),
            (false, None, None) => Ok(None),
        }
    }

    /// The public base URL known before cloudflared starts (a named tunnel's).
    pub fn fixed_url(&self) -> Option<String> {
        match self {
            Mode::Named {
                hostname: Some(h), ..
            } => Some(format!("https://{}", h.to_ascii_lowercase())),
            _ => None,
        }
    }
}

fn valid_hostname(h: &str) -> Result<()> {
    let ok = !h.is_empty()
        && h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if !ok {
        bail!(
            "--hostname {h:?} must be a DNS name like live.example.net (no scheme, port or path)"
        );
    }
    Ok(())
}

/// The host name in a tunnel URL (`--live-tunnel-url`): `https://HOST`,
/// with at most a trailing `/`; no port, path, query or user name.
pub fn hostname_from_url(url: &str) -> Result<String> {
    let bad = || {
        anyhow!(
            "--live-tunnel-url {url:?} must be https://HOST with no port or path, like \
             https://play.example.net"
        )
    };
    let rest = url
        .get(..8)
        .filter(|s| s.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])
        .ok_or_else(bad)?;
    let host = rest.strip_suffix('/').unwrap_or(rest);
    valid_hostname(host).map_err(|_| bad())?;
    Ok(host.to_ascii_lowercase())
}

/// Where cloudflared keeps the origin certificate (`cert.pem`) that
/// `cloudflared tunnel login` writes and that listing, creating and routing
/// tunnels need. cloudflared's own rules: `$TUNNEL_ORIGIN_CERT` (its
/// `--origincert`) names the file outright; else the first `cert.pem` in
/// its default directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginCert {
    /// `$TUNNEL_ORIGIN_CERT`: this file and no other.
    pub explicit: Option<PathBuf>,
    /// The directories searched for `cert.pem`, in order.
    pub dirs: Vec<PathBuf>,
}

impl OriginCert {
    pub fn from_env() -> OriginCert {
        OriginCert::new(
            std::env::var_os("TUNNEL_ORIGIN_CERT"),
            std::env::var_os("HOME"),
        )
    }

    pub fn new(explicit: Option<OsString>, home: Option<OsString>) -> OriginCert {
        let mut dirs = Vec::new();
        if let Some(home) = home.filter(|h| !h.is_empty()).map(PathBuf::from) {
            dirs.extend(
                [".cloudflared", ".cloudflare-warp", "cloudflare-warp"].map(|d| home.join(d)),
            );
        }
        dirs.extend(["/etc/cloudflared", "/usr/local/etc/cloudflared"].map(PathBuf::from));
        OriginCert {
            explicit: explicit.filter(|p| !p.is_empty()).map(PathBuf::from),
            dirs,
        }
    }

    /// The certificate, if there is one.
    pub fn find(&self) -> Option<PathBuf> {
        match &self.explicit {
            Some(p) => p.is_file().then(|| p.clone()),
            None => self
                .dirs
                .iter()
                .map(|d| d.join("cert.pem"))
                .find(|p| p.is_file()),
        }
    }
}

/// What `cloudflared tunnel route dns` said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routed {
    Added,
    Already,
}

/// Makes named tunnel `name` exist and serve `hostname`, doing what is
/// missing, one line to `say` per step:
///
/// 1. no origin certificate: `cloudflared tunnel login`, on this terminal
///    (it prints a link to open in a browser and waits);
/// 2. no tunnel called `name` in `cloudflared tunnel list --output json`:
///    `cloudflared tunnel create NAME`;
/// 3. `cloudflared tunnel route dns NAME HOST`, which is fine to repeat. A
///    DNS record of another kind at HOST is a conflict only its owner can
///    settle, so that is an error saying so.
pub fn provision(
    program: &Path,
    name: &str,
    hostname: &str,
    cert: &OriginCert,
    say: &mut dyn FnMut(&str),
) -> Result<Routed> {
    if cert.find().is_none() {
        if let Some(p) = &cert.explicit {
            bail!(
                "$TUNNEL_ORIGIN_CERT is {}, which does not exist; run `cloudflared tunnel login` \
                 and move the cert.pem it writes there, or unset TUNNEL_ORIGIN_CERT",
                p.display()
            );
        }
        say(
            "no Cloudflare login on this machine; running `cloudflared tunnel login`: open the \
             link it prints and pick the domain for the tunnel",
        );
        let status = Command::new(program)
            .args(["tunnel", "login"])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| spawn_error(program, e))?;
        if !status.success() {
            bail!("`cloudflared tunnel login` failed ({status})");
        }
        let Some(found) = cert.find() else {
            bail!(
                "`cloudflared tunnel login` finished but there is still no cert.pem in {}",
                cert.dirs
                    .first()
                    .map_or_else(|| "~/.cloudflared".into(), |d| d.display().to_string())
            );
        };
        say(&format!("logged in to Cloudflare ({})", found.display()));
    }

    let list = captured(program, &["tunnel", "list", "--output", "json"])?;
    if !list.ok {
        bail!("`cloudflared tunnel list` failed:\n  {}", list.tail());
    }
    match listed_id(&list.stdout, name)? {
        Some(id) => say(&format!("tunnel {name} exists ({id})")),
        None => {
            say(&format!("creating tunnel {name}"));
            let made = captured(program, &["tunnel", "create", name])?;
            if !made.ok {
                bail!(
                    "`cloudflared tunnel create {name}` failed:\n  {}",
                    made.tail()
                );
            }
        }
    }

    say(&format!("routing {hostname} to tunnel {name}"));
    let route = captured(program, &["tunnel", "route", "dns", name, hostname])?;
    let routed = route_outcome(route.ok, &route.all, name, hostname)?;
    if routed == Routed::Already {
        say(&format!("{hostname} already routes to tunnel {name}"));
    }
    Ok(routed)
}

/// The id of the live tunnel called (or with the id) `name` in `cloudflared
/// tunnel list --output json`.
pub fn listed_id(json: &str, name: &str) -> Result<Option<String>> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| {
        anyhow!("`cloudflared tunnel list --output json` printed something else ({e}): {json}")
    })?;
    let Some(tunnels) = v.as_array() else {
        return Ok(None); // `null`: none
    };
    let field =
        |t: &serde_json::Value, k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Ok(tunnels
        .iter()
        .filter(|t| {
            // Deleted tunnels carry a real deletion time; live ones Go's zero time.
            field(t, "deleted_at").is_none_or(|d| d.starts_with("0001-01-01"))
        })
        .find(|t| {
            field(t, "name").as_deref() == Some(name)
                || field(t, "id").is_some_and(|id| id.eq_ignore_ascii_case(name))
        })
        .map(|t| field(t, "id").unwrap_or_default()))
}

/// `cloudflared tunnel route dns`'s outcome from its exit and output.
pub fn route_outcome(ok: bool, output: &str, name: &str, hostname: &str) -> Result<Routed> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("already configured") {
        return Ok(Routed::Already);
    }
    if ok {
        return Ok(Routed::Added);
    }
    if lower.contains("already exists") || lower.contains("code: 1003") {
        bail!(
            "{hostname} already has a DNS record that is not tunnel {name}'s, and cloudflared \
             will not replace it. Delete the old {hostname} record in the Cloudflare dashboard \
             (your domain, DNS, Records), then run this again; or pick another host name.\n\
             cloudflared said:\n  {}",
            output.trim().replace('\n', "\n  ")
        );
    }
    bail!(
        "`cloudflared tunnel route dns {name} {hostname}` failed:\n  {}",
        output.trim().replace('\n', "\n  ")
    )
}

/// A finished cloudflared command's result.
struct Captured {
    ok: bool,
    stdout: String,
    /// stdout then stderr.
    all: String,
}

impl Captured {
    fn tail(&self) -> String {
        let lines: Vec<_> = self.all.trim().lines().collect();
        lines[lines.len().saturating_sub(TAIL)..].join("\n  ")
    }
}

fn captured(program: &Path, args: &[&str]) -> Result<Captured> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| spawn_error(program, e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let all = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    Ok(Captured {
        ok: out.status.success(),
        stdout,
        all,
    })
}

fn spawn_error(program: &Path, e: std::io::Error) -> anyhow::Error {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            anyhow!("{} is missing; {INSTALL_HINT}", program.display())
        }
        _ => anyhow!("starting {}: {e}", program.display()),
    }
}

/// cloudflared's arguments for `mode`, forwarding to `origin`
/// (`http://127.0.0.1:<port>`). `--grace-period` keeps shutdown quick: the
/// server behind it is stopping too.
pub fn args(mode: &Mode, origin: &str) -> Vec<String> {
    let mut a: Vec<String> = ["tunnel", "--no-autoupdate", "--grace-period", "1s"]
        .map(String::from)
        .to_vec();
    match mode {
        Mode::Quick => a.extend(["--url".into(), origin.into()]),
        Mode::Named { name, .. } => {
            a.extend(["run".into(), "--url".into(), origin.into(), name.clone()]);
        }
    }
    a
}

/// `$H2_CLOUDFLARED` if set, else `cloudflared` on `path_env`.
pub fn find(explicit: Option<OsString>, path_env: Option<OsString>) -> Result<PathBuf> {
    if let Some(p) = explicit.filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        if is_executable(&p) {
            return Ok(p);
        }
        bail!(
            "$H2_CLOUDFLARED is {}, which is not an executable file",
            p.display()
        );
    }
    path_env
        .iter()
        .flat_map(std::env::split_paths)
        .map(|d| d.join(format!("cloudflared{}", std::env::consts::EXE_SUFFIX)))
        .find(|p| is_executable(p))
        .ok_or_else(|| anyhow::anyhow!("cloudflared was not found on PATH; {INSTALL_HINT}"))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Windows has no executable bit; `cloudflared` is found as a file.
#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// The quick tunnel's URL if `line` announces it: an
/// `https://<name>.trycloudflare.com` with a single-label name, in the plain
/// or JSON log format. Cloudflare's own links (`www.cloudflare.com`,
/// `api.trycloudflare.com`) are not it.
pub fn quick_url(line: &str) -> Option<String> {
    const SUFFIX: &str = ".trycloudflare.com";
    let mut rest = line;
    while let Some(at) = rest.find("https://") {
        let after = &rest[at + "https://".len()..];
        let host_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
            .unwrap_or(after.len());
        let host = after[..host_len].to_ascii_lowercase();
        if let Some(name) = host.strip_suffix(SUFFIX) {
            let single =
                !name.is_empty() && !name.contains('.') && !name.starts_with('-') && name != "api";
            // The URL ends at the host: no path or port.
            let end = after[host_len..].chars().next();
            if single && !matches!(end, Some('/' | ':')) {
                return Some(format!("https://{host}"));
            }
        }
        rest = &after[host_len..];
    }
    None
}

/// Whether `line` says a connection to Cloudflare's edge is up.
pub fn registered(line: &str) -> bool {
    line.contains("Registered tunnel connection") || line.contains("Connection registered")
}

/// One cloudflared process and its output (both streams, by line). Dropping
/// it stops the process.
struct Run {
    child: Child,
    lines: Receiver<String>,
    tail: VecDeque<String>,
}

impl Drop for Run {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Run {
    fn spawn(program: &Path, args: &[String]) -> Result<Run> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| spawn_error(program, e))?;
        let (tx, lines) = mpsc::channel();
        for out in [
            child
                .stdout
                .take()
                .map(|o| Box::new(o) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|o| Box::new(o) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("cloudflared-log".into())
                .spawn(move || {
                    for line in BufReader::new(out).lines() {
                        let Ok(line) = line else { break };
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                })?;
        }
        Ok(Run {
            child,
            lines,
            tail: VecDeque::new(),
        })
    }

    fn keep(&mut self, line: String) {
        if self.tail.len() == TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(line);
    }

    /// Reads what is waiting, without blocking.
    fn drain(&mut self) {
        while let Ok(l) = self.lines.try_recv() {
            self.keep(l);
        }
    }

    fn failure(&mut self, what: &str) -> anyhow::Error {
        self.drain();
        let tail: Vec<_> = self.tail.iter().map(String::as_str).collect();
        if tail.is_empty() {
            anyhow::anyhow!("cloudflared {what} (it printed nothing)")
        } else {
            anyhow::anyhow!(
                "cloudflared {what}; its last output:\n  {}",
                tail.join("\n  ")
            )
        }
    }

    /// Waits until the tunnel is up: the quick tunnel's URL, or (named) a
    /// registered connection. Returns the public URL if one is known.
    fn ready(
        &mut self,
        mode: &Mode,
        timeout: Duration,
        stop: &AtomicBool,
    ) -> Result<Option<String>> {
        let deadline = Instant::now() + timeout;
        let up = |l: &str| match mode {
            Mode::Quick => quick_url(l).map(Some),
            Mode::Named { .. } => registered(l).then(|| mode.fixed_url()),
        };
        loop {
            if stop.load(Ordering::SeqCst) {
                bail!("stopped");
            }
            if let Ok(l) = self.lines.recv_timeout(Duration::from_millis(100)) {
                let found = up(&l);
                self.keep(l);
                if let Some(url) = found {
                    return Ok(url);
                }
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                // What it printed before exiting may still be in the pipes.
                while let Ok(l) = self.lines.recv_timeout(Duration::from_millis(500)) {
                    let found = up(&l);
                    self.keep(l);
                    if let Some(url) = found {
                        return Ok(url);
                    }
                }
                return Err(self.failure(&format!("exited ({status}) before the tunnel was up")));
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(self.failure(&format!(
                    "did not bring the tunnel up within {} s",
                    timeout.as_secs()
                )));
            }
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Called with the public URL whenever it is (re)learned.
pub type OnUrl = Box<dyn Fn(&str) + Send + Sync>;

/// A supervised cloudflared. Dropping it stops cloudflared.
pub struct Tunnel {
    /// The public base URL (`https://...`), if known.
    pub url: Option<String>,
    run: Arc<Mutex<Option<Run>>>,
    stop: Arc<AtomicBool>,
    supervisor: Option<JoinHandle<()>>,
}

impl Tunnel {
    /// Starts cloudflared (`program`) forwarding to `origin` and waits until
    /// the tunnel is up. `on_url` hears every later URL change (a quick
    /// tunnel restarted after cloudflared died).
    pub fn start(
        program: &Path,
        mode: Mode,
        origin: &str,
        timeout: Duration,
        on_url: OnUrl,
    ) -> Result<Tunnel> {
        let args = args(&mode, origin);
        let stop = Arc::new(AtomicBool::new(false));
        let mut first = Run::spawn(program, &args)?;
        let url = first.ready(&mode, timeout, &stop)?;
        let run = Arc::new(Mutex::new(Some(first)));
        let supervisor = {
            let (run, stop, program, url) = (
                run.clone(),
                stop.clone(),
                program.to_path_buf(),
                url.clone(),
            );
            std::thread::Builder::new()
                .name("cloudflared".into())
                .spawn(move || {
                    supervise(&program, &mode, &args, timeout, url, &run, &stop, &on_url)
                })?
        };
        Ok(Tunnel {
            url,
            run,
            stop,
            supervisor: Some(supervisor),
        })
    }

    /// Whether a cloudflared is running right now (not between restarts).
    #[cfg(test)]
    fn running(&self) -> bool {
        self.run
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|r| matches!(r.child.try_wait(), Ok(None)))
    }

    /// cloudflared's pid, while it runs.
    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        self.run.lock().unwrap().as_ref().map(|r| r.child.id())
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // The supervisor first (it notices within 200 ms), so it cannot
        // start another cloudflared after this one is gone.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.supervisor.take() {
            let _ = t.join();
        }
        drop(self.run.lock().unwrap().take());
    }
}

/// Sleeps up to `d`, waking early when `stop` is set. True if stopped.
fn nap(d: Duration, stop: &AtomicBool) -> bool {
    let until = Instant::now() + d;
    while Instant::now() < until {
        if stop.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
    }
    stop.load(Ordering::SeqCst)
}

/// Watches the running cloudflared; when it exits, starts it again with
/// backoff until stopped.
#[allow(clippy::too_many_arguments)]
fn supervise(
    program: &Path,
    mode: &Mode,
    args: &[String],
    timeout: Duration,
    mut url: Option<String>,
    run: &Mutex<Option<Run>>,
    stop: &AtomicBool,
    on_url: &OnUrl,
) {
    let mut backoff = BACKOFF_FIRST;
    let mut up_since = Instant::now();
    loop {
        if nap(Duration::from_millis(200), stop) {
            return;
        }
        let exited = {
            let mut guard = run.lock().unwrap();
            let Some(r) = guard.as_mut() else { return };
            r.drain();
            match r.child.try_wait() {
                Ok(None) => None,
                Ok(Some(status)) => Some(r.failure(&format!("exited ({status})"))),
                Err(e) => Some(r.failure(&format!("could not be watched ({e})"))),
            }
        };
        let Some(why) = exited else { continue };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if up_since.elapsed() >= BACKOFF_RESET {
            backoff = BACKOFF_FIRST;
        }
        eprintln!("live: {why:#}");
        loop {
            eprintln!("live: restarting cloudflared in {} s", backoff.as_secs());
            if nap(backoff, stop) {
                return;
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
            let started = Run::spawn(program, args)
                .and_then(|mut r| r.ready(mode, timeout, stop).map(|u| (r, u)));
            match started {
                Ok((r, new_url)) => {
                    if stop.load(Ordering::SeqCst) {
                        return; // dropping `r` stops it
                    }
                    *run.lock().unwrap() = Some(r);
                    up_since = Instant::now();
                    if new_url.is_some() && new_url != url {
                        url = new_url;
                        on_url(url.as_deref().unwrap_or_default());
                    } else {
                        eprintln!("live: cloudflared is back");
                    }
                    break;
                }
                Err(_) if stop.load(Ordering::SeqCst) => return,
                Err(e) => eprintln!("live: {e:#}"),
            }
        }
    }
}

/// Starts `program` (`None`: cloudflared from `$H2_CLOUDFLARED`, else
/// PATH) in `mode`, forwarding to the loopback server at `origin`. The public URL goes into
/// `public` (discovery shows it) now and whenever a restart changes it;
/// `who` prefixes the line saying so. With `setup`, a named tunnel with a
/// host name is [`provision`]ed first (`None`: `--no-tunnel-setup`).
pub fn open(
    program: Option<PathBuf>,
    mode: Mode,
    origin: &str,
    public: live_server::public::PublicUrl,
    who: &'static str,
    setup: Option<&OriginCert>,
) -> Result<Tunnel> {
    let program = match program {
        Some(p) => p,
        None => find(std::env::var_os("H2_CLOUDFLARED"), std::env::var_os("PATH"))?,
    };
    if let (
        Some(cert),
        Mode::Named {
            name,
            hostname: Some(host),
        },
    ) = (setup, &mode)
    {
        provision(
            &program,
            name,
            &host.to_ascii_lowercase(),
            cert,
            &mut |line| eprintln!("{who}: {line}"),
        )?;
    }
    eprintln!(
        "{who}: starting cloudflared ({}) for {origin}",
        program.display()
    );
    let later = public.clone();
    let tunnel = Tunnel::start(
        &program,
        mode,
        origin,
        START_TIMEOUT,
        Box::new(move |url| {
            later.set(url, true);
            eprintln!("{who}: the tunnel restarted; public at {url} now");
        }),
    )?;
    if let Some(url) = &tunnel.url {
        public.set(url, true);
    }
    Ok(tunnel)
}

/// Origin for cloudflared: the server's loopback address. `--tunnel` keeps
/// the server on loopback (cloudflared is the only way in from outside) and
/// needs a fixed port to point at.
pub fn origin(listen: std::net::SocketAddr) -> Result<String> {
    if !listen.ip().is_loopback() {
        bail!(
            "a tunnel serves through cloudflared on this machine; --listen {listen} must be \
             loopback (127.0.0.1:PORT)"
        );
    }
    if listen.port() == 0 {
        bail!("a tunnel needs a fixed --listen port, not 0");
    }
    Ok(format!("http://{listen}"))
}

/// What `serve`/`play` print after the tunnel is up, and the warnings a
/// public server deserves.
pub fn notices(url: Option<&str>, mode: &Mode, invite_only: bool, private: bool) -> Vec<String> {
    let mut out = Vec::new();
    match url {
        Some(u) => out.push(format!("public at {u} (through cloudflared)")),
        None => out.push(
            "tunnel up, but its URL is unknown, so it is not printed or published; pass \
             --hostname HOST (h2 play: --live-tunnel-url https://HOST) with the tunnel name"
                .into(),
        ),
    }
    if *mode == Mode::Quick {
        out.push(
            "a quick tunnel's URL is new every run: accounts keep working (they match the \
             server id, not the URL), but friends need the new URL each time; use a named \
             tunnel (--tunnel-name) for a lasting address"
                .into(),
        );
    }
    out.extend(live_server::public::exposure_notices(invite_only, private));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // Real cloudflared 2024-2025 output, plain and JSON log formats.
    const QUICK_LOG: &str = r#"2025-03-04T10:15:01Z INF Thank you for trying Cloudflare Tunnel. Doing so, without a Cloudflare account, is a quick way to experiment and try it out. However, be aware that these account-less Tunnels have no uptime guarantee, are subject to the Cloudflare Online Services Terms of Use (https://www.cloudflare.com/website-terms/), and Cloudflare reserves the right to investigate your use of Tunnels for violations of such terms. If you intend to use Tunnels in production you should use a pre-created named tunnel by following: https://developers.cloudflare.com/cloudflare-one/connections/connect-apps
2025-03-04T10:15:01Z INF Requesting new quick Tunnel on trycloudflare.com...
2025-03-04T10:15:03Z INF +--------------------------------------------------------------------------------------------+
2025-03-04T10:15:03Z INF |  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |
2025-03-04T10:15:03Z INF |  https://seasonal-deck-organisms-sf.trycloudflare.com                                      |
2025-03-04T10:15:03Z INF +--------------------------------------------------------------------------------------------+
2025-03-04T10:15:03Z INF Cannot determine default configuration path. No file [config.yml config.yaml] in [~/.cloudflared ~/.cloudflare-warp ~/cloudflare-warp /etc/cloudflared /usr/local/etc/cloudflared]
2025-03-04T10:15:03Z INF Version 2025.2.1 (Checksum 1a2b3c)
2025-03-04T10:15:04Z INF Registered tunnel connection connIndex=0 connection=8d3c1f2e-1111-2222-3333-444455556666 event=0 ip=198.41.200.13 location=lhr01 protocol=quic"#;

    #[test]
    fn finds_the_quick_tunnel_url_in_real_logs() {
        let urls: Vec<_> = QUICK_LOG.lines().filter_map(quick_url).collect();
        assert_eq!(
            urls,
            ["https://seasonal-deck-organisms-sf.trycloudflare.com"]
        );

        for (line, want) in [
            // JSON log format (--logformat json).
            (
                r#"{"level":"info","time":"2025-03-04T10:15:03Z","message":"|  https://quiet-river-7f3a.trycloudflare.com                                      |"}"#,
                Some("https://quiet-river-7f3a.trycloudflare.com"),
            ),
            // Older releases, no box.
            (
                "2022-09-01T12:00:00Z INF |  Your quick Tunnel has been created! Visit it at: https://Mixed-Case-Name.trycloudflare.com",
                Some("https://mixed-case-name.trycloudflare.com"),
            ),
            // A bare line, as some wrappers print it.
            (
                "https://abc123.trycloudflare.com",
                Some("https://abc123.trycloudflare.com"),
            ),
            // Not the tunnel: the API it asks, errors, other hosts.
            (
                r#"2025-03-04T10:15:02Z ERR Error requesting quick tunnel error="Post \"https://api.trycloudflare.com/tunnel\": dial tcp: lookup api.trycloudflare.com: no such host""#,
                None,
            ),
            (
                "2025-03-04T10:15:01Z INF Requesting new quick Tunnel on trycloudflare.com...",
                None,
            ),
            ("see https://www.cloudflare.com/website-terms/", None),
            ("https://a.b.trycloudflare.com", None),
            ("https://evil.trycloudflare.com.attacker.example", None),
            ("https://x.trycloudflare.com:8443", None),
            ("http://plain.trycloudflare.com", None),
        ] {
            assert_eq!(quick_url(line).as_deref(), want, "{line}");
        }
        assert!(QUICK_LOG.lines().any(registered));
        assert!(registered(
            r#"{"level":"info","connIndex":1,"event":0,"ip":"198.41.192.7","location":"ams08","protocol":"http2","message":"Registered tunnel connection"}"#
        ));
        assert!(!registered("INF Starting tunnel tunnelID=abc"));
    }

    #[test]
    fn flags_pick_a_mode() {
        let named = |n: &str, h: Option<&str>| Mode::Named {
            name: n.into(),
            hostname: h.map(Into::into),
        };
        assert_eq!(Mode::from_flags(false, None, None).unwrap(), None);
        assert_eq!(
            Mode::from_flags(true, None, None).unwrap(),
            Some(Mode::Quick)
        );
        assert_eq!(
            Mode::from_flags(false, Some("h2".into()), Some("Live.Example.net".into())).unwrap(),
            Some(named("h2", Some("Live.Example.net")))
        );
        assert_eq!(
            named("h2", Some("Live.Example.net")).fixed_url().as_deref(),
            Some("https://live.example.net")
        );
        assert_eq!(named("h2", None).fixed_url(), None);
        assert_eq!(Mode::Quick.fixed_url(), None);
        assert!(Mode::from_flags(true, Some("h2".into()), None).is_err());
        assert!(Mode::from_flags(true, None, Some("a.example".into())).is_err());
        assert!(Mode::from_flags(false, None, Some("a.example".into())).is_err());
        for bad in [
            "https://a.example",
            "a.example:443",
            "a.example/x",
            "localhost",
            "-a.example",
            "",
        ] {
            assert!(
                Mode::from_flags(false, Some("h2".into()), Some(bad.into())).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn cloudflared_arguments() {
        let o = "http://127.0.0.1:8074";
        assert_eq!(
            args(&Mode::Quick, o),
            [
                "tunnel",
                "--no-autoupdate",
                "--grace-period",
                "1s",
                "--url",
                o
            ]
        );
        assert_eq!(
            args(
                &Mode::Named {
                    name: "h2live".into(),
                    hostname: Some("live.example.net".into())
                },
                o
            ),
            [
                "tunnel",
                "--no-autoupdate",
                "--grace-period",
                "1s",
                "run",
                "--url",
                o,
                "h2live"
            ]
        );
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "h2-tunnel-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A test-only `cloudflared`: a shell script in its own directory.
    fn fake(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("cloudflared");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn alive(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn quiet() -> OnUrl {
        Box::new(|_| {})
    }

    #[test]
    fn found_on_path_or_explained() {
        let dir = temp("find");
        let none = temp("find-empty");
        let bin = fake(&dir, "exit 0");
        let path = std::env::join_paths([&none, &dir]).unwrap();
        assert_eq!(find(None, Some(path)).unwrap(), bin);
        let missing = find(None, Some(none.clone().into_os_string())).unwrap_err();
        assert!(
            format!("{missing}").contains("brew install cloudflared"),
            "{missing}"
        );
        assert_eq!(find(Some(bin.clone().into()), None).unwrap(), bin);
        assert!(find(Some(none.join("nope").into()), None).is_err());
        // Not executable: skipped.
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(find(None, Some(dir.clone().into_os_string())).is_err());
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(none);
    }

    #[test]
    fn a_quick_tunnel_reports_its_url_and_stops_with_its_owner() {
        let dir = temp("quick");
        let argv = dir.join("argv");
        let bin = fake(
            &dir,
            &format!(
                "echo \"$@\" > {argv}\n\
                 echo '2025-03-04T10:15:01Z INF Requesting new quick Tunnel on trycloudflare.com...' >&2\n\
                 echo '2025-03-04T10:15:03Z INF |  https://brave-test-tunnel.trycloudflare.com   |' >&2\n\
                 exec sleep 30",
                argv = argv.display()
            ),
        );
        let t = Tunnel::start(
            &bin,
            Mode::Quick,
            "http://127.0.0.1:8074",
            Duration::from_secs(10),
            quiet(),
        )
        .unwrap();
        assert_eq!(
            t.url.as_deref(),
            Some("https://brave-test-tunnel.trycloudflare.com")
        );
        let pid = t.pid().unwrap();
        assert!(t.running() && alive(pid));
        assert_eq!(
            std::fs::read_to_string(&argv).unwrap().trim(),
            "tunnel --no-autoupdate --grace-period 1s --url http://127.0.0.1:8074"
        );
        drop(t);
        assert!(!alive(pid), "cloudflared stops with the server");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_named_tunnel_is_up_once_a_connection_registers() {
        let dir = temp("named");
        let bin = fake(
            &dir,
            "echo 'INF Starting tunnel tunnelID=6ff42ae2' >&2\n\
             sleep 0.2\n\
             echo 'INF Registered tunnel connection connIndex=0 location=lhr01 protocol=quic' >&2\n\
             exec sleep 30",
        );
        let mode = Mode::Named {
            name: "h2live".into(),
            hostname: Some("live.example.net".into()),
        };
        let t = Tunnel::start(
            &bin,
            mode,
            "http://127.0.0.1:1",
            Duration::from_secs(10),
            quiet(),
        )
        .unwrap();
        assert_eq!(t.url.as_deref(), Some("https://live.example.net"));
        drop(t);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_start_shows_what_cloudflared_said() {
        let dir = temp("fail");
        let bin = fake(
            &dir,
            "echo 'ERR Couldn'\"'\"'t start tunnel error=\"tunnel credentials file not found\"' >&2\nexit 1",
        );
        let err = Tunnel::start(
            &bin,
            Mode::Quick,
            "http://127.0.0.1:1",
            Duration::from_secs(10),
            quiet(),
        )
        .err()
        .unwrap();
        let msg = format!("{err:#}");
        assert!(msg.contains("exited"), "{msg}");
        assert!(msg.contains("tunnel credentials file not found"), "{msg}");

        // Silent and never up: a timeout, and the process is killed.
        let pidfile = dir.join("pid");
        let slow = fake(
            &dir,
            &format!("echo $$ > {}\nexec sleep 30", pidfile.display()),
        );
        let err = Tunnel::start(
            &slow,
            Mode::Quick,
            "http://127.0.0.1:1",
            Duration::from_millis(500),
            quiet(),
        )
        .err()
        .unwrap();
        assert!(
            format!("{err:#}").contains("did not bring the tunnel up"),
            "{err:#}"
        );
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!alive(pid));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dead_cloudflared_is_restarted_with_its_new_url() {
        let dir = temp("restart");
        let count = dir.join("count");
        // Each run prints a URL numbered by run; the first dies at once.
        let bin = fake(
            &dir,
            &format!(
                "n=$(cat {c} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {c}\n\
                 echo \"INF |  https://run-$n.trycloudflare.com  |\" >&2\n\
                 if [ $n -eq 1 ]; then exit 3; fi\n\
                 exec sleep 30",
                c = count.display()
            ),
        );
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let t = Tunnel::start(
            &bin,
            Mode::Quick,
            "http://127.0.0.1:1",
            Duration::from_secs(10),
            Box::new(move |u| {
                let _ = tx.lock().unwrap().send(u.to_string());
            }),
        )
        .unwrap();
        assert_eq!(t.url.as_deref(), Some("https://run-1.trycloudflare.com"));
        let next = rx.recv_timeout(Duration::from_secs(15)).unwrap();
        assert_eq!(next, "https://run-2.trycloudflare.com");
        let pid = t.pid().unwrap();
        assert!(alive(pid));
        drop(t);
        assert!(!alive(pid));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_origin_is_loopback_with_a_port() {
        assert_eq!(
            origin("127.0.0.1:8074".parse().unwrap()).unwrap(),
            "http://127.0.0.1:8074"
        );
        assert_eq!(
            origin("[::1]:8074".parse().unwrap()).unwrap(),
            "http://[::1]:8074"
        );
        assert!(origin("0.0.0.0:8074".parse().unwrap()).is_err());
        assert!(origin("192.168.1.2:8074".parse().unwrap()).is_err());
        assert!(origin("127.0.0.1:0".parse().unwrap()).is_err());
    }

    #[test]
    fn public_server_notices() {
        let quick = notices(
            Some("https://a.trycloudflare.com"),
            &Mode::Quick,
            false,
            false,
        );
        assert!(quick[0].contains("https://a.trycloudflare.com"));
        assert!(quick.iter().any(|l| l.contains("named tunnel")));
        assert!(quick.iter().any(|l| l.contains("--invite-only")));
        let named = Mode::Named {
            name: "n".into(),
            hostname: None,
        };
        let locked = notices(None, &named, true, true);
        assert_eq!(locked.len(), 1, "{locked:?}");
        assert!(locked[0].contains("--hostname"));
    }

    #[test]
    fn a_tunnel_url_is_https_and_a_host() {
        assert_eq!(
            hostname_from_url("https://play.forbes.red").unwrap(),
            "play.forbes.red"
        );
        assert_eq!(
            hostname_from_url("HTTPS://Play.Example.net/").unwrap(),
            "play.example.net"
        );
        for bad in [
            "http://play.example.net",
            "play.example.net",
            "https://play.example.net:443",
            "https://play.example.net/x",
            "https://play.example.net//",
            "https://play.example.net?x=1",
            "https://u@play.example.net",
            "https://localhost",
            "https://",
            "",
        ] {
            let e = hostname_from_url(bad).unwrap_err();
            assert!(format!("{e}").contains("https://HOST"), "{bad}: {e}");
        }
    }

    #[test]
    fn the_origin_cert_follows_cloudflareds_rules() {
        let home = temp("cert-home");
        let c = OriginCert::new(None, Some(home.clone().into()));
        assert_eq!(c.explicit, None);
        assert_eq!(c.dirs[0], home.join(".cloudflared"));
        assert!(c.dirs.contains(&PathBuf::from("/etc/cloudflared")));
        let hermetic = OriginCert {
            explicit: None,
            dirs: c.dirs[..3].to_vec(),
        };
        assert_eq!(hermetic.find(), None);
        std::fs::create_dir_all(home.join(".cloudflare-warp")).unwrap();
        std::fs::write(home.join(".cloudflare-warp/cert.pem"), "x").unwrap();
        assert_eq!(
            hermetic.find(),
            Some(home.join(".cloudflare-warp/cert.pem"))
        );
        // $TUNNEL_ORIGIN_CERT names the file outright: the others do not count.
        let elsewhere = home.join("mine.pem");
        let explicit = OriginCert::new(Some(elsewhere.clone().into()), Some(home.clone().into()));
        assert_eq!(explicit.explicit.as_deref(), Some(elsewhere.as_path()));
        assert_eq!(explicit.find(), None);
        std::fs::write(&elsewhere, "x").unwrap();
        assert_eq!(explicit.find(), Some(elsewhere));
        // Empty values are unset.
        assert_eq!(OriginCert::new(Some("".into()), None).explicit, None);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn tunnel_list_json() {
        let json = r#"[
            {"id":"6ff42ae2-765d-4adf-8112-31c55c1551ef","name":"h2live","created_at":"2025-03-01T10:00:00Z","deleted_at":"0001-01-01T00:00:00Z","connections":[]},
            {"id":"11111111-2222-3333-4444-555555555555","name":"h2play","created_at":"2025-03-01T10:00:00Z","deleted_at":"2025-03-02T10:00:00Z","connections":[]},
            {"id":"aaaaaaaa-2222-3333-4444-555555555555","name":"h2play","created_at":"2025-03-03T10:00:00Z","connections":[]}
        ]"#;
        assert_eq!(
            listed_id(json, "h2live").unwrap().as_deref(),
            Some("6ff42ae2-765d-4adf-8112-31c55c1551ef")
        );
        // The deleted one is skipped.
        assert_eq!(
            listed_id(json, "h2play").unwrap().as_deref(),
            Some("aaaaaaaa-2222-3333-4444-555555555555")
        );
        assert_eq!(
            listed_id(json, "6FF42AE2-765D-4ADF-8112-31C55C1551EF")
                .unwrap()
                .as_deref(),
            Some("6ff42ae2-765d-4adf-8112-31c55c1551ef")
        );
        assert_eq!(listed_id(json, "other").unwrap(), None);
        assert_eq!(listed_id("[]", "h2play").unwrap(), None);
        assert_eq!(listed_id("null\n", "h2play").unwrap(), None);
        assert!(listed_id("You need to login first", "h2play").is_err());
    }

    // Real `cloudflared tunnel route dns` output (2024-2025 releases).
    const ROUTE_ADDED: &str = "2025-03-04T10:15:01Z INF Added CNAME play.example.net which will route to this tunnel tunnelID=6ff42ae2-765d-4adf-8112-31c55c1551ef";
    const ROUTE_ALREADY: &str = "2025-03-04T10:15:01Z INF play.example.net is already configured to route to your tunnel tunnelID=6ff42ae2-765d-4adf-8112-31c55c1551ef";
    const ROUTE_CONFLICT: &str = "Failed to add route: code: 1003, reason: Failed to create record play.example.net with err An A, AAAA, or CNAME record with that host already exists. For more details, refer to <https://developers.cloudflare.com/dns/manage-dns-records/troubleshooting/records-with-same-name/>.";

    #[test]
    fn route_dns_outcomes() {
        let r = |ok, out| route_outcome(ok, out, "h2play", "play.example.net");
        assert_eq!(r(true, ROUTE_ADDED).unwrap(), Routed::Added);
        assert_eq!(r(true, ROUTE_ALREADY).unwrap(), Routed::Already);
        assert_eq!(r(false, ROUTE_ALREADY).unwrap(), Routed::Already);
        let e = format!("{:#}", r(false, ROUTE_CONFLICT).unwrap_err());
        assert!(
            e.contains("Delete the old play.example.net record in the Cloudflare dashboard"),
            "{e}"
        );
        assert!(
            e.contains("code: 1003"),
            "cloudflared's words are kept: {e}"
        );
        let e = format!(
            "{:#}",
            r(false, "ERR Unauthorized: invalid zone").unwrap_err()
        );
        assert!(
            e.contains("route dns h2play play.example.net` failed") && e.contains("invalid zone"),
            "{e}"
        );
    }

    /// A fake `cloudflared` for [`provision`]: logs each call to `calls`;
    /// `login` writes `cert` (if given); `tunnel list` prints `list`;
    /// `route dns` prints `route` and exits `route_exit`.
    fn fake_setup(
        dir: &Path,
        cert: Option<&Path>,
        list: &str,
        route: &str,
        route_exit: i32,
    ) -> PathBuf {
        let login = cert.map_or_else(
            || "exit 0".to_string(),
            |c| {
                format!(
                    "mkdir -p {d}; echo cert > {c}",
                    d = c.parent().unwrap().display(),
                    c = c.display()
                )
            },
        );
        fake(
            dir,
            &format!(
                "echo \"$@\" >> {calls}\n\
                 case \"$1 $2\" in\n\
                 'tunnel login') {login} ;;\n\
                 'tunnel list') echo '{list}' ;;\n\
                 'tunnel create') echo \"Created tunnel $3 with id 6ff42ae2-765d-4adf-8112-31c55c1551ef\" >&2 ;;\n\
                 'tunnel route') echo '{route}' >&2; exit {route_exit} ;;\n\
                 *) echo \"unexpected: $*\" >&2; exit 9 ;;\n\
                 esac",
                calls = dir.join("calls").display(),
            ),
        )
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn hermetic_cert(home: &Path) -> OriginCert {
        OriginCert {
            explicit: None,
            dirs: vec![home.join(".cloudflared")],
        }
    }

    const LISTED: &str = r#"[{"id":"6ff42ae2-765d-4adf-8112-31c55c1551ef","name":"h2play","deleted_at":"0001-01-01T00:00:00Z"}]"#;

    #[test]
    fn setup_with_no_login_logs_in_creates_and_routes() {
        let dir = temp("setup-fresh");
        let cert_path = dir.join(".cloudflared/cert.pem");
        let bin = fake_setup(&dir, Some(&cert_path), "[]", ROUTE_ADDED, 0);
        let mut said = Vec::new();
        let routed = provision(
            &bin,
            "h2play",
            "play.example.net",
            &hermetic_cert(&dir),
            &mut |l| said.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(routed, Routed::Added);
        assert_eq!(
            calls(&dir),
            [
                "tunnel login",
                "tunnel list --output json",
                "tunnel create h2play",
                "tunnel route dns h2play play.example.net"
            ]
        );
        assert!(cert_path.is_file());
        assert!(said[0].contains("cloudflared tunnel login"), "{said:?}");
        assert!(
            said.contains(&"creating tunnel h2play".to_string()),
            "{said:?}"
        );
        assert!(
            said.contains(&"routing play.example.net to tunnel h2play".to_string()),
            "{said:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_skips_what_is_there() {
        let dir = temp("setup-there");
        std::fs::create_dir_all(dir.join(".cloudflared")).unwrap();
        std::fs::write(dir.join(".cloudflared/cert.pem"), "x").unwrap();
        let bin = fake_setup(&dir, None, LISTED, ROUTE_ALREADY, 0);
        let mut said = Vec::new();
        let routed = provision(
            &bin,
            "h2play",
            "play.example.net",
            &hermetic_cert(&dir),
            &mut |l| said.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(routed, Routed::Already);
        // No login, no create.
        assert_eq!(
            calls(&dir),
            [
                "tunnel list --output json",
                "tunnel route dns h2play play.example.net"
            ]
        );
        assert!(
            said.iter()
                .any(|l| l.contains("tunnel h2play exists (6ff42ae2")),
            "{said:?}"
        );
        assert!(
            said.iter().any(|l| l.contains("already routes")),
            "{said:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_with_a_missing_tunnel_creates_it() {
        let dir = temp("setup-create");
        std::fs::create_dir_all(dir.join(".cloudflared")).unwrap();
        std::fs::write(dir.join(".cloudflared/cert.pem"), "x").unwrap();
        let other = r#"[{"id":"6ff42ae2-765d-4adf-8112-31c55c1551ef","name":"h2live","deleted_at":"0001-01-01T00:00:00Z"}]"#;
        let bin = fake_setup(&dir, None, other, ROUTE_ADDED, 0);
        provision(
            &bin,
            "h2play",
            "play.example.net",
            &hermetic_cert(&dir),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            calls(&dir),
            [
                "tunnel list --output json",
                "tunnel create h2play",
                "tunnel route dns h2play play.example.net"
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_reports_a_dns_conflict_plainly() {
        let dir = temp("setup-conflict");
        std::fs::create_dir_all(dir.join(".cloudflared")).unwrap();
        std::fs::write(dir.join(".cloudflared/cert.pem"), "x").unwrap();
        let bin = fake_setup(&dir, None, LISTED, ROUTE_CONFLICT, 1);
        let e = provision(
            &bin,
            "h2play",
            "play.example.net",
            &hermetic_cert(&dir),
            &mut |_| {},
        )
        .unwrap_err();
        let e = format!("{e:#}");
        assert!(e.contains("already has a DNS record"), "{e}");
        assert!(e.contains("in the Cloudflare dashboard"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn setup_stops_when_login_does_not_help() {
        // A login that writes nothing: the error says where the cert was looked for.
        let dir = temp("setup-nologin");
        let bin = fake_setup(&dir, None, "[]", ROUTE_ADDED, 0);
        let e = provision(
            &bin,
            "h2play",
            "play.example.net",
            &hermetic_cert(&dir),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(format!("{e}").contains("still no cert.pem"), "{e}");
        assert_eq!(calls(&dir), ["tunnel login"]);

        // $TUNNEL_ORIGIN_CERT naming a missing file: no login (it would write elsewhere).
        let _ = std::fs::remove_file(dir.join("calls"));
        let cert = OriginCert {
            explicit: Some(dir.join("nope.pem")),
            dirs: vec![dir.join(".cloudflared")],
        };
        let e = provision(&bin, "h2play", "play.example.net", &cert, &mut |_| {}).unwrap_err();
        assert!(format!("{e}").contains("TUNNEL_ORIGIN_CERT"), "{e}");
        assert!(calls(&dir).is_empty());

        // A failing list is shown with what cloudflared said.
        std::fs::write(dir.join("nope.pem"), "x").unwrap();
        let broken = fake(&dir, "echo 'ERR Error locating origin cert' >&2; exit 1");
        let e = provision(&broken, "h2play", "play.example.net", &cert, &mut |_| {}).unwrap_err();
        assert!(format!("{e}").contains("Error locating origin cert"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn open_sets_up_a_named_tunnel_unless_told_not_to() {
        let dir = temp("open-setup");
        std::fs::create_dir_all(dir.join(".cloudflared")).unwrap();
        std::fs::write(dir.join(".cloudflared/cert.pem"), "x").unwrap();
        let bin = fake(
            &dir,
            &format!(
                "echo \"$@\" >> {calls}\n\
                 case \"$1 $2\" in\n\
                 'tunnel list') echo '[]' ;;\n\
                 'tunnel create') ;;\n\
                 'tunnel route') echo '{ROUTE_ADDED}' >&2 ;;\n\
                 *) echo 'INF Registered tunnel connection connIndex=0' >&2; exec sleep 30 ;;\n\
                 esac",
                calls = dir.join("calls").display()
            ),
        );
        let mode = Mode::Named {
            name: "h2play".into(),
            hostname: Some("Play.Example.net".into()),
        };
        let cert = hermetic_cert(&dir);
        let public = live_server::public::PublicUrl::default();
        let t = open(
            Some(bin.clone()),
            mode.clone(),
            "http://127.0.0.1:1",
            public.clone(),
            "test",
            Some(&cert),
        )
        .unwrap();
        assert_eq!(t.url.as_deref(), Some("https://play.example.net"));
        drop(t);
        assert_eq!(
            calls(&dir),
            [
                "tunnel list --output json",
                "tunnel create h2play",
                "tunnel route dns h2play play.example.net",
                "tunnel --no-autoupdate --grace-period 1s run --url http://127.0.0.1:1 h2play"
            ]
        );

        // --no-tunnel-setup: straight to `run`.
        std::fs::remove_file(dir.join("calls")).unwrap();
        let t = open(
            Some(bin.clone()),
            mode,
            "http://127.0.0.1:1",
            public.clone(),
            "test",
            None,
        )
        .unwrap();
        drop(t);
        assert_eq!(
            calls(&dir),
            ["tunnel --no-autoupdate --grace-period 1s run --url http://127.0.0.1:1 h2play"]
        );

        // Without a host name there is nothing to route: no setup either.
        std::fs::remove_file(dir.join("calls")).unwrap();
        let nameless = Mode::Named {
            name: "h2play".into(),
            hostname: None,
        };
        let t = open(
            Some(bin),
            nameless,
            "http://127.0.0.1:1",
            public,
            "test",
            Some(&cert),
        )
        .unwrap();
        assert_eq!(t.url, None);
        drop(t);
        assert_eq!(calls(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
