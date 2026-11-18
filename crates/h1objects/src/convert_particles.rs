//! The converter's side of [`crate::particles`]: Halo 1 contrails (and
//! particles) built from the definitions, each once. A child of `convert`,
//! for the converter's caches.

use h1map::Tag;
use h1models::Converted;
use map::{FourCC, TagId};

use super::Converter;
use crate::Result;
use crate::particles::{self, H1Contrail, H1Particle};

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

impl Converter<'_> {
    /// A Halo 1 particle as a Halo 2 `prt3` ([`particles::particle`]): the
    /// stem its blend function names, its bitmap (`h1bitmaps`), its
    /// collision and death effects. `None` when the base has no particle
    /// role or none of the particle stems. Converted once.
    pub fn particle(&mut self, tag: &Tag) -> Result<Option<TagId>> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(Some(*id));
        }
        let p = H1Particle::read(self.h1, tag)?;
        if self.particle_role.is_none() {
            self.particle_role = Some(particles::particle_role(self.base)?);
        }
        let Some(role) = self.particle_role.clone().flatten() else {
            return Ok(None);
        };
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let (want, proven) = particles::particle_stem(p.blend);
        let shader = role
            .shaders
            .iter()
            .find(|s| s.name == want)
            .or_else(|| role.shaders.iter().find(|s| s.name == particles::particle_stem(0).0));
        let Some(shader) = shader.cloned() else {
            return Ok(None);
        };
        if shader.name != want {
            rep.no_home.push(format!("shader: {want} not on the base, {} instead", shader.name));
        } else if !proven {
            rep.no_home.push(format!("shader: blend function {} drawn as {want} (unproven)", p.blend));
        }
        let bitmap = match p.bitmap {
            Some(b) => {
                let size = self
                    .h1
                    .tag(b)
                    .ok()
                    .and_then(|t| h1map::bitmap::Bitmap::read(self.h1, t).ok())
                    .and_then(|bm| bm.images.first().map(|i| (i.width, i.height)));
                match (self.bitmap(b), size) {
                    (Some(id), Some((w, h))) => Some((id, w, h)),
                    _ => {
                        rep.no_home.push("bitmap: did not convert".into());
                        None
                    }
                }
            }
            None => None,
        };
        let effect = |c: &mut Self, id: Option<h1map::TagId>, rep: &mut super::ObjectReport, what: &str| {
            let t = id.and_then(|i| c.h1.tag(i).ok())?;
            let got = if t.group == h1map::Group::new(b"effe") {
                c.effect(t).ok().map(|id| (FourCC(*b"effe"), id))
            } else if t.group == h1map::Group::new(b"snd!") {
                c.sound(t, rep).map(|id| (FourCC(*b"snd!"), id))
            } else {
                None
            };
            if got.is_none() {
                rep.no_home.push(format!("{what}: {} did not convert", t.name));
            }
            got
        };
        let collision = effect(self, p.collision_effect, &mut rep, "collision effect");
        let death = effect(self, p.death_effect, &mut rep, "death effect");
        rep.no_home.push(
            "emitter-side fields (lifespan, fade times, radius and frame animation, physics, contact deterioration, fade sizes, orientation, secondary map) not carried: Halo 2 holds them on the effect's emitter".into(),
        );
        let root = particles::particle(&p, &role, &shader, bitmap, collision, death)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"prt3"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(Some(id))
    }
}
