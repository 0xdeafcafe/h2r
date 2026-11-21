//! Direct-boot census for a single display device. The sink submits original
//! Swap numbers. Only completion of that same Swap advances the measured
//! interval. Interpolated images and private captures never enter it.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use frame_scene::Readback;

use super::Census;

/// Original Swaps in one boot of the FrameScene census corpus.
pub const CORPUS_PRESENT: u64 = 3000;

pub struct Checkpoint {
    pub sealed: bool,
    pub inventory: Option<Arc<Census>>,
    pub counters: Option<BTreeMap<String, u64>>,
}

pub struct PresentWindow {
    state: Mutex<State>,
    seal_probe: fn() -> bool,
}

impl Default for PresentWindow {
    fn default() -> Self { Self::new(false) }
}

#[derive(Default)]
struct State {
    owner_enabled: bool,
    submitted: u64,
    pending: VecDeque<(u64, Input, PaletteViews)>,
    completed: u64,
    ordinal_gap: bool,
    map_changed: bool,
    sealed_at_boundary: bool,
    inventory_through: u64,
    inventory_gap: bool,
    inventory_at_boundary: Option<Arc<Census>>,
    measured_at_boundary: Option<Measured>,
    measured: Option<Measured>,
    views: PaletteViews,
}

#[derive(Clone, Default)]
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
        Self { state: Mutex::new(State { owner_enabled, ..Default::default() }), seal_probe: || false }
    }

    /// Reads the active map's last seal once, at the original corpus boundary.
    pub fn with_seal_probe(mut self, probe: fn() -> bool) -> Self {
        self.seal_probe = probe;
        self
    }

    /// A second game start before the boundary interrupts this level's
    /// post-activation interval. Already-submitted Swaps keep their map.
    pub fn next_game(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.measured.is_some() && state.submitted < CORPUS_PRESENT { state.map_changed = true; }
    }

    /// Called under the cumulative inventory's lock. Prove that its rows
    /// cover this device's consecutive original Swaps, starting at one.
    pub(super) fn observe_inventory(&self, present: u64, census: &Census) {
        if present > CORPUS_PRESENT { return; }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if present != state.inventory_through + 1 || present != state.submitted || census.frames != present {
            state.inventory_gap = true;
        }
        state.inventory_through = present;
        if present == CORPUS_PRESENT && !state.inventory_gap && state.inventory_at_boundary.is_none() {
            state.inventory_at_boundary = Some(Arc::new(census.clone()));
        }
    }

    /// Exact prefix of this device's boot. Missing evidence stays absent,
    /// even if an activation or a zero-valued reading appears later.
    pub fn checkpoint(&self) -> Option<Checkpoint> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.submitted >= CORPUS_PRESENT).then(|| Checkpoint {
            sealed: state.sealed_at_boundary,
            inventory: state.inventory_at_boundary.clone(),
            counters: state.measured_at_boundary.as_ref().map(|m| m.counters(state.owner_enabled)),
        })
    }

    /// Opens immediately after the last original Swap submitted by this
    /// device. An already-submitted Swap can still be drawing on its worker.
    pub fn activate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.measured.is_some() { return; }
        let start = state.submitted;
        state.measured = Some(Measured { start, through: start, ..Default::default() });
        state.pending.clear();
        state.views = PaletteViews::default();
    }

    /// Readiness on entry to each game view. Keep every observation until
    /// its original Swap, including an early split view before a later upload.
    pub fn game_view(&self, palette_ready: Option<bool>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.measured.is_some() {
            state.views.count += 1;
            state.views.pending |= palette_ready == Some(false);
            state.views.unknown |= palette_ready.is_none();
        }
    }

    pub(super) fn submit(&self, present: u64, input: Input) {
        let boundary_sealed = present == CORPUS_PRESENT && (self.seal_probe)();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.submitted < CORPUS_PRESENT && present != state.submitted + 1 { state.ordinal_gap = true; }
        if present == CORPUS_PRESENT && state.submitted < present {
            state.sealed_at_boundary = boundary_sealed;
        }
        state.submitted = state.submitted.max(present);
        if state.measured.is_some() {
            let views = std::mem::take(&mut state.views);
            state.pending.push_back((present, input, views));
        }
    }

    pub(crate) fn complete(&self, present: u64, result: DisplayResult,
        residency: Option<(u64, bool)>, worker_drops: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.completed < CORPUS_PRESENT && present != state.completed + 1 { state.ordinal_gap = true; }
        state.completed = state.completed.max(present);
        state.join(present, result, residency, worker_drops);
        if present == CORPUS_PRESENT && !state.ordinal_gap && !state.map_changed && state.measured_at_boundary.is_none() {
            state.measured_at_boundary = state.measured.as_ref()
                .filter(|m| m.start < present && m.through == present && m.gaps == 0)
                .cloned();
        }
    }

    /// A single completed prefix of original presents, sampled without
    /// pausing the guest or draining any in-flight rendering.
    pub fn counters(&self) -> BTreeMap<String, u64> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.measured.as_ref().map(|m| m.counters(state.owner_enabled)).unwrap_or_default()
    }
}

impl State {
    fn join(&mut self, present: u64, result: DisplayResult, residency: Option<(u64, bool)>, worker_drops: u64) {
        let State { pending, measured, .. } = self;
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
}

impl Measured {
    fn counters(&self, owner_enabled: bool) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<_, _> = [
            ("owner_enabled", u64::from(owner_enabled)), ("start", self.start), ("through", self.through), ("gaps", self.gaps),
            ("games", self.games), ("owned", self.owned), ("ordinary", self.ordinary),
            ("failed", self.failed), ("stage_drops", self.drops), ("unresident", self.unresident),
            ("unknown_residency", self.unknown), ("flare_queries", self.flare_queries),
            ("unmatched_flare_frames", self.unmatched_frames),
            ("palette_views", self.palette_views),
            ("palette_pending_frames", self.palette_pending_frames),
            ("palette_unknown_frames", self.palette_unknown_frames),
        ].into_iter().map(|(name, n)| (format!("scene_window_{name}"), n)).collect();
        for (kind, n) in Readback::ALL.into_iter().zip(self.reads) {
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
    fn corpus_freezes_the_original_inventory_and_completed_window_at_the_same_swap() {
        use frame_scene::Slot;
        use super::super::{TargetUse, Uses};

        let w = PresentWindow::new(false).with_seal_probe(|| true);
        let mut inventory = Census::default();
        let target = TargetUse { width: 3, height: 7, format: 9, depth: false, slot: Slot::Opaque, operation: "copy_to" };
        for n in 1..=CORPUS_PRESENT {
            inventory.frames = n;
            if n == CORPUS_PRESENT {
                let once = Uses { count: 1, first_frame: n, last_frame: n };
                inventory.reads.insert((Slot::AfterLights, "surface"), once);
                inventory.targets.insert(target, once);
            }
            w.submit(n, input(0));
            w.observe_inventory(n, &inventory);
            if n < CORPUS_PRESENT {
                w.complete(n, DisplayResult::Ordinary, None, 0);
                assert!(w.checkpoint().is_none());
            }
            if n == CORPUS_PRESENT - 1 { w.activate(); }
        }
        let producer = w.checkpoint().unwrap();
        assert!(producer.sealed);
        assert_eq!(producer.inventory.unwrap().frames, CORPUS_PRESENT);
        assert!(producer.counters.is_none());

        // The game can change after submission while Swap 3000 is still executing.
        w.next_game();
        w.submit(CORPUS_PRESENT + 1, input(9));
        inventory.frames += 1;
        inventory.reads.get_mut(&(Slot::AfterLights, "surface")).unwrap().last_frame = inventory.frames;
        inventory.targets.insert(TargetUse { width: 11, ..target }, Uses { count: 1, first_frame: inventory.frames, last_frame: inventory.frames });
        w.observe_inventory(CORPUS_PRESENT + 1, &inventory);
        w.complete(CORPUS_PRESENT, DisplayResult::Ordinary, Some((0, false)), 0);
        w.complete(CORPUS_PRESENT + 1, DisplayResult::Failed, None, 2);
        let checkpoint = w.checkpoint().unwrap();
        let inventory = checkpoint.inventory.unwrap();
        assert_eq!(inventory.reads[&(Slot::AfterLights, "surface")].last_frame, CORPUS_PRESENT);
        assert_eq!(inventory.targets.len(), 1);
        assert_eq!(inventory.targets[&target].first_frame, CORPUS_PRESENT);
        let counters = checkpoint.counters.unwrap();
        assert_eq!((counters["scene_window_start"], counters["scene_window_through"]), (CORPUS_PRESENT - 1, CORPUS_PRESENT));
        assert_eq!((counters["scene_window_games"], counters["scene_window_failed"]), (1, 0));
        assert_eq!(w.counters()["scene_window_failed"], 1);
    }

    #[test]
    fn absent_or_late_activation_map_changes_and_missing_ordinals_cannot_certify_the_prefix() {
        for case in ["absent", "late", "changed", "producer_gap", "worker_gap", "inventory_gap"] {
            let w = PresentWindow::default();
            let mut inventory = Census::default();
            for n in 1..=CORPUS_PRESENT {
                inventory.frames = n;
                if case != "producer_gap" || n != 1 { w.submit(n, Input::default()); }
                if case != "inventory_gap" || n != 1 { w.observe_inventory(n, &inventory); }
                if case != "worker_gap" || n != 1 { w.complete(n, DisplayResult::Ordinary, None, 0); }
                if n == 2 && !matches!(case, "absent" | "late") { w.activate(); }
                if n == 3 && case == "changed" { w.next_game(); }
            }
            if case == "late" { w.activate(); }
            let checkpoint = w.checkpoint().unwrap();
            assert_eq!(checkpoint.counters.is_some(), case == "inventory_gap", "{case}");
            assert_eq!(checkpoint.inventory.is_some(), !matches!(case, "inventory_gap" | "producer_gap"), "{case}");
        }
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
