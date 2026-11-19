//! The Xbox Live replacement service (docs/live/live.md §4.3, docs/live/live-api.md,
//! docs/live/live-security.md): signed events in an append-only log (SQLite
//! or Postgres), projected into memory, served over HTTP and WebSocket.
//!
//! - [`store`]: the log ([`sqlite::SqliteStore`], [`pg::PostgresStore`],
//!   [`filestore::FileStore`], [`store::MemoryStore`]).
//! - [`blobs`]: content, cloud saves and published art, in the data
//!   directory or an S3-compatible bucket (R2); [`copy`] moves a server's
//!   log and blobs between backends.
//! - [`dispatch`]: the one tailer, feeding [`dispatch::Subscriber`]s.
//! - [`model`]: the read model, a subscriber replayed from 0 at startup.
//! - [`app`]: authentication, write rules, operations.
//! - [`http`]: the routes.
//! - [`identity`]: the server key and id in the data directory.
//!
//! Signing and verification live in `live_proto::sign`, shared with the
//! clients. Run it with `h2 live serve`, or alone with `h2-live-server`
//! ([`standalone`], the Docker image).

// Without `native` this is the Worker's build (crates/live-worker): the
// sockets, SQL drivers and file serving are left out, and what only they
// used goes unused.
#![cfg_attr(not(feature = "native"), allow(dead_code, unused_imports))]

pub mod admin;
pub mod app;
pub mod art;
#[cfg(feature = "native")]
pub mod art_pack;
pub mod arbitration;
pub mod bans;
pub mod blobs;
pub mod bots;
pub mod caching;
#[cfg(feature = "native")]
pub mod check;
pub mod cloud_saves;
pub mod content;
#[cfg(feature = "native")]
pub mod copy;
pub mod default_hopper;
pub mod default_motd;
pub mod dispatch;
pub mod emblem;
pub mod filestore;
pub mod games;
pub mod guard;
pub mod hold;
// Dedicated host lifecycle and billing.
pub mod host_costs;
pub mod hosts;
pub mod http;
#[cfg(feature = "native")]
pub mod httpstore;
pub mod host_identity; // dh-identity
pub mod identity;
pub mod ingest;
pub mod invites;
pub mod local_signin;
pub mod logio;
mod mailbox;
mod matchmaking;
pub mod model;
pub mod mutes;
pub mod page_header;
pub mod page_meta;
pub mod packed;
#[cfg(feature = "native")]
pub mod pg;
pub mod public;
pub mod ranks;
pub mod recovery;
pub mod signins;
pub mod relay;
pub mod reports;
mod replay_archive;
pub mod rt;
mod sessions;
mod sim_runs;
pub mod sql;
#[cfg(feature = "native")]
pub mod sqlite;
#[cfg(feature = "native")]
pub mod standalone;
pub mod stats;
pub mod status;
pub mod store;
pub mod stream;
mod teams;
pub mod title_headers;
pub mod titleblobs;
pub mod usage;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

pub use app::{ApiError, Live, MAX_TITLE_FILE, Policy};

/// The peer's address, as axum's listener records it. A Worker has no
/// socket: there the type exists but is never set, so no request counts
/// as from this machine.
#[cfg(feature = "native")]
pub use axum::extract::ConnectInfo;
#[cfg(not(feature = "native"))]
#[derive(Debug, Clone, Copy)]
pub struct ConnectInfo<T>(pub T);
pub use identity::ServerIdentity;
pub use sessions::{CoopGame, CoopLink, Viewer, strip_addresses};

/// The Postgres database servers used before SQLite was the default, and
/// the one `h2 play --live`'s embedded server tries first.
pub const DEFAULT_DB: &str = "postgres://localhost/h2live";
/// The SQLite database's file name in the data directory: the default log.
pub const DB_FILE: &str = "live.db";
/// Used when `--listen` is not set.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8074";

pub struct Config {
    /// `--db`/`H2_LIVE_DB`; `None`: [`resolve_db`] picks.
    pub db: Option<String>,
    /// Where blobs go ([`blobs::Blobs::configure`]).
    pub blobs: BlobFlags,
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    /// The server id; fixed on first start.
    pub id: Option<String>,
    pub policy: Policy,
    /// PEM certificate chain and key: serve HTTPS.
    pub tls: Option<(PathBuf, PathBuf)>,
    /// Allow plain HTTP on a non-loopback address.
    pub insecure_http: bool,
    /// The built web app, served at `/` ([`web_dir`]), and game art.
    pub site: http::Site,
}

/// `--blobs`, `--public-blobs`, `--public-blobs-url` (or `H2_LIVE_BLOBS`,
/// `H2_LIVE_PUBLIC_BLOBS`, `H2_LIVE_PUBLIC_BLOBS_URL`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobFlags {
    pub private: Option<String>,
    pub public: Option<String>,
    pub public_url: Option<String>,
}

impl BlobFlags {
    /// Fills what the flags left out from the environment.
    pub fn or_env(self, env: &dyn Fn(&str) -> Option<String>) -> Self {
        BlobFlags {
            private: self.private.or_else(|| env("H2_LIVE_BLOBS")),
            public: self.public.or_else(|| env("H2_LIVE_PUBLIC_BLOBS")),
            public_url: self.public_url.or_else(|| env("H2_LIVE_PUBLIC_BLOBS_URL")),
        }
    }
}

#[cfg(feature = "native")]
/// `sqlite://<data>/live.db`.
pub fn default_db(data_dir: &std::path::Path) -> String {
    sqlite::url_for(&data_dir.join(DB_FILE))
}

#[cfg(feature = "native")]
/// The log to open when `--db` is not given: SQLite in the data directory
/// for a new server, or one that already has it. A server from before
/// SQLite was the default kept its log in [`DEFAULT_DB`]: if that database
/// is reachable and holds events (and there is no `live.db`), it is used,
/// with a notice saying how to move it (`store copy`), so an upgrade never
/// starts on an empty log by surprise.
pub async fn resolve_db(
    flag: Option<&str>,
    data_dir: &std::path::Path,
) -> (String, Option<String>) {
    if let Some(db) = flag {
        return (db.to_string(), None);
    }
    let sqlite = default_db(data_dir);
    if data_dir.join(DB_FILE).exists() {
        return (sqlite, None);
    }
    match pg::existing_log(DEFAULT_DB).await {
        Some(head) if head > 0 => {
            let notice = format!(
                "using the log in {DEFAULT_DB} (at position {head}): servers keep it in                  SQLite now; move it with `store copy --from {DEFAULT_DB} --to {sqlite}`,                  or pass --db {DEFAULT_DB} to keep Postgres and silence this"
            );
            (DEFAULT_DB.to_string(), Some(notice))
        }
        _ => (sqlite, None),
    }
}

/// Loopback serves plain HTTP; anything else needs TLS or an explicit
/// `--insecure-http` (security §6).
pub fn check_bind(listen: SocketAddr, tls: bool, insecure_http: bool) -> Result<()> {
    if !listen.ip().is_loopback() && !tls && !insecure_http {
        bail!(
            "{listen} is not loopback: serve TLS with --tls-cert/--tls-key, \
             or pass --insecure-http to accept plain HTTP"
        );
    }
    Ok(())
}

/// The database URL without credentials, for messages.
pub(crate) fn redact(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
            format!("{scheme}://{}", host.split('?').next().unwrap_or(host))
        }
        None => "the database".into(),
    }
}

#[cfg(feature = "native")]
/// Why a connection failed, without the URL's credentials.
pub(crate) fn db_error(db: &str, e: &sqlx::Error) -> anyhow::Error {
    let why = e
        .as_database_error()
        .map_or_else(|| "unreachable".to_string(), |d| d.message().to_string());
    anyhow::anyhow!("connecting to {}: {why}", redact(db))
}

/// The built web app to serve at `/`: `$H2_LIVE_WEB`, else this checkout's
/// `web/dist`, if it has an `index.html`.
pub fn web_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("H2_LIVE_WEB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist"));
    dir.join("index.html").is_file().then_some(dir)
}

/// The line `serve` prints about game art.
pub fn art_notice(art: &art::GameArt) -> String {
    if art.share {
        format!(
            "serving game art from {} to everyone: this redistributes your copy of the game's files",
            art.dir.display()
        )
    } else {
        format!("game art from {} (to this machine only)", art.dir.display())
    }
}

#[cfg(feature = "native")]
/// Serves until the process is stopped.
pub async fn serve(config: Config) -> Result<()> {
    check_bind(config.listen, config.tls.is_some(), config.insecure_http)?;
    let identity = ServerIdentity::open(&config.data_dir, config.id.as_deref())?;
    let (db, notice) = resolve_db(config.db.as_deref(), &config.data_dir).await;
    if let Some(notice) = notice {
        println!("h2 live: {notice}");
    }
    let store = copy::open_db(&db).await?;
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let blob_flags = config.blobs.clone().or_env(&env);
    let blobs = blobs::Blobs::configure(
        &config.data_dir,
        blob_flags.private.as_deref(),
        blob_flags.public.as_deref(),
        blob_flags.public_url.as_deref(),
        &env,
    )
    .context("configuring blob storage")?;
    let id = identity.id.clone();
    let private = config.policy.private;
    let live = Live::start(store.clone(), identity, config.policy).await?;
    live.set_blobs(blobs.clone());
    live.set_data_dir(&config.data_dir);
    let publish = match (&config.site.game_art, &blobs.public) {
        (Some(art), Some(public)) if art.share && !private => {
            let published = art::PublishedArt::new(public.clone());
            live.set_published_art(published.clone());
            Some((art.clone(), published))
        }
        _ => None,
    };
    for name in title_headers::reseal_legacy(&live).await? {
        println!("h2 live: republished {name} with the game's header");
    }
    let app = http::router_with_site(live.clone(), &config.site);
    let reads = if private { "private" } else { "public" };
    let scheme = if config.tls.is_some() {
        "https"
    } else {
        "http"
    };
    println!(
        "h2 live: {id} on {scheme}://{}, log at position {} ({} {}, {reads} reads)",
        config.listen,
        live.position(),
        store.kind(),
        redact(&db)
    );
    println!("h2 live: blobs in {}", blobs.private.describe());
    if let Some(public) = &blobs.public {
        println!("h2 live: public blobs in {}", public.describe());
    }
    if let Some((art, published)) = publish {
        tokio::spawn(async move {
            match published.publish(&art).await {
                Ok(r) => println!(
                    "h2 live: game art {} published ({} of {} files uploaded now); /art/game/ redirects to it",
                    r.stamp, r.uploaded, r.files
                ),
                Err(e) => {
                    eprintln!("h2 live: publishing game art failed ({e}); serving it from here")
                }
            }
        });
    }
    let admins = live.admins.configured();
    if !admins.is_empty() {
        println!("h2 live: admins configured: {}", admins.join(", "));
    }
    if let Some(url) = live.policy().public.get() {
        println!("h2 live: public base URL {url}");
        if let Some(warn) = public::unclaimed_admins_notice(&live.unclaimed_admins()) {
            println!("h2 live: warning: {warn}");
        }
    }
    match &config.site.web {
        Some(dir) => println!("h2 live: web app from {}", dir.display()),
        None => println!("h2 live: no web app (build it: cd web && bun run build)"),
    }
    if let Some(art) = &config.site.game_art {
        println!("h2 live: {}", art_notice(art));
    }
    match config.tls {
        Some((cert, key)) => {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .with_context(|| format!("loading {} and {}", cert.display(), key.display()))?;
            axum_server::bind_rustls(config.listen, tls)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await?;
        }
        None => {
            let listener = tokio::net::TcpListener::bind(config.listen)
                .await
                .with_context(|| format!("binding {}", config.listen))?;
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;
        }
    }
    Ok(())
}

#[cfg(feature = "native")]
/// A server running inside another program (`h2 play --live-local`) on its own
/// tokio runtime. Dropping it stops it.
pub struct Embedded {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// `sqlite`, `postgres`, `file` or `memory`.
    pub store: &'static str,
    pub live: Arc<Live>,
    runtime: Option<tokio::runtime::Runtime>,
}

#[cfg(feature = "native")]
impl Drop for Embedded {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// The `db` of [`start_embedded`] that skips Postgres: a bare in-memory
/// store, nothing kept and no default MOTD (for tests).
pub const MEMORY_DB: &str = "memory";
/// The `db` of [`start_embedded`] that skips Postgres for the file store
/// in the data directory ([`file_store_dir`]).
pub const FILE_DB: &str = "file";

/// Where [`start_embedded`] keeps its log without Postgres.
pub fn file_store_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("log")
}

#[cfg(feature = "native")]
/// Starts a server on `listen` (port 0: a free one). Postgres at `db` if it
/// is reachable (the database is created if missing), else the file store
/// in `data_dir` ([`filestore`]), so accounts, friends, title files and
/// games survive a restart; the in-memory store only if that cannot open
/// (another server holds it), with a warning. [`MEMORY_DB`] and
/// [`FILE_DB`] pick a store without trying Postgres. Unless the store is
/// the in-memory one, a default message of the day ([`default_motd`]) is
/// published if none ever was.
pub fn start_embedded(
    db: &str,
    listen: SocketAddr,
    data_dir: &std::path::Path,
    site: http::Site,
) -> Result<Embedded> {
    let policy = Policy {
        admins: crate::admin::Admins::from_flags_and_env(&[]).map_err(anyhow::Error::msg)?,
        ..Policy::default()
    };
    start_embedded_with(db, listen, data_dir, site, policy)
}

#[cfg(feature = "native")]
/// [`start_embedded`] with a policy of the caller's (tests: an
/// invite-only server).
pub fn start_embedded_with(
    db: &str,
    listen: SocketAddr,
    data_dir: &std::path::Path,
    site: http::Site,
    policy: Policy,
) -> Result<Embedded> {
    check_bind(listen, false, false)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("live-server")
        .enable_all()
        .build()
        .context("starting the live server's runtime")?;
    let identity = ServerIdentity::open(data_dir, None)?;
    let (live, store, listener) = runtime.block_on(async {
        let file_dir = file_store_dir(data_dir);
        let (store, kind): (Arc<dyn store::EventStore>, _) = match db {
            MEMORY_DB => (Arc::new(store::MemoryStore::new()), "memory"),
            _ if sqlite::path_of(db).is_some() => (
                Arc::new(
                    sqlite::SqliteStore::connect(db)
                        .await
                        .with_context(|| format!("opening {db}"))?,
                ),
                "sqlite",
            ),
            FILE_DB => (
                Arc::new(
                    filestore::FileStore::open(&file_dir)
                        .await
                        .with_context(|| format!("opening {}", file_dir.display()))?,
                ),
                "file",
            ),
            _ => match pg::PostgresStore::connect_creating(db).await {
                Ok(pg) => (Arc::new(pg), "postgres"),
                Err(e) => match filestore::FileStore::open(&file_dir).await {
                    Ok(file) => {
                        eprintln!(
                            "live: {}; keeping the log in {}",
                            db_error(db, &e),
                            file_dir.display()
                        );
                        (Arc::new(file), "file")
                    }
                    Err(fe) => {
                        eprintln!(
                            "live: {}; {} did not open ({fe}); using an in-memory store \
                             (nothing is kept after exit)",
                            db_error(db, &e),
                            file_dir.display()
                        );
                        (Arc::new(store::MemoryStore::new()), "memory")
                    }
                },
            },
        };
        let live = Live::start(store, identity, policy).await?;
        live.set_data_dir(data_dir);
        if kind != "memory" {
            match title_headers::reseal_legacy(&live).await {
                Ok(names) => {
                    for name in names {
                        eprintln!("live: republished {name} with the game's header");
                    }
                }
                Err(e) => eprintln!("live: {e:#}"),
            }
            match default_motd::ensure(&live).await {
                Ok(true) => eprintln!("live: published the default message of the day"),
                Ok(false) => {}
                Err(e) => eprintln!("live: {e:#}"),
            }
        }
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .with_context(|| format!("binding {listen}"))?;
        anyhow::Ok((live, kind, listener))
    })?;
    let url = format!("http://{}", listener.local_addr()?);
    let app = http::router_with_site(live.clone(), &site);
    runtime.spawn(async move {
        let app = app.into_make_service_with_connect_info::<SocketAddr>();
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("live: server stopped: {e}");
        }
    });
    Ok(Embedded {
        url,
        store,
        live,
        runtime: Some(runtime),
    })
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_needs_tls_or_consent() {
        let lo: SocketAddr = "127.0.0.1:8074".parse().unwrap();
        let lo6: SocketAddr = "[::1]:8074".parse().unwrap();
        let any: SocketAddr = "0.0.0.0:8074".parse().unwrap();
        assert!(check_bind(lo, false, false).is_ok());
        assert!(check_bind(lo6, false, false).is_ok());
        assert!(check_bind(any, false, false).is_err());
        assert!(check_bind(any, true, false).is_ok());
        assert!(check_bind(any, false, true).is_ok());
    }

    #[tokio::test]
    async fn sqlite_in_the_data_directory_is_the_default() {
        let dir = std::env::temp_dir().join(format!("h2live-resolve-{}", std::process::id()));
        let given = resolve_db(Some("postgres://db/x"), &dir).await;
        assert_eq!(given, ("postgres://db/x".to_string(), None));
        // A server that already has live.db keeps it, whatever else exists.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(DB_FILE), b"").unwrap();
        let (db, notice) = resolve_db(None, &dir).await;
        assert_eq!(db, format!("sqlite://{}/live.db", dir.display()));
        assert_eq!(notice, None);
        assert_eq!(sqlite::path_of(&db), Some(dir.join(DB_FILE)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `h2 live serve` and `h2-live-server` with no `--db` on a new data
    /// directory: the log is SQLite there, blobs beside it.
    #[tokio::test]
    async fn a_new_server_keeps_everything_in_its_data_directory() {
        let dir = std::env::temp_dir().join(format!(
            "h2live-newserver-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let store = copy::open_db(&default_db(&dir)).await.unwrap();
        assert_eq!(store.kind(), "sqlite");
        assert!(dir.join(DB_FILE).is_file());
        let none = |_: &str| None;
        let b = blobs::Blobs::configure(&dir, None, None, None, &none).unwrap();
        assert!(b.private.is_dir());
        assert_eq!(b.private.describe(), dir.display().to_string());
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn database_urls_are_redacted() {
        assert_eq!(
            redact("postgres://alice:hunter2@db.example/live?sslmode=require"),
            "postgres://db.example/live"
        );
        assert_eq!(
            redact("postgres://localhost/h2live"),
            "postgres://localhost/h2live"
        );
        assert_eq!(redact("garbage"), "the database");
    }
}
