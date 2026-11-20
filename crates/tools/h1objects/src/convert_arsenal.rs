//! The converter's side of [`crate::arsenal`]: Halo 1 weapons, equipment,
//! projectiles and damage effects built from the definitions, each once.
//! A child of `convert`, for the converter's caches.

use h1map::Tag;
use h1_assets::models::Converted;
use h1_assets::models::mode::ModelOptions;
use map::{FourCC, TagId};
use tags::defs::Defs;

use super::{Converter, ObjectReport, template_candidates};
use crate::arsenal;
use crate::h1::{self, Equipment, Projectile, Weapon};
use crate::{Error, Result};

impl Converter<'_> {
    /// The material effects field of the H1 item layer.
    pub(super) fn h1_foot(&mut self, b: &[u8], rep: &mut ObjectReport) -> Result<Option<TagId>> {
        let Some(t) = h1::reference(self.h1, b, 0x248) else { return Ok(None) };
        let t = self.h1_tag(t)?;
        Ok(Some(self.foot(t, rep)?))
    }

    /// Halo 1's effect `id`, converted ([`Converter::effect`]); `None`
    /// when the source slot is empty.
    fn h1_effect(
        &mut self,
        id: Option<h1map::TagId>,
        what: &str,
        _rep: &mut ObjectReport,
    ) -> Result<Option<TagId>> {
        let Some(id) = id else {
            return Ok(None);
        };
        let t = self.h1_tag(id)?;
        self.effect(t).map(Some).map_err(|e| {
            h1convert::Error::no_rebuild(&self.h1.header().name, &t.name, format!("{what}: {e}"))
                .into()
        })
    }

    /// Halo 1's damage effect `id`, converted ([`Self::damage`]); `None`
    /// when the source slot is empty.
    fn h1_damage(
        &mut self,
        id: Option<h1map::TagId>,
        what: &str,
        _rep: &mut ObjectReport,
    ) -> Result<Option<TagId>> {
        let Some(id) = id else {
            return Ok(None);
        };
        let tag = self.h1_tag(id)?;
        self.damage(id).map(Some).map_err(|e| {
            h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, format!("{what}: {e}"))
                .into()
        })
    }

    /// The base's Halo 2 counterpart of a Halo 1 tag (the first
    /// [`super::TEMPLATES`] row the base holds), for the switchable Halo 2 HUD.
    fn counterpart(&self, group: &[u8; 4], path: &str) -> Option<(TagId, Vec<u8>)> {
        let d = Defs::builtin().group(FourCC(*group))?;
        let id = template_candidates(group, path)
            .into_iter()
            .find_map(|n| self.base.find(group, n))?;
        Some((id, self.base.head(id, d.size.into()).ok()?))
    }

    /// Halo 1's first-person arms (the model its globals name,
    /// `h1convert::globals::h1_first_person_hands`), converted on its own
    /// like a weapon's first-person model, once. The player
    /// representation names it (`h1convert::globals::set_player_hands`):
    /// the engine binds the arms' nodes to a weapon's first-person graph
    /// by name (docs/halo1/models.md, "First-person graphs"), and
    /// Halo 1's graphs name Halo 1's arms' nodes.
    pub fn first_person_arms(&mut self, id: h1map::TagId) -> Result<TagId> {
        let t = self.h1_tag(id)?;
        let mut rep = Self::report(t, "none: a render model on its own");
        let out = self.render_model(&mut rep, id, None);
        self.tree.reports.push(rep);
        out
    }

    /// A converted render model on its own (the first-person model), with
    /// its shaders, once. `rounds`: the weapon's, when its counters count
    /// its rounds ([`crate::h1::Weapon::rounds_counter`]).
    fn render_model(
        &mut self,
        rep: &mut ObjectReport,
        id: h1map::TagId,
        rounds: Option<i16>,
    ) -> Result<TagId> {
        let t = self.h1_tag(id)?;
        let key = (id, rounds, 0);
        if let Some(&mode) = self.render_models.get(&key) {
            return Ok(mode);
        }
        let (h1_model, _) = h1_assets::models::model_from_map(self.h1, t, &ModelOptions::default())?;
        let shaders = self.model_shaders(rep, &h1_model, rounds, 0)?;
        let opts = ModelOptions { shaders, ..ModelOptions::default() };
        let mut mode = h1_assets::models::mode::convert_model(
            &h1_model,
            h1_assets::models::short_name(&t.name),
            &opts,
        )?;
        crate::rebuild::mode_materials(&mut mode.root, &opts.shaders)?;
        let mode = self.tree.push(&t.name, mode);
        self.render_models.insert(key, mode);
        Ok(mode)
    }

    /// A converted animation graph, once.
    fn graph(&mut self, id: h1map::TagId, model: Option<h1map::TagId>) -> Result<TagId> {
        let t = self.h1_tag(id)?;
        if let Some(d) = self.done.get(&t.id.0) {
            return Ok(*d);
        }
        let m = model.map(|m| self.h1_tag(m)).transpose()?;
        let (_, _, mut conv) = h1_assets::models::graph_from_map(self.h1, t, m)?;
        self.graph_sounds(t, &mut conv)?;
        self.cached(t, conv)
    }

    pub(super) fn graph_sounds(&mut self, t: &Tag, conv: &mut Converted) -> Result<()> {
        // The sounds its animations play (the reloads, the ready), as Halo
        // 2 sound events (`h1_assets::models::sounds`).
        let sounds = h1_assets::models::sounds::read(self.h1, t)?;
        if !sounds.events.is_empty() {
            let mut rep = Self::report(t, "none: the graph's sounds");
            let mut targets = Vec::new();
            for r in &sounds.refs {
                let got = match r.map(|id| self.h1_tag(id)).transpose()? {
                    Some(s) => Some(self.sound(s, &mut rep)?),
                    None => None,
                };
                targets.push(got);
            }
            h1_assets::models::sounds::add_sound_events(&mut conv.root, &sounds, &targets)?;
            rep.no_home.push(format!(
                "{} animation sound events, {} of {} sounds converted",
                sounds.events.len(),
                targets.iter().flatten().count(),
                targets.len()
            ));
            self.tree.reports.push(rep);
        }
        Ok(())
    }

    /// A Halo 1 damage effect from the `jpt!` definition, once.
    pub fn damage(&mut self, id: h1map::TagId) -> Result<TagId> {
        let t = self.h1_tag(id)?;
        if let Some(d) = self.done.get(&t.id.0) {
            return Ok(*d);
        }
        let b = self.h1.tag_data(t)?;
        if b.len() < arsenal::H1_JPT_MIN {
            return Err(Error::Truncated("jpt! root"));
        }
        let sound = if let Some(id) = h1::reference(self.h1, b, 0x114) {
            let mut rep = Self::report(t, "none: rebuilt from tags::defs");
            let out = Some(self.sound(self.h1_tag(id)?, &mut rep)?);
            self.tree.reports.push(rep);
            out
        } else {
            None
        };
        let mut scale = None;
        if arsenal::has_vitality_damage(b) {
            let profile = crate::player_damage::scales(b)?;
            if profile != [1.0f32.to_bits(); 2] {
                scale = Some(crate::player_damage::damage_key(profile));
                self.tree.player_damage_scales.insert(profile);
            }
        }
        let root = arsenal::damage_with_scale(b, sound, scale.as_deref())?;
        self.cached(t, Converted {
            group: FourCC(*b"jpt!"),
            root,
            raw: Vec::new(),
        })
    }

    /// The projectile applies the event's damaging part itself. Keep any
    /// companion impulse or second damage in the effect so it fires once.
    fn detonation_effect(
        &mut self,
        source: Option<h1map::TagId>,
    ) -> Result<(Option<TagId>, Option<TagId>)> {
        let Some(id) = source else { return Ok((None, None)) };
        let tag = self.h1_tag(id)?;
        let effect = h1::Effect::read(self.h1, tag)?;
        let mut primary: Option<(h1map::TagId, usize, bool)> = None;
        for (index, part) in effect
            .events
            .iter()
            .flat_map(|event| &event.parts)
            .enumerate()
        {
            if part.group != h1map::Group::new(b"jpt!") {
                continue;
            }
            let Some(id) = part.reference else { continue };
            let bytes = h1::tag_bytes(
                self.h1,
                self.h1_tag(id)?,
                b"jpt!",
                arsenal::H1_JPT_MIN,
            )?;
            let has_damage = arsenal::has_vitality_damage(bytes);
            if primary.is_none_or(|(_, _, current)| !current && has_damage) {
                primary = Some((id, index, has_damage));
            }
        }
        let Some((primary, index, _)) = primary else {
            return Ok((Some(self.effect(tag)?), None));
        };
        let effect = self.effect_after_detonation(tag, index)?;
        Ok((Some(effect), Some(self.damage(primary)?)))
    }

    pub(super) fn weapon(&mut self, tag: &Tag) -> Result<TagId> {
        let mut w = Weapon::read(self.h1, tag)?;
        self.halo2_change_colours(&mut w.object);
        let bytes = arsenal::weapon_bytes(self.h1, tag)?;
        let cp = self.counterpart(b"weap", &tag.name);
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.physics = Some("none (rebuilt weapons borrow no phmo)".into());
        let rounds = w.rounds_counter();
        let hlmt = self.model(&mut rep, &w.object, &tag.name, rounds)?;
        let predicted = self.predicted(hlmt);
        let mut parts = arsenal::WeaponParts {
            hlmt,
            predicted,
            animation: self.weapon_class(&w.label)?.map(|c| (c, w.label.clone())),
            ..Default::default()
        };
        if parts.animation.is_none() {
            rep.no_home.push(format!(
                "weapon label {:?}: no graph on the map holds it under a weapon class; class and type left null (no unit can hold it)",
                w.label
            ));
        }
        if let Some(id) = w.triggers.first().and_then(|t| t.charging_effect) {
            let effect = self.h1_tag(id)?;
            let target = self.effect(effect).map_err(|err| Error::Invalid(format!(
                "{} {} charging effect {}: {err}", self.h1.header().name, tag.name, effect.name
            )))?;
            parts.charging_effect = Some(target);
        }
        for t in &w.triggers {
            let mut list = Vec::new();
            for f in &t.firing_effects {
                let mut damage = [None; 3];
                for (dest, source) in damage.iter_mut().zip(f.damage) {
                    *dest = if let Some(id) = source {
                        let feedback = self.h1_tag(id)?;
                        Some(self.damage(id).map_err(|err| Error::Invalid(format!(
                            "{} {} trigger damage {}: {err}",
                            self.h1.header().name, tag.name, feedback.name
                        )))?)
                    } else {
                        None
                    };
                }
                let [fire, misfire, empty] = f.effects;
                list.push(arsenal::FiringParts {
                    effects: [
                        self.weapon_slot(fire, &mut rep)?,
                        self.weapon_slot(misfire, &mut rep)?,
                        self.weapon_slot(empty, &mut rep)?,
                    ],
                    damage,
                    shot_count: f.shot_count,
                });
            }
            parts.firing_effects.push(list);
        }
        for m in &w.magazines {
            let fx = [
                self.weapon_slot(m.reloading_effect, &mut rep)?,
                self.weapon_slot(m.chambering_effect, &mut rep)?,
            ];
            parts.magazine_effects.push(fx);
            let mut objects = Vec::new();
            for (rounds, id) in &m.objects {
                if let Some(id) = id {
                    objects.push((*rounds, self.object(self.h1_tag(*id)?)?));
                }
            }
            parts.magazine_objects.push(objects);
        }
        // The weapon's own effects and sounds: Halo 1's, or none.
        for (path, at) in arsenal::WEAPON_EFFECTS.iter().filter(|(path, _)| !path.starts_with("item.")) {
            let id = h1::reference(self.h1, &bytes, *at);
            if let Some((g, x)) = self.weapon_slot(id, &mut rep)? {
                parts.halo1.push(((*path).to_owned(), g, x));
            }
        }
        for t in &w.triggers {
            let p = match t.projectile {
                Some(p) => {
                    Some(self.object(self.h1_tag(p)?)?)
                }
                None => None,
            };
            parts.projectiles.push(p);
        }
        let fp_model = match w.first_person_model {
            Some(m) => Some(self.render_model(&mut rep, m, rounds)?),
            None => None,
        };
        let fp_anim = w
            .first_person_animations
            .map(|a| self.graph(a, w.first_person_model))
            .transpose()?;
        parts.first_person = (fp_model, fp_anim);
        // Halo 1's weapon HUD (docs/halo1/hud.md): `+0x2b0` names it, and the
        // counterpart's becomes the Halo 2 side of the weapon's HUD pair.
        let mut swap = None;
        if let Some(wphi) = w.hud {
            let nhdt = self.weapon_hud(wphi)?;
            parts
                .halo1
                .push(arsenal::halo1_ref("hud", b"nhdt", Some(nhdt)));
            swap = cp
                .as_ref()
                .and_then(|(_, root)| self.base.reference(root, 0x2b0))
                .map(|h2| (nhdt, h2));
        }
        // Halo 1's in the slots named by pairs (arsenal::WEAPON_MELEE_DAMAGE),
        // after the weapon's own effects and sounds above (the overheated
        // effect, arsenal::WEAPON_OVERHEATED, is one of those).
        let melee = self.h1_damage(w.melee_damage, "melee damage", &mut rep)?;
        let response = self.h1_damage(w.melee_response, "melee response", &mut rep)?;
        parts.halo1.extend([
            arsenal::halo1_ref(arsenal::WEAPON_MELEE_DAMAGE, b"jpt!", melee),
            arsenal::halo1_ref(arsenal::WEAPON_MELEE_RESPONSE, b"jpt!", response),
        ]);
        parts.halo1.extend(self.item_effect_refs(&bytes, &mut rep)?);
        let root = arsenal::weapon(&w, &bytes, &parts).map_err(|err| {
            Error::Invalid(format!("{} {}: {err}", self.h1.header().name, tag.name))
        })?;
        let root = self.with_object_attachments(*b"weap", "item.obje", root, &bytes, &w.object, &mut rep)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"weap"),
                root,
                raw: Vec::new(),
            },
        );
        if let Some((halo1, halo2)) = swap {
            self.tree.hud_swaps.push(super::HudSwap {
                owner: id,
                halo1,
                halo2,
            });
        }
        Ok(id)
    }

    /// A weapon's Halo 1 effect or sound slot converted, by the group of
    /// the tag it names (`effe` with [`Converter::effect`], `snd!` with
    /// [`Converter::sound`]); one that does not convert is left out, the
    /// reason in `rep`.
    fn weapon_slot(
        &mut self,
        id: Option<h1map::TagId>,
        rep: &mut ObjectReport,
    ) -> Result<Option<(FourCC, TagId)>> {
        let Some(id) = id else { return Ok(None) };
        let t = self.h1_tag(id)?;
        let group = FourCC(t.group.0.to_be_bytes());
        let converted = match &group.0 {
            b"snd!" => self.sound(t, rep)?,
            b"effe" => self.effect(t)?,
            _ => {
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &t.name,
                    "unsupported weapon effect group",
                )
                .into());
            }
        };
        Ok(Some((group, converted)))
    }

    pub(super) fn projectile(&mut self, tag: &Tag) -> Result<TagId> {
        let mut p = Projectile::read(self.h1, tag)?;
        self.halo2_change_colours(&mut p.object);
        let b = h1::tag_bytes(self.h1, tag, b"proj", h1::PROJ_SIZE)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.physics = Some("none (rebuilt projectiles borrow no phmo)".into());
        let hlmt = self.model(&mut rep, &p.object, &tag.name, None)?;
        let impact_damage = match p.impact_damage {
            Some(j) => Some(self.damage(j)?),
            None => None,
        };
        // The primary detonation damage moves onto the projectile; the
        // rest of each event stays in its effect (Self::detonation_effect).
        let (detonation, detonation_damage) = self.detonation_effect(p.effect)?;
        let (sup, super_damage) = self.detonation_effect(p.super_detonation)?;
        let source = self.h1;
        let base = self.base;
        let responses = arsenal::responses(source, b, &base.materials, |id| {
            let effect = self.h1_tag(id)?;
            self.effect(effect)
        })?;
        let mut parts = arsenal::ProjectileParts {
            hlmt,
            impact_damage,
            detonation_damage,
            halo1: Vec::new(),
            responses,
        };
        // Halo 1's in the slots named by pairs (arsenal::PROJECTILE_FLYBY_SOUND).
        let started = self.h1_effect(p.detonation_started, "detonation started", &mut rep)?;
        let flyby = match p.flyby_sound {
            Some(s) => {
                let t = self.h1_tag(s)?;
                Some(self.sound(t, &mut rep)?)
            }
            None => None,
        };
        let attached = self.h1_damage(p.attached_detonation_damage, "attached detonation damage", &mut rep)?;
        parts.halo1 = vec![
            arsenal::halo1_ref("ref_108", b"jpt!", attached),
            arsenal::halo1_ref("super_detonation_damage", b"jpt!", super_damage),
            arsenal::halo1_ref("attached_super_detonation_damage", b"jpt!", super_damage),
            arsenal::halo1_ref(arsenal::PROJECTILE_DETONATION_STARTED, b"effe", started),
            arsenal::halo1_ref(arsenal::PROJECTILE_AIRBORNE_DETONATION, b"effe", detonation),
            arsenal::halo1_ref(arsenal::PROJECTILE_DETONATION, b"effe", detonation),
            arsenal::halo1_ref(arsenal::PROJECTILE_SUPER_DETONATION, b"effe", sup),
            arsenal::halo1_ref(arsenal::PROJECTILE_FLYBY_SOUND, b"snd!", flyby),
        ];
        let root = arsenal::projectile(&p, b, &parts)?;
        let root = self.with_object_attachments(*b"proj", "object", root, b, &p.object, &mut rep)?;
        rep.no_home.push(format!(
            "{} Halo 1 material responses as {}",
            p.material_responses.len(),
            parts.responses.len()
        ));
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"proj"),
                root,
                raw: Vec::new(),
            },
        );
        Ok(id)
    }

    pub(super) fn item_effect_refs(&mut self, bytes: &[u8], rep: &mut ObjectReport) -> Result<arsenal::Halo1Refs> {
        let mut refs = Vec::new();
        for &(path, at) in arsenal::WEAPON_EFFECTS.iter().filter(|(path, _)| path.starts_with("item.")) {
            if let Some((group, id)) = self.weapon_slot(h1::reference(self.h1, bytes, at), rep)? {
                refs.push((path.to_owned(), group, id));
            }
        }
        if let Some(foot) = self.h1_foot(bytes, rep)? {
            refs.push(arsenal::halo1_ref(arsenal::ITEM_FOOT, b"foot", Some(foot)));
        }
        Ok(refs)
    }

    pub(super) fn equipment(&mut self, tag: &Tag) -> Result<TagId> {
        let mut e = Equipment::read(self.h1, tag)?;
        if h1convert::sandbox::native_powerup(e.powerup_type) {
            return self.powerup(tag, &e);
        }
        self.halo2_change_colours(&mut e.object);
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.physics = Some("none (rebuilt equipment borrows no phmo)".into());
        let hlmt = self.model(&mut rep, &e.object, &tag.name, None)?;
        let predicted = self.predicted(hlmt);
        let mut parts = arsenal::EquipmentParts {
            hlmt,
            predicted,
            halo1: Vec::new(),
        };
        let eb = h1::tag_bytes(self.h1, tag, b"eqip", h1::EQIP_SIZE)?;
        parts.halo1 = self.item_effect_refs(eb, &mut rep)?;
        if let Some((group, id)) = self.weapon_slot(h1::reference(self.h1, eb, 0x310), &mut rep)? {
            parts.halo1.push(("snd_ref_134".to_owned(), group, id));
        }
        let root = arsenal::equipment(&e, eb, &parts)?;
        let root = self.with_object_attachments(*b"eqip", "item.obje", root, eb, &e.object, &mut rep)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"eqip"),
                root,
                raw: Vec::new(),
            },
        );
        Ok(id)
    }
}
