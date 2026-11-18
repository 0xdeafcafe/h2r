//! The player's biped and the grenades in a converted map's globals
//! (`matg`): which unit a player spawns as, and what each grenade type is. Shared by the multiplayer converter (the Halo 1
//! sandbox's cyborg) and the campaign converter (`h1campaign`).
//!
//! **The player representation block**, `matg +0x138` (count) / `+0x13c`
//! (address), elements of `0xbc` bytes. The engine picks the player's unit
//! in `0x0014f040` (E): EAX the player datum; the player's representation
//! index is the byte at player `+0x88` (players at `[0x4e8c24]+0x44`,
//! stride `0x21c`), clamped to the block's last element; when the game
//! globals' mode (`[0x4e6948]+8`) is 2, multiplayer, index 0 becomes 2 and 1
//! becomes 3. From the element it returns the tag datum at `+0xb4` and the
//! word at `+0xb8` (`0x14f0c2`, `0x14f0c9`). Element `+0xb0` is that datum's
//! group, so `+0xb0` is a tag reference (group, datum). D: on every
//! multiplayer map of the disc the four elements' `+0xb0` references are
//! `unit`/none, `unit`/none, `masterchief_mp`, `elite_mp`, and `+0xb8` is 0;
//! on `01a_tutorial`, element 0 is `masterchief` and the rest none.
//!
//! So a multiplayer map's player unit is elements 2 and 3 (one per
//! representation the profile picks), a campaign's elements 0 and 1.
//!
//! **The grenades**, `matg +0x100` (Halo 2) and `matg +0x128` (Halo 1),
//! one element per grenade type in the order of the equipment's grenade
//! type (fragmentation, plasma). Halo 2's elements are `0x2c` bytes: the
//! maximum count (i16) at `+0`, the equipment at `+0x1c` and the
//! projectile at `+0x24` (8-byte references); Halo 1's are `0x44`: the
//! maximum at `+0`, the equipment at `+0x24` and the projectile at
//! `+0x34` (16-byte references). V: on every Halo 2 base and Halo 1 map
//! of the discs, element 0 names the frag grenade's `eqip` and `proj` and
//! element 1 the plasma grenade's (h1objects `weapon_rebuild_disc`).

use map::build::{TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::{Error, Result};

/// `matg +0x138`: the player representation block (count, then address).
pub const PLAYER_REPRESENTATION: usize = 0x138;
/// Its element size (`imul 0xbc` at `0x0014f0bc`).
pub const PLAYER_REPRESENTATION_SIZE: usize = 0xbc;
/// The element's unit: a tag reference (group, then datum at `+0xb4`).
pub const REPRESENTATION_UNIT: usize = 0xb0;
/// The word read beside the unit (`0x14f0c9`); 0 on every disc map.
pub const REPRESENTATION_UNIT_WORD: usize = 0xb8;
/// The element's first-person arms: a `mode` reference (datum at `+0x4`).
/// E: `0x001676aa` takes `[representation + index * 0xbc + 4]` as the
/// arms model whose nodes `0x001dd4c0` binds to the weapon's
/// first-person graph by name. V: every retail element names
/// `...\fp\fp` (or the elite's `fp_arms`) here and its `fp_body` at
/// `+0x8`.
pub const REPRESENTATION_HANDS: usize = 0x0;

/// Halo 1's `matg +0x17c`: the first-person interface block; its
/// element's `+0x0` is the first-person arms (a 16-byte reference, datum
/// at `+0xc`). V: one element on a10 and Blood Gulch, naming
/// `characters\cyborg\fp\fp`.
pub const H1_FIRST_PERSON: usize = 0x17c;

/// Which game's player representations a map spawns from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Players {
    /// Elements 0 and 1.
    Campaign,
    /// Elements 2 and 3: multiplayer maps the 0 and 1 a profile picks.
    Multiplayer,
}

impl Players {
    /// The elements this game's players are made from.
    pub fn elements(self) -> [usize; 2] {
        match self {
            Players::Campaign => [0, 1],
            Players::Multiplayer => [2, 3],
        }
    }
}

/// Makes `biped` the unit every player of `players` spawns as, in the
/// globals of `set` (pinned, as `TagSet::from_map` leaves it). The word
/// beside the unit is cleared. Returns the units it replaced.
pub fn set_player_unit(set: &mut TagSet, biped: TagId, players: Players) -> Result<Vec<TagId>> {
    let globals = set.globals;
    let tag = set
        .tag_mut(globals)
        .ok_or(Error::Invalid("the set has no globals"))?;
    let TagPayload::Pinned { address, bytes } = &mut tag.payload else {
        return Err(Error::Invalid("the base globals should be pinned"));
    };
    set_player_unit_in(bytes, *address, biped, players)
}

/// [`set_player_unit`] on the globals' bytes, whose first byte is at
/// virtual `address`. The block must lie in the bytes.
pub fn set_player_unit_in(
    bytes: &mut [u8],
    address: u32,
    biped: TagId,
    players: Players,
) -> Result<Vec<TagId>> {
    let word = |b: &[u8], at: usize| -> Result<u32> {
        b.get(at..at + 4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .ok_or(Error::Invalid("the globals are too short"))
    };
    let count = word(bytes, PLAYER_REPRESENTATION)? as usize;
    let first = word(bytes, PLAYER_REPRESENTATION + 4)?
        .checked_sub(address)
        .ok_or(Error::Invalid(
            "the player representations lie outside the globals",
        ))? as usize;
    let mut old = Vec::new();
    for e in players.elements() {
        if e >= count {
            return Err(Error::Invalid(
                "the globals have too few player representations",
            ));
        }
        let at = first + e * PLAYER_REPRESENTATION_SIZE + REPRESENTATION_UNIT;
        if at + 12 > bytes.len() {
            return Err(Error::Invalid(
                "the player representations lie outside the globals",
            ));
        }
        old.push(TagId(word(bytes, at + 4)?));
        bytes[at..at + 4].copy_from_slice(&FourCC(*b"bipd").as_u32().to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&biped.0.to_le_bytes());
        let w = at - REPRESENTATION_UNIT + REPRESENTATION_UNIT_WORD;
        bytes[w..w + 4].fill(0);
    }
    Ok(old)
}

/// Makes `hands` (a `mode`) the first-person arms of every player of
/// `players` in the globals of `set` (pinned). Returns the arms it
/// replaced.
pub fn set_player_hands(set: &mut TagSet, hands: TagId, players: Players) -> Result<Vec<TagId>> {
    let globals = set.globals;
    let tag = set
        .tag_mut(globals)
        .ok_or(Error::Invalid("the set has no globals"))?;
    let TagPayload::Pinned { address, bytes } = &mut tag.payload else {
        return Err(Error::Invalid("the base globals should be pinned"));
    };
    set_player_hands_in(bytes, *address, hands, players)
}

/// [`set_player_hands`] on the globals' bytes, whose first byte is at
/// virtual `address`.
pub fn set_player_hands_in(
    bytes: &mut [u8],
    address: u32,
    hands: TagId,
    players: Players,
) -> Result<Vec<TagId>> {
    let mut old = Vec::new();
    for at in representation_fields(bytes, address, players, REPRESENTATION_HANDS)? {
        old.push(TagId(u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap())));
        bytes[at..at + 4].copy_from_slice(&FourCC(*b"mode").as_u32().to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&hands.0.to_le_bytes());
    }
    Ok(old)
}

/// The byte offsets of field `field` (a reference) of `players`' player
/// representations in the globals' bytes.
fn representation_fields(
    bytes: &[u8],
    address: u32,
    players: Players,
    field: usize,
) -> Result<Vec<usize>> {
    let word = |at: usize| -> Result<u32> {
        bytes
            .get(at..at + 4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .ok_or(Error::Invalid("the globals are too short"))
    };
    let count = word(PLAYER_REPRESENTATION)? as usize;
    let first = word(PLAYER_REPRESENTATION + 4)?
        .checked_sub(address)
        .ok_or(Error::Invalid(
            "the player representations lie outside the globals",
        ))? as usize;
    players
        .elements()
        .into_iter()
        .map(|e| {
            if e >= count {
                return Err(Error::Invalid(
                    "the globals have too few player representations",
                ));
            }
            let at = first + e * PLAYER_REPRESENTATION_SIZE + field;
            if at + 8 > bytes.len() {
                return Err(Error::Invalid(
                    "the player representations lie outside the globals",
                ));
            }
            Ok(at)
        })
        .collect()
}

/// Halo 1's first-person arms: the model its globals' first-person
/// interface names ([`H1_FIRST_PERSON`]), if any.
pub fn h1_first_person_hands(h1: &h1map::H1Map) -> Result<Option<h1map::TagId>> {
    let Some(t) = h1
        .tags()
        .iter()
        .find(|t| t.group == h1map::Group::new(b"matg"))
    else {
        return Ok(None);
    };
    let b = h1.tag_data(t)?;
    let Some(blk) = h1map::Block::read(b, H1_FIRST_PERSON) else {
        return Ok(None);
    };
    if blk.count == 0 {
        return Ok(None);
    }
    // Only element 0's leading reference is read.
    let e = h1.read(blk.address, 16)?;
    Ok(h1map::TagRef::read(e, 0)
        .map(|r| r.id)
        .filter(|id| !id.is_none()))
}

/// `matg +0x100`: the grenades block (count, then address).
pub const GRENADES: usize = 0x100;
/// Its element size.
pub const GRENADE_SIZE: usize = 0x2c;
/// The element's maximum count (i16), equipment and projectile.
pub const GRENADE_MAXIMUM: usize = 0;
pub const GRENADE_EQUIPMENT: usize = 0x1c;
pub const GRENADE_PROJECTILE: usize = 0x24;
/// Halo 1's: `matg +0x128`, elements of `0x44`.
pub const H1_GRENADES: usize = 0x128;
pub const H1_GRENADE_SIZE: usize = 0x44;
pub const H1_GRENADE_EQUIPMENT: usize = 0x24;
pub const H1_GRENADE_PROJECTILE: usize = 0x34;

/// One grenade type of a map's globals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grenade<T> {
    pub maximum: i16,
    pub equipment: Option<T>,
    pub projectile: Option<T>,
}

/// Halo 1's grenade types, from its globals: per type the maximum, and
/// the equipment and projectile tags.
pub fn h1_grenades(h1: &h1map::H1Map) -> Result<Vec<Grenade<h1map::TagId>>> {
    let Some(t) = h1
        .tags()
        .iter()
        .find(|t| t.group == h1map::Group::new(b"matg"))
    else {
        return Ok(Vec::new());
    };
    let b = h1.tag_data(t)?;
    let blk =
        h1map::Block::read(b, H1_GRENADES).ok_or(Error::Invalid("Halo 1 globals: grenades"))?;
    let reference = |e: &[u8], at: usize| {
        h1map::TagRef::read(e, at)
            .map(|r| r.id)
            .filter(|id| !id.is_none())
    };
    Ok(h1
        .block(blk, H1_GRENADE_SIZE)?
        .chunks_exact(H1_GRENADE_SIZE)
        .map(|e| Grenade {
            maximum: i16::from_le_bytes([e[0], e[1]]),
            equipment: reference(e, H1_GRENADE_EQUIPMENT),
            projectile: reference(e, H1_GRENADE_PROJECTILE),
        })
        .collect())
}

/// Writes `grenades` over the grenade types of the globals of `set`
/// (pinned), element for element: the maximum, and the equipment and
/// projectile where given (the base's stay where not). Types past the
/// base's count are left out. Returns how many elements were written.
pub fn set_grenades(set: &mut TagSet, grenades: &[Grenade<TagId>]) -> Result<usize> {
    let globals = set.globals;
    let tag = set
        .tag_mut(globals)
        .ok_or(Error::Invalid("the set has no globals"))?;
    let TagPayload::Pinned { address, bytes } = &mut tag.payload else {
        return Err(Error::Invalid("the base globals should be pinned"));
    };
    set_grenades_in(bytes, *address, grenades)
}

/// [`set_grenades`] on the globals' bytes, whose first byte is at
/// virtual `address`.
pub fn set_grenades_in(
    bytes: &mut [u8],
    address: u32,
    grenades: &[Grenade<TagId>],
) -> Result<usize> {
    let word = |b: &[u8], at: usize| -> Result<u32> {
        b.get(at..at + 4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .ok_or(Error::Invalid("the globals are too short"))
    };
    let count = word(bytes, GRENADES)? as usize;
    let first = word(bytes, GRENADES + 4)?
        .checked_sub(address)
        .ok_or(Error::Invalid("the grenades lie outside the globals"))? as usize;
    let n = count.min(grenades.len());
    if first + n * GRENADE_SIZE > bytes.len() {
        return Err(Error::Invalid("the grenades lie outside the globals"));
    }
    for (i, g) in grenades.iter().take(n).enumerate() {
        let at = first + i * GRENADE_SIZE;
        bytes[at + GRENADE_MAXIMUM..at + GRENADE_MAXIMUM + 2]
            .copy_from_slice(&g.maximum.to_le_bytes());
        for (field, group, id) in [
            (GRENADE_EQUIPMENT, b"eqip", g.equipment),
            (GRENADE_PROJECTILE, b"proj", g.projectile),
        ] {
            if let Some(id) = id {
                let r = at + field;
                bytes[r..r + 4].copy_from_slice(&FourCC(*group).as_u32().to_le_bytes());
                bytes[r + 4..r + 8].copy_from_slice(&id.0.to_le_bytes());
            }
        }
    }
    Ok(n)
}

/// `matg +0x168`: the multiplayer globals (`mulg`), a tag reference.
pub const MULTIPLAYER_GLOBALS: usize = 0x168;
/// `mulg +8`: its universal block; element 0 holds the weapon list.
pub const MULG_UNIVERSAL: usize = 0x8;
/// Universal `+0x68`: the weapon list (count, then address), elements of
/// 8 bytes, each a tag reference (group, then datum at `+4`). E:
/// `0x0015eb80` finds a weapon's index in it, and `0x0019eda0` gives a
/// variant's starting weapon from it ([`STARTING_WEAPON_ENTRY`]).
pub const UNIVERSAL_WEAPONS: usize = 0x68;
pub const WEAPON_LIST_ENTRY: usize = 8;

/// A game variant's starting weapon byte (primary `+0xd6`, secondary
/// `+0xd7`; `variants` settings 89 and 94) and the weapon list entry it
/// spawns with. E: `0x0019eda0` (called from `0x0019f03d`..`0x0019f076`
/// with the game's copies at `[0x4e6948]+0x212`/`+0x213`) jumps on the
/// byte minus 1 through the table at `0x0019eee8`: 1 gives none, 2 a
/// random byte in 3..=18, and each other byte one entry. Bytes past 20 (or
/// below 1) keep the caller's default.
pub const STARTING_WEAPON_ENTRY: [(u8, usize); 18] = [
    (3, 3),
    (4, 0),
    (5, 1),
    (6, 6),
    (7, 7),
    (8, 5),
    (9, 2),
    (10, 4),
    (11, 12),
    (12, 9),
    (13, 10),
    (14, 13),
    (15, 11),
    (16, 15),
    (17, 14),
    (18, 8),
    (19, 16),
    (20, 17),
];

fn word_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .ok_or(Error::Invalid("the multiplayer globals are too short"))
}

/// Where the weapon list lies in the `mulg` bytes whose first byte is at
/// virtual `address`: its first entry's offset and its count.
fn weapon_list_at(bytes: &[u8], address: u32) -> Result<(usize, usize)> {
    let outside = Error::Invalid("the weapon list lies outside the multiplayer globals");
    if word_at(bytes, MULG_UNIVERSAL)? == 0 {
        return Err(Error::Invalid("the multiplayer globals have no universal element"));
    }
    let u = word_at(bytes, MULG_UNIVERSAL + 4)?
        .checked_sub(address)
        .ok_or(Error::Invalid("the universal element lies outside the multiplayer globals"))?
        as usize;
    let n = word_at(bytes, u + UNIVERSAL_WEAPONS)? as usize;
    if n == 0 {
        return Ok((0, 0));
    }
    let first = word_at(bytes, u + UNIVERSAL_WEAPONS + 4)?
        .checked_sub(address)
        .ok_or(Error::Invalid("the weapon list lies outside the multiplayer globals"))?
        as usize;
    if first + n * WEAPON_LIST_ENTRY > bytes.len() {
        return Err(outside);
    }
    Ok((first, n))
}

/// The weapon list of the `mulg` bytes at virtual `address`: each entry's
/// datum (`None` for none).
pub fn weapon_list(bytes: &[u8], address: u32) -> Result<Vec<Option<TagId>>> {
    let (first, n) = weapon_list_at(bytes, address)?;
    (0..n)
        .map(|i| {
            let d = word_at(bytes, first + i * WEAPON_LIST_ENTRY + 4)?;
            Ok((d != u32::MAX).then_some(TagId(d)))
        })
        .collect()
}

/// Points weapon list entries at weapons: each `(entry, weap datum)`.
/// Returns how many it wrote; an entry past the list is an error.
pub fn set_weapon_list_in(bytes: &mut [u8], address: u32, entries: &[(usize, TagId)]) -> Result<usize> {
    let (first, n) = weapon_list_at(bytes, address)?;
    for (i, id) in entries {
        if *i >= n {
            return Err(Error::Invalid("a weapon list entry past the list"));
        }
        let at = first + i * WEAPON_LIST_ENTRY;
        bytes[at..at + 4].copy_from_slice(&FourCC(*b"weap").as_u32().to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&id.0.to_le_bytes());
    }
    Ok(entries.len())
}

/// [`set_weapon_list_in`] on the `mulg` the globals of `set` name (both
/// pinned, as `TagSet::from_map` leaves them).
pub fn set_weapon_list(set: &mut TagSet, entries: &[(usize, TagId)]) -> Result<usize> {
    let globals = set.globals;
    let tag = set
        .tag_mut(globals)
        .ok_or(Error::Invalid("the set has no globals"))?;
    let TagPayload::Pinned { bytes, .. } = &tag.payload else {
        return Err(Error::Invalid("the base globals should be pinned"));
    };
    let mulg = TagId(word_at(bytes, MULTIPLAYER_GLOBALS + 4)?);
    let tag = set
        .tag_mut(mulg)
        .ok_or(Error::Invalid("the globals name no multiplayer globals"))?;
    let TagPayload::Pinned { address, bytes } = &mut tag.payload else {
        return Err(Error::Invalid("the base multiplayer globals should be pinned"));
    };
    set_weapon_list_in(bytes, *address, entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented globals: four representations right after a 0x284 root.
    fn globals(address: u32) -> Vec<u8> {
        let mut b = vec![0u8; 0x284 + 4 * PLAYER_REPRESENTATION_SIZE];
        b[PLAYER_REPRESENTATION..PLAYER_REPRESENTATION + 4].copy_from_slice(&4u32.to_le_bytes());
        b[PLAYER_REPRESENTATION + 4..PLAYER_REPRESENTATION + 8]
            .copy_from_slice(&(address + 0x284).to_le_bytes());
        for e in 0..4 {
            let at = 0x284 + e * PLAYER_REPRESENTATION_SIZE + REPRESENTATION_UNIT;
            b[at + 4..at + 8].copy_from_slice(&(0xe000_0010 + e as u32).to_le_bytes());
            b[at + 8..at + 12].copy_from_slice(&7u32.to_le_bytes());
        }
        b
    }

    fn unit(b: &[u8], e: usize) -> (u32, u32, u32) {
        let at = 0x284 + e * PLAYER_REPRESENTATION_SIZE + REPRESENTATION_UNIT;
        let w = |o: usize| u32::from_le_bytes(b[at + o..at + o + 4].try_into().unwrap());
        (w(0), w(4), w(8))
    }

    #[test]
    fn multiplayer_players_spawn_as_the_biped_and_campaigns_keep_theirs() {
        let mut b = globals(0x8000_1000);
        let old = set_player_unit_in(
            &mut b,
            0x8000_1000,
            TagId(0xe123_0042),
            Players::Multiplayer,
        )
        .unwrap();
        assert_eq!(old, [TagId(0xe000_0012), TagId(0xe000_0013)]);
        let bipd = FourCC(*b"bipd").as_u32();
        for e in [2, 3] {
            assert_eq!(unit(&b, e), (bipd, 0xe123_0042, 0));
        }
        for e in [0, 1] {
            assert_eq!(unit(&b, e), (0, 0xe000_0010 + e as u32, 7));
        }
        let mut c = globals(0x8000_1000);
        set_player_unit_in(&mut c, 0x8000_1000, TagId(0xe123_0042), Players::Campaign).unwrap();
        assert_eq!(unit(&c, 0), (bipd, 0xe123_0042, 0));
        assert_eq!(unit(&c, 2), (0, 0xe000_0012, 7));
    }

    #[test]
    fn the_players_arms_are_set_and_the_rest_of_the_element_kept() {
        let mut b = globals(0x8000_1000);
        let old = set_player_hands_in(&mut b, 0x8000_1000, TagId(0xe123_0050), Players::Campaign).unwrap();
        assert_eq!(old, [TagId(0), TagId(0)]);
        let mode = FourCC(*b"mode").as_u32();
        let hands = |b: &[u8], e: usize| {
            let at = 0x284 + e * PLAYER_REPRESENTATION_SIZE + REPRESENTATION_HANDS;
            (
                u32::from_le_bytes(b[at..at + 4].try_into().unwrap()),
                u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()),
            )
        };
        for e in [0, 1] {
            assert_eq!(hands(&b, e), (mode, 0xe123_0050));
            assert_eq!(unit(&b, e), (0, 0xe000_0010 + e as u32, 7));
        }
        for e in [2, 3] {
            assert_eq!(hands(&b, e), (0, 0));
        }
        assert!(set_player_hands_in(&mut b, 0x8000_2000, TagId(1), Players::Campaign).is_err());
    }

    #[test]
    fn a_block_outside_the_globals_or_too_short_is_refused() {
        let mut b = globals(0x8000_1000);
        assert!(set_player_unit_in(&mut b, 0x8000_2000, TagId(1), Players::Campaign).is_err());
        b[PLAYER_REPRESENTATION..PLAYER_REPRESENTATION + 4].copy_from_slice(&2u32.to_le_bytes());
        assert!(set_player_unit_in(&mut b, 0x8000_1000, TagId(1), Players::Multiplayer).is_err());
        assert!(set_player_unit_in(&mut b, 0x8000_1000, TagId(1), Players::Campaign).is_ok());
        b.truncate(0x284 + PLAYER_REPRESENTATION_SIZE);
        assert!(set_player_unit_in(&mut b, 0x8000_1000, TagId(1), Players::Campaign).is_err());
    }

    #[test]
    fn grenades_are_written_element_for_element_and_the_rest_kept() {
        let address: u32 = 0x8000_1000;
        let mut b = vec![0u8; 0x200 + 2 * GRENADE_SIZE];
        b[GRENADES..GRENADES + 4].copy_from_slice(&2u32.to_le_bytes());
        b[GRENADES + 4..GRENADES + 8].copy_from_slice(&(address + 0x200).to_le_bytes());
        for e in 0..2 {
            let at = 0x200 + e * GRENADE_SIZE;
            b[at..at + 2].copy_from_slice(&4i16.to_le_bytes());
            b[at + GRENADE_EQUIPMENT + 4..at + GRENADE_EQUIPMENT + 8]
                .copy_from_slice(&(0xe000_0010 + e as u32).to_le_bytes());
            b[at + GRENADE_PROJECTILE + 4..at + GRENADE_PROJECTILE + 8]
                .copy_from_slice(&(0xe000_0020 + e as u32).to_le_bytes());
        }
        let g = |m: i16, e: Option<u32>, p: Option<u32>| Grenade::<TagId> {
            maximum: m,
            equipment: e.map(TagId),
            projectile: p.map(TagId),
        };
        let n = set_grenades_in(
            &mut b,
            address,
            &[
                g(2, Some(0xe123_0001), Some(0xe123_0002)),
                g(3, None, Some(0xe123_0003)),
                g(9, Some(1u32), Some(1u32)),
            ],
        )
        .unwrap();
        assert_eq!(n, 2, "a third type has no element");
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let (e0, e1) = (0x200, 0x200 + GRENADE_SIZE);
        assert_eq!(i16::from_le_bytes([b[e0], b[e0 + 1]]), 2);
        assert_eq!(i16::from_le_bytes([b[e1], b[e1 + 1]]), 3);
        assert_eq!(
            (w(e0 + GRENADE_EQUIPMENT), w(e0 + GRENADE_EQUIPMENT + 4)),
            (FourCC(*b"eqip").as_u32(), 0xe123_0001)
        );
        assert_eq!(
            (w(e0 + GRENADE_PROJECTILE), w(e0 + GRENADE_PROJECTILE + 4)),
            (FourCC(*b"proj").as_u32(), 0xe123_0002)
        );
        assert_eq!(
            w(e1 + GRENADE_EQUIPMENT + 4),
            0xe000_0011,
            "no equipment: the base's stays"
        );
        assert_eq!(w(e1 + GRENADE_PROJECTILE + 4), 0xe123_0003);
        assert!(set_grenades_in(&mut b, address + 0x1000, &[]).is_err());
    }

    /// Invented `mulg`: a universal element at `+0x40`, a three-entry
    /// weapon list at `+0x100`.
    fn mulg(address: u32) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, MULG_UNIVERSAL, 1);
        put(&mut b, MULG_UNIVERSAL + 4, address + 0x40);
        put(&mut b, 0x40 + UNIVERSAL_WEAPONS, 3);
        put(&mut b, 0x40 + UNIVERSAL_WEAPONS + 4, address + 0x100);
        for (i, d) in [0xe000_0001u32, 0xe000_0002, u32::MAX].into_iter().enumerate() {
            put(&mut b, 0x100 + i * 8, if d == u32::MAX { u32::MAX } else { FourCC(*b"weap").as_u32() });
            put(&mut b, 0x100 + i * 8 + 4, d);
        }
        b
    }

    #[test]
    fn weapon_list_entries_point_at_the_given_weapons() {
        let a = 0x8000_2000;
        let mut b = mulg(a);
        assert_eq!(
            weapon_list(&b, a).unwrap(),
            [Some(TagId(0xe000_0001)), Some(TagId(0xe000_0002)), None]
        );
        set_weapon_list_in(&mut b, a, &[(1, TagId(0xe042_0007)), (2, TagId(0xe042_0008))]).unwrap();
        assert_eq!(
            weapon_list(&b, a).unwrap(),
            [Some(TagId(0xe000_0001)), Some(TagId(0xe042_0007)), Some(TagId(0xe042_0008))]
        );
        assert_eq!(&b[0x110..0x114], &FourCC(*b"weap").as_u32().to_le_bytes());
        assert!(set_weapon_list_in(&mut b, a, &[(3, TagId(1))]).is_err());
    }

    /// Every starting weapon byte names a distinct entry.
    #[test]
    fn starting_weapon_bytes_name_distinct_entries() {
        let mut e: Vec<_> = STARTING_WEAPON_ENTRY.iter().map(|x| x.1).collect();
        e.sort();
        e.dedup();
        assert_eq!(e, (0..18).collect::<Vec<_>>());
    }
}
