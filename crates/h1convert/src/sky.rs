//! The Halo 1 level's own skies (`h1sky`) in place of the base's.
//!
//! Each sky the Halo 1 scenario names (`scnr` skies) is converted with its
//! model, lights and fog by [`h1sky::convert::sky_from_map`] and added as a
//! new tag. The base scenario's skies block (`scnr +0x08`, count; `+0x0c`,
//! pointer; 8-byte tag references) keeps its length: element `k` names
//! Halo 1's sky `k`, or Halo 1's last when the base has more, so no element
//! still names a base sky. Converted clusters all draw sky 0
//! ([`crate::structure`]).

use map::build::TagSet;
use map::{FourCC, TagId};

use crate::{Error, Result};

/// `scnr` skies block field (count; pointer at `+4`) and element size.
pub const SKIES: usize = 0x08;
/// Skies element: an 8-byte tag reference.
pub const SKY: usize = 8;

/// One converted sky, ready to add.
#[derive(Debug, Clone)]
pub struct Sky {
    /// The Halo 1 sky's tag path (the new tag's name).
    pub name: String,
    pub sky: h1sky::h1::Sky,
    /// Its render model's path and conversion.
    pub model: Option<(String, h1models::Converted)>,
}

/// The level's skies, and why any didn't convert.
#[derive(Debug, Clone, Default)]
pub struct Skies {
    pub skies: Vec<Sky>,
    pub notes: Vec<String>,
}

/// Converts every sky the Halo 1 scenario names.
pub fn read(h1: &h1map::H1Map) -> Skies {
    let mut out = Skies::default();
    let refs = match h1.multiplayer() {
        Ok(mp) => mp.skies,
        Err(e) => {
            out.notes
                .push(format!("the scenario's skies don't read: {e}"));
            return out;
        }
    };
    for r in refs {
        let Ok(tag) = h1.tag(r.id) else { continue };
        match h1sky::convert::sky_from_map(h1, tag) {
            Ok((sky, model)) => out.skies.push(Sky {
                name: tag.name.clone(),
                sky,
                model,
            }),
            Err(e) => out.notes.push(format!("{}: {e}", tag.name)),
        }
    }
    out
}

impl Skies {
    /// Adds each sky (and its model) to `set`; returns their datums.
    pub fn install(&self, set: &mut TagSet) -> Result<Vec<TagId>> {
        self.skies
            .iter()
            .map(|s| {
                h1sky::convert::install_sky(
                    set,
                    &s.sky,
                    s.model.as_ref().map(|(n, c)| (n.as_str(), c.clone())),
                    &s.name,
                )
                .map_err(|e| Error::Sky(format!("{}: {e}", s.name)))
            })
            .collect()
    }
}

/// Points every element of the skies block of a scenario payload (loaded
/// at `address`) at `ids` (module docs). Returns the elements rewritten.
pub fn patch_scenario(scnr: &mut [u8], address: u32, ids: &[TagId]) -> Result<usize> {
    let Some(&last) = ids.last() else {
        return Ok(0);
    };
    let word = |b: &[u8], at: usize| -> Result<u32> {
        b.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .ok_or(Error::Invalid("scenario skies outside the tag"))
    };
    let count = word(scnr, SKIES)? as usize;
    if count == 0 {
        return Ok(0);
    }
    let start = word(scnr, SKIES + 4)?.wrapping_sub(address) as usize;
    if start + count * SKY > scnr.len() {
        return Err(Error::Invalid("scenario skies outside the tag"));
    }
    for k in 0..count {
        let id = ids.get(k).copied().unwrap_or(last);
        let at = start + k * SKY;
        scnr[at..at + 4].copy_from_slice(&FourCC(*b"sky ").as_u32().to_le_bytes());
        scnr[at + 4..at + 8].copy_from_slice(&id.0.to_le_bytes());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sky_element_names_a_halo1_sky() {
        let address: u32 = 0x1000;
        let mut b = vec![0u8; 0x40];
        b[SKIES..SKIES + 4].copy_from_slice(&3u32.to_le_bytes());
        b[SKIES + 4..SKIES + 8].copy_from_slice(&(address + 0x20).to_le_bytes());
        let ids = [TagId(0xe1000010), TagId(0xe1010011)];
        assert_eq!(patch_scenario(&mut b, address, &ids).unwrap(), 3);
        let datum =
            |k: usize| u32::from_le_bytes(b[0x24 + 8 * k..0x28 + 8 * k].try_into().unwrap());
        assert_eq!(
            [datum(0), datum(1), datum(2)],
            [ids[0].0, ids[1].0, ids[1].0]
        );
        assert_eq!(b[0x20..0x24], FourCC(*b"sky ").as_u32().to_le_bytes());
        assert_eq!(patch_scenario(&mut b, address, &[]).unwrap(), 0);
    }
}
