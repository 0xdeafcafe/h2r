//! `/art/game/*`: art extracted from the owner's own disc (`h2 live assets
//! extract`), served beside the web app (docs/live/live-api.md, "Optional game
//! art"). By default only to clients on this machine, since the files are
//! the owner's copy of the game's art; `--share-game-art` serves them to
//! everyone.
//!
//! Shared art may also be published to the public blob store
//! ([`crate::blobs`], `--public-blobs`): [`publish`] uploads it once under
//! `art/<stamp>/`, and from then on a request for a file is answered with a
//! redirect to the bucket's public URL, so the bytes skip the server and its
//! tunnel. Art that is not shared is never published.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::ConnectInfo;
use axum::Router;
use axum::extract::Request;
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
                     `h2 live assets extract --out DIR` (docs/live/live-art.md)",
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

/// Shared art published to a public blob store, once [`publish`] has
/// finished: until then (and for anything it did not upload) the server
/// serves the files itself.
#[derive(Clone)]
pub struct PublishedArt {
    store: crate::blobs::BlobStore,
    stamp: Arc<OnceLock<String>>,
}

impl std::fmt::Debug for PublishedArt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishedArt")
            .field("store", &self.store)
            .field("stamp", &self.stamp.get())
            .finish()
    }
}

impl PublishedArt {
    /// `store` must have a public URL.
    pub fn new(store: crate::blobs::BlobStore) -> Self {
        PublishedArt {
            store,
            stamp: Arc::default(),
        }
    }

    /// The published extraction's stamp, once uploaded.
    pub fn stamp(&self) -> Option<&str> {
        self.stamp.get().map(String::as_str)
    }

    /// Where the world fetches `rel` (a path under the art directory).
    fn url(&self, rel: &str) -> Option<String> {
        let stamp = self.stamp()?;
        self.store.public_url(&format!("{ART_PREFIX}{stamp}/{rel}"))
    }

    #[cfg(feature = "native")]
    /// Uploads `art` ([`publish`]) and switches redirects on.
    pub async fn publish(&self, art: &GameArt) -> crate::blobs::Result<PublishReport> {
        let report = publish(art, &self.store).await?;
        let _ = self.stamp.set(report.stamp.clone());
        Ok(report)
    }
}

/// Art's key prefix in the public store.
pub const ART_PREFIX: &str = "art/";
/// The object [`publish`] writes last: the extraction is all there.
const DONE: &str = ".published";
/// Published files are named by extraction ([`stamp`]), so they never
/// change: a browser and Cloudflare's cache may keep them for good.
pub const PUBLISHED_CACHE: &str = "public, max-age=31536000, immutable";

/// What [`publish`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishReport {
    pub stamp: String,
    /// Files uploaded now (0 when this extraction was already published).
    pub uploaded: usize,
    pub files: usize,
}

/// Every file under `dir`, as `/`-separated relative paths, in order;
/// `.gz` copies left out (they are the encoded form of their sibling).
fn art_files(dir: &Path) -> std::io::Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                todo.push(path);
            } else if ty.is_file() && path.extension().is_none_or(|e| e != "gz") {
                let rel = path
                    .strip_prefix(dir)
                    .expect("under dir")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((rel, entry.metadata()?.len()));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The extraction's stamp: 16 hex digits of a SHA-256 over every file's
/// path and size and the manifest's bytes. A new extraction (a new
/// extractor, another disc) is a new stamp, so a new set of addresses.
pub fn stamp(dir: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (rel, size) in art_files(dir)? {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(size.to_le_bytes());
    }
    h.update(std::fs::read(dir.join(MANIFEST))?);
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(hex[..16].to_string())
}

#[cfg(feature = "native")]
/// Uploads shared `art` to `store` as `art/<stamp>/<path>`: each file with
/// its type and [`PUBLISHED_CACHE`]; a file with a `.gz` copy as that copy,
/// marked `Content-Encoding: gzip`. Files already there (same size) are
/// skipped, so an interrupted publish resumes; a published extraction is
/// recognised by its last object and costs one request. Refuses art that is
/// not shared.
pub async fn publish(
    art: &GameArt,
    store: &crate::blobs::BlobStore,
) -> crate::blobs::Result<PublishReport> {
    use futures_util::{StreamExt, TryStreamExt};
    if !art.share {
        return Err(crate::blobs::BlobError::Config(
            "game art is only published when it is shared (--share-game-art)".into(),
        ));
    }
    let dir = art.dir.clone();
    let (stamp, files) = tokio::task::spawn_blocking(move || {
        Ok::<_, std::io::Error>((stamp(&dir)?, art_files(&dir)?))
    })
    .await
    .map_err(|e| crate::blobs::BlobError::Config(e.to_string()))??;
    let base = format!("{ART_PREFIX}{stamp}/");
    if store.head(&format!("{base}{DONE}")).await?.is_some() {
        return Ok(PublishReport {
            stamp,
            uploaded: 0,
            files: files.len(),
        });
    }
    let count = files.len();
    let uploaded = futures_util::stream::iter(files)
        .map(|(rel, _)| {
            let (base, dir) = (&base, &art.dir);
            async move {
                let plain = dir.join(&rel);
                let gz = PathBuf::from(format!("{}.gz", plain.display()));
                let gz = gz.is_file().then_some(gz);
                let source = gz.as_deref().unwrap_or(&plain);
                let size = std::fs::metadata(source)?.len();
                let key = format!("{base}{rel}");
                if store.head(&key).await?.is_some_and(|i| i.size == size) {
                    return Ok::<_, crate::blobs::BlobError>(0);
                }
                let meta = crate::blobs::Meta {
                    content_type: Some(
                        mime_guess::from_path(&plain)
                            .first_or_octet_stream()
                            .essence_str()
                            .to_string(),
                    ),
                    content_encoding: gz.is_some().then(|| "gzip".to_string()),
                    cache_control: Some(PUBLISHED_CACHE.into()),
                };
                let bytes = tokio::fs::read(source).await?;
                store.put(&key, bytes.into(), &meta).await?;
                Ok(1)
            }
        })
        .buffer_unordered(8)
        .try_fold(0usize, |n, one| async move { Ok(n + one) })
        .await?;
    store
        .put(
            &format!("{base}{DONE}"),
            bytes::Bytes::from(format!("{count}\n")),
            &crate::blobs::Meta::default(),
        )
        .await?;
    Ok(PublishReport {
        stamp,
        uploaded,
        files: count,
    })
}

/// Where a request for `uri_path` (under [`PREFIX`]) should go instead of
/// being served here: the published file's public URL, when `published`
/// is ready, the file exists in `dir`, and the client can take how it was
/// stored (a gzipped one needs `Accept-Encoding: gzip`).
fn redirect_for(
    dir: &Path,
    published: &PublishedArt,
    uri_path: &str,
    headers: &HeaderMap,
) -> Option<String> {
    let rel = art_rel(uri_path)?;
    let rel = percent_encoding::percent_decode_str(rel)
        .decode_utf8()
        .ok()?;
    let plain = dir.join(&*rel);
    if !plain.is_file() {
        return None;
    }
    if Path::new(&format!("{}.gz", plain.display())).is_file() && !accepts_gzip(headers) {
        return None;
    }
    published.url(&rel)
}

#[cfg(feature = "native")]
/// `h2 live assets publish`: publishes the art in `dir` to the public store
/// at `to` (`s3://BUCKET[/PREFIX]` with the `H2_LIVE_S3_PUBLIC_*` or
/// `H2_LIVE_S3_*` settings from `env`, or a directory), readable by anyone
/// at `url`, for a server with no art files of its own (the Worker). The
/// art is shared by doing so. Returns what was done and the published
/// extraction's address, the Worker's `H2_LIVE_GAME_ART_URL`.
pub async fn publish_to(
    dir: &Path,
    to: &str,
    url: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> anyhow::Result<(PublishReport, String)> {
    use anyhow::Context;
    let Some(art) = GameArt::found(dir, true) else {
        anyhow::bail!(
            "{} has no {MANIFEST}; extract the art into it with \
             `h2 live assets extract --out DIR` (docs/live/live-art.md)",
            dir.display()
        );
    };
    crate::blobs::check_public_url(url)?;
    let store = crate::blobs::BlobStore::open(&crate::blobs::Location::parse(to)?, env, true)
        .context("opening the public store")?
        .with_public_url(url);
    let report = publish(&art, &store)
        .await
        .with_context(|| format!("publishing {} to {}", dir.display(), store.describe()))?;
    let base = store
        .public_url(&format!("{ART_PREFIX}{}", report.stamp))
        .expect("the store has a public URL");
    Ok((report, base))
}

/// The relative path a request for `uri_path` (under [`PREFIX`]) names,
/// still percent-encoded as it came, if it is one a published extraction
/// can hold: no empty, `.` or `..` segment, no backslash, not a `.gz`
/// copy.
fn art_rel(uri_path: &str) -> Option<&str> {
    let rel = uri_path.strip_prefix(PREFIX)?.strip_prefix('/')?;
    let decoded = percent_encoding::percent_decode_str(rel)
        .decode_utf8()
        .ok()?;
    let bad = decoded.is_empty()
        || decoded
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || s.contains('\\'))
        || decoded.ends_with(".gz");
    (!bad).then_some(rel)
}

/// For a server with no art files of its own (the Worker): where a request
/// for `uri_path` goes, given `base`, the published extraction's public
/// address (`<public bucket URL>/art/<stamp>`, what [`publish`] printed).
/// The redirect's `Cache-Control` is [`cache_control`] of the request's
/// query, as on a server that has the files.
pub fn published_redirect(
    base: &str,
    uri_path: &str,
    query: Option<&str>,
) -> Option<(String, &'static str)> {
    let rel = art_rel(uri_path)?;
    let to = format!("{}/{rel}", base.trim_end_matches('/'));
    Some((to, cache_control(versioned(query))))
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(axum::http::header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|e| {
            let mut parts = e.split(';');
            let name = parts.next().unwrap_or("").trim();
            let refused = parts.any(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .and_then(|q| q.trim().parse::<f32>().ok())
                    == Some(0.0)
            });
            (name == "gzip" || name == "*") && !refused
        })
}

#[cfg(feature = "native")]
/// The art routes: files from `art.dir` under [`PREFIX`], a JSON 404 for
/// anything missing or refused, so the web app falls back to its own art
/// the same way in both cases. With `published` ready, files are
/// redirects to their public copies instead (with the cache policy the
/// file itself would have had: a versioned address for a year, an
/// unversioned one asked about each time).
pub fn routes(art: &GameArt, published: Option<PublishedArt>) -> Router {
    let share = art.share;
    let dir = art.dir.clone();
    // Never publish what is not shared.
    let published = published.filter(|_| share);
    // A file's `.gz` copy (the extractor writes one beside each `map.glb`)
    // goes to a browser that takes gzip.
    let files = tower_http::services::ServeDir::new(&art.dir).precompressed_gzip();
    Router::new()
        .nest_service(PREFIX, files)
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let (dir, published) = (dir.clone(), published.clone());
            async move {
                let peer = req
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|c| c.0);
                if !allowed(share, peer, req.headers()) {
                    return not_found("game art is only served to this machine").into_response();
                }
                let stamped = versioned(req.uri().query());
                if let Some(to) = published
                    .as_ref()
                    .and_then(|p| redirect_for(&dir, p, req.uri().path(), req.headers()))
                {
                    let mut resp = axum::response::Redirect::temporary(&to).into_response();
                    resp.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static(cache_control(stamped)),
                    );
                    return resp;
                }
                let mut resp: Response = next.run(req).await;
                if resp.status() == axum::http::StatusCode::NOT_FOUND {
                    return not_found("no such game art").into_response();
                }
                resp.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static(cache_control(stamped)),
                );
                resp
            }
        }))
}

/// Whether a request names the art's version (`?v=<stamp>`, the
/// manifest's `source`, which every extraction changes): the web app asks
/// for map and model files so.
pub fn versioned(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        q.split('&')
            .any(|kv| kv.strip_prefix("v=").is_some_and(|v| !v.is_empty()))
    })
}

/// How long a browser may keep a file. Art changes in place when it is
/// extracted again, so an unversioned address is asked about each time (a
/// 304 while unchanged, never an old copy). A versioned one names one
/// extraction's file for good: the browser keeps it a year without asking
/// (a new extraction is a new address). Private: the art is the owner's.
pub fn cache_control(versioned: bool) -> &'static str {
    if versioned {
        "private, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
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

    /// Map geometry (docs/live/live-art.md) goes out with glTF and JSON types.
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
        let app = routes(&GameArt::found(&dir, true).unwrap(), None);
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
            let req = axum::http::Request::get(format!("{path}?v=0123abcd"))
                .body(Body::empty())
                .unwrap();
            let r = app.clone().oneshot(req).await.unwrap();
            assert_eq!(r.status(), axum::http::StatusCode::OK, "{path}?v=");
            assert_eq!(
                r.headers()[axum::http::header::CACHE_CONTROL],
                "private, max-age=31536000, immutable",
                "{path}: a versioned address is one extraction's file"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A map's geometry goes out gzipped, from the extractor's copy.
    #[tokio::test]
    async fn geometry_goes_out_compressed_when_the_browser_takes_it() {
        use axum::body::Body;
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("h2-art-gz-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("map/x")).unwrap();
        std::fs::write(dir.join(MANIFEST), "{}").unwrap();
        std::fs::write(dir.join("map/x/map.glb"), b"glTF plain").unwrap();
        std::fs::write(dir.join("map/x/map.glb.gz"), b"gzipped").unwrap();
        let app = routes(&GameArt::found(&dir, true).unwrap(), None);
        let get = |gzip: bool| {
            let mut req = axum::http::Request::get("/art/game/map/x/map.glb?v=s");
            if gzip {
                req = req.header("accept-encoding", "gzip, br");
            }
            app.clone().oneshot(req.body(Body::empty()).unwrap())
        };
        let r = get(true).await.unwrap();
        assert_eq!(r.headers()[axum::http::header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            r.headers()[axum::http::header::CONTENT_TYPE],
            "model/gltf-binary"
        );
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&body[..], b"gzipped");
        let r = get(false).await.unwrap();
        assert!(
            r.headers()
                .get(axum::http::header::CONTENT_ENCODING)
                .is_none()
        );
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&body[..], b"glTF plain");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Shared art published to a public bucket: uploaded once under its
    /// stamp with its type, encoding and a lasting cache policy, then each
    /// file is a redirect there, keeping the version-stamp cache rules.
    #[tokio::test]
    async fn published_art_redirects_to_the_public_bucket() {
        use axum::body::Body;
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("h2-art-pub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("map/x y")).unwrap();
        std::fs::write(dir.join(MANIFEST), r#"{"source":"s"}"#).unwrap();
        std::fs::write(dir.join("emblem.png"), b"png").unwrap();
        std::fs::write(dir.join("map/x y/map.glb"), b"glTF plain").unwrap();
        std::fs::write(dir.join("map/x y/map.glb.gz"), b"gzipped").unwrap();
        let bucket = crate::blobs::BlobStore::memory().with_public_url("https://art.example.net");

        // Art that is not shared never goes public.
        let mine = GameArt::found(&dir, false).unwrap();
        assert!(publish(&mine, &bucket).await.is_err());
        assert!(bucket.list("").await.unwrap().is_empty());

        let art = GameArt::found(&dir, true).unwrap();
        let published = PublishedArt::new(bucket.clone());
        let app = routes(&art, Some(published.clone()));
        let get = |path: &str, gzip: bool| {
            let mut req = axum::http::Request::get(path);
            if gzip {
                req = req.header("accept-encoding", "gzip, deflate, br");
            }
            app.clone().oneshot(req.body(Body::empty()).unwrap())
        };
        // Until it is uploaded, the server serves the files.
        let r = get("/art/game/emblem.png", true).await.unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::OK);

        let report = published.publish(&art).await.unwrap();
        assert_eq!((report.uploaded, report.files), (3, 3));
        let stamp = report.stamp.clone();
        assert_eq!(stamp.len(), 16);
        assert_eq!(stamp, super::stamp(&dir).unwrap());
        let base = format!("art/{stamp}/");
        let glb = bucket
            .get(&format!("{base}map/x y/map.glb"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&glb[..], b"gzipped", "stored as its gzip copy");
        assert!(
            bucket
                .get(&format!("{base}map/x y/map.glb.gz"))
                .await
                .unwrap()
                .is_none()
        );
        // Again: one request, nothing uploaded.
        assert_eq!(publish(&art, &bucket).await.unwrap().uploaded, 0);

        let r = get("/art/game/map/x%20y/map.glb?v=abc", true)
            .await
            .unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            r.headers()[axum::http::header::LOCATION],
            format!("https://art.example.net/art/{stamp}/map/x%20y/map.glb").as_str()
        );
        assert_eq!(
            r.headers()[axum::http::header::CACHE_CONTROL],
            "private, max-age=31536000, immutable"
        );
        let r = get("/art/game/emblem.png", false).await.unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(r.headers()[axum::http::header::CACHE_CONTROL], "no-cache");
        // A client that takes no gzip gets the plain file from here.
        let r = get("/art/game/map/x%20y/map.glb", false).await.unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&body[..], b"glTF plain");
        // Missing files and odd paths are not redirected.
        let r = get("/art/game/nope.png", true).await.unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::NOT_FOUND);
        let r = get("/art/game/map/../emblem.png", true).await.unwrap();
        assert_ne!(r.status(), axum::http::StatusCode::TEMPORARY_REDIRECT);

        // A new extraction is a new stamp.
        std::fs::write(dir.join("emblem.png"), b"png2!").unwrap();
        assert_ne!(super::stamp(&dir).unwrap(), stamp);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `h2 live assets publish`: the art goes to the named store, and the
    /// address it returns is where the Worker's redirects find each file.
    #[tokio::test]
    async fn publishing_for_the_worker_names_the_address() {
        let root = std::env::temp_dir().join(format!("h2-art-to-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (dir, bucket) = (root.join("art"), root.join("bucket"));
        std::fs::create_dir_all(dir.join("map/x")).unwrap();
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(dir.join(MANIFEST), r#"{"source":"s"}"#).unwrap();
        std::fs::write(dir.join("map/x/map.glb"), b"glTF").unwrap();
        let none = |_: &str| None;
        let to = format!("file:{}", bucket.display());
        let (report, base) = publish_to(&dir, &to, "https://art.example.net/", &none)
            .await
            .unwrap();
        assert_eq!((report.uploaded, report.files), (2, 2));
        assert_eq!(
            base,
            format!("https://art.example.net/art/{}", report.stamp)
        );
        let (to_url, _) = published_redirect(&base, "/art/game/map/x/map.glb", None).unwrap();
        let key = to_url.strip_prefix("https://art.example.net/").unwrap();
        assert_eq!(std::fs::read(bucket.join(key)).unwrap(), b"glTF");
        // Again: nothing to upload, the same address.
        let (again, base2) = publish_to(&dir, &to, "https://art.example.net", &none)
            .await
            .unwrap();
        assert_eq!((again.uploaded, base2), (0, base));
        // No art, or no public address: refused.
        assert!(
            publish_to(&bucket, &to, "https://a.example.net", &none)
                .await
                .is_err()
        );
        assert!(publish_to(&dir, &to, "ftp://a", &none).await.is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A server with no files of its own (the Worker) redirects to the
    /// published extraction by path alone, with the same cache rules, and
    /// refuses the paths a server with files refuses.
    #[test]
    fn redirects_without_the_files() {
        let base = "https://art.example.net/art/0123456789abcdef/";
        assert_eq!(
            published_redirect(base, "/art/game/map/x%20y/map.glb", Some("v=abc")),
            Some((
                "https://art.example.net/art/0123456789abcdef/map/x%20y/map.glb".into(),
                "private, max-age=31536000, immutable"
            ))
        );
        assert_eq!(
            published_redirect(base, "/art/game/manifest.json", None),
            Some((
                "https://art.example.net/art/0123456789abcdef/manifest.json".into(),
                "no-cache"
            ))
        );
        for bad in [
            "/art/game/",
            "/art/game",
            "/art/gamex/a.png",
            "/art/game/map/../manifest.json",
            "/art/game/map/%2e%2e/manifest.json",
            "/art/game/map//a.png",
            "/art/game/map/a%5Cb.png",
            "/art/game/map/x/map.glb.gz",
            "/art/game/%ff.png",
        ] {
            assert_eq!(published_redirect(base, bad, None), None, "{bad}");
        }
    }

    #[test]
    fn gzip_acceptance() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert("accept-encoding", v.parse().unwrap());
            accepts_gzip(&m)
        };
        assert!(h("gzip"));
        assert!(h("br, gzip;q=0.8"));
        assert!(h("*"));
        assert!(!h("br"));
        assert!(!h("gzip;q=0"));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[test]
    fn only_a_version_stamp_makes_art_immutable() {
        assert!(versioned(Some("v=abc")));
        assert!(versioned(Some("x=1&v=abc")));
        assert!(!versioned(Some("v=")));
        assert!(!versioned(Some("version=1")));
        assert!(!versioned(None));
        assert_eq!(cache_control(false), "no-cache");
    }
}
