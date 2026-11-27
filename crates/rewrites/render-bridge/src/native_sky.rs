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
    if outcome == "admitted" { "sky_plan_admitted".to_owned() } else { format!("scene_kept_sky_{outcome}") }
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
        assert!(counters().iter().all(|(k, _)| k == "sky_plan_admitted" || k.starts_with("scene_kept_sky_")));
    }

    #[test]
    fn a_short_or_missing_sky_strip_stays_with_the_device() {
        assert_eq!(strip_count(Some(7), Some(7)), Ok(()));
        for pair in [(Some(7), Some(6)), (Some(2), Some(2)), (None, None)] {
            assert_eq!(strip_count(pair.0, pair.1), Err("range"));
        }
    }
}
