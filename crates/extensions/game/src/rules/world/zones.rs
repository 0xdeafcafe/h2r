//! Zones on the host (docs/game/rules-world-api.md, "Zones"): what
//! `inside`, `player_of`, `ticks_held`, `on enter`, `on leave`, `on spawn`
//! and `on death` read in the running game.
//!
//! A zone is a labelled netgame flag ([`super::super::census::Table`]): the
//! host's census has no trigger volumes, so every zone is a flag. Its
//! shape is an [`Area`] round the flag, from the first of these that
//! applies (docs/game/gametype-markers.md, "In a script"):
//!
//! 1. The flag's own area, when the census has one (the Forge document's
//!    area on the flag, `Thing::area`): a radius, a box, a sphere or a
//!    path, at its own heights. A box turns with the flag's facing, as
//!    Forge and Studio draw it (`Area::contains_turned`). The zone's
//!    [`Source`] is [`Source::Area`].
//! 2. The flags that share a label and a number (a King of the Hill hill's
//!    corners, `hill_corner` 0 to 7), when there are three or more: the
//!    area [`Area::of_corners`] makes of them, the convex hull of their
//!    positions across the floor, from [`BELOW`] under the lowest corner to
//!    [`ABOVE`] over the highest. Each corner names the same zone, and the
//!    first corner by handle reports its enters and leaves, so a hill
//!    reports each body once. [`Source::Corners`].
//! 3. Any other flag: the default area of its label's marker round it
//!    ([`default_area`]): the capture plate of a flag return, a bomb goal
//!    or a siege goal (`Area::plate`), so the plate drawn is the plate
//!    scored; else [`Area::default`], an upright cylinder of [`RADIUS`]
//!    from [`BELOW`] under it to [`ABOVE`] over it. [`Source::Default`].
//!
//! The sizes of the default are ours: retail's capture radius and height
//! (Territories' marker `T+0/+0x20/+0x40`) are not traced, so they are
//! "Not verified".
//!
//! A body is a living player's unit ([`bodies`]): its roster slot, team,
//! unit datum and position. [`Zones::occupancy`] is who is in which zone
//! now; [`Seen`] is what the VM was last told, and [`events`] is the
//! difference, in the order the VM hears it: deaths, spawns, leaves, then
//! enters. A zone event the VM carries to the next tick
//! (`Fired::Carried`) is not marked as told, so it is reported again.

use std::collections::{BTreeMap, BTreeSet};

use guest_layout::GuestMemory;
use forge_doc::BlockKind;
use game_markers::Area;

use crate::firefight::runtime::read_players;
use crate::rules::census::Table;

/// A lone flag's cylinder radius, in world units (`Area::default`). Not
/// verified.
pub const RADIUS: f32 = game_markers::area::DEFAULT_RADIUS;
/// How far a lone flag's zone reaches over it, in world units. Not
/// verified.
pub const ABOVE: f32 = game_markers::area::DEFAULT_ABOVE;
/// How far a zone reaches under its lowest flag, in world units. Not
/// verified.
pub const BELOW: f32 = game_markers::area::DEFAULT_BELOW;
/// The roster slots a game has.
pub const SLOTS: u8 = 16;

/// A zone's shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// An area round a centre marker's place (a flag, or a hill's corners'
    /// middle), facing `yaw` radians: the flag's facing, which turns a box.
    Area { at: [f32; 3], yaw: f32, area: Area },
}

impl Shape {
    /// The zone's middle (`Area::middle`). `spawn_near` measures from it
    /// (`super::spawns`).
    pub fn middle(&self) -> [f32; 3] {
        match self {
            Shape::Area { at, area, .. } => area.middle(*at),
        }
    }

    /// Whether `p` is inside, edges included, with a box turned to the
    /// marker's facing (`Area::contains_turned`).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        match self {
            Shape::Area { at, yaw, area } => area.contains_turned(*at, *yaw, p),
        }
    }
}

/// Where a zone's shape came from: the step of the order that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The flag's own area.
    Area,
    /// A hill's corners.
    Corners,
    /// The default cylinder round a lone flag.
    Default,
}

impl Source {
    /// The name `rules.zones` reports.
    pub fn name(self) -> &'static str {
        match self {
            Source::Area => "area",
            Source::Corners => "corners",
            Source::Default => "default",
        }
    }
}

/// One zone: its shape and the handle that reports its enters and leaves.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub shape: Shape,
    pub reporter: u32,
    /// Which step of the order made the shape.
    pub source: Source,
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

/// The area of a flag labelled `label` with no area of its own and no
/// corners: the default area of the label's marker
/// (`game_markers::Kind::of_label`), else [`Area::default`].
pub fn default_area(label: &str) -> Area {
    game_markers::Kind::of_label(label)
        .and_then(|k| k.area().cloned())
        .unwrap_or_default()
}

impl Zones {
    /// The zones of `table`'s netgame flags, at `flags`' positions and
    /// facings (by flag index, as `engine::objectives::netgame_flags` and
    /// `netgame_flag_facings` read them). A flag with no position has no
    /// zone.
    pub fn new(table: &Table, flags: &[([f32; 3], f32)]) -> Zones {
        let at = |index: u32| flags.get(index as usize).map(|(p, _)| *p);
        let yaw = |index: u32| flags.get(index as usize).map_or(0.0, |(_, yaw)| *yaw);
        // The flags with no area of their own that share a label and a
        // number: a hill's corners.
        let mut groups: BTreeMap<(&str, i32), Vec<(u32, [f32; 3])>> = BTreeMap::new();
        for t in table.things() {
            if t.block != BlockKind::NetgameFlags || t.area.is_some() {
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
            let own = t.area.as_ref().map(|a| Zone {
                shape: Shape::Area {
                    at: p,
                    yaw: yaw(t.index),
                    area: a.marker_area(),
                },
                reporter: t.handle(),
                source: Source::Area,
            });
            let corners = || {
                let group = t.number.and_then(|n| groups.get(&(t.text.as_str(), n)))?;
                if group.len() < 3 {
                    return None;
                }
                let places: Vec<[f32; 3]> = group.iter().map(|(_, q)| *q).collect();
                let (centre, area) = Area::of_corners(&places)?;
                // A hull of corners lies on the world's axes.
                Some(Zone {
                    shape: Shape::Area { at: centre, yaw: 0.0, area },
                    reporter: group.iter().map(|(h, _)| *h).min().unwrap_or(t.handle()),
                    source: Source::Corners,
                })
            };
            let zone = own.or_else(corners).unwrap_or_else(|| Zone {
                shape: Shape::Area {
                    at: p,
                    yaw: yaw(t.index),
                    area: default_area(&t.text),
                },
                reporter: t.handle(),
                source: Source::Default,
            });
            zones.insert(t.handle(), zone);
        }
        Zones { zones }
    }

    /// The zones of the loaded map's flags; none when they cannot be read.
    pub fn read(m: &dyn GuestMemory, table: &Table) -> Zones {
        let facings = engine::objectives::netgame_flag_facings(m).unwrap_or_default();
        let flags: Vec<([f32; 3], f32)> = engine::objectives::netgame_flags(m)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, (at, _, _))| (at, facings.get(i).copied().unwrap_or(0.0)))
            .collect();
        Zones::new(table, &flags)
    }

    pub fn get(&self, zone: u32) -> Option<&Zone> {
        self.zones.get(&zone)
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// Every zone, by handle.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Zone)> {
        self.zones.iter().map(|(&h, z)| (h, z))
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
    fn flag_map(flags: &[([f32; 3], u16, u16)]) -> guest_layout::testing::FakeGuest {
        use engine::objectives::{FLAG_TEAM, FLAG_TYPE, NETGAME_FLAG_STRIDE, NETGAME_FLAGS};
        let mut m = guest_layout::testing::FakeGuest::new();
        m.put_u32(guest_layout::points::SCENARIO.base, SCNR);
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
            "rules \"T\" 1\ntype slayer\nlabel territory: zones at least 1\n\non tick { }\n",
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

    fn cylinder(at: [f32; 3]) -> Shape {
        Shape::Area {
            at,
            yaw: 0.0,
            area: Area::default(),
        }
    }

    #[test]
    fn a_flag_is_a_cylinder_around_it() {
        let s = cylinder([10.0, 0.0, 2.0]);
        assert!(s.contains([10.0, 0.0, 2.0]));
        assert!(s.contains([10.0 + RADIUS, 0.0, 2.0]));
        assert!(s.contains([10.5, 0.5, 2.0 + ABOVE]));
        assert!(s.contains([10.0, 0.0, 2.0 - BELOW]));
        assert!(!s.contains([10.0 + RADIUS + 0.01, 0.0, 2.0]));
        assert!(!s.contains([10.0, 0.0, 2.0 + ABOVE + 0.01]));
        assert!(!s.contains([10.0, 0.0, 2.0 - BELOW - 0.01]));
        assert_eq!(s.middle(), [10.0, 0.0, 2.0 + (ABOVE - BELOW) / 2.0]);
    }

    #[test]
    fn the_hull_of_a_square_and_its_middle_is_the_square() {
        let (centre, area) = Area::of_corners(&[
            [0.0, 0.0, 1.0],
            [4.0, 0.0, 1.0],
            [2.0, 2.0, 1.0],
            [4.0, 4.0, 1.0],
            [0.0, 4.0, 1.0],
        ])
        .unwrap();
        // Too few, or in a line: no polygon.
        assert!(Area::of_corners(&[[0.0; 3], [1.0, 1.0, 0.0]]).is_none());
        assert!(Area::of_corners(&[[0.0; 3], [1.0, 1.0, 0.0], [2.0, 2.0, 0.0]]).is_none());
        let p = Shape::Area { at: centre, yaw: 0.0, area };
        assert!(p.contains([2.0, 2.0, 1.0]));
        assert!(p.contains([0.0, 0.0, 1.0]), "an edge is inside");
        assert!(!p.contains([5.0, 2.0, 1.0]));
        assert!(!p.contains([2.0, 2.0, 1.0 + ABOVE + 0.5]));
    }

    /// A hill's four corners make one zone that the first corner
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
            assert_eq!(z.source, Source::Corners);
            assert!(zones.contains(h, [3.0, 3.0, 1.2]));
            assert!(!zones.contains(h, [7.0, 3.0, 1.2]));
            // From under the lowest corner to over the highest.
            assert!(zones.contains(h, [3.0, 3.0, 1.0 - BELOW]));
            assert!(zones.contains(h, [3.0, 3.0, 1.5 + ABOVE]));
            assert!(!zones.contains(h, [3.0, 3.0, 1.5 + ABOVE + 0.1]));
        }
        let lone = corners[4].object;
        assert_eq!(zones.get(lone).unwrap().reporter, lone);
        assert_eq!(zones.get(lone).unwrap().source, Source::Default);
        assert_eq!(zones.get(lone).unwrap().shape, cylinder([50.0, 0.0, 0.0]));
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

    /// A flag's own area is its zone, a concave path's notch is outside it,
    /// and a flag with no area, or a hill's corners, are as before.
    #[test]
    fn a_flags_area_is_its_zone_before_corners_and_the_default() {
        use engine::objectives::kind;
        let m = flag_map(&[
            ([10.0, 0.0, 0.0], kind::HILL, 0),
            ([30.0, 0.0, 0.0], kind::HILL + 1, 0),
            ([50.0, 0.0, 0.0], kind::TERRITORY, 0),
        ]);
        let mut things = census::things(&m).unwrap();
        // An L: its notch is the corner over x > 0, y > 0 (offsets).
        let l = Area::path(vec![
            [-4.0, -4.0],
            [4.0, -4.0],
            [4.0, 0.0],
            [0.0, 0.0],
            [0.0, 4.0],
            [-4.0, 4.0],
        ]);
        assert_eq!(l.validate(), Ok(()));
        let hill = things.iter_mut().find(|t| t.index == 0).unwrap();
        hill.area = Some(l);
        let ir = ::rules::compile(
            "rules \"T\" 1\ntype slayer\nlabel territory: zones at least 1\n\non tick { }\n",
        )
        .unwrap();
        let t = Table::new(&things, &ir);
        let zones = Zones::read(&m, &t);
        let shaped = t.things().iter().find(|t| t.index == 0).unwrap().handle();
        let z = zones.get(shaped).unwrap();
        assert_eq!((z.source, z.reporter), (Source::Area, shaped));
        assert!(zones.contains(shaped, [8.0, -2.0, 0.0]));
        assert!(zones.contains(shaped, [8.0, 2.0, 0.0]));
        assert!(!zones.contains(shaped, [12.0, 2.0, 0.0]), "the notch");
        assert!(!zones.contains(shaped, [8.0, -2.0, 1.6]), "over its reach");
        assert!(!zones.contains(shaped, [14.5, 0.0, 0.0]), "past the path");
        assert_eq!(
            z.shape.middle(),
            Area::path(vec![
                [-4.0, -4.0],
                [4.0, -4.0],
                [4.0, 0.0],
                [0.0, 0.0],
                [0.0, 4.0],
                [-4.0, 4.0]
            ])
            .middle([10.0, 0.0, 0.0])
        );
        // The others have none: a lone flag is the default cylinder.
        let lone = t.things().iter().find(|t| t.index == 1).unwrap().handle();
        assert_eq!(zones.get(lone).unwrap().source, Source::Default);
        assert!(zones.contains(lone, [30.5, 0.0, 0.0]));
    }

    /// A flag return with no area of its own is the capture plate its
    /// marker draws; a territory is still the default cylinder.
    #[test]
    fn a_flag_return_with_no_area_is_the_plate_its_marker_draws() {
        use engine::objectives::kind;
        let m = flag_map(&[
            ([0.0; 3], kind::CTF_RETURN, 0),
            ([20.0, 0.0, 0.0], kind::TERRITORY, 0),
        ]);
        let t = table(&m);
        let zones = Zones::read(&m, &t);
        let handle = |index| t.things().iter().find(|t| t.index == index).unwrap().handle();
        let plate = zones.get(handle(0)).unwrap();
        assert_eq!(plate.source, Source::Default);
        assert_eq!(plate.shape, Shape::Area { at: [0.0; 3], yaw: 0.0, area: Area::plate() });
        assert!(zones.contains(handle(0), [0.4, 0.0, 0.0]));
        assert!(!zones.contains(handle(0), [0.8, 0.0, 0.0]), "inside a cylinder, past the plate");
        assert_eq!(zones.get(handle(1)).unwrap().shape, cylinder([20.0, 0.0, 0.0]));
        assert_eq!(default_area("hill_corner"), Area::default());
        assert_eq!(default_area("assault_bomb_return"), Area::plate());
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

    /// A box on a flag facing a quarter turn and a half: its zone turns
    /// with the flag, and the wall the game draws for it has its corners
    /// where the zone does.
    #[test]
    fn a_turned_box_scores_inside_its_turned_corners_and_draws_them() {
        use crate::forge_glue::markers_world::{Wall, draw_walls};
        use crate::rules::census::Thing;
        use ::rules::labels::Class;
        let yaw = std::f32::consts::FRAC_PI_4;
        let at = [10.0, 5.0, 1.0];
        let boxed = Area::boxed([3.0, 1.0]);
        let table = Table::from_things(vec![Thing {
            text: "territory".into(),
            label: None,
            number: None,
            class: Class::Flag,
            designator: None,
            block: BlockKind::NetgameFlags,
            index: 0,
            area: Some(forge_doc::Area::try_from(&boxed).unwrap()),
        }]);
        let zones = Zones::new(&table, &[(at, yaw)]);
        let handle = table.things()[0].handle();
        let zone = zones.get(handle).unwrap();
        assert_eq!(zone.source, Source::Area);
        assert_eq!(zone.shape, Shape::Area { at, yaw, area: boxed.clone() });
        // The corner along the flag's facing and left of it.
        let (s, c) = yaw.sin_cos();
        let turned = |x: f32, y: f32| [at[0] + x * c - y * s, at[1] + x * s + y * c, at[2]];
        assert!(zones.contains(handle, turned(2.9, 0.9)), "inside its turned corner");
        assert!(!zones.contains(handle, turned(3.1, 0.9)), "past its turned end");
        assert!(!zones.contains(handle, turned(2.9, 1.1)), "past its turned side");
        // The world-axis corner of an unturned box is outside the turned one.
        assert!(!zones.contains(handle, [at[0] + 2.9, at[1] + 0.9, at[2]]));
        // The drawn wall's floor corners are the zone's corners.
        let Shape::Area { at, yaw, area } = &zone.shape;
        let wall = Wall { name: "zone:0".into(), team: None, at: *at, yaw: *yaw, area: area.clone() };
        let (groups, _) = draw_walls(&[wall], None, &game_markers::Palette::default());
        let points: Vec<[f32; 3]> = groups[0].edges.iter().flatten().copied().collect();
        let low = points.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
        let mut corners: Vec<[f32; 3]> = points.into_iter().filter(|p| (p[2] - low).abs() < 1e-4).collect();
        corners.dedup_by(|a, b| a.iter().zip(b.iter()).all(|(a, b)| (a - b).abs() < 1e-4));
        for (x, y) in [(3.0, 1.0), (-3.0, 1.0), (3.0, -1.0), (-3.0, -1.0)] {
            let want = turned(x, y);
            assert!(
                corners.iter().any(|p| (p[0] - want[0]).abs() < 1e-3 && (p[1] - want[1]).abs() < 1e-3),
                "the wall has the corner {want:?}: {corners:?}"
            );
            let on_floor = [want[0], want[1], at[2]];
            assert!(zone.shape.contains(on_floor), "the drawn corner {want:?} scores");
            let past = turned(x * 1.05, y * 1.05);
            assert!(!zone.shape.contains(past), "past the drawn corner {want:?}");
        }
    }
}
