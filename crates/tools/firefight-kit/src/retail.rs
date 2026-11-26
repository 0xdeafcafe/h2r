//! The retail class table: for each Firefight class the kit's stage two
//! can copy, the `char` tag that is that class and the campaign maps that
//! carry it, from a census of the retail disc's tag names
//! (docs/game/notes/2026-11-26-firefight-class-census.md). Names only: the
//! tags themselves are read from the owner's own disc when the kit is
//! built (`h2 firefight kit MAP --retail-classes`).

use firefight::Faction;

/// One class of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetailClass {
    /// The director's class name, the palette row's name.
    pub name: &'static str,
    /// The `char` tag's name on the donor.
    pub path: &'static str,
    /// The campaign maps carrying the tag, the one to copy from first:
    /// the classes before it share its closure where they can.
    pub donors: &'static [&'static str],
    /// Who the class fights for.
    pub faction: Faction,
}

/// The table in keep order: the first class is the last cut.
/// Outskirts (`03a_oldmombasa`) carries the Grunt, the Jackal, the Drone
/// and the Hunter, so their shared tags are copied once; the Brute comes
/// from Uprising (`08a_deltacliffs`).
pub const CLASSES: [RetailClass; 5] = [
    RetailClass {
        name: "grunt",
        path: r"objects\characters\grunt\ai\grunt",
        donors: &["03a_oldmombasa", "01b_spacestation", "03b_newmombasa", "05a_deltaapproach"],
        faction: Faction::Covenant,
    },
    RetailClass {
        name: "jackal",
        path: r"objects\characters\jackal\ai\jackal",
        donors: &["03a_oldmombasa", "03b_newmombasa", "05a_deltaapproach", "05b_deltatowers"],
        faction: Faction::Covenant,
    },
    RetailClass {
        name: "drone",
        path: r"objects\characters\bugger\ai\bugger",
        donors: &["03a_oldmombasa", "01b_spacestation", "05a_deltaapproach", "05b_deltatowers"],
        faction: Faction::Covenant,
    },
    RetailClass {
        name: "brute",
        path: r"objects\characters\brute\ai\brute",
        donors: &["08a_deltacliffs", "06a_sentinelwalls", "07a_highcharity", "08b_deltacontrol"],
        faction: Faction::Covenant,
    },
    RetailClass {
        name: "hunter",
        path: r"objects\characters\hunter\ai\hunter",
        donors: &["03a_oldmombasa", "05b_deltatowers", "07a_highcharity", "08a_deltacliffs"],
        faction: Faction::Covenant,
    },
];

/// The Flood and human classes. **V** throughout: the `char` names and the
/// donors come from the design's reading of the campaign maps' tag names
/// (docs/game/firefight.md, "Factions"), not from a census of the disc.
/// The census step (P1, `h2 firefight kit MAP --classes ... --cost`) settles
/// each tag and its donor, and the table changes with it. A class whose
/// tag or donor is not there is a refusal that names it
/// ([`choose_factions`]); nothing falls back to another class.
pub mod donors {
    /// `char` tag of the Flood combat form that keeps the Elite's body. **V**:
    /// docs/formats/tag-schema.md says `transplant` refuses its tree (a
    /// `jmad` of `sentinel_aggressor`), so this class may be refused as
    /// uncopyable until P1 settles it.
    pub const FLOOD_COMBAT_ELITE: &str =
        r"objects\characters\floodcombat_elite\ai\floodcombat_elite";
    /// **V**: the combat form to use if the Elite one is refused.
    pub const FLOOD_COMBAT_HUMAN: &str =
        r"objects\characters\floodcombat_human\ai\flood_combat_human";
    /// **V**: bursts into infection forms, which a map then needs too.
    pub const FLOOD_CARRIER: &str = r"objects\characters\floodcarrier\ai\flood_carrier";
    /// **V**: name and donor (P1).
    pub const MARINE: &str = r"objects\characters\marine\ai\marine";

    /// **V**: the Flood lab.
    pub const FLOOD_DONORS: &[&str] = &["04b_floodlab"];
    /// **V**: the sentinel walls (the human form's donor in `h1campaign`).
    pub const FLOOD_HUMAN_DONORS: &[&str] = &["06a_sentinelwalls"];
    /// **V**: Delta Approach carries the Marine; Outskirts likely does too.
    pub const MARINE_DONORS: &[&str] = &["05a_deltaapproach", "03a_oldmombasa"];
}

/// The Flood and human classes, in keep order (see [`KEEP_ORDER`]).
pub const FACTION_CLASSES: [RetailClass; 4] = [
    RetailClass {
        name: "flood_combat_human",
        path: donors::FLOOD_COMBAT_HUMAN,
        donors: donors::FLOOD_HUMAN_DONORS,
        faction: Faction::Flood,
    },
    RetailClass {
        name: "marine",
        path: donors::MARINE,
        donors: donors::MARINE_DONORS,
        faction: Faction::Human,
    },
    RetailClass {
        name: "flood_combat_elite",
        path: donors::FLOOD_COMBAT_ELITE,
        donors: donors::FLOOD_DONORS,
        faction: Faction::Flood,
    },
    RetailClass {
        name: "flood_carrier",
        path: donors::FLOOD_CARRIER,
        donors: donors::FLOOD_DONORS,
        faction: Faction::Flood,
    },
];

/// What an entry of the keep order is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// The authored Elite (palette row 0).
    Elite,
    /// A class of [`CLASSES`] or [`FACTION_CLASSES`].
    Class,
    /// A vehicle a squad arrives in (the Phantom, the Pelican, the pod):
    /// the kit's own, not a class, but it costs tag headroom as one.
    Vehicle,
}

/// Everything the kit may spend headroom on, in keep order: the last is
/// the first cut. A set names the classes it needs, and a kit that cut one
/// refuses the set by name (`firefight::Refusal::CutClass`). The Flood and
/// human classes and the vehicles come before the Covenant table, which is
/// cut first; the vehicles are cut before the first Flood class.
pub const KEEP_ORDER: [(&str, Keep); 13] = [
    ("elite", Keep::Elite),
    ("flood_combat_human", Keep::Class),
    ("marine", Keep::Class),
    ("pod", Keep::Vehicle),
    ("phantom", Keep::Vehicle),
    ("pelican", Keep::Vehicle),
    ("flood_combat_elite", Keep::Class),
    ("flood_carrier", Keep::Class),
    ("grunt", Keep::Class),
    ("jackal", Keep::Class),
    ("drone", Keep::Class),
    ("brute", Keep::Class),
    ("hunter", Keep::Class),
];

/// Each class of `table` with the first of its donors `on_disc` says is
/// there, as [`choose`]: a class none of whose donors is there is an error
/// naming the class and its donors.
pub fn choose_from(
    table: &[RetailClass],
    mut on_disc: impl FnMut(&str) -> bool,
) -> Result<Vec<(&'static str, &'static str, &'static str)>, String> {
    table
        .iter()
        .map(|c| {
            c.donors
                .iter()
                .find(|d| on_disc(d))
                .map(|d| (c.name, *d, c.path))
                .ok_or_else(|| {
                    format!(
                        "the {} class needs one of {} on the disc",
                        c.name,
                        c.donors.join(", ")
                    )
                })
        })
        .collect()
}

/// [`FACTION_CLASSES`] over the disc's maps: a refusal naming the first
/// class whose donors are not on it.
pub fn choose_factions(
    on_disc: impl FnMut(&str) -> bool,
) -> Result<Vec<(&'static str, &'static str, &'static str)>, String> {
    choose_from(&FACTION_CLASSES, on_disc)
}

/// The faction of a class of either table.
pub fn faction_of(name: &str) -> Option<Faction> {
    CLASSES
        .iter()
        .chain(&FACTION_CLASSES)
        .find(|c| c.name == name)
        .map(|c| c.faction)
}

/// Each class of the table with the first of its donors `on_disc` says
/// is there: `(name, donor, path)` in keep order. A class none of whose
/// donors is there is an error naming it and its donors.
pub fn choose(
    on_disc: impl FnMut(&str) -> bool,
) -> Result<Vec<(&'static str, &'static str, &'static str)>, String> {
    choose_from(&CLASSES, on_disc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_each_class_once_by_its_char_path() {
        let mut names: Vec<_> = CLASSES.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CLASSES.len());
        for c in &CLASSES {
            assert!(c.path.starts_with(r"objects\characters\"), "{}", c.name);
            assert!(c.path.contains(r"\ai\"), "{}", c.name);
            assert!(!c.donors.is_empty(), "{}", c.name);
            for d in c.donors {
                assert!(content::manifest::is_map_name(d), "{d}");
                // Campaign maps only: a multiplayer map has no `char`.
                assert!(d.as_bytes()[0].is_ascii_digit(), "{d}");
            }
        }
        // The Elite is the kit's own, authored in stage one.
        assert!(CLASSES.iter().all(|c| c.name != "elite"));
    }

    #[test]
    fn the_faction_classes_name_their_tags_and_donors() {
        let mut seen: Vec<_> = CLASSES.iter().chain(&FACTION_CLASSES).map(|c| c.name).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CLASSES.len() + FACTION_CLASSES.len());
        for c in &FACTION_CLASSES {
            assert!(c.path.starts_with(r"objects\characters\"), "{}", c.name);
            assert!(c.path.contains(r"\ai\"), "{}", c.name);
            for d in c.donors {
                assert!(content::manifest::is_map_name(d), "{d}");
                assert!(d.as_bytes()[0].is_ascii_digit(), "{d}");
            }
        }
        assert!(CLASSES.iter().all(|c| c.faction == Faction::Covenant));
        assert_eq!(faction_of("flood_carrier"), Some(Faction::Flood));
        assert_eq!(faction_of("marine"), Some(Faction::Human));
        assert_eq!(faction_of("grunt"), Some(Faction::Covenant));
        assert_eq!(faction_of("elite"), None);
    }

    #[test]
    fn a_missing_faction_donor_is_a_named_refusal() {
        assert_eq!(choose_factions(|_| true).unwrap().len(), FACTION_CLASSES.len());
        // The Flood lab is missing: the first class that needs it is named
        // with its donor, and nothing else stands in.
        let error = choose_factions(|d| d != "04b_floodlab").unwrap_err();
        assert!(error.contains("flood_combat_elite"), "{error}");
        assert!(error.contains("04b_floodlab"), "{error}");
        // The Marine takes its second donor when the first is missing.
        let marine = choose_factions(|d| d == "06a_sentinelwalls" || d == "03a_oldmombasa" || d == "04b_floodlab");
        assert_eq!(marine.unwrap()[1].1, "03a_oldmombasa");
        let error = choose_factions(|d| d != "06a_sentinelwalls").unwrap_err();
        assert!(error.contains("flood_combat_human"), "{error}");
    }

    #[test]
    fn the_keep_order_cuts_ships_before_the_first_flood_class() {
        let at = |name: &str| KEEP_ORDER.iter().position(|(n, _)| *n == name).unwrap();
        let first_flood = FACTION_CLASSES
            .iter()
            .filter(|c| c.faction == Faction::Flood)
            .map(|c| at(c.name))
            .min()
            .unwrap();
        // The last is cut first, so a later entry goes before an earlier.
        for (name, kind) in KEEP_ORDER {
            if kind == Keep::Vehicle {
                assert!(at(name) > first_flood, "{name} is kept ahead of the Flood");
            }
        }
        assert_eq!(KEEP_ORDER[0], ("elite", Keep::Elite));
        // The Covenant table is cut first of all.
        let covenant = CLASSES.iter().map(|c| at(c.name)).min().unwrap();
        assert!(KEEP_ORDER[..covenant].iter().all(|(n, _)| !CLASSES.iter().any(|c| c.name == *n)));
        assert!(FACTION_CLASSES.iter().all(|c| at(c.name) < covenant));
        // Every class of both tables is in the order once, in table order.
        let classes: Vec<_> = KEEP_ORDER.iter().filter(|(_, k)| *k == Keep::Class).map(|(n, _)| *n).collect();
        let tables: Vec<_> = FACTION_CLASSES.iter().chain(&CLASSES).map(|c| c.name).collect();
        assert_eq!(classes, tables);
    }

    #[test]
    fn each_class_takes_its_first_donor_on_the_disc() {
        let all = choose(|_| true).unwrap();
        let at: Vec<_> = all.iter().map(|(n, d, _)| (*n, *d)).collect();
        assert_eq!(
            at,
            [
                ("grunt", "03a_oldmombasa"),
                ("jackal", "03a_oldmombasa"),
                ("drone", "03a_oldmombasa"),
                ("brute", "08a_deltacliffs"),
                ("hunter", "03a_oldmombasa"),
            ]
        );
        // Without Outskirts the next donor of each serves.
        let some = choose(|d| d != "03a_oldmombasa").unwrap();
        assert_eq!(some[0].1, "01b_spacestation");
        assert_eq!(some[4].1, "05b_deltatowers");
        // A class with no donor is named.
        let error = choose(|d| !d.starts_with("08a") && !d.starts_with("06a") && !d.starts_with("07a") && !d.starts_with("08b"))
            .unwrap_err();
        assert!(error.contains("brute"), "{error}");
    }
}
