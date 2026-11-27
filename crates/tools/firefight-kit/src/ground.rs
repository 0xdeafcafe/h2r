//! The AI ground as a build step (docs/game/game-types-without-carriers.md,
//! "Maps for scripted games"): the nav mesh, the sectors, the zones and the
//! sixteen squad slots, built over a map's placed labels and only when
//! asked. Nothing here runs at boot.
//!
//! [`build`] takes the base map's bytes, the labels placed on it and a
//! [`Recipe`]. A label stands in for what the generator would otherwise
//! pick, and with none placed the generator picks as it always has:
//!
//! | Label | Stands in for |
//! |---|---|
//! | `ff_drop` | the generated drop zones (6 to 12), one zone per label |
//! | `ff_base` | the two bases found from the flag stands and starts |
//! | `ff_cover` | the sampled firing positions: a placed one is taken first |
//! | `siege_site`, `siege_goal`, `siege_line` | read by `siege.rules`; the ground only checks each is on it |
//!
//! [`build_with`] adds the stage-two classes (`Classes`, copied from the
//! owner's campaign maps) as the kit's own stage two does: a build over
//! the headroom cuts them from the end of the list when `cut_to_fit` is on,
//! and [`Ground::classes`] and [`Ground::cut`] say what was kept and cut.
//!
//! The result carries a [`Receipt`] of the inputs. [`Receipt::stale`] is
//! the stale rule: the ground is stale when any input differs from the
//! receipt's.
use std::{cmp::Ordering, path::Path};

use anyhow::{Result, ensure};
use content::{MapId, Sha256};
use serde::Serialize;

use crate::{Bases, ClassCost, Classes, Config, Costs, KIT_VERSION, Mode, Package, Team};

type Point = [f32; 3];

/// A placed marker with a facing: a yaw in radians, as the label's facing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Marker {
    pub position: Point,
    pub facing: f32,
}

/// An `ff_base` label: the base and the team that spawns there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct BaseMarker {
    pub position: Point,
    pub team: Team,
}

/// A `siege_line` label: where a side spawns in a phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct LineMarker {
    pub position: Point,
    pub phase: u16,
    pub team: Team,
}

/// The labels placed on a map, in map units. The order they are given in
/// does not matter: [`Labels::canonical`] sorts them, so zone `NN` is the
/// `NN`th drop by position.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Labels {
    pub drops: Vec<Marker>,
    pub bases: Vec<BaseMarker>,
    pub covers: Vec<Marker>,
    pub sites: Vec<Point>,
    pub goals: Vec<Point>,
    pub lines: Vec<LineMarker>,
}

fn by_position(a: &Point, b: &Point) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.total_cmp(y))
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

impl Labels {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The labels in the one order the build and the receipt use.
    pub fn canonical(&self) -> Self {
        let mut l = self.clone();
        let marker = |a: &Marker, b: &Marker| {
            by_position(&a.position, &b.position).then(a.facing.total_cmp(&b.facing))
        };
        l.drops.sort_by(marker);
        l.covers.sort_by(marker);
        l.bases.sort_by(|a, b| {
            a.team
                .index()
                .cmp(&b.team.index())
                .then_with(|| by_position(&a.position, &b.position))
        });
        l.sites.sort_by(by_position);
        l.goals.sort_by(by_position);
        l.lines.sort_by(|a, b| {
            (a.phase, a.team.index())
                .cmp(&(b.phase, b.team.index()))
                .then_with(|| by_position(&a.position, &b.position))
        });
        l
    }

    /// Refuse labels the build cannot use: a coordinate that is not a
    /// number, or `ff_base` labels that are not one for each team.
    pub fn check(&self) -> Result<()> {
        let finite = |what: &str, p: &Point| -> Result<()> {
            ensure!(
                p.iter().all(|x| x.is_finite()),
                "a {what} label has a position that is not a number"
            );
            Ok(())
        };
        for m in self.drops.iter().chain(&self.covers) {
            ensure!(
                m.facing.is_finite(),
                "a label has a facing that is not a number"
            );
        }
        for m in &self.drops {
            finite("ff_drop", &m.position)?;
        }
        for m in &self.covers {
            finite("ff_cover", &m.position)?;
        }
        for b in &self.bases {
            finite("ff_base", &b.position)?;
        }
        for p in &self.sites {
            finite("siege_site", p)?;
        }
        for p in &self.goals {
            finite("siege_goal", p)?;
        }
        for l in &self.lines {
            finite("siege_line", &l.position)?;
        }
        if !self.bases.is_empty() {
            let per_team =
                [Team::Red, Team::Blue].map(|t| self.bases.iter().filter(|b| b.team == t).count());
            ensure!(
                per_team == [1, 1],
                "ff_base: place one base for each of the two teams, or none (red {}, blue {})",
                per_team[0],
                per_team[1]
            );
        }
        Ok(())
    }

    /// The digest of the canonical labels.
    pub fn digest(&self) -> Sha256 {
        let l = self.canonical();
        let mut out = Vec::new();
        fn put(out: &mut Vec<u8>, tag: u8, count: usize) {
            out.push(tag);
            out.extend((count as u32).to_le_bytes());
        }
        fn point(out: &mut Vec<u8>, p: &Point) {
            for x in p {
                out.extend(x.to_bits().to_le_bytes());
            }
        }
        put(&mut out, 1, l.drops.len());
        for m in &l.drops {
            point(&mut out, &m.position);
            out.extend(m.facing.to_bits().to_le_bytes());
        }
        put(&mut out, 2, l.bases.len());
        for b in &l.bases {
            point(&mut out, &b.position);
            out.push(b.team.index() as u8);
        }
        put(&mut out, 3, l.covers.len());
        for m in &l.covers {
            point(&mut out, &m.position);
            out.extend(m.facing.to_bits().to_le_bytes());
        }
        put(&mut out, 4, l.sites.len());
        for p in &l.sites {
            point(&mut out, p);
        }
        put(&mut out, 5, l.goals.len());
        for p in &l.goals {
            point(&mut out, p);
        }
        put(&mut out, 6, l.lines.len());
        for m in &l.lines {
            point(&mut out, &m.position);
            out.extend(m.phase.to_le_bytes());
            out.push(m.team.index() as u8);
        }
        Sha256::of(&out)
    }
}

/// The generator's settings: the kit's version, which team's base is the
/// home base, and whether it builds the kit or the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Recipe {
    pub kit_version: u32,
    pub home: Team,
    pub mode: Mode,
}

impl Recipe {
    /// The kit at this build's version, red home.
    pub fn new(mode: Mode) -> Self {
        Self {
            kit_version: KIT_VERSION,
            home: Team::Red,
            mode,
        }
    }

    pub fn home(self, home: Team) -> Self {
        Self { home, ..self }
    }
}

/// Why built ground no longer matches what it was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stale {
    /// The kit that built it is not this one.
    KitVersion,
    /// The map's collision (the base the ground was built over) changed.
    Collision,
    /// The generator's settings changed.
    Recipe,
    /// A label was placed, moved or removed.
    Labels,
}

/// What a build covers: the base map, the canonical labels and the recipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Receipt {
    pub base: Sha256,
    pub labels: Sha256,
    pub recipe: Recipe,
}

impl Receipt {
    pub fn of(base: Sha256, labels: &Labels, recipe: &Recipe) -> Self {
        Self {
            base,
            labels: labels.digest(),
            recipe: *recipe,
        }
    }

    /// The stale rule: the ground is stale when its kit is not this one, or
    /// any of base map (its collision, with the session's solid objects),
    /// recipe or labels differs from what this receipt covers. A change
    /// that touches none of them (a name, a script, a colour) leaves it
    /// current.
    pub fn stale(&self, base: Sha256, labels: &Labels, recipe: &Recipe) -> Option<Stale> {
        if self.recipe.kit_version != KIT_VERSION {
            Some(Stale::KitVersion)
        } else if self.base != base {
            Some(Stale::Collision)
        } else if self.recipe != *recipe {
            Some(Stale::Recipe)
        } else if self.labels != labels.digest() {
            Some(Stale::Labels)
        } else {
            None
        }
    }
}

/// A placed label the ground cannot honour. The build still succeeds with
/// the rest; the overlay draws these.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    /// The label: `ff_drop`, `ff_cover`, `siege_site`, `siege_goal` or
    /// `siege_line`.
    pub label: &'static str,
    /// Its place in the canonical order of its kind.
    pub index: usize,
    pub position: Point,
    pub reason: &'static str,
}

/// Built ground: the map with it, what it cost and what it covers.
#[derive(Clone, Debug)]
pub struct Ground {
    pub bytes: Vec<u8>,
    pub costs: Costs,
    pub zones: usize,
    pub squads: usize,
    pub bases: Bases,
    pub problems: Vec<Problem>,
    /// The steps taken to fit the map's tag headroom ([`crate::Fit`]).
    pub fitted: Vec<crate::Fit>,
    /// The dropship, kept or cut ([`crate::ShipReport`]).
    pub ship: Option<crate::ShipReport>,
    /// The drop pods, kept or cut ([`crate::PodReport`]).
    pub pods: Option<crate::PodReport>,
    /// What each kept stage-two class added, in palette order after the
    /// Elite; empty for a build with none.
    pub classes: Vec<ClassCost>,
    /// The classes cut to fit the headroom, first cut first.
    pub cut: Vec<String>,
    pub receipt: Receipt,
}

/// Build the ground over `base`, the multiplayer map named `base_name`.
/// Labels take the place of the generator's picks (see the module); with
/// none placed the build is the kit's own. The same inputs give the same
/// bytes. A map with no room for the ground is refused with its cost and
/// biggest part, before anything is installed.
pub fn build(base: &[u8], base_name: &str, labels: &Labels, recipe: &Recipe) -> Result<Ground> {
    build_with(base, base_name, labels, recipe, None)
}

/// [`build`] with the stage-two `classes`: the build
/// `crate::build_package_with` makes with the same classes, over the
/// labels. With [`Classes::cut_to_fit`] a build over the headroom drops
/// classes from the end of the list until it fits; without, it is refused
/// with every class's cost.
pub fn build_with(
    base: &[u8],
    base_name: &str,
    labels: &Labels,
    recipe: &Recipe,
    classes: Option<&Classes<'_>>,
) -> Result<Ground> {
    ensure!(
        recipe.kit_version == KIT_VERSION,
        "the recipe is for Firefight kit v{}, this is v{KIT_VERSION}",
        recipe.kit_version
    );
    labels.check()?;
    let canonical = labels.canonical();
    let config = Config {
        home: recipe.home,
        ..Config::default()
    };
    let built =
        crate::build_with_classes(base, base_name, recipe.mode, &config, &canonical, classes)?;
    Ok(Ground {
        bytes: built.bytes,
        costs: built.costs,
        zones: built.zones,
        squads: built.squads,
        bases: built.bases,
        problems: built.problems,
        fitted: built.fitted,
        ship: built.ship,
        pods: built.pods,
        classes: built.classes,
        cut: built.cut,
        receipt: Receipt::of(Sha256::of(base), &canonical, recipe),
    })
}

impl Ground {
    /// Install the built map as a package in `maps_dir`, as
    /// [`crate::build_package`] does for the kit with no labels; its
    /// package. `base` and `base_name` are those it was built from.
    pub fn install(
        &self,
        base: &[u8],
        base_name: &str,
        maps_dir: &Path,
        package_dir: &str,
        replace: bool,
    ) -> Result<Package> {
        ensure!(
            self.receipt.base == Sha256::of(base),
            "the ground was built over another map"
        );
        let id: MapId = crate::install(
            base,
            base_name,
            self.receipt.recipe.mode,
            None,
            &self.bytes,
            maps_dir,
            package_dir,
            replace,
        )?;
        Ok(Package {
            id,
            costs: self.costs.clone(),
            zones: self.zones,
            squads: self.squads,
            bases: self.bases,
            classes: self.classes.clone(),
            cut: self.cut.clone(),
            fitted: self.fitted.clone(),
            ship: self.ship.clone(),
            pods: self.pods.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
