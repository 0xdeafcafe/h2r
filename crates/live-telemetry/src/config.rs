use std::fmt;
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelemetrySinkConfig {
    File(PathBuf),
    Server {
        base_url: String,
        account: TelemetryAccount,
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

    /// Boot-only configuration boundary. `H2_TELEMETRY_FILE` selects offline
    /// capture; an enabled Live configuration otherwise supplies the account
    /// and server. `H2_TELEMETRY=0` disables either source.
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
