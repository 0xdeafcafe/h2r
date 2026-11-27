//! Packages on Live (docs/live/live-api.md, "Packages"): the parity
//! suite's package cases on the native server. The Worker runs the same
//! cases on its SQL store (`live-worker`, `worker_packages`).
mod common;
#[path = "support/packages.rs"]
mod packages;
use common::*;

#[tokio::test]
async fn native_packages() {
    let h = harness().await;
    packages::exercise(h.live.clone()).await;
}

/// A second server on the same log reads the same packages, credit and
/// model flags.
#[tokio::test]
async fn packages_replay_from_the_log() {
    use live_server::store::{MemoryStore, Store};
    use std::sync::Arc;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let h = start(store.clone(), live_server::Policy::default()).await;
    packages::exercise(h.live.clone()).await;
    let first = h.live.package_list();
    let again = start(store, live_server::Policy::default()).await;
    let second = again.live.package_list();
    assert_eq!(first.items.len(), 3);
    assert_eq!(
        serde_json::to_value(&first.items).unwrap(),
        serde_json::to_value(&second.items).unwrap()
    );
}
