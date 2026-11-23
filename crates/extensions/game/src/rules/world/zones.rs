//! Zones on the host (docs/game/rules-world-api.md, "Zones"): what
//! `inside`, `player_of`, `ticks_held`, `on enter`, `on leave`, `on spawn`
//! and `on death` read in the running game.
//!
//! A zone is a labelled netgame flag ([`super::super::census::Table`]): the
//! host's census has no Forge document at a game's start, so every zone is
//! a flag. Its shape comes from the map's own flags:
//!
//! - The flags that share a label and a number (a King of the Hill hill's
//!   corners, `hill_corner` 0 to 7), when there are three or more, are one
//!   polygon: the convex hull of their positions across the floor, from
//!   [`BELOW`] under the lowest corner to [`ABOVE`] over the highest. Each
//!   corner names the same zone, and the first corner by handle reports its
//!   enters and leaves, so a hill reports each body once.
//! - Any other flag is an upright cylinder of [`RADIUS`] around it, from
//!   [`BELOW`] under it to [`ABOVE`] over it.
//!
//! The sizes are ours: retail's capture radius and height (Territories'
//! marker `T+0/+0x20/+0x40`) are not traced, so they are "Not verified".
//!
//! A body is a living player's unit ([`bodies`]): its roster slot, team,
//! unit datum and position. [`Zones::occupancy`] is who is in which zone
//! now; [`Seen`] is what the VM was last told, and [`events`] is the
//! difference, in the order the VM hears it: deaths, spawns, leaves, then
//! enters. A zone event the VM carries to the next tick
//! (`Fired::Carried`) is not marked as told, so it is reported again.

use std::collections::{BTreeMap, BTreeSet};

use engine::GuestMemory;
use forge_doc::BlockKind;

use crate::firefight::runtime::read_players;
use crate::rules::census::Table;

/// A flag's cylinder radius, in world units. Not verified.
pub const RADIUS: f32 = 1.0;
/// How far a zone reaches over its highest flag, in world units. Not
/// verified.
pub const ABOVE: f32 = 1.5;
/// How far a zone reaches under its lowest flag, in world units. Not
/// verified.
pub const BELOW: f32 = 0.5;
/// The roster slots a game has.
pub const SLOTS: u8 = 16;

/// A zone's shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// An upright cylinder of [`RADIUS`] around one flag.
    Cylinder { at: [f32; 3] },
    /// A convex polygon across the floor (counter-clockwise), between two
    /// heights.
    Polygon {
        corners: Vec<[f32; 2]>,
        low: f32,
        high: f32,
    },
}

impl Shape {
    /// Whether `p` is inside, edges included.
    pub fn contains(&self, p: [f32; 3]) -> bool {
        match self {
            Shape::Cylinder { at } => {
                let (dx, dy) = (p[0] - at[0], p[1] - at[1]);
                dx * dx + dy * dy <= RADIUS * RADIUS && p[2] >= at[2] - BELOW && p[2] <= at[2] + ABOVE
            }
            Shape::Polygon { corners, low, high } => {
                if p[2] < *low || p[2] > *high {
                    return false;
                }
                let n = corners.len();
                (0..n).all(|i| {
                    let (a, b) = (corners[i], corners[(i + 1) % n]);
                    cross(a, b, [p[0], p[1]]) >= 0.0
                })
            }
        }
    }
}

/// `(b - a) x (c - a)`: positive when `c` is left of `a -> b`.
fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// The convex hull of `points`, counter-clockwise (Andrew's monotone
/// chain). Fewer than three points, or all in a line, give fewer than
/// three corners.
pub fn hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> = points.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut lower: Vec<[f32; 2]> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<[f32; 2]> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// One zone: its shape and the handle that reports its enters and leaves.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub shape: Shape,
    pub reporter: u32,
}

/// Every flag's zone in a game, by handle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Zones {
    zones: BTreeMap<u32, Zone>,
}

/// A living player's unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub slot: u8,
    pub team: u8,
    /// The unit's salted datum: what `on enter` passes and `player_of`
    /// reads.
    pub unit: u32,
    pub at: [f32; 3],
}

/// The game's living players' units, from the roster.
pub fn bodies(m: &dyn GuestMemory) -> Vec<Body> {
    read_players(m)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(p, unit)| {
            Some(Body {
                slot: p.slot,
                team: p.team,
                unit: unit?,
                at: p.position.filter(|_| p.alive)?,
            })
        })
        .collect()
}

/// The roster's alive mask: bit `slot` for each listed player whose unit
/// lives, and the mask of the listed players.
pub fn alive_mask(m: &dyn GuestMemory) -> (u16, u16) {
    let mut alive = 0u16;
    let mut listed = 0u16;
    for (p, _) in read_players(m).unwrap_or_default() {
        if p.slot < SLOTS {
            listed |= 1 << p.slot;
            if p.alive {
                alive |= 1 << p.slot;
            }
        }
    }
    (alive, listed)
}

/// The player whose unit is `unit`, living or not: what `player_of` reads.
pub fn player_of(m: &dyn GuestMemory, unit: u32) -> Option<u8> {
    read_players(m)
        .unwrap_or_default()
        .into_iter()
        .find(|(_, u)| *u == Some(unit))
        .map(|(p, _)| p.slot)
}

impl Zones {
    /// The zones of `table`'s netgame flags, at `flags`' positions (by
    /// flag index, as `engine::objectives::netgame_flags` reads them). A
    /// flag with no position has no zone.
    pub fn new(table: &Table, flags: &[[f32; 3]]) -> Zones {
        let at = |index: u32| flags.get(index as usize).copied();
        let mut groups: BTreeMap<(&str, i32), Vec<(u32, [f32; 3])>> = BTreeMap::new();
        for t in table.things() {
            if t.block != BlockKind::NetgameFlags {
                continue;
            }
            if let (Some(n), Some(p)) = (t.number, at(t.index)) {
                groups.entry((t.text.as_str(), n)).or_default().push((t.handle(), p));
            }
        }
        let mut zones = BTreeMap::new();
        for t in table.things() {
            if t.block != BlockKind::NetgameFlags {
                continue;
            }
            let Some(p) = at(t.index) else { continue };
            let group = t.number.and_then(|n| groups.get(&(t.text.as_str(), n)));
            let polygon = group.filter(|g| g.len() >= 3).and_then(|g| {
                let corners = hull(&g.iter().map(|(_, q)| [q[0], q[1]]).collect::<Vec<_>>());
                (corners.len() >= 3).then(|| {
                    let low = g.iter().map(|(_, q)| q[2]).fold(f32::INFINITY, f32::min) - BELOW;
                    let high = g.iter().map(|(_, q)| q[2]).fold(f32::NEG_INFINITY, f32::max) + ABOVE;
                    let reporter = g.iter().map(|(h, _)| *h).min().unwrap_or(t.handle());
                    (Shape::Polygon { corners, low, high }, reporter)
                })
            });
            let (shape, reporter) = polygon.unwrap_or((Shape::Cylinder { at: p }, t.handle()));
            zones.insert(t.handle(), Zone { shape, reporter });
        }
        Zones { zones }
    }

    /// The zones of the loaded map's flags; none when they cannot be read.
    pub fn read(m: &dyn GuestMemory, table: &Table) -> Zones {
        let flags: Vec<[f32; 3]> = engine::objectives::netgame_flags(m)
            .unwrap_or_default()
            .into_iter()
            .map(|(at, _, _)| at)
            .collect();
        Zones::new(table, &flags)
    }

    pub fn get(&self, zone: u32) -> Option<&Zone> {
        self.zones.get(&zone)
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// Whether `at` is inside `zone`; a handle with no zone holds nothing.
    pub fn contains(&self, zone: u32, at: [f32; 3]) -> bool {
        self.zones.get(&zone).is_some_and(|z| z.shape.contains(at))
    }

    /// `ticks_held`'s zone test: the one team whose living players alone
    /// are inside `zone`; none when it is empty or contested.
    pub fn holder(&self, zone: u32, bodies: &[Body]) -> Option<u8> {
        let mut team = None;
        for b in bodies.iter().filter(|b| self.contains(zone, b.at)) {
            match team {
                None => team = Some(b.team),
                Some(t) if t == b.team => {}
                Some(_) => return None,
            }
        }
        team
    }

    /// Who is in which zone now, as `(zone, unit)` pairs: each zone that
    /// reports for itself and that `reports` passes (the script declares
    /// its label).
    pub fn occupancy(&self, bodies: &[Body], reports: impl Fn(u32) -> bool) -> BTreeSet<(u32, u32)> {
        let mut out = BTreeSet::new();
        for (&handle, z) in &self.zones {
            if z.reporter != handle || !reports(handle) {
                continue;
            }
            for b in bodies.iter().filter(|b| z.shape.contains(b.at)) {
                out.insert((handle, b.unit));
            }
        }
        out
    }
}

/// What the VM was last told: the living players (bit per slot) and who
/// is in which zone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub alive: u16,
    pub inside: BTreeSet<(u32, u32)>,
}

/// One change to tell the VM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Death(u8),
    Spawn(u8),
    Leave { zone: u32, unit: u32 },
    Enter { zone: u32, unit: u32 },
}

/// The changes since `seen`, in the order the VM hears them: deaths and
/// spawns by slot, then leaves and enters by zone and unit. `alive` and
/// `listed` are the roster's masks ([`alive_mask`]); a player that left
/// the roster has no death, only its bit cleared ([`Seen::told`]).
pub fn events(seen: &Seen, alive: u16, listed: u16, now: &BTreeSet<(u32, u32)>) -> Vec<Change> {
    let mut out = Vec::new();
    for slot in 0..SLOTS {
        let bit = 1u16 << slot;
        if seen.alive & bit != 0 && alive & bit == 0 && listed & bit != 0 {
            out.push(Change::Death(slot));
        }
    }
    for slot in 0..SLOTS {
        let bit = 1u16 << slot;
        if seen.alive & bit == 0 && alive & bit != 0 {
            out.push(Change::Spawn(slot));
        }
    }
    for &(zone, unit) in seen.inside.difference(now) {
        out.push(Change::Leave { zone, unit });
    }
    for &(zone, unit) in now.difference(&seen.inside) {
        out.push(Change::Enter { zone, unit });
    }
    out
}

impl Seen {
    /// Marks `change` as told.
    pub fn told(&mut self, change: Change) {
        match change {
            Change::Death(s) => self.alive &= !(1 << s),
            Change::Spawn(s) => self.alive |= 1 << s,
            Change::Leave { zone, unit } => {
                self.inside.remove(&(zone, unit));
            }
            Change::Enter { zone, unit } => {
                self.inside.insert((zone, unit));
            }
        }
    }

    /// Clears the bits of the players no longer on the roster.
    pub fn keep_listed(&mut self, listed: u16) {
        self.alive &= listed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::census::{self, Table};

    const SCNR: u32 = 0x1006_0000;
    const FLAGS_AT: u32 = SCNR + 0x400;

    /// A guest whose scenario holds `flags` (position, type, team).
    fn flag_map(flags: &[([f32; 3], u16, u16)]) -> engine::testing::FakeGuest {
        use engine::objectives::{FLAG_TEAM, FLAG_TYPE, NETGAME_FLAG_STRIDE, NETGAME_FLAGS};
        let mut m = engine::testing::FakeGuest::new();
        m.put_u32(engine::points::SCENARIO.base, SCNR);
        m.map(SCNR, 0x2000);
        m.put_u32(SCNR + NETGAME_FLAGS, flags.len() as u32)
            .put_u32(SCNR + NETGAME_FLAGS + 4, FLAGS_AT);
        for (i, (at, ty, team)) in flags.iter().enumerate() {
            let f = FLAGS_AT + i as u32 * NETGAME_FLAG_STRIDE;
            m.put_f32(f, at[0])
                .put_f32(f + 4, at[1])
                .put_f32(f + 8, at[2])
                .put_u16(f + FLAG_TYPE, *ty)
                .put_u16(f + FLAG_TEAM, *team);
        }
        m
    }

    fn table(m: &dyn GuestMemory) -> Table {
        let ir = ::rules::compile(
            "rules \"T\" 1\ncarrier slayer\nlabel territory: zones at least 1\n\non tick { }\n",
        )
        .unwrap();
        Table::new(&census::things(m).unwrap(), &ir)
    }

    fn body(slot: u8, team: u8, at: [f32; 3]) -> Body {
        Body {
            slot,
            team,
            unit: 0xe000_0000 | u32::from(slot),
            at,
        }
    }

    #[test]
    fn a_flag_is_a_cylinder_around_it() {
        let s = Shape::Cylinder { at: [10.0, 0.0, 2.0] };
        assert!(s.contains([10.0, 0.0, 2.0]));
        assert!(s.contains([10.0 + RADIUS, 0.0, 2.0]));
        assert!(s.contains([10.5, 0.5, 2.0 + ABOVE]));
        assert!(s.contains([10.0, 0.0, 2.0 - BELOW]));
        assert!(!s.contains([10.0 + RADIUS + 0.01, 0.0, 2.0]));
        assert!(!s.contains([10.0, 0.0, 2.0 + ABOVE + 0.01]));
        assert!(!s.contains([10.0, 0.0, 2.0 - BELOW - 0.01]));
    }

    #[test]
    fn the_hull_of_a_square_and_its_middle_is_the_square() {
        let h = hull(&[[0.0, 0.0], [4.0, 0.0], [2.0, 2.0], [4.0, 4.0], [0.0, 4.0]]);
        assert_eq!(h, vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]);
        // Too few, or in a line: no polygon.
        assert!(hull(&[[0.0, 0.0], [1.0, 1.0]]).len() < 3);
        assert!(hull(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]).len() < 3);
        let p = Shape::Polygon {
            corners: h,
            low: -1.0,
            high: 3.0,
        };
        assert!(p.contains([2.0, 2.0, 0.0]));
        assert!(p.contains([0.0, 0.0, 0.0]), "an edge is inside");
        assert!(!p.contains([5.0, 2.0, 0.0]));
        assert!(!p.contains([2.0, 2.0, 3.5]));
    }

    /// A hill's four corners make one polygon that the first corner
    /// reports; a territory is a cylinder; a flag of another hill is its
    /// own.
    #[test]
    fn a_hills_corners_make_one_zone() {
        use engine::objectives::kind;
        let m = flag_map(&[
            ([0.0, 0.0, 1.0], kind::HILL, 0),
            ([6.0, 0.0, 1.0], kind::HILL, 0),
            ([6.0, 6.0, 1.5], kind::HILL, 0),
            ([0.0, 6.0, 1.0], kind::HILL, 0),
            ([30.0, 30.0, 0.0], kind::TERRITORY, 0),
            ([50.0, 0.0, 0.0], kind::HILL + 1, 0),
        ]);
        let t = table(&m);
        let zones = Zones::read(&m, &t);
        let corners = t.labelled("hill_corner");
        assert_eq!(corners.len(), 5);
        let hill: Vec<u32> = corners[..4].iter().map(|p| p.object).collect();
        let first = *hill.iter().min().unwrap();
        for &h in &hill {
            let z = zones.get(h).unwrap();
            assert_eq!(z.reporter, first);
            assert!(matches!(z.shape, Shape::Polygon { .. }));
            assert!(zones.contains(h, [3.0, 3.0, 1.2]));
            assert!(!zones.contains(h, [7.0, 3.0, 1.2]));
            // From under the lowest corner to over the highest.
            assert!(zones.contains(h, [3.0, 3.0, 1.0 - BELOW]));
            assert!(zones.contains(h, [3.0, 3.0, 1.5 + ABOVE]));
            assert!(!zones.contains(h, [3.0, 3.0, 1.5 + ABOVE + 0.1]));
        }
        let lone = corners[4].object;
        assert_eq!(zones.get(lone).unwrap().reporter, lone);
        assert!(matches!(zones.get(lone).unwrap().shape, Shape::Cylinder { .. }));
        let site = t.labelled("territory")[0].object;
        assert!(zones.contains(site, [30.5, 30.0, 0.5]));
        assert!(!zones.contains(site, [32.0, 30.0, 0.5]));
        // A handle with no zone holds nothing.
        assert!(!zones.contains(0x4000_ffff, [0.0; 3]));

        // The hill reports once, by its first corner, for each body inside.
        let bodies = [body(0, 0, [3.0, 3.0, 1.0]), body(1, 1, [30.0, 30.0, 0.0])];
        let all = zones.occupancy(&bodies, |_| true);
        assert_eq!(
            all,
            BTreeSet::from([(first, bodies[0].unit), (site, bodies[1].unit)])
        );
        // A zone whose label the script does not declare reports nothing.
        let only_sites = zones.occupancy(&bodies, |h| h == site);
        assert_eq!(only_sites, BTreeSet::from([(site, bodies[1].unit)]));
    }

    #[test]
    fn a_zone_is_held_by_one_team_alone() {
        let m = flag_map(&[([0.0, 0.0, 0.0], engine::objectives::kind::TERRITORY, 0)]);
        let t = table(&m);
        let zones = Zones::read(&m, &t);
        let site = t.labelled("territory")[0].object;
        let a = body(0, 0, [0.0, 0.0, 0.0]);
        let b = body(1, 0, [0.5, 0.0, 0.0]);
        let c = body(2, 1, [0.0, 0.5, 0.0]);
        let away = body(3, 1, [9.0, 0.0, 0.0]);
        assert_eq!(zones.holder(site, &[]), None);
        assert_eq!(zones.holder(site, &[a, b, away]), Some(0));
        assert_eq!(zones.holder(site, &[a, b, c]), None, "contested");
        assert_eq!(zones.holder(site, &[c, away]), Some(1));
    }

    #[test]
    fn events_come_deaths_spawns_leaves_then_enters() {
        let mut seen = Seen::default();
        // Players 0 and 2 spawn; 0 stands in zone 7.
        let now = BTreeSet::from([(7, 100)]);
        let first = events(&seen, 0b101, 0b101, &now);
        assert_eq!(
            first,
            vec![
                Change::Spawn(0),
                Change::Spawn(2),
                Change::Enter { zone: 7, unit: 100 }
            ]
        );
        for c in first {
            seen.told(c);
        }
        assert!(events(&seen, 0b101, 0b101, &now).is_empty(), "nothing new");
        // Player 0 dies (its unit leaves zone 7); player 2 walks into 8.
        let now = BTreeSet::from([(8, 102)]);
        assert_eq!(
            events(&seen, 0b100, 0b101, &now),
            vec![
                Change::Death(0),
                Change::Leave { zone: 7, unit: 100 },
                Change::Enter { zone: 8, unit: 102 }
            ]
        );
        // A carried leave is not marked, so it comes again.
        seen.told(Change::Death(0));
        seen.told(Change::Enter { zone: 8, unit: 102 });
        assert_eq!(
            events(&seen, 0b100, 0b101, &now),
            vec![Change::Leave { zone: 7, unit: 100 }]
        );
        // A player that leaves the roster has no death.
        let mut seen = Seen {
            alive: 0b1,
            inside: BTreeSet::new(),
        };
        assert!(events(&seen, 0, 0, &BTreeSet::new()).is_empty());
        seen.keep_listed(0);
        assert_eq!(seen.alive, 0);
    }
}
