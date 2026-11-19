//! The converter's side of [`crate::particles`]: Halo 1 contrails (and
//! particles) built from the definitions, each once. A child of `convert`,
//! for the converter's caches.

use h1map::Tag;
use h1models::Converted;
use map::{FourCC, TagId};

use super::Converter;
use crate::Result;
use crate::particles::{self, H1Contrail, H1Particle, H1ParticleState, H1ParticleType};

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
    /// A Halo 1 particle as a Halo 2 `prt3` ([`particles::particle`]): the
    /// stem its blend function names, its bitmap (`h1bitmaps`), its
    /// collision and death effects. `None` when the base has no particle
    /// role or none of the particle stems. Converted once.
    pub fn particle(&mut self, tag: &Tag) -> Result<Option<TagId>> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(Some(*id));
        }
        let p = H1Particle::read(self.h1, tag)?;
        let got = self.particle_of(tag, &tag.name, &p)?;
        if let Some(id) = got {
            self.done.insert(tag.id.0, id);
        }
        Ok(got)
    }

    /// The `prt3` of particle `p` (from `tag`: a `part`, or a `pctl` whose
    /// particle state `p` stands for), pushed as `name`. Not cached.
    pub(super) fn particle_of(&mut self, tag: &Tag, name: &str, p: &H1Particle) -> Result<Option<TagId>> {
        if self.particle_role.is_none() {
            self.particle_role = Some(particles::particle_role(self.base)?);
        }
        let Some(role) = self.particle_role.clone().flatten() else {
            return Ok(None);
        };
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.source = name.to_owned();
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
            if c.effects_open.contains(&t.id.0) {
                rep.no_home.push(format!("{what}: {} leads back to the effect being converted, left out", t.name));
                return None;
            }
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
        let (root, roles) = crate::provenance::capture(|| {
            particles::particle(p, &role, &shader, bitmap, collision, death)
        });
        let root = root?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            name,
            Converted {
                group: FourCC(*b"prt3"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(Some(id))
    }
}

impl Converter<'_> {
    /// The built `effe` root of Halo 1 effect `tag` with each event's Halo 1
    /// particles as particle systems ([`particles::particle_system`]): those
    /// whose `part` converts. An event that gains particle systems and is
    /// shorter than a tick lasts a tick. Unchanged when none converts.
    pub(super) fn effect_particle_systems(
        &mut self,
        tag: &Tag,
        root: map::build::Block,
        rep: &mut super::ObjectReport,
    ) -> Result<map::build::Block> {
        let events = particles::effect_particles(self.h1, tag)?;
        // Halo 1 particle systems (`pctl` parts) per event: tag and location.
        let pctl = h1map::Group::new(b"pctl");
        let systems_of: Vec<Vec<(h1map::TagId, i16)>> = crate::h1::Effect::read(self.h1, tag)?
            .events
            .iter()
            .map(|ev| {
                ev.parts
                    .iter()
                    .filter(|p| p.group == pctl)
                    .filter_map(|p| Some((p.reference?, p.location)))
                    .collect()
            })
            .collect();
        let total: usize =
            events.iter().map(|e| e.1.len()).sum::<usize>() + systems_of.iter().map(Vec::len).sum::<usize>();
        if total == 0 {
            return Ok(root);
        }
        if self.emitter_role.is_none() {
            self.emitter_role = Some(particles::emitter_role(self.base)?);
        }
        let Some(role) = self.emitter_role.clone().flatten() else {
            rep.no_home.push(format!("{total} particles left out (the base has no emitter role)"));
            return Ok(root);
        };
        let d = tags::defs::Defs::builtin()
            .group(FourCC(*b"effe"))
            .ok_or_else(|| crate::Error::Invalid("no effe definition".into()))?;
        let inv = |e: tags::defs::DefError| crate::Error::Invalid(e.to_string());
        let mut r = tags::defs::from_built(d, &root).map_err(inv)?;
        let Some(tags::defs::Val::Block(mut evs)) = r.at("events").cloned() else {
            return Ok(root);
        };
        let (mut made, mut left) = (0usize, 0usize);
        for (k, (duration, list)) in events.iter().enumerate() {
            let Some(ev) = evs.get_mut(k) else { continue };
            let d = (duration[0] + duration[1]) * 0.5;
            let mut systems = Vec::new();
            for p in list {
                let part = p.particle.and_then(|i| self.h1.tag(i).ok());
                let got = match part {
                    Some(t) => match (self.particle(t), H1Particle::read(self.h1, t)) {
                        (Ok(Some(id)), Ok(h)) => Some((id, h)),
                        _ => None,
                    },
                    None => None,
                };
                let Some((id, h)) = got else {
                    left += 1;
                    continue;
                };
                systems.push(particles::particle_system(p, &h, id, &role, d)?);
            }
            for (id, location) in systems_of.get(k).into_iter().flatten() {
                let Ok(t) = self.h1.tag(*id) else { continue };
                let states = self.particle_system_states(t)?;
                if states.is_empty() {
                    left += 1;
                }
                for (prt3, state, count) in states {
                    systems.push(particles::particle_system(
                        &state.as_effect_particle(count, *location),
                        &state.as_particle(),
                        prt3,
                        &role,
                        d,
                    )?);
                }
            }
            if systems.is_empty() {
                continue;
            }
            made += systems.len();
            ev.set("particle_systems", tags::defs::Val::Block(systems)).map_err(inv)?;
            if duration[1] < particles::TICK {
                ev.set("duration_bounds", tags::defs::Val::floats(&[particles::TICK; 2])).map_err(inv)?;
            }
        }
        if left > 0 {
            rep.no_home.push(format!("{left} particles or particle systems left out (their part or states did not convert)"));
        }
        if made == 0 {
            return Ok(root);
        }
        rep.no_home.push(format!(
            "{made} particles and particle-system states as particle systems (count, lifespan, speed, cone, size, tint and alpha; distribution radius, direction, offset, angular velocity, physics and state sequences not carried)"
        ));
        r.set("events", tags::defs::Val::Block(evs)).map_err(inv)?;
        tags::defs::to_built(d, &r, &mut |_| None).map_err(inv)
    }
}

pub(super) type ParticleTypes = Vec<(H1ParticleType, Vec<(TagId, H1ParticleState)>)>;

impl Converter<'_> {
    /// A Halo 1 `pctl`'s lasting particle states, with their converted
    /// `prt3` and the type's initial count. Used by one-shot effects.
    pub fn particle_system_states(
        &mut self,
        tag: &Tag,
    ) -> Result<Vec<(TagId, H1ParticleState, u16)>> {
        Ok(self
            .particle_types(tag)?
            .into_iter()
            .flat_map(|(ty, states)| {
                states
                    .into_iter()
                    .map(move |(id, state)| (id, state, ty.initial_count))
            })
            .collect())
    }

    fn particle_types(&mut self, tag: &Tag) -> Result<ParticleTypes> {
        if let Some(v) = self.pctl_done.get(&tag.id.0) {
            return Ok(v.clone());
        }
        let mut out = Vec::new();
        for mut ty in particles::particle_system_types(self.h1, tag)? {
            let mut states = Vec::new();
            for st in std::mem::take(&mut ty.states)
                .into_iter()
                .filter(H1ParticleState::lasts)
            {
                let name = format!("{}\\{}\\{}", tag.name, ty.name, st.name);
                if let Some(id) = self.particle_of(tag, &name, &st.as_particle())? {
                    states.push((id, st));
                }
            }
            out.push((ty, states));
        }
        self.pctl_done.insert(tag.id.0, out.clone());
        Ok(out)
    }

    /// The object dispatcher builds effects but has no pctl branch. Use an
    /// effect with a marker location and a loop over the source emitters.
    pub(super) fn attached_particle_system(
        &mut self,
        tag: &Tag,
        marker: &str,
    ) -> Result<Vec<TagId>> {
        let key = (tag.id.0, marker.to_owned());
        if let Some(ids) = self.attached_particles.get(&key) {
            return Ok(ids.clone());
        }
        if self.emitter_role.is_none() {
            self.emitter_role = Some(particles::emitter_role(self.base)?);
        }
        let Some(role) = self.emitter_role.clone().flatten() else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        for (ty, states) in self.particle_types(tag)? {
            let (root, roles) = crate::provenance::capture(|| {
                particles::attached_particle_effect(&ty, &states, marker, &role)
            });
            if let Some(root) = root? {
                let name = format!("{}\\{}\\attached_{marker}", tag.name, ty.name);
                let id = self.tree.push(
                    &name,
                    Converted {
                        group: FourCC(*b"effe"),
                        root,
                        raw: Vec::new(),
                    },
                );
                self.tree.note_roles(id, roles);
                ids.push(id);
            }
        }
        self.attached_particles.insert(key, ids.clone());
        Ok(ids)
    }
}
