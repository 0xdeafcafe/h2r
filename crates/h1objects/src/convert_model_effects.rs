use std::collections::HashMap;

use h1convert::shad::program::model_effects::{self, Plasma};
use h1convert::shad::{Bound, Library, ONE_ADD_ILLUM, Role, h1_lod_offset, h1_texture_kind};
use h1map::{Group, Tag, bitmap::ImageKind};
use map::{FourCC, TagId};

use super::{Converter, local};
use crate::{Converted, Error, Result};

impl Converter<'_> {
    pub(super) fn model_effect_shader(
        &mut self,
        library: &Library,
        tag: &Tag,
    ) -> Result<Option<TagId>> {
        let bound = |conv: &mut Self, id| -> Result<Bound> {
            let bitmap = conv
                .bitmap(id)
                .ok_or_else(|| Error::Invalid(format!("unreadable bitmap {id}")))?;
            Ok(Bound {
                id: bitmap,
                scale: [1.0; 2],
                lod_offset: h1_lod_offset(conv.h1, id),
            })
        };
        let root = if let Some(default) = model_effects::chicago_default(self.h1, tag)? {
            let maps = HashMap::from([(Role::Base, bound(self, default)?)]);
            library.build(&ONE_ADD_ILLUM, &maps)?
        } else if tag.group == Group::new(b"spla") {
            let source = Plasma::read(self.h1.tag_data(tag)?)?;
            let [first, second] = source.noise.map(|n| n.bitmap);
            for id in [first, second] {
                if h1_texture_kind(self.h1, id) != Some(ImageKind::D3) {
                    return Err(Error::Invalid(format!("plasma noise {id} is not a volume")));
                }
            }
            let vertex = self
                .base
                .find(b"vrtx", model_effects::PLASMA_VERTEX)
                .ok_or_else(|| Error::NoTemplate(model_effects::PLASMA_VERTEX.into()))?;
            let bitmaps = [bound(self, first)?, bound(self, second)?];
            let pass_id = local(self.tree.tags.len());
            let stem_id = local(self.tree.tags.len() + 1);
            let pass = model_effects::plasma_pass(vertex)?;
            let stem = model_effects::plasma_stem(pass_id)?;
            let root = source.instance(stem_id, bitmaps)?;
            for (group, tree) in [(b"spas", pass), (b"stem", stem)] {
                self.tree.push(
                    &tag.name,
                    Converted {
                        group: FourCC(*group),
                        root: tree,
                        raw: Vec::new(),
                    },
                );
            }
            root
        } else {
            return Ok(None);
        };
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"shad"),
                root,
                raw: Vec::new(),
            },
        )
        .map(Some)
    }
}
