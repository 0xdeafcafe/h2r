//! The first-person replay table across the dynamic passes of one view.
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use guest_layout::memory::GuestMemory;
use engine::render::light_gather::first_person::slots::Slots;
use engine::render::light_gather::{Refusal, Result};
use engine::render_loop::occlusion::guest::{FRAME, VIEW_INDEX};

struct Gather {
    frame: u32,
    view: u32,
    slots: Result<Slots>,
}

static GATHER: Mutex<Option<Gather>> = Mutex::new(None);
static FIRST: Mutex<Option<String>> = Mutex::new(None);
static EQUAL: AtomicU64 = AtomicU64::new(0);
static DIFFER: AtomicU64 = AtomicU64::new(0);
static UNREAD: AtomicU64 = AtomicU64::new(0);
static LAYOUT: AtomicU64 = AtomicU64::new(0);

pub(super) fn begin(m: &dyn GuestMemory) {
    let mut gather = GATHER.lock().unwrap_or_else(|e| e.into_inner());
    *gather = m
        .u32(FRAME)
        .ok()
        .zip(m.u32(VIEW_INDEX).ok())
        .map(|(frame, view)| Gather {
            frame,
            view,
            slots: Ok(Slots::default()),
        });
}

pub(super) fn collect(m: &dyn GuestMemory, light: u32) {
    let mut current = GATHER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(g) = current.as_mut()
        && let Ok(slots) = &mut g.slots
        && let Err(why) = slots.gather(m, light)
    {
        g.slots = Err(why);
    }
}

/// The replay's slots, when every checked datum and record matches.
/// The caller reads the guest slots on decline or a difference.
pub(crate) fn replay(m: &dyn GuestMemory) -> Option<Vec<u32>> {
    let current = GATHER.lock().unwrap_or_else(|e| e.into_inner()).take();
    let compare = || -> Result<Option<Vec<u32>>> {
        let g = current.ok_or(Refusal::Layout)?;
        if m.u32(FRAME)? != g.frame || m.u32(VIEW_INDEX)? != g.view {
            return Err(Refusal::Layout);
        }
        let slots = g.slots?;
        if let Some(first) = slots.first_difference(m)? {
            DIFFER.fetch_add(1, Ordering::Relaxed);
            FIRST
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(|| format!("frame {} view {}: {first}", g.frame, g.view));
            Ok(None)
        } else {
            EQUAL.fetch_add(1, Ordering::Relaxed);
            Ok(Some(slots.handles()))
        }
    };
    match compare() {
        Ok(slots) => slots,
        Err(why) => {
            let counter = if why == Refusal::Memory {
                &UNREAD
            } else {
                &LAYOUT
            };
            counter.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

pub(super) fn forget() {
    GATHER.lock().unwrap_or_else(|e| e.into_inner()).take();
    FIRST.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub fn first_mismatch() -> Option<String> {
    FIRST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn counters() -> [(&'static str, u64); 5] {
    let equal = EQUAL.load(Ordering::Relaxed);
    let differ = DIFFER.load(Ordering::Relaxed);
    [
        ("scene_first_person_visibility_checked", equal + differ),
        ("scene_first_person_visibility_equal", equal),
        ("scene_first_person_visibility_differ", differ),
        (
            "scene_first_person_visibility_declined_unread",
            UNREAD.load(Ordering::Relaxed),
        ),
        (
            "scene_first_person_visibility_declined_layout",
            LAYOUT.load(Ordering::Relaxed),
        ),
    ]
}
