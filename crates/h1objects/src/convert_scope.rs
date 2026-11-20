use h1convert::shad::program::{self, scope_filters};
use map::build::Block;
use map::TagId;
use tags::scope_zoom::{Blur, Profile};

use super::Converter;
use crate::h1hud::scope::{self, Entry, ScopeMask, Visibility};
use crate::{Error, Result};

impl Converter<'_> {
    /// Rebuild the masks and filters as a HUD screen effect for each view.
    pub(super) fn weapon_scope(
        &mut self,
        wphi: h1map::TagId,
        name: &str,
        hud: &mut Block,
    ) -> Result<bool> {
        let Some(mask) = ScopeMask::read(self.h1, wphi)? else {
            return Ok(false);
        };
        let name = format!("{name}_h1_scope");
        let (bitmap, sequences) = if mask.full.is_some() || mask.split.is_some() {
            let (art, sequences) = mask.bitmap(self.h1)?;
            (self.tree.push(&format!("{name}_mask"), art), sequences)
        } else {
            (TagId::NULL, [-1; 3])
        };

        let mut masked_effects = [TagId::NULL; 2];
        let mut unmasked_effects = [TagId::NULL; 2];
        let mut unzoomed_effects = [TagId::NULL; 2];
        for (mode, suffix) in ["full", "split"].into_iter().enumerate() {
            let mut profile = mask.filters;
            if mode == 1 {
                // Halo 1 0x000d6bd2: blur only in a single viewport.
                profile.blur = Blur::default();
            }
            let view_name = format!("{name}_{suffix}");
            if sequences[mode] >= 0 {
                masked_effects[mode] = self.scope_effect(&view_name, profile, true)?;
                let unzoomed = profile.without_zoom_only();
                if mask.zoom_only && unzoomed.enabled() {
                    unzoomed_effects[mode] =
                        self.scope_effect(&format!("{view_name}_unzoomed"), unzoomed, false)?;
                }
            } else if profile.enabled() {
                unmasked_effects[mode] = self.scope_effect(&view_name, profile, false)?;
            }
        }
        let mut entries = Vec::new();
        for (effects, bitmap, sequences, visibility) in [
            (
                masked_effects,
                bitmap,
                sequences,
                if mask.zoom_only {
                    Visibility::Zoomed
                } else {
                    Visibility::Always
                },
            ),
            (unmasked_effects, TagId::NULL, [-1; 3], Visibility::Always),
            (unzoomed_effects, TagId::NULL, [-1; 3], Visibility::Unzoomed),
        ] {
            let [full, split] = effects;
            if full != TagId::NULL || split != TagId::NULL {
                entries.push(Entry {
                    bitmap,
                    sequences,
                    visibility,
                    effects: [full, split, split],
                });
            }
        }
        if entries.is_empty() {
            return Ok(false);
        }
        scope::attach(hud, &entries)?;
        Ok(true)
    }

    fn scope_effect(&mut self, name: &str, profile: Profile, masked: bool) -> Result<TagId> {
        let vertex = self
            .base
            .find(b"vrtx", program::SCOPE_VERTEX)
            .ok_or_else(|| Error::NoTemplate(program::SCOPE_VERTEX.into()))?;
        let mut passes = Vec::new();
        if profile.enabled() {
            passes.push(self.program_tag(
                &format!("{name}_copy"),
                b"spas",
                scope_filters::copy(vertex)?,
            ));
            passes.push(self.program_tag(
                &format!("{name}_filters"),
                b"spas",
                scope_filters::composite(vertex, profile, masked)?,
            ));
        }
        if masked {
            let mask_pass = format!("{name}_overlay");
            passes.push(self.program_tag(&mask_pass, b"spas", program::scope_pass(vertex)?));
        }
        let template = self.program_tag(name, b"stem", scope_filters::template(&passes)?);
        let shader = self.program_tag(name, b"shad", program::scope_instance(template)?);
        Ok(self.program_tag(name, b"egor", scope::effect(shader, profile, masked)?))
    }
}
