use std::fs::{self, DirBuilder};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_proto::Client;
use anyhow::{Context, Result, ensure};
use live_proto::hosts::{HostLease, Transport};

use super::live::Live;
use super::supervisor::Options;

static STOPPING: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: libc::c_int) {
    STOPPING.store(true, Ordering::Relaxed);
}

pub(super) fn watch_signals() -> Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only stores to a lock-free atomic.
        let old = unsafe { libc::signal(signal, request_stop as *const () as libc::sighandler_t) };
        if old == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error()).context("installing host signal handler");
        }
    }
    Ok(())
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
        let scratch = Scratch::new()?;
        let socket = scratch.0.join("a.sock");
        let scratch_data = scratch.0.join("data");
        let data = opts.data.as_deref().unwrap_or(&scratch_data);

        let mut cmd = Command::new(std::env::current_exe()?);
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
            .arg(&socket)
            .stdin(Stdio::null())
            .env("H2_HOST_LOBBY", "1")
            .env("H2_XNET_ADDR", address.to_string())
            .env("H2_LEASE", &lease.id)
            .env("H2_HOST_LIVE_URL", live.base())
            .env("H2_HOST_TOKEN", &opts.token)
            .env_remove("H2_TOKEN")
            .env_remove("H2_DATA_URL")
            .env_remove("H2_DATA_SHA")
            .env_remove("H2_BOOT_GFX")
            .env_remove("H2_LEVEL")
            .env_remove("H2_SIM")
            .env_remove("H2_BOOT_INPUT")
            .env_remove("H2_INPUT_REPLAY")
            .env_remove("H2_BOTS");
        if !lease.bots.is_empty() {
            cmd.arg("--host-bots");
        }
        if lease.transport == Transport::Tunnel {
            cmd.env("H2_TUNNEL_LISTEN", opts.tunnel_listen.to_string());
        } else {
            cmd.env_remove("H2_TUNNEL_LISTEN");
        }
        #[cfg(target_os = "linux")]
        stop_on_parent_death(&mut cmd);

        let child = cmd.spawn().context("starting light game host")?;
        Ok(Self {
            child,
            _scratch: scratch,
            socket,
        })
    }

    pub fn exited(&mut self) -> Result<Option<ExitStatus>> {
        self.child.try_wait().context("checking light game host")
    }

    pub fn connect(&self, timeout: Duration) -> std::io::Result<Client> {
        let agent = Client::connect(&self.socket)?;
        agent.set_timeout(Some(timeout))?;
        Ok(agent)
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        // SAFETY: the child hasn't been reaped, so its pid cannot be reused.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        Ok(value) => *value
            .parse::<SocketAddrV4>()
            .context("H2_XNET_ADDR must be IPv4:port")?
            .ip(),
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
