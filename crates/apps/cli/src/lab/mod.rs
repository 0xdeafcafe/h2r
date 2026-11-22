//! `h2 lab`: start levels on another Mac over the LAN, replay input recordings
//! and bring back frame stats, profiles and logs (docs/runtime/lab.md).
//!
//! `h2 lab host` runs on the machine that plays, in the user's GUI session,
//! and owns a lab root. Clients speak [`proto`]'s JSON lines over paired TLS
//! on the LAN or a private Unix socket on the host itself.

mod build_cache;
mod client;
mod discovery;
mod forensics;
pub mod host;
mod lan;
pub mod proto;
mod service;
mod simbench;
mod takeover;
mod transfer;
mod watch;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use agent_proto::takeover::MAX_TIMEOUT_SECONDS;
use anyhow::{Context, Result, bail, ensure};

const DEFAULT_MAX_JOBS: usize = 1;

const USAGE: &str = "usage:
  h2 lab host [install] [--root DIR] [--socket PATH] [--repo DIR] [--disc ISO]
              [--h2 PATH] [--out DIR] [--data DIR] [--cargo PATH]
              [--obj-cache-limit SIZE] [--target-limit SIZE]
              [--listen IP:PORT] [--name NAME] [--max-jobs 1]
              [--takeover-timeout SECONDS] [--takeover-cooldown SECONDS]
                          serve on the private Unix socket; --listen enables LAN
                          install starts an Aqua LaunchAgent with LAN enabled
                          (default 0.0.0.0:29422) and prints a pairing code
                          one build or run at a time; later jobs queue
  h2 lab host status | update | uninstall
                          inspect, update or remove the local LaunchAgent
  h2 lab host watch [--root DIR]
                          follow jobs and LaunchAgent logs on the host
  h2 lab host pair-code | unpair [--root DIR]
                          issue a fresh code or revoke the paired driver
  h2 lab pair HOST CODE   pair with a Bonjour name, IP, or IP:port
  h2 lab record --name TEXT --level MAP [--difficulty D] [--record KEY] [-- play flags]
                          play with your input recorded into recordings/NAME
                          (through the host when it runs, else directly)

  h2 lab <command> --host NAME|IP[:PORT]|local [--socket PATH for local]
    status                the host, its checkout and active jobs
    update                pull, build and restart an idle installed host
    run --name TEXT [--level MAP] [--difficulty D] [--frames N] [--stats-from N] [--no-stats]
        [--replay NAME | --record NAME] [--film NAME] [--perf] [--env KEY=VALUE]...
        [--branch NAME | --rev REV | --current] [--no-sync] [--feature F]...
        [--build-env KEY=VALUE]... [--fresh-data | --own-data] [--note TEXT]
        [--mem-limit SIZE] [--serve] [--wait [--to DIR]] [-- play flags]
                          start a run (building its image first unless cached);
                          prints the job id; --wait waits and fetches it;
                          --serve: no frame limit, driven by `agent` until `stop`;
                          --film NAME: record a film into the host's film library
                          (and runs/<id>/NAME.h2film for `fetch`)
    run --name TEXT --dedicated [--host-bots] [--map MAP [--variant NAME] [--max-players N]]
        [--base-port PORT] [--env KEY=VALUE]... [--branch NAME | --rev REV | --current]
        [--no-sync] [--feature F]... [--build-env KEY=VALUE]... [--mem-limit SIZE] [--note TEXT]
                          a dedicated System Link host: headless `h2 serve --system-link
                          --host-light` on the host's LAN IPv4 (the job's system_link),
                          until `stop`; --map opens its lobby; XNet base 3174 unless
                          --base-port (never the player's 3074-3080); driven by
                          `agent` like --serve
    agent <id> <method> [json-params] [--timeout S]
                          one `h2 agent` call against that run's game, run on the
                          host; its output and exit code come back unchanged
                          (`-` reads a script from stdin)
    bench --name TEXT --level MAP [--difficulty D] [--scale N] [--ticks N] [--env KEY=VALUE]...
          [--branch NAME | --rev REV | --current] [--no-sync] [--feature F]...
          [--build-env KEY=VALUE]... [--timeout S] [--to DIR] [-- play flags]
                          a --serve run: wait for gameplay, capture N ticks (600),
                          render.stats and profile.counters, stop, fetch, and print
                          one summary line (fps, busy p50/p95, phase means)
    sim <match.toml|batch.toml> [--repeat N] [--arm NAME=H2_FEATURES]... [--parallel N]
        [--env KEY=VALUE]... [--name TEXT] [--branch NAME | --rev REV | --current] [--no-sync]
        [--feature F]... [--build-env KEY=VALUE]... [--mem-limit SIZE] [--note TEXT]
        [--data DIR] [--wait [--to DIR]]
                          benchmark `h2 sim run|batch` on the host's image: the file and
                          the tuning files it names are uploaded, every arm runs N times
                          (interleaved, one at a time), reports and logs stay in runs/<id>;
                          --wait fetches them and prints per-arm median/min/max speed-up and
                          ms per tick, and whether every run's state hashes agree
    build [--branch NAME | --rev REV] [--feature F]... [--build-env KEY=VALUE]... [--wait]
    wait <id> [--timeout SECONDS]
    stop <id>             SIGTERM, then SIGKILL after 5 s
    runs [--limit N] [--json]
    mem <id> [--to DIR]   fetch memory evidence and print its largest growers
    logs <id|current> [--follow]
                          show recent output for one job or the active job
    fetch <id> [--to DIR] copy runs/<id> here (default $RUSH_KEEP/lab, else ./lab)
    put <file> [--as NAME] [--level MAP] [--difficulty D] [--commit SHA] [--pgo]
                          upload an input recording (or with --pgo a PGO profile)
    recordings
    sync [--branch NAME]...
                          send branches (default: the current one) to the
                          host's checkout as lab/<name>

--host defaults to $H2_LAB_HOST. LAN commands require pairing.
--own-data plays on the host Mac's own data root, Live accounts and settings,
as `h2 play` there would, instead of the lab's data/ (not with --fresh-data).
A run builds the commit its branch points at (--branch, default this
checkout's current branch, synced first unless --no-sync), with its features
and build variables (H2_OPT, H2_PGO=use:NAME, ...), cached per commit and
build; --current runs the host's own h2 and image as they are.";

pub fn run(args: Vec<String>) -> Result<()> {
    let Some(cmd) = args.first().cloned() else {
        bail!("{USAGE}")
    };
    let rest = args[1..].to_vec();
    match cmd.as_str() {
        "host" => match rest.first().map(String::as_str) {
            Some("watch") => watch::local(&rest[1..]),
            Some("serve-installed") => service::managed(&rest[1..]),
            Some(
                op @ ("install" | "uninstall" | "status" | "update" | "pair-code" | "unpair"
                | "service-abi"),
            ) => service::command(op, &rest[1..]),
            Some("-h" | "--help" | "help") => {
                println!("{USAGE}");
                Ok(())
            }
            _ => host::serve(host_config(&rest)?),
        },
        "pair" => {
            let [host, code] = rest.as_slice() else {
                bail!("usage: h2 lab pair HOST CODE");
            };
            lan::pair(host, discovery::resolve(host)?, code)?;
            println!("Paired with {host}");
            Ok(())
        }
        "record" => client::record(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => client::run(&cmd, rest),
    }
}

/// The cache directory the agent socket lives in too:
/// [`agent_proto::cache_dir`].
pub fn cache_dir() -> PathBuf {
    agent_proto::cache_dir()
}

/// The host's socket: `$H2_LAB_SOCKET`, else `<cache>/lab.sock`.
pub fn default_socket() -> PathBuf {
    std::env::var_os("H2_LAB_SOCKET")
        .filter(|s| !s.is_empty())
        .map_or_else(|| cache_dir().join("lab.sock"), PathBuf::from)
}

pub fn host_config(args: &[String]) -> Result<host::HostConfig> {
    parse_host_config(args, None)
}

fn install_config(args: &[String]) -> Result<host::HostConfig> {
    parse_host_config(
        args,
        Some(SocketAddr::from((Ipv4Addr::UNSPECIFIED, discovery::PORT))),
    )
}

fn parse_host_config(
    args: &[String],
    default_listener: Option<SocketAddr>,
) -> Result<host::HostConfig> {
    let mut root = None;
    let mut socket = None;
    let mut repo = None;
    let mut disc = None;
    let mut h2 = None;
    let mut out = None;
    let mut data = None;
    let mut cargo = None;
    let mut obj_cache_limit = build_cache::DEFAULT_OBJECT_LIMIT;
    let mut target_limit = build_cache::DEFAULT_TARGET_LIMIT;
    let mut max_jobs = DEFAULT_MAX_JOBS;
    let mut listen = None;
    let mut name = None;
    let mut local_only = false;
    let mut takeover = takeover::Config::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .map(PathBuf::from)
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--root" => root = Some(val()?),
            "--socket" => socket = Some(val()?),
            "--repo" => repo = Some(val()?),
            "--disc" => disc = Some(val()?),
            "--h2" => h2 = Some(val()?),
            "--out" => out = Some(val()?),
            "--data" => data = Some(val()?),
            "--cargo" => cargo = Some(val()?),
            "--obj-cache-limit" | "--target-limit" => {
                let size = it.next().with_context(|| format!("{a} needs a size"))?;
                let bytes = mem_watch::parse_size(size).map_err(anyhow::Error::msg)?;
                if a == "--obj-cache-limit" {
                    obj_cache_limit = bytes;
                } else {
                    target_limit = bytes;
                }
            }
            "--max-jobs" => {
                max_jobs = it
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&n| n > 0)
                    .context("--max-jobs needs a positive integer")?;
                // One build or game at a time keeps runs measurable and the
                // one image and checkout consistent (docs/runtime/lab.md).
                ensure!(
                    max_jobs == DEFAULT_MAX_JOBS,
                    "--max-jobs {max_jobs}: the lab runs one job at a time; later jobs queue"
                );
            }
            "--listen" => {
                listen = Some(
                    it.next()
                        .context("--listen needs IP:PORT")?
                        .parse::<SocketAddr>()?,
                );
            }
            "--name" => {
                name = Some(
                    it.next()
                        .context("--name needs a Bonjour service name")?
                        .clone(),
                )
            }
            "--takeover-timeout" => {
                takeover.timeout_seconds = it
                    .next()
                    .context("--takeover-timeout needs seconds")?
                    .parse()?;
                ensure!(
                    (1..=MAX_TIMEOUT_SECONDS).contains(&takeover.timeout_seconds),
                    "--takeover-timeout must be from 1 to {MAX_TIMEOUT_SECONDS} seconds"
                );
            }
            "--takeover-cooldown" => {
                takeover.cooldown_seconds = it
                    .next()
                    .context("--takeover-cooldown needs seconds")?
                    .parse()?;
            }
            "--local-only" => local_only = true,
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    ensure!(
        !local_only || (listen.is_none() && name.is_none() && default_listener.is_none()),
        "--local-only is exclusive with --listen, --name and host install"
    );
    let listen = listen.or(default_listener);
    ensure!(
        name.is_none() || listen.is_some(),
        "--name requires --listen"
    );

    let abs = |p: PathBuf| -> Result<PathBuf> {
        Ok(if p.is_absolute() {
            p
        } else {
            std::env::current_dir()?.join(p)
        })
    };
    let root = abs(root.unwrap_or_else(|| cache_dir().join("lab")))?;
    let repo = repo.unwrap_or_else(crate::repo_root);
    let repo = repo
        .canonicalize()
        .with_context(|| format!("checkout {}", repo.display()))?;
    let disc = match disc.or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from)) {
        Some(d) => d,
        None if repo.join("disc/halo2.iso").is_file() => repo.join("disc/halo2.iso"),
        None => crate::default_disc()?,
    };
    let explicit_h2 = h2.is_some();
    Ok(host::HostConfig {
        socket: abs(socket.unwrap_or_else(default_socket))?,
        data: abs(data.unwrap_or_else(|| root.join("data")))?,
        root,
        repo,
        disc: abs(disc)?,
        h2: abs(h2.map_or_else(std::env::current_exe, Ok)?)?,
        out: out.map(abs).transpose()?,
        cargo: cargo.unwrap_or_else(|| "cargo".into()),
        obj_cache_limit,
        target_limit,
        max_jobs,
        lan: listen.map(|addr| lan::Listen { addr, name }),
        explicit_h2,
        managed: false,
        grace: Duration::from_secs(5),
        takeover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_install_accept_only_a_job_cap_of_one() {
        let mut args = vec!["--disc".into(), "disc.iso".into()];
        assert_eq!(host_config(&args).unwrap().max_jobs, 1);
        assert_eq!(install_config(&args).unwrap().max_jobs, 1);

        args.extend(["--max-jobs".into(), "1".into()]);
        assert_eq!(host_config(&args).unwrap().max_jobs, 1);
        assert_eq!(install_config(&args).unwrap().max_jobs, 1);

        *args.last_mut().unwrap() = "2".into();
        for refused in [host_config(&args), install_config(&args)] {
            assert!(
                refused
                    .unwrap_err()
                    .to_string()
                    .contains("one job at a time")
            );
        }

        for value in ["0", "-1", "many"] {
            *args.last_mut().unwrap() = value.into();
            assert_eq!(
                host_config(&args).unwrap_err().to_string(),
                "--max-jobs needs a positive integer"
            );
        }
    }

    #[test]
    fn only_install_defaults_to_a_lan_listener() {
        let mut args = vec!["--disc".into(), "disc.iso".into()];
        assert!(host_config(&args).unwrap().lan.is_none());
        let installed = install_config(&args).unwrap().lan.unwrap();
        assert_eq!(
            installed.addr,
            "0.0.0.0:29422".parse::<SocketAddr>().unwrap()
        );

        args.extend(["--name".into(), "laptop".into()]);
        assert!(
            host_config(&args)
                .unwrap_err()
                .to_string()
                .contains("--listen")
        );
        assert_eq!(
            install_config(&args).unwrap().lan.unwrap().name.as_deref(),
            Some("laptop")
        );
        args.extend(["--listen".into(), "192.0.2.10:30000".into()]);
        let foreground = host_config(&args).unwrap().lan.unwrap();
        assert_eq!(
            foreground.addr,
            "192.0.2.10:30000".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(foreground.name.as_deref(), Some("laptop"));
    }
}
