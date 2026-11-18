//! The Halo 1 sandbox of a converted multiplayer map: the objects a map's
//! spawns name, converted with [`Converter`], for
//! `h1convert::convert::halo1::prepare_halo1`.
//!
//! ```ignore
//! let base = Base::read(&mut base_map)?;
//! let mut objects = Objects::new(&h1, &base);
//! let done = h1convert::convert::halo1::convert_halo1(&h1, sha, &mut base_map, &mut objects)?;
//! ```

use h1convert::convert::halo1::{Halo1Objects, HudTag, ObjectTags};
use h1map::{Group, H1Map};

use crate::base::Base;
use crate::convert::{Converter, Tree, local};

/// Converts Halo 1 objects on demand, keeping every tag they need.
pub struct Objects<'a> {
    conv: Converter<'a>,
}

impl<'a> Objects<'a> {
    /// Objects of `h1`, built on `base`'s templates (the base map the
    /// converted map is made from).
    pub fn new(h1: &'a H1Map, base: &'a Base) -> Self {
        Self {
            conv: Converter::new(h1, base),
        }
    }
}

impl Halo1Objects for Objects<'_> {
    fn object(&mut self, group: [u8; 4], name: &str) -> Result<usize, String> {
        let tag = self
            .conv
            .h1
            .find(Group::new(&group), name)
            .ok_or_else(|| format!("no {} {name}", String::from_utf8_lossy(&group)))?;
        let id = self.conv.object(tag).map_err(|e| e.to_string())?;
        // A tree tag's placeholder datum holds its index.
        if id.0 & 0xffff_0000 != local(0).0 {
            return Err(format!("{name}: not a converted tag"));
        }
        Ok((id.0 & 0xffff) as usize)
    }

    fn first_person_arms(&mut self, id: h1map::TagId) -> Result<usize, String> {
        let got = self.conv.first_person_arms(id).map_err(|e| e.to_string())?;
        if got.0 & 0xffff_0000 != local(0).0 {
            return Err("first-person arms: not a converted tag".into());
        }
        Ok((got.0 & 0xffff) as usize)
    }

    fn take(&mut self) -> ObjectTags {
        let tree = std::mem::take(&mut self.conv.tree);
        let notes = tree
            .reports
            .iter()
            .map(|r| {
                format!(
                    "{} {} on {} ({} values from Halo 1, {} template references kept)",
                    r.group,
                    r.source,
                    r.template,
                    r.tuning.len(),
                    r.stopgaps.len()
                )
            })
            .collect();
        let (tags, sounds, loops) = (tree.tags, tree.sounds, tree.loops);
        let tag = |id: map::TagId| match crate::convert::local_index(id) {
            Some(k) => HudTag::Added(k),
            None => HudTag::Base(id),
        };
        let hud_swaps = tree
            .hud_swaps
            .iter()
            .map(|s| [tag(s.owner), tag(s.halo1), tag(s.halo2)])
            .collect();
        ObjectTags {
            hud_swaps,
            hud_sensor: tree.hud_sensor,
            count: tags.len(),
            // The tags first, at the datums the placements name; the
            // sounds they play once the collections are in after them.
            install: Box::new(move |set| {
                Tree {
                    tags: tags.clone(),
                    ..Tree::default()
                }
                .install_tags(set)
                .map_err(|e| e.to_string())
            }),
            after: Box::new(move |set, ids| {
                Tree {
                    sounds: sounds.clone(),
                    loops: loops.clone(),
                    ..Tree::default()
                }
                .install_audio_for(set, ids);
                Ok(())
            }),
            notes,
        }
    }
}
