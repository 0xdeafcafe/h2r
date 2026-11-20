use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_proto::{Client, Endpoint};
use anyhow::{Context, Result, ensure};
use boot::agent_glue::HOST_TCP_ENV;
use live_proto::hosts::{HostLease, Transport};

use super::live::Live;
use super::sim_run;
use super::supervisor::Options;

static STOPPING: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: libc::c_int) {
    STOPPING.store(true, Ordering::Relaxed);
}

pub(super) fn watch_stop_requests() -> Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only stores to a lock-free atomic.
        let old = unsafe { libc::signal(signal, request_stop as *const () as libc::sighandler_t) };
        if old == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error()).context("installing host signal handler");
        }
    }
    if std::env::var_os("H2_HOST_PARENT_PIPE").as_deref() == Some(OsStr::new("1")) {
        thread::Builder::new()
            .name("host-parent-pipe".into())
            .spawn(|| watch_parent_pipe(io::stdin().lock(), &STOPPING))
            .context("starting host parent pipe watcher")?;
    }
    Ok(())
}

fn watch_parent_pipe(mut pipe: impl Read, stopping: &AtomicBool) {
    // The pipe carries no commands. Its write end keeps the lease alive.
    let result = io::copy(&mut pipe, &mut io::sink());
    stopping.store(true, Ordering::Relaxed);
    if let Err(error) = result {
        eprintln!("h2 host: parent pipe read failed: {error}");
    }
}

pub(super) fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

pub(super) struct Game {
    child: Child,
    // Kept alive until after the child is reaped.
    _scratch: Scratch,
    socket: PathBuf,
}

impl Game {
    pub fn spawn(
        opts: &Options,
        live: &Live<'_>,
        lease: &HostLease,
        disc: &Path,
        address: SocketAddrV4,
    ) -> Result<Self> {
        Self::spawn_with(opts, live, lease, |cmd, data, socket| {
            cmd.arg("serve")
                .arg(disc)
                .args([
                    "--system-link",
                    "--host-light",
                    "--no-halo1",
                    "--no-mappack",
                ])
                .arg("--out")
                .arg(&opts.out)
                .arg("--data")
                .arg(data)
                .arg("--socket")
                .arg(socket)
                .arg("--film")
                .arg(film_path(opts.film.as_deref(), socket))
                .env("H2_HOST_LEASE_JSON", serde_json::to_string(lease)?)
                .env("H2_HOST_LOBBY", "1")
                .env("H2_XNET_ADDR", address.to_string());
            if !lease.bots.is_empty() {
                cmd.arg("--host-bots");
            }
            if lease.transport == Transport::Tunnel {
                cmd.env("H2_TUNNEL_LISTEN", opts.tunnel_listen.to_string());
            } else {
                cmd.env_remove("H2_TUNNEL_LISTEN");
            }
            Ok(())
        })
    }

    pub fn spawn_sim(
        opts: &Options,
        live: &Live<'_>,
        lease: &HostLease,
        disc: &Path,
    ) -> Result<Self> {
        Self::spawn_with(opts, live, lease, |cmd, data, socket| {
            sim_run::configure(cmd, lease, disc, &opts.out, data, socket)?;
            if let Some(path) = &opts.film {
                cmd.env("H2_FILM_OUT", path);
                if std::env::var_os("H2_FILM_GAME_ID").is_none() {
                    cmd.env("H2_FILM_GAME_ID", &lease.assignment_id);
                }
            }
            Ok(())
        })
    }

    fn spawn_with(
        opts: &Options,
        live: &Live<'_>,
        lease: &HostLease,
        configure: impl FnOnce(&mut Command, &Path, &Path) -> Result<()>,
    ) -> Result<Self> {
        let scratch = Scratch::new()?;
        let socket = scratch.0.join("a.sock");
        let scratch_data = scratch.0.join("data");
        let data = opts.data.as_deref().unwrap_or(&scratch_data);
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.stdin(Stdio::null())
            .env_remove(agent_proto::ENV)
            .env_remove("H2_HOST_LEASE_JSON")
            .env_remove("H2_HOST_PARENT_PIPE")
            .env("H2_LEASE", &lease.id)
            .env("H2_HOST_LIVE_URL", live.base())
            .env("H2_HOST_TOKEN", &opts.token)
            .env_remove("H2_TOKEN")
            .env_remove("H2_DATA_URL")
            .env_remove("H2_DATA_SHA")
            .env_remove("H2_BOOT_GFX")
            .env_remove("H2_LEVEL")
            .env_remove("H2_FILM_OUT")
            .env_remove("H2_SIM")
            .env_remove("H2_BOOT_INPUT")
            .env_remove("H2_INPUT_REPLAY")
            .env_remove("H2_BOTS");
        configure_agent_tcp(&mut cmd, std::env::var_os(agent_proto::ENV).as_deref())?;
        configure(&mut cmd, data, &socket)?;
        #[cfg(target_os = "linux")]
        stop_on_parent_death(&mut cmd);

        let child = cmd.spawn().context("starting dedicated game child")?;
        Ok(Self {
            child,
            _scratch: scratch,
            socket,
        })
    }

    pub fn exited(&mut self) -> Result<Option<ExitStatus>> {
        self.child.try_wait().context("checking dedicated game child")
    }

    pub fn connect(&self, timeout: Duration) -> std::io::Result<Client> {
        let agent = Client::connect(&self.socket)?;
        agent.set_timeout(Some(timeout))?;
        Ok(agent)
    }
}

fn film_path(explicit: Option<&Path>, socket: &Path) -> PathBuf {
    explicit.map_or_else(|| socket.with_file_name("match.h2film"), Path::to_owned)
}

fn configure_agent_tcp(cmd: &mut Command, inherited: Option<&OsStr>) -> Result<()> {
    cmd.env_remove(HOST_TCP_ENV);
    if let Some(endpoint) = inherited
        .and_then(OsStr::to_str)
        .filter(|value| value.starts_with("tcp:"))
        && let Endpoint::Tcp(address) = Endpoint::parse(endpoint).map_err(anyhow::Error::msg)?
    {
        cmd.env(HOST_TCP_ENV, address);
    }
    Ok(())
}

impl Game {
    pub fn shutdown(&mut self, grace: Duration) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        // SAFETY: the child hasn't been reaped, so its pid cannot be reused.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        eprintln!(
            "h2 host: game did not exit within {}s, stopping it",
            grace.as_secs()
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(2));
    }
}

#[cfg(target_os = "linux")]
fn stop_on_parent_death(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: the child uses only syscalls and constructs errors without allocation.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // The supervisor could have exited before prctl was installed.
            if libc::getppid() != parent {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            Ok(())
        });
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("h2h-{}", live_proto::sign::random_id()));
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .with_context(|| format!("creating private host directory {}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn bind_address(base_port: u16) -> Result<SocketAddrV4> {
    let ip = match std::env::var("H2_XNET_ADDR") {
        Ok(value) => value
            .to_socket_addrs()
            .context("H2_XNET_ADDR must be IPv4:port or a hostname:port resolving to IPv4")?
            .find_map(|addr| match addr {
                SocketAddr::V4(addr) => Some(*addr.ip()),
                SocketAddr::V6(_) => None,
            })
            .context("H2_XNET_ADDR has no IPv4 address")?,
        Err(std::env::VarError::NotPresent) => route_ip().unwrap_or(Ipv4Addr::LOCALHOST),
        Err(error) => return Err(error).context("reading H2_XNET_ADDR"),
    };
    checked_address(SocketAddrV4::new(ip, base_port), "XNet bind address")
}

pub(super) fn public_address(base_port: u16) -> Result<Option<SocketAddrV4>> {
    let value = match std::env::var("H2_PUBLIC_ADDR") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(error).context("reading H2_PUBLIC_ADDR"),
    };
    let addr = value
        .parse::<SocketAddrV4>()
        .or_else(|_| {
            value
                .parse::<Ipv4Addr>()
                .map(|ip| SocketAddrV4::new(ip, base_port))
        })
        .context("H2_PUBLIC_ADDR must be IPv4 or IPv4:port")?;
    checked_address(addr, "H2_PUBLIC_ADDR").map(Some)
}

fn checked_address(addr: SocketAddrV4, name: &str) -> Result<SocketAddrV4> {
    let ip = addr.ip();
    ensure!(
        !ip.is_unspecified()
            && !ip.is_broadcast()
            && !ip.is_multicast()
            && (1..=65529).contains(&addr.port()),
        "{name} must be unicast IPv4 with base port 1..=65529"
    );
    Ok(addr)
}

/// Select an IPv4 the same way the XNet HLE does, without sending a packet.
fn route_ip() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(addr) => Some(*addr.ip()),
        SocketAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leased_games_record_in_their_private_directory_by_default() {
        let socket = Path::new("private-lease/a.sock");
        assert_eq!(
            film_path(None, socket),
            Path::new("private-lease/match.h2film")
        );
        let explicit = Path::new("retained.h2film");
        assert_eq!(film_path(Some(explicit), socket), explicit);
    }

    #[test]
    fn supervisor_forwards_only_tcp_agent_addresses_to_the_child() {
        let mut cmd = Command::new("h2");
        let address = |cmd: &Command| {
            cmd.get_envs()
                .find(|(key, _)| *key == OsStr::new(HOST_TCP_ENV))
                .map(|(_, value)| value.map(OsStr::to_os_string))
        };
        configure_agent_tcp(&mut cmd, Some(OsStr::new("tcp://127.0.0.1:14720"))).unwrap();
        assert_eq!(address(&cmd), Some(Some("127.0.0.1:14720".into())));

        for value in [None, Some(OsStr::new("")), Some(OsStr::new("menu.sock"))] {
            configure_agent_tcp(&mut cmd, value).unwrap();
            assert_eq!(address(&cmd), Some(None));
        }
        assert!(configure_agent_tcp(&mut cmd, Some(OsStr::new("tcp://localhost"))).is_err());
    }

    #[test]
    fn parent_pipe_drains_input_and_stops_at_eof() {
        struct Pipe<'a> {
            bytes: &'a [u8],
            stopping: &'a AtomicBool,
        }

        impl Read for Pipe<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                assert!(!self.stopping.load(Ordering::Relaxed));
                self.bytes.read(buf)
            }
        }

        let stopping = AtomicBool::new(false);
        let mut pipe = Pipe {
            bytes: b"ignored",
            stopping: &stopping,
        };
        watch_parent_pipe(&mut pipe, &stopping);
        assert!(pipe.bytes.is_empty());
        assert!(stopping.load(Ordering::Relaxed));
    }
}
