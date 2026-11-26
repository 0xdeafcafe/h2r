//! The retail class table: for each Firefight class the kit's stage two
//! can copy, the `char` tag that is that class and the campaign maps that
//! carry it, from a census of the retail disc's tag names
//! (docs/game/notes/2026-11-26-firefight-class-census.md). Names only: the
//! tags themselves are read from the owner's own disc when the kit is
//! built (`h2 firefight kit MAP --retail-classes`).

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
    },
    RetailClass {
        name: "jackal",
        path: r"objects\characters\jackal\ai\jackal",
        donors: &["03a_oldmombasa", "03b_newmombasa", "05a_deltaapproach", "05b_deltatowers"],
    },
    RetailClass {
        name: "drone",
        path: r"objects\characters\bugger\ai\bugger",
        donors: &["03a_oldmombasa", "01b_spacestation", "05a_deltaapproach", "05b_deltatowers"],
    },
    RetailClass {
        name: "brute",
        path: r"objects\characters\brute\ai\brute",
        donors: &["08a_deltacliffs", "06a_sentinelwalls", "07a_highcharity", "08b_deltacontrol"],
    },
    RetailClass {
        name: "hunter",
        path: r"objects\characters\hunter\ai\hunter",
        donors: &["03a_oldmombasa", "05b_deltatowers", "07a_highcharity", "08a_deltacliffs"],
    },
];

/// Each class of the table with the first of its donors `on_disc` says
/// is there: `(name, donor, path)` in keep order. A class none of whose
/// donors is there is an error naming it and its donors.
pub fn choose(
    mut on_disc: impl FnMut(&str) -> bool,
) -> Result<Vec<(&'static str, &'static str, &'static str)>, String> {
    CLASSES
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
