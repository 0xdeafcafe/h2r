//! A campaign scenario's chapter-title text and its localized string IDs.

use h1map::H1Map;
use map::TagId;
use map::build::{Block, TagSet};

use crate::Result;
use crate::convert::Locales;
use crate::h1::{Ustr, scenario_text};

const CHAPTER_TITLES: u32 = 0x200;

/// The converted scenario text referenced by chapter title records.
pub struct LevelText {
    chapters: Option<(TagId, Vec<u32>)>,
}

impl LevelText {
    /// Append the source help list to the shared locale and keep its row IDs.
    pub fn install(map: &H1Map, set: &mut TagSet, locales: &mut Locales) -> Result<Self> {
        let chapters = if let Some(reference) = scenario_text(map)?.help_text {
            let tag = map.tag(reference.id)?;
            let list = Ustr::read(map, tag)?;
            let (converted, ids) = locales.add_ustr(set, &tag.name, &list)?;
            Some((converted.install(set, &tag.name)?, ids.into_iter().map(|id| id.0).collect()))
        } else {
            None
        };
        Ok(Self { chapters })
    }

    /// The string IDs addressed by Halo 1 chapter title text indices.
    pub fn chapter_text_ids(&self) -> &[u32] {
        self.chapters.as_ref().map_or(&[], |(_, ids)| ids.as_slice())
    }

    /// Install the chapter list reference on a fresh or inherited scenario.
    pub fn write(&self, scenario: &mut Block) {
        scenario.fixups.retain(|f| f.offset != CHAPTER_TITLES);
        scenario.put_u32(CHAPTER_TITLES as usize, u32::from_be_bytes(*b"unic"));
        scenario.put_u32(CHAPTER_TITLES as usize + 4, u32::MAX);
        if let Some((id, _)) = &self.chapters {
            scenario.tag_ref(CHAPTER_TITLES, *id);
        }
    }
}
