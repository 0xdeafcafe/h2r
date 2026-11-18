//! An event store whose content-addressed blobs (title files) live in a
//! [`BlobStore`], not beside the log: on the Worker, R2 rather than the
//! hub's SQLite, where a row holds at most 2 MB and a title file may be up
//! to 2 MiB (docs/live/live-worker.md). Everything else is the inner
//! store's. Blobs the inner store already had are still read from it.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;

use crate::blobs::{BlobStore, Meta};
use crate::store::{EventStore, NewEvent, Query, RecordedEvent, StoreError, UsageRow, Wake};

/// Where the blobs go in the blob store: `title/<sha256>`.
pub const PREFIX: &str = "title/";

pub struct BlobsElsewhere {
    inner: Arc<dyn EventStore>,
    blobs: BlobStore,
}

impl BlobsElsewhere {
    pub fn new(inner: Arc<dyn EventStore>, blobs: BlobStore) -> Self {
        BlobsElsewhere { inner, blobs }
    }
}

fn failed(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("title file blobs: {e}"))
}

fn key(sha256: &str) -> Result<String, StoreError> {
    // A hex SHA-256 only: nothing that walks out of the prefix.
    if sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(format!("{PREFIX}{}", sha256.to_ascii_lowercase()))
    } else {
        Err(failed(format!("{sha256:?} is not a SHA-256")))
    }
}

#[async_trait]
impl EventStore for BlobsElsewhere {
    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        self.inner.append(events, expect).await
    }

    async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        self.inner.read(query).await
    }

    async fn head(&self) -> Result<u64, StoreError> {
        self.inner.head().await
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let meta = Meta {
            content_type: Some("application/octet-stream".into()),
            ..Meta::default()
        };
        self.blobs
            .put(&key(sha256)?, Bytes::copy_from_slice(bytes), &meta)
            .await
            .map_err(failed)
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self.blobs.get(&key(sha256)?).await.map_err(failed)? {
            Some(b) => Ok(Some(b.to_vec())),
            None => self.inner.blob(sha256).await,
        }
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        self.inner.load_checkpoint(name).await
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        self.inner.save_checkpoint(name, position).await
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        self.inner.wake().await
    }

    fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        self.inner.import(events).await
    }

    async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
        let mut names = self.inner.blob_names().await?;
        for info in self.blobs.list(PREFIX).await.map_err(failed)? {
            if let Some(name) = info.key.strip_prefix(PREFIX) {
                names.push(name.to_string());
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        self.inner.checkpoints().await
    }

    async fn add_usage(&self, rows: &[UsageRow], keep_from: &str) -> Result<(), StoreError> {
        self.inner.add_usage(rows, keep_from).await
    }

    async fn usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        self.inner.usage().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    #[tokio::test]
    async fn title_files_go_to_the_blob_store_and_old_ones_still_read() {
        let inner = Arc::new(MemoryStore::new());
        let old = "a".repeat(64);
        inner.put_blob(&old, b"from before").await.unwrap();
        let blobs = BlobStore::memory();
        let store = BlobsElsewhere::new(inner.clone(), blobs.clone());

        let new = "B".repeat(64);
        // Bigger than a Durable Object's 2 MB row.
        let big = vec![7u8; 2 * 1024 * 1024 + 1];
        store.put_blob(&new, &big).await.unwrap();
        assert_eq!(store.blob(&new).await.unwrap().unwrap(), big);
        assert_eq!(
            inner.blob(&new).await.unwrap(),
            None,
            "not in the log's store"
        );
        let key = format!("title/{}", "b".repeat(64));
        assert_eq!(blobs.get(&key).await.unwrap().unwrap().len(), big.len());

        assert_eq!(store.blob(&old).await.unwrap().unwrap(), b"from before");
        assert_eq!(store.blob(&"c".repeat(64)).await.unwrap(), None);
        assert_eq!(
            store.blob_names().await.unwrap(),
            vec!["a".repeat(64), "b".repeat(64)]
        );
        assert!(store.put_blob("../x", b"no").await.is_err());
    }
}
