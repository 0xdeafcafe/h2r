//! Direct-boot census for a single display device. The sink submits original
//! Swap numbers. Only completion of that same Swap advances the measured
//! interval. Interpolated images and private captures never enter it.
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use frame_scene::Readback;

#[derive(Default)]
pub struct PresentWindow(Mutex<State>);

#[derive(Default)]
struct State {
    owner_enabled: bool,
    submitted: u64,
    pending: VecDeque<(u64, Input, PaletteViews)>,
    measured: Option<Measured>,
    views: PaletteViews,
}

#[derive(Default)]
struct Measured {
    start: u64,
    through: u64,
    gaps: u64,
    games: u64,
    owned: u64,
    ordinary: u64,
    failed: u64,
    drops: u64,
    unresident: u64,
    unknown: u64,
    reads: [u64; Readback::COUNT],
    flare_queries: u64,
    unmatched_frames: u64,
    palette_views: u64,
    palette_pending_frames: u64,
    palette_unknown_frames: u64,
}

/// Observations at the actual entries of all views in one original Swap.
#[derive(Default)]
struct PaletteViews {
    count: u64,
    pending: bool,
    unknown: bool,
}

#[derive(Default)]
pub(super) struct Input {
    pub game_view: bool,
    pub reads: [u32; Readback::COUNT],
    pub drops: u64,
}

pub(crate) enum DisplayResult {
    Owned { game_queries: u64 },
    Ordinary,
    Failed,
}

impl PresentWindow {
    pub fn new(owner_enabled: bool) -> Self {
        Self(Mutex::new(State { owner_enabled, ..Default::default() }))
    }

    /// Opens immediately after the last original Swap submitted by this
    /// device. An already-submitted Swap can still be drawing on its worker.
    pub fn activate(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let start = state.submitted;
        state.measured = Some(Measured { start, through: start, ..Default::default() });
        state.pending.clear();
        state.views = PaletteViews::default();
    }

    /// Readiness on entry to each game view. Keep every observation until
    /// its original Swap, including an early split view before a later upload.
    pub fn game_view(&self, palette_ready: Option<bool>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.measured.is_some() {
            state.views.count += 1;
            state.views.pending |= palette_ready == Some(false);
            state.views.unknown |= palette_ready.is_none();
        }
    }

    pub(super) fn submit(&self, present: u64, input: Input) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.submitted = present;
        if state.measured.is_some() {
            let views = std::mem::take(&mut state.views);
            state.pending.push_back((present, input, views));
        }
    }

    pub(crate) fn complete(&self, present: u64, result: DisplayResult,
        residency: Option<(u64, bool)>, worker_drops: u64) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let State { pending, measured, .. } = &mut *state;
        let Some(m) = measured.as_mut() else { return };
        if present <= m.start { return; }
        if present != m.through + 1 || pending.front().map(|p| p.0) != Some(present) {
            m.gaps += 1;
            // Discard stale inputs; never join a different present.
            pending.retain(|(n, _, _)| *n > present);
            m.through = m.through.max(present);
            return;
        }
        let (_, input, views) = pending.pop_front().expect("matching submitted present");
        m.through = present;
        m.drops += input.drops + worker_drops;
        m.failed += u64::from(matches!(result, DisplayResult::Failed));
        if !input.game_view && views.count == 0 { return; }
        m.games += 1;
        m.palette_views += views.count;
        m.palette_pending_frames += u64::from(views.pending);
        m.palette_unknown_frames += u64::from(views.unknown || views.count == 0);
        for (total, count) in m.reads.iter_mut().zip(input.reads) { *total += u64::from(count); }
        match result {
            DisplayResult::Owned { game_queries } => {
                m.owned += 1;
                m.flare_queries += game_queries;
                m.unmatched_frames += u64::from(game_queries != u64::from(input.reads[Readback::PixelCount.index()]));
            }
            DisplayResult::Ordinary => m.ordinary += 1,
            DisplayResult::Failed => {}
        }
        let (missing, unknown) = residency.unwrap_or((0, true));
        m.unresident += missing;
        m.unknown += u64::from(unknown);
    }

    /// A single completed prefix of original presents, sampled without
    /// pausing the guest or draining any in-flight rendering.
    pub fn counters(&self) -> BTreeMap<String, u64> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(m) = &state.measured else { return BTreeMap::new() };
        let mut out: BTreeMap<_, _> = [
            ("owner_enabled", u64::from(state.owner_enabled)), ("start", m.start), ("through", m.through), ("gaps", m.gaps),
            ("games", m.games), ("owned", m.owned), ("ordinary", m.ordinary),
            ("failed", m.failed), ("stage_drops", m.drops), ("unresident", m.unresident),
            ("unknown_residency", m.unknown), ("flare_queries", m.flare_queries),
            ("unmatched_flare_frames", m.unmatched_frames),
            ("palette_views", m.palette_views),
            ("palette_pending_frames", m.palette_pending_frames),
            ("palette_unknown_frames", m.palette_unknown_frames),
        ].into_iter().map(|(name, n)| (format!("scene_window_{name}"), n)).collect();
        for (kind, n) in Readback::ALL.into_iter().zip(m.reads) {
            out.insert(format!("scene_window_read_{}", kind.name()), n);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(reads: u32) -> Input {
        let mut input = Input { game_view: true, ..Default::default() };
        input.reads[Readback::PixelCount.index()] = reads;
        input
    }

    #[test]
    fn late_preload_completion_cannot_hide_a_declined_game_present() {
        let w = PresentWindow::default();
        w.submit(1, input(1));
        w.activate();
        w.submit(2, input(1));
        w.complete(1, DisplayResult::Owned { game_queries: 1 }, None, 2);
        w.submit(3, Input::default());
        w.complete(2, DisplayResult::Ordinary, Some((0, false)), 0);
        w.complete(3, DisplayResult::Ordinary, None, 0);
        let c = w.counters();
        assert_eq!((c["scene_window_start"], c["scene_window_through"]), (1, 3));
        assert_eq!((c["scene_window_games"], c["scene_window_owned"], c["scene_window_ordinary"]), (1, 0, 1));
        assert_eq!((c["scene_window_unknown_residency"], c["scene_window_stage_drops"]), (0, 0));
    }

    #[test]
    fn later_ready_split_views_do_not_hide_an_incomplete_or_unknown_palette() {
        let w = PresentWindow::new(true);
        w.game_view(None);
        w.submit(1, input(0));
        w.activate();
        w.game_view(Some(false));
        w.game_view(None);
        w.game_view(Some(true));
        w.submit(2, input(0));
        w.game_view(Some(true)); // Next Swap is still in flight.
        w.submit(3, input(0));
        w.complete(1, DisplayResult::Ordinary, None, 0);
        w.complete(2, DisplayResult::Owned { game_queries: 0 }, Some((0, false)), 0);
        let c = w.counters();
        assert_eq!(c["scene_window_palette_views"], 3);
        assert_eq!(c["scene_window_palette_pending_frames"], 1);
        assert_eq!(c["scene_window_palette_unknown_frames"], 1);
        w.complete(3, DisplayResult::Owned { game_queries: 0 }, Some((0, false)), 0);
        w.submit(4, input(0)); // Game work without a view-entry observation.
        w.complete(4, DisplayResult::Owned { game_queries: 0 }, Some((0, false)), 0);
        let c = w.counters();
        assert_eq!(c["scene_window_palette_views"], 4);
        assert_eq!(c["scene_window_palette_pending_frames"], 1);
        assert_eq!(c["scene_window_palette_unknown_frames"], 2);
    }

    #[test]
    fn matched_sums_cannot_hide_cross_frame_flare_or_residency_errors() {
        let w = PresentWindow::default();
        w.activate();
        w.submit(1, input(2));
        w.submit(2, input(0));
        w.complete(1, DisplayResult::Owned { game_queries: 1 }, None, 0);
        w.complete(2, DisplayResult::Owned { game_queries: 1 }, Some((3, false)), 1);
        w.submit(3, input(0)); // In flight; no partial evidence in the sample.
        let c = w.counters();
        assert_eq!(c["scene_window_through"], 2);
        assert_eq!((c["scene_window_read_pixel_count"], c["scene_window_flare_queries"]), (2, 2));
        assert_eq!(c["scene_window_unmatched_flare_frames"], 2);
        assert_eq!((c["scene_window_unresident"], c["scene_window_unknown_residency"]), (3, 1));
        assert_eq!(c["scene_window_stage_drops"], 1);
        w.complete(4, DisplayResult::Failed, None, 0);
        assert_eq!(w.counters()["scene_window_gaps"], 1);
    }
}
