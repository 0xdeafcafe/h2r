//! The Halo 1 level's own background sounds and sound environments in
//! place of the base's.
//!
//! Halo 1 keeps both palettes in the structure (D, every BSP of Blood
//! Gulch and a10): background sounds at `sbsp +0x1fc` (`0x74` each: a
//! 32-byte name, the `lsnd` reference at `+0x20`), sound environments at
//! `+0x208` (`0x50` each: name, the `snde` at `+0x20`). Halo 2 keeps them
//! in the scenario: background sounds at `scnr +0x250` (`0x64` each: name,
//! `lsnd` at `+0x20`, an inside-cluster `lsnd` at `+0x28`, cutoff distance
//! `+0x44`, scale flags `+0x48`, interior, portal and exterior scales
//! `+0x4c`..`+0x54`, interpolation speed `+0x58`), sound environments at
//! `+0x258` (`0x48` each: name, `snde` at `+0x20`, cutoff distance
//! `+0x28`, interpolation speed `+0x2c`) (schema; values D on Coagulation,
//! Beaver Creek and Old Mombasa). Halo 1 has no cutoff or interpolation;
//! the values written are ones retail uses (background 2, 1; environment
//! 1, 2).
//!
//! The base scenario stays pinned, so the elements are written into the
//! base's own block storage: as many as it holds (Coagulation 3 and 4,
//! Beaver Creek 2 and 3; a Halo 1 multiplayer level has 1 of each), and
//! the count shrinks to that. Clusters still name neither palette (-1,
//! [`crate::structure`]), so nothing plays yet; the cluster indices come
//! with the structure's next round.

use h1map::{H1Map, TagRef};
use h1sounds::looping::{Looping, SNDE, install_environment, install_looping};
use map::build::TagSet;
use map::{FourCC, TagId};

use crate::{Error, Result};

/// Halo 1 `sbsp` background sound palette (block field) and element size.
pub const H1_BACKGROUND_SOUNDS: usize = 0x1fc;
pub const H1_BACKGROUND_SOUND: usize = 0x74;
/// Halo 1 `sbsp` sound environment palette and element size.
pub const H1_SOUND_ENVIRONMENTS: usize = 0x208;
pub const H1_SOUND_ENVIRONMENT: usize = 0x50;
/// `scnr` background sound palette (block field) and element size.
pub const BACKGROUND_SOUNDS: usize = 0x250;
pub const BACKGROUND_SOUND: usize = 0x64;
/// `scnr` sound environment palette and element size.
pub const SOUND_ENVIRONMENTS: usize = 0x258;
pub const SOUND_ENVIRONMENT: usize = 0x48;

/// Palette entries the engine can name: the ambience pass keeps a slot
/// per entry in 64-entry arrays on its stack (E `0x0018d730`: `+0x38`,
/// `+0x138`, `+0x338`, 4 bytes each), indexed by the cluster's entry.
pub const MAX_PALETTE: usize = 64;

/// One Halo 1 sound environment, converted.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The palette entry's name.
    pub entry: String,
    /// The `snde` tag path (the new tag's name).
    pub name: String,
    pub tag: h1models::Converted,
}

/// The level's palettes, converted.
#[derive(Debug, Clone, Default)]
pub struct Ambience {
    /// Background sounds: the palette entry's name and its looping sound.
    pub backgrounds: Vec<(String, Looping)>,
    pub environments: Vec<Environment>,
    /// Per Halo 1 structure BSP, per entry of its own background sound
    /// palette, the entry's index in [`Self::backgrounds`] (`None`: it
    /// didn't convert). A cluster's `+0x04` indexes the BSP's palette.
    pub bsp_backgrounds: Vec<Vec<Option<usize>>>,
    /// The same for sound environments (cluster `+0x06`).
    pub bsp_environments: Vec<Vec<Option<usize>>>,
    /// What didn't convert.
    pub notes: Vec<String>,
}

/// What [`Ambience::install`] added: each palette entry's name and tag.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    pub backgrounds: Vec<(String, TagId)>,
    pub environments: Vec<(String, TagId)>,
}

fn entry_name(e: &[u8]) -> String {
    let n = &e[..32];
    let end = n.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&n[..end]).into_owned()
}

/// Reads both palettes of every structure BSP (entries the BSPs share
/// once), converting each looping sound with its sounds.
pub fn read(h1: &H1Map) -> Ambience {
    read_with(h1, h1sounds::looping::read_looping)
}

/// Read the palettes when the campaign installs all looping sounds in a single pass.
pub fn read_campaign(h1: &H1Map) -> Ambience {
    read_with(h1, h1sounds::looping::read_definition)
}

fn read_with(
    h1: &H1Map,
    read_loop: fn(&H1Map, &h1map::Tag) -> h1sounds::Result<Looping>,
) -> Ambience {
    let mut out = Ambience::default();
    let bsps = match h1.structure_bsps() {
        Ok(b) => b,
        Err(e) => {
            out.notes
                .push(format!("the structure BSPs don't read: {e}"));
            return out;
        }
    };
    for bsp in bsps {
        out.bsp_backgrounds.push(Vec::new());
        out.bsp_environments.push(Vec::new());
        let s = match h1.structure_bsp(&bsp) {
            Ok(s) => s,
            Err(e) => {
                out.notes.push(format!("{}: {e}", bsp.name));
                continue;
            }
        };
        let palette = |at, len| s.space.block_at(s.root, at, len);
        match palette(H1_BACKGROUND_SOUNDS, H1_BACKGROUND_SOUND) {
            Ok(es) => {
                for e in es {
                    let entry = entry_name(e);
                    let Some(t) = TagRef::read(e, 0x20).and_then(|r| h1.tag(r.id).ok()) else {
                        out.notes.push(format!("background sound {entry}: no lsnd"));
                        push_index(&mut out.bsp_backgrounds, None);
                        continue;
                    };
                    if let Some(k) = out
                        .backgrounds
                        .iter()
                        .position(|(n, l)| *n == entry && l.name == t.name)
                    {
                        push_index(&mut out.bsp_backgrounds, Some(k));
                        continue;
                    }
                    match read_loop(h1, t) {
                        Ok(l) => {
                            out.notes.extend(l.notes.iter().cloned());
                            out.backgrounds.push((entry, l));
                            let k = out.backgrounds.len() - 1;
                            push_index(&mut out.bsp_backgrounds, Some(k));
                        }
                        Err(e) => {
                            out.notes.push(format!("{}: {e}", t.name));
                            push_index(&mut out.bsp_backgrounds, None);
                        }
                    }
                }
            }
            Err(e) => out
                .notes
                .push(format!("{}: background sounds: {e}", bsp.name)),
        }
        match palette(H1_SOUND_ENVIRONMENTS, H1_SOUND_ENVIRONMENT) {
            Ok(es) => {
                for e in es {
                    let entry = entry_name(e);
                    let Some(t) = TagRef::read(e, 0x20)
                        .and_then(|r| h1.tag(r.id).ok())
                        .filter(|t| t.group == SNDE)
                    else {
                        out.notes
                            .push(format!("sound environment {entry}: no snde"));
                        push_index(&mut out.bsp_environments, None);
                        continue;
                    };
                    if let Some(k) = out
                        .environments
                        .iter()
                        .position(|v| v.entry == entry && v.name == t.name)
                    {
                        push_index(&mut out.bsp_environments, Some(k));
                        continue;
                    }
                    match h1sounds::looping::read_environment(h1, t) {
                        Ok(tag) => {
                            out.environments.push(Environment {
                                entry,
                                name: t.name.clone(),
                                tag,
                            });
                            let k = out.environments.len() - 1;
                            push_index(&mut out.bsp_environments, Some(k));
                        }
                        Err(e) => {
                            out.notes.push(format!("{}: {e}", t.name));
                            push_index(&mut out.bsp_environments, None);
                        }
                    }
                }
            }
            Err(e) => out
                .notes
                .push(format!("{}: sound environments: {e}", bsp.name)),
        }
    }
    out
}

fn push_index(per_bsp: &mut [Vec<Option<usize>>], k: Option<usize>) {
    per_bsp.last_mut().expect("one per BSP").push(k);
}

impl Ambience {
    /// Halo 1 BSP `bsp`'s background sound and sound environment palette
    /// indices (a cluster's `+0x04`, `+0x06`; -1 none), as indices into
    /// the converted palettes of which the first `written` entries were
    /// written; -1 for none, or an entry that didn't convert or wasn't
    /// written.
    pub fn index(&self, bsp: usize, h1: [i16; 2], written: [usize; 2]) -> [i16; 2] {
        let maps = [&self.bsp_backgrounds, &self.bsp_environments];
        let mut out = [-1; 2];
        for k in 0..2 {
            let Ok(i) = usize::try_from(h1[k]) else {
                continue;
            };
            if let Some(&Some(j)) = maps[k].get(bsp).and_then(|m| m.get(i))
                && j < written[k].min(MAX_PALETTE)
            {
                out[k] = j as i16;
            }
        }
        out
    }

    /// Adds every looping sound (with its sounds) and sound environment.
    pub fn install(&self, set: &mut TagSet) -> Result<Installed> {
        let loops: Vec<Looping> = self.backgrounds.iter().map(|(_, l)| l.clone()).collect();
        let ids = install_looping(set, &loops).map_err(|e| Error::Sound(e.to_string()))?;
        let backgrounds = self
            .backgrounds
            .iter()
            .map(|(n, _)| n.clone())
            .zip(ids)
            .collect();
        let mut environments = Vec::new();
        for v in &self.environments {
            let id = install_environment(set, &v.name, &v.tag)
                .map_err(|e| Error::Sound(e.to_string()))?;
            environments.push((v.entry.clone(), id));
        }
        Ok(Installed {
            backgrounds,
            environments,
        })
    }
}

fn word(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
        .ok_or(Error::Invalid("scenario sound palette outside the tag"))
}

/// One background sound palette element (`0x64`) naming looping sound
/// `id` as entry `name`; the scenario's and the structure's (`sbsp +0xd4`)
/// hold the same.
pub fn background_entry(name: &str, id: TagId) -> Vec<u8> {
    let lsnd = FourCC(*b"lsnd").as_u32();
    let mut e = named(BACKGROUND_SOUND, name);
    put(&mut e, 0x20, lsnd);
    put(&mut e, 0x24, id.0);
    put(&mut e, 0x28, lsnd);
    put(&mut e, 0x2c, u32::MAX);
    put(&mut e, 0x44, 2.0f32.to_bits());
    put(&mut e, 0x58, 1.0f32.to_bits());
    e
}

/// One sound environment palette element (`0x48`, `sbsp +0xdc` too).
pub fn environment_entry(name: &str, id: TagId) -> Vec<u8> {
    let mut e = named(SOUND_ENVIRONMENT, name);
    put(&mut e, 0x20, FourCC(*b"snde").as_u32());
    put(&mut e, 0x24, id.0);
    put(&mut e, 0x28, 1.0f32.to_bits());
    put(&mut e, 0x2c, 2.0f32.to_bits());
    e
}

fn named(len: usize, name: &str) -> Vec<u8> {
    let mut e = vec![0u8; len];
    let bytes = name.as_bytes();
    let m = bytes.len().min(31);
    e[..m].copy_from_slice(&bytes[..m]);
    e
}

impl Installed {
    /// At most [`MAX_PALETTE`] of each, as [`written`] counts them.
    pub fn bounded(&self) -> Installed {
        Installed {
            backgrounds: self.backgrounds.iter().take(MAX_PALETTE).cloned().collect(),
            environments: self
                .environments
                .iter()
                .take(MAX_PALETTE)
                .cloned()
                .collect(),
        }
    }

    /// The palette elements, background sounds then sound environments,
    /// at most `room` of each (and [`MAX_PALETTE`]).
    pub fn palettes(&self, room: [usize; 2]) -> [Vec<Vec<u8>>; 2] {
        let take = |k: usize, n: usize| n.min(room[k]).min(MAX_PALETTE);
        [
            self.backgrounds
                .iter()
                .take(take(0, self.backgrounds.len()))
                .map(|(n, id)| background_entry(n, *id))
                .collect(),
            self.environments
                .iter()
                .take(take(1, self.environments.len()))
                .map(|(n, id)| environment_entry(n, *id))
                .collect(),
        ]
    }
}

/// How many entries of each palette a rebuilt scenario holds: all of
/// them, up to [`MAX_PALETTE`].
pub fn written(a: &Ambience) -> [usize; 2] {
    [
        a.backgrounds.len().min(MAX_PALETTE),
        a.environments.len().min(MAX_PALETTE),
    ]
}

/// How many elements the scenario payload's palettes have room for: the
/// base's own counts (its storage is reused in place).
pub fn room(scnr: &[u8]) -> Result<[usize; 2]> {
    Ok([
        word(scnr, BACKGROUND_SOUNDS)? as usize,
        word(scnr, SOUND_ENVIRONMENTS)? as usize,
    ])
}

/// Writes `entries` into the palette block at `field` of a scenario
/// payload loaded at `address`, `len`-byte elements, into the base's
/// storage (they are at most as many as it holds, [`Installed::palettes`]
/// with [`room`]); the count becomes theirs. Returns it.
fn write_palette(
    scnr: &mut [u8],
    address: u32,
    field: usize,
    len: usize,
    entries: &[Vec<u8>],
) -> Result<u32> {
    let room = word(scnr, field)? as usize;
    let n = room.min(entries.len());
    if n == 0 {
        scnr[field..field + 8].fill(0);
        return Ok(0);
    }
    let start = word(scnr, field + 4)?.wrapping_sub(address) as usize;
    if start + room * len > scnr.len() {
        return Err(Error::Invalid("scenario sound palette outside the tag"));
    }
    for (k, e) in entries.iter().take(n).enumerate() {
        scnr[start + k * len..start + (k + 1) * len].copy_from_slice(e);
    }
    scnr[field..field + 4].copy_from_slice(&(n as u32).to_le_bytes());
    Ok(n as u32)
}

fn put(e: &mut [u8], at: usize, v: u32) {
    e[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Points the scenario's background sound and sound environment palettes
/// at the Halo 1 level's (module docs). Returns the entries written, per
/// palette.
pub fn patch_scenario(scnr: &mut [u8], address: u32, installed: &Installed) -> Result<[u32; 2]> {
    let [backgrounds, environments] = installed.palettes(room(scnr)?);
    Ok([
        write_palette(
            scnr,
            address,
            BACKGROUND_SOUNDS,
            BACKGROUND_SOUND,
            &backgrounds,
        )?,
        write_palette(
            scnr,
            address,
            SOUND_ENVIRONMENTS,
            SOUND_ENVIRONMENT,
            &environments,
        )?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(address: u32, backgrounds: u32, environments: u32) -> Vec<u8> {
        let mut b = vec![0xaau8; 0x600];
        let mut at = 0x300u32;
        for (field, n, len) in [
            (BACKGROUND_SOUNDS, backgrounds, BACKGROUND_SOUND),
            (SOUND_ENVIRONMENTS, environments, SOUND_ENVIRONMENT),
        ] {
            put(&mut b, field, n);
            put(&mut b, field + 4, address + at);
            at += n * len as u32;
        }
        b
    }

    #[test]
    fn the_palettes_name_halo1s_entries_in_the_bases_storage() {
        let address = 0x8000_0000;
        let mut b = scenario(address, 3, 2);
        let installed = Installed {
            backgrounds: vec![("sound".into(), TagId(0xe1230042))],
            environments: vec![
                ("mountains".into(), TagId(0xe1240043)),
                ("cave".into(), TagId(0xe1250044)),
                ("third".into(), TagId(0xe1260045)),
            ],
        };
        assert_eq!(patch_scenario(&mut b, address, &installed).unwrap(), [1, 2]);
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(w(BACKGROUND_SOUNDS), 1);
        let e = 0x300;
        assert_eq!(&b[e..e + 6], b"sound\0");
        assert_eq!(w(e + 0x20), FourCC(*b"lsnd").as_u32());
        assert_eq!(w(e + 0x24), 0xe1230042);
        assert_eq!(w(e + 0x2c), u32::MAX);
        assert_eq!(f32::from_bits(w(e + 0x44)), 2.0);
        // The base held 2 environments: Halo 1's third is left out.
        assert_eq!(w(SOUND_ENVIRONMENTS), 2);
        let e = 0x300 + 3 * BACKGROUND_SOUND;
        assert_eq!(w(e + 0x24), 0xe1240043);
        assert_eq!(w(e + SOUND_ENVIRONMENT + 0x24), 0xe1250044);
        assert_eq!(
            &b[e + SOUND_ENVIRONMENT..e + SOUND_ENVIRONMENT + 5],
            b"cave\0"
        );
    }

    #[test]
    fn no_halo1_entries_empty_the_palette() {
        let address = 0x8000_0000;
        let mut b = scenario(address, 3, 4);
        let installed = Installed::default();
        assert_eq!(patch_scenario(&mut b, address, &installed).unwrap(), [0, 0]);
        assert!(
            b[BACKGROUND_SOUNDS..BACKGROUND_SOUNDS + 8]
                .iter()
                .all(|&c| c == 0)
        );
        assert!(
            b[SOUND_ENVIRONMENTS..SOUND_ENVIRONMENTS + 8]
                .iter()
                .all(|&c| c == 0)
        );
    }
}
