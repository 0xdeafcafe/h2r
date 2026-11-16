//! The menu as plain data: which items there are, what they say, and which
//! are enabled, for a [`Snapshot`]. The tray UI only draws this.

use crate::config::Config;
use crate::launchd::ServiceState;
use crate::status::{Health, Snapshot, format_uptime};

/// Every item, by what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemId {
    // Status lines (never enabled).
    Server,
    Numbers,
    Tunnel,
    Message,
    // Actions.
    StartServer,
    StopServer,
    RestartServer,
    StartTunnel,
    StopTunnel,
    OpenDashboard,
    OpenStats,
    CopyInvite,
    OpenLogs,
    ViewServerLog,
    ServerAtLogin,
    MenubarAtLogin,
    Quit,
    StopAllAndQuit,
}

impl ItemId {
    /// A stable id for the toolkit's menu items.
    pub fn key(self) -> &'static str {
        match self {
            ItemId::Server => "server",
            ItemId::Numbers => "numbers",
            ItemId::Tunnel => "tunnel",
            ItemId::Message => "message",
            ItemId::StartServer => "start-server",
            ItemId::StopServer => "stop-server",
            ItemId::RestartServer => "restart-server",
            ItemId::StartTunnel => "start-tunnel",
            ItemId::StopTunnel => "stop-tunnel",
            ItemId::OpenDashboard => "open-dashboard",
            ItemId::OpenStats => "open-stats",
            ItemId::CopyInvite => "copy-invite",
            ItemId::OpenLogs => "open-logs",
            ItemId::ViewServerLog => "view-server-log",
            ItemId::ServerAtLogin => "server-at-login",
            ItemId::MenubarAtLogin => "menubar-at-login",
            ItemId::Quit => "quit",
            ItemId::StopAllAndQuit => "stop-all-and-quit",
        }
    }

    pub const ALL: [ItemId; 18] = [
        ItemId::Server,
        ItemId::Numbers,
        ItemId::Tunnel,
        ItemId::Message,
        ItemId::StartServer,
        ItemId::StopServer,
        ItemId::RestartServer,
        ItemId::StartTunnel,
        ItemId::StopTunnel,
        ItemId::OpenDashboard,
        ItemId::OpenStats,
        ItemId::CopyInvite,
        ItemId::OpenLogs,
        ItemId::ViewServerLog,
        ItemId::ServerAtLogin,
        ItemId::MenubarAtLogin,
        ItemId::Quit,
        ItemId::StopAllAndQuit,
    ];

    pub fn from_key(key: &str) -> Option<ItemId> {
        Self::ALL.into_iter().find(|i| i.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub label: String,
    pub enabled: bool,
    /// Some for a check item.
    pub checked: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Item(Item),
    Separator,
}

/// The icon's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overall {
    Running,
    Starting,
    Stopped,
    Error,
}

pub fn overall(s: &Snapshot) -> Overall {
    if s.busy.is_some() {
        return Overall::Starting;
    }
    match (&s.server, &s.health) {
        (ServiceState::Failed { .. } | ServiceState::Unknown(_), Health::Down(_)) => Overall::Error,
        (_, Health::Up(_)) => Overall::Running,
        // It answers but will not say how it is: worth a look.
        (_, Health::UpNoStatus(_)) => Overall::Error,
        // Loaded, not answering yet.
        (ServiceState::Running { .. } | ServiceState::Waiting, Health::Down(_)) => {
            Overall::Starting
        }
        _ => Overall::Stopped,
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn server_line(s: &Snapshot, cfg: &Config) -> String {
    let url = cfg.local_url.trim_end_matches('/');
    match (&s.health, &s.server) {
        (Health::Up(_) | Health::UpNoStatus(_), st) if !st.is_loaded() => {
            format!("Server: running at {url} (not started by this app)")
        }
        (Health::Up(_) | Health::UpNoStatus(_), _) => format!("Server: running at {url}"),
        (Health::Down(_), ServiceState::Running { .. } | ServiceState::Waiting) => {
            format!("Server: starting at {url}")
        }
        (Health::Down(_), st) => format!("Server: {}", st.describe()),
    }
}

fn numbers_line(s: &Snapshot) -> String {
    match &s.health {
        Health::Up(st) => {
            let mut parts = Vec::new();
            if let Some(up) = st.uptime_s {
                parts.push(format!("Up {}", format_uptime(up)));
            }
            parts.push(format!("{} online", st.online));
            parts.push(plural(st.live_games, "live game", "live games"));
            parts.push(plural(st.accounts, "account", "accounts"));
            parts.join(" · ")
        }
        Health::UpNoStatus(why) => format!("No status: {why}"),
        Health::Down(why) => format!("Not answering: {why}"),
    }
}

fn tunnel_line(s: &Snapshot, cfg: &Config) -> String {
    let public = cfg.public_url.clone().or_else(|| match &s.health {
        Health::Up(st) => st.public_url.clone(),
        _ => None,
    });
    let reach = match s.public_reachable {
        Some(true) => " · public URL answers",
        Some(false) => " · public URL not answering",
        None => "",
    };
    match (&s.tunnel, public) {
        (None, None) => "Tunnel: none (local only)".into(),
        (None, Some(url)) => format!("Public: {url}{reach}"),
        (Some(st), Some(url)) => format!("Tunnel: {} · {url}{reach}", st.describe()),
        (Some(st), None) => format!("Tunnel: {}", st.describe()),
    }
}

/// The menu for `s`.
pub fn model(s: &Snapshot, cfg: &Config) -> Vec<Entry> {
    let idle = s.busy.is_none();
    let installed = !matches!(s.server, ServiceState::NotInstalled);
    let loaded = s.server.is_loaded();
    let up = s.health.is_up();
    let tunnel = s.tunnel.as_ref();
    let tunnel_installed = tunnel.is_some_and(|t| !matches!(t, ServiceState::NotInstalled));
    let tunnel_loaded = tunnel.is_some_and(ServiceState::is_loaded);

    let info = |id, label: String| {
        Entry::Item(Item {
            id,
            label,
            enabled: false,
            checked: None,
        })
    };
    let action = |id, label: &str, enabled: bool| {
        Entry::Item(Item {
            id,
            label: label.into(),
            enabled,
            checked: None,
        })
    };
    let check = |id, label: &str, enabled: bool, on: bool| {
        Entry::Item(Item {
            id,
            label: label.into(),
            enabled,
            checked: Some(on),
        })
    };

    let mut m = vec![
        info(ItemId::Server, server_line(s, cfg)),
        info(ItemId::Numbers, numbers_line(s)),
        info(ItemId::Tunnel, tunnel_line(s, cfg)),
    ];
    if let Some(msg) = s
        .busy
        .as_ref()
        .map(|b| format!("{b}…"))
        .or(s.message.clone())
    {
        m.push(info(ItemId::Message, msg));
    }
    m.extend([
        Entry::Separator,
        // A server something else started (a terminal) is not ours to
        // start a second copy of.
        action(
            ItemId::StartServer,
            "Start Server",
            idle && installed && !loaded && !up,
        ),
        action(ItemId::StopServer, "Stop Server", idle && loaded),
        action(ItemId::RestartServer, "Restart Server", idle && loaded),
    ]);
    if tunnel.is_some() {
        m.extend([
            action(
                ItemId::StartTunnel,
                "Start Tunnel",
                idle && tunnel_installed && !tunnel_loaded,
            ),
            action(ItemId::StopTunnel, "Stop Tunnel", idle && tunnel_loaded),
        ]);
    }
    m.extend([
        Entry::Separator,
        action(ItemId::OpenDashboard, "Open Dashboard", true),
        action(ItemId::OpenStats, "Open Stats Site", true),
        action(ItemId::CopyInvite, "Copy New Invite", idle && up),
        Entry::Separator,
        action(ItemId::OpenLogs, "Open Logs Folder", true),
        action(ItemId::ViewServerLog, "View Server Log", true),
        Entry::Separator,
        check(
            ItemId::ServerAtLogin,
            "Run Server at Login",
            idle && installed && s.server_at_login.is_some(),
            s.server_at_login.unwrap_or(false),
        ),
        check(
            ItemId::MenubarAtLogin,
            "Open Menu Bar App at Login",
            idle,
            s.menubar_at_login,
        ),
        Entry::Separator,
        action(ItemId::Quit, "Quit Menu Bar (server keeps running)", true),
        action(
            ItemId::StopAllAndQuit,
            "Stop Everything and Quit",
            idle && (loaded || tunnel_loaded),
        ),
    ]);
    m
}

/// The ids in order, separators as None: when this changes the UI builds
/// the menu again, else it updates the items in place.
pub fn shape(m: &[Entry]) -> Vec<Option<ItemId>> {
    m.iter()
        .map(|e| match e {
            Entry::Item(i) => Some(i.id),
            Entry::Separator => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use crate::status::{ServerStatus, tests::ADMIN_STATUS};

    fn cfg() -> Config {
        Config::defaults(&Paths {
            live_dir: "/x/live".into(),
            launch_agents: "/x/LaunchAgents".into(),
        })
    }

    fn snap(server: ServiceState, health: Health) -> Snapshot {
        Snapshot {
            server,
            tunnel: Some(ServiceState::Unloaded),
            health,
            public_reachable: None,
            server_at_login: Some(true),
            menubar_at_login: false,
            busy: None,
            message: None,
        }
    }

    fn get(m: &[Entry], id: ItemId) -> Item {
        m.iter()
            .find_map(|e| match e {
                Entry::Item(i) if i.id == id => Some(i.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id:?} missing"))
    }

    fn enabled(m: &[Entry]) -> Vec<ItemId> {
        m.iter()
            .filter_map(|e| match e {
                Entry::Item(i) if i.enabled => Some(i.id),
                _ => None,
            })
            .collect()
    }

    fn up() -> Health {
        Health::Up(crate::status::parse_status(ADMIN_STATUS).unwrap())
    }

    #[test]
    fn running_server() {
        let s = snap(ServiceState::Running { pid: Some(9) }, up());
        let m = model(&s, &cfg());
        assert_eq!(overall(&s), Overall::Running);
        assert_eq!(
            get(&m, ItemId::Server).label,
            "Server: running at http://127.0.0.1:8074"
        );
        assert_eq!(
            get(&m, ItemId::Numbers).label,
            "Up 3h 12m · 3 online · 1 live game · 12 accounts"
        );
        assert_eq!(
            get(&m, ItemId::Tunnel).label,
            "Tunnel: stopped · https://live.example.net"
        );
        use ItemId::*;
        assert_eq!(
            enabled(&m),
            [
                StopServer,
                RestartServer,
                StartTunnel,
                OpenDashboard,
                OpenStats,
                CopyInvite,
                OpenLogs,
                ViewServerLog,
                ServerAtLogin,
                MenubarAtLogin,
                Quit,
                StopAllAndQuit
            ]
        );
        assert_eq!(get(&m, ServerAtLogin).checked, Some(true));
        assert_eq!(get(&m, MenubarAtLogin).checked, Some(false));
        assert!(!shape(&m).contains(&Some(Message)));
    }

    #[test]
    fn stopped_server() {
        let s = snap(ServiceState::Unloaded, Health::Down("refused".into()));
        let m = model(&s, &cfg());
        assert_eq!(overall(&s), Overall::Stopped);
        assert_eq!(get(&m, ItemId::Server).label, "Server: stopped");
        assert_eq!(get(&m, ItemId::Numbers).label, "Not answering: refused");
        assert!(get(&m, ItemId::StartServer).enabled);
        assert!(!get(&m, ItemId::StopServer).enabled);
        assert!(!get(&m, ItemId::RestartServer).enabled);
        assert!(!get(&m, ItemId::CopyInvite).enabled, "needs a server");
        assert!(!get(&m, ItemId::StopAllAndQuit).enabled, "nothing to stop");
        assert!(get(&m, ItemId::Quit).enabled);
    }

    #[test]
    fn starting_failing_and_foreign_servers() {
        let s = snap(ServiceState::Waiting, Health::Down("refused".into()));
        assert_eq!(overall(&s), Overall::Starting);
        let m = model(&s, &cfg());
        assert_eq!(
            get(&m, ItemId::Server).label,
            "Server: starting at http://127.0.0.1:8074"
        );
        assert!(get(&m, ItemId::StopServer).enabled);

        let s = snap(
            ServiceState::Failed { last_exit: 78 },
            Health::Down("refused".into()),
        );
        assert_eq!(overall(&s), Overall::Error);
        assert_eq!(
            get(&model(&s, &cfg()), ItemId::Server).label,
            "Server: failed (exit 78)"
        );

        let s = snap(
            ServiceState::Running { pid: None },
            Health::UpNoStatus("401".into()),
        );
        assert_eq!(overall(&s), Overall::Error);
        assert_eq!(
            get(&model(&s, &cfg()), ItemId::Numbers).label,
            "No status: 401"
        );

        // Running from a terminal: not ours to start or stop.
        let s = snap(ServiceState::Unloaded, Health::Up(ServerStatus::default()));
        let m = model(&s, &cfg());
        assert_eq!(overall(&s), Overall::Running);
        assert!(
            get(&m, ItemId::Server)
                .label
                .ends_with("(not started by this app)")
        );
        assert!(!get(&m, ItemId::StartServer).enabled);
        assert!(!get(&m, ItemId::StopServer).enabled);
        assert!(get(&m, ItemId::CopyInvite).enabled);
    }

    #[test]
    fn busy_disables_actions_and_shows_why() {
        let mut s = snap(ServiceState::Running { pid: Some(1) }, up());
        s.busy = Some("Restarting the server".into());
        let m = model(&s, &cfg());
        assert_eq!(overall(&s), Overall::Starting);
        assert_eq!(get(&m, ItemId::Message).label, "Restarting the server…");
        use ItemId::*;
        assert_eq!(
            enabled(&m),
            [OpenDashboard, OpenStats, OpenLogs, ViewServerLog, Quit]
        );

        s.busy = None;
        s.message = Some("Invite copied: ABCD".into());
        assert_eq!(
            get(&model(&s, &cfg()), Message).label,
            "Invite copied: ABCD"
        );
    }

    #[test]
    fn not_installed_and_no_tunnel() {
        let mut s = snap(ServiceState::NotInstalled, Health::Down("refused".into()));
        s.tunnel = None;
        s.server_at_login = Some(false);
        let m = model(&s, &cfg());
        assert_eq!(get(&m, ItemId::Server).label, "Server: not installed");
        assert_eq!(get(&m, ItemId::Tunnel).label, "Tunnel: none (local only)");
        assert!(!get(&m, ItemId::StartServer).enabled);
        assert!(!get(&m, ItemId::ServerAtLogin).enabled);
        assert!(!shape(&m).contains(&Some(ItemId::StartTunnel)));

        let mut c = cfg();
        c.public_url = Some("https://p.example".into());
        s.health = up();
        s.public_reachable = Some(true);
        assert_eq!(
            get(&model(&s, &c), ItemId::Tunnel).label,
            "Public: https://p.example · public URL answers"
        );
    }

    #[test]
    fn keys_round_trip() {
        for id in ItemId::ALL {
            assert_eq!(ItemId::from_key(id.key()), Some(id));
        }
        assert_eq!(ItemId::from_key("nope"), None);
    }
}
