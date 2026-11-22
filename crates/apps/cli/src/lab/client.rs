//! The lab's client commands (`h2 lab status|run|...`), and `h2 lab record`.

use std::cell::Cell;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Seek, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use super::proto::{self, BuildSpec, RunReq, SimReq};
use super::transfer::Artifacts;
use super::{USAGE, default_socket, discovery, forensics, lan, simbench, transfer, watch};

enum Remote {
    Local(PathBuf),
    Unspecified,
    Lan {
        name: String,
        addr: Cell<Option<SocketAddr>>,
    },
}

impl Remote {
    fn connect(&self) -> Result<Conn> {
        match self {
            Self::Local(path) => {
                let socket = UnixStream::connect(path).with_context(|| {
                    format!(
                        "connecting to {}: is `h2 lab host` running?",
                        path.display()
                    )
                })?;
                Ok(Conn::new(socket))
            }
            Self::Unspecified => {
                bail!("which host? specify --host NAME|IP|local or set H2_LAB_HOST")
            }
            Self::Lan { name, addr } => {
                // Validate the command before Bonjour discovery or a TCP connection.
                let address = match addr.get() {
                    Some(address) => address,
                    None => {
                        let address = discovery::resolve(name)?;
                        addr.set(Some(address));
                        address
                    }
                };
                Ok(Conn::new(lan::connect(name, address)?))
            }
        }
    }
}

trait Wire: Read + Write {}
impl<T: Read + Write> Wire for T {}

struct Conn(BufReader<Box<dyn Wire>>);

impl Conn {
    fn new(socket: impl Wire + 'static) -> Self {
        Self(BufReader::new(Box::new(socket)))
    }

    fn request(&mut self, req: Value) -> Result<()> {
        let w = self.0.get_mut();
        writeln!(w, "{req}")?;
        w.flush()?;
        Ok(())
    }

    fn reply(&mut self) -> Result<Value> {
        read_reply(&mut self.0)
    }

    fn log_reply(&mut self) -> Result<Value> {
        read_reply((&mut self.0).take(transfer::FRAME_MAX as u64))
    }

    fn call(&mut self, req: Value) -> Result<Value> {
        self.request(req)?;
        self.reply()
    }
}

fn read_reply(mut reader: impl BufRead) -> Result<Value> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        bail!("the lab host closed the connection");
    }
    ensure!(line.ends_with('\n'), "unterminated lab reply");
    let value: Value =
        serde_json::from_str(&line).with_context(|| format!("bad answer {line:?}"))?;
    if value["ok"] != Value::Bool(true) {
        bail!("{}", value["error"].as_str().unwrap_or("the host refused"));
    }
    Ok(value)
}

/// Options every client command takes; the rest stay for the command.
fn remote(args: Vec<String>) -> Result<(Remote, Vec<String>)> {
    let mut host = std::env::var("H2_LAB_HOST").ok().filter(|h| !h.is_empty());
    let mut socket = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                rest.push(a);
                rest.extend(it.by_ref());
            }
            "--host" => host = Some(it.next().context("--host needs a name, address or local")?),
            "--socket" => socket = Some(it.next().context("--socket needs a path")?),
            _ => rest.push(a),
        }
    }
    let target = match host {
        None => Remote::Unspecified,
        Some(name) if name == "local" => {
            Remote::Local(socket.map_or_else(default_socket, PathBuf::from))
        }
        Some(name) => {
            ensure!(socket.is_none(), "--socket requires --host local");
            Remote::Lan {
                name,
                addr: Cell::new(None),
            }
        }
    };
    Ok((target, rest))
}

pub fn run(cmd: &str, args: Vec<String>) -> Result<()> {
    let (r, args) = remote(args)?;
    match cmd {
        "status" => {
            no_more(&args)?;
            print(&r.connect()?.call(json!({"cmd": "status"}))?)
        }
        "update" => {
            no_more(&args)?;
            print(&r.connect()?.call(json!({"cmd": "update"}))?)
        }
        "run" => run_cmd(&r, args),
        "agent" => agent_cmd(&r, args),
        "bench" => bench_cmd(&r, args),
        "sim" => sim_cmd(&r, args),
        "build" => build_cmd(&r, args),
        "wait" => {
            let (id, timeout) = id_and_timeout(&args)?;
            let mut c = r.connect()?;
            let v = c.call(json!({"cmd": "wait", "id": id, "timeout": timeout}))?;
            print(&v)?;
            finished_ok(&v)
        }
        "stop" => {
            let [id] = args.as_slice() else {
                bail!("usage: h2 lab stop <id>")
            };
            print(&r.connect()?.call(json!({"cmd": "stop", "id": id}))?)
        }
        "clean" => {
            let all = match args.as_slice() {
                [] => false,
                [flag] if flag == "--all" => true,
                _ => bail!("usage: h2 lab clean [--all]"),
            };
            print(&r.connect()?.call(json!({"cmd": "clean", "all": all}))?)
        }
        "runs" => runs_cmd(&r, args),
        "logs" => logs_cmd(&r, args),
        "fetch" => {
            let (id, to) = forensics::fetch_args(cmd, args)?;
            let dest = fetch(&r, &id, to)?;
            println!("{}", dest.display());
            Ok(())
        }
        "mem" => {
            let (id, to) = forensics::fetch_args(cmd, args)?;
            let (dest, summary) = fetch_artifacts(&r, &id, to, Artifacts::Memory, forensics::Summary::read)?;
            println!("Memory evidence: {}", dest.display());
            summary.print()
        }
        "put" => put_cmd(&r, args),
        "recordings" => {
            no_more(&args)?;
            print(&r.connect()?.call(json!({"cmd": "recordings"}))?)
        }
        "sync" => {
            let mut branches = Vec::new();
            let mut it = args.into_iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--branch" => branches.push(it.next().context("--branch needs a name")?),
                    _ => bail!("unexpected {a}\n\n{USAGE}"),
                }
            }
            if branches.is_empty() {
                branches.push(current_branch()?);
            }
            for (b, sha) in sync(&r, &branches)? {
                println!("lab/{b} {sha}");
            }
            Ok(())
        }
        _ => bail!("unknown lab command {cmd:?}\n\n{USAGE}"),
    }
}

fn no_more(args: &[String]) -> Result<()> {
    ensure!(args.is_empty(), "unexpected {}\n\n{USAGE}", args.join(" "));
    Ok(())
}

fn print(v: &Value) -> Result<()> {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("ok");
    }
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

fn id_and_timeout(args: &[String]) -> Result<(String, Option<f64>)> {
    let mut id = None;
    let mut timeout = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--timeout" => {
                timeout = Some(
                    it.next()
                        .context("--timeout needs seconds")?
                        .parse::<f64>()?,
                )
            }
            _ if id.is_none() => id = Some(a.clone()),
            _ => bail!("unexpected {a}"),
        }
    }
    Ok((
        id.context("usage: h2 lab wait <id> [--timeout SECONDS]")?,
        timeout,
    ))
}

/// Exit status for `wait`: 0 for completed and skipped jobs, 1 when
/// a job failed or was stopped, 3 when it is still going.
fn finished_ok(v: &Value) -> Result<()> {
    if v["finished"] != Value::Bool(true) {
        std::process::exit(3);
    }
    if !matches!(v["job"]["state"].as_str(), Some("done" | "skipped")) {
        std::process::exit(1);
    }
    Ok(())
}

fn kv(s: &str, what: &str) -> Result<(String, String)> {
    let (k, v) = s
        .split_once('=')
        .with_context(|| format!("{what} needs KEY=VALUE, not {s:?}"))?;
    Ok((k.to_string(), v.to_string()))
}

/// Which commit a run or build uses, from --branch/--rev/--current.
#[derive(Debug, Default)]
struct Source {
    branch: Option<String>,
    rev: Option<String>,
    current: bool,
    no_sync: bool,
}

impl Source {
    fn take(&mut self, a: &str, it: &mut impl Iterator<Item = String>) -> Result<bool> {
        match a {
            "--branch" => self.branch = Some(it.next().context("--branch needs a name")?),
            "--rev" => self.rev = Some(it.next().context("--rev needs a commit")?),
            "--current" => self.current = true,
            "--no-sync" => self.no_sync = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Sets the request's rev, and the branch it came from for the player's
    /// consent prompt (none for --rev and --current).
    fn apply(&self, r: &Remote, build: &mut BuildSpec) -> Result<()> {
        build.rev = self.resolve(r)?;
        build.branch = (build.rev.is_some() && self.rev.is_none())
            .then(|| match &self.branch {
                Some(b) => Ok(b.clone()),
                None => current_branch(),
            })
            .transpose()?;
        Ok(())
    }

    /// The rev for the request: none for --current; --rev as given (it must
    /// be on the host already); else the branch, synced first and named by
    /// its commit, or `lab/<branch>` as last synced with --no-sync.
    fn resolve(&self, r: &Remote) -> Result<Option<String>> {
        ensure!(
            [self.branch.is_some(), self.rev.is_some(), self.current]
                .iter()
                .filter(|b| **b)
                .count()
                <= 1,
            "--branch, --rev and --current are exclusive"
        );
        if self.current {
            return Ok(None);
        }
        if let Some(rev) = &self.rev {
            return Ok(Some(rev.clone()));
        }
        let branch = match &self.branch {
            Some(b) => b.clone(),
            None => current_branch()?,
        };
        if self.no_sync {
            return Ok(Some(format!("lab/{branch}")));
        }
        let synced = sync(r, std::slice::from_ref(&branch))?;
        Ok(Some(synced[0].1.clone()))
    }
}

/// What `h2 lab run` was asked: the request, its build source, `--wait`
/// and `--to`.
type RunArgs = (RunReq, Source, bool, Option<PathBuf>);

fn run_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let (req, src, wait, to) = parse_run_args(args)?;
    let (mut c, v) = launch(r, req, &src)?;
    let id = v["id"].as_str().context("no id in the answer")?.to_string();
    if !wait {
        print(&v)?;
        return Ok(());
    }
    eprintln!("lab: job {id} started");
    wait_and_fetch(r, &mut c, &id, to)
}

fn parse_run_args(args: Vec<String>) -> Result<RunArgs> {
    let mut req = RunReq::default();
    let mut src = Source::default();
    let mut wait = false;
    let mut to = None;
    let mut dedicated = false;
    let mut host_opts = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if src.take(&a, &mut it)? {
            continue;
        }
        if let Some(opt) = take_dedicated(&a, &mut it, &mut req)? {
            host_opts.push(opt);
            continue;
        }
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--level" => req.level = Some(val()?),
            "--name" => req.name = proto::job_name(&val()?)?.into(),
            "--difficulty" => req.difficulty = Some(val()?),
            "--mem-limit" => {
                req.mem_limit = Some(mem_watch::parse_size(&val()?).map_err(anyhow::Error::msg)?)
            }
            "--frames" => req.frames = Some(val()?.parse().context("--frames needs a count")?),
            "--stats-from" => {
                req.stats_from = Some(val()?.parse().context("--stats-from needs a present")?)
            }
            "--no-stats" => req.stats = Some(false),
            "--replay" => req.replay = Some(val()?),
            "--record" => req.record = Some(val()?),
            "--film" => req.film = Some(val()?),
            "--perf" => req.perf = true,
            "--env" => {
                let (k, v) = kv(&val()?, "--env")?;
                req.env.insert(k, v);
            }
            "--feature" => req.build.features.push(val()?),
            "--build-env" => {
                let (k, v) = kv(&val()?, "--build-env")?;
                req.build.env.insert(k, v);
            }
            "--fresh-data" => req.fresh_data = true,
            "--own-data" => req.own_data = true,
            "--note" => req.note = Some(val()?),
            "--serve" => req.serve = true,
            "--dedicated" => dedicated = true,
            "--wait" => wait = true,
            "--to" => to = Some(PathBuf::from(val()?)),
            "--" => req.args.extend(it.by_ref()),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    ensure!(
        !(req.own_data && req.fresh_data),
        "--own-data plays on the host's own data; --fresh-data asks for an empty one"
    );
    ensure!(
        !(wait && req.serve),
        "--serve runs until `h2 lab stop`; --wait would wait forever (h2 lab bench does the whole loop)"
    );
    if dedicated {
        req.dedicated.get_or_insert_default();
        ensure!(
            !wait,
            "a --dedicated host runs until `h2 lab stop`; --wait would wait forever"
        );
    } else if let Some(opt) = host_opts.first() {
        bail!("{opt} sets up a --dedicated host; add --dedicated");
    }
    Ok((req, src, wait, to))
}

/// A `--dedicated` host's own flags, into `req.dedicated`. Returns the flag
/// when `a` is one.
fn take_dedicated(
    a: &str,
    it: &mut impl Iterator<Item = String>,
    req: &mut RunReq,
) -> Result<Option<&'static str>> {
    let flag = match a {
        "--host-bots" => "--host-bots",
        "--map" => "--map",
        "--variant" => "--variant",
        "--max-players" => "--max-players",
        "--base-port" => "--base-port",
        _ => return Ok(None),
    };
    let d = req.dedicated.get_or_insert_default();
    let mut val = || it.next().with_context(|| format!("{flag} needs a value"));
    match flag {
        "--host-bots" => d.bots = true,
        "--map" => d.map = Some(val()?),
        "--variant" => d.variant = Some(val()?),
        "--max-players" => {
            d.max_players = Some(
                val()?
                    .parse()
                    .context("--max-players needs a count from 1 to 16")?,
            )
        }
        _ => d.base_port = Some(val()?.parse().context("--base-port needs a UDP port")?),
    }
    Ok(Some(flag))
}

/// Validates `req`, resolves and syncs its build, and starts it. Returns
/// the connection and the host's answer (with the job's `id`).
fn launch(r: &Remote, mut req: RunReq, src: &Source) -> Result<(Conn, Value)> {
    if req.perf && !req.build.features.iter().any(|f| f == "perf") && !src.current {
        req.build.features.push("perf".into());
    }
    // Refuse what the host would before syncing anything.
    proto::validate_run(&RunReq {
        build: BuildSpec {
            rev: Some("x".into()),
            ..req.build.clone()
        },
        ..req.clone()
    })?;
    src.apply(r, &mut req.build)?;
    proto::validate_run(&req)?;
    let mut c = r.connect()?;
    let mut body = serde_json::to_value(&req)?;
    body["cmd"] = "run".into();
    let v = c.call(body)?;
    Ok((c, v))
}

/// `h2 lab sim`: uploads a match or batch file (with the tuning files it
/// names) as a sim benchmark job. With `--wait`, fetches the run and prints
/// its summary (`simbench`).
fn sim_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut src = Source::default();
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if !src.take(&a, &mut it)? {
            rest.push(a);
        }
    }
    let simbench::Parsed { mut req, path, data, wait, to } = simbench::parse_args(rest)?;
    let loaded = simbench::load_files(&path)?;
    // The bots' meshes come from this Mac's data root; a map without one
    // is refused before anything syncs.
    let root = data.unwrap_or_else(content::store::default_data_root);
    req.data = simbench::bot_data(&loaded.maps, &root)?;
    req.file = loaded.file;
    req.batch = loaded.batch;
    req.files = loaded.files;
    // Refuse what the host would before syncing anything.
    proto::validate_sim(&SimReq {
        build: BuildSpec {
            rev: Some("x".into()),
            ..req.build.clone()
        },
        ..req.clone()
    })?;
    src.apply(r, &mut req.build)?;
    proto::validate_sim(&req)?;
    let mut c = r.connect()?;
    let mut body = serde_json::to_value(&req)?;
    body["cmd"] = "sim".into();
    let v = c.call(body)?;
    let id = v["id"].as_str().context("no id in the answer")?.to_string();
    if !wait {
        return print(&v);
    }
    eprintln!("lab: sim job {id} started");
    let v = c.call(json!({"cmd": "wait", "id": id}))?;
    let dest = fetch(r, &id, to)?;
    print(&v)?;
    match std::fs::read_to_string(dest.join(simbench::SUMMARY_TEXT)) {
        Ok(text) => print!("{text}"),
        Err(e) => eprintln!("lab: no {} in {}: {e}", simbench::SUMMARY_TEXT, dest.display()),
    }
    println!("{}", dest.display());
    finished_ok(&v)
}

fn build_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut spec = BuildSpec::default();
    let mut src = Source::default();
    let mut wait = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if src.take(&a, &mut it)? {
            continue;
        }
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--feature" => spec.features.push(val()?),
            "--build-env" => {
                let (k, v) = kv(&val()?, "--build-env")?;
                spec.env.insert(k, v);
            }
            "--wait" => wait = true,
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    ensure!(
        !src.current,
        "build needs a branch or commit; --current is the host's own image"
    );
    src.apply(r, &mut spec)?;
    proto::validate_build(&spec)?;
    let mut c = r.connect()?;
    let mut body = serde_json::to_value(&spec)?;
    body["cmd"] = "build".into();
    let v = c.call(body)?;
    if !wait {
        return print(&v);
    }
    let id = v["id"].as_str().context("no id in the answer")?.to_string();
    let v = c.call(json!({"cmd": "wait", "id": id}))?;
    print(&v)?;
    finished_ok(&v)
}

/// Waits for `id`, fetches its directory and prints its metadata and where
/// it went.
fn wait_and_fetch(r: &Remote, c: &mut Conn, id: &str, to: Option<PathBuf>) -> Result<()> {
    let v = c.call(json!({"cmd": "wait", "id": id}))?;
    let dest = fetch(r, id, to)?;
    print(&v)?;
    println!("{}", dest.display());
    finished_ok(&v)
}

/// Copies the host's `runs/<id>` into `to` as `<to>/<id>`.
fn fetch(r: &Remote, id: &str, to: Option<PathBuf>) -> Result<PathBuf> {
    fetch_artifacts(r, id, to, Artifacts::Run, |_| ()).map(|(path, ())| path)
}

fn fetch_artifacts<T>(
    r: &Remote,
    id: &str,
    to: Option<PathBuf>,
    artifacts: Artifacts,
    inspect: impl FnOnce(&Path) -> T,
) -> Result<(PathBuf, T)> {
    proto::check_job_id(id)?;
    let to = to.unwrap_or_else(
        || match std::env::var_os("RUSH_KEEP").filter(|v| !v.is_empty()) {
            Some(k) => PathBuf::from(k).join("lab"),
            None => PathBuf::from("lab"),
        },
    );
    let mut archive = transfer::TempFile::new(&std::env::temp_dir())?;
    let mut conn = r.connect()?;
    conn.call(json!({"cmd": artifacts.command(), "id": id}))?;
    transfer::receive_archive(&mut conn.0, &mut archive.file)?;
    conn.reply()?;
    std::fs::create_dir_all(&to).with_context(|| format!("creating {}", to.display()))?;
    transfer::extract(&archive.path, &to, id, artifacts, inspect)
}

fn logs_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut id = None;
    let mut follow = false;
    for arg in args {
        match arg.as_str() {
            "--follow" if !follow => follow = true,
            _ if id.is_none() && !arg.starts_with('-') => id = Some(arg),
            _ => bail!("unexpected {arg}\n\n{USAGE}"),
        }
    }
    let id = id.context("usage: h2 lab logs <id|current> [--follow] --host NAME")?;
    proto::check_log_id(&id)?;

    let mut conn = r.connect()?;
    conn.request(json!({"cmd": "logs", "id": id, "follow": follow}))?;
    conn.log_reply()?;

    let mut stdout = io::stdout().lock();
    let color = stdout.is_terminal();
    let received = transfer::receive_logs(&mut conn.0, |line| {
        watch::write_line(&mut stdout, line, color)
    });
    if received
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<io::Error>())
        .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
    {
        return Ok(());
    }
    received?;
    conn.log_reply()?;
    Ok(())
}

fn runs_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut limit = None;
    let mut as_json = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--limit" => {
                limit = Some(
                    it.next()
                        .context("--limit needs a count")?
                        .parse::<usize>()?,
                )
            }
            "--json" => as_json = true,
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let v = r.connect()?.call(json!({"cmd": "runs", "limit": limit}))?;
    if as_json {
        return print(&v);
    }
    for j in v["runs"].as_array().into_iter().flatten() {
        let s = |k: &str| j[k].as_str().unwrap_or("-").to_string();
        let commit: String = s("commit").chars().take(10).collect();
        let dur = j["duration_s"]
            .as_f64()
            .map_or("-".into(), |d| format!("{d:.1}s"));
        let exit = match (j["exit_code"].as_i64(), j["signal"].as_i64()) {
            (Some(c), _) => format!("exit {c}"),
            (_, Some(s)) => format!("signal {s}"),
            _ => "-".into(),
        };
        println!(
            "{} {} {} {} {} {} {} {}",
            s("id"),
            s("kind"),
            s("state"),
            exit,
            dur,
            commit,
            s("level"),
            s("name")
        );
        if let (Some(last), Some(peak)) = (
            j["last_footprint_bytes"].as_u64(),
            j["peak_footprint_bytes"].as_u64(),
        ) {
            println!("    game memory: {last} bytes last, {peak} bytes peak");
        }
        if let Some(e) = j["error"].as_str() {
            println!("    error: {e}");
        }
        if let Some(addr) = j["system_link"].as_str() {
            println!("    System Link host at {addr}");
        }
        if let Some(reason) = j["skip_reason"].as_str() {
            println!("    {reason}");
        }
        for l in j["summary"].as_array().into_iter().flatten() {
            println!("    {}", l.as_str().unwrap_or(""));
        }
    }
    Ok(())
}

fn put_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut file = None;
    let mut name = None;
    let mut level = None;
    let mut difficulty = None;
    let mut commit = None;
    let mut pgo = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--as" => name = Some(val()?),
            "--level" => level = Some(val()?),
            "--difficulty" => difficulty = Some(val()?),
            "--commit" => commit = Some(val()?),
            "--pgo" => pgo = true,
            _ if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let file = file.context("usage: h2 lab put <file> [--as NAME]")?;
    let name = match name {
        Some(n) => n,
        None => file
            .file_name()
            .and_then(|n| n.to_str())
            .context("name it with --as")?
            .to_string(),
    };
    let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    ensure!(
        bytes.len() <= proto::PUT_MAX,
        "{} is over {} MiB",
        file.display(),
        proto::PUT_MAX >> 20
    );
    let mut req = json!({"cmd": "put", "name": name, "hex": proto::to_hex(&bytes)});
    if pgo {
        req["kind"] = "pgo".into();
    }
    for (k, v) in [
        ("level", level),
        ("difficulty", difficulty),
        ("commit", commit),
    ] {
        if let Some(v) = v {
            req[k] = v.into();
        }
    }
    print(&r.connect()?.call(req)?)
}

/// This checkout's branch.
fn current_branch() -> Result<String> {
    let o = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("running git")?;
    ensure!(o.status.success(), "not in a git checkout; name a --branch");
    let b = String::from_utf8_lossy(&o.stdout).trim().to_string();
    ensure!(
        b != "HEAD",
        "HEAD is detached here; name a --branch or a --rev the host has"
    );
    Ok(b)
}

/// Sends committed branch contents as git bundles and updates the host's
/// lab refs. Known commits only need a ref update.
fn sync(r: &Remote, branches: &[String]) -> Result<Vec<(String, String)>> {
    let repo = Path::new(".");
    let dirty = transfer::git(repo)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()?;
    if !dirty.stdout.is_empty() {
        eprintln!("lab: this checkout has uncommitted changes; the host builds commits only");
    }
    let mut synced = Vec::new();
    for branch in branches {
        let (source, _) = transfer::branch_refs(branch)?;
        let commit = transfer::resolve_commit(repo, &source)?
            .with_context(|| format!("no local branch {branch}"))?;
        let info = r
            .connect()?
            .call(json!({"cmd": "sync_bases", "branch": branch, "commit": commit}))?;
        let mut bundle = if info["known"] == true {
            None
        } else {
            let bases: Vec<String> = serde_json::from_value(info["bases"].clone())?;
            Some(transfer::make_bundle(&source, &bases)?)
        };
        let bytes = match &bundle {
            Some(bundle) => bundle.file.metadata()?.len(),
            None => 0,
        };
        ensure!(
            bytes <= proto::BUNDLE_MAX,
            "git bundle is too large for lab sync"
        );
        let mut conn = r.connect()?;
        conn.call(json!({"cmd": "sync", "branch": branch, "commit": commit, "bytes": bytes}))?;
        if let Some(bundle) = &mut bundle {
            bundle.file.rewind()?;
            let w = conn.0.get_mut();
            io::copy(&mut bundle.file, &mut *w)?;
            w.flush()?;
        }
        let result = conn.reply()?;
        ensure!(
            result["commit"].as_str() == Some(commit.as_str()),
            "unexpected commit from server"
        );
        synced.push((branch.clone(), commit));
    }
    Ok(synced)
}

/// `h2 lab record`: the user plays; their input goes to
/// `recordings/<name>`. Through the host when it is up (so the agent sees
/// the run), else straight from here into the default lab root.
pub fn record(args: Vec<String>) -> Result<()> {
    let mut req = RunReq {
        stats: Some(false),
        ..RunReq::default()
    };
    let mut recording = None;
    let mut socket = None;
    let mut host_args = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--level" => req.level = Some(val()?),
            "--name" => req.name = proto::job_name(&val()?)?.into(),
            "--difficulty" => req.difficulty = Some(val()?),
            "--record" => recording = Some(val()?),
            "--socket" => socket = Some(PathBuf::from(val()?)),
            "--root" | "--repo" | "--disc" | "--h2" | "--out" | "--data" => {
                host_args.push(a.clone());
                host_args.push(val()?);
            }
            "--" => req.args.extend(it.by_ref()),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    proto::job_name(&req.name)?;
    let level = req
        .level
        .clone()
        .context("h2 lab record needs --level MAP")?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = recording.unwrap_or_else(|| {
        if proto::check_name("recording", &req.name).is_ok() {
            req.name.clone()
        } else {
            format!("{level:.40}-{secs}")
        }
    });
    req.record = Some(name.clone());
    proto::validate_run(&req)?;
    let socket = socket.unwrap_or_else(default_socket);
    if let Ok(socket) = UnixStream::connect(&socket) {
        let mut c = Conn::new(socket);
        let mut body = serde_json::to_value(&req)?;
        body["cmd"] = "run".into();
        let v = c.call(body)?;
        let id = v["id"].as_str().unwrap_or("?").to_string();
        eprintln!("lab: recording {name} in run {id} through the host; quit the game to finish");
        let v = c.call(json!({"cmd": "wait", "id": id}))?;
        print(&v)?;
        return finished_ok(&v);
    }
    let mut cfg_args = host_args;
    cfg_args.push("--socket".into());
    cfg_args.push(socket.display().to_string());
    let cfg = super::host_config(&cfg_args)?;
    eprintln!(
        "lab: no host on {}; recording {name} directly into {}",
        socket.display(),
        cfg.root.display()
    );
    let st = super::host::run_direct(&cfg, &req)?;
    ensure!(st.success(), "the game exited with {st}");
    println!("{}", cfg.root.join("recordings").join(&name).display());
    Ok(())
}

/// What one forwarded `h2 agent` call answered.
struct AgentAnswer {
    exit: i32,
    stdout: String,
    stderr: String,
}

fn agent_request(
    c: &mut Conn,
    id: &str,
    args: &[String],
    timeout: Option<f64>,
    stdin: Option<String>,
) -> Result<AgentAnswer> {
    let mut req = json!({"cmd": "agent", "id": id, "args": args});
    if let Some(t) = timeout {
        req["timeout"] = t.into();
    }
    if let Some(s) = stdin {
        req["stdin"] = s.into();
    }
    let v = c.call(req)?;
    Ok(AgentAnswer {
        exit: v["exit"].as_i64().unwrap_or(1) as i32,
        stdout: v["stdout"].as_str().unwrap_or("").to_string(),
        stderr: v["stderr"].as_str().unwrap_or("").to_string(),
    })
}

/// `h2 lab agent <id> <method> [json-params] [--timeout S]`: the host runs
/// `h2 agent ...` against run `<id>`'s game. What it printed comes out here
/// unchanged and so does its exit code. `-` reads a script from stdin.
fn agent_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut id = None;
    let mut timeout = None;
    let mut call = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--timeout" if id.is_some() => {
                timeout = Some(
                    it.next()
                        .context("--timeout needs seconds")?
                        .parse::<f64>()
                        .context("--timeout needs seconds")?,
                )
            }
            _ if id.is_none() => id = Some(a),
            _ => call.push(a),
        }
    }
    let id =
        id.context("usage: h2 lab agent --host H <id> <method> [json-params] [--timeout S]")?;
    ensure!(
        !call.is_empty(),
        "which call? h2 lab agent <id> <method> [json-params]"
    );
    proto::check_name("id", &id)?;
    proto::check_agent_args(&call)?;
    let stdin = if call == ["-"] {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Some(s)
    } else {
        None
    };
    let a = agent_request(&mut r.connect()?, &id, &call, timeout, stdin)?;
    print!("{}", a.stdout);
    eprint!("{}", a.stderr);
    std::io::stdout().flush()?;
    if a.exit != 0 {
        std::process::exit(a.exit);
    }
    Ok(())
}

/// `h2 lab bench`: a `--serve` run of `--level`, the agent waiting for
/// gameplay, a capture, `render.stats` and `profile.counters`, then stop and
/// fetch. The three answers are saved into the fetched run directory and one
/// line summarises them, so two invocations with different `--env` are an A/B.
fn bench_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut req = RunReq {
        serve: true,
        bench: true,
        ..RunReq::default()
    };
    let mut src = Source::default();
    let mut ticks = 600u64;
    let mut gameplay_timeout = 600.0f64;
    let mut to = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if src.take(&a, &mut it)? {
            continue;
        }
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--level" => req.level = Some(val()?),
            "--name" => req.name = proto::job_name(&val()?)?.into(),
            "--difficulty" => req.difficulty = Some(val()?),
            "--scale" => {
                let n: u32 = val()?.parse().context("--scale needs a number")?;
                req.env.insert("H2_RENDER_SCALE".into(), n.to_string());
            }
            "--ticks" => ticks = val()?.parse().context("--ticks needs a count")?,
            "--timeout" => gameplay_timeout = val()?.parse().context("--timeout needs seconds")?,
            "--env" => {
                let (k, v) = kv(&val()?, "--env")?;
                req.env.insert(k, v);
            }
            "--feature" => req.build.features.push(val()?),
            "--build-env" => {
                let (k, v) = kv(&val()?, "--build-env")?;
                req.build.env.insert(k, v);
            }
            "--fresh-data" => req.fresh_data = true,
            "--note" => req.note = Some(val()?),
            "--to" => to = Some(PathBuf::from(val()?)),
            "--" => req.args.extend(it.by_ref()),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    proto::job_name(&req.name)?;
    ensure!(req.level.is_some(), "h2 lab bench needs --level MAP");
    ensure!(
        (1..=36_000).contains(&ticks),
        "--ticks is 1 to 36000 (profile.capture's limit)"
    );
    ensure!(
        gameplay_timeout.is_finite() && gameplay_timeout > 0.0,
        "--timeout needs positive seconds"
    );
    let (mut c, v) = launch(r, req.clone(), &src)?;
    let id = v["id"].as_str().context("no id in the answer")?.to_string();
    eprintln!("lab: bench job {id} started");
    let measured = match bench_measure(&mut c, &id, ticks, gameplay_timeout) {
        Ok(BenchOutcome::Skipped(v)) => return print(&v),
        Ok(BenchOutcome::Captured(m)) => Ok(m),
        Err(e) => Err(e),
    };
    // Whatever happened, the game does not stay up.
    let _ = c.call(json!({"cmd": "stop", "id": id}));
    let ended = c.call(json!({"cmd": "wait", "id": id, "timeout": 60}));
    let m = match measured {
        Ok(m) => m,
        Err(e) => {
            let state = ended.ok().map(|v| v["job"]["state"].to_string());
            bail!(
                "bench run {id} failed: {e:#} (run state {}; h2 lab fetch {id} for its logs)",
                state.as_deref().unwrap_or("unknown")
            );
        }
    };
    let dest = fetch(r, &id, to)?;
    for (name, text) in [
        ("capture.json", &m.capture),
        ("render-stats.json", &m.render),
        ("profile-counters.json", &m.counters),
    ] {
        let mut file = transfer::TempFile::new(&dest)?;
        file.file.write_all(text.as_bytes())?;
        std::fs::rename(&file.path, dest.join(name))
            .with_context(|| format!("saving {}", dest.join(name).display()))?;
    }
    let tag = {
        let mut t: Vec<String> = req.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        t.extend(req.build.env.iter().map(|(k, v)| format!("{k}={v}")));
        t.join(" ")
    };
    let line = bench_summary(
        &serde_json::from_str(&m.capture).unwrap_or(Value::Null),
        req.level.as_deref().unwrap_or("?"),
        &tag,
    );
    println!("{line}");
    println!("{}", dest.display());
    Ok(())
}

/// The three answers a bench keeps, as the JSON `h2 agent` printed.
struct Measured {
    capture: String,
    render: String,
    counters: String,
}

enum BenchOutcome {
    Captured(Measured),
    Skipped(Value),
}

/// With the job started: waits for it to run, then for gameplay with
/// `map_ticks >= 30`, then reads the capture, `render.stats` and
/// `profile.counters`.
fn bench_measure(
    c: &mut Conn,
    id: &str,
    ticks: u64,
    gameplay_timeout: f64,
) -> Result<BenchOutcome> {
    // The build can take a long time; the game then has to boot.
    loop {
        let v = c.call(json!({"cmd": "wait", "id": id, "timeout": 0}))?;
        if v["finished"] == Value::Bool(true) {
            if v["job"]["state"] == "skipped" {
                return Ok(BenchOutcome::Skipped(v));
            }
            bail!(
                "the job ended before gameplay ({}{})",
                v["job"]["state"].as_str().unwrap_or("?"),
                v["job"]["error"]
                    .as_str()
                    .map_or(String::new(), |e| format!(": {e}"))
            );
        }
        if v["job"]["state"] == "running" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let ms = ((gameplay_timeout * 1000.0) as u64).to_string();
    let call = |c: &mut Conn, args: &[&str], timeout: f64| -> Result<String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let a = agent_request(c, id, &args, Some(timeout), None)?;
        ensure!(
            a.exit == 0,
            "h2 agent {} exited {}: {}",
            args.join(" "),
            a.exit,
            a.stderr.trim()
        );
        Ok(a.stdout)
    };
    let until = r#"{"phase":"gameplay","map_ticks":30}"#;
    // Two minutes more than the wait itself for the game's socket to open.
    call(c, &["wait", until, ms.as_str()], gameplay_timeout + 150.0)
        .context("waiting for gameplay")?;
    let capture = call(
        c,
        &["capture", ticks.to_string().as_str()],
        ticks as f64 / 5.0 + 90.0,
    )
    .context("capturing")?;
    let parsed: Value = serde_json::from_str(&capture)
        .with_context(|| format!("the capture answer is not JSON: {}", capture.trim()))?;
    ensure!(
        parsed["frames"].as_u64().unwrap_or(0) > 0,
        "the capture read zero frames: {}",
        capture.trim()
    );
    let render = call(c, &["render"], 60.0).context("render.stats")?;
    let counters = call(c, &["counters"], 60.0).context("profile.counters")?;
    Ok(BenchOutcome::Captured(Measured {
        capture,
        render,
        counters,
    }))
}

/// One line for a `profile.capture` answer: fps, busy-time percentiles and
/// each phase's mean ms per frame, biggest first (pacing left out).
pub fn bench_summary(cap: &Value, level: &str, tag: &str) -> String {
    let n = |v: &Value| v.as_f64().unwrap_or(f64::NAN);
    let mut phases: Vec<(&str, f64)> = cap["phases"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| k.as_str() != "pace")
        .map(|(k, v)| (k.as_str(), n(&v["mean"])))
        .collect();
    phases.sort_by(|a, b| b.1.total_cmp(&a.1));
    let phases: Vec<String> = phases.iter().map(|(k, m)| format!("{k} {m:.2}")).collect();
    let mut s = format!(
        "bench {level}{}{}: {:.1} fps, busy p50 {:.2} p95 {:.2} ms over {} frames ({} over {:.1} ms); phase mean ms: {}",
        if tag.is_empty() { "" } else { " " },
        tag,
        n(&cap["fps"]),
        n(&cap["busy_ms"]["p50"]),
        n(&cap["busy_ms"]["p95"]),
        cap["frames"].as_u64().unwrap_or(0),
        cap["over_budget"].as_u64().unwrap_or(0),
        n(&cap["budget_ms"]),
        phases.join(", ")
    );
    if cap["complete"] == Value::Bool(false) {
        s += " [capture incomplete: it ran out of time]";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_game_command_requires_a_label_before_discovering_the_host() {
        for name in [None, Some("  ")] {
            let mut args = vec!["--level".into(), "fixture".into()];
            if let Some(label) = name {
                args.extend(["--name".into(), label.into()]);
            }
            assert!(
                record(args.clone())
                    .unwrap_err()
                    .to_string()
                    .contains("--name")
            );
            args.extend(["--host".into(), "missing.invalid".into()]);
            for cmd in ["run", "bench"] {
                assert!(
                    run(cmd, args.clone())
                        .unwrap_err()
                        .to_string()
                        .contains("--name")
                );
            }
        }
    }

    #[test]
    fn own_data_parses_and_refuses_fresh_data() {
        let a = |extra: &[&str]| {
            let mut v: Vec<String> = ["--name", "Play", "--level", "x", "--serve"]
                .map(String::from)
                .into();
            v.extend(extra.iter().map(|s| s.to_string()));
            parse_run_args(v)
        };
        let (req, ..) = a(&["--own-data"]).unwrap();
        assert!(req.own_data && !req.fresh_data && req.serve);
        assert!(!a(&[]).unwrap().0.own_data);
        for extra in [["--own-data", "--fresh-data"], ["--fresh-data", "--own-data"]] {
            let e = a(&extra).unwrap_err().to_string();
            assert!(e.contains("--own-data") && e.contains("--fresh-data"), "{e}");
        }
    }

    #[test]
    fn the_bench_line_has_fps_busy_percentiles_and_phases_biggest_first() {
        let cap = json!({
            "frames": 600, "fps": 29.97, "budget_ms": 33.3, "over_budget": 4, "complete": true,
            "busy_ms": {"mean": 10.0, "p50": 9.5, "p95": 14.25},
            "phases": {
                "cpu": {"mean": 3.0}, "render": {"mean": 5.5},
                "pace": {"mean": 20.0}, "d3d": {"mean": 1.0}
            }
        });
        let s = bench_summary(&cap, "03a_oldmombasa", "H2_RENDER_SCALE=4");
        assert_eq!(
            s,
            "bench 03a_oldmombasa H2_RENDER_SCALE=4: 30.0 fps, busy p50 9.50 p95 14.25 ms over 600 frames (4 over 33.3 ms); phase mean ms: render 5.50, cpu 3.00, d3d 1.00"
        );
        assert!(!s.contains("pace"));
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn dedicated_flags_fill_the_dedicated_request() {
        let (req, src, wait, _) = parse_run_args(words(
            "--name Join --dedicated --host-bots --map lockout --variant slayer \
             --max-players 4 --base-port 31000 --rev master --env H2_FEATURES=x",
        ))
        .unwrap();
        assert!(!wait && src.rev.as_deref() == Some("master"));
        let d = req.dedicated.as_ref().unwrap();
        assert!(d.bots);
        assert_eq!(d.map.as_deref(), Some("lockout"));
        assert_eq!(d.variant.as_deref(), Some("slayer"));
        assert_eq!((d.max_players, d.base_port), (Some(4), Some(31000)));
        assert_eq!(req.env["H2_FEATURES"], "x");
        proto::validate_run(&req).unwrap();

        let (req, ..) = parse_run_args(words("--name Join --dedicated")).unwrap();
        assert_eq!(req.dedicated, Some(proto::Dedicated::default()));
        let (req, ..) = parse_run_args(words("--name Join --serve")).unwrap();
        assert!(req.dedicated.is_none());
    }

    #[test]
    fn dedicated_flags_need_dedicated_and_never_wait() {
        let err = |s: &str| match parse_run_args(words(s)) {
            Ok(_) => panic!("{s} parsed"),
            Err(e) => e.to_string(),
        };
        assert!(err("--name Join --map lockout").contains("add --dedicated"));
        assert!(err("--name Join --host-bots").contains("add --dedicated"));
        assert!(err("--name Join --dedicated --wait").contains("--wait"));
        assert!(err("--name Join --dedicated --max-players many").contains("--max-players"));
        assert!(err("--name Join --dedicated --base-port 70000").contains("--base-port"));
        assert!(err("--name Join --dedicated --map").contains("needs a value"));
    }

    #[test]
    fn an_incomplete_capture_says_so() {
        let s = bench_summary(&json!({"complete": false}), "x", "");
        assert!(
            s.ends_with("[capture incomplete: it ran out of time]"),
            "{s}"
        );
    }
}
