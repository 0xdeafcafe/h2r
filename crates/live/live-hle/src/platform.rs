//! Live on the app builds: tvOS and iOS today (crates/apps/apple-host), the
//! Xbox app when it exists (docs/runtime/windows.md). There Live is on by
//! default, against [`live_proto::DEFAULT_SERVER`]; `h2 play` on a desktop
//! is off unless a flag asks (docs/live/live.md "Playing on Live").
//!
//! The app's switch is `live.toml` in its data root:
//!
//! ```toml
//! enabled = false                    # play offline
//! server = "https://live.example"    # or sign in to this service
//! ```
//!
//! `$H2_LIVE_SERVER` (set from an Xcode scheme, say) wins over the file for
//! a run: a URL replaces the server, `off` turns Live off. Accounts are in
//! `live/accounts.json` under the data root, the file the in-game account
//! setup writes ([`crate::account_setup`]).

use std::path::{Path, PathBuf};

use live_proto::ServerOverride;
use serde::{Deserialize, Serialize};

use crate::LiveConfig;
use crate::account_setup::{self, LocalEntry};

/// The switch's file name, in the data root.
pub const FILE: &str = "live.toml";

/// What `live.toml` holds. Missing keys are the defaults: on, the
/// default service.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

/// What an app build does about Live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Offline; `why` is the line to log.
    Off { why: String },
    /// Signed in to `server`; `from` names what chose it.
    On { server: String, from: &'static str },
}

/// The decision for a switch and `$H2_LIVE_SERVER` (`env`).
pub fn decide(switch: &Switch, env: Option<&str>) -> Decision {
    match ServerOverride::parse(env) {
        ServerOverride::Off(v) => {
            return Decision::Off {
                why: format!("off (H2_LIVE_SERVER={v})"),
            };
        }
        ServerOverride::Url(server) => {
            return Decision::On {
                server,
                from: "H2_LIVE_SERVER",
            };
        }
        ServerOverride::Unset => {}
    }
    if switch.enabled == Some(false) {
        return Decision::Off {
            why: format!("off ({FILE}: enabled = false)"),
        };
    }
    match switch
        .server
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => Decision::On {
            server: s.trim_end_matches('/').to_string(),
            from: FILE,
        },
        None => Decision::On {
            server: live_proto::DEFAULT_SERVER.to_string(),
            from: "default",
        },
    }
}

/// Reads `<data>/live.toml`. Missing is the default; a file that does not
/// parse is the default too, with the reason to log.
pub fn read(data: &Path) -> (Switch, Option<String>) {
    let path = data.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(s) => (s, None),
            Err(e) => (
                Switch::default(),
                Some(format!("{} ignored: {}", path.display(), e.message())),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Switch::default(), None),
        Err(e) => (
            Switch::default(),
            Some(format!("{} ignored: {e}", path.display())),
        ),
    }
}

/// Writes `<data>/live.toml`, for a settings screen.
pub fn write(data: &Path, switch: &Switch) -> std::io::Result<()> {
    let text = toml::to_string(switch).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(data)?;
    std::fs::write(data.join(FILE), text)
}

/// Where an app build keeps its Live accounts.
pub fn accounts_path(data: &Path) -> PathBuf {
    data.join("live").join("accounts.json")
}

/// The boot configuration for an app build with data root `data`, and the
/// lines to log.
pub fn config(data: &Path, env: Option<&str>) -> (LiveConfig, Vec<String>) {
    let (switch, problem) = read(data);
    let mut lines: Vec<String> = problem.into_iter().collect();
    let config = match decide(&switch, env) {
        Decision::Off { why } => {
            lines.push(why);
            LiveConfig::default()
        }
        Decision::On { server, from } => {
            let note = match from {
                "default" => format!(" (the default; {FILE} enabled = false turns it off)"),
                other => format!(" ({other})"),
            };
            lines.push(format!("on, {server}{note}"));
            LiveConfig {
                enabled: true,
                accounts_path: Some(accounts_path(data)),
                server_url: Some(server),
                offer_new_account: true,
            }
        }
    };
    (config, lines)
}

/// The account this machine signs in to `config`'s server with: the first
/// usable entry in its accounts file made or recovered there. `None` when
/// Live is off or there is none yet (the in-game setup makes one).
pub fn account(config: &LiveConfig) -> Option<LocalEntry> {
    if !config.enabled {
        return None;
    }
    let server = config.server_url.as_deref()?.trim_end_matches('/');
    account_setup::load(config.accounts_path.as_deref()?)
        .ok()?
        .into_iter()
        .find(|e| e.server.trim_end_matches('/').eq_ignore_ascii_case(server))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("h2-live-platform-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn app_builds_are_on_against_the_default_service() {
        let data = temp("default");
        let (config, lines) = config(&data, None);
        assert_eq!(
            config,
            LiveConfig {
                enabled: true,
                accounts_path: Some(data.join("live/accounts.json")),
                server_url: Some(live_proto::DEFAULT_SERVER.into()),
                offer_new_account: true,
            }
        );
        assert!(
            lines[0].starts_with("on, https://live.forbes.red (the default"),
            "{lines:?}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn the_switch_and_the_variable_decide() {
        let on = |server: &str, from| Decision::On {
            server: server.into(),
            from,
        };
        let off = Switch {
            enabled: Some(false),
            server: None,
        };
        let custom = Switch {
            enabled: None,
            server: Some(" https://live.example/ ".into()),
        };
        assert_eq!(
            decide(&Switch::default(), None),
            on(live_proto::DEFAULT_SERVER, "default")
        );
        assert!(matches!(decide(&off, None), Decision::Off { .. }));
        assert_eq!(decide(&custom, None), on("https://live.example", FILE));
        // The variable wins over the file, both ways.
        assert_eq!(
            decide(&off, Some("http://127.0.0.1:8074/")),
            on("http://127.0.0.1:8074", "H2_LIVE_SERVER")
        );
        assert_eq!(
            decide(&custom, Some("off")),
            Decision::Off {
                why: "off (H2_LIVE_SERVER=off)".into()
            }
        );
        assert_eq!(decide(&custom, Some(" ")), on("https://live.example", FILE));
    }

    #[test]
    fn the_file_round_trips_and_a_bad_one_keeps_the_default() {
        let data = temp("file");
        let custom = Switch {
            enabled: Some(true),
            server: Some("https://live.example".into()),
        };
        write(&data, &custom).unwrap();
        assert_eq!(read(&data), (custom, None));
        let (c, _) = config(&data, None);
        assert_eq!(c.server_url.as_deref(), Some("https://live.example"));

        write(
            &data,
            &Switch {
                enabled: Some(false),
                server: None,
            },
        )
        .unwrap();
        let (c, lines) = config(&data, None);
        assert_eq!(c, LiveConfig::default());
        assert_eq!(lines, ["off (live.toml: enabled = false)"]);

        std::fs::write(data.join(FILE), "enabled = \"maybe\"\n").unwrap();
        let (s, problem) = read(&data);
        assert_eq!(s, Switch::default());
        assert!(problem.unwrap().contains("ignored"));
        let (c, lines) = config(&data, None);
        assert!(c.enabled, "a bad file is the default: on");
        assert_eq!(lines.len(), 2, "{lines:?}");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn the_account_is_the_one_made_on_this_server() {
        let data = temp("account");
        let (config, _) = config(&data, None);
        assert!(account(&config).is_none(), "no file yet");
        let entry = |server: &str, tag: &str, xuid: u64| {
            let key = live_proto::sign::KeyPair::generate();
            let jwk = serde_json::to_value(key.private_jwk()).unwrap();
            serde_json::json!({
                "xuid": live_proto::Xuid(xuid), "gamertag": tag, "server": server,
                "server_id": "live.test", "kid": key.kid(), "key": jwk,
            })
        };
        let path = accounts_path(&data);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_string(&[
                entry("http://127.0.0.1:8074", "Local", 0x0009_0000_0000_0001),
                entry("https://LIVE.forbes.red/", "Chief", 0x0009_0000_0000_0002),
            ])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(account(&config).unwrap().gamertag, "Chief");
        assert!(account(&LiveConfig::default()).is_none(), "off: none");
        let _ = std::fs::remove_dir_all(&data);
    }
}
