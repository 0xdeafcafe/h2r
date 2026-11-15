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
pub mod guard;
pub mod http;
pub mod identity;
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

/// Serves until the process is stopped.
pub async fn serve(config: Config) -> Result<()> {
    check_bind(config.listen, config.tls.is_some(), config.insecure_http)?;
    let identity = ServerIdentity::open(&config.data_dir, config.id.as_deref())?;
    let db = redact(&config.db);
    let store = pg::PostgresStore::connect(&config.db).await.map_err(|e| {
        anyhow::anyhow!(
            "connecting to {db}: {}",
            e.as_database_error()
                .map_or("unreachable".into(), |d| d.message().to_string())
        )
    })?;
    let id = identity.id.clone();
    let private = config.policy.private;
    let live = Arc::new(Live::start(Arc::new(store), identity, config.policy).await?);
    let app = http::router(live.clone());
    let reads = if private { "private" } else { "public" };
    let scheme = if config.tls.is_some() {
        "https"
    } else {
        "http"
    };
    println!(
        "h2 live: {id} on {scheme}://{}, log at position {} ({db}, {reads} reads)",
        config.listen,
        live.position()
    );
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
