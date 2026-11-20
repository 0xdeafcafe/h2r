//! H1 object texture overrides and the image permutation on each model material.

use super::Converter;
use crate::{Converted, Result};
use h1convert::shad::{self, Bound, Library, Role};
use h1map::{
    Tag,
    bitmap::{Bitmap, Image, ImageKind},
};
use map::{FourCC, TagId};
use std::collections::HashMap;

fn image_index(permutation: i16, count: usize) -> Option<usize> {
    // Signed IDIV, followed by the bounds check in 0x1290e0.
    let count = i32::try_from(count).ok()?;
    usize::try_from(i32::from(permutation).checked_rem(count)?).ok()
}

/// Each derived bitmap is sampled directly by the model material. Its source
/// sequence and sprite indices refer to the unsliced bitmap.
fn selected_bitmap(
    bitmap: &Bitmap,
    image: Image,
    pixels: Vec<u8>,
    baked: bool,
) -> h1_assets::bitmaps::Result<Converted> {
    let mut selected = bitmap.clone();
    selected.images = vec![image];
    selected.sequences.clear();
    if baked {
        selected.encoding = h1_assets::bitmaps::mask::INTERPOLATED_ALPHA;
    }
    let mut data = Some(pixels);
    h1_assets::bitmaps::convert::convert_with(&selected, |_| Ok(data.take().expect("one selected image")))
        .map(|(converted, _)| converted)
}

pub(super) fn material_permutation(texture_override: i16, shader_permutation: i16) -> i16 {
    if texture_override == 0 {
        shader_permutation
    } else {
        texture_override
    }
}

impl Converter<'_> {
    pub(super) fn permuted_shader(
        &mut self,
        tag: &Tag,
        permutation: i16,
        library: &Library,
    ) -> Result<Option<TagId>> {
        let model_shader = tag.group == h1map::Group::new(b"soso");
        if !model_shader && tag.group != h1map::Group::new(b"senv") {
            return Ok(None);
        }
        let key = (tag.id, permutation);
        if let Some(&id) = self.permuted_shaders.get(&key) {
            return Ok(Some(id));
        }
        let h1 = self.h1;
        let fail = |why: &str| h1convert::Error::no_rebuild(&h1.header().name, &tag.name, why);
        let Some(base_id) = h1convert::textures::h1_maps_of(h1, tag).and_then(|m| m.base) else {
            return Ok(None);
        };
        let base = h1.tag(base_id).map_err(|e| fail(&e.to_string()))?;
        let base_bitmap = Bitmap::read(h1, base).map_err(|e| fail(&e.to_string()))?;
        if base_bitmap.images.is_empty() {
            return Err(fail("model base bitmap has no images").into());
        }
        if base_bitmap.images.len() == 1 {
            return Ok(None);
        }
        let mut recipe =
            shad::read_h1(h1, tag).ok_or_else(|| fail("no multi-image shader recipe"))?;
        if !library.has(recipe.layout) {
            return Err(fail(recipe.layout.stem).into());
        }
        let reflected = model_shader
            && [
                shad::TEX_BUMP_ENV.stem,
                shad::TEX_BUMP_ENV_FOUR_CHANGE_COLOR.stem,
            ]
            .contains(&recipe.layout.stem);
        let mask = if reflected {
            let reference = h1map::TagRef::read(h1.tag_data(tag)?, shad::SOSO_MULTIPURPOSE)
                .ok_or_else(|| fail("short reflection mask reference"))?;
            if reference.id.is_none() {
                None
            } else {
                let tag = h1.tag(reference.id).map_err(|e| fail(&e.to_string()))?;
                if tag.group != h1map::Group::new(b"bitm") || tag.group != reference.group {
                    return Err(fail("invalid reflection mask reference").into());
                }
                Some(Bitmap::read(h1, tag).map_err(|e| fail(&e.to_string()))?)
            }
        } else {
            None
        };
        let mut maps = HashMap::new();
        let stem = format!("{}_permutation_{permutation}", tag.name);
        for (role, source) in &recipe.maps {
            let bitmap_tag = h1.tag(source.bitmap)?;
            let bitmap = Bitmap::read(h1, bitmap_tag)?;
            let k = image_index(permutation, bitmap.images.len())
                .ok_or_else(|| fail("invalid texture permutation"))?;
            let image = &bitmap.images[k];
            let kind = if *role == Role::Cube {
                ImageKind::Cube
            } else {
                ImageKind::D2
            };
            if image.kind != kind {
                return Err(fail("texture permutation has the wrong image type").into());
            }
            let pixels = bitmap.pixels(h1, k)?;
            let bind = |id| Bound {
                id,
                scale: source.scale,
                lod_offset: shad::lod_offset(image.width, image.height),
            };
            let converted = |image, pixels, baked| {
                selected_bitmap(&bitmap, image, pixels, baked).map_err(|e| fail(&e.to_string()))
            };
            let baked = match (role, &mask) {
                (Role::Base, Some(mask)) => {
                    let m = image_index(permutation, mask.images.len())
                        .ok_or_else(|| fail("invalid reflection mask permutation"))?;
                    Some(h1_assets::bitmaps::mask::red_into_alpha(
                        image,
                        pixels,
                        &mask.images[m],
                        mask.pixels(h1, m)?,
                    ))
                }
                (Role::ChangeColour, _) => {
                    let (slot, _) = shad::soso_change_colour(h1, tag)
                        .ok_or_else(|| fail("missing change colour selector"))?;
                    Some(h1_assets::bitmaps::mask::channel_into(
                        image,
                        pixels,
                        shad::SOSO_CHANGE_COLOUR_CHANNEL,
                        slot,
                    ))
                }
                _ => None,
            };
            let id = if let Some(baked) = baked {
                let (derived, bytes) = baked.map_err(|e| fail(&e.to_string()))?;
                self.tree.push(
                    &format!("{stem}_{role:?}"),
                    converted(derived, bytes, true)?,
                )
            } else if bitmap.images.len() == 1 {
                self.source_bitmap(Some(source.bitmap))?
                    .expect("source bitmap")
            } else {
                self.tree.push(
                    &format!("{stem}_{role:?}"),
                    converted(image.clone(), pixels.to_vec(), false)?,
                )
            };
            maps.insert(*role, bind(id));
            if model_shader
                && *role == Role::Base
                && recipe.layout.stem == shad::TEX_BUMP_ALPHA_TEST.stem
            {
                let (flat, bytes) = h1_assets::bitmaps::mask::cutout_bump(image, pixels)
                    .map_err(|e| fail(&e.to_string()))?;
                maps.insert(
                    Role::Bump,
                    bind(
                        self.tree
                            .push(&format!("{stem}_cutout"), converted(flat, bytes, true)?),
                    ),
                );
            }
        }
        recipe.extras.cinematic_reflection = reflected;
        let root = library.build_with(recipe.layout, &maps, &recipe.extras)?;
        let id = self.tree.push(
            &stem,
            Converted {
                group: FourCC(*b"shad"),
                root,
                raw: Vec::new(),
            },
        );
        self.permuted_shaders.insert(key, id);
        Ok(Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonzero_object_override_replaces_the_shader_entry() {
        assert_eq!(material_permutation(0, 1), 1);
        assert_eq!(material_permutation(2, 1), 2);
    }

    #[test]
    fn permutation_wraps_independently_at_each_sampler() {
        assert_eq!(image_index(3, 2), Some(1));
        assert_eq!(image_index(3, 1), Some(0));
        assert_eq!(image_index(-2, 2), Some(0));
        assert_eq!(image_index(-1, 2), None);
        assert_eq!(image_index(1, 0), None);
    }
}
