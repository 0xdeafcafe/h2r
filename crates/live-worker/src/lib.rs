//! The Live server as a Cloudflare Worker (docs/live/live-worker.md).
//!
//! The routes are live-server's own axum router, built without its
//! `native` feature. Everything with one owner runs in one Durable Object,
//! `LiveHub`: the read model, the dispatcher, and the writer of the event
//! log, which is kept in the object's own SQLite storage ([`sqlstore`]
//! over `dosql`). Blobs go to R2. The Worker's fetch handler sends every
//! API request to that object.
//!
//! [`sqlstore`] and [`settings`] build and are tested on this machine as
//! well. The Worker itself (`entry`, `dosql`, `r2`) builds only for wasm32.

pub mod settings;
pub mod site;
pub mod sqlstore;

#[cfg(target_arch = "wasm32")]
mod dosql;
#[cfg(target_arch = "wasm32")]
mod entry;
#[cfg(target_arch = "wasm32")]
mod ingest;
#[cfg(target_arch = "wasm32")]
mod r2;
#[cfg(target_arch = "wasm32")]
mod relay;
pub mod relayroom;
pub mod sockets;

/// The Worker's bindings (wrangler.toml). Its variables and secrets are in
/// [`settings`].
pub mod bindings {
    /// R2: content, cloud saves (private).
    pub const BLOBS: &str = "BLOBS";
    /// The Durable Object namespace of [`HUB_CLASS`].
    pub const HUB: &str = "LIVE_HUB";
    pub const HUB_CLASS: &str = "LiveHub";
    /// The one hub's name: one log, one owner.
    pub const HUB_NAME: &str = "live";
    /// The Durable Object namespace of [`RELAY_CLASS`]: one object per
    /// co-op session, named by the session id.
    pub const RELAY: &str = "COOP_RELAY";
    pub const RELAY_CLASS: &str = "CoopRelay";
    /// The web app's static assets (`[assets]` in wrangler.toml): the
    /// Worker reads `index.html` from it for the app's pages.
    pub const ASSETS: &str = "ASSETS";
}

/// The response header that reports a hub's cold start on the request
/// that caused it: `start;dur=<ms>` (the log replayed into the read model)
/// and `events;desc=<head>`.
pub const COLD_START_HEADER: &str = "server-timing";
