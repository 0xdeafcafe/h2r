//! The layout census: `h2 firefight kit MAP --layout-report`.
use std::fmt::Write as _;
use std::io::Cursor;

use anyhow::{Context, Result, ensure};
use h1campaign::scenario::{STARTING_LOCATION, STARTING_LOCATIONS};
use map::Map;
use pathfind::geom::V3;
use scenario_edit::scene::metadata_block;

use super::{Census, from_bases, point};
use crate::ground::Labels;
use crate::{Config, Mode, bases, nav};

/// What a map has and what the layout of `base_name` (or `config.layout`)
/// makes of it, in text: every flag, start cluster and weapon pad with its
/// kind, team and index; then each drop's anchor, the point it wants, the
/// sector it snapped to and its distance to each base; each refusal by
/// name; and the cost table of the build. The numbers settle the layout's
/// `V` constants.
pub fn layout_report(base: &[u8], base_name: &str, config: &Config) -> Result<String> {
    let mut map = Map::from_reader(Cursor::new(base))?;
    ensure!(
        map.header().name.eq_ignore_ascii_case(base_name),
        "base name does not match the map header"
    );
    let scenario = map.scenario().context("missing scenario")?.id;
    let scnr = map.read_tag_bytes(scenario)?;
    let starts = metadata_block(&mut map, &scnr, STARTING_LOCATIONS as usize, STARTING_LOCATION)?
    .iter()
    .map(|r| bases::start_row(r))
    .collect();
    let flags = metadata_block(
        &mut map,
        &scnr,
        bases::NETGAME_FLAGS as usize,
        bases::NETGAME_FLAG,
    )?
    .iter()
    .map(|r| bases::flag_row(r))
    .collect();
    let census = Census::read(&mut map, &scnr, starts, flags);
    let mut out = format!("census of {base_name}\n{}", census.describe());

    let layout = config.layout.or_else(|| super::for_map(base_name));
    let config = Config {
        layout,
        ..*config
    };
    let Some(layout) = layout else {
        out.push_str("no authored layout for this map: the generator picks the drops\n");
        return Ok(out);
    };
    let _ = writeln!(
        out,
        "layout for {} (digest {}): home {} base, {} teams",
        layout.map,
        layout.digest(),
        layout.home.index(),
        if layout.two_teams { 2 } else { 1 }
    );

    match nav::prepare(&mut map, &config, &Labels::default(), nav::Lean::default()) {
        Err(e) => {
            let _ = writeln!(out, "no ground to place the drops on: {e:#}");
        }
        Ok(nav) => {
            let points: Vec<V3> = nav.bases.points.to_vec();
            let _ = writeln!(
                out,
                "bases from {:?}: home {}, other {}",
                nav.bases.source,
                point(points[0]),
                point(points[1])
            );
            let distances = |p: V3| {
                from_bases(&points, p)
                    .iter()
                    .enumerate()
                    .map(|(i, d)| format!("base {i} {d:.1}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            for s in &nav.snapped {
                let _ = writeln!(
                    out,
                    "drop '{}': anchor {} wants {}, snapped to sector {} at {} ({:.1} of {:.1} allowed); {}",
                    s.name,
                    point(s.anchor),
                    point(s.wanted),
                    s.sector,
                    point(s.centre),
                    crate::builtin::flat(s.wanted, s.centre),
                    s.radius,
                    distances(s.centre)
                );
            }
            for why in &nav.refused {
                let _ = writeln!(out, "refused: {why}");
            }
        }
    }
    match crate::build_with_classes(base, base_name, Mode::Kit, &config, &Labels::default(), None)
    {
        Ok(built) => {
            let _ = writeln!(out, "costs: {}", built.costs);
            if !built.fitted.is_empty() {
                let steps: Vec<String> = built.fitted.iter().map(ToString::to_string).collect();
                let _ = writeln!(out, "steps taken to fit: {}", steps.join(", "));
            }
        }
        Err(e) => {
            let _ = writeln!(out, "build refused: {e:#}");
        }
    }
    Ok(out)
}
