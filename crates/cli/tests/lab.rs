//! `h2 lab` end to end on this machine with a fake game: a host on a
//! temporary socket, runs through `--host local` and through a stand-in for
//! ssh (`H2_LAB_SSH`), builds of a branch from a scratch git checkout with a
//! fake cargo, and the image cache. Nothing here builds or runs the game.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A fake `h2`: `build` makes an image directory; `play` logs its
/// arguments and environment, writes frames to the CSV one per 50 ms until
/// H2_BOOT_STOP_AT_PRESENT, writes the input recording, prints a frame stats
/// report, and exits 143 on SIGTERM.
const FAKE_H2: &str = r#"#!/bin/sh
cmd=$1; shift
if [ "$cmd" = build ]; then
  out=
  while [ $# -gt 0 ]; do [ "$1" = --out ] && out=$2; shift; done
  mkdir -p "$out" && echo image > "$out/libh2game.dylib"
  echo "fake build opt=$H2_OPT objcache=$H2_OBJ_CACHE"
  exit 0
fi
if [ "$cmd" = agent ]; then
  # `h2 agent ARGS --socket S --timeout T`: answers like a game would when
  # the socket is there. The host takes the exit code and the output as is.
  sock=; args=
  while [ $# -gt 0 ]; do
    case "$1" in --socket) sock=$2; shift;; --timeout) shift;; *) args="$args|$1";; esac
    shift
  done
  [ -S "$sock" ] || { echo "no socket at $sock" >&2; exit 1; }
  [ -z "$H2_AGENT$H2_XBOX" ] || { echo "H2_* leaked into the agent call" >&2; exit 1; }
  IFS='|'; set -- $args; shift; unset IFS
  case "$1" in
    wait) echo '{"at":{"phase":"gameplay","map_ticks":31}}';;
    capture) echo "{\"frames\":$2,\"fps\":24.0,\"budget_ms\":33.3,\"over_budget\":1,\"complete\":true,\"busy_ms\":{\"p50\":8.0,\"p95\":12.5},\"phases\":{\"cpu\":{\"mean\":2.0},\"render\":{\"mean\":4.0},\"pace\":{\"mean\":9.0}},\"csv\":\"a,b\\n1,2\\n\"}";;
    render) echo '{"interp":0}';;
    counters) echo '{"draws":100}';;
    fail) echo '{"code":3,"message":"nope"}' >&2; exit 2;;
    -) while read -r line; do echo "script: $line"; done;;
    *) echo "{\"method\":\"$1\",\"params\":\"$2\"}";;
  esac
  exit 0
fi
echo "fake play $*"
echo "env level=$H2_LEVEL opt=$H2_OPT replay=$H2_INPUT_REPLAY record=$H2_INPUT_RECORD profile=$H2_PROFILE from=$H2_FRAME_STATS_FROM home=$H2_LEAK scale=$H2_RENDER_SCALE agent=$H2_AGENT"
trap 'echo "got TERM"; exit 143' TERM
n=${H2_BOOT_STOP_AT_PRESENT:-${FAKE_FRAMES:-1000000}}
[ -n "$H2_FRAME_STATS_CSV" ] && echo "frame,total_us" > "$H2_FRAME_STATS_CSV"
[ -n "$H2_INPUT_RECORD" ] && echo "pad reads" > "$H2_INPUT_RECORD"
[ -n "$H2_PROFILE" ] && mkdir -p "$H2_PROFILE" && echo "folded" > "$H2_PROFILE/stacks.folded"
i=0
while [ $i -lt $n ]; do
  i=$((i+1))
  [ -n "$H2_FRAME_STATS_CSV" ] && echo "$i,33000" >> "$H2_FRAME_STATS_CSV"
  sleep 0.05 & wait $!
done
echo "frame stats: $n frames from 0, 30.00 fps; frame ms p50 33.0 p90 33.0 p99 33.0 max 33.0; over 33.3 ms: 0"
echo "  worst frames:"
echo "    #1 33.0 ms: cpu 10.0"
echo "outcome: stopped at present $n"
"#;

/// Stands in for ssh: drops the host and runs the command with sh, as the
/// remote login shell would.
const FAKE_SSH: &str = "#!/bin/sh\nshift\nexec sh -c \"$*\"\n";

fn scratch(name: &str) -> PathBuf {
    // Short: Unix socket paths are limited to about 100 bytes.
    let p = std::env::temp_dir().join(format!("h2lab{name}{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

fn script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=lab",
            "-c",
            "user.email=lab@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Lab {
    dir: PathBuf,
    socket: PathBuf,
    root: PathBuf,
    /// The laptop's checkout.
    repo: PathBuf,
    /// This machine's checkout, a clone of it.
    dev: PathBuf,
    cargo_log: PathBuf,
    _host: Host,
}

impl Lab {
    fn start() -> Lab {
        Lab::named("a")
    }

    fn named(name: &str) -> Lab {
        let dir = scratch(name);
        let fake_h2 = dir.join("fake-h2");
        script(&fake_h2, FAKE_H2);
        script(&dir.join("fake-ssh"), FAKE_SSH);
        let cargo_log = dir.join("cargo.log");
        // A fake cargo: logs its call and "builds" h2 as the fake.
        script(
            &dir.join("fake-cargo"),
            &format!(
                "#!/bin/sh\necho \"$* in $(pwd)\" >> '{}'\nmkdir -p \"$CARGO_TARGET_DIR/release\"\ncp '{}' \"$CARGO_TARGET_DIR/release/h2\"\n",
                cargo_log.display(),
                fake_h2.display()
            ),
        );
        let disc = dir.join("halo2.iso");
        fs::write(&disc, b"not a disc").unwrap();
        let repo = dir.join("laptop");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "master"]);
        fs::write(repo.join("README"), "lab\n").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "-q", "-m", "one"]);
        let dev = dir.join("dev");
        let o = Command::new("git")
            .arg("clone")
            .arg("-q")
            .arg(&repo)
            .arg(&dev)
            .output()
            .unwrap();
        assert!(o.status.success());
        let socket = dir.join("s");
        let root = dir.join("root");
        let child = Command::new(env!("CARGO_BIN_EXE_h2"))
            .args(["lab", "host", "--root"])
            .arg(&root)
            .arg("--socket")
            .arg(&socket)
            .arg("--repo")
            .arg(&repo)
            .arg("--disc")
            .arg(&disc)
            .arg("--h2")
            .arg(&fake_h2)
            .arg("--cargo")
            .arg(dir.join("fake-cargo"))
            // The host's own H2_* never reach a run.
            .env("H2_LEAK", "leaked")
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let host = Host(child);
        let t = Instant::now();
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            assert!(t.elapsed() < Duration::from_secs(20), "host never listened");
            std::thread::sleep(Duration::from_millis(50));
        }
        let mode = fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        Lab {
            dir,
            socket,
            root,
            repo,
            dev,
            cargo_log,
            _host: host,
        }
    }

    /// `h2 lab <args> --host local --socket S`, from the dev checkout.
    fn local(&self, args: &[&str]) -> Output {
        self.h2(
            args,
            &["--host", "local", "--socket", self.socket.to_str().unwrap()],
        )
    }

    /// The same through the fake ssh and `h2 lab pipe`.
    fn ssh(&self, args: &[&str]) -> Output {
        self.h2(
            args,
            &[
                "--host",
                "laptop",
                "--socket",
                self.socket.to_str().unwrap(),
                "--remote-h2",
                env!("CARGO_BIN_EXE_h2"),
            ],
        )
    }

    fn h2(&self, args: &[&str], target: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_h2"));
        // The target before the command's own arguments, which may end in `-- play flags`.
        c.arg("lab")
            .arg(args[0])
            .args(target)
            .args(&args[1..])
            .current_dir(&self.dev);
        c.env("H2_LAB_SSH", self.dir.join("fake-ssh"))
            .env_remove("H2_LAB_HOST")
            .env_remove("RUSH_KEEP");
        c.output().unwrap()
    }
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    assert!(
        !o.status.success(),
        "should fail: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The JSON object at the start of a command's output.
fn json(s: &str) -> Value {
    serde_json::Deserializer::from_str(s)
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap()
}

#[test]
fn a_lab_host_runs_waits_stops_fetches_and_builds_branches() {
    let lab = Lab::start();

    // status, locally and through ssh + pipe.
    let s = json(&ok(&lab.local(&["status"])));
    assert_eq!(s["version"], 1);
    assert!(s["active"].is_null());
    let s2 = json(&ok(&lab.ssh(&["status"])));
    assert_eq!(s2["repo"], s["repo"]);

    // A run of the host's own image, waited for and fetched.
    let to = lab.dir.join("fetched");
    let out = ok(&lab.local(&[
        "run",
        "--current",
        "--level",
        "03a_oldmombasa",
        "--difficulty",
        "heroic",
        "--frames",
        "3",
        "--stats-from",
        "1",
        "--wait",
        "--to",
        to.to_str().unwrap(),
        "--",
        "--no-audio",
        "--fps",
        "60",
    ]));
    let v = json(&out);
    let id = v["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["job"]["state"], "done");
    assert_eq!(v["job"]["exit_code"], 0);
    assert!(
        v["job"]["summary"][0]
            .as_str()
            .unwrap()
            .starts_with("frame stats: 3 frames")
    );
    let run = to.join(&id);
    assert!(out.trim_end().ends_with(run.to_str().unwrap()), "{out}");
    let csv = fs::read_to_string(run.join("frames.csv")).unwrap();
    assert_eq!(csv.lines().count(), 4, "{csv}");
    let log = fs::read_to_string(run.join("run.log")).unwrap();
    assert!(
        log.contains("--level 03a_oldmombasa --difficulty heroic --no-audio --fps 60"),
        "{log}"
    );
    assert!(log.contains("from=1"), "{log}");
    assert!(
        !log.contains("leaked"),
        "the host's own H2_* reached the run: {log}"
    );
    assert!(
        fs::read_to_string(run.join("meta.json"))
            .unwrap()
            .contains("\"done\"")
    );

    // runs lists it with its summary.
    let runs = ok(&lab.local(&["runs"]));
    assert!(runs.contains(&format!("{id} run done exit 0")), "{runs}");
    assert!(runs.contains("frame stats: 3 frames"), "{runs}");

    // Refusals: host-owned variables, paths out of the root, unknown flags.
    assert!(
        err(&lab.local(&["run", "--current", "--env", "H2_FRAME_STATS_CSV=/tmp/x"]))
            .contains("not a run variable")
    );
    assert!(err(&lab.local(&["run", "--current", "--replay", "../../x"])).contains("must be 1-64"));
    assert!(
        err(&lab.local(&["run", "--current", "--", "--out", "/tmp"]))
            .contains("not one the lab passes")
    );
    assert!(
        err(&lab.local(&["run", "--current", "--replay", "nothing"]))
            .contains("no recording nothing")
    );

    // One at a time: a long run, a refused second, stop, then it's stopped.
    let v = json(&ok(&lab.local(&[
        "run",
        "--current",
        "--level",
        "ascension",
    ])));
    let long = v["id"].as_str().unwrap().to_string();
    let t = Instant::now();
    loop {
        let s = json(&ok(&lab.local(&["status"])));
        if s["active"]["state"] == "running" {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "never running: {s}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let busy = err(&lab.local(&["run", "--current", "--level", "ascension"]));
    assert!(
        busy.contains(&format!("busy: job {long} is running")),
        "{busy}"
    );
    // A short wait times out (exit 3) while it runs.
    let w = lab.local(&["wait", &long, "--timeout", "0.2"]);
    assert_eq!(w.status.code(), Some(3));
    ok(&lab.local(&["stop", &long]));
    let w = lab.local(&["wait", &long, "--timeout", "10"]);
    assert_eq!(w.status.code(), Some(1), "a stopped job isn't done");
    assert_eq!(
        json(&String::from_utf8_lossy(&w.stdout))["job"]["state"],
        "stopped"
    );
    let log = fs::read_to_string(lab.root.join("runs").join(&long).join("run.log")).unwrap();
    assert!(log.contains("got TERM"), "{log}");
    assert!(err(&lab.local(&["stop", &long])).contains("not running (stopped)"));

    // Record, then replay the recording (level from its manifest), through ssh.
    let v = json(&ok(&lab.ssh(&[
        "run",
        "--current",
        "--level",
        "03a_oldmombasa",
        "--record",
        "fight",
        "--frames",
        "2",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ])));
    assert_eq!(v["job"]["state"], "done");
    let recs = json(&ok(&lab.ssh(&["recordings"])));
    let r = &recs["recordings"][0];
    assert_eq!(r["name"], "fight");
    assert_eq!(r["level"], "03a_oldmombasa");
    assert!(r["snapshot"].is_null());
    assert!(lab.root.join("recordings/fight/input.bin").is_file());
    let v = json(&ok(&lab.ssh(&[
        "run",
        "--current",
        "--replay",
        "fight",
        "--frames",
        "2",
        "--perf",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ])));
    let rid = v["job"]["id"].as_str().unwrap();
    let log = fs::read_to_string(to.join(rid).join("run.log")).unwrap();
    assert!(
        log.contains("level=") && log.contains("--level 03a_oldmombasa"),
        "{log}"
    );
    assert!(log.contains("recordings/fight/input.bin"), "{log}");
    assert!(
        to.join(rid).join("profile/stacks.folded").is_file(),
        "the profile comes back with the run"
    );
    // Recording names are not reused.
    assert!(
        err(&lab.local(&["run", "--current", "--level", "x", "--record", "fight"]))
            .contains("exists")
    );

    // put: an uploaded recording, through ssh.
    let file = lab.dir.join("mine.bin");
    fs::write(&file, b"\x00\x01recorded elsewhere\xff").unwrap();
    let v = json(&ok(&lab.ssh(&[
        "put",
        file.to_str().unwrap(),
        "--as",
        "mine",
        "--level",
        "ascension",
    ])));
    assert_eq!(v["bytes"], 21);
    assert_eq!(
        fs::read(lab.root.join("recordings/mine/input.bin")).unwrap(),
        fs::read(&file).unwrap()
    );

    // A branch: synced over (fake) ssh into lab/<branch>, built in a
    // worktree with its build variables, run; the second run reuses the
    // image.
    git(&lab.dev, &["checkout", "-q", "-b", "perf-work"]);
    fs::write(lab.dev.join("README"), "faster\n").unwrap();
    git(&lab.dev, &["commit", "-q", "-am", "two"]);
    let head = git(&lab.dev, &["rev-parse", "HEAD"]);
    let v = json(&ok(&lab.ssh(&[
        "run",
        "--level",
        "ascension",
        "--frames",
        "2",
        "--build-env",
        "H2_OPT=2",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ])));
    assert_eq!(v["job"]["state"], "done", "{v}");
    assert_eq!(v["job"]["commit"], head.as_str());
    assert_eq!(v["job"]["cached"], false);
    assert_eq!(
        git(&lab.repo, &["rev-parse", "refs/heads/lab/perf-work"]),
        head
    );
    assert_eq!(
        git(&lab.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "master",
        "the laptop's checkout is untouched"
    );
    let bid = v["job"]["id"].as_str().unwrap();
    let build_log = fs::read_to_string(to.join(bid).join("build.log")).unwrap();
    assert!(
        build_log.contains("fake build opt=2 objcache="),
        "{build_log}"
    );
    assert!(
        fs::read_to_string(to.join(bid).join("run.log"))
            .unwrap()
            .contains("opt=2"),
        "the build env is set on the run too"
    );
    assert!(lab.root.join("src").join(&head).join("README").is_file());
    let cargo = fs::read_to_string(&lab.cargo_log).unwrap();
    assert_eq!(cargo.lines().count(), 1);
    assert!(cargo.contains("build --release -p cli --bin h2"), "{cargo}");
    let v = json(&ok(&lab.ssh(&[
        "run",
        "--level",
        "ascension",
        "--frames",
        "1",
        "--build-env",
        "H2_OPT=2",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ])));
    assert_eq!(v["job"]["cached"], true);
    assert_eq!(
        fs::read_to_string(&lab.cargo_log).unwrap().lines().count(),
        1,
        "cached image, no build"
    );
    // Another build environment is another image.
    let v = json(&ok(&lab.local(&[
        "build",
        "--branch",
        "perf-work",
        "--no-sync",
        "--feature",
        "perf",
        "--wait",
    ])));
    assert_eq!(v["job"]["state"], "done", "{v}");
    assert_eq!(v["job"]["cached"], false);
    assert!(
        fs::read_to_string(&lab.cargo_log)
            .unwrap()
            .contains("--features perf")
    );
    assert!(
        err(&lab.local(&["build", "--branch", "nope", "--no-sync"])).contains("is not a commit")
    );

    // Host-side refusals of H2_PGO profiles that aren't there.
    assert!(
        err(&lab.local(&[
            "build",
            "--branch",
            "perf-work",
            "--no-sync",
            "--build-env",
            "H2_PGO=use:none"
        ]))
        .contains("no PGO profile")
    );
}

#[test]
fn record_without_a_host_runs_directly() {
    let dir = scratch("d");
    fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("fake-h2");
    script(&fake, FAKE_H2);
    let disc = dir.join("halo2.iso");
    fs::write(&disc, b"x").unwrap();
    let root = dir.join("root");
    let o = Command::new(env!("CARGO_BIN_EXE_h2"))
        .args([
            "lab",
            "record",
            "--level",
            "03a_oldmombasa",
            "--name",
            "solo",
            "--socket",
        ])
        .arg(dir.join("none.sock"))
        .arg("--root")
        .arg(&root)
        .arg("--h2")
        .arg(&fake)
        .arg("--disc")
        .arg(&disc)
        .env("FAKE_FRAMES", "2")
        .env("H2_LEAK", "leaked")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let rec = root.join("recordings/solo");
    // The game's own output comes first, on this terminal.
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).lines().last(),
        rec.to_str()
    );
    assert!(!String::from_utf8_lossy(&o.stdout).contains("leaked"));
    assert!(rec.join("input.bin").is_file());
    let m: Value = serde_json::from_slice(&fs::read(rec.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["level"], "03a_oldmombasa");
    assert_eq!(m["input"], "input.bin");
    assert!(m["snapshot"].is_null());
    let meta = fs::read_to_string(root.join("runs/0001/meta.json")).unwrap();
    assert!(meta.contains("\"done\""), "{meta}");
}

/// Binds a listening socket where the host said run `id`'s game would
/// (its meta.json, once the run has one), the way a game opens its agent
/// socket; the fake `h2 agent` only checks that it is there. Returns the
/// path and the listener to keep alive.
fn open_agent_socket(root: &Path, id: &str) -> (PathBuf, std::os::unix::net::UnixListener) {
    let meta = root.join("runs").join(id).join("meta.json");
    let t = Instant::now();
    loop {
        if let Ok(b) = fs::read(&meta) {
            let v: Value = serde_json::from_slice(&b).unwrap();
            if let Some(p) = v["agent_socket"].as_str() {
                let p = PathBuf::from(p);
                assert!(p.as_os_str().len() <= 100, "{p:?} is too long for SUN_LEN");
                let l = std::os::unix::net::UnixListener::bind(&p).unwrap();
                return (p, l);
            }
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "run {id} never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn lab_agent_forwards_calls_to_a_serve_runs_game_and_returns_exit_codes() {
    let lab = Lab::named("g");
    let v = json(&ok(&lab.local(&[
        "run",
        "--current",
        "--level",
        "ascension",
        "--serve",
    ])));
    let id = v["id"].as_str().unwrap().to_string();
    let (sock, _listener) = open_agent_socket(&lab.root, &id);
    // The game got the same path in H2_AGENT; no frame limit was set.
    let t = Instant::now();
    let log = lab.root.join("runs").join(&id).join("run.log");
    while !fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("agent=")
    {
        assert!(t.elapsed() < Duration::from_secs(10), "no run.log");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = fs::read_to_string(&log).unwrap();
    assert!(
        text.contains(&format!("agent={}", sock.display())),
        "{text}"
    );
    assert!(
        sock.starts_with(lab.root.join("a")) || sock.starts_with("/tmp"),
        "{sock:?}"
    );

    // A short form, a method with JSON, and the same through ssh + pipe.
    let out = json(&ok(&lab.local(&[
        "agent",
        &id,
        "capture",
        "12",
        "--timeout",
        "20",
    ])));
    assert_eq!(out["frames"], 12);
    assert_eq!(out["busy_ms"]["p95"], 12.5);
    let out = json(&ok(&lab.ssh(&[
        "agent",
        &id,
        "session.wait",
        r#"{"until":{"phase":"gameplay"}}"#,
    ])));
    assert_eq!(out["method"], "session.wait");
    assert_eq!(out["params"], r#"{"until":{"phase":"gameplay"}}"#);

    // The exit code and stderr of the call come back as they were.
    let f = lab.ssh(&["agent", &id, "fail"]);
    assert_eq!(f.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&f.stderr).contains(r#""code":3"#));
    assert!(f.stdout.is_empty());

    // A script on stdin.
    let mut c = Command::new(env!("CARGO_BIN_EXE_h2"));
    c.args(["lab", "agent", "--host", "local", "--socket"])
        .arg(&lab.socket)
        .args([id.as_str(), "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"status\ncount\n")
            .unwrap();
    }
    let o = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "script: status\nscript: count\n"
    );

    // What isn't forwarded.
    assert!(err(&lab.local(&["agent", &id, "flight", "show", "x"])).contains("reads files"));
    assert!(err(&lab.local(&["agent", "9999", "status"])).contains("no job 9999"));
    assert!(
        err(&lab.local(&["run", "--current", "--serve", "--frames", "5"])).contains("never stops")
    );

    // Stopping the run ends it, removes the socket, and calls are refused.
    ok(&lab.local(&["stop", &id]));
    let w = lab.local(&["wait", &id, "--timeout", "10"]);
    assert_eq!(w.status.code(), Some(1));
    assert!(!sock.exists(), "the host removes the socket after the run");
    assert!(err(&lab.local(&["agent", &id, "status"])).contains("is stopped"));
}

#[test]
fn lab_bench_measures_a_level_and_saves_the_answers_for_an_ab() {
    let lab = Lab::named("b");
    let to = lab.dir.join("fetched");
    let mut dirs = Vec::new();
    for (n, env, scale) in [(1, "H2_FPS=60", "4"), (2, "H2_FPS=30", "2")] {
        let id = format!("{n:04}");
        let root = lab.root.clone();
        let opener = {
            let id = id.clone();
            std::thread::spawn(move || open_agent_socket(&root, &id))
        };
        let out = ok(&lab.local(&[
            "bench",
            "--current",
            "--level",
            "03a_oldmombasa",
            "--scale",
            scale,
            "--ticks",
            "12",
            "--env",
            env,
            "--to",
            to.to_str().unwrap(),
            "--",
            "--unpaced",
        ]));
        let _held = opener.join().unwrap();
        let line = out.lines().next().unwrap();
        assert_eq!(
            line,
            format!(
                "bench 03a_oldmombasa {env} H2_RENDER_SCALE={scale}: 24.0 fps, busy p50 8.00 p95 12.50 ms over 12 frames (1 over 33.3 ms); phase mean ms: render 4.00, cpu 2.00"
            )
        );
        let dest = to.join(&id);
        assert_eq!(out.lines().nth(1).unwrap(), dest.to_str().unwrap());
        let cap: Value =
            serde_json::from_slice(&fs::read(dest.join("capture.json")).unwrap()).unwrap();
        assert_eq!(cap["frames"], 12);
        assert!(dest.join("render-stats.json").is_file());
        let counters: Value =
            serde_json::from_slice(&fs::read(dest.join("profile-counters.json")).unwrap()).unwrap();
        assert_eq!(counters["draws"], 100);
        let log = fs::read_to_string(dest.join("run.log")).unwrap();
        assert!(
            log.contains(&format!("scale={scale}")) && log.contains("--unpaced"),
            "{log}"
        );
        assert!(log.contains("got TERM"), "the bench stops its game: {log}");
        dirs.push(dest);
    }
    let runs = ok(&lab.local(&["runs"]));
    assert_eq!(runs.matches("run stopped").count(), 2, "{runs}");
    assert_ne!(dirs[0], dirs[1]);
    // Needs a level; a build that fails to start is reported, not hung on.
    assert!(err(&lab.local(&["bench", "--current"])).contains("needs --level"));
}
