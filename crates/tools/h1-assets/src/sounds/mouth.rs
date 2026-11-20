//! Retail Xbox sound extra-info resource. The loader patches the two
//! pointers in the resident header using the data and block locators.
use crate::sounds::gestalt::{Child, Nested, SoundGestalt, blocks};
use crate::sounds::{Error, Result};
use map::build::TagSet;

pub(crate) fn add(curves: &[&[u8]], gestalt: &mut SoundGestalt, set: &mut TagSet) -> Result<u16> {
    let index = gestalt.count(blocks::EXTRA_INFOS);
    if index > i16::MAX as usize || curves.len() > 128 {
        return Err(Error::Unsupported(format!(
            "sound mouth table: extra-info index {index}, {} signed-byte descriptors",
            curves.len()
        )));
    }
    let mut data = Vec::new();
    let mut descriptors = Vec::new();
    for &curve in curves {
        if curve.len() > i16::MAX as usize + 1 {
            return Err(Error::Unsupported(format!(
                "mouth curve has {} ticks",
                curve.len()
            )));
        }
        descriptors.extend_from_slice(&(data.len() as u32).to_le_bytes());
        descriptors.extend_from_slice(&(curve.len() as u32).to_le_bytes());
        // H1 has no separate facial-animation sequence.
        descriptors.extend_from_slice(&[0; 8]);
        data.extend_from_slice(curve);
    }

    let block_start = (4 + data.len()).next_multiple_of(4) + 4;
    let resource_size = block_start + descriptors.len();
    let mut raw = b"hklb".to_vec();
    for value in [resource_size, data.len(), 0, curves.len(), 0] {
        raw.extend_from_slice(&(value as u32).to_le_bytes());
    }
    raw.extend_from_slice(b"crsr");
    raw.extend_from_slice(&data);
    raw.resize(24 + block_start - 4, 0);
    raw.extend_from_slice(b"crsr");
    raw.extend_from_slice(&descriptors);
    raw.extend_from_slice(b"fklb");

    let mut bytes = vec![0; 0x2c];
    for (at, value) in [
        (0xc, raw.len() as u32),
        (0x10, 16),
        (0x14, resource_size as u32),
        (0x24, 0x0100fff8),
        (0x28, u32::MAX),
    ] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut locators = Vec::new();
    for (locator, size, offset) in [
        ([1, 2, 0, 0, 0, 0, 0xff, 0xff], data.len(), 4),
        ([0, 2, 8, 0, 8, 0, 16, 0], descriptors.len(), block_start),
    ] {
        locators.extend_from_slice(&locator);
        locators.extend_from_slice(&(size as u32).to_le_bytes());
        locators.extend_from_slice(&(offset as u32).to_le_bytes());
    }
    gestalt.nested[blocks::EXTRA_INFOS].push(Nested {
        bytes,
        children: vec![
            Child {
                count: 0,
                bytes: Vec::new(),
            },
            Child {
                count: 2,
                bytes: locators,
            },
        ],
    });
    gestalt
        .extra_raws
        .insert(index, set.add_raw(raw, crate::sounds::convert::CHUNK_ALIGN));
    Ok(index as u16)
}
