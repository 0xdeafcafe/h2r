//! What each menu item does, run on the worker thread (launchctl and HTTP
//! can block; the UI never waits on them).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::launchd::{AgentSpec, Services};
use crate::menu::ItemId;
use crate::status::Backend;

/// Opening things and the clipboard.
pub trait Desktop: Send + Sync {
    /// Opens a URL or a path in its usual app.
    fn open(&self, target: &str) -> Result<()>;
    fn copy(&self, text: &str) -> Result<()>;
}

/// `open`/`pbcopy` on macOS, `xdg-open`/`wl-copy` on Linux, `start`/`clip`
/// on Windows.
pub struct SystemDesktop;

impl Desktop for SystemDesktop {
    fn open(&self, target: &str) -> Result<()> {
        let mut cmd = if cfg!(target_os = "macos") {
            std::process::Command::new("open")
        } else if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        } else {
            std::process::Command::new("xdg-open")
        };
        let status = cmd
            .arg(target)
            .status()
            .with_context(|| format!("opening {target}"))?;
        if !status.success() {
            bail!("could not open {target}");
        }
        Ok(())
    }

    fn copy(&self, text: &str) -> Result<()> {
        use std::io::Write;
        let program = if cfg!(target_os = "macos") {
            "pbcopy"
        } else if cfg!(windows) {
            "clip"
        } else {
            "wl-copy"
        };
        let mut child = std::process::Command::new(program)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("running {program}"))?;
        child
            .stdin
            .take()
            .context("no stdin")?
            .write_all(text.as_bytes())?;
        if !child.wait()?.success() {
            bail!("{program} failed");
        }
        Ok(())
    }
}

/// The menu bar's own agent: `h2 live menubar` at login, in the GUI
/// session only.
pub fn menubar_spec(cfg: &Config, exe: &Path) -> AgentSpec {
    let log = cfg.logs_dir.join("menubar.log");
    AgentSpec {
        label: cfg.menubar.label.clone(),
        program_arguments: vec![
            exe.to_string_lossy().into_owned(),
            "live".into(),
            "menubar".into(),
        ],
        environment: vec![crate::update::path_entry(
            cfg.update
                .as_ref()
                .map(|u| u.path.as_slice())
                .unwrap_or_default(),
        )],
        stdout: Some(log.clone()),
        stderr: Some(log),
        run_at_load: true,
        keep_alive: false,
        process_type: Some("Interactive".into()),
        aqua_only: true,
    }
}

/// What a finished action leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Done; a line worth showing in the menu, if any.
    Done(Option<String>),
    /// The menu bar should quit.
    Quit,
}

/// What a long action shows while it runs.
pub fn busy_label(id: ItemId) -> Option<&'static str> {
    Some(match id {
        ItemId::StartServer => "Starting the server",
        ItemId::StopServer => "Stopping the server",
        ItemId::RestartServer => "Restarting the server",
        ItemId::StartTunnel => "Starting the tunnel",
        ItemId::StopTunnel => "Stopping the tunnel",
        ItemId::CopyInvite => "Making an invite",
        ItemId::StopAllAndQuit => "Stopping everything",
        ItemId::ServerAtLogin | ItemId::MenubarAtLogin => "Updating login items",
        _ => return None,
    })
}

pub struct Ctx<'a> {
    pub cfg: &'a Config,
    pub services: &'a dyn Services,
    pub backend: &'a dyn Backend,
    pub desktop: &'a dyn Desktop,
    /// This program, for the menu bar's own agent.
    pub exe: PathBuf,
}

impl Ctx<'_> {
    fn server_log(&self) -> PathBuf {
        self.services
            .log_path(&self.cfg.server)
            .unwrap_or_else(|| self.cfg.default_server_log())
    }

    fn tunnel(&self) -> Result<&crate::config::Agent> {
        self.cfg
            .tunnel
            .as_ref()
            .context("no tunnel agent is configured")
    }

    /// Runs `id`'s action. `Quit` is the UI's own; it is here so a test
    /// can see every item lead somewhere.
    pub fn perform(&self, id: ItemId) -> Result<Outcome> {
        let done = Ok(Outcome::Done(None));
        match id {
            ItemId::Server | ItemId::Numbers | ItemId::Tunnel | ItemId::Message => done,
            ItemId::StartServer => self.services.start(&self.cfg.server).and(done),
            ItemId::StopServer => self.services.stop(&self.cfg.server).and(done),
            ItemId::RestartServer => self.services.restart(&self.cfg.server).and(done),
            ItemId::StartTunnel => self.services.start(self.tunnel()?).and(done),
            ItemId::StopTunnel => self.services.stop(self.tunnel()?).and(done),
            ItemId::OpenDashboard => self.desktop.open(&self.cfg.dashboard_url()).and(done),
            ItemId::OpenStats => self.desktop.open(&self.cfg.stats_url()).and(done),
            ItemId::CopyInvite => {
                let code = self
                    .backend
                    .create_invite(&self.cfg.local_url, &self.cfg.data_dir)?;
                self.desktop.copy(&code)?;
                Ok(Outcome::Done(Some(format!("Invite copied: {code}"))))
            }
            ItemId::OpenLogs => {
                let log = self.server_log();
                let dir = log.parent().unwrap_or(&self.cfg.logs_dir);
                self.desktop.open(&dir.to_string_lossy()).and(done)
            }
            ItemId::ViewServerLog => {
                let log = self.server_log();
                if !log.exists() {
                    bail!("{} does not exist yet", log.display());
                }
                self.desktop.open(&log.to_string_lossy()).and(done)
            }
            ItemId::ServerAtLogin => {
                let on = self.services.run_at_login(&self.cfg.server)?;
                self.services
                    .set_run_at_login(&self.cfg.server, !on)
                    .and(done)
            }
            ItemId::MenubarAtLogin => {
                if self.cfg.menubar.plist.exists() {
                    // Leaves this running copy alone.
                    self.services.uninstall(&self.cfg.menubar, false)
                } else {
                    let spec = menubar_spec(self.cfg, &self.exe);
                    self.services.install(&self.cfg.menubar, &spec, false)
                }
                .and(done)
            }
            // Status lines, and the update thread's own items.
            ItemId::UpdateStatus
            | ItemId::UpdateError
            | ItemId::ArtStale
            | ItemId::UpdateNow
            | ItemId::PauseUpdates => done,
            ItemId::Quit => Ok(Outcome::Quit),
            ItemId::StopAllAndQuit => {
                if let Some(t) = &self.cfg.tunnel {
                    self.services.stop(t)?;
                }
                self.services.stop(&self.cfg.server)?;
                Ok(Outcome::Quit)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::sync::Mutex;

    use super::*;
    use crate::config::Paths;
    use crate::launchd::fake::FakeLaunchd;
    use crate::launchd::{Launchd, ServiceState};
    use crate::status::tests::FakeBackend;

    #[derive(Default)]
    pub struct FakeDesktop {
        pub opened: Mutex<Vec<String>>,
        pub copied: Mutex<Vec<String>>,
    }

    impl Desktop for FakeDesktop {
        fn open(&self, target: &str) -> Result<()> {
            self.opened.lock().unwrap().push(target.into());
            Ok(())
        }
        fn copy(&self, text: &str) -> Result<()> {
            self.copied.lock().unwrap().push(text.into());
            Ok(())
        }
    }

    #[test]
    fn every_item_does_its_thing() {
        let root = std::env::temp_dir().join(format!("h2-menubar-act-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths {
            live_dir: root.join("live"),
            launch_agents: root.join("LaunchAgents"),
        };
        let mut cfg = Config::defaults(&paths);
        cfg.public_url = Some("https://live.example.net".into());
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        let backend = FakeBackend::default();
        let desktop = FakeDesktop::default();
        let ctx = Ctx {
            cfg: &cfg,
            services: &ld,
            backend: &backend,
            desktop: &desktop,
            exe: "/Applications/h2/h2".into(),
        };
        // A server and tunnel agent as --install-server would write them.
        let log = cfg.logs_dir.join("server.log");
        for agent in [&cfg.server, cfg.tunnel.as_ref().unwrap()] {
            let mut spec = menubar_spec(&cfg, Path::new("/bin/true"));
            spec.label = agent.label.clone();
            spec.stderr = Some(log.clone());
            ld.install(agent, &spec, false).unwrap();
        }

        let ok = Outcome::Done(None);
        assert_eq!(ctx.perform(ItemId::StartServer).unwrap(), ok);
        assert!(ld.state(&cfg.server).is_running());
        assert_eq!(ctx.perform(ItemId::StartTunnel).unwrap(), ok);
        assert!(ld.state(cfg.tunnel.as_ref().unwrap()).is_running());
        ctx.perform(ItemId::RestartServer).unwrap();
        ctx.perform(ItemId::StopTunnel).unwrap();
        assert_eq!(
            ld.state(cfg.tunnel.as_ref().unwrap()),
            ServiceState::Unloaded
        );

        ctx.perform(ItemId::OpenDashboard).unwrap();
        ctx.perform(ItemId::OpenStats).unwrap();
        ctx.perform(ItemId::OpenLogs).unwrap();
        assert!(
            ctx.perform(ItemId::ViewServerLog).is_err(),
            "no log written yet"
        );
        fs::write(&log, "hi\n").unwrap();
        ctx.perform(ItemId::ViewServerLog).unwrap();
        assert_eq!(
            *desktop.opened.lock().unwrap(),
            [
                "https://live.example.net".to_string(),
                "https://live.example.net/stats".into(),
                cfg.logs_dir.to_string_lossy().into(),
                log.to_string_lossy().into(),
            ]
        );

        assert_eq!(
            ctx.perform(ItemId::CopyInvite).unwrap(),
            Outcome::Done(Some("Invite copied: INVITE-1".into()))
        );
        assert_eq!(*desktop.copied.lock().unwrap(), ["INVITE-1"]);

        assert!(ld.run_at_login(&cfg.server).unwrap());
        ctx.perform(ItemId::ServerAtLogin).unwrap();
        assert!(!ld.run_at_login(&cfg.server).unwrap());
        ctx.perform(ItemId::ServerAtLogin).unwrap();
        assert!(ld.run_at_login(&cfg.server).unwrap());

        // The menu bar's own login item: written, then removed, never
        // loaded (this copy is already running).
        ctx.perform(ItemId::MenubarAtLogin).unwrap();
        let text = fs::read_to_string(&cfg.menubar.plist).unwrap();
        assert!(text.contains("<string>/Applications/h2/h2</string>"));
        assert!(text.contains("<string>Aqua</string>"));
        assert!(!ld.state(&cfg.menubar).is_loaded());
        ctx.perform(ItemId::MenubarAtLogin).unwrap();
        assert!(!cfg.menubar.plist.exists());

        assert_eq!(ctx.perform(ItemId::Quit).unwrap(), Outcome::Quit);
        assert!(ld.state(&cfg.server).is_running(), "quit leaves it running");
        ctx.perform(ItemId::StartTunnel).unwrap();
        assert_eq!(ctx.perform(ItemId::StopAllAndQuit).unwrap(), Outcome::Quit);
        assert_eq!(ld.state(&cfg.server), ServiceState::Unloaded);
        assert_eq!(
            ld.state(cfg.tunnel.as_ref().unwrap()),
            ServiceState::Unloaded
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn without_a_tunnel_the_tunnel_items_fail_cleanly() {
        let paths = Paths {
            live_dir: "/nonexistent/live".into(),
            launch_agents: "/nonexistent/LA".into(),
        };
        let mut cfg = Config::defaults(&paths);
        cfg.tunnel = None;
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        let ctx = Ctx {
            cfg: &cfg,
            services: &ld,
            backend: &FakeBackend::default(),
            desktop: &FakeDesktop::default(),
            exe: "/h2".into(),
        };
        assert!(ctx.perform(ItemId::StartTunnel).is_err());
        assert!(ctx.perform(ItemId::StartServer).is_err(), "no plist");
        assert_eq!(ctx.perform(ItemId::StopAllAndQuit).unwrap(), Outcome::Quit);
    }
}
