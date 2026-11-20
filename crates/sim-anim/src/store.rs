//! The clip store: decoded clips keyed by the animation's data-sizes
//! address (`element + 0x3c`, in tag data, stable while a map is loaded).
//!
//! Each lookup compares the [`Shape`] it was built from, so a key reused
//! for a different animation rebuilds. The whole store drops when the
//! [`Epoch`] moves (map load, checkpoint revert and restore, snapshot
//! load) and when it grows past [`BUDGET`]. A clip never changes a
//! result, only whether the blob is read again, so flushing is always
//! safe.
use crate::clip::{Clip, Layout, Shape};
use sim_core::Epoch;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Bytes of clips held before the store starts over.
pub const BUDGET: usize = 256 << 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub builds: u64,
    pub flushes: u64,
}

#[derive(Default)]
struct Inner {
    seen: u64,
    clips: HashMap<u32, Arc<Clip>>,
    bytes: usize,
    stats: Stats,
}

impl Inner {
    fn flush(&mut self) {
        if !self.clips.is_empty() {
            self.stats.flushes += 1;
        }
        self.clips.clear();
        self.bytes = 0;
    }
}

pub struct ClipStore {
    epoch: &'static Epoch,
    inner: Mutex<Inner>,
}

impl ClipStore {
    pub fn new(epoch: &'static Epoch) -> Self {
        Self {
            epoch,
            inner: Mutex::new(Inner {
                seen: epoch.get(),
                ..Inner::default()
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The clip for `key`, built from `fetch(layout)` (the blob's
    /// `layout.len` bytes) when the store has none for this shape. `None`
    /// when the shape has no layout or the blob cannot be read.
    pub fn get(
        &self,
        key: u32,
        shape: Shape,
        fetch: impl FnOnce(&Layout) -> Option<Box<[u8]>>,
    ) -> Option<Arc<Clip>> {
        let epoch = self.epoch.get();
        {
            let mut inner = self.lock();
            if inner.seen != epoch {
                inner.flush();
                inner.seen = epoch;
            }
            if let Some(clip) = inner.clips.get(&key)
                && clip.shape == shape
            {
                let clip = clip.clone();
                inner.stats.hits += 1;
                return Some(clip);
            }
        }
        let layout = Layout::new(&shape)?;
        let clip = Arc::new(Clip::decode(shape, fetch(&layout)?)?);
        let mut inner = self.lock();
        inner.stats.builds += 1;
        if inner.seen == self.epoch.get() {
            let size = clip.footprint();
            if inner.bytes + size > BUDGET {
                inner.flush();
            }
            inner.bytes += size;
            if let Some(old) = inner.clips.insert(key, clip.clone()) {
                inner.bytes -= old.footprint();
            }
        }
        Some(clip)
    }

    /// Drop every clip.
    pub fn clear(&self) {
        self.lock().flush();
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    pub fn len(&self) -> usize {
        self.lock().clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
