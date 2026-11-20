//! The Halo 2 sandbox: each Halo 1 multiplayer item or vehicle and the
//! nearest Halo 2 one (docs/halo1/multiplayer.md, "Halo 2 sandbox").
//!
//! Items are recognised by the plain English words in the last part of their
//! Halo 1 tag path ("assault rifle", "warthog"), read at run time; the Halo
//! 2 side is a role that the converter resolves to a tag of the base map by
//! the words in its tag name. No tag list is copied from either game.

/// Pickups retained from Halo 1 in both asset settings. Halo 2's native
/// equip pickup branches on these same type values at 0x00151f3e.
pub fn native_powerup(kind: i16) -> bool {
    matches!(kind, 2 | 3 | 5)
}

/// What a Halo 1 item becomes in the Halo 2 sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2Item {
    /// A Halo 2 weapon, by the words its tag name holds.
    Weapon(&'static str),
    /// A Halo 2 powerup or grenade, by the words its tag name holds.
    Equipment(&'static str),
    /// A Halo 2 vehicle, by the words its tag name holds.
    Vehicle(&'static str),
    /// Nothing: the Halo 2 sandbox has no item for it (health packs, ammo
    /// pickups) and the spawn is dropped.
    Dropped,
}

/// Rows whose Halo 2 item depends on the game variant: in a precision
/// variant (its primary starting weapon is the battle rifle, carbine or
/// sniper rifle) the item becomes this instead. Evaluated when a game
/// starts, from the variant: the converted maps place the default item and
/// record each swap (`apply::precision_swaps`, the package manifest's
/// `precision`), and `boot::precision` makes it when a precision variant
/// starts (docs/halo1/multiplayer.md).
pub const PRECISION: &[(&str, H2Item)] = &[("assault rifle", H2Item::Weapon("battle_rifle"))];

/// Halo 2 starting weapons that make a variant a precision one.
pub const PRECISION_STARTING_WEAPONS: &[&str] = &["battle_rifle", "carbine", "sniper_rifle"];

/// The item for a Halo 1 path in a precision variant: the [`PRECISION`]
/// row when one matches, else the default.
pub fn h2_equivalent_precision(h1_tag_path: &str) -> Option<H2Item> {
    let path = h1_tag_path.to_ascii_lowercase();
    PRECISION
        .iter()
        .find(|(words, _)| path.contains(words))
        .map(|&(_, item)| item)
        .or_else(|| h2_equivalent(h1_tag_path))
}

/// The Halo 1 sandbox's weapon list (`mulg`, [`crate::globals`]): per
/// Halo 2 weapon (the last part of its tag path) the Halo 1 weapons (the
/// last part of theirs) its entry names instead, first the one on the map.
/// A game variant starts players with a list entry
/// ([`crate::globals::STARTING_WEAPON_ENTRY`]), and the Halo 1 cyborg holds
/// only Halo 1 weapons (its graph has no Halo 2 weapon type), so every
/// entry names a Halo 1 weapon. Rows with a counterpart in [`TABLE`] or
/// [`PRECISION`] are that row reversed; the others (D, the coordinator's
/// rule: the nearest Halo 1 weapon by role) are marked. Each list ends in
/// the assault rifle, which every Halo 1 multiplayer map has.
pub const HALO1_WEAPON_LIST: &[(&str, &[&str])] = &[
    ("magnum", &["pistol", "assault rifle"]),
    ("smg", &["assault rifle"]),
    // PRECISION reversed: the assault rifle is the battle rifle there.
    ("battle_rifle", &["assault rifle"]),
    ("shotgun", &["shotgun", "assault rifle"]),
    ("sniper_rifle", &["sniper rifle", "assault rifle"]),
    ("rocket_launcher", &["rocket launcher", "assault rifle"]),
    ("plasma_pistol", &["plasma pistol", "plasma rifle", "pistol", "assault rifle"]),
    ("plasma_rifle", &["plasma rifle", "assault rifle"]),
    ("needler", &["needler", "plasma pistol", "plasma rifle", "assault rifle"]),
    // D: a Covenant mid-range precision rifle; Halo 1's is the pistol.
    ("covenant_carbine", &["pistol", "assault rifle"]),
    // D: a long-range precision rifle.
    ("beam_rifle", &["sniper rifle", "pistol", "assault rifle"]),
    // D: an explosive launcher.
    ("brute_shot", &["rocket launcher", "assault rifle"]),
    // D: an automatic Covenant plasma weapon.
    ("brute_plasma_rifle", &["plasma rifle", "assault rifle"]),
    // D: no melee weapon in Halo 1; the nearest close-range Covenant one.
    ("energy_blade", &["plasma rifle", "shotgun", "assault rifle"]),
    // D: a continuous Covenant beam.
    ("sentinel_aggressor_beam", &["plasma rifle", "assault rifle"]),
];

/// The Halo 1 weapons the Halo 2 weapon `h2_tag_path`'s list entry may
/// name, in order of preference ([`HALO1_WEAPON_LIST`]).
pub fn halo1_weapon_for(h2_tag_path: &str) -> Option<&'static [&'static str]> {
    let last = h2_tag_path.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    HALO1_WEAPON_LIST
        .iter()
        .find(|(h2, _)| *h2 == last)
        .map(|(_, h1)| *h1)
}

/// One row of the table: words in the Halo 1 name, and the Halo 2 item.
/// More specific rows come first ("plasma pistol" before "pistol").
pub const TABLE: &[(&str, H2Item)] = &[
    ("ammo", H2Item::Dropped),
    ("health pack", H2Item::Dropped),
    ("double speed", H2Item::Dropped),
    ("full-spectrum vision", H2Item::Dropped),
    ("plasma pistol", H2Item::Weapon("plasma_pistol")),
    ("plasma rifle", H2Item::Weapon("plasma_rifle")),
    ("plasma cannon", H2Item::Weapon("rocket_launcher")),
    ("assault rifle", H2Item::Weapon("smg")),
    ("pistol", H2Item::Weapon("magnum")),
    ("needler", H2Item::Weapon("needler")),
    ("sniper rifle", H2Item::Weapon("sniper_rifle")),
    ("rocket launcher", H2Item::Weapon("rocket_launcher")),
    ("shotgun", H2Item::Weapon("shotgun")),
    ("flamethrower", H2Item::Weapon("shotgun")),
    ("frag grenade", H2Item::Equipment("frag_grenade")),
    ("plasma grenade", H2Item::Equipment("plasma_grenade")),
    ("over shield", H2Item::Equipment("super_shield")),
    ("active camouflage", H2Item::Equipment("invisibility")),
    // Halo 1's rocket warthog (`rwarthog`) is Halo 2's gauss warthog.
    ("rwarthog", H2Item::Vehicle("warthog_gauss")),
    ("rocket warthog", H2Item::Vehicle("warthog_gauss")),
    ("warthog", H2Item::Vehicle("warthog")),
    ("ghost", H2Item::Vehicle("ghost")),
    ("scorpion", H2Item::Vehicle("scorpion")),
    ("banshee", H2Item::Vehicle("banshee")),
];

/// A Halo 2 multiplayer vehicle: its role (the name of the vehicle
/// collection, `multiplayer\single_vehicles\<role>`), the vehicle (`vehi`)
/// it spawns by last name, the model variant it names (`None`: the
/// model's default), and its netgame equipment type.
///
/// Every retail multiplayer map holds every one of these `vehi` tags; only
/// the collections differ per map. A role the base has no collection for
/// gets a built one (`h1convert::convert`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VehicleClass {
    /// Collection name (last component), as [`H2Item::Vehicle`] holds it.
    pub role: &'static str,
    /// The vehicle tag's last name component.
    pub vehicle: &'static str,
    /// Model variant name, as the collection entry's string ID.
    pub variant: Option<&'static str>,
    /// Netgame equipment type byte (`+0x14`).
    pub netgame_type: u8,
}

/// The vehicles a converted map can place. Netgame types (V: every retail
/// and Map Pack multiplayer map's netgame equipment): 1 warthog and gauss
/// warthog, 2 ghost, 3 wraith and scorpion (Containment's scorpion), 4
/// banshee, 6 and 7 turrets. Each type reads its own byte of the game
/// variant (`+0x209 + type - 1`, E `0x0015a4d0`), which may swap the
/// vehicle for another of its class or remove it; types 3 and 5 go to the
/// heavy-vehicle chooser (E `0x0015a250`: a wraith or a scorpion, both with
/// variant `multiplayer`). Retail maps also hold entries with a type and no
/// item, slots a variant's vehicle option fills: Coagulation has two of
/// type 3, which is how a scorpion appears there. Collections name the
/// model variant as retail ones do: the gauss warthog is the warthog with
/// `gauss` (Headlong, Zanzibar), the wraith and scorpion use `multiplayer`
/// (Waterworks, Containment).
pub const VEHICLES: &[VehicleClass] = &[
    VehicleClass {
        role: "warthog",
        vehicle: "warthog",
        variant: None,
        netgame_type: 1,
    },
    VehicleClass {
        role: "warthog_gauss",
        vehicle: "warthog",
        variant: Some("gauss"),
        netgame_type: 1,
    },
    VehicleClass {
        role: "ghost",
        vehicle: "ghost",
        variant: None,
        netgame_type: 2,
    },
    VehicleClass {
        role: "wraith",
        vehicle: "wraith",
        variant: Some("multiplayer"),
        netgame_type: 3,
    },
    VehicleClass {
        role: "banshee",
        vehicle: "banshee",
        variant: None,
        netgame_type: 4,
    },
    VehicleClass {
        role: "scorpion",
        vehicle: "scorpion",
        variant: Some("multiplayer"),
        netgame_type: 3,
    },
];

/// The class of a vehicle role.
pub fn vehicle_class(role: &str) -> Option<&'static VehicleClass> {
    VEHICLES.iter().find(|v| v.role == role)
}

/// The netgame equipment type byte (`+0x14`) for an item. V, on every
/// retail multiplayer map: weapons 0, grenades 8, powerups 9; vehicles by
/// [`VEHICLES`]. The engine's spawner switches on it (E `0x0015b0ef`): 0
/// and 8..9 go through the game variant's weapon and powerup settings,
/// 1..7 through its vehicle settings.
pub fn netgame_type(item: H2Item) -> u8 {
    match item {
        H2Item::Weapon(_) | H2Item::Dropped => 0,
        H2Item::Equipment(r) if r.contains("grenade") => 8,
        H2Item::Equipment(_) => 9,
        H2Item::Vehicle(r) => vehicle_class(r).map_or(1, |c| c.netgame_type),
    }
}

/// The [`TABLE`] rows whose Halo 2 item is `item`, in table order.
pub fn default_sources(item: H2Item) -> Vec<&'static str> {
    TABLE
        .iter()
        .filter(|(_, i)| *i == item)
        .map(|&(words, _)| words)
        .collect()
}

/// The Halo 2 equivalent of a Halo 1 item or vehicle tag, by its path.
/// `None` when no row matches (the converter reports it).
pub fn h2_equivalent(h1_tag_path: &str) -> Option<H2Item> {
    let last = h1_tag_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(h1_tag_path)
        .to_ascii_lowercase();
    // The folder names the weapon when the file is generic.
    let path = h1_tag_path.to_ascii_lowercase();
    TABLE
        .iter()
        .find(|(words, _)| last.contains(words))
        .or_else(|| TABLE.iter().find(|(words, _)| path.contains(words)))
        .map(|&(_, item)| item)
}

/// Whether a Halo 2 tag name has every word of `role` (`_`-separated) in
/// its last path component, the way the converter finds the base map's tag.
pub fn h2_name_matches(h2_tag_path: &str, role: &str) -> bool {
    let last = h2_tag_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(h2_tag_path)
        .to_ascii_lowercase();
    role.split('_').all(|w| last.contains(w))
}
