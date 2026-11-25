//! Our weapon ids: the weapons the extension block names for the
//! starting and secondary weapon (`+0x11a`, `+0x11c`) that the record's
//! byte cannot, because the multiplayer globals' list has no slot for them
//! (docs/game/game-options.md, "Any weapon as the starting or secondary
//! weapon").
//!
//! An id names a tag by path, never a datum, which differs per map. The
//! game resolves the path in the loaded map's tag names
//! (`variant_weapons`); a map that has no such tag keeps the record's own
//! weapon. Ids are permanent: a new weapon takes the next id, and an id a
//! build doesn't know makes the block ignored
//! ([`crate::ext::Why::Range`]).

/// One weapon of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weapon {
    /// The id the block stores (never 0, which means "the record's").
    pub id: u16,
    /// The weapon tag's path, `/` between parts, lowercase.
    pub path: &'static str,
    /// What the editors call it.
    pub label: &'static str,
}

/// The weapons, by id.
pub const TABLE: &[Weapon] = &[
    Weapon {
        id: 1,
        path: "objects/weapons/rifle/smg_silenced/smg_silenced",
        label: "Silenced SMG",
    },
    Weapon {
        id: 2,
        path: "objects/weapons/melee/gravity_hammer/gravity_hammer",
        label: "Gravity Hammer",
    },
    Weapon {
        id: 3,
        path: "objects/weapons/rifle/spiker/spiker",
        label: "Spiker",
    },
];

/// What the editors call id 0.
pub const AS_SET: &str = "Not Used";

/// The weapon of an id, if the table has it.
pub fn get(id: u16) -> Option<&'static Weapon> {
    TABLE.iter().find(|w| w.id == id)
}

/// Whether the block may hold `id`: 0 or a weapon of the table.
pub fn is_known(id: u16) -> bool {
    id == 0 || get(id).is_some()
}

/// An id's place in a picker whose first choice is [`AS_SET`]: 0 for 0 (or
/// an id the table lacks), else 1 and the table's index.
pub fn step(id: u16) -> usize {
    TABLE.iter().position(|w| w.id == id).map_or(0, |i| i + 1)
}

/// The id of picker choice `step` (0 for the first, or out of range).
pub fn id_at(step: usize) -> u16 {
    step.checked_sub(1)
        .and_then(|i| TABLE.get(i))
        .map_or(0, |w| w.id)
}

/// A picker's choices: [`AS_SET`], then each weapon's label.
pub fn choices() -> Vec<&'static str> {
    std::iter::once(AS_SET)
        .chain(TABLE.iter().map(|w| w.label))
        .collect()
}

/// The name of an id: [`AS_SET`] for 0, the label, or "Unknown weapon N".
pub fn name(id: u16) -> String {
    match (id, get(id)) {
        (0, _) => AS_SET.into(),
        (_, Some(w)) => w.label.into(),
        (n, None) => format!("Unknown weapon {n}"),
    }
}

/// A tag path as the table compares it: lowercase, `/` between parts.
pub fn canonical(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

/// The id whose tag path is `path` (either slash, any case).
pub fn id_of_path(path: &str) -> Option<u16> {
    let p = canonical(path);
    TABLE.iter().find(|w| w.path == p).map(|w| w.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_nonzero_and_paths_canonical() {
        for (i, a) in TABLE.iter().enumerate() {
            assert_ne!(a.id, 0);
            assert_eq!(canonical(a.path), a.path, "{}", a.path);
            for b in &TABLE[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.path, b.path);
                assert_ne!(a.label, b.label);
            }
        }
    }

    #[test]
    fn a_picker_step_and_an_id_round_trip() {
        assert_eq!(choices().len(), TABLE.len() + 1);
        assert_eq!(choices()[0], AS_SET);
        assert_eq!(step(0), 0);
        assert_eq!(id_at(0), 0);
        for (i, w) in TABLE.iter().enumerate() {
            assert_eq!(step(w.id), i + 1);
            assert_eq!(id_at(i + 1), w.id);
            assert_eq!(choices()[i + 1], w.label);
        }
        assert_eq!(step(999), 0);
        assert_eq!(id_at(TABLE.len() + 1), 0);
    }

    #[test]
    fn paths_are_found_in_either_slash_and_case() {
        let w = &TABLE[1];
        assert_eq!(id_of_path(w.path), Some(w.id));
        assert_eq!(
            id_of_path(&w.path.replace('/', "\\").to_uppercase()),
            Some(w.id)
        );
        assert_eq!(id_of_path("objects/weapons/rifle/smg/smg"), None);
    }

    #[test]
    fn names_say_what_an_id_is() {
        assert_eq!(name(0), AS_SET);
        assert_eq!(name(TABLE[0].id), TABLE[0].label);
        assert_eq!(name(999), "Unknown weapon 999");
        assert!(is_known(0) && is_known(TABLE[0].id) && !is_known(999));
    }

    /// No table weapon is one of the multiplayer list's: those are the
    /// record's byte's to name.
    #[test]
    fn the_table_holds_no_weapon_the_rules_list_already_gives() {
        for w in TABLE {
            assert!(
                !w.path.is_empty() && w.path.starts_with("objects/weapons/"),
                "{}",
                w.path
            );
        }
    }
}
