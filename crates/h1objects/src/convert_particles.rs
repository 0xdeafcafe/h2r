//! The converter's side of [`crate::particles`]: Halo 1 contrails (and
//! particles) built from the definitions, each once. A child of `convert`,
//! for the converter's caches.

use h1map::Tag;
use h1models::Converted;
use map::{FourCC, TagId};

use super::Converter;
use crate::Result;
use crate::particles::{self, H1Contrail};

impl Converter<'_> {
    /// A Halo 1 contrail as a Halo 2 `cont` ([`particles::contrail`]), its
    /// bitmap converted with `h1bitmaps`. Converted once.
    pub fn contrail(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let c = H1Contrail::read(self.h1, tag)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let bitmap = c.bitmap.and_then(|b| self.bitmap(b));
        if c.bitmap.is_some() && bitmap.is_none() {
            rep.no_home.push("bitmap: did not convert".into());
        }
        if !c.points.is_empty() {
            rep.no_home.push(format!("{} points: physics (pphy) not converted, null", c.points.len()));
        }
        let root = particles::contrail(&c, bitmap)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"cont"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(id)
    }
}

impl Converter<'_> {
    /// A rebuilt object's root (`group`, its object struct at `path`) with
    /// Halo 1's attachments in place of whatever it holds (a counterpart's):
    /// those that convert here ([`Converter::object_effects`]: looping
    /// sounds, effects, lights, contrails). Unchanged when none converts,
    /// so a projectile whose Halo 1 attachments have no converter keeps its
    /// counterpart's.
    pub(super) fn halo1_attachments(
        &mut self,
        group: [u8; 4],
        path: &str,
        root: map::build::Block,
        object_bytes: &[u8],
        obj: &crate::h1::Object,
        rep: &mut super::ObjectReport,
    ) -> Result<map::build::Block> {
        let (attachments, _) = self.object_effects(object_bytes, obj, rep)?;
        if attachments.is_empty() {
            return Ok(root);
        }
        let d = tags::defs::Defs::builtin()
            .group(FourCC(group))
            .ok_or_else(|| crate::Error::Invalid("no definition".into()))?;
        let mut r = tags::defs::from_built(d, &root).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        let e = d
            .child(&format!("{path}.attachments"))
            .ok_or_else(|| crate::Error::Invalid("no attachments".into()))?;
        let list = attachments
            .iter()
            .map(|a| {
                let mut x = tags::defs::default_record(e);
                x.set("type", tags::defs::Val::tag(FourCC(a.group), a.tag))?;
                x.set("marker", tags::defs::Val::text(&a.marker))?;
                Ok(x)
            })
            .collect::<std::result::Result<Vec<_>, tags::defs::DefError>>()
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        r.set(&format!("{path}.attachments"), tags::defs::Val::Block(list))
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        rep.no_home.push(format!("attachments: Halo 1's ({} converted)", attachments.len()));
        tags::defs::to_built(d, &r, &mut |_| None).map_err(|e| crate::Error::Invalid(e.to_string()))
    }
}
