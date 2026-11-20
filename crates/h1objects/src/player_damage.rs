//! Install Halo 1's per-damage cyborg scales in Halo 2's native damage table.

use std::collections::{BTreeMap, BTreeSet};

use map::build::{GrownTag, TagPayload, TagSet};

use crate::base::{GLOBAL_MATERIAL_SIZE, GLOBAL_MATERIALS};
use crate::{Error, Result};

const DAMAGE_TABLE: usize = 0xd0;
const MATERIAL_KEYS: [&str; 2] = ["h1_cyborg_body", "h1_cyborg_shield"];
const MATERIAL_TYPES: [usize; 2] = [21, 22];
const H1_DAMAGE_SCALES: usize = 0x200;

/// Halo 1 indexes damage scales at jpt! +0x200 by the collision or shield
/// material type (0x0009505c and 0x00094a45 read descriptor +0x3c).
const H1_DAMAGE_SCALE_END: usize = H1_DAMAGE_SCALES + 4 * crate::arsenal::MATERIALS.len();
pub(crate) type Scales = [u32; 2];

pub(crate) fn scales(bytes: &[u8]) -> Result<Scales> {
    if bytes.len() < H1_DAMAGE_SCALE_END {
        return Err(Error::Truncated("jpt! material scales"));
    }
    Ok(MATERIAL_TYPES.map(|kind| word(bytes, H1_DAMAGE_SCALES + 4 * kind)))
}

pub(crate) fn damage_key([body, shield]: Scales) -> String {
    format!("h1_cyborg_{body:08x}_{shield:08x}")
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn error(reason: &str) -> Error {
    Error::Invalid(format!("Halo 1 cyborg damage table: {reason}"))
}

fn pinned_ranges(set: &TagSet) -> impl Iterator<Item = (u32, &[u8])> {
    set.slots
        .iter()
        .filter_map(|s| match &s.tag.as_ref()?.payload {
            TagPayload::Pinned { address, bytes } => Some((*address, bytes.as_slice())),
            _ => None,
        })
        .chain(set.pinned.iter().flat_map(|p| {
            p.unowned
                .iter()
                .map(|(address, bytes)| (*address, bytes.as_slice()))
        }))
}

fn read_pinned(set: &TagSet, address: u32, len: usize) -> Result<&[u8]> {
    pinned_ranges(set)
        .find_map(|(base, bytes)| {
            let i = address.checked_sub(base)? as usize;
            bytes.get(i..i.checked_add(len)?)
        })
        .ok_or_else(|| error("block outside pinned metadata"))
}

fn write_pinned(set: &mut TagSet, address: u32, value: u32) -> Result<()> {
    let ranges = set
        .slots
        .iter_mut()
        .filter_map(|s| match &mut s.tag.as_mut()?.payload {
            TagPayload::Pinned { address, bytes } => Some((*address, bytes.as_mut_slice())),
            _ => None,
        });
    let unowned = set.pinned.iter_mut().flat_map(|p| {
        p.unowned
            .iter_mut()
            .map(|(address, bytes)| (*address, bytes.as_mut_slice()))
    });
    for (base, bytes) in ranges.chain(unowned) {
        if let Some(i) = address.checked_sub(base).map(|i| i as usize)
            && let Some(dst) = bytes.get_mut(i..i + 4)
        {
            dst.copy_from_slice(&value.to_le_bytes());
            return Ok(());
        }
    }
    Err(error("field outside pinned metadata"))
}

fn block<'a>(set: &'a TagSet, field: &[u8], stride: usize) -> Result<&'a [u8]> {
    let count = word(field, 0) as usize;
    if count == 0 {
        return Ok(&[]);
    }
    let len = count
        .checked_mul(stride)
        .ok_or_else(|| error("block size overflow"))?;
    read_pinned(set, word(field, 4), len)
}

/// The damage lookup (0x000d5bc0) multiplies any hits from a two-by-two
/// lookup: jpt! +0x50/+0x54 select rows, global material +0x10/+0x14
/// select columns. Give the cyborg materials distinct secondary keys so
/// thin metals and other shields still use their own scales. Mirror each
/// former column's stock scale under the new key for existing rows.
pub(crate) fn install(set: &mut TagSet, profiles: &BTreeSet<Scales>) -> Result<()> {
    if profiles.is_empty() {
        return Ok(());
    }
    let Some(TagPayload::Pinned { bytes: globals, .. }) = set.tag(set.globals).map(|t| &t.payload)
    else {
        return Err(error("missing pinned globals"));
    };
    let table_field = globals
        .get(DAMAGE_TABLE..DAMAGE_TABLE + 8)
        .ok_or_else(|| error("missing damage table field"))?;
    let matrix = block(set, table_field, 8)?;
    if matrix.len() != 8 {
        return Err(error("expected one damage matrix"));
    }
    let matrix_address = word(table_field, 4);
    let material_field = globals
        .get(GLOBAL_MATERIALS..GLOBAL_MATERIALS + 8)
        .ok_or_else(|| error("missing materials field"))?;
    let materials = block(set, material_field, GLOBAL_MATERIAL_SIZE)?;
    let mut material_slots = Vec::new();
    for kind in MATERIAL_TYPES {
        let name = crate::arsenal::MATERIALS[kind].1;
        let mut matches = materials
            .chunks_exact(GLOBAL_MATERIAL_SIZE)
            .enumerate()
            .filter(|(_, material)| {
                let index = (word(material, 0) & 0x00ff_ffff) as usize;
                set.strings.get(index).and_then(Option::as_deref) == Some(name)
            });
        let (i, material) = matches
            .next()
            .ok_or_else(|| error(&format!("missing material {name}")))?;
        if matches.next().is_some() {
            return Err(error(&format!("duplicate material {name}")));
        }
        let address = word(material_field, 4) + (i * GLOBAL_MATERIAL_SIZE + 0x14) as u32;
        material_slots.push((address, word(material, 0x14)));
    }
    let mut rows = BTreeMap::new();
    // The arrays the new ones replace: the rows and each row's values.
    let mut replaced = Vec::new();
    let old_rows = block(set, matrix, 12)?;
    if !old_rows.is_empty() {
        replaced.push((word(matrix, 4), old_rows.len() as u32));
    }
    for row in old_rows.chunks_exact(12) {
        let old_values = block(set, &row[4..], 8)?;
        if !old_values.is_empty() {
            replaced.push((word(row, 8), old_values.len() as u32));
        }
        let values: BTreeMap<_, _> = old_values
            .chunks_exact(8)
            .map(|value| (word(value, 0), word(value, 4)))
            .collect();
        rows.insert(word(row, 0), values);
    }
    replaced.sort_unstable();
    replaced.dedup();

    let keys = [set.intern(MATERIAL_KEYS[0])?, set.intern(MATERIAL_KEYS[1])?];
    for row in rows.values_mut() {
        for ((_, original), key) in material_slots.iter().zip(keys) {
            if let Some(value) = row.get(original).copied() {
                row.insert(key, value);
            }
        }
    }
    for &profile in profiles {
        rows.insert(
            set.intern(&damage_key(profile))?,
            keys.into_iter().zip(profile).collect(),
        );
    }

    // The pinned intermediate retains all stock matg fields at their
    // original addresses. Append the new matrix arrays to metadata and
    // change the existing matrix header. The globals are recorded as grown
    // there (`PinnedImage::grown`): fresh builds read the new arrays
    // through the regular matg schema and leave the replaced rows out;
    // string IDs stay in the same order.
    let globals = set.globals;
    let pinned = set
        .pinned
        .as_ref()
        .ok_or_else(|| error("missing pinned metadata"))?;
    let offset = pinned
        .metadata_size
        .checked_next_multiple_of(16)
        .ok_or_else(|| error("metadata size overflow"))?;
    let address = pinned
        .metadata_base
        .checked_add(offset)
        .ok_or_else(|| error("metadata address overflow"))?;
    let mut bytes = vec![0; rows.len() * 12];
    let count = rows.len() as u32;
    for (i, (name, row)) in rows.into_iter().enumerate() {
        let at = i * 12;
        let values = (bytes.len() + 15) & !15;
        bytes[at..at + 4].copy_from_slice(&name.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&(row.len() as u32).to_le_bytes());
        if !row.is_empty() {
            bytes[at + 8..at + 12].copy_from_slice(&(address + values as u32).to_le_bytes());
            bytes.resize(values, 0);
            for (key, value) in row {
                bytes.extend_from_slice(&key.to_le_bytes());
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    for ((address, _), key) in material_slots.into_iter().zip(keys) {
        write_pinned(set, address, key)?;
    }
    write_pinned(set, matrix_address, count)?;
    write_pinned(set, matrix_address + 4, address)?;
    let pinned = set.pinned.as_mut().expect("pinned metadata");
    pinned.metadata_size = offset + bytes.len() as u32;
    pinned.unowned.push((address, bytes));
    pinned.grown.push(GrownTag {
        tag: globals,
        region: address,
        replaced,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::build::PinnedImage;
    use map::{CacheType, FourCC};

    #[test]
    fn material_scales_are_indexed_as_body_then_shield() {
        let pair = [0.5f32.to_bits(), 2.0f32.to_bits()];
        let mut source = vec![0; H1_DAMAGE_SCALE_END];
        for (kind, value) in MATERIAL_TYPES.into_iter().zip(pair) {
            let at = 0x200 + 4 * kind;
            source[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(scales(&source).unwrap(), pair);
        assert!(scales(&source[..0x200]).is_err());
    }

    #[test]
    fn native_rows_keep_their_scale_when_cyborg_keys_are_added() {
        const BASE: u32 = 0x8030_0000;
        let mut set = TagSet::new(CacheType::Multiplayer, "map", "scenario");
        set.pinned = Some(PinnedImage {
            header: Vec::new(),
            file: Vec::new(),
            tables: [(0, 0); 5],
            index: Vec::new(),
            index_entries_end: 0,
            index_size: 0,
            bsp_size: 0,
            metadata_base: BASE,
            metadata_size: 0x500,
            unowned: Vec::new(),
            grown: Vec::new(),
        });
        set.globals = set
            .add_tag(
                FourCC(*b"matg"),
                "globals",
                TagPayload::Pinned {
                    address: BASE,
                    bytes: vec![0; 0x500],
                },
            )
            .unwrap();
        let stock_row = set.intern("stock").unwrap();
        let former_key = set.intern("former").unwrap();
        let body_address = BASE + 0x2c0;
        let shield_address = body_address + GLOBAL_MATERIAL_SIZE as u32;
        for (at, value) in [
            (BASE + DAMAGE_TABLE as u32, 1),
            (BASE + DAMAGE_TABLE as u32 + 4, BASE + 0x284),
            (BASE + 0x284, 1),
            (BASE + 0x288, BASE + 0x28c),
            (BASE + 0x28c, stock_row),
            (BASE + 0x290, 1),
            (BASE + 0x294, BASE + 0x2a0),
            (BASE + 0x2a0, former_key),
            (BASE + 0x2a4, 3.0f32.to_bits()),
            (BASE + GLOBAL_MATERIALS as u32, 2),
            (BASE + GLOBAL_MATERIALS as u32 + 4, body_address),
            (body_address + 0x14, former_key),
        ] {
            write_pinned(&mut set, at, value).unwrap();
        }
        for (at, kind) in [body_address, shield_address]
            .into_iter()
            .zip(MATERIAL_TYPES)
        {
            let name = set.intern(crate::arsenal::MATERIALS[kind].1).unwrap();
            write_pinned(&mut set, at, name).unwrap();
        }

        let pair = [0.5f32.to_bits(), 2.0f32.to_bits()];
        install(&mut set, &BTreeSet::from([pair])).unwrap();
        // The new arrays sit past the metadata, recorded as the globals',
        // with the stock rows and their values as the arrays replaced.
        let pinned = set.pinned.as_ref().unwrap();
        assert_eq!(
            pinned.grown,
            [GrownTag {
                tag: set.globals,
                region: BASE + 0x500,
                replaced: vec![(BASE + 0x28c, 12), (BASE + 0x2a0, 8)],
            }]
        );
        assert!(pinned.unowned.iter().any(|(at, _)| *at == BASE + 0x500));
        let key = set.intern(&damage_key(pair)).unwrap();
        let body = word(read_pinned(&set, body_address + 0x14, 4).unwrap(), 0);
        let shield = word(read_pinned(&set, shield_address + 0x14, 4).unwrap(), 0);
        assert_ne!(body, shield);
        assert_ne!(body, former_key);
        let matrix = read_pinned(&set, BASE + 0x284, 8).unwrap();
        let rows = block(&set, matrix, 12).unwrap();
        let row = |key| {
            let row = rows
                .chunks_exact(12)
                .find(|row| word(row, 0) == key)
                .unwrap();
            block(&set, &row[4..], 8)
                .unwrap()
                .chunks_exact(8)
                .map(|value| (word(value, 0), word(value, 4)))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            row(stock_row),
            BTreeMap::from([(body, 3.0f32.to_bits()), (former_key, 3.0f32.to_bits()),])
        );
        assert_eq!(
            row(key),
            BTreeMap::from([(body, pair[0]), (shield, pair[1])])
        );
    }
}
