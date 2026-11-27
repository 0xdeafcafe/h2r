//! Sky model strip admission at the indexed issue inside the sky stencil bracket.
//!
//! A strip the stage takes draws from the item extracted at its issue
//! (`SkyItem`: the view's node rows, the section's decompression, the
//! material pass). A strip it refuses keeps its guest draw and is counted
//! `scene_kept_sky_<cause>` ([`counters`]); one it takes is
//! `sky_plan_admitted` (docs/graphics/retire-d3d.md, "By caller").

use std::collections::BTreeMap;
use std::sync::Mutex;

pub use scene_extract::layout::sky::{Refusal, SkyItem};

static OUTCOMES: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// Counts one strip's admission: `admitted`, or the cause it keeps its
/// guest draw under. The device can still decline the external pass during
/// attachment and resource reservation (`native_sky_*`).
pub fn count(outcome: &'static str) {
    let mut counts = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    *counts.entry(outcome).or_default() += 1;
}

/// `sky_plan_admitted` and `scene_kept_sky_<cause>`.
pub fn counters() -> Vec<(String, u64)> {
    OUTCOMES.lock().unwrap_or_else(|e| e.into_inner())
        .iter().map(|(&outcome, &n)| (counter_name(outcome), n)).collect()
}

/// The counter of `outcome`: `sky_plan_admitted`, or the kept strip's
/// `scene_kept_sky_<cause>`.
pub fn counter_name(outcome: &str) -> String {
    if outcome == "admitted" {
        "sky_plan_admitted".to_owned()
    } else if outcome.starts_with("rows_") {
        format!("sky_{outcome}")
    } else {
        format!("scene_kept_sky_{outcome}")
    }
}

/// The rows arm (docs/graphics/retire-d3d-r5.md, "The sky's node rows"):
/// the guest's rows answer, and the rows made from the sky's transform
/// (`scene_extract::layout::sky_rows`) are compared with them at every
/// strip until [`ROWS_TO_ANSWER`] have matched with none differing; then
/// the made rows are the ones the cut may draw, one strip in
/// [`ROWS_SAMPLE`] still compared, and any difference returns the rows to
/// the guest until the next BSP. Counted `sky_rows_<outcome>`: `exact`,
/// `close`, `differ`, `declined_<why>`, `skipped` (not sampled), with
/// `sky_rows_answering` the strips at which the made rows answer.
pub const ROWS_TO_ANSWER: u64 = 256;
pub const ROWS_SAMPLE: u64 = 64;

#[derive(Debug, Default)]
struct RowsArm {
    generation: u64,
    matched: u64,
    strips: u64,
    stopped: bool,
}

static ROWS: Mutex<RowsArm> = Mutex::new(RowsArm { generation: 0, matched: 0, strips: 0, stopped: false });

/// Whether this strip's rows are compared: every strip until the arm
/// answers, then one in [`ROWS_SAMPLE`]. A new `generation` (BSP) starts
/// the arm again.
pub fn rows_compared(generation: u64) -> bool {
    let mut arm = ROWS.lock().unwrap_or_else(|e| e.into_inner());
    if arm.generation != generation {
        *arm = RowsArm { generation, ..RowsArm::default() };
    }
    arm.strips += 1;
    let answering = !arm.stopped && arm.matched >= ROWS_TO_ANSWER;
    !answering || arm.strips % ROWS_SAMPLE == 0
}

/// Records one strip's comparison (`Ok` the outcome of
/// `sky_rows::compare`, `Err` why no rows were made; `None` not
/// compared) and answers whether the made rows answer at this strip.
pub fn rows_outcome(outcome: Option<Result<&'static str, &'static str>>) -> bool {
    let mut arm = ROWS.lock().unwrap_or_else(|e| e.into_inner());
    let name = match outcome {
        None => "skipped".to_owned(),
        Some(Ok(matched @ ("exact" | "close"))) => {
            arm.matched += 1;
            matched.to_owned()
        }
        Some(Ok(other)) => {
            arm.stopped = true;
            other.to_owned()
        }
        Some(Err(why)) => {
            // A strip it cannot make is the guest's; the arm does not answer it.
            let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
            *o.entry(declined_name(why)).or_default() += 1;
            return false;
        }
    };
    let answers = !arm.stopped && arm.matched >= ROWS_TO_ANSWER;
    drop(arm);
    let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    *o.entry(row_name(&name)).or_default() += 1;
    if answers {
        *o.entry("rows_answering").or_default() += 1;
    }
    answers
}

/// The outcome key of a rows outcome (`counter_name` makes it
/// `sky_rows_<outcome>`).
fn row_name(outcome: &str) -> &'static str {
    match outcome {
        "exact" => "rows_exact",
        "close" => "rows_close",
        "differ" => "rows_differ",
        _ => "rows_skipped",
    }
}

fn declined_name(why: &'static str) -> &'static str {
    match why {
        "site" => "rows_declined_site",
        "sky" => "rows_declined_sky",
        "rest" => "rows_declined_rest",
        "palette" => "rows_declined_palette",
        "node" => "rows_declined_node",
        _ => "rows_declined_constants",
    }
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
        assert!(counters().iter().all(|(k, _)| k == "sky_plan_admitted" || k.starts_with("scene_kept_sky_") || k.starts_with("sky_rows_")));
    }

    /// The made rows answer after enough matching strips, are then sampled,
    /// and one difference returns them to the guest until the next BSP.
    #[test]
    fn the_rows_arm_answers_after_its_matches_and_stops_at_a_difference() {
        let read = |name: &str| counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n);
        let generation = 0xfeed_0001;
        for _ in 0..ROWS_TO_ANSWER - 1 {
            assert!(rows_compared(generation));
            assert!(!rows_outcome(Some(Ok("exact"))));
        }
        let declined = read("sky_rows_declined_palette");
        assert!(rows_compared(generation));
        assert!(!rows_outcome(Some(Err("palette"))), "a strip it cannot make is the guest's");
        assert_eq!(read("sky_rows_declined_palette"), declined + 1);
        assert!(rows_compared(generation));
        assert!(rows_outcome(Some(Ok("close"))), "answers from the last match");
        let compared = (0..ROWS_SAMPLE).filter(|_| rows_compared(generation)).count();
        assert_eq!(compared, 1, "one strip in the sample once answering");
        assert!(!rows_outcome(Some(Ok("differ"))));
        assert!(rows_compared(generation), "stopped: every strip is compared again");
        assert!(!rows_outcome(Some(Ok("exact"))), "stopped until the next BSP");
        assert!(rows_compared(generation + 1));
        assert!(!rows_outcome(Some(Ok("exact"))), "a new BSP starts again");
        assert!(counters().iter().any(|(k, _)| k == "sky_rows_exact"));
    }

    #[test]
    fn a_short_or_missing_sky_strip_stays_with_the_device() {
        assert_eq!(strip_count(Some(7), Some(7)), Ok(()));
        for pair in [(Some(7), Some(6)), (Some(2), Some(2)), (None, None)] {
            assert_eq!(strip_count(pair.0, pair.1), Err("range"));
        }
    }
}
