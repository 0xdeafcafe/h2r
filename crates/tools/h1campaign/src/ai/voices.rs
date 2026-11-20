//! Spawn-time dialogue selectors from the source unit definitions.

use h1map::{Group, H1Map, Space, TagId};

use crate::ai::{Result, bytes::i16_at};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub tag: TagId,
    pub name: String,
    /// Generic selectors in author order, including repeated zeroes. The
    /// original unit constructor skips selectors 100 and above.
    pub generic: Vec<i16>,
}

pub(crate) fn generic(space: &Space<'_>, unit: &[u8]) -> Result<Vec<i16>> {
    Ok(space
        .block_at(unit, 0x2b4, 0x18)?
        .into_iter()
        .map(|entry| i16_at(entry, 0))
        .filter(|voice| *voice < 100)
        .take(16)
        .collect())
}

pub(crate) fn read(map: &H1Map) -> Result<Vec<Definition>> {
    let space = map.tag_space();
    let mut definitions = Vec::new();
    for tag in map
        .tags()
        .iter()
        .filter(|t| t.group == Group::new(b"bipd") || t.group == Group::new(b"vehi"))
    {
        let voices = generic(&space, map.tag_data(tag)?)?;
        if !voices.is_empty() {
            definitions.push(Definition {
                tag: tag.id,
                name: tag.name.clone(),
                generic: voices,
            });
        }
    }
    Ok(definitions)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Squad {
    pub encounter: u16,
    pub leader_rule: u16,
    pub generic: Option<u16>,
    /// Author-order starting locations with any unit override applied.
    pub locations: Vec<Option<u16>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_generic_profiles_keep_order_and_duplicate_zeroes() {
        let mut bytes = vec![0; 0x300 + 5 * 0x18];
        bytes[0x2b4..0x2b8].copy_from_slice(&5u32.to_le_bytes());
        bytes[0x2b8..0x2bc].copy_from_slice(&0x1300u32.to_le_bytes());
        for (record, voice) in bytes[0x300..]
            .chunks_exact_mut(0x18)
            .zip([2i16, 0, 101, 0, 4])
        {
            record[..2].copy_from_slice(&voice.to_le_bytes());
        }
        assert_eq!(
            generic(&Space::new(&bytes, 0x1000), &bytes).unwrap(),
            [2, 0, 0, 4]
        );
    }
}
