//! The launchd agents the menu bar drives: writing their plists and
//! `launchctl` for the rest.
//!
//! - start: `bootstrap gui/<uid> <plist>` when the agent is not loaded,
//!   else `kickstart gui/<uid>/<label>`;
//! - stop: `bootout gui/<uid>/<label>` (unloads it, so `KeepAlive` does
//!   not bring it back);
//! - restart: `kickstart -k`;
//! - state: `print gui/<uid>/<label>` (exit 113 when it is not loaded);
//! - run at login: launchd's enable/disable override (`print-disabled`,
//!   `enable`, `disable`). A `KeepAlive` agent starts as soon as it loads
//!   whatever its `RunAtLoad` says, so editing `RunAtLoad` would not keep
//!   it from starting at login; a disabled agent is not loaded at login at
//!   all, and the override leaves the plist (perhaps someone else's)
//!   alone. Starting a disabled agent enables it for the bootstrap and
//!   disables it again: it runs now, not at the next login.
//!
//! Every command goes through [`Runner`], so tests drive a fake launchd.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::config::Agent;

/// What a command printed and how it exited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Runs a program to completion (launchctl, plutil).
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output>;
}

/// The real one.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        let out = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .with_context(|| format!("running {program}"))?;
        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// An agent as launchd sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    /// No plist and not loaded.
    NotInstalled,
    /// A plist, not loaded (stopped).
    Unloaded,
    /// Loaded and running.
    Running { pid: Option<u32> },
    /// Loaded, not running, and it last exited cleanly or never ran
    /// (waiting for launchd to start it again).
    Waiting,
    /// Loaded, not running, and it last exited with an error.
    Failed { last_exit: i32 },
    /// launchctl itself failed.
    Unknown(String),
}

impl ServiceState {
    pub fn is_loaded(&self) -> bool {
        matches!(
            self,
            ServiceState::Running { .. } | ServiceState::Waiting | ServiceState::Failed { .. }
        )
    }

    pub fn is_running(&self) -> bool {
        matches!(self, ServiceState::Running { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            ServiceState::NotInstalled => "not installed".into(),
            ServiceState::Unloaded => "stopped".into(),
            ServiceState::Running { pid: Some(p) } => format!("running (pid {p})"),
            ServiceState::Running { pid: None } => "running".into(),
            ServiceState::Waiting => "starting".into(),
            ServiceState::Failed { last_exit } => format!("failed (exit {last_exit})"),
            ServiceState::Unknown(e) => format!("unknown ({e})"),
        }
    }
}

/// Parses `launchctl print <service>`: the service's own `state`, `pid`
/// and `last exit code` (one tab in; nested blocks are deeper).
pub fn parse_print(text: &str) -> ServiceState {
    let mut state = None;
    let mut pid = None;
    let mut last_exit = None;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix('\t') else {
            continue;
        };
        if rest.starts_with('\t') {
            continue;
        }
        let Some((k, v)) = rest.split_once(" = ") else {
            continue;
        };
        match k.trim() {
            "state" if state.is_none() => state = Some(v.trim().to_string()),
            "pid" if pid.is_none() => pid = v.trim().parse().ok(),
            "last exit code" if last_exit.is_none() => {
                let num: String = v
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                last_exit = num.parse::<i32>().ok();
            }
            _ => {}
        }
    }
    match (state.as_deref(), pid) {
        (Some("running"), pid) => ServiceState::Running { pid },
        (_, Some(pid)) => ServiceState::Running { pid: Some(pid) },
        _ => match last_exit {
            Some(code) if code != 0 => ServiceState::Failed { last_exit: code },
            _ => ServiceState::Waiting,
        },
    }
}

/// Parses `launchctl print-disabled <domain>` for `label`: `Some(true)`
/// when disabled, `Some(false)` when explicitly enabled, `None` when
/// launchd has no override (enabled). Older systems print `true`/`false`.
pub fn parse_disabled(text: &str, label: &str) -> Option<bool> {
    let quoted = format!("\"{label}\"");
    text.lines().find_map(|line| {
        let (k, v) = line.trim().split_once("=>")?;
        if k.trim() != quoted {
            return None;
        }
        match v.trim() {
            "disabled" | "true" => Some(true),
            "enabled" | "false" => Some(false),
            _ => None,
        }
    })
}

/// A LaunchAgent plist to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub label: String,
    pub program_arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub stdout: Option<PathBuf>,
    pub stderr: Option<PathBuf>,
    pub run_at_load: bool,
    pub keep_alive: bool,
    /// `Interactive` for the menu bar (an app the user sees), else none.
    pub process_type: Option<String>,
    /// Only in a logged-in GUI session (the menu bar needs one).
    pub aqua_only: bool,
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

impl AgentSpec {
    /// The plist's XML.
    pub fn to_plist(&self) -> String {
        let mut x = String::from(concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\">\n<dict>\n"
        ));
        let key = |x: &mut String, k: &str| x.push_str(&format!("\t<key>{k}</key>\n"));
        let string = |x: &mut String, k: &str, v: &str| {
            key(x, k);
            x.push_str(&format!("\t<string>{}</string>\n", xml_escape(v)));
        };
        let boolean = |x: &mut String, k: &str, v: bool| {
            key(x, k);
            x.push_str(if v { "\t<true/>\n" } else { "\t<false/>\n" });
        };
        string(&mut x, "Label", &self.label);
        key(&mut x, "ProgramArguments");
        x.push_str("\t<array>\n");
        for a in &self.program_arguments {
            x.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(a)));
        }
        x.push_str("\t</array>\n");
        if !self.environment.is_empty() {
            key(&mut x, "EnvironmentVariables");
            x.push_str("\t<dict>\n");
            for (k, v) in &self.environment {
                x.push_str(&format!(
                    "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                    xml_escape(k),
                    xml_escape(v)
                ));
            }
            x.push_str("\t</dict>\n");
        }
        boolean(&mut x, "RunAtLoad", self.run_at_load);
        boolean(&mut x, "KeepAlive", self.keep_alive);
        if let Some(p) = &self.stdout {
            string(&mut x, "StandardOutPath", &p.to_string_lossy());
        }
        if let Some(p) = &self.stderr {
            string(&mut x, "StandardErrorPath", &p.to_string_lossy());
        }
        if let Some(t) = &self.process_type {
            string(&mut x, "ProcessType", t);
        }
        if self.aqua_only {
            string(&mut x, "LimitLoadToSessionType", "Aqua");
        }
        x.push_str("</dict>\n</plist>\n");
        x
    }
}

/// What the menu bar does to agents; [`Launchd`] is the real one.
pub trait Services: Send + Sync {
    fn state(&self, agent: &Agent) -> ServiceState;
    fn start(&self, agent: &Agent) -> Result<()>;
    fn stop(&self, agent: &Agent) -> Result<()>;
    fn restart(&self, agent: &Agent) -> Result<()>;
    fn run_at_login(&self, agent: &Agent) -> Result<bool>;
    fn set_run_at_login(&self, agent: &Agent, on: bool) -> Result<()>;
    /// Writes `spec` to the agent's plist; `load` bootstraps it as well.
    fn install(&self, agent: &Agent, spec: &AgentSpec, load: bool) -> Result<()>;
    /// Unloads the agent if loaded and removes its plist.
    fn uninstall(&self, agent: &Agent, unload: bool) -> Result<()>;
    /// The plist's `StandardErrorPath` (else `StandardOutPath`).
    fn log_path(&self, agent: &Agent) -> Option<PathBuf>;
}

pub struct Launchd {
    runner: Arc<dyn Runner>,
    /// `gui/<uid>`.
    domain: String,
}

/// The logged-in user's GUI domain.
pub fn gui_domain() -> String {
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0;
    format!("gui/{uid}")
}

impl Launchd {
    pub fn new(runner: Arc<dyn Runner>, domain: String) -> Self {
        Launchd { runner, domain }
    }

    pub fn system() -> Self {
        Self::new(Arc::new(SystemRunner), gui_domain())
    }

    fn target(&self, agent: &Agent) -> String {
        format!("{}/{}", self.domain, agent.label)
    }

    fn launchctl(&self, args: &[&str]) -> Result<Output> {
        self.runner.run("launchctl", args)
    }

    fn checked(&self, args: &[&str]) -> Result<()> {
        let out = self.launchctl(args)?;
        if !out.ok() {
            let why = if out.stderr.trim().is_empty() {
                out.stdout.trim()
            } else {
                out.stderr.trim()
            };
            bail!("launchctl {} failed ({}): {why}", args.join(" "), out.code);
        }
        Ok(())
    }

    fn disabled(&self, agent: &Agent) -> Result<bool> {
        let out = self.launchctl(&["print-disabled", &self.domain])?;
        if !out.ok() {
            bail!("launchctl print-disabled failed ({})", out.code);
        }
        Ok(parse_disabled(&out.stdout, &agent.label).unwrap_or(false))
    }

    fn bootstrap(&self, agent: &Agent) -> Result<()> {
        if !agent.plist.exists() {
            bail!("{} does not exist", agent.plist.display());
        }
        let plist = agent.plist.to_string_lossy();
        let disabled = self.disabled(agent)?;
        let target = self.target(agent);
        if disabled {
            self.checked(&["enable", &target])?;
        }
        let result = self.checked(&["bootstrap", &self.domain, &plist]);
        if disabled {
            // Runs now, still not at login.
            self.checked(&["disable", &target])?;
        }
        result
    }

    fn plist_value(&self, plist: &Path, key: &str) -> Option<String> {
        let out = self
            .runner
            .run(
                "plutil",
                &["-extract", key, "raw", "-o", "-", &plist.to_string_lossy()],
            )
            .ok()?;
        out.ok()
            .then(|| out.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

impl Services for Launchd {
    fn state(&self, agent: &Agent) -> ServiceState {
        match self.launchctl(&["print", &self.target(agent)]) {
            Ok(out) if out.ok() => parse_print(&out.stdout),
            // 113: "Could not find service".
            Ok(out) if out.code == 113 => {
                if agent.plist.exists() {
                    ServiceState::Unloaded
                } else {
                    ServiceState::NotInstalled
                }
            }
            Ok(out) => ServiceState::Unknown(format!("launchctl print exit {}", out.code)),
            Err(e) => ServiceState::Unknown(e.to_string()),
        }
    }

    fn start(&self, agent: &Agent) -> Result<()> {
        match self.state(agent) {
            ServiceState::Running { .. } => Ok(()),
            s if s.is_loaded() => self.checked(&["kickstart", &self.target(agent)]),
            _ => self.bootstrap(agent),
        }
    }

    fn stop(&self, agent: &Agent) -> Result<()> {
        if !self.state(agent).is_loaded() {
            return Ok(());
        }
        self.checked(&["bootout", &self.target(agent)])
    }

    fn restart(&self, agent: &Agent) -> Result<()> {
        if self.state(agent).is_loaded() {
            self.checked(&["kickstart", "-k", &self.target(agent)])
        } else {
            self.bootstrap(agent)
        }
    }

    fn run_at_login(&self, agent: &Agent) -> Result<bool> {
        Ok(agent.plist.exists() && !self.disabled(agent)?)
    }

    fn set_run_at_login(&self, agent: &Agent, on: bool) -> Result<()> {
        if on && !agent.plist.exists() {
            bail!("{} does not exist", agent.plist.display());
        }
        let verb = if on { "enable" } else { "disable" };
        self.checked(&[verb, &self.target(agent)])
    }

    fn install(&self, agent: &Agent, spec: &AgentSpec, load: bool) -> Result<()> {
        if spec.label != agent.label {
            bail!("plist label {} is not {}", spec.label, agent.label);
        }
        if let Some(dir) = agent.plist.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        for log in [&spec.stdout, &spec.stderr].into_iter().flatten() {
            if let Some(dir) = log.parent() {
                fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
        }
        if self.state(agent).is_loaded() {
            self.checked(&["bootout", &self.target(agent)])?;
        }
        let tmp = agent.plist.with_extension("plist.tmp");
        fs::write(&tmp, spec.to_plist()).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &agent.plist)
            .with_context(|| format!("writing {}", agent.plist.display()))?;
        self.checked(&["enable", &self.target(agent)])?;
        if load {
            self.bootstrap(agent)?;
        }
        Ok(())
    }

    fn uninstall(&self, agent: &Agent, unload: bool) -> Result<()> {
        if unload && self.state(agent).is_loaded() {
            self.checked(&["bootout", &self.target(agent)])?;
        }
        match fs::remove_file(&agent.plist) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", agent.plist.display())),
        }
    }

    fn log_path(&self, agent: &Agent) -> Option<PathBuf> {
        if !agent.plist.exists() {
            return None;
        }
        self.plist_value(&agent.plist, "StandardErrorPath")
            .or_else(|| self.plist_value(&agent.plist, "StandardOutPath"))
            .map(PathBuf::from)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A launchd in memory: what `launchctl` would say and do, with every
    //! command recorded. plutil is answered from the plist files the tests
    //! write.
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct World {
        /// label -> pid (None: loaded, not running)
        pub loaded: HashMap<String, Option<u32>>,
        pub disabled: HashSet<String>,
        /// label -> last exit code of a loaded, stopped job
        pub last_exit: HashMap<String, i32>,
        pub calls: Vec<String>,
        next_pid: u32,
    }

    pub struct FakeLaunchd {
        pub world: Mutex<World>,
        pub domain: String,
    }

    impl FakeLaunchd {
        pub fn new() -> Arc<Self> {
            Arc::new(FakeLaunchd {
                world: Mutex::new(World {
                    next_pid: 100,
                    ..World::default()
                }),
                domain: "gui/501".into(),
            })
        }

        pub fn calls(&self) -> Vec<String> {
            std::mem::take(&mut self.world.lock().unwrap().calls)
        }

        fn label_of<'a>(&self, target: &'a str) -> &'a str {
            target
                .strip_prefix(&format!("{}/", self.domain))
                .unwrap_or(target)
        }
    }

    fn out(code: i32, stdout: &str) -> Output {
        Output {
            code,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn label_in_plist(path: &str) -> Option<String> {
        let text = fs::read_to_string(path).ok()?;
        let after = text.split("<key>Label</key>").nth(1)?;
        let s = after.split("<string>").nth(1)?;
        Some(s.split("</string>").next()?.to_string())
    }

    impl Runner for FakeLaunchd {
        fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
            let mut w = self.world.lock().unwrap();
            if program == "plutil" {
                // -extract KEY raw -o - PATH
                let (key, path) = (args[1], args[5]);
                let text = fs::read_to_string(path).unwrap_or_default();
                let needle = format!("<key>{key}</key>");
                let value = text
                    .split(&needle)
                    .nth(1)
                    .and_then(|r| r.split("<string>").nth(1))
                    .and_then(|r| r.split("</string>").next());
                return Ok(match value {
                    Some(v) => out(0, &format!("{v}\n")),
                    None => out(1, ""),
                });
            }
            assert_eq!(program, "launchctl");
            w.calls.push(args.join(" "));
            Ok(match args {
                ["print", target] => {
                    let label = self.label_of(target);
                    match w.loaded.get(label) {
                        None => Output {
                            code: 113,
                            stdout: String::new(),
                            stderr: format!("Could not find service \"{label}\""),
                        },
                        Some(Some(pid)) => out(
                            0,
                            &format!(
                                "{target} = {{\n\tstate = running\n\tpid = {pid}\n\tlast exit code = (never exited)\n}}\n"
                            ),
                        ),
                        Some(None) => {
                            let code = w.last_exit.get(label).copied().unwrap_or(0);
                            out(
                                0,
                                &format!(
                                    "{target} = {{\n\tstate = not running\n\tlast exit code = {code}\n}}\n"
                                ),
                            )
                        }
                    }
                }
                ["print-disabled", _] => {
                    let mut s = String::from("\tdisabled services = {\n");
                    for l in &w.disabled {
                        s.push_str(&format!("\t\t\"{l}\" => disabled\n"));
                    }
                    s.push_str("\t}\n");
                    out(0, &s)
                }
                ["bootstrap", _, plist] => {
                    let Some(label) = label_in_plist(plist) else {
                        return Ok(out(5, ""));
                    };
                    if w.disabled.contains(&label) {
                        return Ok(out(119, ""));
                    }
                    if w.loaded.contains_key(&label) {
                        return Ok(out(5, ""));
                    }
                    w.next_pid += 1;
                    let pid = w.next_pid;
                    w.loaded.insert(label, Some(pid));
                    out(0, "")
                }
                ["bootout", target] => {
                    let label = self.label_of(target).to_string();
                    match w.loaded.remove(&label) {
                        Some(_) => out(0, ""),
                        None => out(3, ""),
                    }
                }
                ["kickstart", rest @ ..] => {
                    let label = self.label_of(rest.last().unwrap()).to_string();
                    let kill = rest.contains(&"-k");
                    w.next_pid += 1;
                    let pid = w.next_pid;
                    match w.loaded.get_mut(&label) {
                        None => out(113, ""),
                        Some(slot) => {
                            if slot.is_none() || kill {
                                *slot = Some(pid);
                            }
                            out(0, "")
                        }
                    }
                }
                ["enable", target] => {
                    let label = self.label_of(target).to_string();
                    w.disabled.remove(&label);
                    out(0, "")
                }
                ["disable", target] => {
                    let label = self.label_of(target).to_string();
                    w.disabled.insert(label);
                    out(0, "")
                }
                other => panic!("unexpected launchctl {other:?}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeLaunchd;
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2-menubar-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spec(label: &str, log: &Path) -> AgentSpec {
        AgentSpec {
            label: label.into(),
            program_arguments: vec![
                "/opt/h2 & co/h2".into(),
                "live".into(),
                "serve".into(),
                "--public-url".into(),
                "https://x.example/<a>".into(),
            ],
            environment: vec![("H2_LIVE_WEB".into(), "/srv/web \"quoted\"".into())],
            stdout: Some(log.into()),
            stderr: Some(log.into()),
            run_at_load: true,
            keep_alive: true,
            process_type: None,
            aqua_only: false,
        }
    }

    #[test]
    fn parses_launchctl_print() {
        let running = "gui/501/x = {\n\tactive count = 1\n\tstate = running\n\tenvironment = {\n\t\tstate = nope\n\t}\n\tpid = 97232\n\tlast exit code = (never exited)\n}\n";
        assert_eq!(
            parse_print(running),
            ServiceState::Running { pid: Some(97232) }
        );
        let failed = "gui/501/x = {\n\tstate = not running\n\truns = 4\n\tlast exit code = 78: EX_CONFIG\n}\n";
        assert_eq!(parse_print(failed), ServiceState::Failed { last_exit: 78 });
        let waiting =
            "gui/501/x = {\n\tstate = not running\n\tlast exit code = (never exited)\n}\n";
        assert_eq!(parse_print(waiting), ServiceState::Waiting);
        let clean = "gui/501/x = {\n\tstate = waiting\n\tlast exit code = 0\n}\n";
        assert_eq!(parse_print(clean), ServiceState::Waiting);
    }

    #[test]
    fn parses_print_disabled() {
        let text = "\tdisabled services = {\n\t\t\"a.b\" => disabled\n\t\t\"c.d\" => enabled\n\t\t\"e.f\" => true\n\t}\n";
        assert_eq!(parse_disabled(text, "a.b"), Some(true));
        assert_eq!(parse_disabled(text, "c.d"), Some(false));
        assert_eq!(parse_disabled(text, "e.f"), Some(true));
        assert_eq!(parse_disabled(text, "a"), None);
    }

    #[test]
    fn plist_has_the_keys_and_escapes() {
        let s = spec("org.h2recomp.live.server", Path::new("/l/server.log")).to_plist();
        assert!(s.contains("<key>Label</key>\n\t<string>org.h2recomp.live.server</string>"));
        assert!(s.contains("<string>/opt/h2 &amp; co/h2</string>"));
        assert!(s.contains("<string>https://x.example/&lt;a&gt;</string>"));
        assert!(
            s.contains("<key>H2_LIVE_WEB</key>\n\t\t<string>/srv/web &quot;quoted&quot;</string>")
        );
        assert!(s.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(s.contains("<key>KeepAlive</key>\n\t<true/>"));
        assert!(s.contains("<key>StandardErrorPath</key>\n\t<string>/l/server.log</string>"));
        assert!(!s.contains("LimitLoadToSessionType"));
    }

    /// plutil accepts the XML and reads back what was written.
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_the_plist() {
        let dir = temp("plutil");
        let mut sp = spec("org.h2recomp.live.menubar", &dir.join("m.log"));
        sp.process_type = Some("Interactive".into());
        sp.aqua_only = true;
        sp.keep_alive = false;
        let path = dir.join("a.plist");
        fs::write(&path, sp.to_plist()).unwrap();
        let p = path.to_str().unwrap();
        let run = |args: &[&str]| SystemRunner.run("plutil", args).unwrap();
        let lint = run(&["-lint", p]);
        assert!(lint.ok(), "{lint:?}");
        let get = |k: &str| run(&["-extract", k, "raw", "-o", "-", p]).stdout;
        assert_eq!(get("Label").trim(), "org.h2recomp.live.menubar");
        assert_eq!(get("ProgramArguments.0").trim(), "/opt/h2 & co/h2");
        assert_eq!(get("ProgramArguments.4").trim(), "https://x.example/<a>");
        assert_eq!(
            get("EnvironmentVariables.H2_LIVE_WEB").trim(),
            "/srv/web \"quoted\""
        );
        assert_eq!(get("RunAtLoad").trim(), "true");
        assert_eq!(get("KeepAlive").trim(), "false");
        assert_eq!(get("LimitLoadToSessionType").trim(), "Aqua");
        assert_eq!(get("ProcessType").trim(), "Interactive");
        // And the real runner reads the log path the same way.
        let agent = Agent {
            label: "org.h2recomp.live.menubar".into(),
            plist: path.clone(),
        };
        let real = Launchd::new(Arc::new(SystemRunner), "gui/0".into());
        assert_eq!(real.log_path(&agent), Some(dir.join("m.log")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn start_stop_restart_against_a_fake_launchd() {
        let dir = temp("sm");
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        let agent = Agent::in_dir(&dir, "org.h2recomp.live.server");

        assert_eq!(ld.state(&agent), ServiceState::NotInstalled);
        assert!(ld.start(&agent).is_err(), "no plist to bootstrap");
        assert!(!ld.run_at_login(&agent).unwrap());

        ld.install(&agent, &spec(&agent.label, &dir.join("s.log")), false)
            .unwrap();
        assert!(agent.plist.exists());
        assert_eq!(ld.state(&agent), ServiceState::Unloaded);
        assert!(ld.run_at_login(&agent).unwrap());
        fake.calls();

        ld.start(&agent).unwrap();
        assert_eq!(
            fake.calls(),
            [
                "print gui/501/org.h2recomp.live.server",
                "print-disabled gui/501",
                &format!("bootstrap gui/501 {}", agent.plist.display()),
            ]
        );
        assert_eq!(ld.state(&agent), ServiceState::Running { pid: Some(101) });

        // Starting a running agent does nothing.
        fake.calls();
        ld.start(&agent).unwrap();
        assert_eq!(fake.calls(), ["print gui/501/org.h2recomp.live.server"]);

        ld.restart(&agent).unwrap();
        assert_eq!(
            fake.calls().last().unwrap(),
            "kickstart -k gui/501/org.h2recomp.live.server"
        );
        assert_eq!(ld.state(&agent), ServiceState::Running { pid: Some(102) });

        // A loaded job that crashed: start kickstarts, not bootstraps.
        {
            let mut w = fake.world.lock().unwrap();
            w.loaded.insert(agent.label.clone(), None);
            w.last_exit.insert(agent.label.clone(), 1);
        }
        assert_eq!(ld.state(&agent), ServiceState::Failed { last_exit: 1 });
        fake.calls();
        ld.start(&agent).unwrap();
        assert_eq!(
            fake.calls().last().unwrap(),
            "kickstart gui/501/org.h2recomp.live.server"
        );
        assert!(ld.state(&agent).is_running());

        ld.stop(&agent).unwrap();
        assert_eq!(ld.state(&agent), ServiceState::Unloaded);
        fake.calls();
        ld.stop(&agent).unwrap();
        assert_eq!(fake.calls(), ["print gui/501/org.h2recomp.live.server"]);

        // Restarting a stopped agent loads it.
        ld.restart(&agent).unwrap();
        assert!(ld.state(&agent).is_running());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_at_login_is_the_enable_override() {
        let dir = temp("login");
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        let agent = Agent::in_dir(&dir, "red.example.server");
        // An adopted agent: its plist is someone else's and stays as is.
        let text = spec(&agent.label, &dir.join("s.log")).to_plist();
        fs::write(&agent.plist, &text).unwrap();

        assert!(ld.run_at_login(&agent).unwrap());
        ld.set_run_at_login(&agent, false).unwrap();
        assert!(!ld.run_at_login(&agent).unwrap());
        assert_eq!(fs::read_to_string(&agent.plist).unwrap(), text);

        // Start while disabled: enabled for the bootstrap, disabled again.
        fake.calls();
        ld.start(&agent).unwrap();
        let calls = fake.calls();
        assert_eq!(
            calls[1..],
            [
                "print-disabled gui/501".to_string(),
                "enable gui/501/red.example.server".into(),
                format!("bootstrap gui/501 {}", agent.plist.display()),
                "disable gui/501/red.example.server".into(),
            ]
        );
        assert!(ld.state(&agent).is_running());
        assert!(!ld.run_at_login(&agent).unwrap());

        ld.set_run_at_login(&agent, true).unwrap();
        assert!(ld.run_at_login(&agent).unwrap());
        assert_eq!(ld.log_path(&agent), Some(dir.join("s.log")));

        // No plist: nothing to load at login.
        let gone = Agent::in_dir(&dir, "gone");
        assert!(ld.set_run_at_login(&gone, true).is_err());
        assert_eq!(ld.log_path(&gone), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_replaces_a_loaded_agent_and_uninstall_removes_it() {
        let dir = temp("inst");
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        let agent = Agent::in_dir(&dir.join("LaunchAgents"), "org.h2recomp.live.menubar");
        let log = dir.join("logs/m.log");
        ld.install(&agent, &spec(&agent.label, &log), true).unwrap();
        assert!(log.parent().unwrap().is_dir());
        assert!(ld.state(&agent).is_running());
        fake.calls();
        ld.install(&agent, &spec(&agent.label, &log), true).unwrap();
        let calls = fake.calls();
        assert!(calls.contains(&"bootout gui/501/org.h2recomp.live.menubar".to_string()));
        assert!(calls.last().unwrap().starts_with("bootstrap"));

        assert!(
            ld.install(&agent, &spec("other", &log), false).is_err(),
            "label mismatch"
        );

        // Without unloading (the menu bar removing its own login item).
        ld.uninstall(&agent, false).unwrap();
        assert!(!agent.plist.exists());
        assert!(ld.state(&agent).is_loaded());
        ld.uninstall(&agent, true).unwrap();
        assert_eq!(ld.state(&agent), ServiceState::NotInstalled);
        fs::remove_dir_all(&dir).unwrap();
    }
}
