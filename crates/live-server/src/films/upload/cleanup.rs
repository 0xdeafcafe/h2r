//! Published film bytes are immutable. Only remove temporary objects once the
//! signed receipt has chosen a winner and its stats import has finished.
use std::collections::BTreeSet;

use serde_json::json;

use crate::{
    app::{ApiError, Live},
    blobs::{BlobStore, Result as BlobResult},
    films::{
        HOST_FILM_CLEANED, HOST_FILM_CLEANUP_REQUESTED, HOST_FILM_PREFIX, host_film_key,
        import_stream, parts,
    },
    store::StoreError,
};

// Both storage homes use the same limit per invocation. Unsorted native
// directory listings need no continuation token: deleted keys are themselves
// durable progress, and the set of retained keys is bounded by the maximum
// number of parts (128) plus the selected receipt's single physical object.
const DELETE_BUDGET: usize = 8;

async fn collect(store: &BlobStore, id: &str, chosen: &str) -> BlobResult<bool> {
    let retained = parts::retained_keys(store, chosen).await?;
    let mut budget = DELETE_BUDGET;
    for (dir, keep) in [
        (format!("film-staging/{id}"), BTreeSet::new()),
        (format!("{HOST_FILM_PREFIX}{id}"), retained),
    ] {
        // Taking one extra key distinguishes exhaustion from a full batch.
        // Include space for every retained key even if the backend lists
        // those before the first removable object.
        let max_read = keep.len() + budget + 1;
        let keys = store.first_keys(&dir, max_read).await?;
        for key in keys {
            if keep.contains(&key) {
                continue;
            }
            if budget == 0 {
                return Ok(false);
            }
            store.delete(&key).await?;
            budget -= 1;
        }
    }
    Ok(true)
}

fn chosen_blob(state: &crate::model::State, id: &str) -> Option<String> {
    state
        .games
        .get(id)
        .filter(|g| g.has_host_film)
        .map(|g| g.film_blob.clone().unwrap_or_else(|| host_film_key(id)))
        .or_else(|| {
            state
                .films
                .standalone_host_blob(id)
                .map(|(_, key)| key.to_owned())
        })
}

/// One replica may finish writing after another collected this game. Its
/// final reconciliation reopens cleanup. A scanner already in flight has an
/// older journal generation and cannot mark this later request complete.
pub(super) async fn reconcile(
    live: &Live,
    id: &str,
    candidate: &str,
    cursor_race: bool,
    wrote_cursor: bool,
) -> Result<(), ApiError> {
    let mut chosen = chosen_blob(&live.model.read(), id);
    if chosen.is_none() && (cursor_race || wrote_cursor) {
        // A create-if-absent cursor write can succeed *after* the winner
        // removed its staging key. Observe the log head after the write;
        // a subsequent publication is responsible for its own first pass.
        live.wait_applied(live.store.head().await?).await?;
        chosen = chosen_blob(&live.model.read(), id);
    }
    if chosen.is_none_or(|key| key == candidate && !cursor_race && !wrote_cursor) {
        return Ok(());
    }
    let stream = format!("{}:cleanup", import_stream(id));
    let dirty = live.server_event(stream, HOST_FILM_CLEANUP_REQUESTED, json!({"game": id}));
    let rows = live.store.append(vec![dirty], &[]).await?;
    live.wait_applied(rows[0].position).await
}

pub(crate) async fn cleanup_published(live: &Live, id: &str) {
    let (chosen, generation) = {
        let state = live.model.read();
        if !state.films.cleanup.contains(id) || state.films.imports.contains_key(id) {
            return;
        }
        (
            chosen_blob(&state, id),
            state.films.cleanup_versions.get(id).copied().unwrap_or(0),
        )
    };
    let (Some(chosen), Some(blobs)) = (chosen, live.blobs()) else {
        return;
    };
    match collect(&blobs.private, id, &chosen).await {
        Ok(false) => return,
        Err(e) => {
            eprintln!("live: collecting film objects for {id}: {e}");
            return;
        }
        Ok(true) => {}
    }

    // A restart rebuilds the unfinished queue by replaying the signed log.
    // The cleanup marker is separate from the journal used to apply stats,
    // so it cannot advance another import's cursor.
    let stream = format!("{}:cleanup", import_stream(id));
    let done = live.server_event(stream.clone(), HOST_FILM_CLEANED, json!({"game": id}));
    match live.store.append(vec![done], &[(stream, generation)]).await {
        Ok(rows) => {
            if let Err(e) = live.wait_applied(rows[0].position).await {
                eprintln!("live: applying film cleanup marker: {e}");
            }
        }
        Err(StoreError::Conflict { .. }) => {} // Another worker finished.
        Err(e) => eprintln!("live: recording film cleanup for {id}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::Meta;
    use bytes::Bytes;

    #[tokio::test]
    async fn a_late_candidate_reopens_completed_cleanup_and_fences_a_stale_scanner() {
        use crate::{Policy, ServerIdentity, app::Timers, blobs::Blobs, store::MemoryStore};
        use live_proto::sign::KeyPair;
        use std::sync::Arc;

        let objects = BlobStore::memory();
        let live = Live::start_with(
            Arc::new(MemoryStore::new()),
            ServerIdentity {
                id: "live.test".into(),
                key: KeyPair::generate(),
            },
            Policy::default(),
            Timers::Driven,
        )
        .await
        .unwrap();
        live.set_blobs(Blobs {
            private: objects.clone(),
            public: None,
        });
        let id = "game";
        let chosen = "films/game/chosen.h2film";
        let late = "films/game/late.h2film";
        let receipt = live.server_event(import_stream(id), crate::films::HOST_FILM_STORED,
            json!({"game":id, "blob":chosen, "catalogue_host":"host:lease", "catalogue_players":[],
                "summary":live_proto::film::Summary::default(),
                "stats_discrepancy":{"cause":"missing_start","event_index":0},
                "web_ranges":{"size":10,"summary_bytes":1,"index_offset":4,"index_size":1,"index_sha256":"abc"}}));
        let rows = live.store.append(vec![receipt], &[]).await.unwrap();
        live.wait_applied(rows[0].position).await.unwrap();
        objects
            .put(chosen, Bytes::from_static(b"kept"), &Meta::default())
            .await
            .unwrap();
        cleanup_published(&live, id).await;
        assert!(!live.model.read().films.cleanup_due());

        objects
            .put(late, Bytes::from_static(b"late"), &Meta::default())
            .await
            .unwrap();
        reconcile(&live, id, late, false, true).await.unwrap();
        assert!(live.model.read().films.cleanup_due());
        let stream = format!("{}:cleanup", import_stream(id));
        let stale = live.server_event(stream.clone(), HOST_FILM_CLEANED, json!({"game":id}));
        assert!(matches!(
            live.store.append(vec![stale], &[(stream, 1)]).await,
            Err(StoreError::Conflict { .. })
        ));
        cleanup_published(&live, id).await;
        assert!(!live.model.read().films.cleanup_due());
        assert!(objects.head(late).await.unwrap().is_none());
        assert!(objects.head(chosen).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn repeated_bounded_batches_keep_only_the_manifest_and_its_exact_parts() {
        let store = BlobStore::memory();
        let chosen = "films/game-id/chosen.parts";
        let sha = "a".repeat(64);
        let part = parts::part_key(chosen, 0, &sha);
        let spare = parts::part_key(chosen, 0, &"b".repeat(64));
        let put = |k: String| {
            let store = store.clone();
            async move {
                store
                    .put(&k, Bytes::from_static(b"x"), &Meta::default())
                    .await
                    .unwrap();
            }
        };
        put(part.clone()).await;
        parts::publish(&store, chosen, 1, &[sha]).await.unwrap();
        put(spare).await;
        for n in 0..DELETE_BUDGET + 1 {
            put(format!("film-staging/game-id/{n}")).await;
            put(format!("films/game-id/old.parts.data/{n}")).await;
        }
        put("films/game-id-extra/keep".into()).await;

        let before = store.list("").await.unwrap().len();
        assert!(!collect(&store, "game-id", chosen).await.unwrap());
        assert_eq!(before - store.list("").await.unwrap().len(), DELETE_BUDGET);
        assert!(!collect(&store, "game-id", chosen).await.unwrap());
        assert!(collect(&store, "game-id", chosen).await.unwrap());
        assert_eq!(store.list("").await.unwrap().len(), 3);
        assert!(store.head(chosen).await.unwrap().is_some());
        assert!(store.head(&part).await.unwrap().is_some());
        assert!(
            store
                .head("films/game-id-extra/keep")
                .await
                .unwrap()
                .is_some()
        );
    }
}
