//! Player starting profiles (`scnr +0xf8` in Halo 2): Halo 1's, with each
//! weapon its converted tag.
//!
//! Halo 1 (`scnr +0x348`, `0x68` each; **S**, the editing kits' scenario
//! layout, checked by `h1_starting_profiles_read` on all ten levels: every
//! weapon reference names a `weap` or none): name (32), health modifier
//! `f32 +0x20`, shield modifier `+0x24`, primary weapon reference (16 bytes)
//! `+0x28`, its rounds loaded and total `i16 +0x38`, `+0x3a`, secondary
//! weapon `+0x3c`, `+0x4c`, `+0x4e`, fragmentation and plasma grenades
//! `i8 +0x50`, `+0x51`.
//!
//! Halo 2 (`0x44` each; **V** on 05a's and 03a's profiles): name (32), the
//! two modifiers `+0x20`, `+0x24`, primary weapon reference (8 bytes)
//! `+0x28`, rounds `i16 +0x30`, `+0x32`, secondary `+0x34`, `+0x3c`,
//! `+0x3e`, fragmentation and plasma grenades `u8 +0x40`, `+0x41`.

use h1map::H1Map;
use map::TagId;
use map::build::Block;

use crate::Result;

/// Halo 1's block field and stride.
pub const H1: (usize, usize) = (0x348, 0x68);
/// Halo 2's block field and stride.
pub const H2: (u32, usize) = (0xf8, 0x44);

/// One Halo 1 profile, its weapons as Halo 1 tags.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub health: f32,
    pub shield: f32,
    /// (weapon, rounds loaded, rounds total), primary then secondary.
    pub weapons: [(Option<h1map::TagId>, i16, i16); 2],
    pub frags: u8,
    pub plasmas: u8,
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().expect("4"))
}

/// The level's starting profiles.
pub fn read(h1: &H1Map) -> Result<Vec<Profile>> {
    let scnr = h1.tag_data(h1.scenario())?;
    let elems = h1.tag_space().block_at(scnr, H1.0, H1.1)?;
    Ok(elems
        .into_iter()
        .map(|e| {
            let end = e[..32].iter().position(|&c| c == 0).unwrap_or(32);
            let weapon = |at: usize| {
                let r = h1map::TagRef::read(e, at)?;
                (!r.id.is_none()).then_some(r.id)
            };
            Profile {
                name: String::from_utf8_lossy(&e[..end]).into_owned(),
                health: f32_at(e, 0x20),
                shield: f32_at(e, 0x24),
                weapons: [
                    (weapon(0x28), i16_at(e, 0x38), i16_at(e, 0x3a)),
                    (weapon(0x3c), i16_at(e, 0x4c), i16_at(e, 0x4e)),
                ],
                frags: e[0x50],
                plasmas: e[0x51],
            }
        })
        .collect())
}

/// Halo 2's elements, each Halo 1 weapon given by `weapon` (its converted
/// tag, or none).
pub fn elements(
    profiles: &[Profile],
    mut weapon: impl FnMut(h1map::TagId) -> Option<TagId>,
) -> Vec<Block> {
    profiles
        .iter()
        .map(|p| {
            let mut b = Block::zeroed(H2.1);
            let name = p.name.as_bytes();
            let n = name.len().min(31);
            b.bytes[..n].copy_from_slice(&name[..n]);
            b.put_f32(0x20, p.health).put_f32(0x24, p.shield);
            for (k, (w, loaded, total)) in p.weapons.iter().enumerate() {
                let at = 0x28 + 0xc * k;
                b.tag_ref(at as u32, w.and_then(&mut weapon).unwrap_or(TagId::NULL));
                b.put_u16(at + 8, *loaded as u16)
                    .put_u16(at + 10, *total as u16);
            }
            b.bytes[0x40] = p.frags;
            b.bytes[0x41] = p.plasmas;
            b
        })
        .collect()
}

/// Halo 2's equipment evaluator reads a signed profile index from the
/// argument's low word and indexes the rebuilt scenario block (`0x0cce00`).
/// The rebuild keeps the Halo 1 block order.
pub fn script_index(profiles: &[Profile], name: &str) -> Option<u32> {
    if name.eq_ignore_ascii_case("none") {
        return Some(u32::MAX);
    }
    let index = profiles.iter().position(|p| p.name.eq_ignore_ascii_case(name))?;
    Some(u32::from(i16::try_from(index).ok()? as u16))
}
