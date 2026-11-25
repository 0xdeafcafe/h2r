//! The playtest layers as viewport overlays (docs/editor/playtest.md,
//! "What is measured"; slice PT5): each measure of the shown result
//! ([`crate::app::playtest_shown::Shown`]) drawn over the map. A hidden
//! layer uploads nothing, a measure refused for too few events draws
//! nothing and says so ([`refused`]), and a stale result keeps its marks
//! dimmed. Shared by the window and `view.snapshot`.
//!
//! Places are the trace's: heat, traffic and the like are drawn at each
//! bin's mean place; the polygons of "never entered" and the unreached
//! islands are outlined on the bots' mesh, by the polygon index the
//! trace recorded.
use scenario_edit::geometry::{self, Vec3, add};
use scenario_edit::layers::{LayerId, PlaytestLayer as L};
use scenario_edit::nav_view::{BOT_LIFT, BotNav};
use scenario_edit::render::{self, Viewport};
use sim_bench::spatial::compare::{Diff, Marks};
use sim_bench::spatial::{Bin, Measures};

use super::nav::{Lines, ring_lines};
use crate::app::App;
use crate::app::playtest_shown::Shown;
use crate::theme;

/// Flow lines drawn: the most crossed.
pub const FLOW_LINES: usize = 200;
/// The shade a stale result keeps.
const STALE: f32 = 0.4;
/// Marks stand this far over the floor they are on.
const LIFT: f32 = 0.1;

type Drawn = (Vec<render::Vertex>, Vec<render::Vertex>);

/// How many marks `layer` draws for `m`: 0 when its measure was refused.
pub fn count(layer: L, m: &Measures) -> usize {
    match layer {
        L::Kills => m.heat.as_ref().map_or(0, |h| h.kills.len()),
        L::Deaths => m.heat.as_ref().map_or(0, |h| h.deaths.len()),
        L::Spawns => m
            .spawns
            .as_ref()
            .map_or(0, |s| s.starts.len() + s.spawn_kills.len()),
        L::Traffic => m.flow.as_ref().map_or(0, |f| f.cells.len()),
        L::Flow => m
            .flow
            .as_ref()
            .map_or(0, |f| f.crossings.len().min(FLOW_LINES)),
        L::Unvisited => m
            .flow
            .as_ref()
            .map_or(0, |f| f.never.as_ref().map_or(0, Vec::len)),
        L::Sightlines => m
            .sight
            .as_ref()
            .map_or(0, |s| s.longest.len() + s.commonest.len()),
        L::WeaponUse => m.items.as_ref().map_or(0, |i| i.placements.len()),
        L::Stuck => m.stuck.as_ref().map_or(0, |s| {
            s.spots.len() + s.islands.as_ref().map_or(0, Vec::len)
        }),
    }
}

/// Why `layer` has nothing to draw: its measure's refusal.
pub fn refused(layer: L, m: &Measures) -> Option<String> {
    let e = match layer {
        L::Kills | L::Deaths => m.heat.as_ref().err(),
        L::Spawns => m.spawns.as_ref().err(),
        L::Traffic | L::Flow | L::Unvisited => m.flow.as_ref().err(),
        L::Sightlines => m.sight.as_ref().err(),
        L::WeaponUse => m.items.as_ref().err(),
        L::Stuck => m.stuck.as_ref().err(),
    };
    e.map(ToString::to_string)
}

/// A difference layer's marks; `None` for the sightlines, which have no
/// difference layer.
pub fn marks(layer: L, d: &Diff) -> Option<&Marks> {
    Some(match layer {
        L::Kills => &d.kills,
        L::Deaths => &d.deaths,
        L::Traffic => &d.traffic,
        L::Flow => &d.flow,
        L::Unvisited => &d.unvisited,
        L::Spawns => &d.spawns,
        L::WeaponUse => &d.weapon_use,
        L::Stuck => &d.stuck,
        L::Sightlines => return None,
    })
}

/// How many marks `layer` of the shown result draws: its difference's when
/// one is drawn, else its measure's.
pub fn count_shown(layer: L, shown: &Shown) -> usize {
    match shown.diff() {
        Some(c) => marks(layer, &c.comparison.diff)
            .and_then(|m| m.as_ref().ok())
            .map_or(0, Vec::len),
        None => shown.measures.as_ref().map_or(0, |m| count(layer, m)),
    }
}

/// Why `layer` of the shown result has nothing to draw.
pub fn refused_shown(layer: L, shown: &Shown) -> Option<String> {
    if let Some(c) = shown.diff() {
        return match marks(layer, &c.comparison.diff) {
            None => Some("the sightlines have no difference layer".into()),
            Some(m) => m.as_ref().err().cloned(),
        };
    }
    shown.measures.as_ref().and_then(|m| refused(layer, m))
}

fn dim(color: [f32; 3], stale: bool) -> [f32; 3] {
    if stale {
        color.map(|c| c * STALE)
    } else {
        color
    }
}

fn lift(p: Vec3) -> Vec3 {
    add(p, [0.0, 0.0, LIFT])
}

/// `t` of the way from `a` to `b`.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// `n` over `max`, widened so a few marks differ: 0 to 1.
fn share(n: f32, max: f32) -> f32 {
    if max <= 0.0 {
        0.0
    } else {
        (n / max).clamp(0.0, 1.0).sqrt()
    }
}

/// A flat square of half-side `half` centred on `at`.
fn square(at: Vec3, half: f32, color: [f32; 3]) -> Vec<render::Vertex> {
    let [x, y, z] = lift(at);
    let c = |dx: f32, dy: f32| [x + dx, y + dy, z];
    let (a, b, d, e) = (
        c(-half, -half),
        c(half, -half),
        c(half, half),
        c(-half, half),
    );
    let n = [0.0, 0.0, 1.0];
    render::overlay_vertices(
        &[
            geometry::Tri { p: [a, b, d], n },
            geometry::Tri { p: [a, d, e], n },
        ],
        color,
    )
}

/// A post from the floor up `height`.
fn post(lines: &mut Lines, at: Vec3, height: f32, color: [f32; 3]) {
    let a = lift(at);
    lines.segment(a, add(a, [0.0, 0.0, height]), color);
}

/// A diamond round `at`.
fn diamond(lines: &mut Lines, at: Vec3, r: f32, color: [f32; 3]) {
    let a = lift(at);
    ring_lines(
        lines,
        &[
            add(a, [r, 0.0, 0.0]),
            add(a, [0.0, r, 0.0]),
            add(a, [-r, 0.0, 0.0]),
            add(a, [0.0, -r, 0.0]),
        ],
        color,
    );
}

/// A cross on the floor at `at`.
fn cross(lines: &mut Lines, at: Vec3, r: f32, color: [f32; 3]) {
    let a = lift(at);
    lines.segment(add(a, [-r, -r, 0.0]), add(a, [r, r, 0.0]), color);
    lines.segment(add(a, [-r, r, 0.0]), add(a, [r, -r, 0.0]), color);
}

/// A square and a post a bin: the more counted, the larger and brighter.
fn heat(bins: &[Bin], base: [f32; 3], stale: bool) -> Drawn {
    let max = bins.iter().map(|b| b.count).max().unwrap_or(0) as f32;
    let (mut tris, mut lines) = (Vec::new(), Lines(Vec::new()));
    for b in bins {
        let t = share(b.count as f32, max);
        let color = dim(base.map(|c| c * (0.45 + 0.55 * t)), stale);
        tris.extend(square(b.at, 0.3 + 0.9 * t, color));
        post(&mut lines, b.at, 0.3 + 1.5 * t, color);
    }
    (tris, lines.0)
}

/// The mesh polygon `i`, lifted.
fn poly_ring(nav: &BotNav, i: u32) -> Option<Vec<Vec3>> {
    let p = nav.polys.get(i as usize)?;
    Some(scenario_edit::nav_view::lifted(
        &p.verts,
        p.normal,
        BOT_LIFT * 3.0,
    ))
}

/// What `layer` of `m` draws; `nav` is the bots' mesh, for the layers that
/// outline polygons.
pub fn build(layer: L, m: &Measures, nav: Option<&BotNav>, stale: bool) -> Drawn {
    let base = theme::playtest_color(layer);
    let mut lines = Lines(Vec::new());
    match layer {
        L::Kills | L::Deaths => {
            let Ok(h) = &m.heat else {
                return Drawn::default();
            };
            let bins = if layer == L::Kills {
                &h.kills
            } else {
                &h.deaths
            };
            return heat(bins, base, stale);
        }
        L::Traffic => {
            let Ok(f) = &m.flow else {
                return Drawn::default();
            };
            let max = f.cells.iter().map(|c| c.seconds).fold(0.0, f32::max);
            let mut tris = Vec::new();
            for c in &f.cells {
                let t = share(c.seconds, max);
                tris.extend(square(
                    c.at,
                    0.5 + 0.7 * t,
                    dim(mix([0.1, 0.25, 0.5], base, t), stale),
                ));
            }
            return (tris, Vec::new());
        }
        L::Flow => {
            let Ok(f) = &m.flow else {
                return Drawn::default();
            };
            let max = f.crossings.iter().map(|c| c.count).max().unwrap_or(0) as f32;
            for c in f.crossings.iter().take(FLOW_LINES) {
                let t = share(c.count as f32, max);
                lines.arrow(
                    lift(c.a),
                    lift(c.b),
                    dim(base.map(|v| v * (0.35 + 0.65 * t)), stale),
                );
            }
        }
        L::Unvisited => {
            // Each polygon outlined and hatched from its first corner.
            let (Ok(f), Some(nav)) = (&m.flow, nav) else {
                return Drawn::default();
            };
            for ring in f.never.iter().flatten().filter_map(|&i| poly_ring(nav, i)) {
                ring_lines(&mut lines, &ring, dim(base, stale));
                for k in 2..ring.len().saturating_sub(1) {
                    lines.segment(ring[0], ring[k], dim(base, stale));
                }
            }
        }
        L::Spawns => {
            let Ok(s) = &m.spawns else {
                return Drawn::default();
            };
            for row in &s.starts {
                // Green for a spawn nobody died or was seen at; red when a
                // spawn kill or an enemy in sight was the rule.
                let unfair = if row.uses == 0 {
                    None
                } else {
                    let died = row.spawn_deaths as f32 / row.uses as f32;
                    Some(died.max(row.seen_share.unwrap_or(0.0)).clamp(0.0, 1.0))
                };
                let color = match unfair {
                    Some(u) => mix(base, [1.0, 0.2, 0.15], u),
                    None => [0.5, 0.5, 0.5],
                };
                let color = dim(color, stale);
                diamond(&mut lines, row.at, 0.7, color);
                post(&mut lines, row.at, 1.0, color);
            }
            for &at in &s.spawn_kills {
                cross(&mut lines, at, 0.3, dim([1.0, 0.15, 0.15], stale));
            }
        }
        L::Sightlines => {
            let Ok(s) = &m.sight else {
                return Drawn::default();
            };
            for l in &s.longest {
                lines.segment(lift(l.a), lift(l.b), dim(base, stale));
            }
            for l in &s.commonest {
                lines.segment(lift(l.a), lift(l.b), dim([1.0, 1.0, 1.0], stale));
            }
        }
        L::WeaponUse => {
            let Ok(i) = &m.items else {
                return Drawn::default();
            };
            let max = i
                .placements
                .iter()
                .map(|p| p.per_minute)
                .fold(0.0, f32::max);
            for p in &i.placements {
                if p.never {
                    // Never picked: ringed red, crossed out.
                    let red = dim([1.0, 0.2, 0.2], stale);
                    diamond(&mut lines, p.at, 0.8, red);
                    cross(&mut lines, p.at, 0.4, red);
                } else {
                    diamond(
                        &mut lines,
                        p.at,
                        0.3 + 0.9 * share(p.per_minute, max),
                        dim(base, stale),
                    );
                }
            }
        }
        L::Stuck => {
            let Ok(s) = &m.stuck else {
                return Drawn::default();
            };
            let max = s.spots.iter().map(|p| p.ticks).max().unwrap_or(0) as f32;
            for p in &s.spots {
                let t = share(p.ticks as f32, max);
                let color = dim(base, stale);
                cross(&mut lines, p.at, 0.2 + 0.5 * t, color);
                post(&mut lines, p.at, 0.5 + 1.0 * t, color);
                if let Some(to) = p.toward {
                    lines.arrow(lift(p.at), lift(to), color);
                }
            }
            if let Some(nav) = nav {
                let color = dim([1.0, 0.25, 0.25], stale);
                for island in s.islands.iter().flatten() {
                    for ring in island.polys.iter().filter_map(|&i| poly_ring(nav, i)) {
                        ring_lines(&mut lines, &ring, color);
                    }
                }
            }
        }
    }
    (Vec::new(), lines.0)
}

/// More here than before, and less.
const MORE: [f32; 3] = [1.0, 0.3, 0.2];
const LESS: [f32; 3] = [0.2, 0.5, 1.0];

/// What the difference layer `layer` draws: a mark where the count moved,
/// red for more and blue for less, larger the more it moved; a polygon
/// opened (never entered before) is green and one closed is red.
pub fn build_diff(layer: L, d: &Diff, nav: Option<&BotNav>, stale: bool) -> Drawn {
    let Some(Ok(deltas)) = marks(layer, d) else {
        return Drawn::default();
    };
    let max = deltas.iter().map(|m| m.change().abs()).fold(0.0, f32::max);
    let (mut tris, mut lines) = (Vec::new(), Lines(Vec::new()));
    for m in deltas {
        let t = share(m.change().abs(), max);
        let color = dim(if m.change() > 0.0 { MORE } else { LESS }, stale);
        match layer {
            L::Kills | L::Deaths | L::Traffic => {
                tris.extend(square(m.at, 0.3 + 0.9 * t, color));
                post(&mut lines, m.at, 0.3 + 1.5 * t, color);
            }
            L::Flow => {
                if let Some(to) = m.to {
                    lines.arrow(lift(m.at), lift(to), color);
                }
            }
            L::Unvisited => {
                let (Some(nav), Some(i)) = (nav, m.poly) else {
                    continue;
                };
                // Opened is good news, closed is bad.
                let color = dim(if m.a > m.b { [0.2, 0.9, 0.3] } else { MORE }, stale);
                if let Some(ring) = poly_ring(nav, i) {
                    ring_lines(&mut lines, &ring, color);
                }
            }
            L::Spawns | L::WeaponUse | L::Stuck => {
                diamond(&mut lines, m.at, 0.4 + 0.8 * t, color);
                post(&mut lines, m.at, 0.5 + 1.0 * t, color);
            }
            L::Sightlines => {}
        }
    }
    (tris, lines.0)
}

/// Upload the playtest overlays (empty when hidden, with no result, or
/// refused).
pub(crate) fn upload(vp: &mut Viewport, device: &wgpu::Device, app: &App) {
    let nav = app.session.bot_nav().map(|n| &**n);
    for layer in L::ALL {
        let id = LayerId::Playtest(layer);
        let shown = &app.playtest;
        let (tris, lines) = match (shown.diff(), shown.measures.as_ref()) {
            (Some(c), _) if app.layers.visible(id) => {
                build_diff(layer, &c.comparison.diff, nav, shown.stale)
            }
            (None, Some(m)) if app.layers.visible(id) => build(layer, m, nav, shown.stale),
            _ => Drawn::default(),
        };
        vp.set_layer_overlay(device, id, &tris, &lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenario_edit::nav_view::BotPoly;
    use sim_bench::spatial::compare::Delta;
    use sim_bench::spatial::{
        Cell, TooFew,
        flow::{Crossing, Dwell, Flow},
        heat::Heat,
        items::{Items, Placement},
        sight::{Line, Sight},
        spawns::{Spawns, StartRow},
        stuck::{Island, Spot, Stuck},
    };

    fn bin(x: f32, count: u32) -> Bin {
        Bin {
            cell: Cell::Grid(x as i32, 0),
            at: [x, 0.0, 0.0],
            count,
        }
    }

    fn few(measure: &'static str) -> TooFew {
        TooFew {
            measure,
            what: "events",
            have: 1,
            need: 10,
        }
    }

    fn quad(x: f32) -> BotPoly {
        let verts = vec![
            [x, 0.0, 0.0],
            [x + 1.0, 0.0, 0.0],
            [x + 1.0, 1.0, 0.0],
            [x, 1.0, 0.0],
        ];
        BotPoly {
            bsp: 0,
            poly: 0,
            normal: [0.0, 0.0, 1.0],
            verts,
            out_of_map: false,
        }
    }

    fn nav() -> BotNav {
        BotNav {
            polys: vec![quad(0.0), quad(2.0), quad(4.0)],
            ..BotNav::default()
        }
    }

    /// Every measure with a few invented marks.
    fn measures() -> Measures {
        let row = |start, uses, deaths, seen| StartRow {
            start: Some(start),
            at: [f32::from(start), 5.0, 0.0],
            uses,
            mean_life_s: None,
            spawn_deaths: deaths,
            mean_in_sight: None,
            seen_share: seen,
            mean_nearest_enemy: None,
            mean_first_engagement_s: None,
        };
        let line = |x: f32| Line {
            a: [x, 0.0, 0.0],
            b: [x, 9.0, 0.0],
            distance: 9.0,
            count: 2,
            kills: 1,
        };
        let place = |item, never| Placement {
            item,
            at: [f32::from(item), -3.0, 0.0],
            respawn_s: None,
            pickups: 4,
            per_minute: 1.5,
            first_pickup_s: Some(10.0),
            kills: 1,
            never,
        };
        Measures {
            traces: 8,
            seconds: 960.0,
            heat: Ok(Heat {
                kills: vec![bin(1.0, 9), bin(2.0, 1)],
                deaths: vec![bin(3.0, 4)],
                ..Heat::default()
            }),
            spawns: Ok(Spawns {
                spawns: 20,
                starts: vec![
                    row(0, 10, 0, Some(0.0)),
                    row(1, 10, 6, Some(0.8)),
                    row(2, 0, 0, None),
                ],
                spawn_kills: vec![[1.0, 1.0, 0.0], [2.0, 2.0, 0.0]],
                opening: Default::default(),
            }),
            flow: Ok(Flow {
                samples: 500,
                cells: vec![
                    Dwell {
                        cell: Cell::Poly(0),
                        at: [0.5, 0.5, 0.0],
                        seconds: 40.0,
                    },
                    Dwell {
                        cell: Cell::Poly(1),
                        at: [2.5, 0.5, 0.0],
                        seconds: 10.0,
                    },
                ],
                crossings: vec![Crossing {
                    from: Cell::Poly(0),
                    to: Cell::Poly(1),
                    a: [0.5, 0.5, 0.0],
                    b: [2.5, 0.5, 0.0],
                    count: 7,
                }],
                never: Some(vec![2, 9]),
            }),
            sight: Ok(Sight {
                segments: 30,
                median: 12.0,
                longest: vec![line(0.0)],
                commonest: vec![line(1.0), line(2.0)],
                by_weapon: Default::default(),
            }),
            items: Ok(Items {
                pickups: 8,
                loose: 0,
                placements: vec![place(0, false), place(1, true)],
            }),
            stuck: Ok(Stuck {
                samples: 400,
                spots: vec![Spot {
                    cell: Cell::Poly(0),
                    at: [1.0, 1.0, 0.0],
                    spells: 3,
                    ticks: 90,
                    repaths: 2,
                    toward: Some([3.0, 1.0, 0.0]),
                }],
                islands: Some(vec![Island {
                    polys: vec![1, 2],
                    at: [3.0, 0.5, 0.0],
                }]),
            }),
        }
    }

    /// Each layer counts its marks and draws the geometry they make:
    /// squares are two triangles (six vertices) and a post a segment.
    #[test]
    fn each_layer_counts_its_marks_and_draws_them() {
        let m = measures();
        let nav = nav();
        let sizes = |l| {
            let (t, li) = build(l, &m, Some(&nav), false);
            (t.len(), li.len())
        };
        assert_eq!((count(L::Kills, &m), sizes(L::Kills)), (2, (12, 4)));
        assert_eq!((count(L::Deaths, &m), sizes(L::Deaths)), (1, (6, 2)));
        // Three diamonds (4 sides) and posts, and two crosses (2 segments).
        assert_eq!(
            (count(L::Spawns, &m), sizes(L::Spawns)),
            (5, (0, 2 * (3 * 5 + 2 * 2)))
        );
        assert_eq!((count(L::Traffic, &m), sizes(L::Traffic)), (2, (12, 0)));
        // One arrow: its line and two head strokes.
        assert_eq!((count(L::Flow, &m), sizes(L::Flow)), (1, (0, 2 * 3)));
        // Polygon 2 outlined (4 sides) with one diagonal; 9 is not on the mesh.
        assert_eq!(
            (count(L::Unvisited, &m), sizes(L::Unvisited)),
            (2, (0, 2 * 5))
        );
        assert_eq!(
            (count(L::Sightlines, &m), sizes(L::Sightlines)),
            (3, (0, 2 * 3))
        );
        // A used placement's diamond; a never-picked one's diamond and cross.
        assert_eq!(
            (count(L::WeaponUse, &m), sizes(L::WeaponUse)),
            (2, (0, 2 * (4 + 4 + 2)))
        );
        // The spot's cross, post and arrow, and the island's two polygons.
        assert_eq!(
            (count(L::Stuck, &m), sizes(L::Stuck)),
            (2, (0, 2 * (2 + 1 + 3 + 8)))
        );
    }

    /// The hotter bin is the larger and brighter one.
    #[test]
    fn heat_marks_grow_and_brighten_with_their_count() {
        let (tris, _) = build(L::Kills, &measures(), None, false);
        let (hot, cool) = (&tris[..6], &tris[6..]);
        let width = |v: &[render::Vertex]| {
            let xs: Vec<f32> = v.iter().map(|v| v.position[0]).collect();
            xs.iter().cloned().fold(f32::MIN, f32::max)
                - xs.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(width(hot) > width(cool));
        assert!(hot[0].color[0] > cool[0].color[0]);
    }

    /// A stale result keeps every mark, dimmed.
    #[test]
    fn a_stale_result_keeps_its_marks_dimmed() {
        let m = measures();
        for layer in L::ALL {
            let (t, l) = build(layer, &m, Some(&nav()), false);
            let (ts, ls) = build(layer, &m, Some(&nav()), true);
            assert_eq!((t.len(), l.len()), (ts.len(), ls.len()), "{layer:?}");
            let bright =
                |v: &[render::Vertex]| v.iter().map(|v| v.color.iter().sum::<f32>()).sum::<f32>();
            assert!(
                bright(&ts) + bright(&ls) < bright(&t) + bright(&l),
                "{layer:?}"
            );
        }
    }

    /// A measure with too few events draws nothing, counts none and says why.
    #[test]
    fn a_refused_measure_draws_nothing_and_says_why() {
        let mut m = measures();
        m.heat = Err(few("heat"));
        m.stuck = Err(few("stuck"));
        for layer in [L::Kills, L::Deaths, L::Stuck] {
            assert_eq!(count(layer, &m), 0);
            let (t, l) = build(layer, &m, Some(&nav()), false);
            assert!(t.is_empty() && l.is_empty(), "{layer:?}");
            assert!(refused(layer, &m).unwrap().contains("too few"), "{layer:?}");
        }
        assert_eq!(refused(L::Spawns, &m), None);
        assert!(count(L::Spawns, &m) > 0);
    }

    fn delta(x: f32, a: f32, b: f32) -> Delta {
        Delta {
            at: [x, 0.0, 0.0],
            to: Some([x, 5.0, 0.0]),
            poly: Some(x as u32),
            a,
            b,
        }
    }

    fn diff(marks: Vec<Delta>) -> Diff {
        let m = || Ok(marks.clone());
        Diff {
            a_traces: 4,
            b_traces: 4,
            kills: m(),
            deaths: m(),
            traffic: m(),
            flow: m(),
            unvisited: m(),
            spawns: m(),
            weapon_use: m(),
            stuck: Err("stuck: too few samples (1, at least 300 needed)".into()),
        }
    }

    /// A difference layer draws a mark for each place that moved, red for
    /// more and blue for less; a layer refused says why; the sightlines
    /// have none.
    #[test]
    fn a_difference_layer_marks_each_place_that_moved() {
        let d = diff(vec![delta(0.0, 1.0, 5.0), delta(1.0, 4.0, 2.0)]);
        let (tris, lines) = build_diff(L::Kills, &d, None, false);
        assert_eq!((tris.len(), lines.len()), (12, 4));
        assert!(tris[0].color[0] > tris[0].color[2], "more is red");
        assert!(tris[6].color[2] > tris[6].color[0], "less is blue");
        assert_eq!(build_diff(L::Flow, &d, None, false).1.len(), 2 * 2 * 3);
        // Polygon 0 opened (green) and 1 closed (red) outline their rings.
        let d = diff(vec![delta(0.0, 1.0, 0.0), delta(1.0, 0.0, 1.0)]);
        let (_, lines) = build_diff(L::Unvisited, &d, Some(&nav()), false);
        assert_eq!(lines.len(), 2 * 2 * 4);
        assert!(build_diff(L::Stuck, &d, None, false).1.is_empty());
        assert!(build_diff(L::Sightlines, &d, None, false).1.is_empty());
        assert!(marks(L::Sightlines, &d).is_none());
        assert!(marks(L::Stuck, &d).unwrap().is_err());
    }

    /// The layers that outline polygons need the mesh; without it they draw
    /// the rest.
    #[test]
    fn polygon_layers_need_the_bots_mesh() {
        let m = measures();
        assert!(build(L::Unvisited, &m, None, false).1.is_empty());
        let (_, lines) = build(L::Stuck, &m, None, false);
        assert_eq!(lines.len(), 2 * (2 + 1 + 3), "the spot but not the island");
    }
}
