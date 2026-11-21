#![cfg(feature = "conformance")]
mod common;
#[path = "support/film_library.rs"]
mod library;

#[tokio::test]
async fn native_film_library() {
    use live_server::{Policy, blobs::Blobs, sqlite::SqliteStore};
    let root = common::temp_dir();
    let store = SqliteStore::open(&root.join("events.db")).await.unwrap();
    let h = common::start_driven(
        std::sync::Arc::new(store),
        Policy {
            write_rate: (1000., 1000),
            ..Policy::default()
        },
    )
    .await;
    h.live.set_blobs(Blobs::dir(&root).unwrap());
    library::exercise(h.live.clone()).await;
    drop(h);
    std::fs::remove_dir_all(root).unwrap();
}
