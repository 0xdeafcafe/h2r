//! A dedicated System Link host job (`h2 lab run --dedicated`,
//! docs/runtime/lab.md, "Dedicated System Link host"): headless
//! `h2 serve --system-link --host-light` on the lab's image, its XNet bound
//! to this Mac's LAN IPv4 so a player on another Mac can join, and its
//! agent socket where `h2 lab agent` finds it. It runs until `h2 lab stop`.
//! With a map the job opens the lobby itself once the host is at the menu.

use std::fs::{self, File};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_proto::host_lobby::OpenParams;
use anyhow::{Context, Result, ensure};
use serde_json::json;

use super::super::proto::{Dedicated, RunReq};
use super::{HostConfig, Image, JobOutcome, Shared, agent_socket_path};

/// How long the host may take to open its agent socket and reach the main
/// menu before the job gives up opening the lobby (the host keeps running).
const MENU_TIMEOUT: Duration = Duration::from_secs(600);

/// Seconds `host.lobby.open` may take: it waits for retail to publish the
/// lobby's parameters.
const OPEN_TIMEOUT_S: u64 = 120;

/// Ports an XNet host binds from its base: title channels +0, +1, +5, +6,
/// QoS +2, co-op +4 (`live_hle::NetworkConfig`).
const XNET_PORTS: u16 = 7;

pub(super) fn run(
    sh: &Arc<Shared>,
    id: &str,
    run: &RunReq,
    d: &Dedicated,
    image: &Image,
    started: Instant,
) -> Result<JobOutcome> {
    let cfg = &sh.cfg;
    let run_dir = cfg.dir("runs").join(id);
    let mut log = File::create(run_dir.join("run.log"))?;
    writeln!(
        log,
        "lab job {id} · {} · dedicated System Link host",
        run.name
    )?;
    let ip = route_ipv4().context(
        "no IPv4 default route: a dedicated host needs a LAN address another Mac can reach",
    )?;
    let xnet = SocketAddrV4::new(ip, d.base_port());
    check_xnet_ports(xnet)?;
    // A data root of the job's own, removed with its scratch directory.
    let data = cfg.dir("scratch").join(id).join("data");
    fs::create_dir_all(&data)?;
    let sock = agent_socket_path(cfg, id);
    if let Some(dir) = sock.parent() {
        fs::create_dir_all(dir)?;
    }
    let _ = fs::remove_file(&sock);
    let cmd = serve_command(cfg, id, run, d, image, xnet, &data, &sock);
    let note = join_note(xnet);
    writeln!(log, "lab: {note}")?;
    sh.update(id, |m| {
        m.state = "running".into();
        m.run_elapsed_s = Some(started.elapsed().as_secs_f64());
        m.agent_socket = Some(sock.display().to_string());
        m.h2 = Some(image.h2.display().to_string());
        m.system_link = Some(xnet.to_string());
        m.notes.push(note);
    });
    if let Some(params) = d.lobby() {
        let sh = Arc::clone(sh);
        let (id, h2, sock, log) = (
            id.to_owned(),
            image.h2.clone(),
            sock.clone(),
            log.try_clone()?,
        );
        std::thread::spawn(move || open_lobby(&sh, &id, &h2, &sock, &params, log));
    }
    let st = sh.step(id, cmd, &log)?;
    Ok(JobOutcome::Finished(Some(st)))
}

/// What the job's notes and log tell the person joining.
fn join_note(xnet: SocketAddrV4) -> String {
    format!(
        "System Link host at {xnet} (UDP {}-{}): join from a Mac on this LAN running the same \
         commit with XNet base port {} (H2_XNET_ADDR=<its IP>:{} for a base other than 3074)",
        xnet.port(),
        xnet.port() + XNET_PORTS - 1,
        xnet.port(),
        xnet.port()
    )
}

/// `h2 serve <disc> --system-link --host-light` for a dedicated job, on the
/// lab's image and with its own data root, the host's own `H2_*` variables
/// cleaned out as for a game run. `xnet` is the LAN address and base port
/// the host binds; `sock` the agent socket `h2 lab agent` calls.
#[allow(clippy::too_many_arguments)]
fn serve_command(
    cfg: &HostConfig,
    id: &str,
    run: &RunReq,
    d: &Dedicated,
    image: &Image,
    xnet: SocketAddrV4,
    data: &Path,
    sock: &Path,
) -> Command {
    let run_dir = cfg.dir("runs").join(id);
    let mut cmd = Command::new(&image.h2);
    cmd.arg("serve")
        .arg(&cfg.disc)
        .args(["--system-link", "--host-light"]);
    if d.bots {
        cmd.arg("--host-bots");
    }
    if let Some(out) = &image.out {
        cmd.arg("--out").arg(out);
    }
    // A fresh data root would otherwise import Halo 1 and the map pack on
    // every job; `h2 host run` leaves both out too.
    cmd.arg("--data")
        .arg(data)
        .arg("--socket")
        .arg(sock)
        .args(["--no-halo1", "--no-mappack"]);
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cmd.env_remove(k);
        }
    }
    cmd.envs(&image.env).envs(&run.env);
    cmd.env("H2_HOST_LOBBY", "1")
        .env("H2_BOOT_REALTIME", "1")
        .env("H2_XNET_ADDR", xnet.to_string())
        .env("H2_LAB_JOB_ID", id)
        .env("H2_LAB_JOB_NAME", &run.name)
        .env(agent_proto::ENV, sock)
        .env(agent_proto::OUT_ENV, &run_dir)
        .env("H2_MEM_REPORT_DIR", run_dir.join("mem"));
    cmd.current_dir(&run_dir);
    cmd
}

/// The IPv4 the default route leaves from, chosen as the XNet HLE does
/// (`live_hle::config::lan_address`): a connected UDP socket's local
/// address, without sending a packet. None without a route or on loopback.
fn route_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_loopback() && !a.ip().is_unspecified() => Some(*a.ip()),
        _ => None,
    }
}

/// Refuses a base whose seven ports another process holds, before the host
/// starts and fails on its own less plainly.
fn check_xnet_ports(base: SocketAddrV4) -> Result<()> {
    for port in base.port()..base.port() + XNET_PORTS {
        UdpSocket::bind(SocketAddrV4::new(*base.ip(), port)).with_context(|| {
            format!(
                "UDP {}:{port} is in use; a dedicated host needs {} to {} free (--base-port picks another base)",
                base.ip(),
                base.port(),
                base.port() + XNET_PORTS - 1
            )
        })?;
    }
    Ok(())
}

/// Opens the job's lobby once the host is at the menu, through the image's
/// own `h2 agent` as `h2 lab agent` calls it, and notes the outcome in the
/// job. A failure leaves the host up at the menu for the driver.
fn open_lobby(sh: &Shared, id: &str, h2: &Path, sock: &Path, params: &OpenParams, mut log: File) {
    let result = (|| -> Result<()> {
        let deadline = Instant::now() + MENU_TIMEOUT;
        while !sock.exists() {
            ensure!(running(sh, id), "the host exited first");
            ensure!(
                Instant::now() < deadline,
                "the host opened no agent socket in {} s",
                MENU_TIMEOUT.as_secs()
            );
            std::thread::sleep(Duration::from_millis(250));
        }
        let wait =
            json!({"until": {"phase": "menu"}, "timeout_ms": MENU_TIMEOUT.as_millis() as u64});
        agent(
            h2,
            sock,
            agent_proto::method::WAIT,
            &wait.to_string(),
            MENU_TIMEOUT.as_secs() + 20,
        )?;
        agent(
            h2,
            sock,
            agent_proto::method::HOST_LOBBY_OPEN,
            &serde_json::to_string(params)?,
            OPEN_TIMEOUT_S,
        )
    })();
    let note = match result {
        Ok(()) => format!(
            "System Link lobby open: {} ({}), up to {} players",
            params.map,
            params.variant.as_deref().unwrap_or("Slayer"),
            params.max_players
        ),
        Err(e) => format!(
            "opening the lobby failed: {e:#}; the host stays at the menu \
             (h2 lab agent {id} host.lobby.open '{}')",
            serde_json::to_string(params).unwrap_or_default()
        ),
    };
    let _ = writeln!(log, "lab: {note}");
    sh.update(id, |m| m.notes.push(note));
}

/// Whether job `id` is still the running job (not stopping).
fn running(sh: &Shared, id: &str) -> bool {
    let inner = sh.inner.lock().unwrap();
    inner.active.as_ref().is_some_and(|a| a.id == id && !a.stop)
        && inner.jobs.get(id).is_some_and(|m| m.state == "running")
}

/// One `h2 agent <method> <params> --socket <sock> --timeout <secs>` call;
/// an error carries its output.
fn agent(h2: &Path, sock: &Path, method: &str, params: &str, secs: u64) -> Result<()> {
    let out = agent_command(h2, sock, method, params, secs)
        .output()
        .with_context(|| format!("starting {}", h2.display()))?;
    ensure!(
        out.status.success(),
        "{method}: {}{}",
        String::from_utf8_lossy(&out.stderr).trim(),
        String::from_utf8_lossy(&out.stdout).trim()
    );
    Ok(())
}

fn agent_command(h2: &Path, sock: &Path, method: &str, params: &str, secs: u64) -> Command {
    let mut cmd = Command::new(h2);
    cmd.arg("agent")
        .arg(method)
        .arg(params)
        .arg("--socket")
        .arg(sock)
        .arg("--timeout")
        .arg(secs.to_string())
        .stdin(Stdio::null());
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cmd.env_remove(k);
        }
    }
    cmd
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;

    use super::*;

    fn cfg() -> HostConfig {
        HostConfig {
            root: "/lab".into(),
            socket: "/lab/lab.sock".into(),
            repo: "/repo".into(),
            disc: "/discs/halo2.iso".into(),
            h2: "/bin/h2".into(),
            out: None,
            data: "/lab/data".into(),
            cargo: "cargo".into(),
            obj_cache_limit: 0,
            target_limit: 0,
            max_jobs: 1,
            lan: None,
            managed: false,
            explicit_h2: false,
            grace: Duration::from_secs(5),
            takeover: super::super::takeover::Config::default(),
        }
    }

    fn image() -> Image {
        Image {
            h2: "/lab/image/h2".into(),
            out: Some("/lab/image".into()),
            env: BTreeMap::from([("H2_OPT".into(), "2".into())]),
        }
    }

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// The variables the command sets (removals of the host's own are not
    /// listed).
    fn envs(cmd: &Command) -> BTreeMap<String, String> {
        cmd.get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    fn command(d: &Dedicated, run: &RunReq) -> Command {
        serve_command(
            &cfg(),
            "0007",
            run,
            d,
            &image(),
            "192.168.1.20:3074".parse().unwrap(),
            Path::new("/lab/scratch/0007/data"),
            Path::new("/lab/a/0007.sock"),
        )
    }

    #[test]
    fn a_dedicated_host_is_a_light_system_link_serve_on_the_labs_image() {
        let run = RunReq {
            name: "Join test".into(),
            env: BTreeMap::from([("H2_FEATURES".into(), "x".into())]),
            ..RunReq::default()
        };
        let cmd = command(&Dedicated::default(), &run);
        assert_eq!(cmd.get_program(), OsStr::new("/lab/image/h2"));
        assert_eq!(
            args(&cmd),
            [
                "serve",
                "/discs/halo2.iso",
                "--system-link",
                "--host-light",
                "--out",
                "/lab/image",
                "--data",
                "/lab/scratch/0007/data",
                "--socket",
                "/lab/a/0007.sock",
                "--no-halo1",
                "--no-mappack",
            ]
        );
        let env = envs(&cmd);
        for (k, v) in [
            ("H2_OPT", "2"),
            ("H2_FEATURES", "x"),
            ("H2_HOST_LOBBY", "1"),
            ("H2_BOOT_REALTIME", "1"),
            ("H2_XNET_ADDR", "192.168.1.20:3074"),
            ("H2_LAB_JOB_ID", "0007"),
            ("H2_LAB_JOB_NAME", "Join test"),
            ("H2_AGENT", "/lab/a/0007.sock"),
            ("H2_AGENT_OUT", "/lab/runs/0007"),
            ("H2_MEM_REPORT_DIR", "/lab/runs/0007/mem"),
        ] {
            assert_eq!(env.get(k).map(String::as_str), Some(v), "{k}");
        }
        // No frame limit, no frame stats file: it runs until stopped.
        assert!(!env.contains_key("H2_BOOT_STOP_AT_PRESENT"));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/lab/runs/0007")));
    }

    #[test]
    fn bots_add_host_bots_and_a_current_image_keeps_serves_default_out() {
        let d = Dedicated {
            bots: true,
            ..Dedicated::default()
        };
        let cmd = serve_command(
            &cfg(),
            "0008",
            &RunReq::default(),
            &d,
            &Image {
                h2: "/bin/h2".into(),
                out: None,
                env: BTreeMap::new(),
            },
            "10.0.0.5:31000".parse().unwrap(),
            Path::new("/d"),
            Path::new("/s.sock"),
        );
        let a = args(&cmd);
        assert_eq!(&a[2..5], ["--system-link", "--host-light", "--host-bots"]);
        assert!(!a.iter().any(|a| a == "--out"));
        assert_eq!(envs(&cmd)["H2_XNET_ADDR"], "10.0.0.5:31000");
    }

    #[test]
    fn the_join_note_names_the_address_and_port_range() {
        let note = join_note("192.168.1.20:3074".parse().unwrap());
        assert!(note.contains("192.168.1.20:3074"), "{note}");
        assert!(note.contains("UDP 3074-3080"), "{note}");
    }

    #[test]
    fn a_held_port_in_the_range_is_refused() {
        let held = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        // A base whose seven ports include the held one.
        let base = if port <= 65529 {
            port
        } else {
            port - (XNET_PORTS - 1)
        };
        let err = check_xnet_ports(SocketAddrV4::new(Ipv4Addr::LOCALHOST, base))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&format!("127.0.0.1:{port} is in use")),
            "{err}"
        );
    }

    #[test]
    fn agent_calls_use_the_runs_socket_and_timeout() {
        let cmd = agent_command(
            Path::new("/lab/image/h2"),
            Path::new("/lab/a/0007.sock"),
            agent_proto::method::HOST_LOBBY_OPEN,
            r#"{"map":"lockout"}"#,
            120,
        );
        assert_eq!(
            args(&cmd),
            [
                "agent",
                "host.lobby.open",
                r#"{"map":"lockout"}"#,
                "--socket",
                "/lab/a/0007.sock",
                "--timeout",
                "120",
            ]
        );
    }
}
