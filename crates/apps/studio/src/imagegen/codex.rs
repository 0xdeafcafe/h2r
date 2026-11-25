//! The Codex CLI as an image source: ChatGPT's image generation on the
//! user's own ChatGPT sign-in, with no API key (docs/editor/agent-api.md,
//! "Generating images").
//!
//! A generation is one non-interactive run:
//!
//! ```text
//! codex exec --image ref1.png --image ref2.png \
//!     --json --skip-git-repo-check --sandbox workspace-write \
//!     --cd <job dir> --output-last-message <job dir>/codex-last-message.txt
//! ```
//!
//! with the prompt on stdin (`--image` takes many values, so a prompt
//! given as an argument after it would be read as an image). The prompt
//! asks Codex to make one picture with its image generation tool and copy
//! it to `out.png` in the job's directory, the run's working root. The
//! picture is taken from the first of:
//!
//! 1. `out.png` in the job's directory;
//! 2. a `saved_path` (or any `.../generated_images/...png` path) in the
//!    run's JSON events;
//! 3. the newest PNG in `$CODEX_HOME/generated_images/<thread id>/`, the
//!    thread id from the run's `thread.started` event (Codex saves each
//!    generated picture there as `<call id>.png`);
//! 4. a PNG path in the last message.
//!
//! Before the run, `codex --version` (missing: [`GenError::Missing`]) and
//! `codex login status` (signed out: [`GenError::Unauthenticated`]). A run
//! that fails says why in an `error` or `turn.failed` event: an allowance
//! used up is [`GenError::Quota`], a sign-in problem
//! [`GenError::Unauthenticated`]. Studio never reads Codex's credentials.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{GenError, GenRequest, Generated, ImageSource};

/// How long one generation may take.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// The file the prompt asks Codex to copy its picture to.
pub const OUT_FILE: &str = "out.png";

const INSTALL: &str = "install the Codex CLI (`npm install -g @openai/codex` or `brew install codex`), then run `codex login` and sign in with ChatGPT";

/// The Codex CLI, run as a child process.
#[derive(Clone, Debug)]
pub struct CodexCli {
    /// The program: `codex` on `PATH` by default.
    pub program: PathBuf,
    /// Codex's home (`$CODEX_HOME`, else `~/.codex`): where it saves
    /// generated pictures.
    pub home: Option<PathBuf>,
    /// `--model`, when set.
    pub model: Option<String>,
    pub timeout: Duration,
}

impl Default for CodexCli {
    fn default() -> Self {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")));
        CodexCli {
            program: PathBuf::from("codex"),
            home,
            model: None,
            timeout: TIMEOUT,
        }
    }
}

/// What a run's `--json` events say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Events {
    pub thread_id: Option<String>,
    /// `error` and `turn.failed` messages, in order.
    pub errors: Vec<String>,
    /// Picture paths named anywhere in the events.
    pub saved: Vec<PathBuf>,
    /// The last agent message's text.
    pub last_message: Option<String>,
    /// The generator's rewritten prompt, when an event carries one.
    pub revised_prompt: Option<String>,
}

/// Reads `codex exec --json` output: one JSON object per line; lines that
/// are not JSON are skipped.
pub fn parse_events(stdout: &str) -> Events {
    let mut ev = Events::default();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match v["type"].as_str() {
            Some("thread.started") => {
                ev.thread_id = v["thread_id"].as_str().map(str::to_owned);
            }
            Some("error") => {
                if let Some(m) = v["message"].as_str() {
                    ev.errors.push(m.to_owned());
                }
            }
            Some("turn.failed") => {
                if let Some(m) = v["error"]["message"].as_str()
                    && !ev.errors.iter().any(|e| e == m)
                {
                    ev.errors.push(m.to_owned());
                }
            }
            Some("item.completed") if v["item"]["type"] == "agent_message" => {
                ev.last_message = v["item"]["text"].as_str().map(str::to_owned);
            }
            _ => {}
        }
        if let Some(p) = find_key(&v, "revised_prompt").and_then(Value::as_str) {
            ev.revised_prompt = Some(p.to_owned());
        }
        collect_paths(&v, &mut ev.saved);
    }
    ev
}

fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o
            .get(key)
            .or_else(|| o.values().find_map(|x| find_key(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
        _ => None,
    }
}

/// Every `saved_path`, and every string that is a path to a PNG under a
/// `generated_images` directory.
fn collect_paths(v: &Value, out: &mut Vec<PathBuf>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                match x.as_str() {
                    Some(s) if k == "saved_path" || is_generated_png(s) => {
                        let p = PathBuf::from(s);
                        if !out.contains(&p) {
                            out.push(p);
                        }
                    }
                    _ => collect_paths(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_paths(x, out)),
        _ => {}
    }
}

fn is_generated_png(s: &str) -> bool {
    s.len() < 4096
        && !s.contains('\n')
        && s.contains("generated_images")
        && s.to_ascii_lowercase().ends_with(".png")
}

/// The kind of failure a message from Codex describes.
pub fn classify(message: &str) -> GenError {
    let m = message.to_ascii_lowercase();
    let quota = ["usage limit", "rate limit", "quota", "purchase more credits", "insufficient_quota"];
    let auth = [
        "not logged in",
        "log in again",
        "login again",
        "sign in",
        "unauthorized",
        "401",
        "token expired",
        "refresh token",
        "authentication",
    ];
    if quota.iter().any(|k| m.contains(k)) {
        GenError::Quota(format!("ChatGPT's image generation is out of allowance: {message}"))
    } else if auth.iter().any(|k| m.contains(k)) {
        GenError::Unauthenticated(format!(
            "Codex is not signed in: run `codex login` and sign in with ChatGPT ({message})"
        ))
    } else {
        GenError::Failed(format!("codex failed: {message}"))
    }
}

/// The prompt Codex is given: the job's prompt inside what Codex is to do
/// with it.
pub fn instructions(req: &GenRequest) -> String {
    let size = match req.size {
        Some((w, h)) if w > h * 5 / 4 => " Make it landscape (1536x1024).",
        Some((w, h)) if h > w * 5 / 4 => " Make it portrait (1024x1536).",
        Some(_) => " Make it square (1024x1024).",
        None => "",
    };
    format!(
        "Use your image generation tool to make exactly one image.{size} Do not write code, browse, or change any file except as said here.\n\n\
         The image:\n{prompt}\n\n\
         When it is made, copy the generated PNG file to ./{OUT_FILE} in the working directory and reply with only its absolute path.",
        prompt = req.prompt.trim(),
    )
}

/// The arguments of `codex exec` for `req`: the images first, so the
/// flags after them end the image list; the prompt goes on stdin.
pub fn exec_args(req: &GenRequest, model: Option<&str>) -> Vec<String> {
    let mut a = vec!["exec".to_owned()];
    for r in &req.references {
        a.push("--image".into());
        a.push(r.display().to_string());
    }
    a.extend(
        [
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "workspace-write",
            "--color",
            "never",
        ]
        .map(str::to_owned),
    );
    a.push("--cd".into());
    a.push(req.work_dir.display().to_string());
    a.push("--output-last-message".into());
    a.push(req.work_dir.join("codex-last-message.txt").display().to_string());
    if let Some(m) = model {
        a.push("--model".into());
        a.push(m.to_owned());
    }
    a
}

/// Runs `program args`, feeding `stdin`, for at most `timeout`: exit
/// success, stdout and stderr.
fn run(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<(bool, String, String), GenError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GenError::Missing(format!(
                "`{}` was not found: {INSTALL}",
                program.display()
            ))
        } else {
            GenError::Failed(format!("could not run {}: {e}", program.display()))
        }
    })?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // A closed pipe (the child exited early) shows in its output.
        let _ = pipe.write_all(text.as_bytes());
    }
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GenError::Failed(format!(
                    "{} took longer than {} s and was stopped",
                    program.display(),
                    timeout.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(GenError::Failed(format!("{}: {e}", program.display()))),
        }
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    Ok((status.success(), out, err))
}

fn is_png(p: &Path) -> bool {
    std::fs::read(p)
        .map(|b| b.starts_with(b"\x89PNG\r\n\x1a\n"))
        .unwrap_or(false)
}

impl CodexCli {
    /// Codex is installed and signed in: its version line.
    pub fn check(&self) -> Result<String, GenError> {
        let short = Duration::from_secs(30);
        let (ok, out, err) = run(&self.program, &["--version".into()], None, None, short)?;
        if !ok {
            return Err(GenError::Missing(format!(
                "`{} --version` failed ({}): {INSTALL}",
                self.program.display(),
                err.trim()
            )));
        }
        let version = out.trim().to_owned();
        let (ok, out, err) = run(
            &self.program,
            &["login".into(), "status".into()],
            None,
            None,
            short,
        )?;
        let said = format!("{} {}", out.trim(), err.trim()).trim().to_owned();
        if !ok || said.to_ascii_lowercase().contains("not logged in") {
            return Err(GenError::Unauthenticated(format!(
                "Codex is not signed in: run `codex login` and sign in with ChatGPT (codex login status: {})",
                if said.is_empty() { "no answer" } else { &said }
            )));
        }
        Ok(version)
    }

    /// The picture the run made, by the order in the module's notes.
    pub fn collect(&self, work_dir: &Path, ev: &Events) -> Option<PathBuf> {
        let out = work_dir.join(OUT_FILE);
        if is_png(&out) {
            return Some(out);
        }
        if let Some(p) = ev.saved.iter().rev().find(|p| is_png(p)) {
            return Some(p.clone());
        }
        if let (Some(home), Some(thread)) = (&self.home, &ev.thread_id) {
            let dir = home.join("generated_images").join(thread);
            let newest = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
                .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
                .max();
            if let Some((_, p)) = newest {
                return Some(p);
            }
        }
        let last = ev.last_message.clone().or_else(|| {
            std::fs::read_to_string(work_dir.join("codex-last-message.txt")).ok()
        })?;
        last.split_whitespace()
            .map(|w| w.trim_matches(|c: char| "`'\"()<>[],".contains(c)))
            .filter(|w| w.to_ascii_lowercase().ends_with(".png"))
            .map(PathBuf::from)
            .find(|p| is_png(p))
    }
}

impl ImageSource for CodexCli {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn generate(&self, req: &GenRequest) -> Result<Generated, GenError> {
        let version = self.check()?;
        for r in &req.references {
            if !r.is_file() {
                return Err(GenError::Failed(format!(
                    "reference {} is not a file",
                    r.display()
                )));
            }
        }
        let args = exec_args(req, self.model.as_deref());
        let (ok, out, err) = run(
            &self.program,
            &args,
            Some(&req.work_dir),
            Some(&instructions(req)),
            self.timeout,
        )?;
        // Kept for a look when a run goes wrong; never credentials.
        let _ = std::fs::write(req.work_dir.join("codex-events.jsonl"), &out);
        let ev = parse_events(&out);
        if let Some(png) = self.collect(&req.work_dir, &ev) {
            return Ok(Generated {
                png,
                generator: format!("{version} (ChatGPT image generation)"),
                revised_prompt: ev.revised_prompt,
            });
        }
        if let Some(m) = ev.errors.last() {
            return Err(classify(m));
        }
        if !ok {
            let lines: Vec<&str> = err.lines().collect();
            let tail = lines[lines.len().saturating_sub(5)..].join(" ");
            return Err(classify(if tail.trim().is_empty() { "codex exited with an error" } else { tail.trim() }));
        }
        Err(GenError::NoImage(format!(
            "codex finished without an image{}",
            ev.last_message
                .map(|m| format!(": it said {:?}", m.chars().take(300).collect::<String>()))
                .unwrap_or_default()
        )))
    }
}
