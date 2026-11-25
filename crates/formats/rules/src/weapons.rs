//! The weapons a host gives through `set_loadout`: the multiplayer weapon
//! list's, each by the tag path a script names and the starting-weapon
//! value (settings 89 and 94) the game maps to the list's slot. See
//! `docs/game/rules-engine.md`, "The engine API", and
//! `docs/game/game-options.md`, "Starting weapon \"None\"".
//!
//! The host asks the game's own routine, `STARTING_WEAPON_PICK`
//! (`0x19eda0`), for the tag of a value, so the map's multiplayer globals
//! decide which tag a value is; this table only says which value a path
//! asks for. A path outside it is not given: the host refuses the call (or,
//! for a `weapon` tag the script declares, the script) by the path's name.

/// One weapon of the list: the tag path a script names, the value's name
/// in a script's `settings` block (`settings::TABLE`'s
/// `starting_weapon` choices) and the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weapon {
    pub path: &'static str,
    pub name: &'static str,
    pub value: u8,
}

const fn w(path: &'static str, name: &'static str, value: u8) -> Weapon {
    Weapon { path, name, value }
}

/// Every weapon a host gives, by the value the starting-weapon setting
/// stores for it. Values 15, 16 and 20 are slots the editor does not offer
/// and have no name here, so no path asks for them.
pub const LIST: &[Weapon] = &[
    w("objects/weapons/rifle/battle_rifle/battle_rifle", "battle_rifle", 3),
    w("objects/weapons/pistol/magnum/magnum", "magnum", 4),
    w("objects/weapons/rifle/smg/smg", "smg", 5),
    w("objects/weapons/pistol/plasma_pistol/plasma_pistol", "plasma_pistol", 6),
    w("objects/weapons/rifle/plasma_rifle/plasma_rifle", "plasma_rifle", 7),
    w("objects/weapons/support_high/rocket_launcher/rocket_launcher", "rocket_launcher", 8),
    w("objects/weapons/rifle/shotgun/shotgun", "shotgun", 9),
    w("objects/weapons/rifle/sniper_rifle/sniper_rifle", "sniper_rifle", 10),
    w("objects/weapons/support_low/brute_shot/brute_shot", "brute_shot", 11),
    w("objects/weapons/pistol/needler/needler", "needler", 12),
    w("objects/weapons/rifle/covenant_carbine/covenant_carbine", "carbine", 13),
    w("objects/weapons/rifle/beam_rifle/beam_rifle", "beam_rifle", 14),
    w("objects/weapons/melee/energy_blade/energy_blade", "energy_sword", 17),
    w("objects/weapons/rifle/brute_plasma_rifle/brute_plasma_rifle", "brute_plasma_rifle", 18),
    w(
        "objects/characters/sentinel_aggressor/weapons/beam/sentinel_aggressor_beam",
        "sentinel_beam",
        19,
    ),
];

/// A tag path as the list compares it: lowercase, `/` between parts.
pub(crate) fn canonical(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

/// The list's weapon a tag path names, in either slash.
pub fn of_path(path: &str) -> Option<&'static Weapon> {
    let p = canonical(path);
    LIST.iter().find(|w| w.path == p)
}

/// The starting-weapon value a tag path asks for.
pub fn value_of(path: &str) -> Option<u8> {
    of_path(path).map(|w| w.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{self, Kind};

    #[test]
    fn every_weapon_is_a_starting_weapon_choice_of_its_name() {
        let (_, row) = settings::named("starting_weapon").unwrap();
        let Kind::Choice { names, .. } = row.kind else {
            panic!("starting_weapon is a choice");
        };
        for w in LIST {
            let choice = names.iter().find(|n| n.name == w.name).unwrap_or_else(|| panic!("{}", w.name));
            assert_eq!(choice.value, i32::from(w.value), "{}", w.name);
            // The engine's list values: 3 to 20 name a slot.
            assert!((3..=20).contains(&w.value), "{}", w.name);
        }
        // One path and one value each.
        for (i, a) in LIST.iter().enumerate() {
            for b in &LIST[i + 1..] {
                assert_ne!(a.path, b.path);
                assert_ne!(a.value, b.value);
            }
            assert_eq!(canonical(a.path), a.path, "{} is stored canonical", a.path);
        }
    }

    #[test]
    fn a_path_is_found_in_either_slash_and_case() {
        assert_eq!(value_of("objects/weapons/rifle/smg/smg"), Some(5));
        assert_eq!(value_of("objects\\weapons\\rifle\\smg\\smg"), Some(5));
        assert_eq!(value_of("Objects/Weapons/Rifle/SMG/smg"), Some(5));
        assert_eq!(value_of("objects/weapons/rifle/smg"), None);
        assert_eq!(value_of("objects/forge/block_wall/block_wall"), None);
    }

    /// Every weapon a built-in script names is one the host gives.
    #[test]
    fn the_builtins_weapons_are_on_the_list() {
        for (key, src) in crate::builtin::examples() {
            let ir = crate::compile(src).unwrap();
            for t in ir.tags.iter().filter(|t| t.group == crate::ir::TagGroup::Weapon) {
                assert!(of_path(&t.path).is_some(), "{key}: {}", t.path);
            }
        }
    }
}
