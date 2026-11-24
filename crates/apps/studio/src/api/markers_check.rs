//! `check.run`'s markers area (docs/game/gametype-markers.md, "Agent
//! API"): the game type markers of the open map against their rules.
//!
//! - `markers.area.invalid` (error): a stored area `Area::validate`
//!   refuses.
//! - `markers.hill.corners_and_area` (error): a flag with an area that is
//!   a corner of a hill with corners; the host reads the area first, so
//!   the other corners would draw a second hill.
//! - `markers.area.no_floor` (warning): a hill, territory or site whose
//!   area holds no floor at its marker: a straight look down from the
//!   top of the area meets nothing, or meets it outside the area's
//!   heights.
//! - `markers.area.overlap` (warning): two zones of one kind whose areas
//!   share ground and height, so a player stands in both.
//!
//! A spawn or marker under the floor or off the map is not checked yet.
use scenario_edit::geometry::Ray;
use scenario_edit::marker_view::Placed;
use scenario_edit::marker_view::game_markers::{Area, Kind};

use super::checks::{Finding, Severity};
use crate::App;

fn finding(code: &str, severity: Severity, target: &str, message: String) -> Finding {
    Finding {
        area: "markers",
        code: code.into(),
        severity,
        message,
        target: target.into(),
        path: None,
    }
}

/// Whether two areas, at their markers, share a point: the heights
/// overlap, and an outline point (or an edge's middle, or the middle) of
/// one is inside the other. A cross whose arms meet with no point of
/// either inside the other is missed.
pub fn overlap(a: ([f32; 3], &Area), b: ([f32; 3], &Area)) -> bool {
    let (lo_a, hi_a) = (a.0[2] - a.1.below, a.0[2] + a.1.above);
    let (lo_b, hi_b) = (b.0[2] - b.1.below, b.0[2] + b.1.above);
    let (lo, hi) = (lo_a.max(lo_b), hi_a.min(hi_b));
    if lo > hi {
        return false;
    }
    let z = (lo + hi) / 2.0;
    let samples = |x: ([f32; 3], &Area)| {
        let outline = x.1.outline();
        let n = outline.len();
        let mut out: Vec<[f32; 3]> = outline
            .iter()
            .map(|o| [x.0[0] + o[0], x.0[1] + o[1], z])
            .collect();
        out.extend((0..n).map(|i| {
            let (p, q) = (outline[i], outline[(i + 1) % n]);
            [x.0[0] + (p[0] + q[0]) / 2.0, x.0[1] + (p[1] + q[1]) / 2.0, z]
        }));
        out.push([x.0[0], x.0[1], z]);
        out
    };
    samples(a).iter().any(|p| b.1.contains(b.0, *p))
        || samples(b).iter().any(|p| a.1.contains(a.0, *p))
}

/// The zone markers of `placed` with their area and marker place.
fn zones(placed: &[Placed]) -> Vec<(&Placed, &Area)> {
    placed
        .iter()
        .filter_map(|p| match &p.marker.kind {
            Kind::Hill(a) | Kind::Territory(a) | Kind::Site(a) => Some((p, a)),
            _ => None,
        })
        .collect()
}

/// `check.run`'s markers row ([`super::checks::CHECKS`]).
pub fn check(app: &mut App, target: Option<&str>) -> Result<Vec<Finding>, String> {
    // No map, no markers: nothing to find.
    let Some(scenario) = app.session.scenario() else {
        return Ok(Vec::new());
    };
    let placed = super::markers::placed_now(app, scenario);
    let wanted = |id: &str| target.is_none_or(|t| t == id);
    let mut out = Vec::new();
    let flags = scenario_edit::BlockKind::NetgameFlags;
    for (i, area) in scenario.flag_areas() {
        let id = format!("netgame_flags[{i}]");
        if !wanted(&id) {
            continue;
        }
        if let Err(e) = area.validate() {
            out.push(finding("markers.area.invalid", Severity::Error, &id, e.to_string()));
        }
        if scenario.is_hill_corner(i) && i < scenario.block(flags).len() {
            out.push(finding(
                "markers.hill.corners_and_area",
                Severity::Error,
                &id,
                "a hill with corners takes no area: convert its corners to one flag, or clear this area"
                    .into(),
            ));
        }
    }
    let zones = zones(&placed);
    for (p, area) in &zones {
        if !wanted(&p.id) {
            continue;
        }
        let at = p.place.at;
        let ray = Ray {
            origin: [at[0], at[1], at[2] + area.above + 0.01],
            dir: [0.0, 0.0, -1.0],
        };
        match app.session.surface_hit(&ray) {
            Some(hit) if hit[2] >= at[2] - area.below && hit[2] <= at[2] + area.above => {}
            Some(hit) => out.push(finding(
                "markers.area.no_floor",
                Severity::Warning,
                &p.id,
                format!(
                    "The floor under the {} is at {:.2} and its area reaches {:.2} under to {:.2} over {:.2}.",
                    p.marker.kind.name(),
                    hit[2],
                    area.below,
                    area.above,
                    at[2],
                ),
            )),
            None => out.push(finding(
                "markers.area.no_floor",
                Severity::Warning,
                &p.id,
                format!("Nothing is under the {}: its area holds no floor.", p.marker.kind.name()),
            )),
        }
    }
    for (i, (a, area_a)) in zones.iter().enumerate() {
        for (b, area_b) in &zones[i + 1..] {
            if a.marker.kind.name() != b.marker.kind.name()
                || !(wanted(&a.id) || wanted(&b.id))
                || a.id == b.id
            {
                continue;
            }
            if overlap((a.place.at, area_a), (b.place.at, area_b)) {
                out.push(finding(
                    "markers.area.overlap",
                    Severity::Warning,
                    &format!("{} and {}", a.id, b.id),
                    format!(
                        "The areas of the {} at {} and at {} share ground, so a player stands in both.",
                        a.marker.kind.name(),
                        a.id,
                        b.id
                    ),
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_circles_overlap_when_they_share_ground_and_height() {
        let (a, b) = (Area::radius(3.0), Area::radius(3.0));
        assert!(overlap(([0.0; 3], &a), ([4.0, 0.0, 0.0], &b)));
        assert!(!overlap(([0.0; 3], &a), ([7.0, 0.0, 0.0], &b)));
        // Another floor: the heights do not meet.
        assert!(!overlap(([0.0; 3], &a), ([2.0, 0.0, 5.0], &b)));
        // One inside the other, with no outline point of the small one
        // outside.
        let big = Area::radius(10.0);
        assert!(overlap(([0.0; 3], &big), ([1.0, 0.0, 0.0], &a)));
    }

    #[test]
    fn an_l_does_not_overlap_a_circle_in_its_notch() {
        let l = Area::path(vec![
            [-6.0, -6.0],
            [6.0, -6.0],
            [6.0, 0.0],
            [0.0, 0.0],
            [0.0, 6.0],
            [-6.0, 6.0],
        ]);
        let small = Area::radius(1.0);
        assert!(!overlap(([0.0; 3], &l), ([3.5, 3.5, 0.0], &small)));
        assert!(overlap(([0.0; 3], &l), ([-3.0, 3.0, 0.0], &small)));
    }
}
