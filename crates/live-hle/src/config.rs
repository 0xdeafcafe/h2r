use live_proto::sign::{KeyPair, PrivateJwk};
use serde::Deserialize;
use std::sync::Arc;
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::PathBuf,
};

/// One client configuration, supplied explicitly by the host. No environment
/// variables, default account paths or server connections are read here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveConfig {
    pub enabled: bool,
    pub accounts_path: Option<PathBuf>,
    /// Base URL for all Live service calls. None selects local sign-in only.
    pub server_url: Option<String>,
    /// List [`NEW_ACCOUNT_TAG`] for sign-in beside the accounts, and read a
    /// missing accounts file as none: picking it makes or adds an account
    /// in game ([`crate::signin`]). The app builds set it
    /// ([`crate::platform::config`]); they have no terminal to ask in.
    pub offer_new_account: bool,
}

/// The identity listed when [`LiveConfig::offer_new_account`] is set; its
/// xuid is outside the stand-ins' range ([`crate::account_setup::stand_in_xuid`]).
pub const NEW_ACCOUNT_TAG: &str = "New Account";
pub const NEW_ACCOUNT_XUID: u64 = 0x0009_0001_0000_0000;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not configure the Live HTTP worker")]
    Worker,
    #[error("invalid Live network configuration: {0}")]
    Network(String),
    #[error("reading Live accounts: {0}")]
    Read(#[from] std::io::Error),
    // Never format serde's input/type errors: the file may contain private keys.
    #[error("invalid Live accounts JSON")]
    Json,
    #[error(
        "Live accounts must contain at most 16 distinct nonzero hex XUIDs and 1..15 byte ASCII gamertags"
    )]
    Accounts,
}

#[derive(Deserialize)]
struct StoredAccount {
    xuid: String,
    gamertag: String,
    #[serde(default)]
    key: Option<serde_json::Value>,
    /// The player's identity, listed for sign-in, whose key for this server
    /// is not on this machine yet: picking it asks for a recovery code in
    /// game (docs/live/live.md, "In-game sign-in").
    #[serde(default)]
    needs_code: bool,
    // The host-supplied server_url is authoritative. Ignore server/future fields.
}

#[derive(Clone)]
pub(crate) struct Account {
    pub xuid: u64,
    pub gamertag: String,
    pub key: Option<Arc<KeyPair>>,
    /// Signing in asks for a code first ([`crate::signin`]); no key is
    /// kept for it.
    pub needs_code: bool,
}

impl LiveConfig {
    pub(crate) fn accounts(&self) -> Result<Vec<Account>, ConfigError> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        let mut accounts = match (&self.accounts_path, self.offer_new_account) {
            (None, _) => Vec::new(),
            (Some(path), offer) => match std::fs::read(path) {
                Ok(json) => parse_accounts(&json)?,
                // The app's first start: no account yet, made in game.
                Err(e) if offer && e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(e.into()),
            },
        };
        if self.offer_new_account
            && self.server_url.is_some()
            && accounts.len() < 16
            && accounts.iter().all(|a| a.xuid != NEW_ACCOUNT_XUID)
        {
            accounts.push(Account {
                xuid: NEW_ACCOUNT_XUID,
                gamertag: NEW_ACCOUNT_TAG.into(),
                key: None,
                needs_code: true,
            });
        }
        Ok(accounts)
    }
}

pub(crate) fn parse_accounts(json: &[u8]) -> Result<Vec<Account>, ConfigError> {
    let stored: Vec<StoredAccount> = serde_json::from_slice(json).map_err(|_| ConfigError::Json)?;
    if stored.len() > 16 {
        return Err(ConfigError::Accounts);
    }
    let mut seen = BTreeSet::new();
    let accounts: Vec<Account> = stored
        .into_iter()
        .map(|s| {
            let hex = s
                .xuid
                .strip_prefix("0x")
                .or_else(|| s.xuid.strip_prefix("0X"))
                .unwrap_or(&s.xuid);
            let xuid =
                (hex.len() <= 16 && !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| u64::from_str_radix(hex, 16).ok())
                    .flatten()
                    .filter(|x| *x != 0)
                    .ok_or(ConfigError::Accounts)?;
            if !seen.insert(xuid)
                || s.gamertag.is_empty()
                || s.gamertag.len() > 15
                || !s
                    .gamertag
                    .bytes()
                    .all(|b| b.is_ascii_graphic() || b == b' ')
            {
                return Err(ConfigError::Accounts);
            }
            Ok(Account {
                xuid,
                gamertag: s.gamertag,
                // A missing/invalid key makes this identity unusable for server
                // logon, while old identity-only files still serve System Link.
                // Never retain or format serde/JWK errors or input values.
                key: s
                    .key
                    .filter(|_| !s.needs_code)
                    .and_then(|v| serde_json::from_value::<PrivateJwk>(v).ok())
                    .and_then(|j| KeyPair::from_private_jwk(&j).ok())
                    .map(Arc::new),
                needs_code: s.needs_code,
            })
        })
        .collect::<Result<_, _>>()?;
    // An identity waiting for a code is listed once, and not beside a
    // signed-in-ready account of the same gamertag.
    let ready: BTreeSet<String> = accounts
        .iter()
        .filter(|a| !a.needs_code && a.key.is_some())
        .map(|a| a.gamertag.to_ascii_lowercase())
        .collect();
    let mut waiting = BTreeSet::new();
    Ok(accounts
        .into_iter()
        .filter(|a| {
            let tag = a.gamertag.to_ascii_lowercase();
            !a.needs_code || (!ready.contains(&tag) && waiting.insert(tag))
        })
        .collect())
}

/// Ask the host routing table for its default-route IPv4 without sending a
/// datagram. With no IPv4 route, an offline host uses loopback. Port 3074 is
/// the default host base for the title's virtual datagram channels.
pub(crate) fn lan_address() -> SocketAddrV4 {
    let ip = (|| {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
        match socket.local_addr().ok()? {
            std::net::SocketAddr::V4(a) => Some(*a.ip()),
            _ => None,
        }
    })()
    .unwrap_or(Ipv4Addr::LOCALHOST);
    SocketAddrV4::new(ip, 3074)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_only_accounts_ignore_invalid_keys_server_and_future_fields() {
        let accounts = parse_accounts(br#"[{"xuid":"0X00091234567890AB","gamertag":"Player","server":"https://example/base","key":{"d":"private-test-marker","x":"public-test-marker"},"future":[1,2]}]"#).unwrap();
        assert_eq!(accounts[0].xuid, 0x0009_1234_5678_90ab);
        assert_eq!(accounts[0].gamertag, "Player");
        assert!(accounts[0].key.is_none());
    }
    #[test]
    fn identities_waiting_for_a_code_are_listed_once_without_a_key() {
        let key = KeyPair::generate();
        let data = serde_json::json!([
            {"xuid":"0009000000000001", "gamertag":"Ready", "key":key.private_jwk()},
            // The same gamertag ready to sign in: not listed twice.
            {"xuid":"0009000000000002", "gamertag":"ready", "needs_code":true},
            // A key from another server is never used for this one.
            {"xuid":"0009000000000003", "gamertag":"Away", "needs_code":true, "key":key.private_jwk()},
            {"xuid":"0009000000000004", "gamertag":"AWAY", "needs_code":true},
        ]);
        let accounts = parse_accounts(&serde_json::to_vec(&data).unwrap()).unwrap();
        let listed: Vec<_> = accounts
            .iter()
            .map(|a| (a.gamertag.as_str(), a.needs_code, a.key.is_some()))
            .collect();
        assert_eq!(listed, [("Ready", false, true), ("Away", true, false)]);
    }
    #[test]
    fn accounts_parse_private_jwks_without_exposing_secrets() {
        let key = KeyPair::generate();
        let data = serde_json::json!([{"xuid":"00091234567890ab", "gamertag":"Player", "key":key.private_jwk(), "server":"https://ignored.example"}]);
        let accounts = parse_accounts(&serde_json::to_vec(&data).unwrap()).unwrap();
        let parsed = accounts[0].key.as_ref().unwrap();
        assert_eq!(parsed.public(), key.public());
        assert!(!format!("{parsed:?}").contains(&key.secret_b64()));
    }
    #[test]
    fn account_errors_never_echo_contents_or_private_key_material() {
        for data in [
            r#"[{"xuid":"0009000011112222","gamertag":{"d":"private-test-marker"},"key":{"d":"private-test-marker"}}]"#,
            r#"[{"xuid":"private-test-marker","gamertag":"Player"}]"#,
            r#"[{"xuid":"1","gamertag":"Player","key":{"d":"private-test-marker"}}"#,
        ] {
            let error = match parse_accounts(data.as_bytes()) {
                Err(e) => e,
                Ok(_) => panic!("invalid fixture"),
            };
            assert!(!format!("{error:?} {error}").contains("private-test-marker"));
        }
    }
    #[test]
    fn accounts_reject_duplicate_zero_malformed_ids_and_invalid_names() {
        for data in [
            r#"[{"xuid":"0","gamertag":"Player"}]"#,
            r#"[{"xuid":"-1","gamertag":"Player"}]"#,
            r#"[{"xuid":"10000000000000000","gamertag":"Player"}]"#,
            r#"[{"xuid":"1","gamertag":""}]"#,
            r#"[{"xuid":"1","gamertag":"sixteenlettersxx"}]"#,
            r#"[{"xuid":"1","gamertag":"bad\u0000name"}]"#,
            r#"[{"xuid":"1","gamertag":"é"}]"#,
            r#"[{"xuid":"1","gamertag":"One"},{"xuid":"0001","gamertag":"Two"}]"#,
        ] {
            assert!(parse_accounts(data.as_bytes()).is_err());
        }
    }
    #[test]
    fn accounts_limit_is_sixteen_and_empty_is_supported() {
        assert!(parse_accounts(b"[]").unwrap().is_empty());
        let mut records = Vec::new();
        for id in 1..=17 {
            records.push(format!(r#"{{"xuid":"{id:x}","gamertag":"Player{id}"}}"#));
        }
        assert!(parse_accounts(format!("[{}]", records[..16].join(",")).as_bytes()).is_ok());
        assert!(parse_accounts(format!("[{}]", records.join(",")).as_bytes()).is_err());
    }
    #[test]
    fn missing_enabled_accounts_file_is_an_error_and_default_is_disabled() {
        assert_eq!(
            LiveConfig::default(),
            LiveConfig {
                enabled: false,
                accounts_path: None,
                server_url: None,
                offer_new_account: false,
            }
        );
        let config = LiveConfig {
            enabled: true,
            accounts_path: Some("/nonexistent/live-hle-accounts.json".into()),
            server_url: Some("https://server.example/base".into()),
            offer_new_account: false,
        };
        assert!(matches!(config.accounts(), Err(ConfigError::Read(_))));
    }
    #[test]
    fn the_app_lists_a_new_account_row_and_starts_with_no_file() {
        let config = LiveConfig {
            enabled: true,
            accounts_path: Some("/nonexistent/live-hle-accounts.json".into()),
            server_url: Some("https://server.example/base".into()),
            offer_new_account: true,
        };
        let accounts = config.accounts().unwrap();
        let listed: Vec<_> = accounts
            .iter()
            .map(|a| (a.xuid, a.gamertag.as_str(), a.needs_code))
            .collect();
        assert_eq!(listed, [(NEW_ACCOUNT_XUID, NEW_ACCOUNT_TAG, true)]);
        // Without a server there is nothing to make an account on.
        let offline = LiveConfig {
            server_url: None,
            ..config.clone()
        };
        assert!(offline.accounts().unwrap().is_empty());
        // A file that is there but bad is still an error.
        let dir = std::env::temp_dir().join(format!("live-hle-new-row-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.json");
        std::fs::write(&path, b"not json").unwrap();
        let bad = LiveConfig {
            accounts_path: Some(path),
            ..config
        };
        assert!(matches!(bad.accounts(), Err(ConfigError::Json)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
