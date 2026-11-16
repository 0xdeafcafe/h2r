//! `/art/game/*`: art extracted from the owner's own disc (`h2 live assets
//! extract`), served beside the web app (docs/live-api.md, "Optional game
//! art"). By default only to clients on this machine, since the files are
//! the owner's copy of the game's art; `--share-game-art` serves them to
//! everyone.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::extract::{ConnectInfo, Request};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};

use crate::app::not_found;

/// The manifest's file name inside the art directory.
pub const MANIFEST: &str = "manifest.json";
/// Where the art is mounted.
pub const PREFIX: &str = "/art/game";

/// Extracted art and who may fetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameArt {
    pub dir: PathBuf,
    /// Serve to any client, not only loopback ones.
    pub share: bool,
}

impl GameArt {
    /// `dir` if it holds a manifest.
    pub fn found(dir: &Path, share: bool) -> Option<Self> {
        dir.join(MANIFEST).is_file().then(|| GameArt {
            dir: dir.to_path_buf(),
            share,
        })
    }

    /// The art to serve: `flag` (`--game-art DIR`), which must hold a
    /// manifest, else `default` if art was extracted there, else none.
    pub fn from_flag(
        flag: Option<PathBuf>,
        default: &Path,
        share: bool,
    ) -> anyhow::Result<Option<Self>> {
        match flag {
            Some(dir) => match Self::found(&dir, share) {
                Some(art) => Ok(Some(art)),
                None => anyhow::bail!(
                    "{} has no {MANIFEST}; extract the art into it with \
                     `h2 live assets extract --out DIR` (docs/live-art.md)",
                    dir.display()
                ),
            },
            None => Ok(Self::found(default, share)),
        }
    }
}

/// Whether a request may have the art: always when shared; otherwise only
/// from a loopback peer that is not a proxy or tunnel forwarding someone
/// else (any proxy header, [`crate::public::forwarded`]). A request whose
/// peer is unknown is refused.
pub fn allowed(share: bool, peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    if share {
        return true;
    }
    peer.is_some_and(|p| p.ip().is_loopback()) && !crate::public::forwarded(headers)
}

/// The art routes: files from `art.dir` under [`PREFIX`], a JSON 404 for
/// anything missing or refused, so the web app falls back to its own art
/// the same way in both cases.
pub fn routes(art: &GameArt) -> Router {
    let share = art.share;
    let files = tower_http::services::ServeDir::new(&art.dir);
    Router::new()
        .nest_service(PREFIX, files)
        .layer(middleware::from_fn(
            move |req: Request, next: Next| async move {
                let peer = req
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|c| c.0);
                if !allowed(share, peer, req.headers()) {
                    return not_found("game art is only served to this machine").into_response();
                }
                let mut resp: Response = next.run(req).await;
                if resp.status() == axum::http::StatusCode::NOT_FOUND {
                    return not_found("no such game art").into_response();
                }
                // Art changes in place when it is extracted again (new
                // models, materials): browsers ask each time and get a 304
                // while it is unchanged, never an old copy.
                resp.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-cache"),
                );
                resp
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only_unless_shared() {
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let lo6: SocketAddr = "[::1]:5000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.20:5000".parse().unwrap();
        let none = HeaderMap::new();
        assert!(allowed(false, Some(lo), &none));
        assert!(allowed(false, Some(lo6), &none));
        assert!(!allowed(false, Some(lan), &none));
        assert!(!allowed(false, None, &none), "an unknown peer is refused");
        let mut proxied = HeaderMap::new();
        proxied.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert!(
            !allowed(false, Some(lo), &proxied),
            "a local proxy forwarding a stranger"
        );
        for (name, value) in [
            ("forwarded", "for=203.0.113.9"),
            ("cf-connecting-ip", "203.0.113.9"),
            ("cf-ray", "8f1e2d3c4b5a6978-LHR"),
            ("x-forwarded-proto", "https"),
        ] {
            let mut tunnelled = HeaderMap::new();
            tunnelled.insert(name, value.parse().unwrap());
            assert!(
                !allowed(false, Some(lo), &tunnelled),
                "cloudflared on loopback delivering {name}"
            );
        }
        assert!(allowed(true, Some(lan), &proxied));
        assert!(allowed(true, None, &none));
    }

    #[test]
    fn a_named_art_dir_needs_a_manifest() {
        let dir = std::env::temp_dir().join(format!("h2-art-flag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let given = dir.join("given");
        let default = dir.join("default");
        std::fs::create_dir_all(&given).unwrap();
        let err = GameArt::from_flag(Some(given.clone()), &default, true).unwrap_err();
        assert!(err.to_string().contains("no manifest.json"), "{err}");
        assert_eq!(GameArt::from_flag(None, &default, true).unwrap(), None);
        std::fs::write(given.join(MANIFEST), "{}").unwrap();
        let art = GameArt::from_flag(Some(given.clone()), &default, true).unwrap();
        assert_eq!(
            art,
            Some(GameArt {
                dir: given,
                share: true
            })
        );
        std::fs::create_dir_all(&default).unwrap();
        std::fs::write(default.join(MANIFEST), "{}").unwrap();
        let art = GameArt::from_flag(None, &default, false).unwrap();
        assert_eq!(
            art,
            Some(GameArt {
                dir: default,
                share: false
            })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Map geometry (docs/live-art.md) goes out with glTF and JSON types.
    #[tokio::test]
    async fn geometry_files_have_their_content_types() {
        use axum::body::Body;
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("h2-art-types-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("map/x/views")).unwrap();
        std::fs::write(dir.join(MANIFEST), "{}").unwrap();
        std::fs::write(dir.join("map/x/map.glb"), b"glTF").unwrap();
        std::fs::write(dir.join("map/x/views.json"), "{}").unwrap();
        std::fs::write(dir.join("map/x/views/top.png"), b"png").unwrap();
        let app = routes(&GameArt::found(&dir, true).unwrap());
        for (path, ty) in [
            ("/art/game/map/x/map.glb", "model/gltf-binary"),
            ("/art/game/map/x/views.json", "application/json"),
            ("/art/game/map/x/views/top.png", "image/png"),
        ] {
            let req = axum::http::Request::get(path).body(Body::empty()).unwrap();
            let r = app.clone().oneshot(req).await.unwrap();
            assert_eq!(r.status(), axum::http::StatusCode::OK, "{path}");
            assert_eq!(r.headers()[axum::http::header::CONTENT_TYPE], ty, "{path}");
            assert_eq!(
                r.headers()[axum::http::header::CACHE_CONTROL],
                "no-cache",
                "{path}: re-extracted art must not be served from a stale cache"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
