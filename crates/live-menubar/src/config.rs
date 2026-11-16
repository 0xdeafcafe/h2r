//! `menubar.toml`: which launchd agents the menu bar manages and where the
//! server it watches lives.
//!
//! The file sits beside the rest of the Live host data
//! (`~/Library/Application Support/h2recomp/live/menubar.toml`). Every key
//! is optional; a missing file is the defaults. The labels and plist paths
//! are configurable so the menu bar can drive agents someone already wrote
//! (`h2 live menubar --adopt-existing SERVER TUNNEL`) without rewriting them.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_SERVER_LABEL: &str = "org.h2recomp.live.server";
pub const DEFAULT_TUNNEL_LABEL: &str = "org.h2recomp.live.tunnel";
pub const DEFAULT_MENUBAR_LABEL: &str = "org.h2recomp.live.menubar";
pub const DEFAULT_LOCAL_URL: &str = "http://127.0.0.1:8074";
pub const FILE_NAME: &str = "menubar.toml";

/// One launchd agent: its label and the plist that defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub label: String,
    pub plist: PathBuf,
}

impl Agent {
    /// `label` with its plist in `launch_agents` (`~/Library/LaunchAgents`).
    pub fn in_dir(launch_agents: &Path, label: &str) -> Self {
        Agent {
            label: label.into(),
            plist: launch_agents.join(format!("{label}.plist")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The server's agent (runs `h2-live-server` or `h2 live serve`).
    pub server: Agent,
    /// The cloudflared agent, if there is one (saved as an empty label
    /// when there is none, so a load does not bring the default back).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<Agent>,
    /// The menu bar's own agent (open at login).
    pub menubar: Agent,
    /// Where the menu bar reaches the server on this machine.
    pub local_url: String,
    /// Where everyone else reaches it (the tunnel's URL), if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// The server's data directory (its key signs the admin status and
    /// invite requests).
    pub data_dir: PathBuf,
    /// Where the agents write their logs.
    pub logs_dir: PathBuf,
    /// Opt-in updates from a git checkout (`[update]`); none is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateConfig>,
}

pub const DEFAULT_UPDATE_INTERVAL_MINUTES: u64 = 10;

/// `[update]`: follow a branch of a dedicated clone, rebuild what changed
/// and install it (docs/live/live-hosting.md "Updates").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// The checkout to fast-forward and build. A clone of its own, not a
    /// working checkout: it is never touched while dirty.
    pub repo: PathBuf,
    pub branch: String,
    pub remote: String,
    pub interval_minutes: u64,
    /// Holds `bin/` (h2-live-server, h2), `web/` and the build's
    /// `target/`.
    pub install_dir: PathBuf,
    /// The disc to re-extract game art from when the extractor changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mappack: Option<PathBuf>,
    /// cargo and bun, when not on PATH, ~/.cargo/bin or ~/.bun/bin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bun: Option<PathBuf>,
}

impl UpdateConfig {
    pub fn bin_dir(&self) -> PathBuf {
        self.install_dir.join("bin")
    }

    pub fn web_dir(&self) -> PathBuf {
        self.install_dir.join("web")
    }

    pub fn target_dir(&self) -> PathBuf {
        self.install_dir.join("target")
    }

    pub fn state_file(&self) -> PathBuf {
        self.install_dir.join("update-state.json")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialUpdate {
    repo: Option<PathBuf>,
    branch: Option<String>,
    remote: Option<String>,
    interval_minutes: Option<u64>,
    install_dir: Option<PathBuf>,
    disc: Option<PathBuf>,
    mappack: Option<PathBuf>,
    cargo: Option<PathBuf>,
    bun: Option<PathBuf>,
}

/// The directories defaults hang off, so tests pass temp dirs.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/Library/Application Support/h2recomp/live`.
    pub live_dir: PathBuf,
    /// `~/Library/LaunchAgents`.
    pub launch_agents: PathBuf,
}

impl Paths {
    /// The real ones under `$HOME`.
    pub fn from_home() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .context("HOME is not set")?;
        let home = PathBuf::from(home);
        Ok(Paths {
            live_dir: home.join("Library/Application Support/h2recomp/live"),
            launch_agents: home.join("Library/LaunchAgents"),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.live_dir.join(FILE_NAME)
    }
}

impl Config {
    pub fn defaults(paths: &Paths) -> Self {
        Config {
            server: Agent::in_dir(&paths.launch_agents, DEFAULT_SERVER_LABEL),
            tunnel: Some(Agent::in_dir(&paths.launch_agents, DEFAULT_TUNNEL_LABEL)),
            menubar: Agent::in_dir(&paths.launch_agents, DEFAULT_MENUBAR_LABEL),
            local_url: DEFAULT_LOCAL_URL.into(),
            public_url: None,
            data_dir: paths.live_dir.join("server"),
            logs_dir: paths.live_dir.join("logs"),
            update: None,
        }
    }

    /// `path`, or the defaults when it does not exist. Keys the file leaves
    /// out take their defaults.
    pub fn load(path: &Path, paths: &Paths) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::defaults(paths));
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Self::parse(&text, paths).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str, paths: &Paths) -> Result<Self> {
        let partial: Partial = toml::from_str(text)?;
        let d = Self::defaults(paths);
        Ok(Config {
            server: partial.server.unwrap_or(d.server),
            tunnel: match partial.tunnel {
                Some(t) if t.label.is_empty() => None,
                Some(t) => Some(t),
                None => d.tunnel,
            },
            menubar: partial.menubar.unwrap_or(d.menubar),
            local_url: partial.local_url.unwrap_or(d.local_url),
            public_url: partial.public_url.filter(|u| !u.is_empty()),
            data_dir: partial.data_dir.unwrap_or(d.data_dir),
            logs_dir: partial.logs_dir.unwrap_or(d.logs_dir),
            update: match partial.update {
                Some(PartialUpdate {
                    repo: Some(repo),
                    branch,
                    remote,
                    interval_minutes,
                    install_dir,
                    disc,
                    mappack,
                    cargo,
                    bun,
                }) if !repo.as_os_str().is_empty() => Some(UpdateConfig {
                    repo,
                    branch: branch.unwrap_or_else(|| "master".into()),
                    remote: remote.unwrap_or_else(|| "origin".into()),
                    interval_minutes: interval_minutes
                        .unwrap_or(DEFAULT_UPDATE_INTERVAL_MINUTES)
                        .max(1),
                    install_dir: install_dir.unwrap_or_else(|| paths.live_dir.clone()),
                    disc,
                    mappack,
                    cargo,
                    bun,
                }),
                _ => None,
            },
        })
    }

    pub fn to_toml(&self) -> Result<String> {
        let mut out = String::from(
            "# The menu bar app's settings (h2 live menubar; docs/live/live-hosting.md).\n",
        );
        out.push_str(&toml::to_string_pretty(self)?);
        if self.tunnel.is_none() {
            out.push_str("\n[tunnel]\nlabel = \"\"\nplist = \"\"\n");
        }
        Ok(out)
    }

    /// Writes the file atomically (a temp file renamed over it).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, self.to_toml()?).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// Points the config at existing agents by label, their plists in
    /// `launch_agents`. Their files are not touched.
    pub fn adopt(&mut self, launch_agents: &Path, server: &str, tunnel: Option<&str>) {
        self.server = Agent::in_dir(launch_agents, server);
        self.tunnel = tunnel.map(|t| Agent::in_dir(launch_agents, t));
    }

    /// The dashboard: the public URL when there is one, else the local one.
    pub fn dashboard_url(&self) -> String {
        self.public_url
            .as_deref()
            .unwrap_or(&self.local_url)
            .trim_end_matches('/')
            .to_string()
    }

    pub fn stats_url(&self) -> String {
        format!("{}/stats", self.dashboard_url())
    }

    /// The server's log as `--install-server` writes it; an adopted agent's
    /// own `StandardErrorPath` wins when the plist names one.
    pub fn default_server_log(&self) -> PathBuf {
        self.logs_dir.join("server.log")
    }

    pub fn default_tunnel_log(&self) -> PathBuf {
        self.logs_dir.join("tunnel.log")
    }
}

/// What the file may say; anything missing is a default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Partial {
    server: Option<Agent>,
    tunnel: Option<Agent>,
    menubar: Option<Agent>,
    local_url: Option<String>,
    public_url: Option<String>,
    data_dir: Option<PathBuf>,
    logs_dir: Option<PathBuf>,
    update: Option<PartialUpdate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn paths(root: &Path) -> Paths {
        Paths {
            live_dir: root.join("live"),
            launch_agents: root.join("LaunchAgents"),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2-menubar-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let root = temp("cfg-missing");
        let p = paths(&root);
        let c = Config::load(&p.config_file(), &p).unwrap();
        assert_eq!(c.server.label, DEFAULT_SERVER_LABEL);
        assert_eq!(
            c.server.plist,
            root.join("LaunchAgents/org.h2recomp.live.server.plist")
        );
        assert_eq!(c.tunnel.unwrap().label, DEFAULT_TUNNEL_LABEL);
        assert_eq!(c.menubar.label, DEFAULT_MENUBAR_LABEL);
        assert_eq!(c.local_url, DEFAULT_LOCAL_URL);
        assert_eq!(c.data_dir, root.join("live/server"));
        assert_eq!(c.public_url, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn save_then_load_round_trips() {
        let root = temp("cfg-round");
        let p = paths(&root);
        let mut c = Config::defaults(&p);
        c.adopt(
            &p.launch_agents,
            "red.forbes.h2live.server",
            Some("red.forbes.h2live.tunnel"),
        );
        c.public_url = Some("https://live.example.net/".into());
        c.save(&p.config_file()).unwrap();
        let text = fs::read_to_string(p.config_file()).unwrap();
        assert!(
            text.contains("label = \"red.forbes.h2live.server\""),
            "{text}"
        );
        let back = Config::load(&p.config_file(), &p).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.dashboard_url(), "https://live.example.net");
        assert_eq!(back.stats_url(), "https://live.example.net/stats");
        assert!(!p.live_dir.join("menubar.toml.tmp").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn partial_files_fill_in_defaults() {
        let root = temp("cfg-partial");
        let p = paths(&root);
        let c = Config::parse("public_url = \"https://h.example\"\n", &p).unwrap();
        assert_eq!(c.public_url.as_deref(), Some("https://h.example"));
        assert_eq!(c.server, Config::defaults(&p).server);
        assert_eq!(c.dashboard_url(), "https://h.example");

        // An empty tunnel label means "no tunnel".
        let c = Config::parse("[tunnel]\nlabel = \"\"\nplist = \"\"\n", &p).unwrap();
        assert_eq!(c.tunnel, None);
        assert_eq!(c.dashboard_url(), DEFAULT_LOCAL_URL);

        let mut none = Config::defaults(&p);
        none.tunnel = None;
        none.save(&p.config_file()).unwrap();
        assert_eq!(Config::load(&p.config_file(), &p).unwrap(), none);

        assert!(Config::parse("bogus = 1\n", &p).is_err());
        assert_eq!(Config::defaults(&p).update, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_update_section() {
        let root = temp("cfg-update");
        let p = paths(&root);
        // Off unless a repo is named.
        let c = Config::parse("[update]\nbranch = \"main\"\n", &p).unwrap();
        assert_eq!(c.update, None);
        let c = Config::parse("[update]\nrepo = \"/src/h2\"\n", &p).unwrap();
        let u = c.update.clone().unwrap();
        assert_eq!(u.repo, PathBuf::from("/src/h2"));
        assert_eq!(u.branch, "master");
        assert_eq!(u.remote, "origin");
        assert_eq!(u.interval_minutes, DEFAULT_UPDATE_INTERVAL_MINUTES);
        assert_eq!(u.install_dir, root.join("live"));
        assert_eq!(u.bin_dir(), root.join("live/bin"));
        assert_eq!(u.web_dir(), root.join("live/web"));
        assert_eq!(u.disc, None);

        let mut c = c;
        let u = c.update.as_mut().unwrap();
        u.disc = Some("/discs/h2.iso".into());
        u.interval_minutes = 30;
        c.save(&p.config_file()).unwrap();
        assert_eq!(Config::load(&p.config_file(), &p).unwrap(), c);
        assert!(Config::parse("[update]\nrepo = \"/x\"\nbogus = 1\n", &p).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
