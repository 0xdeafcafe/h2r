use h1convert::shad::program;
use map::build::Block;
use map::{FourCC, TagId};

use super::{Converter, local};
use crate::h1hud::scope::{self, ScopeMask};
use crate::{Converted, Error, Result};

const SCOPE_EFFECT: &str = "h1\\hud\\scope_mask";

impl Converter<'_> {
    /// Add the screen effect to a fresh weapon HUD, using the Halo 1
    /// view-specific masks and a shared Halo 2 compositing program.
    pub(super) fn weapon_scope(
        &mut self,
        wphi: h1map::TagId,
        name: &str,
        hud: &mut Block,
    ) -> Result<bool> {
        let Some(mask) = ScopeMask::read(self.h1, wphi)? else {
            return Ok(false);
        };
        let (art, sequences) = mask.bitmap(self.h1)?;
        let effect = self.scope_effect()?;
        let bitmap = self.tree.push(&format!("{name}_h1_scope_mask"), art);
        scope::attach(hud, mask, bitmap, sequences, effect)?;
        Ok(true)
    }

    fn scope_effect(&mut self) -> Result<TagId> {
        let effect_group = FourCC(*b"egor");
        if let Some(index) = self
            .tree
            .tags
            .iter()
            .position(|(name, tag)| name == SCOPE_EFFECT && tag.group == effect_group)
        {
            return Ok(local(index));
        }
        let vertex = self
            .base
            .find(b"vrtx", program::SCOPE_VERTEX)
            .ok_or_else(|| Error::NoTemplate(program::SCOPE_VERTEX.into()))?;
        let first = self.tree.tags.len();
        let tags = [
            (FourCC(*b"spas"), program::scope_pass(vertex)?),
            (FourCC(*b"stem"), program::scope_stem(local(first))?),
            (FourCC(*b"shad"), program::scope_instance(local(first + 1))?),
            (effect_group, scope::effect(local(first + 2))?),
        ];
        let mut id = TagId::NULL;
        for (group, root) in tags {
            id = self.tree.push(
                SCOPE_EFFECT,
                Converted {
                    group,
                    root,
                    raw: Vec::new(),
                },
            );
        }
        Ok(id)
    }
}
