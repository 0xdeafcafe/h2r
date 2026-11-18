//! Writes held while a log is copied in ([`crate::logio`]).
//!
//! A server that is being copied into must write nothing of its own: its
//! sweeps, seeing a copied presence or game gone quiet, would otherwise
//! append `PresenceExpired` or `GameEnded` at its head, and the next page
//! (the source's own events at those positions) would no longer follow.
//! The first import holds writes, and they stay held, across restarts too
//! (the hold is a checkpoint row, [`HOLD`]), until the operator releases
//! them (`POST /v1/admin/log/release`, `h2 live store copy --release`).
//! While held, every append fails with [`StoreError::Held`]; imports go on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::store::{EventStore, NewEvent, Query, RecordedEvent, StoreError, Wake};

/// The checkpoint that keeps the hold: 1 held, 0 (or none) not.
pub const HOLD: &str = "live.import-hold";

/// `inner`, with appends refused while `held`.
pub struct HeldStore {
    inner: Arc<dyn EventStore>,
    held: Arc<AtomicBool>,
}

impl HeldStore {
    /// Wraps `inner`, held if its [`HOLD`] checkpoint says so.
    pub async fn open(inner: Arc<dyn EventStore>) -> Result<Self, StoreError> {
        let held = inner.load_checkpoint(HOLD).await?.is_some_and(|p| p > 0);
        Ok(HeldStore {
            inner,
            held: Arc::new(AtomicBool::new(held)),
        })
    }

    pub fn held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }

    /// Holds or releases writes, durably first.
    pub async fn set_held(&self, held: bool) -> Result<(), StoreError> {
        self.inner.save_checkpoint(HOLD, u64::from(held)).await?;
        self.held.store(held, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl EventStore for HeldStore {
    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        if self.held() {
            return Err(StoreError::Held);
        }
        self.inner.append(events, expect).await
    }

    async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        self.inner.read(query).await
    }

    async fn head(&self) -> Result<u64, StoreError> {
        self.inner.head().await
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.inner.put_blob(sha256, bytes).await
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.blob(sha256).await
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
        self.inner.blob_names().await
    }

    /// Not the hold: it is this server's, not part of the log a copy takes.
    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let mut all = self.inner.checkpoints().await?;
        all.retain(|(name, _)| name != HOLD);
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;
    use crate::store::conformance::event;
    use live_proto::sign::KeyPair;

    #[tokio::test]
    async fn a_hold_refuses_appends_survives_a_restart_and_is_not_copied() {
        let key = KeyPair::generate();
        let inner: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
        let store = HeldStore::open(inner.clone()).await.unwrap();
        store.append(vec![event(&key, "a", 0)], &[]).await.unwrap();
        store.set_held(true).await.unwrap();
        assert!(matches!(
            store.append(vec![event(&key, "a", 1)], &[]).await,
            Err(StoreError::Held)
        ));
        // A restart: still held.
        let again = HeldStore::open(inner.clone()).await.unwrap();
        assert!(again.held());
        assert!(again.checkpoints().await.unwrap().is_empty());
        again.set_held(false).await.unwrap();
        again.append(vec![event(&key, "a", 1)], &[]).await.unwrap();
        assert!(!HeldStore::open(inner).await.unwrap().held());
    }
}
