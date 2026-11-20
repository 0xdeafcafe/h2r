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
        let rep = Self::report(tag, "none: rebuilt from tags::defs");
        let bitmap = self.source_bitmap(c.bitmap)?;
        let mut physics = Vec::with_capacity(c.points.len());
        for point in &c.points {
            let r = h1map::TagRef::read(point, 0x10)
                .ok_or(crate::Error::Truncated("contrail point physics"))?;
            let id = if r.id.is_none() {
                None
            } else if let Some(id) = self.done.get(&r.id.0) {
                Some(*id)
            } else {
                let source = self.h1_tag(r.id)?;
                let bytes = crate::h1::tag_bytes(self.h1, source, b"pphy", 0x34)?;
                let root = particles::point_physics(bytes)?;
                Some(self.cached(
                    source,
                    Converted {
                        group: FourCC(*b"pphy"),
                        root,
                        raw: vec![],
                    },
                )?)
            };
            physics.push(id);
        }
        let root = particles::contrail(&c, bitmap, &physics)?;
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

    fn particle_bitmap(&mut self, id: h1map::TagId) -> Result<((TagId, u16, u16), usize)> {
        let t = self.h1_tag(id)?;
        let bm = h1map::bitmap::Bitmap::read(self.h1, t)?;
        let image = bm.images.first().ok_or_else(|| {
            h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &t.name,
                "particle bitmap has no images",
            )
        })?;
        Ok((
            (
                self.source_bitmap(Some(id))?.expect("source bitmap"),
                image.width,
                image.height,
            ),
            bm.sequences.len(),
        ))
    }

    /// The `prt3` of particle `p` (from `tag`: a `part`, or a `pctl` whose
    /// particle state `p` stands for), pushed as `name`. Not cached.
    pub(super) fn particle_of(&mut self, tag: &Tag, name: &str, p: &H1Particle) -> Result<Option<TagId>> {
        if self.particle_shaders.is_none() {
            self.particle_shaders = Some(particles::particle_shaders(self.base)?);
        }
        use h1convert::shad::program::{glass, particle_vertex};
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.source = name.to_owned();
        let source = self.h1;
        let fail = |why: String| h1convert::Error::no_rebuild(&source.header().name, name, why);
        let (mut want, proven) = particles::particle_stem(p.blend);
        if !proven {
            return Err(fail(format!("no particle program for blend {}", p.blend)).into());
        }
        let mut mask = None;
        let mut orientation = 0;
        if p.blend == 6 {
            if p.bitmap.is_none() {
                return Err(fail("blend 6 has no primary bitmap".into()).into());
            }
            let m = particles::Mask::read(source, tag)?;
            let data = source.tag_data(tag)?;
            orientation = u16::from_le_bytes(data[0xac..0xae].try_into().unwrap());
            let fade = u16::from_le_bytes(data[0xdc..0xde].try_into().unwrap());
            if fade != 0 && orientation != 0 {
                return Err(fail(format!("unsupported blend 6 angular fade {fade}")).into());
            }
            if orientation > 1 {
                return Err(fail(format!(
                    "unsupported blend 6 particle orientation {orientation}"
                ))
                .into());
            }
            let coordinates = m.map_or(glass::ParticleCoordinates::Primary, |m| m.coordinates);
            want = coordinates.stem();
            if !self
                .particle_shaders
                .as_ref()
                .unwrap()
                .iter()
                .any(|s| s.name == want)
            {
                let vertex = if m.is_some() {
                    self.program_tag(
                        want,
                        b"vrtx",
                        particle_vertex::build(coordinates == glass::ParticleCoordinates::Screen)?,
                    )
                } else {
                    self.base
                        .find(b"vrtx", glass::PARTICLE_VERTEX)
                        .ok_or_else(|| fail(format!("missing {}", glass::PARTICLE_VERTEX)))?
                };
                let pass =
                    self.program_tag(want, b"spas", glass::maximum_pass(vertex, coordinates)?);
                let template = glass::maximum_graph(pass, coordinates);
                let stem = self.program_tag(want, b"stem", glass::maximum_template(&template)?);
                let mut vectors = vec![[1., 1., 0., 0.], [0., 0., 0., 1.]];
                if m.is_some() {
                    vectors.push([1., 1., 0., 0.]);
                }
                self.particle_shaders
                    .as_mut()
                    .unwrap()
                    .push(particles::ParticleShader {
                        name: want.to_string(),
                        stem,
                        template,
                        colors: vec![],
                        vectors,
                    });
            }
            if let Some(m) = m {
                mask = Some(m.bind(self.particle_bitmap(m.bitmap)?.0));
            }
        }
        let shader = self
            .particle_shaders
            .as_ref()
            .unwrap()
            .iter()
            .find(|s| s.name == want)
            .cloned()
            .ok_or_else(|| fail(format!("missing particle program {want}")))?;
        let bitmap = p.bitmap.map(|id| self.particle_bitmap(id)).transpose()?;
        let mut particle = p.clone();
        if p.blend == 6 {
            let total = bitmap.expect("checked primary bitmap").1;
            let (first, count) = particles::maximum_sequences(p.sequences, total)
                .ok_or_else(|| fail("unsupported blend 6 sprite sequence phases".into()))?;
            particle.sequences = [first, count, 0, 0];
        }
        let bitmap = bitmap.map(|(binding, _)| binding);
        let effect = |c: &mut Self,
                      id: Option<h1map::TagId>,
                      rep: &mut super::ObjectReport|
         -> Result<Option<(FourCC, TagId)>> {
            let Some(id) = id else { return Ok(None) };
            let t = c.h1_tag(id)?;
            let group = FourCC(t.group.0.to_be_bytes());
            let id = match &group.0 {
                b"effe" => c.effect(t)?,
                b"snd!" => c.sound(t, rep)?,
                _ => {
                    return Err(h1convert::Error::no_rebuild(
                        &c.h1.header().name,
                        &t.name,
                        "particle event group",
                    )
                    .into());
                }
            };
            Ok(Some((group, id)))
        };
        let collision = effect(self, p.collision_effect, &mut rep)?;
        let death = effect(self, p.death_effect, &mut rep)?;
        rep.no_home.push("Lifespan, fade times, radius, frame animation, physics, contact deterioration and fade sizes belong to the emitter".into());
        if p.blend != 6 {
            rep.no_home
                .push("Particle orientation and secondary map are not carried".into());
        }
        let (root, roles) = crate::provenance::capture(|| {
            particles::particle_with_mask(
                &particle,
                &shader,
                bitmap,
                mask,
                orientation,
                collision,
                death,
            )
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
        let d = tags::defs::Defs::builtin()
            .group(FourCC(*b"effe"))
            .ok_or_else(|| crate::Error::Invalid("no effe definition".into()))?;
        let inv = |e: tags::defs::DefError| crate::Error::Invalid(e.to_string());
        let mut r = tags::defs::from_built(d, &root).map_err(inv)?;
        let Some(tags::defs::Val::Block(mut evs)) = r.at("events").cloned() else {
            return Ok(root);
        };
        let mut made = 0usize;
        for (k, (duration, list)) in events.iter().enumerate() {
            let ev = evs.get_mut(k).ok_or_else(|| {
                h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &tag.name,
                    format!("missing output event {k}"),
                )
            })?;
            let d = (duration[0] + duration[1]) * 0.5;
            let mut systems = Vec::new();
            for p in list {
                let Some(source) = p.particle else { continue };
                let t = self.h1_tag(source)?;
                let id = self.particle(t)?.ok_or_else(|| {
                    h1convert::Error::no_rebuild(
                        &self.h1.header().name,
                        &t.name,
                        "missing particle definition",
                    )
                })?;
                let h = H1Particle::read(self.h1, t)?;
                systems.push(particles::particle_system(p, &h, id, d)?);
            }
            for (id, location) in systems_of.get(k).into_iter().flatten() {
                let t = self.h1_tag(*id)?;
                let states = self.particle_system_states(t)?;
                for (prt3, state, count) in states {
                    systems.push(particles::particle_system(
                        &state.as_effect_particle(count, *location),
                        &state.as_particle(),
                        prt3,
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
        let mut ids = Vec::new();
        for (ty, states) in self.particle_types(tag)? {
            if ty.initial_count == 0
                && !ty
                    .emitter_states
                    .iter()
                    .any(|s| s.particles_per_second > 0.0)
            {
                continue;
            }
            let name = format!("{}\\{}\\attached_{marker}", tag.name, ty.name);
            let mut particles = Vec::new();
            for (index, (particle, source)) in states.into_iter().enumerate() {
                let movement = if let Some(physics) = source.physics.or(ty.default_physics) {
                    let p = crate::particle_movement::Halo1Physics::read(self.h1, physics)?;
                    let root =
                        p.movement(ty.flags, source.collision_radius_scale * ty.particle_radius)?;
                    Some(self.tree.push(
                        &format!("{name}\\movement_{index}"),
                        Converted {
                            group: FourCC(*b"pmov"),
                            root,
                            raw: Vec::new(),
                        },
                    ))
                } else {
                    None
                };
                particles.push(particles::AttachmentParticle {
                    tag: particle,
                    source,
                    movement,
                });
            }
            let root = particles::attached_particle_effect(&ty, &particles, marker);
            if let Some(root) = root? {
                let id = self.tree.push(
                    &name,
                    Converted {
                        group: FourCC(*b"effe"),
                        root,
                        raw: Vec::new(),
                    },
                );
                ids.push(id);
            }
        }
        self.attached_particles.insert(key, ids.clone());
        Ok(ids)
    }
}
