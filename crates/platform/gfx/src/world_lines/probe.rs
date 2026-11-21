//! Where the window drew the marks on its last presents, for the agent's
//! `render.marks` (docs/runtime/agent-api.md, "render.marks"). The window
//! writes one [`Presented`] per present while the probe is armed; the
//! agent reads them. Unarmed, a present costs one atomic load.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// The presents kept.
pub const KEEP: usize = 32;

/// The anchors kept per present: the first groups the window visited.
pub const ANCHORS: usize = 16;

/// What kind of frame a present showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A genuine guest image, its marks on its own cameras.
    Guest,
    /// A genuine guest image without a usable world camera for its marks.
    GuestWithoutWorldView,
    /// An in-between frame the host made.
    Synthetic,
    /// A frame with no present metadata.
    Raw,
    /// The loading picture.
    Loading,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Guest => "guest",
            Kind::GuestWithoutWorldView => "no_world_view",
            Kind::Synthetic => "synthetic",
            Kind::Raw => "raw",
            Kind::Loading => "loading",
        }
    }
}

/// One present as the window drew it.
#[derive(Clone, Debug, PartialEq)]
pub struct Presented {
    /// Counts the presents recorded since the probe was armed.
    pub seq: u64,
    /// When the window drew it.
    pub at: Instant,
    pub kind: Kind,
    /// A made frame's blend fraction (0 the frame before, 1 the newest).
    pub fraction: Option<f32>,
    /// A made frame drew some view with another camera than its marks'
    /// image, and the marks were moved onto it.
    pub shifted: bool,
    /// The first [`ANCHORS`] groups' anchors in window pixels through the
    /// first view's camera, `None` where the anchor is off screen or
    /// behind the camera. Empty without marks.
    pub anchors: Vec<Option<[f32; 2]>>,
}

/// The probe: armed by its first reader, then a ring of the last
/// [`KEEP`] presents.
pub struct Probe {
    armed: AtomicBool,
    seq: AtomicU64,
    ring: Mutex<VecDeque<Presented>>,
}

/// The window's.
pub static PRESENTED: Probe = Probe::new();

impl Default for Probe {
    fn default() -> Self {
        Self::new()
    }
}

impl Probe {
    pub const fn new() -> Probe {
        Probe {
            armed: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            ring: Mutex::new(VecDeque::new()),
        }
    }

    /// Whether the window should record its presents.
    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Records a present (the window, while armed). `seq` is filled in.
    pub fn record(&self, mut p: Presented) {
        if !self.armed() {
            return;
        }
        p.seq = self.seq.fetch_add(1, Ordering::Relaxed);
        p.anchors.truncate(ANCHORS);
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() == KEEP {
            ring.pop_front();
        }
        ring.push_back(p);
    }

    /// Arms the probe and returns the presents kept, oldest first, and
    /// whether it was armed before this call (a first call returns none).
    pub fn read(&self) -> (Vec<Presented>, bool) {
        let was = self.armed.swap(true, Ordering::Relaxed);
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        (ring.iter().cloned().collect(), was)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(n: usize) -> Presented {
        Presented {
            seq: 99,
            at: Instant::now(),
            kind: Kind::Synthetic,
            fraction: Some(0.5),
            shifted: true,
            anchors: vec![Some([1.0, 2.0]); n],
        }
    }

    #[test]
    fn the_probe_records_only_once_armed_and_keeps_the_last_presents() {
        let p = Probe::new();
        p.record(present(1));
        let (none, was) = p.read();
        assert!(none.is_empty() && !was, "nothing before the first read");
        for _ in 0..KEEP + 3 {
            p.record(present(ANCHORS + 4));
        }
        let (kept, was) = p.read();
        assert!(was);
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept[0].seq, 3);
        assert_eq!(kept[KEEP - 1].seq, KEEP as u64 + 2);
        assert!(kept.iter().all(|k| k.anchors.len() == ANCHORS));
    }
}
