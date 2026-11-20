//! One supervisor, writable data directory and network port range per lease.

use super::{HostBoot, HostError, HostLease, HostProvider, ProviderKind};
use live_proto::hosts::Transport;
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

pub struct LocalProvider {
    root: PathBuf,
    bin: Option<PathBuf>,
    children: Mutex<HashMap<String, LocalHost>>,
}

struct LocalHost {
    lease_id: String,
    slot: u16,
    tunnel_addr: Option<SocketAddr>,
    child: Child,
}

/// A local UDP host uses base..=base+6 for XNet. A tunnel listener takes
/// one TCP port. Each host keeps its slot until the supervisor is reaped.
fn port_for(first: u16, width: u16, slot: u16) -> Result<u16, HostError> {
    let base = u32::from(first) + u32::from(slot) * u32::from(width);
    if first == 0 || base + u32::from(width) - 1 > u32::from(u16::MAX) {
        return Err(HostError::new("local host port range exhausted"));
    }
    Ok(base as u16)
}

impl LocalProvider {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        Self {
            root: data_root.as_ref().join("hosts"),
            bin: std::env::var_os("LIVE_HOST_BIN").map(PathBuf::from),
            children: Mutex::default(),
        }
    }

    fn command(
        &self,
        lease: &HostLease,
        boot: &HostBoot,
        slot: u16,
    ) -> Result<(Command, Option<SocketAddr>), HostError> {
        if lease.id.is_empty()
            || !lease
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(HostError::new("invalid lease directory name"));
        }
        let bin = match &self.bin {
            Some(path) => path.clone(),
            None => std::env::current_exe()?,
        };
        let lease_json = serde_json::to_string(lease)
            .map_err(|_| HostError::new("could not encode local host lease"))?;
        let mut cmd = Command::new(bin);
        // Each local supervisor owns a private game socket. A Live process
        // embedded in the desktop game can have its own H2_AGENT endpoint.
        cmd.env_remove("H2_AGENT");
        // Child keeps the write end. If Live goes away the supervisor sees
        // EOF and closes its game even when Live could not run Drop.
        cmd.env("H2_HOST_PARENT_PIPE", "1");
        cmd.env("H2_HOST_LEASE_JSON", lease_json);
        cmd.env("H2_HOST_ARCHIVE", boot.archive.as_str());
        cmd.args([
            "host",
            "run",
            "--lease",
            &lease.id,
            "--live",
            &boot.live_url,
            "--token",
            &boot.token,
            "--data-url",
            &boot.data_url,
            "--data-sha",
            &boot.data_sha256,
        ])
        .stdin(Stdio::piped());
        if let Some(region) = &lease.region {
            cmd.args(["--region", region]);
        }

        let first = std::env::var("H2_BASE_PORT")
            .unwrap_or_else(|_| "31000".into())
            .parse()
            .map_err(|_| HostError::new("invalid H2_BASE_PORT"))?;
        cmd.env("H2_BASE_PORT", port_for(first, 7, slot)?.to_string());
        let tunnel_addr = if lease.transport == Transport::Tunnel {
            let mut address: SocketAddr = std::env::var("H2_TUNNEL_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:31100".into())
                .parse()
                .map_err(|_| HostError::new("invalid H2_TUNNEL_LISTEN"))?;
            // The host listener trusts connections from the relay. Keep it
            // private even when a shared environment uses the container bind.
            if address.ip().is_unspecified() {
                address.set_ip(if address.is_ipv4() {
                    Ipv4Addr::LOCALHOST.into()
                } else {
                    Ipv6Addr::LOCALHOST.into()
                });
            }
            if !address.ip().is_loopback() {
                return Err(HostError::new("local H2_TUNNEL_LISTEN must use a loopback address"));
            }
            address.set_port(port_for(address.port(), 1, slot)?);
            cmd.env("H2_TUNNEL_LISTEN", address.to_string());
            Some(address)
        } else {
            if let Ok(public) = std::env::var("H2_PUBLIC_ADDR") {
                // A bare IP already uses H2_BASE_PORT. An explicit forwarding
                // base reserves a matching external window for each local host.
                if let Ok(mut address) = public.parse::<SocketAddrV4>() {
                    address.set_port(port_for(address.port(), 7, slot)?);
                    cmd.env("H2_PUBLIC_ADDR", address.to_string());
                } else if public.parse::<Ipv4Addr>().is_err() {
                    return Err(HostError::new("invalid H2_PUBLIC_ADDR"));
                }
            }
            None
        };
        let data = self.root.join(&lease.id);
        std::fs::create_dir_all(&data)?;
        cmd.arg("--data").arg(data);
        #[cfg(unix)]
        cmd.process_group(0);
        Ok((cmd, tunnel_addr))
    }
}

#[cfg(unix)]
fn stop_child(child: &mut Child) -> io::Result<()> {
    unsafe extern "C" {
        fn killpg(group: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
    }
    // The supervisor spawns the actual light game. Stop the whole group, or
    // killing just the supervisor on macOS detaches a still-running match.
    // SAFETY: command() made this child its group leader. The caller holds
    // the unreaped Child under the table lock, so its PID cannot be reused.
    const SIGKILL: std::ffi::c_int = 9;
    let group = i32::try_from(child.id()).map_err(|_| io::Error::other("invalid child PID"))?;
    if unsafe { killpg(group, SIGKILL) } != 0 {
        let error = io::Error::last_os_error();
        if child.try_wait()?.is_none() {
            return Err(error);
        }
    }
    child.wait()?;
    Ok(())
}

#[cfg(not(unix))]
fn stop_child(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_none() {
        if let Err(error) = child.kill()
            && child.try_wait()?.is_none()
        {
            return Err(error);
        }
        child.wait()?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl HostProvider for LocalProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Local
    }

    async fn start(&self, lease: &HostLease, boot: &HostBoot) -> Result<String, HostError> {
        let mut children = self.children.lock().unwrap();
        if let Some((pid, host)) = children
            .iter_mut()
            .find(|(_, host)| host.lease_id == lease.id)
        {
            let pid = pid.clone();
            if host.child.try_wait()?.is_some() {
                children.remove(&pid);
                return Err(HostError::new("local host already exited"));
            }
            return Ok(pid);
        }
        let slot = (0..=u16::MAX)
            .find(|slot| children.values().all(|host| host.slot != *slot))
            .ok_or_else(|| HostError::new("no free local host slots"))?;
        let (mut command, tunnel_addr) = self.command(lease, boot, slot)?;
        let child = command.spawn()?;
        let pid = child.id().to_string();
        children.insert(
            pid.clone(),
            LocalHost {
                lease_id: lease.id.clone(),
                slot,
                tunnel_addr,
                child,
            },
        );
        Ok(pid)
    }

    async fn stop(&self, lease_id: &str, provider_ref: &str) -> Result<(), HostError> {
        let mut children = self.children.lock().unwrap();
        if let Some(host) = children.get_mut(provider_ref)
            && host.lease_id == lease_id
        {
            stop_child(&mut host.child)?;
            children.remove(provider_ref);
        }
        Ok(())
    }

    #[cfg(feature = "native")]
    fn tunnel_addr(&self, lease_id: &str, provider_ref: &str) -> Option<SocketAddr> {
        let mut children = self.children.lock().unwrap();
        let host = children.get_mut(provider_ref)?;
        if host.lease_id != lease_id || !matches!(host.child.try_wait(), Ok(None)) {
            return None;
        }
        host.tunnel_addr
    }

    async fn alive(&self, provider_ref: &str) -> Result<bool, HostError> {
        let mut children = self.children.lock().unwrap();
        let Some(host) = children.get_mut(provider_ref) else {
            return Ok(false);
        };
        if host.child.try_wait()?.is_some() {
            children.remove(provider_ref);
            return Ok(false);
        }
        Ok(true)
    }
}

impl Drop for LocalProvider {
    fn drop(&mut self) {
        for host in self.children.get_mut().unwrap().values_mut() {
            let _ = stop_child(&mut host.child);
        }
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
