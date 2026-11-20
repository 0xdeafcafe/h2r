use std::fmt;
use std::path::PathBuf;

use live_proto::hosts::HostArchive;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct TelemetryAccount {
    pub xuid: String,
    pub gamertag: String,
    /// The signed-in account's private JWK, as stored in accounts.json.
    pub key: live_proto::sign::PrivateJwk,
}

impl fmt::Debug for TelemetryAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetryAccount")
            .field("xuid", &self.xuid)
            .field("gamertag", &self.gamertag)
            .field("key", &"<redacted>")
            .finish()
    }
}

/// Private boot credential supplied by the host supervisor.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostToken(pub(crate) String);

impl fmt::Debug for HostToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted host token>")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelemetrySinkConfig {
    /// Install the recorded hooks during playback without producing files.
    Discard { film_effects: bool },
    File(PathBuf),
    Server {
        base_url: String,
        account: TelemetryAccount,
    },
    Host {
        base_url: String,
        token: HostToken,
        game: Option<live_proto::game::SimGame>,
    },
    Simulation {
        base_url: String,
        account: TelemetryAccount,
        game: live_proto::game::SimGame,
        /// Deferred recordings take this exclusive lock during upload.
        deferred_lock: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub sink: TelemetrySinkConfig,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sink: TelemetrySinkConfig::File(PathBuf::new()),
        }
    }
}

#[derive(Deserialize)]
struct StoredAccount {
    xuid: String,
    gamertag: String,
    key: live_proto::sign::PrivateJwk,
}

impl TelemetryConfig {
    pub fn file(path: PathBuf) -> Self {
        Self {
            enabled: true,
            sink: TelemetrySinkConfig::File(path),
        }
    }

    /// Dedicated matches and hosted training runs use the private lease
    /// bearer for ingest, including when the machine has no Live accounts.
    pub fn for_host(
        base_url: String,
        token: String,
        game: Option<live_proto::game::SimGame>,
    ) -> Result<Self, String> {
        let archive = std::env::var("H2_HOST_ARCHIVE")
            .map_or(Ok(HostArchive::default()), |value| HostArchive::parse(&value))?;
        Self::host_archive(base_url, token, game, archive)
    }

    fn host_archive(
        base_url: String,
        token: String,
        game: Option<live_proto::game::SimGame>,
        archive: HostArchive,
    ) -> Result<Self, String> {
        if !live_proto::bearer_allowed(&base_url) {
            return Err("host telemetry requires HTTPS or a loopback URL".into());
        }
        if token.is_empty() {
            return Err("missing host telemetry credential".into());
        }
        Ok(Self {
            enabled: archive == HostArchive::Dual,
            sink: TelemetrySinkConfig::Host {
                base_url,
                token: HostToken(token),
                game,
            },
        })
    }

    /// The local observer still uses the host's mode and simulation metadata
    /// when the separate live transport is switched off.
    pub fn film_only_host(&self) -> bool {
        !self.enabled && matches!(self.sink, TelemetrySinkConfig::Host { .. })
    }

    /// Training metrics have their own private endpoint. In film-only mode
    /// the film has already created and ended the game when we reach it.
    pub fn publish_sim_report_after_film(&self, report: serde_json::Value) -> Result<(), String> {
        let TelemetrySinkConfig::Host { base_url, token, game: Some(game) } = &self.sink else {
            return Err("the host report needs a leased simulation".into());
        };
        let sink = crate::server::ServerSink::for_host(base_url.clone(), token.clone())?;
        sink.report_after_film(&live_proto::game::sim_game_id(&game.run_id), &report)
    }

    /// Boot-only configuration boundary. `H2_TELEMETRY_FILE` selects offline
    /// capture. The supervisor supplies the dedicated host credential.
    /// Ordinary Live consoles use the signed-in account. `H2_TELEMETRY=0`
    /// disables all three.
    pub fn from_live(
        live_enabled: bool,
        accounts_path: Option<&std::path::Path>,
        server_url: Option<&str>,
    ) -> Result<Self, String> {
        if std::env::var("H2_TELEMETRY").is_ok_and(|v| v == "0") {
            return Ok(Self::default());
        }
        if let Some(path) = std::env::var_os("H2_TELEMETRY_FILE").filter(|v| !v.is_empty()) {
            return Ok(Self::file(path.into()));
        }
        if let Some(token) = std::env::var_os("H2_HOST_TOKEN") {
            let token = token.into_string()
                .map_err(|_| "invalid host telemetry credential")?;
            let base_url = std::env::var("H2_HOST_LIVE_URL")
                .map_err(|_| "H2_HOST_LIVE_URL is required with H2_HOST_TOKEN")?;
            return Self::for_host(base_url, token, None);
        }
        if !live_enabled {
            return Ok(Self::default());
        }
        let (Some(accounts_path), Some(base_url)) = (accounts_path, server_url) else {
            return Ok(Self::default());
        };
        let bytes =
            std::fs::read(accounts_path).map_err(|e| format!("reading Live accounts: {e}"))?;
        let stored: Vec<StoredAccount> =
            serde_json::from_slice(&bytes).map_err(|_| "invalid Live accounts JSON".to_owned())?;
        let Some(first) = stored.into_iter().next() else {
            return Ok(Self::default());
        };
        let xuid = live_proto::Xuid::parse(&first.xuid)
            .map_err(|_| "invalid Live account XUID".to_owned())?
            .to_string();
        if first.gamertag.is_empty()
            || first.gamertag.len() > 15
            || !first
                .gamertag
                .bytes()
                .all(|b| b.is_ascii_graphic() || b == b' ')
        {
            return Err("invalid Live account gamertag".into());
        }
        if !live_proto::bearer_allowed(base_url) {
            return Err("telemetry server credentials require HTTPS or a loopback URL".into());
        }
        Ok(Self {
            enabled: true,
            sink: TelemetrySinkConfig::Server {
                base_url: base_url.to_owned(),
                account: TelemetryAccount {
                    xuid,
                    gamertag: first.gamertag,
                    key: first.key,
                },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::sign::KeyPair;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn accounts_file() -> (PathBuf, KeyPair) {
        let key = KeyPair::from_secret([11; 32]);
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-accounts-{}.json",
            live_proto::sign::random_id()
        ));
        let data = serde_json::json!([{
            "xuid": "0009A1B2C3D4E5F6",
            "gamertag": "Sneaky Pete",
            "key": key.private_jwk(),
            "server": "https://ignored.example"
        }]);
        std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        (path, key)
    }

    #[test]
    fn archive_cutover_preserves_sim_identity_and_disables_only_legacy_transport() {
        let game = live_proto::game::SimGame {
            run_id: "training-1".into(),
            batch_id: None,
            seed: "9".into(),
            gametype: "Slayer".into(),
            pace: "unpaced".into(),
            bots: vec![],
            deferred: false,
        };
        for archive in [HostArchive::default(), HostArchive::Film] {
            let config = TelemetryConfig::host_archive(
                "https://live.example".into(), "lease".into(), Some(game.clone()), archive,
            ).unwrap();
            assert_eq!(HostArchive::parse(archive.as_str()), Ok(archive));
            assert_eq!(config.film_only_host(), archive == HostArchive::Film);
            assert_eq!(config.enabled, archive == HostArchive::Dual);
            assert!(matches!(config.sink, TelemetrySinkConfig::Host { game: Some(stored), .. } if stored == game));
        }
        assert!(HostArchive::parse("films").is_err());
        assert!(!TelemetryConfig::default().film_only_host());
    }

    #[test]
    fn live_account_enables_server_without_credential_environment() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (path, key) = accounts_file();
        let config =
            TelemetryConfig::from_live(true, Some(&path), Some("http://localhost:8074")).unwrap();
        let TelemetrySinkConfig::Server { base_url, account } = config.sink else {
            panic!("expected server")
        };
        assert!(config.enabled);
        assert_eq!(base_url, "http://localhost:8074");
        assert_eq!(account.xuid, "0009a1b2c3d4e5f6");
        assert_eq!(account.gamertag, "Sneaky Pete");
        let parsed = KeyPair::from_private_jwk(&account.key).unwrap();
        assert_eq!(parsed.public(), key.public());
        assert!(!format!("{account:?}").contains(&key.secret_b64()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn opt_out_and_invalid_json_never_expose_keys() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (path, key) = accounts_file();
        unsafe { std::env::set_var("H2_TELEMETRY", "0") };
        let config =
            TelemetryConfig::from_live(true, Some(&path), Some("http://localhost:8074")).unwrap();
        assert!(!config.enabled);
        unsafe { std::env::remove_var("H2_TELEMETRY") };
        std::fs::write(
            &path,
            br#"[{"xuid":"private-test-marker","key":{"d":"private-test-marker"}}]"#,
        )
        .unwrap();
        let error = TelemetryConfig::from_live(true, Some(&path), Some("http://localhost:8074"))
            .unwrap_err();
        assert!(!error.contains("private-test-marker"));
        assert!(!format!("{:?}", key.private_jwk()).contains(&key.secret_b64()));
        let _ = std::fs::remove_file(path);
    }
}
