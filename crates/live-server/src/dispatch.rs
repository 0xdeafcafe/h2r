//! The one tailer of the log. The [`Dispatcher`] reads events by position
//! and hands each to every [`Subscriber`] in order; the in-memory
//! projections are a subscriber, replayed from position 0 at startup. After
//! each event it publishes the applied position (write handlers wait on it
//! for read-your-writes) and broadcasts the event (stream readers follow
//! it).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, watch};

use crate::store::{EventStore, Query, RecordedEvent, Wake};

/// The longest the dispatcher sleeps without a wake.
pub const POLL: Duration = Duration::from_secs(1);
const BATCH: usize = 1000;
/// Stream readers further behind than this re-read from the store.
const FEED_CAPACITY: usize = 4096;

/// Turns events into state. `apply` runs on the dispatcher task, in log
/// order, so it must not block.
pub trait Subscriber: Send + Sync {
    fn name(&self) -> &str;
    fn apply(&self, event: &RecordedEvent);
}

pub struct Dispatcher {
    store: Arc<dyn EventStore>,
    subscribers: Vec<Arc<dyn Subscriber>>,
    applied: watch::Sender<u64>,
    feed: broadcast::Sender<Arc<RecordedEvent>>,
}

impl Dispatcher {
    pub fn new(store: Arc<dyn EventStore>) -> Self {
        Dispatcher {
            store,
            subscribers: Vec::new(),
            applied: watch::channel(0).0,
            feed: broadcast::channel(FEED_CAPACITY).0,
        }
    }

    pub fn subscribe(&mut self, s: Arc<dyn Subscriber>) {
        self.subscribers.push(s);
    }

    /// The last position every subscriber has applied.
    pub fn applied(&self) -> watch::Receiver<u64> {
        self.applied.subscribe()
    }

    /// Every applied event, as it is applied.
    pub fn feed(&self) -> broadcast::Sender<Arc<RecordedEvent>> {
        self.feed.clone()
    }

    /// Tails the log forever.
    pub async fn run(self, mut wake: Box<dyn Wake>) {
        let mut last = 0;
        loop {
            match self.store.read(&Query::after(last, BATCH)).await {
                Ok(batch) => {
                    let full = batch.len() == BATCH;
                    for event in batch {
                        for s in &self.subscribers {
                            s.apply(&event);
                        }
                        last = event.position;
                        self.applied.send_replace(last);
                        // No receivers is fine.
                        let _ = self.feed.send(Arc::new(event));
                    }
                    if full {
                        continue;
                    }
                }
                Err(e) => eprintln!("live: reading the log at {last} failed: {e}"),
            }
            tokio::select! {
                _ = wake.wait() => {}
                _ = crate::rt::sleep(POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use live_proto::sign::KeyPair;

    use super::*;
    use crate::store::MemoryStore;
    use crate::store::conformance::event;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<u64>>);

    impl Subscriber for Recorder {
        fn name(&self) -> &str {
            "recorder"
        }
        fn apply(&self, e: &RecordedEvent) {
            self.0.lock().unwrap().push(e.position);
        }
    }

    #[tokio::test]
    async fn replays_then_follows_in_order() {
        let store = Arc::new(MemoryStore::new());
        let key = KeyPair::generate();
        // More than one batch already in the log.
        for n in 0..(BATCH as u64 + 5) {
            store.append(vec![event(&key, "s", n)], &[]).await.unwrap();
        }
        let rec = Arc::new(Recorder::default());
        let mut d = Dispatcher::new(store.clone());
        d.subscribe(rec.clone());
        let mut applied = d.applied();
        let mut feed = d.feed().subscribe();
        let wake = store.wake().await.unwrap();
        let task = tokio::spawn(d.run(wake));

        let head = BATCH as u64 + 5;
        applied.wait_for(|p| *p == head).await.unwrap();
        assert_eq!(*rec.0.lock().unwrap(), (1..=head).collect::<Vec<_>>());
        assert_eq!(feed.recv().await.unwrap().position, 1);

        // A later append is woken, not polled for.
        let started = std::time::Instant::now();
        store.append(vec![event(&key, "s", 0)], &[]).await.unwrap();
        applied.wait_for(|p| *p == head + 1).await.unwrap();
        assert!(started.elapsed() < POLL);
        task.abort();
    }
}
