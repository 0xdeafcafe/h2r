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
//! - Named tunnel (`--tunnel-name NAME [--hostname HOST]`): one the owner
//!   made with `cloudflared tunnel create` and routed to HOST, so the URL
//!   stays `https://HOST`.
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

use anyhow::{Result, bail};

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
        .map(|d| d.join("cloudflared"))
        .find(|p| is_executable(p))
        .ok_or_else(|| anyhow::anyhow!("cloudflared was not found on PATH; {INSTALL_HINT}"))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
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
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    anyhow::anyhow!("{} is missing; {INSTALL_HINT}", program.display())
                }
                _ => anyhow::anyhow!("starting {}: {e}", program.display()),
            })?;
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
/// `who` prefixes the line saying so.
pub fn open(
    program: Option<PathBuf>,
    mode: Mode,
    origin: &str,
    public: live_server::public::PublicUrl,
    who: &'static str,
) -> Result<Tunnel> {
    let program = match program {
        Some(p) => p,
        None => find(std::env::var_os("H2_CLOUDFLARED"), std::env::var_os("PATH"))?,
    };
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
            "tunnel up; pass --hostname with --tunnel-name to print and publish its URL".into(),
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
    if !invite_only {
        out.push(
            "anyone with the URL can create an account; --invite-only limits that to invite codes"
                .into(),
        );
    }
    if !private {
        out.push("reads are public (games, accounts, the log); --private needs a token".into());
    }
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
}
