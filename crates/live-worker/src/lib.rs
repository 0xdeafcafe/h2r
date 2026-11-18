//! The Live server as a Cloudflare Worker (docs/live/live-worker.md).
//!
//! The routes are live-server's own axum router, built without its
//! `native` feature. Everything with one owner runs in one Durable Object,
//! `LiveHub`: the read model, the dispatcher, and the writer of the event
//! log, which is kept in D1 ([`d1store`]). Blobs go to R2. The Worker's
//! fetch handler sends every API request to that object.
//!
//! [`d1store`] builds and is tested on this machine as well. The Worker
//! itself (`entry`, `r2`) builds only for wasm32.

pub mod d1store;

#[cfg(target_arch = "wasm32")]
mod d1;
#[cfg(target_arch = "wasm32")]
mod entry;
#[cfg(target_arch = "wasm32")]
mod r2;

/// The Worker's bindings and variables (wrangler.toml).
pub mod bindings {
    /// The D1 database holding the event log.
    pub const DB: &str = "DB";
    /// R2: content, cloud saves (private).
    pub const BLOBS: &str = "BLOBS";
    /// The Durable Object namespace of [`HUB_CLASS`].
    pub const HUB: &str = "LIVE_HUB";
    pub const HUB_CLASS: &str = "LiveHub";
    /// The one hub's name: one log, one owner.
    pub const HUB_NAME: &str = "live";
    /// The server id (the host in every author id): a variable.
    pub const SERVER_ID: &str = "LIVE_SERVER_ID";
    /// The server's Ed25519 key, base64 as in `server.key`: a secret.
    pub const SERVER_KEY: &str = "LIVE_SERVER_KEY";
}
