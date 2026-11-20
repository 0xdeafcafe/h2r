use h1convert::shad::program::model_defaults::{ModelDefaults, VERTEX};
use h1convert::shad::{Bound, Library, TEX_BUMP, h1_lod_offset};
use h1map::Tag;
use map::{FourCC, TagId};

use super::{Converter, local};
use crate::{Converted, Error, Result};

impl Converter<'_> {
    pub(super) fn model_default_shader(
        &mut self,
        library: &Library,
        tag: &Tag,
        source: ModelDefaults,
    ) -> Result<TagId> {
        let mask = source
            .multipurpose
            .map(|id| -> Result<Bound> {
                let bitmap = self.bitmap(id).ok_or_else(|| {
                    Error::Invalid(format!("{}: unreadable multipurpose bitmap {id}", tag.name))
                })?;
                Ok(Bound {
                    id: bitmap,
                    scale: [1.0, 1.0],
                    lod_offset: h1_lod_offset(self.h1, id),
                })
            })
            .transpose()?;
        let program = source.passes(self.base.find(b"vrtx", VERTEX))?;
        let mut pending = Vec::new();
        let first = self.tree.tags.len();
        let stem = if program.is_empty() {
            library
                .stem(&TEX_BUMP)
                .ok_or_else(|| Error::NoTemplate(TEX_BUMP.stem.into()))?
        } else {
            let mut passes = Vec::new();
            for (layer, root) in program {
                passes.push((layer, local(first + pending.len())));
                let name = format!("{}_layer_{layer}", tag.name);
                pending.push((
                    name,
                    Converted {
                        group: FourCC(*b"spas"),
                        root,
                        raw: Vec::new(),
                    },
                ));
            }
            let root = library.model_default_stem(&passes, mask.is_some())?;
            let stem = local(first + pending.len());
            pending.push((
                tag.name.clone(),
                Converted {
                    group: FourCC(*b"stem"),
                    root,
                    raw: Vec::new(),
                },
            ));
            stem
        };
        let root = library.model_default_instance(stem, mask)?;
        for (name, converted) in pending {
            self.tree.push(&name, converted);
        }
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"shad"),
                root,
                raw: Vec::new(),
            },
        )
    }
}
