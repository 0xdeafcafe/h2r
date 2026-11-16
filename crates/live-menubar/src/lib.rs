//! `h2 live menubar`: an optional menu bar (tray) app for a Live server
//! run on this machine by launchd (docs/live/live-hosting.md "Menu bar
//! app").
//!
//! - [`config`]: `menubar.toml`, the agents it manages;
//! - [`launchd`]: plists and `launchctl`, behind [`launchd::Services`];
//! - [`status`]: one [`status::Snapshot`] of the agents and the server;
//! - [`menu`]: the menu as data; [`icon`]: the template icon;
//! - [`actions`]: what each item does; [`app`]: the tray and event loop.
//!
//! macOS is complete. The tray runs on Windows and Linux too (tray-icon),
//! but the agents are launchd's, so there the service items fail with a
//! message rather than doing anything.

pub mod actions;
pub mod app;
pub mod config;
pub mod icon;
pub mod launchd;
pub mod menu;
pub mod status;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::actions::{Desktop, SystemDesktop, menubar_spec};
use crate::config::{Config, Paths};
use crate::launchd::{AgentSpec, Launchd, Runner, Services, SystemRunner};
use crate::status::Backend;

pub const USAGE: &str = "usage: h2 live menubar [--config FILE] [command]

  (no command)            run the menu bar app (no Dock icon)
  --status                print what the menu would show, once, and exit
  --install               open the menu bar app at login (writes its
                          LaunchAgent; it starts at the next login)
  --uninstall             stop opening it at login (removes that agent)
  --adopt-existing SERVER TUNNEL
                          manage existing LaunchAgents by label (TUNNEL may
                          be `none`); their plists are read, never
                          rewritten. Their --public-url, --data and
                          --listen fill in menubar.toml
  --install-server [-- SERVE FLAGS...]
                          write and load a server agent running
                          `h2 live serve SERVE FLAGS` (KeepAlive, logs in
                          live/logs/server.log)
  --install-tunnel --token-file FILE [--cloudflared PATH]
                          write and load a cloudflared agent for a named
                          tunnel to the local URL
  --public-url URL | --data DIR | --local-url URL
                          set these in menubar.toml (with any command, or
                          alone)

menubar.toml defaults to ~/Library/Application Support/h2recomp/live/
menubar.toml.";

/// Everything the commands touch, so tests pass fakes.
pub struct Env {
    pub paths: Paths,
    pub runner: Arc<dyn Runner>,
    pub services: Arc<dyn Services>,
    pub backend: Arc<dyn Backend>,
    pub desktop: Arc<dyn Desktop>,
    /// This program (`h2`).
    pub exe: PathBuf,
}

impl Env {
    /// The real machine, with the CLI's operator calls.
    pub fn system(backend: Arc<dyn Backend>) -> Result<Self> {
        Ok(Env {
            paths: Paths::from_home()?,
            runner: Arc::new(SystemRunner),
            services: Arc::new(Launchd::system()),
            backend,
            desktop: Arc::new(SystemDesktop),
            exe: std::env::current_exe().context("finding this program")?,
        })
    }
}

/// What `h2 live serve`'s flags say about where it is.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ServeFacts {
    pub public_url: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub local_url: Option<String>,
}

/// Reads `--public-url`, `--data` and `--listen` from a server's
/// arguments (`h2-live-server ...` or `h2 live serve ...`).
pub fn serve_facts(args: &[String]) -> ServeFacts {
    let mut f = ServeFacts::default();
    let mut tls = false;
    let mut listen = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let mut value = || inline.clone().or_else(|| it.next().cloned());
        match flag {
            "--public-url" => f.public_url = value(),
            "--data" => f.data_dir = value().map(PathBuf::from),
            "--listen" => listen = value(),
            "--tls-cert" => tls = true,
            _ => {}
        }
    }
    if let Some(addr) = listen {
        let addr = addr
            .replace("0.0.0.0", "127.0.0.1")
            .replace("[::]", "[::1]");
        let scheme = if tls { "https" } else { "http" };
        f.local_url = Some(format!("{scheme}://{addr}"));
    }
    f
}

fn plist_arguments(runner: &dyn Runner, plist: &Path) -> Result<Vec<String>> {
    let out = runner.run(
        "plutil",
        &[
            "-extract",
            "ProgramArguments",
            "json",
            "-o",
            "-",
            &plist.to_string_lossy(),
        ],
    )?;
    if !out.ok() {
        bail!(
            "{} has no ProgramArguments: {}",
            plist.display(),
            out.stderr.trim()
        );
    }
    serde_json::from_str(out.stdout.trim())
        .with_context(|| format!("reading ProgramArguments of {}", plist.display()))
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// `h2 live menubar ...`.
pub fn main(args: Vec<String>, env: &Env) -> Result<()> {
    let mut args = args;
    // Everything after `--` is the server's, flags it shares with ours
    // (`--data`, `--public-url`) included.
    let serve_args: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => args.drain(i..).skip(1).collect(),
        None => Vec::new(),
    };
    let mut take = |name: &str, n: usize| -> Result<Option<Vec<String>>> {
        let Some(i) = args.iter().position(|a| a == name) else {
            return Ok(None);
        };
        if i + n >= args.len() {
            bail!("{name} needs {n} value(s)\n\n{USAGE}");
        }
        let v: Vec<String> = args.drain(i..=i + n).skip(1).collect();
        Ok(Some(v))
    };
    let config_flag = take("--config", 1)?.map(|v| PathBuf::from(&v[0]));
    let public_url = take("--public-url", 1)?.map(|v| v[0].clone());
    let data = take("--data", 1)?.map(|v| PathBuf::from(&v[0]));
    let local_url = take("--local-url", 1)?.map(|v| v[0].clone());
    let adopt = take("--adopt-existing", 2)?;
    let settings_given = public_url.is_some() || data.is_some() || local_url.is_some();
    let adopting = adopt.is_some();
    let token_file = take("--token-file", 1)?.map(|v| PathBuf::from(&v[0]));
    let cloudflared = take("--cloudflared", 1)?.map(|v| PathBuf::from(&v[0]));
    let mut flag = |name: &str| match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    };
    let status = flag("--status");
    let install = flag("--install");
    let uninstall = flag("--uninstall");
    let install_server = flag("--install-server");
    let install_tunnel = flag("--install-tunnel");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        bail!("{USAGE}");
    }
    if let Some(bad) = args.first() {
        bail!("unexpected {bad}\n\n{USAGE}");
    }
    if !serve_args.is_empty() && !install_server {
        bail!("arguments after -- go with --install-server\n\n{USAGE}");
    }

    let path = config_flag.unwrap_or_else(|| env.paths.config_file());
    let mut cfg = Config::load(&path, &env.paths)?;
    let before = cfg.clone();

    if let Some(v) = adopt {
        let tunnel = (v[1] != "none").then_some(v[1].as_str());
        cfg.adopt(&env.paths.launch_agents, &v[0], tunnel);
        for agent in std::iter::once(&cfg.server).chain(cfg.tunnel.as_ref()) {
            if !agent.plist.exists() {
                bail!("{} does not exist", agent.plist.display());
            }
        }
        let facts = serve_facts(&plist_arguments(env.runner.as_ref(), &cfg.server.plist)?);
        cfg.public_url = facts.public_url.or(cfg.public_url);
        cfg.data_dir = facts.data_dir.unwrap_or(cfg.data_dir);
        cfg.local_url = facts.local_url.unwrap_or(cfg.local_url);
        if let Some(log) = env.services.log_path(&cfg.server)
            && let Some(dir) = log.parent()
        {
            cfg.logs_dir = dir.into();
        }
        println!(
            "managing {} and {}",
            cfg.server.label,
            cfg.tunnel
                .as_ref()
                .map_or("no tunnel", |t| t.label.as_str())
        );
    }
    if install_server {
        let facts = serve_facts(&serve_args);
        cfg.public_url = facts.public_url.or(cfg.public_url);
        cfg.data_dir = facts.data_dir.unwrap_or(cfg.data_dir);
        cfg.local_url = facts.local_url.unwrap_or(cfg.local_url);
    }
    if let Some(u) = public_url {
        cfg.public_url = Some(u).filter(|u| !u.is_empty());
    }
    if let Some(d) = data {
        cfg.data_dir = d;
    }
    if let Some(u) = local_url {
        cfg.local_url = u;
    }
    if cfg != before || !path.exists() {
        cfg.save(&path)?;
        println!("wrote {}", path.display());
    }

    let services = env.services.as_ref();
    if install_server {
        let log = cfg.logs_dir.join("server.log");
        let mut program = vec![
            env.exe.to_string_lossy().into_owned(),
            "live".into(),
            "serve".into(),
        ];
        program.extend(serve_args);
        let spec = AgentSpec {
            label: cfg.server.label.clone(),
            program_arguments: program,
            environment: Vec::new(),
            stdout: Some(log.clone()),
            stderr: Some(log),
            run_at_load: true,
            keep_alive: true,
            process_type: None,
            aqua_only: false,
        };
        services.install(&cfg.server, &spec, true)?;
        println!("wrote and loaded {}", cfg.server.plist.display());
    }
    if install_tunnel {
        let tunnel = cfg
            .tunnel
            .clone()
            .context("menubar.toml has no tunnel agent")?;
        let token = token_file.context("--install-tunnel needs --token-file FILE")?;
        let cloudflared = match cloudflared.or_else(|| which("cloudflared")) {
            Some(c) => c,
            None => bail!("cloudflared not found; pass --cloudflared PATH"),
        };
        let log = cfg.logs_dir.join("tunnel.log");
        let spec = AgentSpec {
            label: tunnel.label.clone(),
            program_arguments: vec![
                cloudflared.to_string_lossy().into_owned(),
                "tunnel".into(),
                "--no-autoupdate".into(),
                "--url".into(),
                cfg.local_url.clone(),
                "run".into(),
                "--token-file".into(),
                token.to_string_lossy().into_owned(),
            ],
            environment: Vec::new(),
            stdout: Some(log.clone()),
            stderr: Some(log),
            run_at_load: true,
            keep_alive: true,
            process_type: None,
            aqua_only: false,
        };
        services.install(&tunnel, &spec, true)?;
        println!("wrote and loaded {}", tunnel.plist.display());
    }
    if install {
        services.install(&cfg.menubar, &menubar_spec(&cfg, &env.exe), false)?;
        println!(
            "wrote {}; the menu bar opens at login (run `h2 live menubar` to open it now)",
            cfg.menubar.plist.display()
        );
    }
    if uninstall {
        // Unloading would quit a running menu bar; removing the plist is
        // enough to stop the next login from opening it.
        services.uninstall(&cfg.menubar, false)?;
        println!("removed {}", cfg.menubar.plist.display());
    }
    if status {
        let snap = status::collect(&cfg, services, env.backend.as_ref());
        for e in menu::model(&snap, &cfg) {
            if let menu::Entry::Item(i) = e
                && !i.enabled
                && i.checked.is_none()
                && matches!(
                    i.id,
                    menu::ItemId::Server
                        | menu::ItemId::Numbers
                        | menu::ItemId::Tunnel
                        | menu::ItemId::Message
                )
            {
                println!("{}", i.label);
            }
        }
        println!(
            "run server at login: {}",
            snap.server_at_login
                .map_or("unknown", |on| if on { "on" } else { "off" })
        );
        println!(
            "open menu bar at login: {}",
            if snap.menubar_at_login { "on" } else { "off" }
        );
    }
    let command = status
        || install
        || uninstall
        || install_server
        || install_tunnel
        || adopting
        || settings_given;
    if command {
        return Ok(());
    }
    app::run(
        cfg,
        env.services.clone(),
        env.backend.clone(),
        env.desktop.clone(),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::actions::tests::FakeDesktop;
    use crate::launchd::fake::FakeLaunchd;
    use crate::status::tests::FakeBackend;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn serve_flags_name_the_server() {
        let f = serve_facts(&s(&[
            "/x/h2-live-server",
            "--data",
            "/d/server",
            "--db",
            "postgres://localhost/h2live",
            "--listen",
            "127.0.0.1:8074",
            "--public-url",
            "https://live.example.net",
            "--behind-cloudflared",
        ]));
        assert_eq!(
            f,
            ServeFacts {
                public_url: Some("https://live.example.net".into()),
                data_dir: Some("/d/server".into()),
                local_url: Some("http://127.0.0.1:8074".into()),
            }
        );
        let f = serve_facts(&s(&[
            "--listen=0.0.0.0:9000",
            "--tls-cert",
            "c.pem",
            "--tls-key",
            "k.pem",
        ]));
        assert_eq!(f.local_url.as_deref(), Some("https://127.0.0.1:9000"));
        assert_eq!(serve_facts(&s(&["--private"])), ServeFacts::default());
    }

    /// The real plutil reads ProgramArguments back as JSON.
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_reads_program_arguments() {
        let dir = std::env::temp_dir().join(format!("h2-menubar-args-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let spec = AgentSpec {
            label: "x".into(),
            program_arguments: s(&[
                "/a b/h2-live-server",
                "--data",
                "/d & e",
                "--listen",
                "127.0.0.1:8074",
            ]),
            environment: Vec::new(),
            stdout: None,
            stderr: None,
            run_at_load: true,
            keep_alive: true,
            process_type: None,
            aqua_only: false,
        };
        let plist = dir.join("x.plist");
        fs::write(&plist, spec.to_plist()).unwrap();
        let args = plist_arguments(&SystemRunner, &plist).unwrap();
        assert_eq!(args, spec.program_arguments);
        assert_eq!(serve_facts(&args).data_dir, Some("/d & e".into()));
        fs::remove_dir_all(&dir).unwrap();
    }

    struct World {
        root: PathBuf,
        fake: Arc<FakeLaunchd>,
        env: Env,
    }

    fn world(name: &str) -> World {
        let root =
            std::env::temp_dir().join(format!("h2-menubar-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths {
            live_dir: root.join("live"),
            launch_agents: root.join("LaunchAgents"),
        };
        fs::create_dir_all(&paths.launch_agents).unwrap();
        let fake = FakeLaunchd::new();
        let env = Env {
            paths,
            runner: fake.clone(),
            services: Arc::new(Launchd::new(fake.clone(), fake.domain.clone())),
            backend: Arc::new(FakeBackend::default()),
            desktop: Arc::new(FakeDesktop::default()),
            exe: "/Users/me/bin/h2".into(),
        };
        World { root, fake, env }
    }

    /// A plist like the hand-written agents this was made for, in the
    /// shape the fake plutil can read (ProgramArguments as JSON too).
    fn existing_agent(w: &World, label: &str, args: &[&str], log: &str) {
        let spec = AgentSpec {
            label: label.into(),
            program_arguments: s(args),
            environment: vec![("H2_LIVE_WEB".into(), "/srv/web".into())],
            stdout: Some(log.into()),
            stderr: Some(log.into()),
            run_at_load: true,
            keep_alive: true,
            process_type: None,
            aqua_only: false,
        };
        let plist = w.env.paths.launch_agents.join(format!("{label}.plist"));
        fs::write(&plist, spec.to_plist()).unwrap();
        fs::write(
            plist.with_extension("args.json"),
            serde_json::to_string(&args).unwrap(),
        )
        .unwrap();
    }

    /// The fake plutil answers `-extract ProgramArguments json` from the
    /// `.args.json` beside the plist.
    struct ArgsRunner(Arc<FakeLaunchd>);
    impl Runner for ArgsRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<launchd::Output> {
            if program == "plutil" && args[1] == "ProgramArguments" {
                let p = Path::new(args[5]).with_extension("args.json");
                return Ok(launchd::Output {
                    code: 0,
                    stdout: fs::read_to_string(p)?,
                    stderr: String::new(),
                });
            }
            self.0.run(program, args)
        }
    }

    #[test]
    fn adopting_existing_agents() {
        let mut w = world("adopt");
        w.env.runner = Arc::new(ArgsRunner(w.fake.clone()));
        let logs = w.root.join("live/logs");
        existing_agent(
            &w,
            "red.example.h2live.server",
            &[
                "/x/h2-live-server",
                "--data",
                "/x/live/server",
                "--listen",
                "127.0.0.1:8074",
                "--public-url",
                "https://live.example.net",
            ],
            &logs.join("server.log").to_string_lossy(),
        );
        existing_agent(
            &w,
            "red.example.h2live.tunnel",
            &["/opt/homebrew/bin/cloudflared", "tunnel", "run"],
            &logs.join("tunnel.log").to_string_lossy(),
        );
        let server_plist = fs::read(
            w.env
                .paths
                .launch_agents
                .join("red.example.h2live.server.plist"),
        )
        .unwrap();

        main(
            s(&[
                "--adopt-existing",
                "red.example.h2live.server",
                "red.example.h2live.tunnel",
            ]),
            &w.env,
        )
        .unwrap();
        let cfg = Config::load(&w.env.paths.config_file(), &w.env.paths).unwrap();
        assert_eq!(cfg.server.label, "red.example.h2live.server");
        assert_eq!(
            cfg.tunnel.as_ref().unwrap().label,
            "red.example.h2live.tunnel"
        );
        assert_eq!(cfg.public_url.as_deref(), Some("https://live.example.net"));
        assert_eq!(cfg.data_dir, PathBuf::from("/x/live/server"));
        assert_eq!(cfg.local_url, "http://127.0.0.1:8074");
        assert_eq!(cfg.logs_dir, logs);
        assert_eq!(cfg.menubar.label, config::DEFAULT_MENUBAR_LABEL);
        // Nothing was loaded, unloaded or rewritten.
        assert!(w.fake.calls().iter().all(|c| c.starts_with("print")));
        assert_eq!(
            fs::read(
                w.env
                    .paths
                    .launch_agents
                    .join("red.example.h2live.server.plist")
            )
            .unwrap(),
            server_plist
        );

        // A missing agent is refused, and the config stays as it was.
        assert!(main(s(&["--adopt-existing", "nope", "none"]), &w.env).is_err());
        let again = Config::load(&w.env.paths.config_file(), &w.env.paths).unwrap();
        assert_eq!(again, cfg);
        fs::remove_dir_all(&w.root).unwrap();
    }

    #[test]
    fn install_and_uninstall_the_login_item() {
        let w = world("install");
        main(s(&["--install"]), &w.env).unwrap();
        let cfg = Config::load(&w.env.paths.config_file(), &w.env.paths).unwrap();
        let text = fs::read_to_string(&cfg.menubar.plist).unwrap();
        assert!(text.contains("<string>/Users/me/bin/h2</string>\n\t\t<string>live</string>\n\t\t<string>menubar</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(text.contains("<key>KeepAlive</key>\n\t<false/>"));
        // Written and enabled, not loaded: no second copy now.
        let calls = w.fake.calls();
        assert!(calls.contains(&format!("enable gui/501/{}", cfg.menubar.label)));
        assert!(!calls.iter().any(|c| c.starts_with("bootstrap")));

        main(s(&["--uninstall"]), &w.env).unwrap();
        assert!(!cfg.menubar.plist.exists());
        assert!(!w.fake.calls().iter().any(|c| c.starts_with("bootout")));
        fs::remove_dir_all(&w.root).unwrap();
    }

    #[test]
    fn install_server_and_tunnel_agents() {
        let w = world("server");
        let token = w.root.join("live.token");
        main(
            s(&[
                "--install-server",
                "--",
                "--listen",
                "127.0.0.1:9100",
                "--data",
                "/srv/live",
                "--public-url",
                "https://h.example",
                "--behind-cloudflared",
            ]),
            &w.env,
        )
        .unwrap();
        let cfg = Config::load(&w.env.paths.config_file(), &w.env.paths).unwrap();
        assert_eq!(cfg.local_url, "http://127.0.0.1:9100");
        assert_eq!(cfg.public_url.as_deref(), Some("https://h.example"));
        assert_eq!(cfg.data_dir, PathBuf::from("/srv/live"));
        let text = fs::read_to_string(&cfg.server.plist).unwrap();
        assert!(text.contains("<string>--data</string>\n\t\t<string>/srv/live</string>"));
        assert!(text.contains("<string>serve</string>\n\t\t<string>--listen</string>\n\t\t<string>127.0.0.1:9100</string>"));
        assert!(w.env.services.state(&cfg.server).is_running());

        main(
            s(&[
                "--install-tunnel",
                "--token-file",
                &token.to_string_lossy(),
                "--cloudflared",
                "/opt/cf/cloudflared",
            ]),
            &w.env,
        )
        .unwrap();
        let tunnel = cfg.tunnel.as_ref().unwrap();
        let text = fs::read_to_string(&tunnel.plist).unwrap();
        assert!(text.contains("<string>/opt/cf/cloudflared</string>"));
        assert!(text.contains("<string>http://127.0.0.1:9100</string>"));
        assert!(text.contains(&format!("<string>{}</string>", token.display())));
        assert!(w.env.services.state(tunnel).is_running());

        assert!(main(s(&["--install-tunnel"]), &w.env).is_err(), "no token");
        assert!(main(s(&["--bogus"]), &w.env).is_err());
        assert!(main(s(&["--", "x"]), &w.env).is_err());
        fs::remove_dir_all(&w.root).unwrap();
    }

    #[test]
    fn settings_alone_update_the_file() {
        let w = world("set");
        main(
            s(&["--public-url", "https://p.example", "--data", "/d"]),
            &w.env,
        )
        .unwrap();
        let cfg = Config::load(&w.env.paths.config_file(), &w.env.paths).unwrap();
        assert_eq!(cfg.public_url.as_deref(), Some("https://p.example"));
        assert_eq!(cfg.data_dir, PathBuf::from("/d"));
        main(s(&["--status"]), &w.env).unwrap();
        fs::remove_dir_all(&w.root).unwrap();
    }
}
