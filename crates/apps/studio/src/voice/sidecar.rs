//! The local voice model as a managed Python process: Studio writes the
//! script (`crates/apps/studio/sidecar/voice_clone.py`, built in), makes a
//! venv for it under the caches dir, installs the backend's packages and
//! weights on first use, then runs it and talks to it in JSON lines.
//! docs/editor/voice-cloning.md, "The sidecar".
//!
//! **Protocol.** Studio writes one request per line on the process's
//! stdin, `{"id": n, "op": "...", "params": {...}}`. The process answers
//! with any number of `{"id": n, "progress": {stage, done, total, text?}}`
//! lines and then one `{"id": n, "ok": true, "result": {...}}` or
//! `{"id": n, "ok": false, "error": "..."}`. Ops: `hello`, `prepare`
//! (downloads the weights; the only op run with the network allowed),
//! `say`, `transcribe` and `train`. Its stderr goes to
//! `<root>/<backend>/sidecar.log`.
//!
//! **Where things are.** `<caches>/voice-clone/<backend>/` holds
//! `sidecar.py`, `requirements.txt`, `venv/`, `installed.json` and the
//! logs; `<caches>/voice-clone/hf/` the downloaded weights (`HF_HOME`).
//! Every run but `prepare` sets `HF_HUB_OFFLINE=1`: once installed, the
//! model never reaches the network, and no audio is ever sent anywhere.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use content::Sha256;
use serde_json::{Value, json};

use super::VoiceError;

/// The sidecar's script, as Studio writes it into the venv's directory.
pub const SCRIPT: &str = include_str!("../../sidecar/voice_clone.py");

/// The clone models Studio can run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    /// F5-TTS through its MLX port (`f5-tts-mlx`): zero-shot from a 24 kHz
    /// reference, and the fine-tune.
    #[default]
    F5Mlx,
    /// Qwen3-TTS's base model (`qwen-tts`, PyTorch on MPS): zero-shot
    /// only.
    Qwen3Tts,
}

impl Backend {
    pub const ALL: [Backend; 2] = [Backend::F5Mlx, Backend::Qwen3Tts];

    pub fn id(self) -> &'static str {
        match self {
            Backend::F5Mlx => "f5-mlx",
            Backend::Qwen3Tts => "qwen3-tts",
        }
    }

    pub fn parse(s: &str) -> Result<Backend, VoiceError> {
        Backend::ALL
            .into_iter()
            .find(|b| b.id() == s)
            .ok_or_else(|| VoiceError::Invalid(format!("backend: f5-mlx or qwen3-tts, not {s:?}")))
    }

    /// Its pip requirements.
    pub fn requirements(self) -> &'static str {
        match self {
            Backend::F5Mlx => include_str!("../../sidecar/requirements-f5-mlx.txt"),
            Backend::Qwen3Tts => include_str!("../../sidecar/requirements-qwen3-tts.txt"),
        }
    }

    /// Whether it can be fine-tuned.
    pub fn trains(self) -> bool {
        self == Backend::F5Mlx
    }
}

/// How far a long step is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub stage: String,
    pub done: u64,
    pub total: u64,
    pub text: Option<String>,
}

impl Progress {
    pub fn new(stage: &str, done: u64, total: u64) -> Progress {
        Progress {
            stage: stage.to_owned(),
            done,
            total,
            text: None,
        }
    }

    fn from_json(v: &Value) -> Progress {
        Progress {
            stage: v["stage"].as_str().unwrap_or("working").to_owned(),
            done: v["done"].as_u64().unwrap_or(0),
            total: v["total"].as_u64().unwrap_or(0),
            text: v["text"].as_str().map(|s| s.chars().take(200).collect()),
        }
    }
}

/// A running sidecar.
pub trait Sidecar: Send {
    /// Runs `op` and waits for its answer, passing on its progress.
    fn call(
        &mut self,
        op: &str,
        params: Value,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<Value, VoiceError>;
}

/// Installs and starts sidecars.
pub trait Launcher: Send + Sync {
    /// Whether `backend` is installed (its venv, packages and weights).
    fn installed(&self, backend: Backend) -> bool;
    /// Installs `backend`: the long step, run on a job's thread.
    fn install(
        &self,
        backend: Backend,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<(), VoiceError>;
    /// Starts `backend`'s sidecar, network off.
    fn launch(&self, backend: Backend) -> Result<Box<dyn Sidecar>, VoiceError>;
    /// Where it lives, for status.
    fn dir(&self, backend: Backend) -> PathBuf;
}

/// The stamp of what an install was made from: the script and the
/// requirements. A new Studio with another script reinstalls.
pub fn stamp(backend: Backend) -> String {
    let both = format!("{SCRIPT}\0{}", backend.requirements());
    Sha256::of(both.as_bytes()).to_string()
}

/// The last `n` lines of `path`, for an error.
fn tail(path: &Path, n: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// An executable named `name` on the `PATH`, or in the places Homebrew and
/// the uv installer put one.
fn which(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Studio's launcher: the venvs under `<caches>/voice-clone`.
#[derive(Clone, Debug)]
pub struct Managed {
    root: PathBuf,
}

impl Default for Managed {
    fn default() -> Self {
        Managed::new(agent_proto::cache_dir().join("voice-clone"))
    }
}

impl Managed {
    pub fn new(root: PathBuf) -> Managed {
        Managed { root }
    }

    fn home(&self, b: Backend) -> PathBuf {
        self.root.join(b.id())
    }

    fn python(&self, b: Backend) -> PathBuf {
        self.home(b).join("venv/bin/python")
    }

    /// Runs `cmd` to the end, its stdout lines as progress of `stage`, its
    /// stderr to `log`. An error carries the log's tail.
    fn run(
        &self,
        mut cmd: Command,
        stage: &str,
        log: &Path,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<(), VoiceError> {
        let err =
            File::create(log).map_err(|e| VoiceError::Io(format!("{}: {e}", log.display())))?;
        let what = format!("{:?}", cmd.get_program());
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| VoiceError::Setup(format!("{stage}: could not start {what}: {e}")))?;
        let out = child.stdout.take().expect("piped");
        let mut n = 0;
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            n += 1;
            let mut p = Progress::new(stage, n, 0);
            p.text = Some(line.chars().take(200).collect());
            progress(&p);
        }
        let status = child
            .wait()
            .map_err(|e| VoiceError::Setup(format!("{stage}: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(VoiceError::Setup(format!(
                "{stage} failed ({status}); {}:\n{}",
                log.display(),
                tail(log, 12)
            )))
        }
    }

    fn spawn(&self, b: Backend, online: bool) -> Result<Process, VoiceError> {
        let home = self.home(b);
        let log = home.join("sidecar.log");
        let err = File::options()
            .create(true)
            .append(true)
            .open(&log)
            .map_err(|e| VoiceError::Io(format!("{}: {e}", log.display())))?;
        let mut cmd = Command::new(self.python(b));
        // -I: no user site, no PYTHON* variables, not the script's
        // directory on the path; -u: unbuffered, so progress arrives.
        cmd.args(["-I", "-u"])
            .arg(home.join("sidecar.py"))
            .arg("--backend")
            .arg(b.id())
            .current_dir(&home)
            .env("HF_HOME", self.root.join("hf"))
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("DO_NOT_TRACK", "1")
            .env("TOKENIZERS_PARALLELISM", "false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(err));
        if !online {
            cmd.env("HF_HUB_OFFLINE", "1")
                .env("TRANSFORMERS_OFFLINE", "1");
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| VoiceError::Setup(format!("the voice sidecar did not start: {e}")))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Process {
            child,
            stdin,
            stdout,
            next: 1,
            log,
        })
    }
}

impl Launcher for Managed {
    fn installed(&self, b: Backend) -> bool {
        let Ok(bytes) = std::fs::read(self.home(b).join("installed.json")) else {
            return false;
        };
        let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        self.python(b).is_file() && v["stamp"].as_str() == Some(stamp(b).as_str())
    }

    fn install(&self, b: Backend, progress: &mut dyn FnMut(&Progress)) -> Result<(), VoiceError> {
        let home = self.home(b);
        let io = |p: &Path, e: std::io::Error| VoiceError::Io(format!("{}: {e}", p.display()));
        std::fs::create_dir_all(&home).map_err(|e| io(&home, e))?;
        let script = home.join("sidecar.py");
        std::fs::write(&script, SCRIPT).map_err(|e| io(&script, e))?;
        let reqs = home.join("requirements.txt");
        std::fs::write(&reqs, b.requirements()).map_err(|e| io(&reqs, e))?;
        let _ = std::fs::remove_file(home.join("installed.json"));
        let uv = which("uv");
        let venv = home.join("venv");

        // 1. The venv: uv's Python 3.12 when uv is there, else python3's.
        progress(&Progress::new("venv", 0, 3));
        if !self.python(b).is_file() {
            let mut cmd = match &uv {
                Some(uv) => {
                    let mut c = Command::new(uv);
                    c.args(["venv", "--python", "3.12"]).arg(&venv);
                    c
                }
                None => {
                    let py = which("python3").ok_or_else(|| {
                        VoiceError::Setup(
                            "the local voice needs uv (https://docs.astral.sh/uv/) or python3 3.10 or later".into(),
                        )
                    })?;
                    let mut c = Command::new(py);
                    c.args(["-m", "venv"]).arg(&venv);
                    c
                }
            };
            cmd.current_dir(&home);
            self.run(cmd, "venv", &home.join("install-venv.log"), progress)?;
        }

        // 2. The packages.
        progress(&Progress::new("packages", 1, 3));
        let mut cmd = match &uv {
            Some(uv) => {
                let mut c = Command::new(uv);
                c.args(["pip", "install", "--python"])
                    .arg(self.python(b))
                    .arg("-r")
                    .arg(&reqs);
                c
            }
            None => {
                let mut c = Command::new(self.python(b));
                c.args(["-m", "pip", "install", "-r"]).arg(&reqs);
                c
            }
        };
        cmd.current_dir(&home);
        self.run(
            cmd,
            "packages",
            &home.join("install-packages.log"),
            progress,
        )?;

        // 3. The weights: the model's and the Whisper's, downloaded once.
        progress(&Progress::new("weights", 2, 3));
        let mut p = self.spawn(b, true)?;
        p.call("prepare", json!({}), progress)?;
        drop(p);

        let done = json!({"stamp": stamp(b), "backend": b.id()});
        let at = home.join("installed.json");
        std::fs::write(&at, done.to_string()).map_err(|e| io(&at, e))?;
        progress(&Progress::new("installed", 3, 3));
        Ok(())
    }

    fn launch(&self, b: Backend) -> Result<Box<dyn Sidecar>, VoiceError> {
        if !self.installed(b) {
            return Err(VoiceError::Setup(format!(
                "the local voice model ({}) is not installed",
                b.id()
            )));
        }
        Ok(Box::new(self.spawn(b, false)?))
    }

    fn dir(&self, b: Backend) -> PathBuf {
        self.home(b)
    }
}

/// A sidecar process.
pub struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    log: PathBuf,
}

impl Process {
    fn stopped(&self, why: &str) -> VoiceError {
        VoiceError::Model(format!(
            "the voice sidecar {why}; {}:\n{}",
            self.log.display(),
            tail(&self.log, 12)
        ))
    }
}

/// Reads answers to request `id` from `lines` until its result.
pub fn read_answer(
    id: u64,
    lines: &mut dyn BufRead,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Option<Value>, VoiceError> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = lines
            .read_line(&mut line)
            .map_err(|e| VoiceError::Model(format!("reading the voice sidecar: {e}")))?;
        if n == 0 {
            return Ok(None);
        }
        // Anything that is not one of our lines (a library printing) is
        // passed over.
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v["id"].as_u64() != Some(id) {
            continue;
        }
        if let Some(p) = v.get("progress") {
            progress(&Progress::from_json(p));
            continue;
        }
        return match v["ok"].as_bool() {
            Some(true) => Ok(Some(v.get("result").cloned().unwrap_or(Value::Null))),
            _ => Err(VoiceError::Model(
                v["error"]
                    .as_str()
                    .unwrap_or("the voice sidecar failed without saying why")
                    .chars()
                    .take(600)
                    .collect(),
            )),
        };
    }
}

impl Sidecar for Process {
    fn call(
        &mut self,
        op: &str,
        params: Value,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<Value, VoiceError> {
        let id = self.next;
        self.next += 1;
        let req = json!({"id": id, "op": op, "params": params});
        writeln!(self.stdin, "{req}")
            .and_then(|()| self.stdin.flush())
            .map_err(|_| self.stopped("did not take the request"))?;
        match read_answer(id, &mut self.stdout, progress)? {
            Some(v) => Ok(v),
            None => Err(self.stopped("stopped")),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
