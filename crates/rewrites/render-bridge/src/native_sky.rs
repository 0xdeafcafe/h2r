//! Sky model strip admission at the indexed issue inside the sky stencil bracket.
//!
//! A strip the stage takes draws from the item extracted at its issue
//! (`SkyItem`: the view's node rows, the section's decompression, the
//! material pass). A strip it refuses keeps its guest draw and is counted
//! `scene_kept_sky_<cause>` ([`counters`]); one it takes is
//! `sky_plan_admitted` (docs/graphics/retire-d3d.md, "By caller").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;

pub use scene_extract::layout::sky::{Refusal, SkyItem};

static OUTCOMES: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// Counts one strip's admission: `admitted`, or the cause it keeps its
/// guest draw under. The device can still decline the external pass during
/// attachment and resource reservation (`native_sky_*`).
pub fn count(outcome: &'static str) {
    let mut counts = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    *counts.entry(outcome).or_default() += 1;
}

/// `sky_plan_admitted`, `scene_kept_sky_<cause>` and
/// `scene_dropped_sky_<why>`.
pub fn counters() -> Vec<(String, u64)> {
    OUTCOMES.lock().unwrap_or_else(|e| e.into_inner())
        .iter().map(|(&outcome, &n)| (counter_name(outcome), n)).collect()
}

/// The counter of `outcome`: `sky_plan_admitted`, or the kept strip's
/// `scene_kept_sky_<cause>`.
pub fn counter_name(outcome: &str) -> String {
    if outcome == "admitted" {
        "sky_plan_admitted".to_owned()
    } else if let Some(why) = outcome.strip_prefix("dropped_") {
        format!("scene_dropped_sky_{why}")
    } else if outcome.starts_with("plan_") || outcome.starts_with("geometry_") || outcome.starts_with("cut_") {
        format!("sky_{outcome}")
    } else {
        format!("scene_kept_sky_{outcome}")
    }
}

/// Counts one strip's look from the material against the draw:
/// `sky_plan_<outcome>`, `matched`, `program`, `textures`, or why the
/// load has no look (`unloaded`, `shader`, `look`, `extraction`).
pub fn count_plan(outcome: &'static str) {
    let name = match outcome {
        "matched" => "plan_matched",
        "program" => "plan_program",
        "textures" => "plan_textures",
        "unloaded" => "plan_unloaded",
        "shader" => "plan_shader",
        "extraction" => "plan_extraction",
        _ => "plan_look",
    };
    *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
}

/// Counts one strip's section geometry from the load against the draw:
/// `sky_geometry_<outcome>`, `matched`, `indices`, `window`, `streams`,
/// `part`, or why the load has none (`unloaded`, `model`, `section`,
/// `extraction`).
pub fn count_geometry(outcome: &'static str) {
    let name = match outcome {
        "matched" => "geometry_matched",
        "indices" => "geometry_indices",
        "window" => "geometry_window",
        "streams" => "geometry_streams",
        "part" => "geometry_part",
        "unloaded" => "geometry_unloaded",
        "model" => "geometry_model",
        "extraction" => "geometry_extraction",
        "absolute" => "geometry_absolute",
        _ => "geometry_section",
    };
    *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
}

/// The cut's arm (docs/graphics/retire-d3d-r5.md, "The sky's cut"): the
/// guest's sky pass is passed over for the rest of a BSP once
/// [`CUT_TO_ANSWER`] strips in a row have had the look and geometry from
/// the load matching; one strip that does not
/// stops it until the next BSP. Counted `sky_cut_<outcome>`: `waiting`,
/// `ready`, `stopped`.
pub const CUT_TO_ANSWER: u64 = 256;

#[derive(Debug, Default)]
struct CutArm {
    generation: u64,
    run: u64,
    stopped: bool,
}

static CUT: Mutex<CutArm> = Mutex::new(CutArm { generation: 0, run: 0, stopped: false });

/// What draws the cut: the game's glue over the device and the strips the
/// last guest pass admitted.
pub trait CutHost: Send + Sync {
    /// A guest sky pass starts: its admitted strips are kept for the cut.
    fn begin_guest(&self, mem: &dyn GuestMemory);
    /// The guest sky pass ended.
    fn end_guest(&self);
    /// Draws the kept strips as one native pass with the guest's sky
    /// passed over; the number drawn, or why not (the sky is dropped this
    /// pass, `scene_dropped_sky_<why>`).
    fn draw(&self, mem: &mut dyn GuestMemory) -> Result<usize, &'static str>;
}

static CUT_HOST: Mutex<Option<Arc<dyn CutHost>>> = Mutex::new(None);

/// Installs the cut's host (the game's glue). Without one the guest runs.
pub fn set_cut_host(host: Arc<dyn CutHost>) {
    *CUT_HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Whether this sky pass is the cut's: the arm ready for `generation`.
/// A ready arm takes every pass: the guest's `0x0003ede0` never runs
/// again for the BSP.
pub fn cut_takes(generation: u64) -> bool {
    let arm = CUT.lock().unwrap_or_else(|e| e.into_inner());
    arm.generation == generation && !arm.stopped && arm.run >= CUT_TO_ANSWER
}

/// Stops the arm for `generation` (a check the strips do not see, such
/// as the view block, failed at a guest pass).
pub fn cut_miss(generation: u64) {
    let mut arm = CUT.lock().unwrap_or_else(|e| e.into_inner());
    if arm.generation == generation {
        arm.stopped = true;
    }
    drop(arm);
    *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).entry("cut_stopped").or_default() += 1;
}

/// What `0x0003ede0`'s entry does with the guest's pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyTurn {
    /// The cut drew the sky; the guest's pass is passed over.
    Drawn,
    /// The cut's pass was refused: the sky is not drawn this pass and the
    /// guest's pass is passed over, counted `scene_dropped_sky_<why>`.
    Dropped,
    /// The arm is not ready: the guest's pass runs and its strips are
    /// recorded for the cut (`sky_cut_waiting`, `sky_cut_stopped`).
    Guest,
}

impl SkyTurn {
    /// Whether the guest's pass is passed over.
    pub fn passes_over(self) -> bool {
        self != SkyTurn::Guest
    }
}

/// At `0x0003ede0`'s entry. A ready arm draws the cut or drops the sky
/// (`sky_cut_drawn`, `scene_dropped_sky_<why>`), and never runs the guest.
pub fn before_sky(mem: &mut dyn GuestMemory, generation: u64) -> SkyTurn {
    let Some(host) = CUT_HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return SkyTurn::Guest };
    if !cut_takes(generation) {
        host.begin_guest(mem);
        return SkyTurn::Guest;
    }
    let (name, turn) = match host.draw(mem) {
        Ok(_) => ("cut_drawn", SkyTurn::Drawn),
        Err(why) => (dropped_name(why), SkyTurn::Dropped),
    };
    *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
    turn
}

/// The cut drawn on the bound target outside the scene's sky pass: the
/// ocean's reflection (`water_end::ocean::reflection`), with the arm's
/// readiness not asked. `Err` names why nothing was drawn (`host` with no
/// cut host).
pub fn draw_cut(mem: &mut dyn GuestMemory) -> Result<usize, &'static str> {
    let host = CUT_HOST.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("host")?;
    host.draw(mem)
}

/// At `0x0003ede0`'s return, when the guest ran.
pub fn after_sky() {
    if let Some(host) = CUT_HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        host.end_guest();
    }
}

/// The outcome key of a refused cut pass (`counter_name` makes it
/// `scene_dropped_sky_<why>`).
fn dropped_name(why: &'static str) -> &'static str {
    match why {
        "unloaded" => "dropped_unloaded",
        "strips" => "dropped_strips",
        "rows" => "dropped_rows",
        "constants" => "dropped_constants",
        "device" => "dropped_device",
        _ => "dropped_pass",
    }
}

/// Records one strip (`all`: look and geometry both the plan's) and
/// answers whether the cut's arm is ready for this BSP.
pub fn cut_outcome(generation: u64, all: bool) -> bool {
    let mut arm = CUT.lock().unwrap_or_else(|e| e.into_inner());
    if arm.generation != generation {
        *arm = CutArm { generation, ..CutArm::default() };
    }
    if !all {
        arm.stopped = true;
    } else if !arm.stopped {
        arm.run += 1;
    }
    let ready = !arm.stopped && arm.run >= CUT_TO_ANSWER;
    let name = if arm.stopped { "cut_stopped" } else if ready { "cut_ready" } else { "cut_waiting" };
    drop(arm);
    *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
    ready
}

/// The draw has to consume the entire captured strip for this model part.
fn strip_count(expected: Option<u32>, resolved: Option<usize>) -> Result<(), &'static str> {
    match (expected, resolved) {
        (Some(n), Some(actual)) if n >= 3 && n as usize == actual => Ok(()),
        _ => Err("range"),
    }
}

/// Validate extraction and the view at the issue before touching native resources.
/// An error leaves this strip at its original position in the device's draw stream.
pub fn admit<T>(
    item: Result<SkyItem, Refusal>,
    view: u32,
    expected: Option<u32>,
    resolved: Option<usize>,
    prepare: impl FnOnce(SkyItem) -> Result<T, &'static str>,
) -> Result<T, &'static str> {
    let item = item.map_err(|why| match why {
        Refusal::Site => "site",
        Refusal::Geometry => "geometry",
        Refusal::Material => "material",
        Refusal::Transform => "transform",
    })?;
    if view == u32::MAX {
        return Err("view");
    }
    strip_count(expected, resolved)?;
    prepare(item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_item_does_not_consume_the_next_views_strip() {
        let make_item = || SkyItem {
            model: 3, section: 1, part: 2, shader: 9, pass: 0, rows: Vec::new(), decompression: [[0.0; 4]; 3],
        };
        assert_eq!(admit::<()>(Err(Refusal::Transform), 0, Some(5), Some(5),
            |_| panic!("failed extraction cannot submit")), Err("transform"));
        assert_eq!(admit::<()>(Ok(make_item()), u32::MAX, Some(5), Some(5),
            |_| panic!("missing view cannot submit")), Err("view"));
        assert_eq!(admit::<()>(Ok(make_item()), 0, Some(5), Some(5),
            |_| Err("stencil")), Err("stencil"));
        assert_eq!(admit(Ok(make_item()), 1, Some(5), Some(5),
            |item| Ok(item.shader)), Ok(9));
    }

    #[test]
    fn a_kept_strip_is_counted_scene_kept_by_its_cause() {
        assert_eq!(counter_name("admitted"), "sky_plan_admitted");
        assert_eq!(counter_name("cull"), "scene_kept_sky_cull");
        let read = |name: &str| counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n);
        let before = read("scene_kept_sky_range");
        count("range");
        assert_eq!(read("scene_kept_sky_range"), before + 1);
        assert!(counters().iter().all(|(k, _)| k == "sky_plan_admitted" || k.starts_with("scene_kept_sky_") || k.starts_with("sky_plan_") || k.starts_with("sky_geometry_") || k.starts_with("sky_cut_") || k.starts_with("scene_dropped_sky_")));
        let before = read("sky_geometry_window");
        count_geometry("window");
        assert_eq!(read("sky_geometry_window"), before + 1);
        assert_eq!(counter_name("geometry_matched"), "sky_geometry_matched");
    }

    /// The cut's arm is ready after a run of strips with everything the
    /// plan's, stops at one that is not, and starts again at a new BSP.
    #[test]
    fn the_cut_arm_is_ready_after_its_run_and_stops_at_a_miss() {
        let generation = 0xc07_0001;
        for _ in 0..CUT_TO_ANSWER - 1 {
            assert!(!cut_outcome(generation, true));
        }
        assert!(cut_outcome(generation, true));
        assert!(cut_outcome(generation, true));
        assert!(!cut_outcome(generation, false));
        assert!(!cut_outcome(generation, true));
        assert!(!cut_outcome(generation + 1, true));
        assert_eq!(counter_name("cut_ready"), "sky_cut_ready");
        // Ready, the cut takes every pass; a miss returns every pass to the
        // guest for the BSP.
        let ready = generation + 2;
        assert!(!cut_takes(ready));
        for _ in 0..CUT_TO_ANSWER {
            cut_outcome(ready, true);
        }
        assert!((0..256).all(|_| cut_takes(ready)), "no pass is the guest's once ready");
        assert!(!cut_takes(ready + 1), "another BSP's");
        cut_miss(ready);
        assert!((0..256).all(|_| !cut_takes(ready)));
        assert_eq!(counter_name("dropped_device"), "scene_dropped_sky_device");
        assert!(SkyTurn::Dropped.passes_over() && SkyTurn::Drawn.passes_over() && !SkyTurn::Guest.passes_over());
    }

    #[test]
    fn a_short_or_missing_sky_strip_stays_with_the_device() {
        assert_eq!(strip_count(Some(7), Some(7)), Ok(()));
        for pair in [(Some(7), Some(6)), (Some(2), Some(2)), (None, None)] {
            assert_eq!(strip_count(pair.0, pair.1), Err("range"));
        }
    }
}
