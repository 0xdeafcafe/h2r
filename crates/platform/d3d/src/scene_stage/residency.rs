//! Model and material lookup failures on the worker that owns the displayed
//! frame. Planning refusals keep their guest draws; private re-recordings
//! have no effect on these counts.
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

use frame_scene::InstanceKey;

use crate::backend::DrawOutcome;

thread_local! {
    // Only the final execution of a committed scene stage opens a scope.
    static LOOKUPS: RefCell<Option<HashSet<InstanceKey>>> = const { RefCell::new(None) };
}

/// A planned object draw actually failed its model, geometry or material
/// lookup. The salted handle identifies multiple parts of the same instance.
pub fn missing_object(key: InstanceKey) {
    LOOKUPS.with_borrow_mut(|pending| {
        if let Some(missing) = pending {
            missing.insert(key);
        }
    });
}

#[derive(Default)]
pub(crate) struct Frame {
    missing: HashSet<InstanceKey>,
    saw_inputs: bool,
    unknown: bool,
}

impl Frame {
    /// The legacy route reports the actual result of its retained guest
    /// packet. A skipped packet carries no confirmed native instance key.
    pub fn guest_draw(&mut self, result: &DrawOutcome) {
        self.saw_inputs = true;
        self.unknown |= *result != DrawOutcome::Drawn;
    }

    /// Consult the lookup evidence only if the guest has relinquished the
    /// stage and the backend actually drops it. Copied targets and separate
    /// stages of one object contribute once to this present.
    pub fn record<T>(&mut self, run: impl FnOnce() -> T, dropped: impl FnOnce(&T) -> bool) -> T {
        self.saw_inputs = true;
        let prior = LOOKUPS.replace(Some(HashSet::new()));
        let result = run();
        let missing = LOOKUPS.replace(prior).expect("observing a scene stage");
        if dropped(&result) {
            self.unknown |= missing.is_empty();
            self.missing.extend(missing);
        }
        result
    }

    /// A stage was skipped before its content could be inspected.
    pub fn unclassified(&mut self) {
        self.saw_inputs = true;
        self.unknown = true;
    }

    pub fn present(&mut self, complete: bool) -> Option<(u64, bool)> {
        if !self.saw_inputs && complete { return None; }
        let observed = (self.missing.len() as u64, self.unknown || !complete);
        let mut totals = TOTALS.lock().unwrap_or_else(|e| e.into_inner());
        self.finish(&mut totals, complete);
        Some(observed)
    }

    fn finish(&mut self, totals: &mut Totals, complete: bool) {
        totals.frames += 1;
        totals.unresident += self.missing.len() as u64;
        totals.unknown_frames += u64::from(self.unknown || !complete);
        self.saw_inputs = false;
        self.missing.clear();
        self.unknown = false;
    }
}

#[derive(Default)]
struct Totals {
    frames: u64,
    unresident: u64,
    unknown_frames: u64,
}

impl Totals {
    fn counters(&self) -> Option<[(&'static str, u64); 3]> {
        (self.frames > 0).then_some([
            ("scene_unresident", self.unresident),
            ("scene_residency_frames", self.frames),
            ("scene_residency_unknown_frames", self.unknown_frames),
        ])
    }
}

static TOTALS: Mutex<Totals> = Mutex::new(Totals { frames: 0, unresident: 0, unknown_frames: 0 });

/// Cumulative render-worker observations. Subtract two snapshots over the
/// same post-load interval and require no unknown frames.
pub fn counters() -> Option<[(&'static str, u64); 3]> {
    TOTALS.lock().unwrap_or_else(|e| e.into_inner()).counters()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_dropped_instances_once_per_present_and_excludes_fallback_and_private_draws() {
        let mut totals = Totals::default();
        assert!(totals.counters().is_none());
        let mut frame = Frame::default();
        let a = InstanceKey(17);
        let b = InstanceKey(29);
        missing_object(b); // private recording or pre-admission probe
        assert!(frame.record(|| { missing_object(b); true }, |recorded| !recorded));
        for _ in 0..2 {
            assert!(!frame.record(|| { missing_object(a); missing_object(a); false }, |recorded| !recorded));
        }
        frame.finish(&mut totals, true);
        assert_eq!((totals.frames, totals.unresident, totals.unknown_frames), (1, 1, 0));
        assert!(!frame.record(|| { missing_object(a); missing_object(b); false }, |recorded| !recorded));
        frame.finish(&mut totals, true);
        assert_eq!((totals.frames, totals.unresident, totals.unknown_frames), (2, 3, 0));
    }

    #[test]
    fn accepts_measured_zero_and_preserves_unknown_coverage_on_unclassified_or_failed_frames() {
        let mut totals = Totals::default();
        let mut frame = Frame::default();
        frame.guest_draw(&DrawOutcome::Drawn);
        frame.finish(&mut totals, true);
        assert_eq!(totals.counters().unwrap()[0], ("scene_unresident", 0));
        assert!(!frame.record(|| false, |recorded| !recorded));
        frame.unclassified();
        frame.finish(&mut totals, true);
        frame.finish(&mut totals, false);
        frame.guest_draw(&DrawOutcome::Skipped("synthetic refusal".into()));
        frame.finish(&mut totals, true);
        assert_eq!((totals.frames, totals.unresident, totals.unknown_frames), (4, 0, 3));
    }
}
