//! `h2 lab`: start levels on another Mac over SSH, replay input recordings
//! and bring back frame stats, profiles and logs (docs/runtime/lab.md).
//!
//! `h2 lab host` runs on the machine that plays, in the user's GUI session,
//! and owns a lab root and a Unix socket. Everything else is a client: it
//! reaches the socket through `ssh HOST <h2> lab pipe` (or directly with
//! `--host local`) and speaks [`proto`]'s JSON lines.

mod client;
pub mod host;
pub mod proto;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage:
  h2 lab host [--root DIR] [--socket PATH] [--repo DIR] [--disc ISO] [--h2 PATH]
              [--out DIR] [--data DIR] [--cargo PATH]
                          serve the lab on this machine (run it in a Terminal in
                          your logged-in session, so games get a window and GPU)
  h2 lab pipe [--socket PATH]
                          bridge stdin/stdout to the lab socket (what ssh runs)
  h2 lab record --level MAP [--difficulty D] [--name NAME] [-- play flags]
                          play with your input recorded into recordings/NAME
                          (through the host when it runs, else directly)

  h2 lab <command> --host [user@]HOST|local [--socket PATH] [--remote-h2 PATH]
    status                the host, its checkout and the job running
    run [--level MAP] [--difficulty D] [--frames N] [--stats-from N] [--no-stats]
        [--replay NAME | --record NAME] [--perf] [--env KEY=VALUE]...
        [--branch NAME | --rev REV | --current] [--no-sync] [--feature F]...
        [--build-env KEY=VALUE]... [--fresh-data] [--note TEXT]
        [--serve] [--wait [--to DIR]] [-- play flags]
                          start a run (building its image first unless cached);
                          prints the job id; --wait waits and fetches it;
                          --serve: no frame limit, driven by `agent` until `stop`
    agent <id> <method> [json-params] [--timeout S]
                          one `h2 agent` call against that run's game, run on the
                          host; its output and exit code come back unchanged
                          (`-` reads a script from stdin)
    bench --level MAP [--difficulty D] [--scale N] [--ticks N] [--env KEY=VALUE]...
          [--branch NAME | --rev REV | --current] [--no-sync] [--feature F]...
          [--build-env KEY=VALUE]... [--timeout S] [--to DIR] [-- play flags]
                          a --serve run: wait for gameplay, capture N ticks (600),
                          render.stats and profile.counters, stop, fetch, and print
                          one summary line (fps, busy p50/p95, phase means)
    build [--branch NAME | --rev REV] [--feature F]... [--build-env KEY=VALUE]... [--wait]
    wait <id> [--timeout SECONDS]
    stop <id>             SIGTERM, then SIGKILL after 5 s
    runs [--limit N] [--json]
    fetch <id> [--to DIR] copy runs/<id> here (default $RUSH_KEEP/lab, else ./lab)
    put <file> [--as NAME] [--level MAP] [--difficulty D] [--commit SHA] [--pgo]
                          upload an input recording (or with --pgo a PGO profile)
    recordings
    sync [--branch NAME]...
                          push branches (default: the current one) into the host's
                          checkout as lab/<name>, over SSH; never anywhere else

--host defaults to $H2_LAB_HOST. With a remote host the lab runs
`ssh HOST <remote-h2> lab pipe`: --remote-h2 or $H2_LAB_REMOTE_H2, else the
link `h2 lab host` keeps at Library/Caches/h2recomp/lab/h2 under the remote
home. $H2_LAB_SSH replaces the ssh command (it gets HOST, then the command).
A run builds the commit its branch points at (--branch, default this
checkout's current branch, pushed first unless --no-sync), with its features
and build variables (H2_OPT, H2_PGO=use:NAME, ...), cached per commit and
build; --current runs the host's own h2 and image as they are.";

pub fn run(args: Vec<String>) -> Result<()> {
    let Some(cmd) = args.first().cloned() else {
        bail!("{USAGE}")
    };
    let rest = args[1..].to_vec();
    match cmd.as_str() {
        "host" => host::serve(host_config(&rest)?),
        "pipe" => pipe(rest),
        "record" => client::record(rest),
        "-h" | "--help" | "help" => bail!("{USAGE}"),
        _ => client::run(&cmd, rest),
    }
}

/// `~/Library/Caches/h2recomp` (macOS), else `$XDG_CACHE_HOME/h2recomp` or
/// `~/.cache/h2recomp`: where the agent socket lives too.
pub fn cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cache = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(Into::into)
            .or_else(|| home.map(|h| h.join(".cache")))
    };
    cache.unwrap_or_else(std::env::temp_dir).join("h2recomp")
}

/// The host's socket: `$H2_LAB_SOCKET`, else `<cache>/lab.sock`.
pub fn default_socket() -> PathBuf {
    std::env::var_os("H2_LAB_SOCKET")
        .filter(|s| !s.is_empty())
        .map_or_else(|| cache_dir().join("lab.sock"), PathBuf::from)
}

pub fn host_config(args: &[String]) -> Result<host::HostConfig> {
    let mut root = None;
    let mut socket = None;
    let mut repo = None;
    let mut disc = None;
    let mut h2 = None;
    let mut out = None;
    let mut data = None;
    let mut cargo = None;
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
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
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
    Ok(host::HostConfig {
        socket: abs(socket.unwrap_or_else(default_socket))?,
        data: abs(data.unwrap_or_else(|| root.join("data")))?,
        root,
        repo,
        disc: abs(disc)?,
        h2: abs(h2.map_or_else(std::env::current_exe, Ok)?)?,
        out: out.map(abs).transpose()?,
        cargo: cargo.unwrap_or_else(|| "cargo".into()),
        grace: Duration::from_secs(5),
    })
}

/// `h2 lab pipe`: stdin to the socket, the socket to stdout, until both
/// directions end. It is what `ssh HOST h2 lab pipe` runs.
fn pipe(args: Vec<String>) -> Result<()> {
    let mut socket = default_socket();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = it.next().context("--socket needs a path")?.into(),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let s = std::os::unix::net::UnixStream::connect(&socket).with_context(|| {
        format!(
            "connecting to {}: is `h2 lab host` running?",
            socket.display()
        )
    })?;
    let mut to_sock = s.try_clone()?;
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_sock);
        let _ = to_sock.shutdown(std::net::Shutdown::Write);
    });
    let mut from = s;
    let mut stdout = std::io::stdout().lock();
    // Line by line so each answer leaves at once (stdout is a pipe here).
    let mut buf = [0u8; 64 << 10];
    loop {
        use std::io::{Read, Write};
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        stdout.write_all(&buf[..n])?;
        stdout.flush()?;
    }
    drop(up);
    Ok(())
}
