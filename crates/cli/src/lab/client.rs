//! The lab's client commands (`h2 lab status|run|...`), and `h2 lab record`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use super::proto::{self, BuildSpec, RunReq, sh_quote};
use super::{USAGE, default_socket};

/// Where the host is.
enum Target {
    /// The socket on this machine.
    Local(PathBuf),
    Ssh {
        host: String,
        /// `--socket` for the remote pipe, when not its default.
        socket: Option<String>,
        remote_h2: String,
    },
}

/// The remote h2 when neither --remote-h2 nor $H2_LAB_REMOTE_H2 says: the
/// link `h2 lab host` keeps in its default root, relative to the remote
/// home (where ssh starts).
const REMOTE_H2: &str = "Library/Caches/h2recomp/lab/h2";

struct Remote {
    target: Target,
    /// The ssh command: $H2_LAB_SSH, else ssh in batch mode.
    ssh: Vec<String>,
    ssh_overridden: bool,
}

impl Remote {
    /// A command line run on the host's machine: through ssh, or `sh -c`.
    fn shell(&self, line: &str) -> Command {
        match &self.target {
            Target::Local(_) => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(line);
                c
            }
            Target::Ssh { host, .. } => {
                let mut c = Command::new(&self.ssh[0]);
                c.args(&self.ssh[1..]).arg(host).arg(line);
                c
            }
        }
    }

    fn connect(&self) -> Result<Conn> {
        match &self.target {
            Target::Local(p) => {
                let s = std::os::unix::net::UnixStream::connect(p).with_context(|| {
                    format!("connecting to {}: is `h2 lab host` running?", p.display())
                })?;
                Ok(Conn {
                    r: BufReader::new(Box::new(s.try_clone()?)),
                    w: Some(Box::new(s)),
                    child: None,
                })
            }
            Target::Ssh {
                socket, remote_h2, ..
            } => {
                let mut line = format!("{} lab pipe", sh_quote(remote_h2));
                if let Some(s) = socket {
                    line += &format!(" --socket {}", sh_quote(s));
                }
                let mut child = self
                    .shell(&line)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .with_context(|| format!("starting {}", self.ssh[0]))?;
                let r = child.stdout.take().unwrap();
                let w = child.stdin.take().unwrap();
                Ok(Conn {
                    r: BufReader::new(Box::new(r)),
                    w: Some(Box::new(w)),
                    child: Some(child),
                })
            }
        }
    }

    /// Where `git push` sends to: the checkout's path, or `HOST:path`.
    fn git_url(&self, repo: &str) -> String {
        match &self.target {
            Target::Local(_) => repo.to_string(),
            Target::Ssh { host, .. } => format!("{host}:{repo}"),
        }
    }
}

struct Conn {
    r: BufReader<Box<dyn Read + Send>>,
    w: Option<Box<dyn Write + Send>>,
    child: Option<Child>,
}

impl Conn {
    fn call(&mut self, req: Value) -> Result<Value> {
        let w = self.w.as_mut().unwrap();
        writeln!(w, "{req}")?;
        w.flush()?;
        let mut line = String::new();
        if self.r.read_line(&mut line)? == 0 {
            bail!("the lab host closed the connection (is `h2 lab host` running there?)");
        }
        let v: Value =
            serde_json::from_str(&line).with_context(|| format!("bad answer {line:?}"))?;
        if v["ok"] != Value::Bool(true) {
            bail!("{}", v["error"].as_str().unwrap_or("the host refused"));
        }
        Ok(v)
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.w = None;
        if let Some(mut c) = self.child.take() {
            let _ = c.wait();
        }
    }
}

/// Options every client command takes; the rest stay for the command.
fn remote(args: Vec<String>) -> Result<(Remote, Vec<String>)> {
    let mut host = std::env::var("H2_LAB_HOST").ok().filter(|h| !h.is_empty());
    let mut socket = None;
    let mut remote_h2 = std::env::var("H2_LAB_REMOTE_H2")
        .ok()
        .filter(|h| !h.is_empty());
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                rest.push(a);
                rest.extend(it.by_ref());
            }
            "--host" => host = Some(it.next().context("--host needs [user@]HOST or local")?),
            "--socket" => socket = Some(it.next().context("--socket needs a path")?),
            "--remote-h2" => remote_h2 = Some(it.next().context("--remote-h2 needs a path")?),
            _ => rest.push(a),
        }
    }
    let host = host.context("which host? --host [user@]HOST (or local), or set H2_LAB_HOST")?;
    ensure!(!host.starts_with('-'), "--host {host:?} is not a host name");
    let (ssh, ssh_overridden) = match std::env::var("H2_LAB_SSH")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(s) => (s.split_whitespace().map(String::from).collect(), true),
        None => (
            ["ssh", "-o", "BatchMode=yes"].map(String::from).to_vec(),
            false,
        ),
    };
    let target = if host == "local" {
        Target::Local(socket.map_or_else(default_socket, PathBuf::from))
    } else {
        Target::Ssh {
            host,
            socket,
            remote_h2: remote_h2.unwrap_or_else(|| REMOTE_H2.into()),
        }
    };
    Ok((
        Remote {
            target,
            ssh,
            ssh_overridden,
        },
        rest,
    ))
}

pub fn run(cmd: &str, args: Vec<String>) -> Result<()> {
    let (r, args) = remote(args)?;
    match cmd {
        "status" => {
            no_more(&args)?;
            print(&r.connect()?.call(json!({"cmd": "status"}))?)
        }
        "run" => run_cmd(&r, args),
        "agent" => agent_cmd(&r, args),
        "bench" => bench_cmd(&r, args),
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
        "runs" => runs_cmd(&r, args),
        "fetch" => {
            let mut id = None;
            let mut to = None;
            let mut it = args.into_iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--to" => {
                        to = Some(PathBuf::from(it.next().context("--to needs a directory")?))
                    }
                    _ if id.is_none() && !a.starts_with('-') => id = Some(a),
                    _ => bail!("unexpected {a}\n\n{USAGE}"),
                }
            }
            let id = id.context("usage: h2 lab fetch <id> [--to DIR]")?;
            let root = status(&r)?["root"]
                .as_str()
                .context("status names no root")?
                .to_string();
            let dest = fetch(&r, &root, &id, to)?;
            println!("{}", dest.display());
            Ok(())
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
            let repo = status(&r)?["repo"]
                .as_str()
                .context("status names no checkout")?
                .to_string();
            for (b, sha) in sync(&r, &repo, &branches)? {
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

fn status(r: &Remote) -> Result<Value> {
    r.connect()?.call(json!({"cmd": "status"}))
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

/// Exit status for `wait`: 0 when the job finished well, 1 when it failed
/// or was stopped, 3 when it is still going.
fn finished_ok(v: &Value) -> Result<()> {
    if v["finished"] != Value::Bool(true) {
        std::process::exit(3);
    }
    if v["job"]["state"] != "done" {
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
#[derive(Default)]
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

    /// The rev for the request: none for --current; --rev as given (it must
    /// be on the host already); else the branch, pushed first and named by
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
        let repo = status(r)?["repo"]
            .as_str()
            .context("status names no checkout")?
            .to_string();
        let pushed = sync(r, &repo, std::slice::from_ref(&branch))?;
        Ok(Some(pushed[0].1.clone()))
    }
}

fn run_cmd(r: &Remote, args: Vec<String>) -> Result<()> {
    let mut req = RunReq::default();
    let mut src = Source::default();
    let mut wait = false;
    let mut to = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if src.take(&a, &mut it)? {
            continue;
        }
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--level" => req.level = Some(val()?),
            "--difficulty" => req.difficulty = Some(val()?),
            "--frames" => req.frames = Some(val()?.parse().context("--frames needs a count")?),
            "--stats-from" => {
                req.stats_from = Some(val()?.parse().context("--stats-from needs a present")?)
            }
            "--no-stats" => req.stats = Some(false),
            "--replay" => req.replay = Some(val()?),
            "--record" => req.record = Some(val()?),
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
            "--note" => req.note = Some(val()?),
            "--serve" => req.serve = true,
            "--wait" => wait = true,
            "--to" => to = Some(PathBuf::from(val()?)),
            "--" => req.args.extend(it.by_ref()),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    ensure!(
        !(wait && req.serve),
        "--serve runs until `h2 lab stop`; --wait would wait forever (h2 lab bench does the whole loop)"
    );
    let (mut c, v) = launch(r, req, &src)?;
    let id = v["id"].as_str().context("no id in the answer")?.to_string();
    if !wait {
        print(&v)?;
        return Ok(());
    }
    eprintln!("lab: job {id} started");
    wait_and_fetch(r, &mut c, &id, to)
}

/// Validates `req`, resolves and pushes its build, and starts it. Returns
/// the connection and the host's answer (with the job's `id`).
fn launch(r: &Remote, mut req: RunReq, src: &Source) -> Result<(Conn, Value)> {
    if req.perf && !req.build.features.iter().any(|f| f == "perf") && !src.current {
        req.build.features.push("perf".into());
    }
    // Refuse what the host would before pushing anything.
    proto::validate_run(&RunReq {
        build: BuildSpec {
            rev: Some("x".into()),
            ..req.build.clone()
        },
        ..req.clone()
    })?;
    req.build.rev = src.resolve(r)?;
    proto::validate_run(&req)?;
    let mut c = r.connect()?;
    let mut body = serde_json::to_value(&req)?;
    body["cmd"] = "run".into();
    let v = c.call(body)?;
    Ok((c, v))
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
    spec.rev = src.resolve(r)?;
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
    let root = c.call(json!({"cmd": "status"}))?["root"]
        .as_str()
        .context("status names no root")?
        .to_string();
    let dest = fetch(r, &root, id, to)?;
    print(&v)?;
    println!("{}", dest.display());
    finished_ok(&v)
}

/// Copies the host's `runs/<id>` into `to` (default `$RUSH_KEEP/lab`, else
/// `./lab`) as `<to>/<id>`, through tar over the same ssh.
fn fetch(r: &Remote, root: &str, id: &str, to: Option<PathBuf>) -> Result<PathBuf> {
    proto::check_name("id", id)?;
    let to = to.unwrap_or_else(
        || match std::env::var_os("RUSH_KEEP").filter(|v| !v.is_empty()) {
            Some(k) => PathBuf::from(k).join("lab"),
            None => PathBuf::from("lab"),
        },
    );
    std::fs::create_dir_all(&to).with_context(|| format!("creating {}", to.display()))?;
    let runs = format!("{}/runs", root.trim_end_matches('/'));
    let mut src = r
        .shell(&format!(
            "tar -C {} -cf - {}",
            sh_quote(&runs),
            sh_quote(id)
        ))
        .stdout(Stdio::piped())
        .spawn()
        .context("starting the remote tar")?;
    let st = Command::new("tar")
        .arg("-C")
        .arg(&to)
        .arg("-xf")
        .arg("-")
        .stdin(src.stdout.take().unwrap())
        .status()
        .context("running tar")?;
    let sst = src.wait()?;
    ensure!(
        sst.success() && st.success(),
        "fetching runs/{id} failed (remote tar {sst}, local tar {st})"
    );
    Ok(to.join(id))
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
            "{} {} {} {} {} {} {}",
            s("id"),
            s("kind"),
            s("state"),
            exit,
            dur,
            commit,
            s("level")
        );
        if let Some(e) = j["error"].as_str() {
            println!("    error: {e}");
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

/// Pushes each local branch to the host's checkout as `lab/<branch>` (forced:
/// they are the lab's copies), over the lab's ssh. Returns each branch's
/// commit. Uncommitted changes here are not in it; says so.
fn sync(r: &Remote, repo: &str, branches: &[String]) -> Result<Vec<(String, String)>> {
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()?;
    if !dirty.stdout.is_empty() {
        eprintln!("lab: this checkout has uncommitted changes; the host builds commits only");
    }
    let mut out = Vec::new();
    let mut push = Command::new("git");
    push.args(["push", "--force", "--quiet"])
        .arg(r.git_url(repo));
    for b in branches {
        proto::check_rev(b)?;
        let o = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("refs/heads/{b}"))
            .output()?;
        ensure!(o.status.success(), "no local branch {b}");
        let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
        push.arg(format!("refs/heads/{b}:refs/heads/lab/{b}"));
        out.push((b.clone(), sha));
    }
    if r.ssh_overridden {
        push.env("GIT_SSH_COMMAND", r.ssh.join(" "))
            .env("GIT_SSH_VARIANT", "simple");
    }
    let st = push.status().context("running git push")?;
    ensure!(st.success(), "git push to the lab host failed");
    Ok(out)
}

/// `h2 lab record`: the user plays; their input goes to
/// `recordings/<name>`. Through the host when it is up (so the agent sees
/// the run), else straight from here into the default lab root.
pub fn record(args: Vec<String>) -> Result<()> {
    let mut req = RunReq {
        stats: Some(false),
        ..RunReq::default()
    };
    let mut name = None;
    let mut socket = None;
    let mut host_args = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--level" => req.level = Some(val()?),
            "--difficulty" => req.difficulty = Some(val()?),
            "--name" => name = Some(val()?),
            "--socket" => socket = Some(PathBuf::from(val()?)),
            "--root" | "--repo" | "--disc" | "--h2" | "--out" | "--data" => {
                host_args.push(a.clone());
                host_args.push(val()?);
            }
            "--" => req.args.extend(it.by_ref()),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let level = req
        .level
        .clone()
        .context("h2 lab record needs --level MAP")?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = name.unwrap_or_else(|| format!("{level}-{secs}"));
    req.record = Some(name.clone());
    proto::validate_run(&req)?;
    let socket = socket.unwrap_or_else(default_socket);
    if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket) {
        let mut c = Conn {
            r: BufReader::new(Box::new(s.try_clone()?)),
            w: Some(Box::new(s)),
            child: None,
        };
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
    let measured = bench_measure(&mut c, &id, ticks, gameplay_timeout);
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
    let root = status(r)?["root"]
        .as_str()
        .context("status names no root")?
        .to_string();
    let dest = fetch(r, &root, &id, to)?;
    for (name, text) in [
        ("capture.json", &m.capture),
        ("render-stats.json", &m.render),
        ("profile-counters.json", &m.counters),
    ] {
        std::fs::write(dest.join(name), text)
            .with_context(|| format!("writing {}", dest.join(name).display()))?;
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

/// With the job started: waits for it to run, then for gameplay with
/// `map_ticks >= 30`, then reads the capture, `render.stats` and
/// `profile.counters`.
fn bench_measure(c: &mut Conn, id: &str, ticks: u64, gameplay_timeout: f64) -> Result<Measured> {
    // The build can take a long time; the game then has to boot.
    loop {
        let v = c.call(json!({"cmd": "wait", "id": id, "timeout": 0}))?;
        if v["finished"] == Value::Bool(true) {
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
    let render = call(c, &["render"], 60.0).context("render.stats")?;
    let counters = call(c, &["counters"], 60.0).context("profile.counters")?;
    Ok(Measured {
        capture,
        render,
        counters,
    })
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

    #[test]
    fn an_incomplete_capture_says_so() {
        let s = bench_summary(&json!({"complete": false}), "x", "");
        assert!(
            s.ends_with("[capture incomplete: it ran out of time]"),
            "{s}"
        );
    }
}
