//! The Halo 2 sandbox: each Halo 1 multiplayer item or vehicle and the
//! nearest Halo 2 one (docs/game/h1-multiplayer.md, "Halo 2 sandbox").
//!
//! Items are recognised by the plain English words in the last part of their
//! Halo 1 tag path ("assault rifle", "warthog"), read at run time; the Halo
//! 2 side is a role that the converter resolves to a tag of the base map by
//! the words in its tag name. No tag list is copied from either game.

/// What a Halo 1 item becomes in the Halo 2 sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2Item {
    /// A Halo 2 weapon, by the words its tag name holds.
    Weapon(&'static str),
    /// A Halo 2 powerup or grenade, by the words its tag name holds.
    Equipment(&'static str),
    /// A Halo 2 vehicle, by the words its tag name holds.
    Vehicle(&'static str),
    /// Nothing: Halo 2 has no equivalent (health packs, ammo pickups) and
    /// the spawn is dropped.
    Dropped,
}

/// Rows whose Halo 2 item depends on the game variant: in a precision
/// variant (its primary starting weapon is the battle rifle, carbine or
/// sniper rifle) the item becomes this instead. Evaluated when a game
/// starts, from the variant; until that hook exists the converted maps
/// place the default item (docs/game/h1-multiplayer.md).
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
    ("warthog", H2Item::Vehicle("warthog")),
    ("ghost", H2Item::Vehicle("ghost")),
    ("scorpion", H2Item::Vehicle("scorpion")),
    ("banshee", H2Item::Vehicle("banshee")),
];

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
