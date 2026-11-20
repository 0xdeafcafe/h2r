//! What a server says about itself (docs/live/live-api.md "Read endpoints"; docs/live/live-admin.md §5):
//!
//! - `GET /v1/server`: the public facts — id, software, public URL, whether
//!   sign-up needs an invite, whether reads are private, and counts. A read
//!   like any other (a token on a `--private` server).
//! - `GET /v1/admin/status`: those plus what an admin running the server
//!   wants on one screen: native uptime, the tunnel, game art, admins, the
//!   log and the invites. Admin-only (an `admin.read` token from an admin account
//!   or the operator). Nothing secret: no keys, no file contents, no
//!   database URL. The data directory's path only to a caller on this
//!   machine (a loopback peer with no proxy headers, like game art).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::ConnectInfo;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use crate::app::{ApiError, Live, now};

/// What the router knows that [`Live`] does not: the game art it serves.
#[derive(Debug, Clone, Default)]
pub struct SiteFacts {
    pub game_art: Option<crate::art::GameArt>,
}

/// How the public URL is reached: a Cloudflare quick tunnel (a
/// `trycloudflare.com` name that changes every run), a named tunnel, or the
/// operator's own proxy (`--public-url`).
pub fn tunnel_kind(url: &str, cloudflare: bool) -> &'static str {
    let host = url
        .split_once("://")
        .map_or(url, |(_, r)| r)
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    if !cloudflare {
        "proxy"
    } else if host.ends_with(".trycloudflare.com") {
        "quick"
    } else {
        "named"
    }
}

impl Live {
    /// `GET /v1/server`.
    pub fn server_facts(&self) -> Value {
        let s = self.model.read();
        let live_games = s
            .games
            .values()
            .filter(|g| g.ended.is_none() && g.started.game.sim.is_none())
            .count();
        json!({
            "id": self.identity.id,
            "software": self.discovery().software,
            "url": self.policy().public.get(),
            "invite_only": self.policy().invite_only,
            "private": self.policy().private,
            "federation": false,
            "counts": {
                "accounts": s.accounts.len(),
                "online": s.presence.len(),
                "live_games": live_games,
                "sessions": s.sessions.len(),
            },
            "position": self.position(),
        })
    }

    /// `GET /v1/admin/status`, without the caller checks.
    pub async fn admin_status(&self, site: &SiteFacts, local: bool) -> Result<Value, ApiError> {
        let mut v = self.server_facts();
        let now = now();
        // A Durable Object's cold start does not measure service availability.
        #[cfg(feature = "native")]
        {
            v["started_at"] = json!(rfc3339(self.started_at));
            v["uptime_s"] = json!((now - self.started_at).max(0));
        }
        v["public"] = match self.policy().public.get_with_kind() {
            Some((url, cloudflare)) => {
                json!({"url": url, "tunnel": tunnel_kind(&url, cloudflare)})
            }
            None => json!({"url": null, "tunnel": null}),
        };
        v["game_art"] = art_facts(site.game_art.as_ref());
        let mut admins: Vec<Value> = self
            .admins
            .configured()
            .into_iter()
            .map(|t| json!({"gamertag": t, "source": "configured"}))
            .collect();
        for (xuid, tag) in self.admins.granted_list() {
            let configured = live_proto::Gamertag::new(&tag)
                .ok()
                .is_some_and(|t| self.admins.is_configured(&t));
            if !configured {
                admins.push(json!({"gamertag": tag, "xuid": xuid, "source": "granted"}));
            }
        }
        v["admins"] = json!(admins);
        v["log"] = json!({
            "position": self.position(),
            "head": self.store.head().await?,
            "store": self.store.kind(),
        });
        v["invites"] = self.invites.counts(now);
        if local && let Some(dir) = self.data_dir() {
            v["data_dir"] = json!(dir.display().to_string());
        }
        Ok(v)
    }
}

#[cfg(feature = "native")]
fn rfc3339(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

/// Whether game art is served, to whom, and which extraction made it (the
/// manifest's `version`, `extractor` when it records one, and `source`,
/// the disc's cache key). Never a path or a file's contents.
fn art_facts(art: Option<&crate::art::GameArt>) -> Value {
    let Some(art) = art else {
        return json!({"present": false, "shared": false});
    };
    let manifest: Value = std::fs::read(art.dir.join(crate::art::MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let pick = |k: &str| manifest.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "present": true,
        "shared": art.share,
        "manifest_version": pick("version"),
        "extractor": pick("extractor"),
        "source": pick("source"),
        "maps": manifest.get("maps").and_then(Value::as_object).map_or(0, |m| m.len()),
    })
}

/// A caller on this machine: a loopback peer that no proxy forwarded.
fn local_caller(req: &Request) -> bool {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    peer.is_some_and(|p| p.ip().is_loopback()) && !crate::public::forwarded(req.headers())
}

pub async fn get_server(State(live): State<Arc<Live>>, req: Request) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(req.headers()))?;
    Ok(crate::http::json(StatusCode::OK, &live.server_facts()))
}

pub async fn get_admin_status(
    State(live): State<Arc<Live>>,
    axum::Extension(site): axum::Extension<SiteFacts>,
    req: Request,
) -> Result<Response, ApiError> {
    crate::invites::admin(
        &live,
        crate::http::bearer(req.headers()),
        crate::invites::LXM_ADMIN_READ,
    )?;
    let v = live.admin_status(&site, local_caller(&req)).await?;
    Ok(crate::http::json(StatusCode::OK, &v))
}

/// Where the server keeps its key and log, for [`Live::set_data_dir`].
pub(crate) type DataDir = std::sync::OnceLock<PathBuf>;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "native"))]
    #[tokio::test]
    async fn worker_status_omits_uptime() {
        let live = Live::start_with(
            Arc::new(crate::store::MemoryStore::new()),
            crate::ServerIdentity {
                id: "live.test".into(),
                key: live_proto::sign::KeyPair::generate(),
            },
            crate::Policy::default(),
            crate::app::Timers::Driven,
        )
        .await
        .unwrap();
        let status = live.admin_status(&SiteFacts::default(), false).await.unwrap();
        assert_eq!(status["id"], "live.test");
        assert!(status.get("started_at").is_none());
        assert!(status.get("uptime_s").is_none());
    }

    #[test]
    fn tunnel_kinds() {
        assert_eq!(
            tunnel_kind("https://a-b-c.trycloudflare.com", true),
            "quick"
        );
        assert_eq!(tunnel_kind("https://live.example.net", true), "named");
        assert_eq!(
            tunnel_kind("https://trycloudflare.com.evil.net", true),
            "named"
        );
        assert_eq!(
            tunnel_kind("https://live.example.net:8443/x", false),
            "proxy"
        );
    }

    #[test]
    fn art_facts_never_name_the_directory() {
        assert_eq!(art_facts(None)["present"], false);
        let dir = std::env::temp_dir().join(format!("h2-status-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::art::MANIFEST),
            r#"{"version":1,"extractor":7,"source":"abc","maps":{"a":{},"b":{}}}"#,
        )
        .unwrap();
        let v = art_facts(Some(&crate::art::GameArt {
            dir: dir.clone(),
            share: false,
        }));
        assert_eq!(v["present"], true);
        assert_eq!(v["shared"], false);
        assert_eq!(v["extractor"], 7);
        assert_eq!(v["maps"], 2);
        assert!(!v.to_string().contains(&dir.display().to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
