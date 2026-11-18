//! The Halo 2-assets variant of a campaign level (docs/halo1/campaign.md
//! §5): Halo 1's player, weapons and vehicles swapped for their nearest
//! Halo 2 equivalents, the retail tags themselves, from the base or a
//! donor map off the Halo 2 disc.
//!
//! A Halo 1 tag is recognised by plain words in its path, read at run time;
//! the Halo 2 tag is found by its group and the last component of its
//! name ([`find`]), in the set or a donor ([`wanted`]). No tag list is
//! copied from either game. The multiplayer table
//! (`h1convert::sandbox::TABLE`, the user's decisions there) is followed
//! where the two overlap; the campaign differs where its Halo 2 sandbox
//! has an item multiplayer lacks, or where the item is a scripted set
//! piece ([`SWAPS`]).
//!
//! What stays Halo 1's in both variants, and why:
//!
//! - **Placed bipeds.** They are the cutscenes' casts and the levels'
//!   bodies; the scripts animate them on Halo 1's skeletons, which Halo 2's
//!   bipeds lack (cutscenes keep Halo 1's characters until retargeting
//!   exists, §5).
//! - **The AI's characters** are retail Halo 2 `char` tags in both
//!   variants (`h1ai::convert::character_for`), so their bipeds and
//!   weapons are Halo 2's already; the squads name them as before.
//! - **Scenery, devices, health packs** are the level, not its sandbox;
//!   the health model is Halo 1's whatever the assets (§6).

use std::collections::HashMap;
use std::io::Cursor;

use h1map::H1Map;
use map::build::TagSet;
use map::{FourCC, Map, TagId};

use crate::Result;

/// Which assets a level is converted with.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Assets {
    /// Halo 1's own, converted (the default).
    #[default]
    Halo1,
    /// Halo 2's retail characters, weapons and vehicles.
    Halo2,
}

/// What a Halo 1 object becomes in the Halo 2-assets variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swap {
    /// The retail Halo 2 tag of the same group whose name's last component
    /// is this.
    Retail(&'static str),
    /// Halo 1's own, converted as in the Halo 1 variant, for this reason.
    Keep(&'static str),
    /// Left out, for this reason.
    Drop(&'static str),
}

const CINEMATIC: &str = "a cutscene prop; cutscenes keep Halo 1's objects";
const PLAYBACK: &str = "flown by a recorded animation made for Halo 1's vehicle";

/// Halo 1 group, words that must all be in the Halo 1 path (`_` read as a
/// space), and the swap. First match wins, so specific rows come first.
pub const SWAPS: &[(&[u8; 4], &[&str], Swap)] = &[
    // Cutscene props (a10's `scorpion_cinematic`): animated by Halo 1's
    // cutscenes, which keep Halo 1's objects in both variants.
    (b"weap", &["cinematic"], Swap::Keep(CINEMATIC)),
    (b"vehi", &["cinematic"], Swap::Keep(CINEMATIC)),
    (b"eqip", &["cinematic"], Swap::Keep(CINEMATIC)),
    // Flown by a recorded animation (d20's `banshee_oldplayback`): the
    // recording replays Halo 1's controls, which on Halo 2's vehicle would
    // fly another path.
    (b"vehi", &["playback"], Swap::Keep(PLAYBACK)),
    // Weapons. As multiplayer's table except the fuel rod.
    (b"weap", &["plasma pistol"], Swap::Retail("plasma_pistol")),
    (b"weap", &["plasma rifle"], Swap::Retail("plasma_rifle")),
    // Halo 2's campaign has the fuel rod gun (multiplayer has none, so
    // there it is the rocket launcher).
    (b"weap", &["plasma cannon"], Swap::Retail("flak_cannon")),
    (b"weap", &["fuel rod"], Swap::Retail("flak_cannon")),
    // The user's multiplayer rule: the SMG by default (both automatic
    // bullet hoses); a campaign has no game variant to make it precise.
    (b"weap", &["assault rifle"], Swap::Retail("smg")),
    // The same slot, the user's multiplayer choice.
    (b"weap", &["pistol"], Swap::Retail("magnum")),
    (b"weap", &["needler"], Swap::Retail("needler")),
    (b"weap", &["sniper"], Swap::Retail("sniper_rifle")),
    (b"weap", &["rocket launcher"], Swap::Retail("rocket_launcher")),
    (b"weap", &["shotgun"], Swap::Retail("shotgun")),
    // Close range; Halo 2 has no flamethrower (multiplayer's choice).
    (b"weap", &["flamethrower"], Swap::Retail("shotgun")),
    (b"weap", &["energy sword"], Swap::Retail("energy_blade")),
    // Grenades: Halo 2's own (the globals keep the base's grenade types).
    (b"eqip", &["frag grenade"], Swap::Retail("frag_grenade")),
    (b"eqip", &["plasma grenade"], Swap::Retail("plasma_grenade")),
    (
        b"eqip",
        &["health"],
        Swap::Keep("Halo 1's health model stays in both variants (§6)"),
    ),
    (
        b"eqip",
        &["ammo"],
        Swap::Drop("Halo 2 has no ammo pickups, and the Halo 1 weapons they fill are gone"),
    ),
    // Vehicles the player and the AI drive and fight in.
    (b"vehi", &["rwarthog"], Swap::Retail("warthog")),
    (b"vehi", &["rocket warthog"], Swap::Retail("warthog")),
    (b"vehi", &["warthog"], Swap::Retail("warthog")),
    (b"vehi", &["ghost"], Swap::Retail("ghost")),
    (b"vehi", &["scorpion"], Swap::Retail("scorpion")),
    (b"vehi", &["banshee"], Swap::Retail("banshee")),
    (b"vehi", &["wraith"], Swap::Retail("wraith")),
    // Halo 1's shade is Halo 2's Covenant anti-personnel turret.
    (b"vehi", &["c gun turret"], Swap::Retail("c_turret_ap")),
    (b"vehi", &["turret"], Swap::Retail("c_turret_ap")),
    // Set pieces: the scripts fly them with Halo 1's recorded and custom
    // animations, open their bays and load the AI into Halo 1's seats.
    (
        b"vehi",
        &["pelican"],
        Swap::Keep("a scripted set piece flown by Halo 1's animations"),
    ),
    (
        b"vehi",
        &["dropship"],
        Swap::Keep("a scripted set piece flown by Halo 1's animations"),
    ),
    (
        b"vehi",
        &["lifepod"],
        Swap::Keep("a scripted set piece; Halo 2 has none"),
    ),
    // The player.
    (b"bipd", &["cyborg"], Swap::Retail("masterchief")),
];

/// The swap for a Halo 1 tag of `group` at `path`. Groups and paths no row
/// names stay Halo 1's.
pub fn swap_for(group: &[u8; 4], path: &str) -> Swap {
    let path = path.to_ascii_lowercase().replace('_', " ");
    SWAPS
        .iter()
        .find(|(g, words, _)| *g == group && words.iter().all(|w| path.contains(w)))
        .map_or(Swap::Keep("no Halo 2 equivalent named"), |r| r.2)
}

/// Whether `name`'s last path component is `last`.
fn last_is(name: &str, last: &str) -> bool {
    name.rsplit(['\\', '/'])
        .next()
        .is_some_and(|l| l.eq_ignore_ascii_case(last))
}

/// The tag of `group` in `set` whose name ends in `last`, the shortest
/// name when there are several.
pub fn find(set: &TagSet, group: FourCC, last: &str) -> Option<TagId> {
    set.slots
        .iter()
        .filter_map(|s| {
            let t = s.tag.as_ref().filter(|t| t.group == group)?;
            let n = s.name.as_deref().filter(|n| last_is(n, last))?;
            Some((n.len(), t.id))
        })
        .min_by_key(|c| c.0)
        .map(|(_, id)| id)
}

/// The Halo 1 tags a level places, gives the player or makes the player:
/// the weapon, equipment and vehicle palettes, the starting profiles'
/// weapons, and the cyborg.
pub fn swapped_sources(h1: &H1Map) -> Result<Vec<(FourCC, String)>> {
    let mut out: Vec<(FourCC, String)> = Vec::new();
    let mut add = |id: h1map::TagId| -> Result<()> {
        let t = h1.tag(id)?;
        let k = (FourCC(t.group.0.to_be_bytes()), t.name.clone());
        if !out.contains(&k) {
            out.push(k);
        }
        Ok(())
    };
    use crate::scenario::h1 as at;
    for pal in [at::WEAPON_PALETTE, at::EQUIPMENT_PALETTE, at::VEHICLE_PALETTE] {
        for e in crate::scenario::h1_block(h1, pal)? {
            if let Some(r) = h1map::TagRef::read(e, 0).filter(|r| !r.id.is_none()) {
                add(r.id)?;
            }
        }
    }
    for p in crate::profiles::read(h1)? {
        for w in p.weapons.iter().filter_map(|w| w.0) {
            add(w)?;
        }
    }
    if let Some(t) = h1.find(h1map::Group::new(b"bipd"), crate::scenario::PLAYER_BIPED) {
        add(t.id)?;
    }
    Ok(out)
}

/// The retail tags (group and full name) the Halo 2-assets variant of `h1`
/// places that `set` lacks, as the first donor that has each names it, for
/// [`crate::donor::bring`]. A role no donor has is left out here; the
/// placement then keeps Halo 1's object (a `dropped` line says so).
pub fn wanted(h1: &H1Map, set: &TagSet, donors: &crate::donor::Donors) -> Result<Vec<(FourCC, String)>> {
    let mut roles: Vec<(FourCC, &'static str)> = Vec::new();
    for (group, name) in swapped_sources(h1)? {
        if let Swap::Retail(last) = swap_for(&group.0, &name)
            && find(set, group, last).is_none()
            && !roles.contains(&(group, last))
        {
            roles.push((group, last));
        }
    }
    let mut out = Vec::new();
    if roles.is_empty() {
        return Ok(out);
    }
    for (_, bytes) in &donors.maps {
        let src = Map::from_reader(Cursor::new(bytes.as_slice()))?;
        roles.retain(|&(group, last)| {
            let hit = src
                .tags()
                .iter()
                .filter(|t| t.group == group && t.location.is_some())
                .filter_map(|t| t.name.clone())
                .filter(|n| last_is(n, last))
                .min_by_key(|n| n.len());
            match hit {
                Some(n) => {
                    out.push((group, n));
                    false
                }
                None => true,
            }
        });
    }
    Ok(out)
}

/// What the Halo 2-assets variant placed instead of Halo 1's tags: by
/// (group, Halo 1 name, lower case), the retail tag. The scripts' object
/// definitions and references resolve through it.
#[derive(Debug, Default, Clone)]
pub struct Aliases(pub HashMap<(FourCC, String), TagId>);

impl Aliases {
    /// Records that the Halo 1 tag `group` `name` is `to`.
    pub fn insert(&mut self, group: FourCC, name: &str, to: TagId) {
        self.0.insert((group, name.to_ascii_lowercase()), to);
    }

    /// The tag standing in for the Halo 1 tag `group` `name`.
    pub fn get(&self, group: FourCC, name: &str) -> Option<TagId> {
        self.0.get(&(group, name.to_ascii_lowercase())).copied()
    }

    /// The tag standing in for a Halo 1 tag of any group named `name`.
    pub fn by_name(&self, name: &str) -> Option<(FourCC, TagId)> {
        let n = name.to_ascii_lowercase();
        self.0
            .iter()
            .find(|((_, k), _)| *k == n)
            .map(|((g, _), id)| (*g, *id))
    }
}

/// The retail tag for the Halo 1 object `tag` in `set`: `Ok(Some)` to place
/// it, `Ok(None)` to convert Halo 1's own, `Err(why)` to leave it out.
pub fn stand_in(set: &TagSet, tag: &h1map::Tag) -> std::result::Result<Option<TagId>, String> {
    let group = tag.group.0.to_be_bytes();
    match swap_for(&group, &tag.name) {
        Swap::Retail(last) => Ok(find(set, FourCC(group), last)),
        Swap::Keep(_) => Ok(None),
        Swap::Drop(why) => Err(why.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo1_items_swap_by_the_words_in_their_paths() {
        let w = |p: &str| swap_for(b"weap", p);
        assert_eq!(w("weapons\\assault rifle\\assault rifle"), Swap::Retail("smg"));
        assert_eq!(w("weapons\\pistol\\pistol"), Swap::Retail("magnum"));
        assert_eq!(
            w("weapons\\plasma pistol\\plasma pistol"),
            Swap::Retail("plasma_pistol"),
            "before the plain pistol"
        );
        assert_eq!(w("weapons\\plasma_cannon\\plasma_cannon"), Swap::Retail("flak_cannon"));
        assert_eq!(w("weapons\\sniper rifle\\sniper rifle"), Swap::Retail("sniper_rifle"));
        assert_eq!(
            swap_for(b"vehi", "vehicles\\rwarthog\\rwarthog"),
            Swap::Retail("warthog")
        );
        assert_eq!(
            swap_for(b"vehi", "vehicles\\c gun turret\\c gun turret_mp"),
            Swap::Retail("c_turret_ap")
        );
        assert!(matches!(
            swap_for(b"vehi", "vehicles\\pelican\\pelican"),
            Swap::Keep(_)
        ));
        assert_eq!(
            swap_for(b"vehi", "vehicles\\scorpion\\scorpion_cinematic"),
            Swap::Keep(CINEMATIC)
        );
        assert_eq!(
            swap_for(b"vehi", "vehicles\\banshee\\banshee_oldplayback"),
            Swap::Keep(PLAYBACK)
        );
        assert!(matches!(
            swap_for(b"eqip", "powerups\\health pack"),
            Swap::Keep(_)
        ));
        assert!(matches!(
            swap_for(b"eqip", "powerups\\assault rifle ammo\\assault rifle ammo"),
            Swap::Drop(_)
        ));
        assert_eq!(
            swap_for(b"bipd", "characters\\cyborg\\cyborg"),
            Swap::Retail("masterchief")
        );
        // Other bipeds (cutscene casts, bodies) stay Halo 1's.
        assert!(matches!(
            swap_for(b"bipd", "characters\\marine\\marine"),
            Swap::Keep(_)
        ));
        // The scenery and devices are the level's.
        assert!(matches!(swap_for(b"scen", "scenery\\rocks\\boulder"), Swap::Keep(_)));
    }

    #[test]
    fn a_role_is_found_by_the_last_part_of_the_name() {
        assert!(last_is("objects\\weapons\\rifle\\smg\\smg", "smg"));
        assert!(!last_is("objects\\weapons\\rifle\\smg\\smg_ammo", "smg"));
        assert!(last_is("objects/vehicles/warthog/WARTHOG", "warthog"));
    }

    #[test]
    fn aliases_answer_by_group_and_by_name_without_case() {
        let mut a = Aliases::default();
        a.insert(FourCC(*b"weap"), "Weapons\\Pistol\\Pistol", TagId(7));
        assert_eq!(a.get(FourCC(*b"weap"), "weapons\\pistol\\pistol"), Some(TagId(7)));
        assert_eq!(a.get(FourCC(*b"vehi"), "weapons\\pistol\\pistol"), None);
        assert_eq!(
            a.by_name("WEAPONS\\PISTOL\\PISTOL"),
            Some((FourCC(*b"weap"), TagId(7)))
        );
    }
}
