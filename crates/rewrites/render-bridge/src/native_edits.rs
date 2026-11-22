//! The native renderer's refresh queue after a live raw edit
//! (docs/editor/realtime-edits.md, "The native refresh").
//!
//! Lane R1's stub of the interface the page fixes: the hot-edit glue
//! pushes, the world and object glue drain. Lane R2 owns this file and
//! replaces it.

use std::sync::Mutex;

/// What to re-read from the overlaid map file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A structure section: `bsp` the `sbsp` datum, `kind` 0 cluster or 1
    /// instanced definition; `grown` when its counts changed.
    Section { bsp: u32, kind: u8, index: u32, grown: bool },
    /// A render model, by datum.
    Model { tag: u32 },
    /// One image of a bitmap, by datum and index.
    Image { bitmap: u32, image: u32 },
}

static QUEUE: Mutex<Vec<Edit>> = Mutex::new(Vec::new());

/// Queues `edit` for the next drain; a repeat of a queued edit is dropped.
pub fn push(edit: Edit) {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if !q.contains(&edit) {
        q.push(edit);
    }
}

/// Everything queued since the last drain, in order.
pub fn drain() -> Vec<Edit> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
}
