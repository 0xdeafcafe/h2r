//! The Worker's settings: wrangler.toml variables and secrets, under the
//! names `h2-live-server` reads from its environment where there is one
//! (docs/live/live-worker.md, "Settings").
//!
//! | name | kind | meaning |
//! |---|---|---|
//! | `H2_LIVE_ID` | variable | the server id, the host in every author id |
//! | `H2_LIVE_SERVER_KEY` | secret | the server's Ed25519 key, base64 as in `server.key` |
//! | `H2_LIVE_PUBLIC_URL` | variable | the URL clients use, shown in discovery |
//! | `H2_LIVE_ADMINS` | variable | admin gamertags, comma-separated |
//! | `H2_LIVE_INVITE_ONLY` | variable | `1`: account creation needs an invite |
//! | `H2_LIVE_PRIVATE` | variable | `1`: reads need a token too |
//! | `H2_LIVE_GAME_ART_URL` | variable | published game art, `<public bucket URL>/art/<stamp>` ([`crate::site`]) |

use live_server::admin::Admins;
use live_server::{Policy, ServerIdentity};

pub const ID: &str = "H2_LIVE_ID";
pub const SERVER_KEY: &str = "H2_LIVE_SERVER_KEY";
pub const PUBLIC_URL: &str = "H2_LIVE_PUBLIC_URL";
pub const ADMINS: &str = "H2_LIVE_ADMINS";
pub const INVITE_ONLY: &str = "H2_LIVE_INVITE_ONLY";
pub const PRIVATE: &str = "H2_LIVE_PRIVATE";
pub const GAME_ART_URL: &str = "H2_LIVE_GAME_ART_URL";

/// What the hub starts with.
pub struct Settings {
    pub identity: ServerIdentity,
    pub policy: Policy,
}

fn flag(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Result<bool, String> {
    match env(name).as_deref().map(str::trim) {
        None | Some("" | "0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(other) => Err(format!("{name} is 1 or 0, not {other:?}")),
    }
}

impl Settings {
    /// From the Worker's variables and secrets (`env` looks a name up in
    /// both). A missing id or key, or a bad value, is an error: the hub
    /// answers 500 with it rather than start as some other server.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let id = env(ID).ok_or(format!("{ID} is not set (wrangler.toml [vars])"))?;
        let key = env(SERVER_KEY).ok_or(format!(
            "{SERVER_KEY} is not set (wrangler secret put {SERVER_KEY})"
        ))?;
        let identity = ServerIdentity::from_secret(id.trim(), &key).map_err(|e| e.to_string())?;
        let admins: Vec<String> = env(ADMINS)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        let policy = Policy {
            private: flag(env, PRIVATE)?,
            invite_only: flag(env, INVITE_ONLY)?,
            admins: Admins::from_flags_and_env(&admins)?,
            ..Policy::default()
        };
        let url = env(PUBLIC_URL)
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("https://{}", identity.id));
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(format!("{PUBLIC_URL} wants https://HOST, got {url:?}"));
        }
        // Every request reaches a Worker through Cloudflare's edge.
        policy.public.set_edge(&url);
        Ok(Settings { identity, policy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::sign::KeyPair;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn settings_come_from_variables_and_secrets() {
        let key = KeyPair::generate().secret_b64();
        let s = Settings::from_env(&env(&[
            (ID, "live.example.net"),
            (SERVER_KEY, &format!("{key}\n")),
            (ADMINS, "Alpha, Bravo"),
            (INVITE_ONLY, "1"),
            (PRIVATE, "0"),
        ]))
        .unwrap();
        assert_eq!(s.identity.id, "live.example.net");
        assert_eq!(s.identity.key.secret_b64(), key);
        assert!(s.policy.invite_only);
        assert!(!s.policy.private);
        let admins: Vec<String> = s
            .policy
            .admins
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        assert_eq!(admins, ["Alpha", "Bravo"]);
        assert_eq!(
            s.policy.public.get_with_kind(),
            Some(("https://live.example.net".into(), true))
        );
    }

    #[test]
    fn the_public_url_and_bad_settings() {
        let key = KeyPair::generate().secret_b64();
        let s = Settings::from_env(&env(&[
            (ID, "live.example.net"),
            (SERVER_KEY, &key),
            (PUBLIC_URL, "https://play.example.net/"),
        ]))
        .unwrap();
        assert_eq!(
            s.policy.public.get().as_deref(),
            Some("https://play.example.net")
        );
        assert!(!s.policy.invite_only && s.policy.admins.is_empty());

        let err = |pairs: &[(&str, &str)]| Settings::from_env(&env(pairs)).err().unwrap();
        assert!(err(&[(SERVER_KEY, &key)]).contains(ID));
        assert!(err(&[(ID, "live.example.net")]).contains("wrangler secret put"));
        assert!(
            err(&[(ID, "live.example.net"), (SERVER_KEY, "nope")]).contains("not a server key")
        );
        assert!(
            err(&[
                (ID, "live.example.net"),
                (SERVER_KEY, &key),
                (INVITE_ONLY, "yes")
            ])
            .contains("1 or 0")
        );
        assert!(
            err(&[
                (ID, "live.example.net"),
                (SERVER_KEY, &key),
                (ADMINS, "not a tag!")
            ])
            .contains("not a tag")
        );
    }
}
