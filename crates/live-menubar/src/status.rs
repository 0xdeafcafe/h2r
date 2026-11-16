//! What the menu bar shows: one [`Snapshot`] of the agents and the server,
//! taken off the UI thread every few seconds.
//!
//! The server's numbers come from `GET /v1/admin/status`, what the
//! dashboard's Server screen reads (docs/live/live-admin.md §5), signed
//! with the server key in the data directory like `h2 live invite create`;
//! the CLI supplies that call as a [`Backend`]. `/healthz` says whether
//! the server answers at all, locally and at the public URL.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::Config;
use crate::launchd::{ServiceState, Services};

/// The calls that need the operator's key, and plain health checks.
pub trait Backend: Send + Sync {
    /// `GET /v1/admin/status` at `local_url`, signed with the key in
    /// `data_dir`: the JSON body.
    fn admin_status(&self, local_url: &str, data_dir: &Path) -> Result<String>;
    /// A new invite code (`h2 live invite create`).
    fn create_invite(&self, local_url: &str, data_dir: &Path) -> Result<String>;
    /// `GET <base>/healthz` answers 200.
    fn healthz(&self, base: &str) -> Result<()> {
        healthz(base)
    }
}

/// `GET <base>/healthz`, with a short timeout.
pub fn healthz(base: &str) -> Result<()> {
    let url = format!("{}/healthz", base.trim_end_matches('/'));
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build()
        .get(&url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    Ok(())
}

/// The parts of `/v1/admin/status` the menu shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerStatus {
    pub id: String,
    pub uptime_s: Option<i64>,
    pub accounts: u64,
    pub online: u64,
    pub live_games: u64,
    pub public_url: Option<String>,
    /// `quick`, `named` or `proxy`.
    pub tunnel_kind: Option<String>,
    pub invite_only: bool,
    pub open_invites: Option<u64>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    id: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    invite_only: bool,
    #[serde(default)]
    counts: Counts,
    #[serde(default)]
    uptime_s: Option<i64>,
    #[serde(default)]
    public: Option<Public>,
    #[serde(default)]
    invites: Option<Invites>,
}

#[derive(Deserialize, Default)]
struct Counts {
    #[serde(default)]
    accounts: u64,
    #[serde(default)]
    online: u64,
    #[serde(default)]
    live_games: u64,
}

#[derive(Deserialize)]
struct Public {
    url: Option<String>,
    tunnel: Option<String>,
}

#[derive(Deserialize)]
struct Invites {
    open: Option<u64>,
}

/// Parses `/v1/admin/status` (or the public `/v1/server`, which lacks the
/// uptime, tunnel and invites).
pub fn parse_status(json: &str) -> Result<ServerStatus> {
    let raw: Raw = serde_json::from_str(json).context("parsing the server status")?;
    let (public_url, tunnel_kind) = match raw.public {
        Some(p) => (p.url, p.tunnel),
        None => (raw.url, None),
    };
    Ok(ServerStatus {
        id: raw.id,
        uptime_s: raw.uptime_s,
        accounts: raw.counts.accounts,
        online: raw.counts.online,
        live_games: raw.counts.live_games,
        public_url,
        tunnel_kind,
        invite_only: raw.invite_only,
        open_invites: raw.invites.and_then(|i| i.open),
    })
}

/// Whether the server answers, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// `/healthz` and the status both answered.
    Up(ServerStatus),
    /// `/healthz` answered; the status did not (no key here, not admin...).
    UpNoStatus(String),
    /// Nothing answers at the local URL.
    Down(String),
}

impl Health {
    pub fn is_up(&self) -> bool {
        !matches!(self, Health::Down(_))
    }
}

/// Everything the menu needs, at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub server: ServiceState,
    /// None when no tunnel agent is configured.
    pub tunnel: Option<ServiceState>,
    pub health: Health,
    /// `/healthz` at the public URL; None when there is none or the
    /// server is down anyway.
    pub public_reachable: Option<bool>,
    /// None when launchd could not say.
    pub server_at_login: Option<bool>,
    pub menubar_at_login: bool,
    /// An action in progress ("Restarting the server").
    pub busy: Option<String>,
    /// The last action's outcome worth showing (an error, or "Invite
    /// copied").
    pub message: Option<String>,
    /// Updates, when `[update]` is configured.
    pub update: Option<crate::update::UpdateView>,
    /// Unix seconds when this was taken (for "5 min ago").
    pub now: i64,
}

impl Snapshot {
    /// Before the first poll.
    pub fn initial() -> Self {
        Snapshot {
            server: ServiceState::Unknown("checking".into()),
            tunnel: None,
            health: Health::Down("checking".into()),
            public_reachable: None,
            server_at_login: None,
            menubar_at_login: false,
            busy: Some("Checking".into()),
            message: None,
            update: None,
            now: unix_now(),
        }
    }
}

/// Takes a snapshot. Never fails: what goes wrong is in the snapshot.
pub fn collect(cfg: &Config, services: &dyn Services, backend: &dyn Backend) -> Snapshot {
    let server = services.state(&cfg.server);
    let tunnel = cfg.tunnel.as_ref().map(|t| services.state(t));
    let health = match backend.healthz(&cfg.local_url) {
        Err(e) => Health::Down(first_line(&e)),
        Ok(()) => match backend
            .admin_status(&cfg.local_url, &cfg.data_dir)
            .and_then(|s| parse_status(&s))
        {
            Ok(s) => Health::Up(s),
            Err(e) => Health::UpNoStatus(first_line(&e)),
        },
    };
    let public = cfg.public_url.clone().or_else(|| match &health {
        Health::Up(s) => s.public_url.clone(),
        _ => None,
    });
    let public_reachable = match public {
        Some(url) if health.is_up() && url != cfg.local_url => Some(backend.healthz(&url).is_ok()),
        _ => None,
    };
    Snapshot {
        server,
        tunnel,
        health,
        public_reachable,
        server_at_login: services.run_at_login(&cfg.server).ok(),
        menubar_at_login: cfg.menubar.plist.exists(),
        busy: None,
        message: None,
        update: None,
        now: unix_now(),
    }
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn first_line(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

/// `3d 4h`, `2h 5m`, `12m`, `40s`.
pub fn format_uptime(s: i64) -> String {
    let s = s.max(0);
    let (d, h, m) = (s / 86_400, s / 3600 % 24, s / 60 % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::Paths;
    use crate::launchd::Launchd;
    use crate::launchd::fake::FakeLaunchd;

    /// What `/v1/admin/status` answers, as recorded from a local server.
    pub const ADMIN_STATUS: &str = r#"{
      "id": "live.example.net",
      "software": {"name": "h2-live-server", "version": "0.1.0"},
      "url": "https://live.example.net",
      "invite_only": true,
      "private": false,
      "federation": false,
      "counts": {"accounts": 12, "online": 3, "live_games": 1, "sessions": 2},
      "position": 4410,
      "started_at": "2026-11-16T09:00:00Z",
      "uptime_s": 11520,
      "public": {"url": "https://live.example.net", "tunnel": "named"},
      "game_art": {"present": true, "shared": true, "maps": 23},
      "admins": [{"gamertag": "afr", "source": "configured"}],
      "log": {"position": 4410, "head": 4410, "store": "postgres"},
      "invites": {"open": 2, "used": 9, "revoked": 0, "expired": 1},
      "data_dir": "/Users/someone/Library/Application Support/h2recomp/live/server"
    }"#;

    /// A scripted backend.
    #[derive(Default)]
    pub struct FakeBackend {
        pub up: Mutex<Vec<String>>,
        pub status: Mutex<Option<String>>,
        pub invites: Mutex<u32>,
    }

    impl Backend for FakeBackend {
        fn admin_status(&self, _: &str, _: &Path) -> Result<String> {
            self.status
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("401 not an admin\nmore"))
        }
        fn create_invite(&self, _: &str, _: &Path) -> Result<String> {
            let mut n = self.invites.lock().unwrap();
            *n += 1;
            Ok(format!("INVITE-{n}"))
        }
        fn healthz(&self, base: &str) -> Result<()> {
            if self.up.lock().unwrap().iter().any(|u| u == base) {
                Ok(())
            } else {
                anyhow::bail!("connection refused")
            }
        }
    }

    #[test]
    fn parses_the_admin_status() {
        let s = parse_status(ADMIN_STATUS).unwrap();
        assert_eq!(
            s,
            ServerStatus {
                id: "live.example.net".into(),
                uptime_s: Some(11520),
                accounts: 12,
                online: 3,
                live_games: 1,
                public_url: Some("https://live.example.net".into()),
                tunnel_kind: Some("named".into()),
                invite_only: true,
                open_invites: Some(2),
            }
        );
        // The public /v1/server: no uptime, tunnel or invites.
        let s = parse_status(
            r#"{"id":"h","url":null,"invite_only":false,"counts":{"accounts":1,"online":0,"live_games":0,"sessions":0}}"#,
        )
        .unwrap();
        assert_eq!(s.uptime_s, None);
        assert_eq!(s.public_url, None);
        assert_eq!(s.accounts, 1);
        assert!(parse_status("<html>").is_err());
    }

    #[test]
    fn uptimes() {
        assert_eq!(format_uptime(40), "40s");
        assert_eq!(format_uptime(12 * 60 + 5), "12m");
        assert_eq!(format_uptime(11520), "3h 12m");
        assert_eq!(format_uptime(3 * 86_400 + 4 * 3600 + 7), "3d 4h");
        assert_eq!(format_uptime(-5), "0s");
    }

    fn setup() -> (PathBuf, Config, Arc<FakeLaunchd>, Launchd) {
        let root = std::env::temp_dir().join(format!("h2-menubar-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths {
            live_dir: root.join("live"),
            launch_agents: root.join("LaunchAgents"),
        };
        std::fs::create_dir_all(&paths.launch_agents).unwrap();
        let cfg = Config::defaults(&paths);
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        (root, cfg, fake, ld)
    }

    #[test]
    fn collect_reports_what_answers() {
        let (root, mut cfg, fake, ld) = setup();
        let backend = FakeBackend::default();

        let s = collect(&cfg, &ld, &backend);
        assert_eq!(s.server, ServiceState::NotInstalled);
        assert_eq!(s.tunnel, Some(ServiceState::NotInstalled));
        assert!(matches!(s.health, Health::Down(ref e) if e == "connection refused"));
        assert_eq!(s.public_reachable, None);
        assert_eq!(s.server_at_login, Some(false));
        assert!(!s.menubar_at_login);

        // Up, but the status call is refused: one line of why.
        backend.up.lock().unwrap().push(cfg.local_url.clone());
        fake.world
            .lock()
            .unwrap()
            .loaded
            .insert(cfg.server.label.clone(), Some(7));
        let s = collect(&cfg, &ld, &backend);
        assert_eq!(s.server, ServiceState::Running { pid: Some(7) });
        assert_eq!(s.health, Health::UpNoStatus("401 not an admin".into()));

        // The status names the public URL; it is checked too.
        *backend.status.lock().unwrap() = Some(ADMIN_STATUS.into());
        let s = collect(&cfg, &ld, &backend);
        assert!(matches!(s.health, Health::Up(ref st) if st.online == 3));
        assert_eq!(s.public_reachable, Some(false));
        backend
            .up
            .lock()
            .unwrap()
            .push("https://live.example.net".into());
        assert_eq!(collect(&cfg, &ld, &backend).public_reachable, Some(true));

        // Without a tunnel agent there is no tunnel state.
        cfg.tunnel = None;
        assert_eq!(collect(&cfg, &ld, &backend).tunnel, None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
