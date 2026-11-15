//! The Xbox Live replacement service (docs/live.md §4.3, docs/live-api.md,
//! docs/live-security.md): signed events in an append-only Postgres log,
//! projected into memory, served over HTTP and WebSocket.
//!
//! - [`store`]: the log ([`pg::PostgresStore`], [`store::MemoryStore`]).
//! - [`dispatch`]: the one tailer, feeding [`dispatch::Subscriber`]s.
//! - [`model`]: the read model, a subscriber replayed from 0 at startup.
//! - [`app`]: authentication, write rules, operations.
//! - [`http`]: the routes.
//! - [`identity`]: the server key and id in the data directory.
//!
//! Signing and verification live in `live_proto::sign`, shared with the
//! clients. Run it with `h2 live serve`.

pub mod app;
pub mod dispatch;
pub mod games;
pub mod guard;
pub mod http;
pub mod identity;
pub mod ingest;
pub mod model;
pub mod pg;
pub mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

pub use app::{ApiError, Live, MAX_TITLE_FILE, Policy};
pub use identity::ServerIdentity;

/// Used when neither `--db` nor `H2_LIVE_DB` is set.
pub const DEFAULT_DB: &str = "postgres://localhost/h2live";
/// Used when `--listen` is not set.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8074";

pub struct Config {
    pub db: String,
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    /// The server id; fixed on first start.
    pub id: Option<String>,
    pub policy: Policy,
    /// PEM certificate chain and key: serve HTTPS.
    pub tls: Option<(PathBuf, PathBuf)>,
    /// Allow plain HTTP on a non-loopback address.
    pub insecure_http: bool,
    /// The built web app, served at `/` ([`web_dir`]).
    pub web: Option<PathBuf>,
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
fn redact(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
            format!("{scheme}://{}", host.split('?').next().unwrap_or(host))
        }
        None => "the database".into(),
    }
}

/// Why a connection failed, without the URL's credentials.
fn db_error(db: &str, e: &sqlx::Error) -> anyhow::Error {
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

/// Serves until the process is stopped.
pub async fn serve(config: Config) -> Result<()> {
    check_bind(config.listen, config.tls.is_some(), config.insecure_http)?;
    let identity = ServerIdentity::open(&config.data_dir, config.id.as_deref())?;
    let store = pg::PostgresStore::connect_creating(&config.db)
        .await
        .map_err(|e| db_error(&config.db, &e))?;
    let id = identity.id.clone();
    let private = config.policy.private;
    let live = Live::start(Arc::new(store), identity, config.policy).await?;
    let app = http::router_with_web(live.clone(), config.web.as_deref());
    let reads = if private { "private" } else { "public" };
    let scheme = if config.tls.is_some() {
        "https"
    } else {
        "http"
    };
    println!(
        "h2 live: {id} on {scheme}://{}, log at position {} ({}, {reads} reads)",
        config.listen,
        live.position(),
        redact(&config.db)
    );
    match &config.web {
        Some(dir) => println!("h2 live: web app from {}", dir.display()),
        None => println!("h2 live: no web app (build it: cd web && bun run build)"),
    }
    match config.tls {
        Some((cert, key)) => {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .with_context(|| format!("loading {} and {}", cert.display(), key.display()))?;
            axum_server::bind_rustls(config.listen, tls)
                .serve(app.into_make_service())
                .await?;
        }
        None => {
            let listener = tokio::net::TcpListener::bind(config.listen)
                .await
                .with_context(|| format!("binding {}", config.listen))?;
            axum::serve(listener, app).await?;
        }
    }
    Ok(())
}

/// A server running inside another program (`h2 play --live`) on its own
/// tokio runtime. Dropping it stops it.
pub struct Embedded {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// `postgres` or `memory`.
    pub store: &'static str,
    pub live: Arc<Live>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for Embedded {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// The `db` of [`start_embedded`] that skips Postgres.
pub const MEMORY_DB: &str = "memory";

/// Starts a server on `listen` (port 0: a free one). Postgres at `db` if it
/// is reachable (the database is created if missing), else the in-memory
/// store, with a one-line warning: nothing then survives the process.
/// [`MEMORY_DB`] asks for the in-memory store without trying Postgres.
pub fn start_embedded(
    db: &str,
    listen: SocketAddr,
    data_dir: &std::path::Path,
    web: Option<PathBuf>,
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
        let (store, kind): (Arc<dyn store::EventStore>, _) = match db {
            MEMORY_DB => (Arc::new(store::MemoryStore::new()), "memory"),
            _ => match pg::PostgresStore::connect_creating(db).await {
                Ok(pg) => (Arc::new(pg), "postgres"),
                Err(e) => {
                    eprintln!(
                        "live: {}; using an in-memory store (nothing is kept after exit)",
                        db_error(db, &e)
                    );
                    (Arc::new(store::MemoryStore::new()), "memory")
                }
            },
        };
        let live = Live::start(store, identity, Policy::default()).await?;
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .with_context(|| format!("binding {listen}"))?;
        anyhow::Ok((live, kind, listener))
    })?;
    let url = format!("http://{}", listener.local_addr()?);
    let app = http::router_with_web(live.clone(), web.as_deref());
    runtime.spawn(async move {
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

#[cfg(test)]
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
