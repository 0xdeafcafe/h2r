//! `h2 lab` end to end on this machine with a fake game: a host on a
//! temporary socket, runs through `--host local` and paired TLS over loopback,
//! builds of a bundled branch from a scratch git checkout with a fake cargo,
//! and the image cache. Nothing here builds or runs the game.
#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use agent_proto::takeover::{self, Decision, Operation};
use serde_json::{Value, json};

/// A fake `h2`: `build` makes an image directory; `play` logs its
/// arguments and environment, writes frames to the CSV one per 50 ms until
/// H2_BOOT_STOP_AT_PRESENT, writes input and agent artifacts, prints a frame
/// stats report, and exits 143 on SIGTERM.
const FAKE_H2: &str = r#"#!/bin/sh
cmd=$1; shift
if [ "$cmd" = build ]; then
  out=; check=
  while [ $# -gt 0 ]; do
    case "$1" in --out) out=$2; shift;; --check) check=1;; esac
    shift
  done
  case "$(uname -s):$(uname -m)" in Linux:x86_64) lib=libh2game.so;; *) lib=libh2game.dylib;; esac
  if [ "$check" = 1 ]; then
    [ -n "$out" ] && [ -f "$out/$lib" ] && exit 0
    exit 3
  fi
  mkdir -p "$out" && echo image > "$out/$lib"
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
    capture) printf '{"frames":%s,"fps":24.0,"budget_ms":33.3,"over_budget":1,"complete":true,"busy_ms":{"p50":8.0,"p95":12.5},"phases":{"cpu":{"mean":2.0},"render":{"mean":4.0},"pace":{"mean":9.0}},"csv":"a,b\\n1,2\\n"}\n' "$2";;
    render) echo '{"interp":0}';;
    counters) echo '{"draws":100}';;
    flight) printf '%s\n' "$@";;
    socket) printf '%s\n' "$sock";;
    fail) echo '{"code":3,"message":"nope"}' >&2; exit 2;;
    -) while read -r line; do echo "script: $line"; done;;
    *) p=$(printf '%s' "$2" | sed 's/[\\"]/\\&/g'); printf '{"method":"%s","params":"%s"}\n' "$1" "$p";;
  esac
  exit 0
fi
if [ "$cmd" = sim ]; then
  # `sim run|batch ENTRY --disc D --out O --data X [--report R | --parallel N]`:
  # copies the report the test staged for H2_FEATURES under <root>/sim-fixtures.
  # The step directory is <root>/runs/<id>/sim/<step>.
  mode=$1; entry=$2; shift 2
  report=; parallel=; data=
  while [ $# -gt 0 ]; do
    case "$1" in --report) report=$2; shift;; --parallel) parallel=$2; shift;; --data) data=$2; shift;; --disc|--out) shift;; esac
    shift
  done
  root=$(cd "$PWD/../../../.." && pwd)
  fix="$root/sim-fixtures/${H2_FEATURES:-none}.json"
  echo "fake sim $mode entry=$entry features=$H2_FEATURES opt=$H2_OPT parallel=$parallel leaked=$H2_LEAK"
  echo "fake sim data=$data bots=$(ls "$data/bots" 2>/dev/null | tr '\n' ' ')"
  [ "$H2_FEATURES" = navlog ] && echo "bots: lockout: nav mesh none yet (bots walk straight until built with h2 bots nav lockout)"
  [ -f "$fix" ] || { echo "no fixture $fix" >&2; exit 4; }
  if [ "$mode" = batch ]; then
    out=$(sed -n 's/^out = "\(.*\)"$/\1/p' "$entry")
    d="$(dirname "$entry")/$out/runs/m1"
    mkdir -p "$d/data" && echo x > "$d/data/blob" && cp "$fix" "$d/report.json"
  else
    cp "$fix" "$report"
  fi
  exit 0
fi
echo "fake play $*"
echo "lab job id=$H2_LAB_JOB_ID name=$H2_LAB_JOB_NAME"
trap 'echo "got TERM"; exit 143' TERM
if [ -n "$H2_AGENT_OUT" ]; then
  mkdir -p "$H2_AGENT_OUT/traces" "$H2_AGENT_OUT/flights" "$H2_AGENT_OUT/inputs" || exit 1
  run_id=${PWD##*/}
  printf '{"tick":1,"ticks":1,"map":"%s","objects":{}}\n' "$run_id" > "$H2_AGENT_OUT/traces/trace.jsonl"
  cp "$H2_AGENT_OUT/traces/trace.jsonl" "$H2_AGENT_OUT/flights/flight.jsonl"
  printf '# h2 input log v1\n# run %s\n' "$run_id" > "$H2_AGENT_OUT/inputs/pads.input"
fi
echo "env level=$H2_LEVEL opt=$H2_OPT replay=$H2_INPUT_REPLAY record=$H2_INPUT_RECORD profile=$H2_PROFILE from=$H2_FRAME_STATS_FROM home=$H2_LEAK scale=$H2_RENDER_SCALE pgo=$H2_PGO_OUT agent=$H2_AGENT agent_out=$H2_AGENT_OUT"
n=${H2_BOOT_STOP_AT_PRESENT:-${FAKE_FRAMES:-1000000}}
[ -n "$H2_FRAME_STATS_CSV" ] && echo "frame,total_us" > "$H2_FRAME_STATS_CSV"
[ -n "$H2_INPUT_RECORD" ] && echo "pad reads" > "$H2_INPUT_RECORD"
film=; prev=
for a in "$@"; do [ "$prev" = --film ] && film=$a; prev=$a; done
[ -n "$film" ] && { mkdir -p "$(dirname "$film")" && echo "not a real film" > "$film"; echo "data=$(dirname "$film")"; }
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
    endpoint: String,
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
        Lab::with_host_options(name, &[])
    }

    fn with_host_options(name: &str, options: &[&str]) -> Lab {
        Lab::with_setup(name, options, |_, _| {})
    }

    fn with_setup(name: &str, options: &[&str], setup: impl FnOnce(&Path, &Path)) -> Lab {
        let dir = scratch(name);
        let fake_h2 = dir.join("fake-h2");
        script(&fake_h2, FAKE_H2);
        let cargo_log = dir.join("cargo.log");
        // The test can hold fake cargo after it logs the call and before
        // it produces the binary. Bound the hold if the test panics.
        script(
            &dir.join("fake-cargo"),
            &format!(
                r#"#!/bin/sh
echo "$* in $(pwd)" >> '{}'
gate="$(dirname "$0")/cargo.hold"
waits=0
while [ -f "$gate" ]; do
  if [ "$waits" -ge 300 ]; then
    echo "fake cargo gate timed out" >&2
    exit 1
  fi
  waits=$((waits+1))
  sleep 0.1
done
mkdir -p "$CARGO_TARGET_DIR/release"
cp '{}' "$CARGO_TARGET_DIR/release/h2"
"#,
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
        setup(&root, &repo);
        let host_home = dir.join("host-home");
        fs::create_dir(&host_home).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_h2"))
            .args(["lab", "host", "--listen", "127.0.0.1:0", "--root"])
            .arg(&root)
            .arg("--socket")
            .arg(&socket)
            .arg("--repo")
            .arg(&repo)
            .arg("--disc")
            .arg(&disc)
            .arg("--h2")
            .arg(&fake_h2)
            .arg("--out")
            .arg(dir.join("current-image"))
            .arg("--cargo")
            .arg(dir.join("fake-cargo"))
            .args(options)
            .env("HOME", &host_home)
            .env("XDG_CACHE_HOME", &host_home)
            // The host's own H2_* never reach a run.
            .env("H2_LEAK", "leaked")
            // Bound fake play if a test panics before stopping its runs.
            .env("FAKE_FRAMES", "1200")
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
        fs::create_dir(dir.join("driver-home")).unwrap();
        let mut lab = Lab {
            dir,
            socket,
            root,
            endpoint: String::new(),
            repo,
            dev,
            cargo_log,
            _host: host,
        };
        let status = json(&ok(&lab.local(&["status"])));
        lab.endpoint = status["listen"].as_str().unwrap().to_owned();
        assert!(err(&lab.lan(&["status"])).contains("unpaired"));
        let code = ok(&lab.h2(
            &["host", "pair-code", "--root", lab.root.to_str().unwrap()],
            &[],
        ));
        ok(&lab.h2(&["pair", &lab.endpoint, code.trim()], &[]));
        lab
    }

    /// `h2 lab <args> --host local --socket S`, from the dev checkout.
    fn local(&self, args: &[&str]) -> Output {
        self.h2(
            args,
            &["--host", "local", "--socket", self.socket.to_str().unwrap()],
        )
    }

    /// The same operations over the paired loopback connection.
    fn lan(&self, args: &[&str]) -> Output {
        self.h2(args, &["--host", &self.endpoint])
    }

    fn h2(&self, args: &[&str], target: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_h2"));
        // The target before the command's own arguments, which may end in `-- play flags`.
        c.arg("lab")
            .arg(args[0])
            .args(target)
            .args(&args[1..])
            .current_dir(&self.dev);
        c.env("HOME", self.dir.join("driver-home"))
            .env("H2_DATA", self.dir.join("driver-data"))
            .env_remove("H2_LAB_HOST")
            .env_remove("XDG_CONFIG_HOME")
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

struct FakeGame {
    socket: PathBuf,
    window: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<takeover::Request>>>,
    worker: Option<JoinHandle<()>>,
}

impl FakeGame {
    fn listen(lab: &Lab, window: bool, decision: Decision) -> Self {
        // Lab jobs with explicit agent sockets advertise here. This fake
        // answers the game API directly and never starts a game process.
        Self::at(lab.root.join("a").join("fake.sock"), window, decision)
    }

    fn at(path: PathBuf, window: bool, decision: Decision) -> Self {
        let listener = UnixListener::bind(&path).unwrap();
        let window = Arc::new(AtomicBool::new(window));
        let shutdown = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker = {
            let (window, shutdown, requests) = (window.clone(), shutdown.clone(), requests.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let mut stream = stream.unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut line = String::new();
                    if !matches!(BufReader::new(&stream).read_line(&mut line), Ok(n) if n > 0) {
                        continue;
                    }
                    let request: agent_proto::Request = serde_json::from_str(&line).unwrap();
                    let result = match request.method.as_str() {
                        takeover::INFO => json!(takeover::Info {
                            window: window.load(Ordering::Acquire),
                            pid: None,
                        }),
                        takeover::REQUEST => {
                            let params = serde_json::from_value(request.params).unwrap();
                            requests.lock().unwrap().push(params);
                            json!(takeover::Requested { id: 1 })
                        }
                        takeover::STATUS => {
                            let params: takeover::StatusParams =
                                serde_json::from_value(request.params).unwrap();
                            assert_eq!(params.id, 1);
                            json!(takeover::Status { decision })
                        }
                        method => panic!("unexpected game API method {method}"),
                    };
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({ "jsonrpc": "2.0", "id": request.id, "result": result })
                    );
                }
            })
        };
        Self {
            socket: path,
            window,
            shutdown,
            requests,
            worker: Some(worker),
        }
    }

    fn close_window(&self) {
        self.window.store(false, Ordering::Release);
    }
}

impl Drop for FakeGame {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.socket); // Wake accept.
        self.worker.take().unwrap().join().unwrap();
        let _ = fs::remove_file(&self.socket);
    }
}

fn start_fake_run(lab: &Lab) -> String {
    let value = json(&ok(&lab.local(&[
        "run",
        "--name",
        "Synthetic job",
        "--current",
        "--frames",
        "1",
    ])));
    value["id"].as_str().unwrap().to_owned()
}

fn wait_fake_job(lab: &Lab, id: &str) -> Value {
    json(&ok(&lab.local(&["wait", id, "--timeout", "10"])))
}

#[test]
fn takeover_allow_waits_for_the_game_window_to_close() {
    let lab = Lab::named("allow");
    let game = FakeGame::listen(&lab, true, Decision::Allowed);
    let id = start_fake_run(&lab);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let state = json(&ok(&lab.local(&["status"])));
        if state["active"]["state"] == "awaiting_quit" {
            assert_eq!(state["active"]["name"], "Synthetic job");
            break;
        }
        assert!(Instant::now() < deadline, "no quit wait: {state}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let before_quit = lab.local(&["wait", &id, "--timeout", "0"]);
    assert_eq!(before_quit.status.code(), Some(3));
    assert!(!lab.root.join("runs").join(&id).join("run.log").exists());
    let second = start_fake_run(&lab);
    assert_eq!(
        json(&ok(&lab.local(&["status"])))["queued"],
        json!([second])
    );
    ok(&lab.local(&["stop", &second]));
    let stopped = lab.local(&["wait", &second, "--timeout", "0"]);
    assert_eq!(stopped.status.code(), Some(1));
    assert_eq!(
        json(&String::from_utf8_lossy(&stopped.stdout))["job"]["state"],
        "stopped"
    );

    let requests = game.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].requester, "Local lab");
    assert_eq!(requests[0].operation, Operation::Run);
    assert_eq!(requests[0].revision, "installed image");
    assert_eq!(requests[0].timeout_seconds, 60);
    assert!(requests[0].estimate_seconds > 0);
    drop(requests);

    game.close_window();
    assert_eq!(wait_fake_job(&lab, &id)["job"]["state"], "done");
}

#[test]
fn takeover_declines_a_build_and_suppresses_prompts_until_a_headless_run() {
    let lab = Lab::named("decline");
    let commit = git(&lab.repo, &["rev-parse", "HEAD"]);
    // A cached build can complete during play because it doesn't compile.
    assert_eq!(
        json(&ok(&lab.local(&["build", "--rev", &commit, "--wait"])))["job"]["state"],
        "done",
    );
    let cargo_before = fs::read(&lab.cargo_log).unwrap();
    let game = FakeGame::listen(&lab, true, Decision::Declined);
    assert_eq!(
        json(&ok(&lab.local(&["build", "--rev", &commit, "--wait"])))["job"]["cached"],
        true,
    );
    assert!(game.requests.lock().unwrap().is_empty());

    let response = json(&ok(&lab.lan(&[
        "build",
        "--rev",
        &commit,
        "--build-env",
        "H2_OPT=2",
        "--wait",
    ])));
    assert_eq!(response["finished"], true);
    assert_eq!(response["job"]["state"], "skipped");
    assert_eq!(response["job"]["skip_reason"], "declined by the player");
    assert_eq!(response["job"]["error"], Value::Null);
    {
        let requests = game.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].requester, "Lab 127.0.0.1");
        assert_eq!(requests[0].operation, Operation::Build);
        assert_eq!(requests[0].revision, &commit[..12]);
        assert!(requests[0].estimate_seconds > 0);
    }
    assert_eq!(fs::read(&lab.cargo_log).unwrap(), cargo_before);
    let id = start_fake_run(&lab);
    assert_eq!(
        wait_fake_job(&lab, &id)["job"]["skip_reason"],
        "declined by the player"
    );
    assert_eq!(game.requests.lock().unwrap().len(), 1);

    game.close_window();
    let id = start_fake_run(&lab);
    assert_eq!(wait_fake_job(&lab, &id)["job"]["state"], "done");
    assert_eq!(game.requests.lock().unwrap().len(), 1);
}

#[test]
fn takeover_without_an_answer_times_out_and_can_disable_the_cooldown() {
    let lab = Lab::with_host_options(
        "timeout",
        &["--takeover-timeout", "1", "--takeover-cooldown", "0"],
    );
    let game = FakeGame::listen(&lab, true, Decision::Pending);
    for _ in 0..2 {
        let started = Instant::now();
        let id = start_fake_run(&lab);
        let response = wait_fake_job(&lab, &id);
        assert_eq!(response["job"]["state"], "skipped");
        assert_eq!(response["job"]["skip_reason"], "declined by the player");
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(!lab.root.join("runs").join(id).join("run.log").exists());
    }
    let requests = game.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.timeout_seconds == 1));
}

#[test]
fn a_lab_host_runs_waits_stops_fetches_and_builds_branches() {
    let lab = Lab::start();

    // status, locally and through paired TCP.
    let s = json(&ok(&lab.local(&["status"])));
    assert_eq!(s["version"], 1);
    assert!(s["active"].is_null());
    let s2 = json(&ok(&lab.lan(&["status"])));
    assert_eq!(s2["repo"], s["repo"]);

    // A run of the host's own image, waited for and fetched.
    let to = lab.dir.join("fetched");
    let out = ok(&lab.local(&[
        "run",
        "--name",
        "Synthetic job",
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
    let meta: Value = serde_json::from_slice(&fs::read(run.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["name"], "Synthetic job");
    assert_eq!(meta["request"]["name"], meta["name"]);
    let csv = fs::read_to_string(run.join("frames.csv")).unwrap();
    assert_eq!(csv.lines().count(), 4, "{csv}");
    let log = fs::read_to_string(run.join("run.log")).unwrap();
    assert!(
        log.contains(&format!("lab job id={id} name=Synthetic job")),
        "{log}"
    );
    assert!(
        log.contains(&format!("--title h2 · lab {id} · Synthetic job ·")),
        "{log}"
    );
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
    assert!(runs.contains("Synthetic job"), "{runs}");
    assert!(runs.contains("frame stats: 3 frames"), "{runs}");

    // Refusals: host-owned variables, paths out of the root, unknown flags.
    assert!(
        err(&lab.local(&[
            "run",
            "--name",
            "Synthetic job",
            "--current",
            "--env",
            "H2_FRAME_STATS_CSV=/tmp/x"
        ]))
        .contains("not a run variable")
    );
    assert!(
        err(&lab.local(&[
            "run",
            "--name",
            "Synthetic job",
            "--current",
            "--replay",
            "../../x"
        ]))
        .contains("must be 1-64")
    );
    assert!(
        err(&lab.local(&[
            "run",
            "--name",
            "Synthetic job",
            "--current",
            "--",
            "--out",
            "/tmp"
        ]))
        .contains("not one the lab passes")
    );
    assert!(
        err(&lab.local(&[
            "run",
            "--name",
            "Synthetic job",
            "--current",
            "--replay",
            "nothing"
        ]))
        .contains("no recording nothing")
    );

    // One at a time: a long run, a queued second, and a stop.
    let v = json(&ok(&lab.local(&[
        "run",
        "--name",
        "Synthetic job",
        "--current",
        "--level",
        "ascension",
    ])));
    let long = v["id"].as_str().unwrap().to_string();
    let t = Instant::now();
    loop {
        let s = json(&ok(&lab.local(&["status"])));
        if s["active"]["state"] == "running" {
            assert_eq!(s["jobs_active"], serde_json::json!([s["active"]]));
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "never running: {s}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let next = start_fake_run(&lab);
    assert_eq!(json(&ok(&lab.local(&["status"])))["queued"], json!([next]));
    // A commit the host doesn't have can be synced while the game runs.
    let commit = git(
        &lab.dev,
        &[
            "commit-tree",
            "HEAD^{tree}",
            "-p",
            "HEAD",
            "-m",
            "sync during a run",
        ],
    );
    git(&lab.dev, &["branch", "while-running", &commit]);
    let synced = ok(&lab.lan(&["sync", "--branch", "while-running"]));
    assert_eq!(synced.trim(), format!("lab/while-running {commit}"));
    assert_eq!(
        git(&lab.repo, &["rev-parse", "refs/heads/lab/while-running"]),
        commit
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

    assert_eq!(wait_fake_job(&lab, &next)["job"]["state"], "done");

    // Record, then replay the recording (level from its manifest), over the LAN transport.
    let v = json(&ok(&lab.lan(&[
        "run",
        "--name",
        "Synthetic job",
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
    let recs = json(&ok(&lab.lan(&["recordings"])));
    let r = &recs["recordings"][0];
    assert_eq!(r["name"], "fight");
    assert_eq!(r["level"], "03a_oldmombasa");
    assert!(r["snapshot"].is_null());
    assert!(lab.root.join("recordings/fight/input.bin").is_file());
    let v = json(&ok(&lab.lan(&[
        "run",
        "--name",
        "Synthetic job",
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
        err(&lab.local(&[
            "run",
            "--name",
            "Synthetic job",
            "--current",
            "--level",
            "x",
            "--record",
            "fight"
        ]))
        .contains("exists")
    );

    // put: an uploaded recording, over the LAN transport.
    let file = lab.dir.join("mine.bin");
    fs::write(&file, b"\x00\x01recorded elsewhere\xff").unwrap();
    let v = json(&ok(&lab.lan(&[
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

    // A branch: synced over paired TCP into lab/<branch>, built in a
    // worktree with its build variables, run; the second run reuses the
    // image.
    git(&lab.dev, &["checkout", "-q", "-b", "perf-work"]);
    fs::write(lab.dev.join("README"), "faster\n").unwrap();
    git(&lab.dev, &["commit", "-q", "-am", "two"]);
    let head = git(&lab.dev, &["rev-parse", "HEAD"]);
    let v = json(&ok(&lab.lan(&[
        "run",
        "--name",
        "Synthetic job",
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
    assert!(lab.root.join("src/current/README").is_file());
    let cargo = fs::read_to_string(&lab.cargo_log).unwrap();
    assert_eq!(cargo.lines().count(), 1);
    assert!(cargo.contains("build --release -p cli --bin h2"), "{cargo}");
    let v = json(&ok(&lab.lan(&[
        "run",
        "--name",
        "Synthetic job",
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
fn a_running_job_pins_the_image_and_queued_branches_share_one_checkout() {
    let lab = Lab::named("fifo");
    let first = git(&lab.repo, &["rev-parse", "HEAD"]);
    git(
        &lab.repo,
        &["update-ref", "refs/remotes/origin/master", &first],
    );
    let active = json(&ok(&lab.local(&[
        "run",
        "--name",
        "Synthetic job",
        "--rev",
        "HEAD",
    ])));
    let run_id = active["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while json(&ok(&lab.local(&["status"])))["active"]["state"] != "running" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    let stamp = lab.root.join("image/image.json");
    let checkout = lab.root.join("src/current");
    let keep = checkout.join("untracked-sentinel");
    fs::write(&keep, b"keep").unwrap();
    git(
        &lab.repo,
        &["commit", "-q", "--allow-empty", "-m", "second"],
    );
    let second = git(&lab.repo, &["rev-parse", "HEAD"]);
    git(
        &lab.repo,
        &["update-ref", "refs/remotes/origin/master", &second],
    );
    let b = json(&ok(&lab.local(&["build", "--rev", "origin/master"])));
    assert_eq!(b["job"]["commit"], second);
    git(
        &lab.repo,
        &["update-ref", "refs/remotes/origin/master", &first],
    );
    let a = json(&ok(&lab.local(&["build", "--rev", "origin/master"])));
    assert_eq!(a["job"]["commit"], first);
    let status = json(&ok(&lab.local(&["status"])));
    assert_eq!(status["active"]["id"], run_id);
    assert_eq!(status["queued"], json!([b["id"], a["id"]]));
    let stamped: Value = serde_json::from_slice(&fs::read(&stamp).unwrap()).unwrap();
    assert_eq!(stamped["commit"], first);
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), first);
    ok(&lab.local(&["stop", run_id]));
    for v in [&b, &a] {
        let result = wait_fake_job(&lab, v["id"].as_str().unwrap());
        assert_eq!(result["job"]["state"], "done", "{result}");
        assert_eq!(result["job"]["cached"], false);
    }
    let stamped: Value = serde_json::from_slice(&fs::read(&stamp).unwrap()).unwrap();
    assert_eq!(stamped["commit"], first);
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), first);
    assert!(keep.is_file());
    assert_eq!(fs::read_dir(lab.root.join("src")).unwrap().count(), 1);
    assert_eq!(
        fs::read_to_string(&lab.cargo_log).unwrap().lines().count(),
        3
    );
    assert_eq!(
        json(&ok(&lab.local(&["build", "--rev", &first, "--wait"])))["job"]["cached"],
        true
    );
    // A partial replacement cannot reuse a missing or damaged stamp.
    fs::write(&stamp, b"{").unwrap();
    assert_eq!(
        json(&ok(&lab.local(&["build", "--rev", &first, "--wait"])))["job"]["cached"],
        false
    );
}

#[test]
fn host_migration_removes_only_the_legacy_build_directories() {
    let lab = Lab::with_setup("migrate", &[], |root, repo| {
        let commit = git(repo, &["rev-parse", "HEAD"]);
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                src.join(commit).to_str().unwrap(),
                "HEAD",
            ],
        );
        for dir in [
            "images/a123456789abcdef",
            "images/other",
            "src/other",
            "runs/0001",
            "recordings/demo",
        ] {
            let path = root.join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("sentinel"), b"keep").unwrap();
        }
        std::os::unix::fs::symlink(
            root.join("recordings"),
            root.join("images/0123456789abcdef"),
        )
        .unwrap();
    });
    let old = lab
        .root
        .join("src")
        .join(git(&lab.repo, &["rev-parse", "HEAD"]));
    assert!(!old.exists());
    assert!(!git(&lab.repo, &["worktree", "list", "--porcelain"]).contains(old.to_str().unwrap()));
    assert!(!lab.root.join("images/a123456789abcdef").exists());
    for dir in ["images/other", "src/other", "runs/0001", "recordings/demo"] {
        assert!(lab.root.join(dir).join("sentinel").is_file());
    }
    assert!(lab.root.join("images/0123456789abcdef").is_symlink());
}

#[test]
fn the_build_caches_can_be_capped_independently_of_the_published_image() {
    let lab = Lab::with_host_options("caps", &["--obj-cache-limit", "0", "--target-limit", "0"]);
    let obj = lab.root.join("obj-cache");
    fs::create_dir_all(&obj).unwrap();
    fs::write(obj.join("unused.o"), b"old").unwrap();
    assert_eq!(
        json(&ok(&lab.local(&["build", "--rev", "HEAD", "--wait"])))["job"]["cached"],
        false
    );
    assert!(!obj.join("unused.o").exists());
    assert_eq!(fs::read_dir(lab.root.join("target")).unwrap().count(), 0);
    assert_eq!(
        json(&ok(&lab.local(&["build", "--rev", "HEAD", "--wait"])))["job"]["cached"],
        true
    );
}

#[test]
fn the_lan_pairing_can_be_revoked_without_stopping_the_host() {
    let lab = Lab::named("r");
    ok(&lab.lan(&["status"]));
    ok(&lab.h2(
        &["host", "unpair", "--root", lab.root.to_str().unwrap()],
        &[],
    ));
    assert!(err(&lab.lan(&["status"])).contains("authentication"));
    ok(&lab.local(&["status"]));
}

#[test]
fn a_film_run_records_into_the_hosts_library_and_the_run() {
    let lab = Lab::named("film");
    let to = lab.dir.join("fetched");
    let run = |name: &str, extra: &[&str]| {
        let mut args = vec![
            "run", "--name", "Film job", "--current", "--level", "03a_oldmombasa", "--frames", "2",
            "--film", name, "--wait", "--to", to.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        lab.local(&args)
    };
    // Bad names are refused before anything starts.
    for bad in ["../x", "a/b", "x.h2film", ".h"] {
        assert!(!run(bad, &[]).status.success(), "{bad}");
    }
    // `-- --film PATH` can't aim the game's own flag at a path.
    assert!(err(&run("ok", &["--", "--film", "/tmp/x"])).contains("not one the lab passes"));

    // The game records into the data root's films/, which `theater.play`
    // resolves; the run keeps a copy that `fetch` brings back.
    let v = json(&ok(&run("first", &[])));
    assert_eq!(v["job"]["state"], "done");
    let id = v["job"]["id"].as_str().unwrap();
    let library = lab.root.join("data/films/first.h2film");
    assert!(library.is_file());
    assert!(lab.root.join("runs").join(id).join("first.h2film").is_file());
    assert!(to.join(id).join("first.h2film").is_file(), "fetched with the run");
    let log = fs::read_to_string(to.join(id).join("run.log")).unwrap();
    assert!(log.contains("--film ") && log.contains("films/first.h2film"), "{log}");
    // The fake's file has no readable header: said in the job's notes, no ID.
    let notes = v["job"]["notes"].to_string();
    assert!(notes.contains("film first: header unreadable"), "{notes}");
    assert!(v["job"].get("film_id").is_none());

    // A name in the library is never reused, at submission.
    assert!(err(&run("first", &[])).contains("film first exists"));

    // --fresh-data: the film is in the job's scratch library while it runs,
    // and only the run's copy survives.
    let v = json(&ok(&run("fresh", &["--fresh-data"])));
    let id = v["job"]["id"].as_str().unwrap();
    assert!(to.join(id).join("fresh.h2film").is_file());
    assert!(!lab.root.join("data/films/fresh.h2film").exists());
    assert!(!lab.root.join("scratch").join(id).exists());
    let log = fs::read_to_string(to.join(id).join("run.log")).unwrap();
    assert!(log.contains(&format!("scratch/{id}/data/films/fresh.h2film")), "{log}");
    // Playback (`--serve` + `theater.play`) reads the library the game has;
    // the game's own `--data` is the one the film was recorded under.
    assert!(log.contains(&format!("--data {}", lab.root.join("scratch").join(id).join("data").display())), "{log}");
}

#[test]
fn an_own_data_run_plays_on_the_hosts_default_data_root() {
    let lab = Lab::named("own");
    let to = lab.dir.join("fetched");
    let run = |extra: &[&str]| {
        let mut args = vec![
            "run", "--name", "Own", "--current", "--level", "03a_oldmombasa", "--frames", "2",
            "--wait", "--to", to.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        lab.local(&args)
    };
    // Refused before anything starts.
    let e = err(&run(&["--own-data", "--fresh-data"]));
    assert!(e.contains("--own-data") && e.contains("--fresh-data"), "{e}");

    // The lab's own data is passed with --data; --own-data leaves it out,
    // and the game's request for consent says whose data it is.
    let id = |v: &str| json(v)["job"]["id"].as_str().unwrap().to_owned();
    let plain = id(&ok(&run(&[])));
    let own = id(&ok(&run(&["--own-data"])));
    let log = |id: &str| fs::read_to_string(to.join(id).join("run.log")).unwrap();
    assert!(log(&plain).contains("--data "), "{}", log(&plain));
    assert!(!log(&own).contains("--data"), "{}", log(&own));
    let meta = fs::read_to_string(lab.root.join("runs").join(&own).join("meta.json")).unwrap();
    assert!(meta.contains("\"own_data\": true"), "{meta}");
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
    let path = wait_for_agent_socket_path(root, id);
    let listener = UnixListener::bind(&path).unwrap();
    (path, listener)
}

fn wait_for_agent_socket_path(root: &Path, id: &str) -> PathBuf {
    let meta = root.join("runs").join(id).join("meta.json");
    let t = Instant::now();
    loop {
        if let Ok(b) = fs::read(&meta) {
            let v: Value = serde_json::from_slice(&b).unwrap();
            if let Some(p) = v["agent_socket"].as_str() {
                let p = PathBuf::from(p);
                assert!(p.as_os_str().len() <= 100, "{p:?} is too long for SUN_LEN");
                return p;
            }
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "run {id} never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_run_log(root: &Path, id: &str) -> String {
    let log = root.join("runs").join(id).join("run.log");
    let t = Instant::now();
    loop {
        if let Ok(text) = fs::read_to_string(&log)
            && text.contains("agent=")
        {
            return text;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no run.log for {id}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn serve_runs_queue_one_at_a_time_and_stream_their_logs() {
    let lab = Lab::named("m");
    let args = [
        "run",
        "--name",
        "Synthetic job",
        "--current",
        "--level",
        "ascension",
        "--serve",
    ];
    let first = json(&ok(&lab.local(&args)));
    let first = first["id"].as_str().unwrap().to_owned();
    let (first_socket, _first_listener) = open_agent_socket(&lab.root, &first);
    // A second job waits for the first instead of running beside it.
    let second = json(&ok(&lab.local(&args)));
    let second = second["id"].as_str().unwrap().to_owned();
    let status = json(&ok(&lab.local(&["status"])));
    assert_eq!(status["max_jobs"], 1, "{status}");
    assert_eq!(status["queued"], json!([second]), "{status}");
    let jobs = status["jobs_active"].as_array().unwrap();
    assert_eq!(jobs.len(), 1, "{status}");
    assert_eq!(jobs[0]["id"], first.as_str());

    let log = wait_for_run_log(&lab.root, &first);
    let run = lab.root.join("runs").join(&first);
    assert!(log.contains(&format!("agent={}", first_socket.display())), "{log}");
    assert!(log.contains(&format!("agent_out={}\n", run.display())), "{log}");
    let out = ok(&lab.local(&["agent", &first, "socket"]));
    assert_eq!(out.trim_end(), first_socket.to_str().unwrap());

    let current = ok(&lab.lan(&["logs", "current"]));
    assert!(current.contains(&format!("[{first}] running")), "{current}");
    assert!(current.contains(&format!("[{first}] run | fake play")), "{current}");
    let named = ok(&lab.lan(&["logs", &second]));
    assert!(!named.contains(&format!("[{first}]")), "{named}");

    ok(&lab.local(&["stop", &first]));
    let waited = lab.local(&["wait", &first, "--timeout", "10"]);
    assert_eq!(waited.status.code(), Some(1));
    assert_eq!(
        json(&String::from_utf8_lossy(&waited.stdout))["job"]["state"],
        "stopped"
    );
    let stopped = ok(&lab.lan(&["logs", &first, "--follow"]));
    assert!(stopped.contains(&format!("[{first}] cancelled")), "{stopped}");
    assert!(!first_socket.exists());

    // The queued job starts once the first has finished.
    let (second_socket, _second_listener) = open_agent_socket(&lab.root, &second);
    let t = Instant::now();
    loop {
        let status = json(&ok(&lab.local(&["status"])));
        if status["active"]["id"] == second.as_str() && status["active"]["state"] == "running" {
            assert_eq!(status["jobs_active"], json!([status["active"]]));
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "second never ran: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
    ok(&lab.local(&["stop", &second]));
    let waited = lab.local(&["wait", &second, "--timeout", "10"]);
    assert_eq!(waited.status.code(), Some(1));
    assert!(!second_socket.exists());
}

#[test]
fn lab_agent_forwards_calls_to_a_serve_runs_game_and_returns_exit_codes() {
    let lab = Lab::named("g");
    let v = json(&ok(&lab.local(&[
        "run",
        "--name",
        "Synthetic job",
        "--current",
        "--level",
        "ascension",
        "--serve",
    ])));
    let id = v["id"].as_str().unwrap().to_string();
    let (sock, _listener) = open_agent_socket(&lab.root, &id);
    // The game got the same path in H2_AGENT; no frame limit was set.
    let text = wait_for_run_log(&lab.root, &id);
    assert!(
        text.contains(&format!("agent={}", sock.display())),
        "{text}"
    );
    assert!(
        sock.starts_with(lab.root.join("a")) || sock.starts_with("/tmp"),
        "{sock:?}"
    );

    // A short form, a method with JSON, and the same through the paired transport.
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
    let out = json(&ok(&lab.lan(&[
        "agent",
        &id,
        "session.wait",
        r#"{"until":{"phase":"gameplay"}}"#,
    ])));
    assert_eq!(out["method"], "session.wait");
    assert_eq!(out["params"], r#"{"until":{"phase":"gameplay"}}"#);

    // The exit code and stderr of the call come back as they were.
    let f = lab.lan(&["agent", &id, "fail"]);
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

    // Flight show is forced through the job's socket before user options.
    let out = ok(&lab.lan(&[
        "agent",
        &id,
        "flight",
        "show",
        "flight.jsonl",
        "--field",
        "physics.crouch",
    ]));
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "flight",
            "show",
            "--remote",
            "flight.jsonl",
            "--field",
            "physics.crouch"
        ]
    );
    assert!(err(&lab.local(&["agent", &id, "raw-diff", "x", "y"])).contains("reads files"));
    assert!(err(&lab.local(&["agent", "9999", "status"])).contains("no job 9999"));
    assert!(
        err(&lab.local(&[
            "run",
            "--name",
            "Synthetic job",
            "--current",
            "--serve",
            "--frames",
            "5"
        ]))
        .contains("never stops")
    );

    // Stopping the run ends it, removes the socket, and calls are refused.
    ok(&lab.local(&["stop", &id]));
    let w = lab.local(&["wait", &id, "--timeout", "10"]);
    assert_eq!(w.status.code(), Some(1));
    assert!(!sock.exists(), "the host removes the socket after the run");
    assert!(err(&lab.local(&["agent", &id, "status"])).contains("is stopped"));

    // The paired driver can fetch the files written by the game after the run ends.
    let to = lab.dir.join("fetched");
    ok(&lab.lan(&["fetch", &id, "--to", to.to_str().unwrap()]));
    let fetched = to.join(&id);
    for path in ["traces/trace.jsonl", "flights/flight.jsonl"] {
        let row = json(&fs::read_to_string(fetched.join(path)).unwrap());
        assert_eq!(row["tick"], 1);
        assert_eq!(row["map"], id);
    }
    assert_eq!(
        fs::read_to_string(fetched.join("inputs/pads.input")).unwrap(),
        format!("# h2 input log v1\n# run {id}\n")
    );
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
        let args = [
            "bench",
            "--name",
            "Synthetic job",
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
        ];
        let out = if n == 1 {
            ok(&lab.local(&args))
        } else {
            ok(&lab.lan(&args))
        };
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
    assert!(
        err(&lab.local(&["bench", "--name", "Synthetic job", "--current"]))
            .contains("needs --level")
    );
}

#[test]
fn watchdog_exit_fetches_the_game_report_with_the_failed_run() {
    let lab = Lab::named("memreport");
    // A synthetic game leaves a report exactly where the host requested.
    script(
        &lab.dir.join("fake-h2"),
        &format!(
            "#!/bin/sh\nmkdir -p \"$H2_MEM_REPORT_DIR\"\necho '{{\"trigger\":\"hard\"}}' > \"$H2_MEM_REPORT_DIR/game.json\"\nexit {}\n",
            mem_watch::MEMORY_LIMIT_EXIT
        ),
    );
    let dest = lab.dir.join("results");
    let out = lab.local(&[
        "run",
        "--name",
        "Synthetic job",
        "--current",
        "--mem-limit",
        "13G",
        "--wait",
        "--to",
        dest.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let answer = json(&String::from_utf8(out.stdout).unwrap());
    let job = &answer["job"];
    assert_eq!(job["state"], "failed");
    assert_eq!(job["exit_code"], mem_watch::MEMORY_LIMIT_EXIT);
    assert!(job["error"].as_str().unwrap().contains("memory limit"));
    assert_eq!(job["request"]["mem_limit"], 13_u64 << 30);
    let path = dest.join(job["id"].as_str().unwrap()).join("mem/game.json");
    assert_eq!(json(&fs::read_to_string(path).unwrap())["trigger"], "hard");
}

/// A report the fake `h2 sim` hands out for runs with this `H2_FEATURES`
/// (`none` when the variable is unset).
fn sim_fixture(root: &Path, features: &str, speedup: f64, ms: f64, hashes: &[(u32, &str)]) {
    use sim_bench::metrics::{End, Finish, Tally, TickHash};
    let mut r = Tally::new().report(Finish::bare(End::TickLimit));
    r.wall.speedup = speedup;
    r.wall.ms_per_tick = ms;
    r.state_hashes_v1 = hashes
        .iter()
        .map(|(tick, h)| TickHash {
            tick: *tick,
            gameplay: (*h).to_string(),
            arena: None,
        })
        .collect();
    let dir = root.join("sim-fixtures");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{features}.json")),
        serde_json::to_vec(&r).unwrap(),
    )
    .unwrap();
}

/// `m/lockout.toml` with a tuning file, and `m/batch.toml` over them, in the
/// driver's checkout.
fn write_sim_files(lab: &Lab) {
    let m = lab.dev.join("m");
    fs::create_dir_all(m.join("tuning")).unwrap();
    fs::write(m.join("tuning/base.toml"), "skill = \"legendary\"\n").unwrap();
    // The driver's nav mesh for the map (`h2 bots nav lockout`).
    let bots = lab.dir.join("driver-data/bots");
    fs::create_dir_all(&bots).unwrap();
    fs::write(bots.join("lockout.0.bots"), b"mesh").unwrap();
    fs::write(bots.join("lockout.0.vis"), b"grid").unwrap();
    fs::write(
        m.join("lockout.toml"),
        "map = \"lockout\"\n[[bot]]\nparams = \"tuning/base.toml\"\n",
    )
    .unwrap();
    fs::write(
        m.join("batch.toml"),
        "out = \"results\"\nmatch = \"lockout.toml\"\n[a]\nparams = \"tuning/base.toml\"\n",
    )
    .unwrap();
}

#[test]
fn lab_sim_runs_arms_interleaved_and_summarizes_speed_and_hashes() {
    let h = [(1, "aa"), (2, "bb")];
    let lab = Lab::with_setup("simab", &[], |root, _| {
        sim_fixture(root, "impl.sim.native", 4.0, 8.0, &h);
        sim_fixture(root, "-impl.sim.native", 3.0, 10.0, &h);
        sim_fixture(root, "none", 2.0, 20.0, &h);
    });
    write_sim_files(&lab);
    let to = lab.dir.join("fetched");
    let out = ok(&lab.local(&[
        "sim",
        "m/lockout.toml",
        "--name",
        "Sim A/B",
        "--branch",
        "master",
        "--build-env",
        "H2_OPT=2",
        "--repeat",
        "2",
        "--arm",
        "on=impl.sim.native",
        "--arm",
        "off=-impl.sim.native",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ]));
    let v = json(&out);
    let id = v["job"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["job"]["kind"], "sim");
    assert_eq!(v["job"]["state"], "done", "{out}");
    assert_eq!(v["job"]["name"], "Sim A/B");
    assert!(v["job"]["request"]["files"].is_array(), "file text stays out of the metadata");
    assert!(
        out.contains("sim benchmark Sim A/B: 2 arm(s) x 2 repeat(s), 4 runs, 0 failed"),
        "{out}"
    );
    assert!(
        out.contains("state hashes: identical across all runs and arms (3 report pair(s) compared)"),
        "{out}"
    );
    assert!(out.contains("off vs on: median ms/tick +25.0%, median speedup -25.0%"), "{out}");

    let run = to.join(&id);
    // A B, then B A: drift lands on both arms.
    let log = fs::read_to_string(run.join("run.log")).unwrap();
    let order: Vec<&str> = log
        .lines()
        .filter_map(|l| l.strip_prefix("lab: sim run "))
        .filter(|l| l.contains('/') && !l.contains(" in "))
        .collect();
    assert_eq!(order, ["1/4: on-r1", "2/4: off-r1", "3/4: off-r2", "4/4: on-r2"], "{log}");
    for step in ["on-r1", "off-r1", "off-r2", "on-r2"] {
        let dir = run.join("sim").join(step);
        assert!(dir.join("report.json").is_file(), "{step}");
        assert!(dir.join("in/lockout.toml").is_file(), "{step}");
        assert!(dir.join("in/tuning/base.toml").is_file(), "{step}");
    }
    let on = fs::read_to_string(run.join("sim/on-r1/sim.log")).unwrap();
    assert!(on.contains("fake sim run entry="), "{on}");
    assert!(on.contains("features=impl.sim.native opt=2 parallel= leaked=\n"), "{on}");
    let off = fs::read_to_string(run.join("sim/off-r2/sim.log")).unwrap();
    assert!(off.contains("features=-impl.sim.native"), "{off}");
    let summary = json(&fs::read_to_string(run.join("sim-summary.json")).unwrap());
    assert_eq!(summary["arms"][0]["speedup"]["median"], 4.0);
    assert_eq!(summary["arms"][1]["ms_per_tick"]["max"], 10.0);
    assert_eq!(summary["hashes"]["identical"], true);
    // What `--wait` prints is the summary file.
    assert!(out.contains(&fs::read_to_string(run.join("sim-summary.txt")).unwrap()), "{out}");
    // `lab runs` carries the same summary.
    let runs = ok(&lab.local(&["runs", "--limit", "1"]));
    assert!(runs.contains("sim benchmark Sim A/B"), "{runs}");

    // The image the build made is the one the sim ran on.
    let meta = json(&fs::read_to_string(run.join("meta.json")).unwrap());
    assert_eq!(meta["build_env"]["H2_OPT"], "2");
}

#[test]
fn lab_sim_batches_are_sequential_and_their_data_directories_are_removed() {
    let lab = Lab::with_setup("simbatch", &[], |root, _| {
        sim_fixture(root, "none", 2.0, 20.0, &[(1, "aa")]);
    });
    write_sim_files(&lab);
    let to = lab.dir.join("fetched");
    let out = ok(&lab.local(&[
        "sim",
        "m/batch.toml",
        "--current",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ]));
    let v = json(&out);
    assert_eq!(v["job"]["state"], "done", "{out}");
    let step = to.join(v["job"]["id"].as_str().unwrap()).join("sim/default-r1");
    let log = fs::read_to_string(step.join("sim.log")).unwrap();
    assert!(log.contains("fake sim batch entry="), "{log}");
    assert!(log.contains("parallel=1 "), "one at a time unless asked: {log}");
    let results = step.join("in/results/runs/m1");
    assert!(results.join("report.json").is_file());
    assert!(!results.join("data").exists(), "the data directory outlived its run");
    assert!(step.join("in/tuning/base.toml").is_file(), "tuning files upload too");

    let out = ok(&lab.local(&[
        "sim",
        "m/batch.toml",
        "--current",
        "--parallel",
        "2",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ]));
    let id = json(&out)["job"]["id"].as_str().unwrap().to_string();
    let log = fs::read_to_string(to.join(id).join("sim/default-r1/sim.log")).unwrap();
    assert!(log.contains("parallel=2 "), "{log}");
}

#[test]
fn lab_sim_names_the_first_differing_tick_and_fails_a_job_whose_run_failed() {
    let lab = Lab::with_setup("simdiff", &[], |root, _| {
        sim_fixture(root, "impl.sim.native", 4.0, 8.0, &[(1, "aa"), (2, "bb")]);
        sim_fixture(root, "-impl.sim.native", 3.0, 10.0, &[(1, "aa"), (2, "XX")]);
    });
    write_sim_files(&lab);
    let to = lab.dir.join("fetched");
    let out = ok(&lab.local(&[
        "sim",
        "m/lockout.toml",
        "--current",
        "--arm",
        "on=impl.sim.native",
        "--arm",
        "off=-impl.sim.native",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ]));
    assert!(
        out.contains("state hashes: DIFFER, first at tick 2 (off-r1 against on-r1, report.json)"),
        "{out}"
    );
    assert_eq!(json(&out)["job"]["state"], "done", "a difference is a result, not a failure");

    let o = lab.local(&[
        "sim",
        "m/lockout.toml",
        "--current",
        "--arm",
        "on=impl.sim.native",
        "--arm",
        "bad=no-such-fixture",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(json(&out)["job"]["state"], "failed", "{out}");
    assert!(out.contains("2 runs, 1 failed"), "{out}");
    assert!(!json(&out)["job"]["notes"].as_array().unwrap().is_empty());
}

#[test]
fn lab_sim_refuses_bad_requests_before_the_host_sees_them() {
    let lab = Lab::named("sr");
    write_sim_files(&lab);
    let refused = |args: &[&str], what: &str| {
        let mut a = vec!["sim", "m/lockout.toml", "--current"];
        a.extend_from_slice(args);
        let e = err(&lab.local(&a));
        assert!(e.contains(what), "{args:?}: {e}");
    };
    refused(&["--repeat", "0"], "--repeat");
    refused(&["--parallel", "2"], "batch");
    refused(&["--arm", "a=x", "--arm", "a=y"], "named twice");
    refused(&["--env", "H2_SIM=/x"], "not a run variable");
    refused(&["--build-env", "H2_OPT=2"], "branch or commit");
    let m = lab.dev.join("m");
    fs::write(
        m.join("escape.toml"),
        "out = \"../../x\"\nmatch = \"lockout.toml\"\n[a]\n",
    )
    .unwrap();
    let e = err(&lab.local(&["sim", "m/escape.toml", "--current"]));
    assert!(e.contains("no `..`"), "{e}");
    fs::write(m.join("gone.toml"), "[[bot]]\nparams = \"nope.toml\"\n").unwrap();
    let e = err(&lab.local(&["sim", "m/gone.toml", "--current"]));
    assert!(e.contains("nope.toml"), "{e}");
    // Nothing reached the host: no job exists.
    let runs = json(&ok(&lab.local(&["runs", "--json"])));
    assert!(runs["runs"].as_array().unwrap().is_empty(), "{runs}");
}

#[test]
fn lab_sim_uploads_the_drivers_nav_meshes_and_runs_on_them() {
    let lab = Lab::with_setup("simnav", &[], |root, _| {
        sim_fixture(root, "none", 2.0, 20.0, &[(1, "aa")]);
    });
    write_sim_files(&lab);
    let to = lab.dir.join("fetched");
    let out = ok(&lab.local(&[
        "sim",
        "m/lockout.toml",
        "--current",
        "--wait",
        "--to",
        to.to_str().unwrap(),
    ]));
    let v = json(&out);
    let id = v["job"]["id"].as_str().unwrap();
    let step = to.join(id).join("sim/default-r1");
    let log = fs::read_to_string(step.join("sim.log")).unwrap();
    let line = log.lines().find(|l| l.starts_with("fake sim data=")).unwrap();
    // The job's own data root holds the driver's mesh and grid; it is
    // gone with the job.
    assert!(
        line.contains(&format!("/runs/{id}/sim-data bots=lockout.0.bots lockout.0.vis ")),
        "{line}"
    );
    assert!(!to.join(id).join("sim-data").exists());
}

#[test]
fn lab_sim_refuses_a_map_the_driver_has_no_nav_mesh_for() {
    let lab = Lab::named("sn");
    write_sim_files(&lab);
    fs::remove_file(lab.dir.join("driver-data/bots/lockout.0.bots")).unwrap();
    let e = err(&lab.local(&["sim", "m/lockout.toml", "--current"]));
    assert!(e.contains("h2 bots nav lockout"), "{e}");
    // A batch names its maps.
    fs::write(
        lab.dev.join("m/maps.toml"),
        "out = \"r\"\nmatch = \"lockout.toml\"\nmaps = [\"ascension\"]\n[a]\n",
    )
    .unwrap();
    let e = err(&lab.local(&["sim", "m/maps.toml", "--current"]));
    assert!(e.contains("h2 bots nav ascension"), "{e}");
    // Nothing reached the host: no job exists.
    let runs = json(&ok(&lab.local(&["runs", "--json"])));
    assert!(runs["runs"].as_array().unwrap().is_empty(), "{runs}");
}

#[test]
fn lab_sim_fails_a_run_whose_bots_had_no_nav_mesh() {
    let lab = Lab::with_setup("simnonav", &[], |root, _| {
        use sim_bench::metrics::{End, Finish, Tally};
        let mut r = Tally::new().report(Finish::bare(End::TickLimit));
        r.nav = "missing".into();
        let dir = root.join("sim-fixtures");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("none.json"), serde_json::to_vec(&r).unwrap()).unwrap();
        r.nav = "loaded".into();
        fs::write(dir.join("navlog.json"), serde_json::to_vec(&r).unwrap()).unwrap();
    });
    write_sim_files(&lab);
    // The report says the mesh was missing.
    let o = lab.local(&["sim", "m/lockout.toml", "--current", "--wait"]);
    assert_eq!(o.status.code(), Some(1));
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert_eq!(json(&out)["job"]["state"], "failed", "{out}");
    assert!(out.contains("1 runs, 1 failed"), "{out}");
    assert!(out.contains("nav mesh missing"), "{out}");
    // Or the log says so, with a report that claims to have loaded it.
    let o = lab.local(&["sim", "m/lockout.toml", "--current", "--wait", "--arm", "x=navlog"]);
    assert_eq!(o.status.code(), Some(1));
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert_eq!(json(&out)["job"]["state"], "failed", "{out}");
    assert!(out.contains("nav mesh none yet"), "{out}");
}
