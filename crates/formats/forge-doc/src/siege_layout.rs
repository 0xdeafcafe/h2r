//! The Siege layout recipe (docs/game/rules-engine.md, "Siege layouts"): a
//! labels-only document for one map, in the form of the built-in layouts
//! (`rules::builtin::LAYOUTS`), scaffolded from the map's own rows.
//!
//! The attack line runs from the attackers' CTF flag stand to the
//! defenders'. The map's territory flags between the two, ordered by how
//! far along the line they are, are the sites: the nearer half of them
//! numbered 0, the farther half 1 (at most two a phase, a flag within
//! [`SAME_SITE`] of a kept one dropped; two sites make two phases of
//! one). The defenders' stand is the
//! `siege_goal`, so the final stand stays at the same end of the map when
//! the sides swap. Each phase's starting locations are labelled
//! `siege_attack` and `siege_defend` with the phase's number: the
//! attackers' the ones nearest their stand, then nearest the sites they
//! took, on their side of the phase's front; the defenders' the ones
//! nearest the phase's sites past them, then nearest their stand. A
//! barrier is named by its placement (`crates:22`) and the phase it opens
//! in: the recipe cannot judge which object blocks a way.
//!
//! `h2 siege layout <map>` reads a map with [`read`], plans with [`plan`]
//! and writes [`document`]; [`scenario`] is the fake-game walk through the
//! plan's phases that the layout's test runs (`rules/tests/modes.rs`).
use std::collections::BTreeMap;
use std::io::{Read, Seek};

use serde_json::{Value, json};

use crate::labels::{map_error, read_block, rows_of_file};
use crate::{BlockKind, Result};

/// A CTF flag stand's netgame flag type.
pub const FLAG_STAND: u16 = 0;
/// A territory's netgame flag type.
pub const TERRITORY: u16 = 10;
/// Two territory flags nearer than this (world units, across the floor)
/// are one site.
pub const SAME_SITE: f32 = 4.0;
/// How far along the line (0 at the attackers' stand, 1 at the
/// defenders') a site may be, at either end: a base's own territory is not
/// a site.
pub const SITE_SPAN: (f32, f32) = (0.12, 0.88);
/// The most sites a phase has.
pub const SITES_PER_PHASE: usize = 2;
/// The scenario's ticks between one phase's first enter and the next:
/// Siege's default capture time (15 s at 30 ticks a second) and a margin.
pub const SCENARIO_PHASE_TICKS: usize = 470;

/// One netgame flag of a map: where it is, its type and its team.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SiegeFlag {
    pub position: [f32; 3],
    pub kind: u16,
    pub team: u16,
}

/// What the recipe reads of a map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SiegeMap {
    pub flags: Vec<SiegeFlag>,
    pub starts: Vec<[f32; 3]>,
    /// Each block's count, so a barrier's index is checked.
    pub counts: BTreeMap<BlockKind, u32>,
}

/// An object that stands open from a phase on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Barrier {
    pub block: BlockKind,
    pub index: u32,
    pub phase: i32,
}

impl Barrier {
    /// `crates:22=2`: the block's name, the placement and the phase.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let bad = || format!("a barrier is <block>:<index>=<phase> (crates:22=2), not {text:?}");
        let (place, phase) = text.split_once('=').ok_or_else(bad)?;
        let (block, index) = place.split_once(':').ok_or_else(bad)?;
        let block = crate::SOLID_BLOCKS
            .iter()
            .copied()
            .find(|b| b.name() == block)
            .ok_or_else(|| format!("a barrier is scenery, a crate, a machine or a vehicle, not {block:?}"))?;
        Ok(Self {
            block,
            index: index.parse().map_err(|_| bad())?,
            phase: phase.parse().map_err(|_| bad())?,
        })
    }
}

/// What the recipe is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    /// The team whose stand the attackers start at: 0 (red) or 1 (blue).
    pub attack_team: u16,
    /// Starting locations per side and phase, at most.
    pub per_side: usize,
    pub barriers: Vec<Barrier>,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            attack_team: 0,
            per_side: 6,
            barriers: Vec::new(),
        }
    }
}

/// A planned layout: each placement's index with its phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Territory flags, with the phase each is a site of.
    pub sites: Vec<(u32, i32)>,
    /// The defenders' flag stand.
    pub goal: u32,
    /// Starting locations, with the phase each side spawns there.
    pub attack: Vec<(u32, i32)>,
    pub defend: Vec<(u32, i32)>,
    pub barriers: Vec<Barrier>,
    /// The phases of sites; the final assault is phase `phases`.
    pub phases: i32,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

fn centroid(points: &[[f32; 3]]) -> [f32; 3] {
    let n = points.len().max(1) as f32;
    let mut c = [0.0; 3];
    for p in points {
        for k in 0..3 {
            c[k] += p[k] / n;
        }
    }
    c
}

/// The layout the recipe makes on `map`.
pub fn plan(map: &SiegeMap, recipe: &Recipe) -> std::result::Result<Plan, String> {
    if recipe.attack_team > 1 {
        return Err("the attackers are team 0 (red) or 1 (blue)".into());
    }
    let stand = |team: u16| {
        map.flags
            .iter()
            .position(|f| f.kind == FLAG_STAND && f.team == team)
            .ok_or_else(|| format!("the map has no CTF flag stand for team {team}: Siege's line runs between the two"))
    };
    let from_i = stand(recipe.attack_team)?;
    let goal = stand(1 - recipe.attack_team)?;
    let from = map.flags[from_i].position;
    let to = map.flags[goal].position;
    let line = sub(to, from);
    let len2 = line[0] * line[0] + line[1] * line[1];
    if len2 < 1.0 {
        return Err("the two flag stands are at one place: there is no line to attack along".into());
    }
    let along = |p: [f32; 3]| {
        let d = sub(p, from);
        (d[0] * line[0] + d[1] * line[1]) / len2
    };
    let mut candidates: Vec<(u32, f32)> = map
        .flags
        .iter()
        .enumerate()
        .filter(|(_, f)| f.kind == TERRITORY)
        .map(|(i, f)| (i as u32, along(f.position)))
        .filter(|&(_, t)| (SITE_SPAN.0..=SITE_SPAN.1).contains(&t))
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut kept: Vec<(u32, f32)> = Vec::new();
    for c in candidates {
        let at = map.flags[c.0 as usize].position;
        if kept
            .iter()
            .all(|k| flat(map.flags[k.0 as usize].position, at) >= SAME_SITE)
        {
            kept.push(c);
        }
    }
    // Two phases whenever there are two sites: a layout whose sites are all
    // numbered 0 is not ordered (`siege.rules` reads it as unnumbered).
    let phases = kept.len().min(2);
    let first = kept.len().div_ceil(2).min(SITES_PER_PHASE);
    let groups: Vec<Vec<(u32, f32)>> = match phases {
        0 => Vec::new(),
        1 => vec![kept.clone()],
        _ => vec![
            kept[..first].to_vec(),
            kept[first..].iter().take(SITES_PER_PHASE).copied().collect(),
        ],
    };
    let at = |i: u32| map.flags[i as usize].position;
    let sites: Vec<(u32, i32)> = groups
        .iter()
        .enumerate()
        .flat_map(|(k, g)| g.iter().map(move |s| (s.0, k as i32)))
        .collect();
    let mut attack = Vec::new();
    let mut defend = Vec::new();
    let starts: Vec<(u32, [f32; 3], f32)> = map
        .starts
        .iter()
        .enumerate()
        .map(|(i, &p)| (i as u32, p, along(p)))
        .collect();
    let nearest = |point: [f32; 3], keep: &dyn Fn(f32) -> bool, taken: &[u32]| {
        let mut pool: Vec<&(u32, [f32; 3], f32)> = starts
            .iter()
            .filter(|s| keep(s.2) && !taken.contains(&s.0))
            .collect();
        pool.sort_by(|a, b| dist(a.1, point).total_cmp(&dist(b.1, point)).then(a.0.cmp(&b.0)));
        pool.into_iter()
            .take(recipe.per_side)
            .map(|s| s.0)
            .collect::<Vec<u32>>()
    };
    for k in 0..=phases {
        let (front, centre) = match groups.get(k) {
            Some(g) => {
                let points: Vec<[f32; 3]> = g.iter().map(|s| at(s.0)).collect();
                (g.iter().map(|s| s.1).fold(f32::MAX, f32::min), centroid(&points))
            }
            None => (1.0, to),
        };
        let back = if k == 0 {
            from
        } else {
            centroid(&groups[k - 1].iter().map(|s| at(s.0)).collect::<Vec<_>>())
        };
        let a = nearest(back, &|t| t < front - 0.05, &[]);
        let d = match groups.get(k) {
            Some(g) => {
                let far = g.iter().map(|s| s.1).fold(f32::MIN, f32::max);
                nearest(centre, &|t| t > far, &a)
            }
            None => nearest(to, &|_| true, &a),
        };
        if a.is_empty() || d.is_empty() {
            return Err(format!("phase {k} has no starting location for a side"));
        }
        attack.extend(a.into_iter().map(|i| (i, k as i32)));
        defend.extend(d.into_iter().map(|i| (i, k as i32)));
    }
    for b in &recipe.barriers {
        let count = map.counts.get(&b.block).copied().unwrap_or(0);
        if b.index >= count {
            return Err(format!("the map has {count} {} entries: no {}", b.block.name(), b.index));
        }
        if b.phase < 0 || b.phase > phases as i32 {
            return Err(format!("a barrier opens in a phase from 0 to {phases}, not {}", b.phase));
        }
    }
    Ok(Plan {
        sites,
        goal: goal as u32,
        attack,
        defend,
        barriers: recipe.barriers.clone(),
        phases: phases as i32,
    })
}

fn op(id: usize, block: &str, index: u32, text: &str, number: Option<i32>) -> Value {
    let label = match number {
        Some(n) => json!({"number": n, "text": text}),
        None => json!({"text": text}),
    };
    json!({
        "edit": {"forge": {"label": {"label": label, "target": {"base": {"block": block, "index": index}}}}},
        "id": id,
    })
}

/// The plan as a labels-only document of `map` (its short name), in the
/// built-in layouts' form.
pub fn document(map: &str, plan: &Plan, description: &str) -> Value {
    let flags = BlockKind::NetgameFlags.name();
    let starts = BlockKind::StartingLocations.name();
    let mut ops = Vec::new();
    for &(i, n) in &plan.sites {
        ops.push((flags, i, "siege_site", Some(n)));
    }
    ops.push((flags, plan.goal, "siege_goal", None));
    for &(i, n) in &plan.attack {
        ops.push((starts, i, "siege_attack", Some(n)));
    }
    for &(i, n) in &plan.defend {
        ops.push((starts, i, "siege_defend", Some(n)));
    }
    for b in &plan.barriers {
        ops.push((b.block.name(), b.index, "siege_barrier", Some(b.phase)));
    }
    let ops: Vec<Value> = ops
        .into_iter()
        .enumerate()
        .map(|(k, (block, i, text, n))| op(k + 1, block, i, text, n))
        .collect();
    json!({
        "author": "h2recomp",
        "base": {"map": map, "sha256": "0".repeat(64)},
        "builder": {"bots": 5, "forge_build": 1, "pathfind": 1},
        "description": description,
        "format": 2,
        "imports": [],
        "ops": ops,
    })
}

/// The document as the built-in layouts are written: one-space indent,
/// keys sorted, a final newline.
pub fn to_text(doc: &Value) -> String {
    let mut out = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
    serde::Serialize::serialize(doc, &mut ser).expect("a JSON value serialises");
    let mut text = String::from_utf8(out).expect("JSON is UTF-8");
    text.push('\n');
    text
}

/// The scenario object of a site or goal flag, a start and a barrier.
pub const SCENARIO_FLAG: u32 = 1000;
pub const SCENARIO_START: u32 = 2000;
pub const SCENARIO_OBJECT: u32 = 3000;
/// The attacker's unit in the scenario.
pub const SCENARIO_ATTACKER: u32 = 100;

/// A fake-game walk through the plan (`rules::scenario`): one attacker
/// stands on the first site of each phase in turn, for the capture time,
/// then on the goal; both players respawn as each phase opens.
pub fn scenario(plan: &Plan) -> Value {
    let mut map = Vec::new();
    for &(i, n) in &plan.sites {
        map.push(json!({"label": "siege_site", "object": SCENARIO_FLAG + i, "number": n, "class": "flag"}));
    }
    map.push(json!({"label": "siege_goal", "object": SCENARIO_FLAG + plan.goal, "class": "flag"}));
    for (text, list) in [("siege_attack", &plan.attack), ("siege_defend", &plan.defend)] {
        for &(i, n) in list {
            map.push(json!({"label": text, "object": SCENARIO_START + i, "number": n, "class": "spawn"}));
        }
    }
    for b in &plan.barriers {
        map.push(json!({"label": "siege_barrier", "object": SCENARIO_OBJECT + b.index, "number": b.phase, "class": "object"}));
    }
    let fronts: Vec<u32> = (0..plan.phases)
        .filter_map(|k| plan.sites.iter().find(|s| s.1 == k).map(|s| SCENARIO_FLAG + s.0))
        .chain([SCENARIO_FLAG + plan.goal])
        .collect();
    let mut ticks = vec![json!({}); SCENARIO_PHASE_TICKS * fronts.len() + 10];
    let mut last: Option<u32> = None;
    for (k, &zone) in fronts.iter().enumerate() {
        let mut events = Vec::new();
        if let Some(z) = last {
            events.push(json!({"leave": {"zone": z, "object": SCENARIO_ATTACKER}}));
        }
        events.push(json!({"spawn": 0}));
        events.push(json!({"spawn": 1}));
        events.push(json!({"enter": {"zone": zone, "object": SCENARIO_ATTACKER}}));
        ticks[k * SCENARIO_PHASE_TICKS] = json!({"events": events});
        last = Some(zone);
    }
    json!({
        "teams": [0, 1],
        "variant": {"team_play": 1},
        "players": [
            {"slot": 0, "team": 0, "unit": SCENARIO_ATTACKER},
            {"slot": 1, "team": 1, "unit": SCENARIO_ATTACKER + 1},
        ],
        "map": map,
        "ticks": ticks,
    })
}

fn f32_at(e: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]])
}

fn u16_at(e: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([e[at], e[at + 1]])
}

fn point(e: &[u8]) -> [f32; 3] {
    [f32_at(e, 0), f32_at(e, 4), f32_at(e, 8)]
}

/// The rows the recipe reads of a map file: each flag's position (`+0`),
/// type (`+0x10`) and team (`+0x12`), each start's position (`+0`), and
/// the block counts.
pub fn read<R: Read + Seek>(map: &mut map::Map<R>) -> Result<SiegeMap> {
    let counts = rows_of_file(map)?.counts;
    let scnr = map
        .scenario()
        .ok_or_else(|| map_error("the map has no scenario"))?;
    let (id, loc) = (scnr.id, scnr.location);
    let loc = loc.ok_or_else(|| map_error("the scenario has no payload"))?;
    let root = map.read_tag_bytes(id).map_err(map_error)?;
    let flags = read_block(map, &root, loc.space, BlockKind::NetgameFlags, crate::CENSUS_FLAG_LIMIT)?
        .iter()
        .map(|e| SiegeFlag {
            position: point(e),
            kind: u16_at(e, 0x10),
            team: u16_at(e, 0x12),
        })
        .collect();
    let starts = read_block(map, &root, loc.space, BlockKind::StartingLocations, crate::CENSUS_ENTRY_LIMIT)?
        .iter()
        .map(|e| point(e))
        .collect();
    Ok(SiegeMap { flags, starts, counts })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(x: f32, y: f32, kind: u16, team: u16) -> SiegeFlag {
        SiegeFlag {
            position: [x, y, 0.0],
            kind,
            team,
        }
    }

    /// A straight map: red's stand at x 0, blue's at x 100, five
    /// territories along it (one beside another, one in each base) and a
    /// start every ten units.
    fn line_map() -> SiegeMap {
        let flags = vec![
            flag(0.0, 0.0, FLAG_STAND, 0),
            flag(100.0, 0.0, FLAG_STAND, 1),
            flag(2.0, 5.0, TERRITORY, 0),
            flag(30.0, 0.0, TERRITORY, 0),
            flag(31.0, 1.0, TERRITORY, 0),
            flag(40.0, 10.0, TERRITORY, 0),
            flag(60.0, 0.0, TERRITORY, 0),
            flag(70.0, -10.0, TERRITORY, 0),
            flag(98.0, 5.0, TERRITORY, 0),
        ];
        let starts = (0..=20).map(|i| [i as f32 * 5.0, 3.0, 0.0]).collect();
        let counts = [(BlockKind::Crates, 4)].into();
        SiegeMap { flags, starts, counts }
    }

    #[test]
    fn the_sites_are_the_territories_between_the_stands_in_order_two_a_phase() {
        let plan = plan(&line_map(), &Recipe::default()).unwrap();
        // The base territories (2, 8) and the one beside 3 (4) are not sites.
        assert_eq!(plan.sites, [(3, 0), (5, 0), (6, 1), (7, 1)]);
        assert_eq!(plan.goal, 1);
        assert_eq!(plan.phases, 2);
    }

    #[test]
    fn each_side_spawns_behind_its_front_in_each_phase() {
        let plan = plan(&line_map(), &Recipe { per_side: 2, ..Recipe::default() }).unwrap();
        // Starts are 5 units apart: start i is at x 5i.
        let of = |list: &[(u32, i32)], k: i32| {
            let mut v: Vec<u32> = list.iter().filter(|s| s.1 == k).map(|s| s.0).collect();
            v.sort();
            v
        };
        assert_eq!(of(&plan.attack, 0), [0, 1], "at the attackers' stand");
        assert_eq!(of(&plan.defend, 0), [9, 10], "past the first sites (x 30 and 40)");
        assert_eq!(of(&plan.attack, 1), [6, 7], "at the sites taken, short of the next (x 60)");
        assert_eq!(of(&plan.defend, 1), [15, 16], "past the second sites (x 70)");
        assert_eq!(of(&plan.attack, 2), [12, 13], "at the second sites, short of the stand");
        assert_eq!(of(&plan.defend, 2), [19, 20], "at the defenders' stand");
        for k in 0..=2 {
            assert!(of(&plan.attack, k).iter().all(|a| !of(&plan.defend, k).contains(a)));
        }
    }

    #[test]
    fn the_attackers_may_start_at_blue() {
        let plan = plan(&line_map(), &Recipe { attack_team: 1, ..Recipe::default() }).unwrap();
        assert_eq!(plan.goal, 0);
        assert_eq!(plan.sites, [(7, 0), (6, 0), (5, 1), (4, 1)]);
    }

    #[test]
    fn a_map_without_two_stands_or_a_bad_barrier_is_refused() {
        let mut map = line_map();
        map.flags.remove(1);
        assert!(plan(&map, &Recipe::default()).unwrap_err().contains("no CTF flag stand for team 1"));
        let barrier = |s: &str| Recipe {
            barriers: vec![Barrier::parse(s).unwrap()],
            ..Recipe::default()
        };
        assert!(plan(&line_map(), &barrier("crates:4=1")).unwrap_err().contains("4 crates entries"));
        assert!(plan(&line_map(), &barrier("crates:3=3")).unwrap_err().contains("from 0 to 2"));
        assert!(Barrier::parse("netgame_flags:1=1").is_err());
        assert!(Barrier::parse("crates:1").is_err());
        assert_eq!(plan(&line_map(), &barrier("crates:3=2")).unwrap().barriers[0].phase, 2);
    }

    /// The document reads as a Forge document whose labels are the plan's,
    /// in the built-in layouts' form.
    #[test]
    fn the_document_is_a_layout_of_the_plans_labels() {
        let recipe = Recipe {
            barriers: vec![Barrier::parse("crates:2=2").unwrap()],
            ..Recipe::default()
        };
        let plan = plan(&line_map(), &recipe).unwrap();
        let doc = document("line", &plan, "test");
        let text = to_text(&doc);
        let read = crate::ForgeDoc::from_bytes(text.as_bytes(), &crate::Limits::default()).unwrap();
        assert_eq!(read.ops.len(), 4 + 1 + plan.attack.len() + plan.defend.len() + 1);
        let counts = [
            (BlockKind::NetgameFlags, 9),
            (BlockKind::StartingLocations, 21),
            (BlockKind::Crates, 4),
        ]
        .into();
        let labels = read.labels(&counts).unwrap();
        let find = |block: BlockKind, i: u32| {
            labels.active.iter().find(|l| l.block == block && l.index == i).map(|l| l.label.clone())
        };
        assert_eq!(find(BlockKind::NetgameFlags, 1).unwrap().text, "siege_goal");
        assert_eq!(find(BlockKind::NetgameFlags, 6).unwrap().number, Some(1));
        assert_eq!(find(BlockKind::Crates, 2).unwrap().text, "siege_barrier");
        assert_eq!(find(BlockKind::StartingLocations, 0).unwrap().text, "siege_attack");
    }

    /// The scenario stands on one site a phase, then the goal.
    #[test]
    fn the_scenario_walks_one_site_a_phase_then_the_goal() {
        let plan = plan(&line_map(), &Recipe::default()).unwrap();
        let s = scenario(&plan);
        let ticks = s["ticks"].as_array().unwrap();
        assert_eq!(ticks.len(), SCENARIO_PHASE_TICKS * 3 + 10);
        let entered: Vec<u64> = ticks
            .iter()
            .flat_map(|t| t["events"].as_array().cloned().unwrap_or_default())
            .filter_map(|e| e["enter"]["zone"].as_u64())
            .collect();
        let f = u64::from(SCENARIO_FLAG);
        assert_eq!(entered, [f + 3, f + 6, f + 1]);
    }
}
