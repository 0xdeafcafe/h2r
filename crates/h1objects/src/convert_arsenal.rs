//! The converter's side of [`crate::arsenal`]: Halo 1 weapons, equipment,
//! projectiles and damage effects built from the definitions, each once.
//! A child of `convert`, for the converter's caches.

use h1map::Tag;
use h1models::Converted;
use h1models::mode::ModelOptions;
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::Defs;

use super::{Converter, ObjectReport, template_candidates};
use crate::arsenal::{self, Role};
use crate::h1::{self, Equipment, Projectile, Weapon};
use crate::{Error, Result};

/// A Halo 1 tag's own bytes: its data up to the next tag's (H1 tags are laid
/// out one after another, each with its child blocks after its root).
fn extent<'a>(map: &'a h1map::H1Map, tag: &Tag) -> Result<&'a [u8]> {
    let b = map.tag_data(tag)?;
    let Some(r) = tag.data.clone() else {
        return Ok(b);
    };
    let next = map
        .tags()
        .iter()
        .filter_map(|t| t.data.as_ref().map(|d| d.start))
        .filter(|s| *s > r.start)
        .min()
        .unwrap_or(r.end);
    Ok(&b[..(next.min(r.end) - r.start)])
}

impl Converter<'_> {
    /// The group's role words over the base ([`arsenal::role`]), once.
    fn item_role(&mut self, group: &[u8; 4]) -> Result<Role> {
        if let Some(r) = self.item_roles.get(group) {
            return Ok(r.clone());
        }
        let blocks: &[&str] = if group == b"weap" {
            arsenal::WEAPON_BLOCKS
        } else {
            &[]
        };
        let r = arsenal::role(self.base, group, blocks)?.unwrap_or_default();
        self.item_roles.insert(*group, r.clone());
        Ok(r)
    }

    /// Halo 1's material effects: the first `foot` the Halo 1 root `b`
    /// names, converted ([`Converter::foot`]); `None` when it names none
    /// or it does not convert (noted).
    fn h1_foot(&mut self, b: &[u8], rep: &mut ObjectReport) -> Result<Option<TagId>> {
        let foot = h1map::Group::new(b"foot");
        let Some(t) = crate::refs::scan(self.h1, b)
            .into_iter()
            .find(|f| f.group == foot)
            .and_then(|f| f.tag)
        else {
            return Ok(None);
        };
        let t = self.h1_tag(t)?;
        Ok(match self.foot(t, rep) {
            Ok(x) => Some(x),
            Err(e) => {
                rep.no_home
                    .push(format!("material effects {}: not converted ({e})", t.name));
                None
            }
        })
    }

    /// Halo 1's effect `id`, converted ([`Converter::effect`]); `None`
    /// when there is none or it does not convert (noted as `what`).
    fn h1_effect(
        &mut self,
        id: Option<h1map::TagId>,
        what: &str,
        rep: &mut ObjectReport,
    ) -> Result<Option<TagId>> {
        let Some(id) = id else {
            return Ok(None);
        };
        let t = self.h1_tag(id)?;
        Ok(match self.effect(t) {
            Ok(x) => Some(x),
            Err(e) => {
                rep.no_home
                    .push(format!("{what} {}: not converted ({e})", t.name));
                None
            }
        })
    }

    /// Halo 1's damage effect `id`, converted ([`Self::damage`]); `None`
    /// when there is none or it does not convert (noted as `what`).
    fn h1_damage(
        &mut self,
        id: Option<h1map::TagId>,
        what: &str,
        rep: &mut ObjectReport,
    ) -> Result<Option<TagId>> {
        let Some(id) = id else {
            return Ok(None);
        };
        Ok(match self.damage(id) {
            Ok(x) => Some(x),
            Err(e) => {
                rep.no_home.push(format!("{what}: not converted ({e})"));
                None
            }
        })
    }

    /// The base's Halo 2 counterpart of a Halo 1 tag (the first
    /// [`super::TEMPLATES`] row the base holds): only its references are
    /// used, for slots Halo 1 has nothing converted for.
    fn counterpart(&self, group: &[u8; 4], path: &str) -> Option<(TagId, Vec<u8>)> {
        let d = Defs::builtin().group(FourCC(*group))?;
        let id = template_candidates(group, path)
            .into_iter()
            .find_map(|n| self.base.find(group, n))?;
        Some((id, self.base.head(id, d.size.into()).ok()?))
    }

    fn rebuilt_report(
        tag: &Tag,
        counterpart: Option<TagId>,
        base: &crate::base::Base,
    ) -> ObjectReport {
        let c = counterpart
            .and_then(|c| base.name(c))
            .map_or("none".to_owned(), |n| n.1.to_owned());
        Self::report(
            tag,
            &format!("none: rebuilt from tags::defs (Halo 2 references from {c})"),
        )
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
        let key = (id, rounds);
        if let Some(&mode) = self.render_models.get(&key) {
            return Ok(mode);
        }
        let (h1_model, _) = h1models::model_from_map(self.h1, t, &ModelOptions::default())?;
        let shaders = self.model_shaders(rep, &h1_model, rounds);
        let mut padded = shaders.clone();
        padded.resize(h1_model.shaders.len(), TagId::NULL);
        let mut mode = h1models::mode::convert_model(
            &h1_model,
            h1models::short_name(&t.name),
            &ModelOptions {
                shaders,
                ..ModelOptions::default()
            },
        )?;
        crate::rebuild::mode_materials(&mut mode.root, &padded)?;
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
        let (_, _, mut conv) = h1models::graph_from_map(self.h1, t, m)?;
        // The sounds its animations play (the reloads, the ready), as Halo
        // 2 sound events (`h1models::sounds`).
        let sounds = h1models::sounds::read(self.h1, t)?;
        if !sounds.events.is_empty() {
            let mut rep = Self::report(t, "none: the graph's sounds");
            let mut targets = Vec::new();
            for r in &sounds.refs {
                let got = match r.map(|id| self.h1_tag(id)).transpose()? {
                    Some(s) => self.sound(s, &mut rep),
                    None => None,
                };
                targets.push(got);
            }
            h1models::sounds::add_sound_events(&mut conv.root, &sounds, &targets)?;
            rep.no_home.push(format!(
                "{} animation sound events, {} of {} sounds converted",
                sounds.events.len(),
                targets.iter().flatten().count(),
                targets.len()
            ));
            self.tree.reports.push(rep);
        }
        self.cached(t, conv)
    }

    /// A Halo 1 damage effect from the `jpt!` definition, once.
    pub(super) fn damage(&mut self, id: h1map::TagId) -> Result<TagId> {
        let t = self.h1_tag(id)?;
        if let Some(d) = self.done.get(&t.id.0) {
            return Ok(*d);
        }
        let b = self.h1.tag_data(t)?;
        if b.len() < arsenal::H1_JPT_MIN {
            return Err(Error::Truncated("jpt! root"));
        }
        let role = self.item_role(b"jpt!")?;
        let (root, roles) = crate::provenance::capture(|| arsenal::damage(b, &role));
        let id = self.cached(
            t,
            Converted {
                group: FourCC(*b"jpt!"),
                root: root?,
                raw: Vec::new(),
            },
        )?;
        self.tree.note_roles(id, roles);
        Ok(id)
    }

    /// The first `jpt!` a Halo 1 effect names (its events' parts), for a
    /// projectile whose detonation damage Halo 1 keeps in its effect.
    fn effect_damage(&self, effe: Option<h1map::TagId>) -> Option<h1map::TagId> {
        let t = self.h1.tag(effe?).ok()?;
        let b = extent(self.h1, t).ok()?;
        let jpt = h1map::Group::new(b"jpt!");
        crate::refs::scan(self.h1, b)
            .into_iter()
            .find(|f| f.group == jpt && f.tag.is_some())?
            .tag
    }

    pub(super) fn weapon(&mut self, tag: &Tag) -> Result<TagId> {
        let w = Weapon::read(self.h1, tag)?;
        let bytes = arsenal::weapon_bytes(self.h1, tag)?;
        let cp = self.counterpart(b"weap", &tag.name);
        let mut rep = Self::rebuilt_report(tag, cp.as_ref().map(|c| c.0), self.base);
        rep.physics = Some("none (rebuilt weapons borrow no phmo)".into());
        let rounds = w.rounds_counter();
        let hlmt = self.model(&mut rep, &w.object, None, &tag.name, true, rounds)?;
        let predicted = self.predicted(hlmt);
        let d = Defs::builtin().group(FourCC(*b"weap")).expect("weap");
        let bd = d.child("barrels").expect("barrels");
        let fd = d.child("barrels.block_e4").expect("firing effects");
        // Per counterpart barrel, its firing effects' damage (the shot's
        // rumble and shake): `jpt!` from Halo 1 carries only damage bounds,
        // so these stay Halo 2's, as the melee damage does.
        let mut halo2_damage: Vec<Vec<[Option<TagId>; 3]>> = Vec::new();
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
        if let Some((_, root)) = &cp {
            parts.halo2 = arsenal::halo2_refs(
                self.base,
                d,
                root,
                &["item.obje.model", "first_person", "barrels", "magazines"],
            );
            let mut i = 0;
            while let Some(e) = self.base.element(root, 0x2d0, bd.size as usize, i)? {
                parts
                    .barrel_halo2
                    .push(arsenal::halo2_refs(self.base, bd, &e, &["projectile"]));
                let mut list = Vec::new();
                let mut k = 0;
                while let Some(x) = self.base.element(&e, 0xe4, fd.size as usize, k)? {
                    list.push(arsenal::FIRING_DAMAGE_AT.map(|at| self.base.reference(&x, at)));
                    k += 1;
                }
                halo2_damage.push(list);
                i += 1;
            }
        }
        // What a shot plays: Halo 1's effects, converted (their sounds with
        // them), never the counterpart's.
        for (i, t) in w.triggers.iter().enumerate() {
            let mut list = Vec::new();
            let barrel_damage = halo2_damage.get(i).or(halo2_damage.last());
            for (k, f) in t.firing_effects.iter().enumerate() {
                let damage = barrel_damage
                    .and_then(|l| l.get(k).or(l.last()))
                    .copied()
                    .unwrap_or_default();
                for (j, n) in damage
                    .iter()
                    .flatten()
                    .filter_map(|j| Some((j, self.base.name(*j)?.1)))
                {
                    rep.no_home.push(format!(
                        "Halo 2 reference barrels[{i}].block_e4[{k}] damage: jpt! {n} ({j:?})"
                    ));
                }
                list.push(arsenal::FiringParts {
                    effects: f.effects.map(|e| self.weapon_slot(e, &mut rep)),
                    damage,
                });
            }
            parts.firing_effects.push(list);
        }
        for m in &w.magazines {
            let fx = [
                self.weapon_slot(m.reloading_effect, &mut rep),
                self.weapon_slot(m.chambering_effect, &mut rep),
            ];
            parts.magazine_effects.push(fx);
        }
        // The weapon's own effects and sounds: Halo 1's, or none.
        parts
            .halo2
            .retain(|r| !arsenal::WEAPON_EFFECTS.iter().any(|(p, _)| *p == r.0));
        for (path, at) in arsenal::WEAPON_EFFECTS {
            let id = h1::reference(self.h1, &bytes, *at);
            if let Some((g, x)) = self.weapon_slot(id, &mut rep) {
                parts.halo1.push(((*path).to_owned(), g, x));
            }
        }
        for t in &w.triggers {
            let p = match t.projectile {
                Some(p) => {
                    let tpl = cp.as_ref().and_then(|c| {
                        let i = parts.projectiles.len();
                        let e = self
                            .base
                            .element(&c.1, 0x2d0, bd.size as usize, i)
                            .ok()
                            .flatten()?;
                        self.base.reference(&e, 0x8c)
                    });
                    let pt = self.h1_tag(p)?;
                    Some(self.object_with(pt, tpl)?)
                }
                None => None,
            };
            parts.projectiles.push(p);
        }
        let fp_model = match w.first_person_model {
            Some(m) => Some(self.render_model(&mut rep, m, rounds)?),
            None => None,
        };
        let fp_anim = match w.first_person_animations {
            Some(a) => match self.graph(a, w.first_person_model) {
                Ok(id) => Some(id),
                Err(e) => {
                    rep.no_home
                        .push(format!("first-person animations do not convert: {e}"));
                    None
                }
            },
            None => None,
        };
        parts.first_person = (fp_model, fp_anim);
        // Halo 1's weapon HUD (docs/halo1/hud.md): `+0x2b0` names it, and the
        // counterpart's becomes the Halo 2 side of the weapon's HUD pair.
        let mut halo2_hud = None;
        if let Some(wphi) = w.hud {
            match self.weapon_hud(wphi) {
                Ok((nhdt, skipped)) => {
                    match parts.halo2.iter_mut().find(|r| r.0 == "hud") {
                        Some(r) => {
                            halo2_hud = Some(r.2);
                            r.2 = nhdt;
                        }
                        None => parts.halo2.push(("hud".into(), FourCC(*b"nhdt"), nhdt)),
                    }
                    for s in skipped {
                        rep.no_home.push(format!("HUD: {s}"));
                    }
                }
                Err(e) => rep
                    .no_home
                    .push(format!("HUD: Halo 1's does not convert: {e}")),
            }
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
        if let Some(f) = self.h1_foot(&bytes[..h1::WEAP_SIZE.min(bytes.len())], &mut rep)? {
            parts
                .halo1
                .push(arsenal::halo1_ref(arsenal::ITEM_FOOT, b"foot", Some(f)));
        }
        // The held weapon's integrated-light input carries the unit's
        // flashlight to light attachments, including a50's marine rifle.
        let inputs = h1::FunctionInputs::read(&bytes, w.object.object_type);
        if inputs.weapon.contains(&14) {
            let functions = crate::functions::convert(
                &h1::functions(self.h1, &bytes)?,
                &inputs,
                &mut rep.no_home,
            );
            for attachment in h1::attachments(self.h1, &bytes)? {
                if attachment.group != h1map::Group::new(b"ligh") {
                    continue;
                }
                let Some(tag) = attachment.tag else { continue };
                let scale = crate::functions::primary_scale(attachment.primary_scale, &functions);
                if !functions.iter().flatten().any(|f| {
                    f.export == scale && f.import == "integrated_light_power"
                }) {
                    continue;
                }
                let light = self.h1_tag(tag)?;
                let tag = self.light(light)?;
                parts.attachments.push(crate::rebuild::AttachmentSpec {
                    group: *b"ligh",
                    tag,
                    marker: attachment.marker,
                    primary_scale: scale,
                    h1_primary: attachment.primary_scale,
                });
            }
            parts.functions = functions.into_iter().flatten().collect();
        }

        let role = self.item_role(b"weap")?;
        let (root, roles) =
            crate::provenance::capture(|| arsenal::weapon(&w, &bytes, &parts, &role));
        let root = root?;
        let halo1 = &parts.halo1;
        for (p, g, id) in parts
            .halo2
            .iter()
            .filter(|r| !halo1.iter().any(|h| h.0 == r.0))
        {
            let n = self.base.name(*id).map_or("?", |n| n.1);
            rep.no_home.push(format!("Halo 2 reference {p}: {g} {n}"));
        }
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"weap"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        if let (Some(halo2), Some((_, _, halo1))) =
            (halo2_hud, parts.halo2.iter().find(|r| r.0 == "hud"))
        {
            self.tree.hud_swaps.push(super::HudSwap {
                owner: id,
                halo1: *halo1,
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
    ) -> Option<(FourCC, TagId)> {
        let t = self.h1_tag(id?).ok()?;
        if t.group == h1map::Group::new(b"snd!") {
            return Some((FourCC(*b"snd!"), self.sound(t, rep)?));
        }
        Some((FourCC(*b"effe"), self.weapon_effect(id, rep)?))
    }

    /// A weapon's Halo 1 effect converted ([`Converter::effect`]); one that
    /// does not convert is left out, the reason in `rep`.
    fn weapon_effect(&mut self, id: Option<h1map::TagId>, rep: &mut ObjectReport) -> Option<TagId> {
        let t = self.h1_tag(id?).ok()?;
        match self.effect(t) {
            Ok(x) => Some(x),
            Err(e) => {
                rep.no_home
                    .push(format!("effe {}: not converted ({e})", t.name));
                None
            }
        }
    }

    /// `counterpart`: the Halo 2 projectile the counterpart weapon's barrel
    /// fires, when there is one.
    pub(super) fn projectile(&mut self, tag: &Tag, counterpart: Option<TagId>) -> Result<TagId> {
        let p = Projectile::read(self.h1, tag)?;
        let b = h1::tag_bytes(self.h1, tag, b"proj", h1::PROJ_SIZE)?;
        let d = Defs::builtin().group(FourCC(*b"proj")).expect("proj");
        let cp = counterpart
            .and_then(|c| Some((c, self.base.head(c, d.size.into()).ok()?)))
            .or_else(|| self.counterpart(b"proj", &tag.name));
        let mut rep = Self::rebuilt_report(tag, cp.as_ref().map(|c| c.0), self.base);
        rep.physics = Some("none (rebuilt projectiles borrow no phmo)".into());
        let hlmt = self.model(&mut rep, &p.object, None, &tag.name, true, None)?;
        let impact_damage = match p.impact_damage {
            Some(j) => Some(self.damage(j)?),
            None => None,
        };
        let det = p
            .attached_detonation_damage
            .or_else(|| self.effect_damage(p.effect))
            .or_else(|| self.effect_damage(p.super_detonation));
        let detonation_damage = match det {
            Some(j) => Some(self.damage(j)?),
            None => None,
        };
        let mut parts = arsenal::ProjectileParts {
            hlmt,
            impact_damage,
            detonation_damage,
            halo2: Vec::new(),
            halo1: Vec::new(),
            responses: arsenal::responses(self.h1, b, &self.base.materials)?,
        };
        if let Some((_, root)) = &cp {
            // Damage is Halo 1's; every other reference (effects, sounds,
            // the counterpart's other damage slots) is Halo 2's.
            parts.halo2 = arsenal::halo2_refs(
                self.base,
                d,
                root,
                &["object.model", "impact_damage", "detonation_damage"],
            );
            // Every damage slot is Halo 1's or none.
            parts.halo2.retain(|r| r.1 != FourCC(*b"jpt!"));
        }
        // Halo 1's in the slots named by pairs (arsenal::PROJECTILE_FLYBY_SOUND).
        let started = self.h1_effect(p.detonation_started, "detonation started", &mut rep)?;
        let detonation = self.h1_effect(p.effect, "detonation effect", &mut rep)?;
        let sup = self.h1_effect(p.super_detonation, "super detonation", &mut rep)?;
        let flyby = match p.flyby_sound {
            Some(s) => {
                let t = self.h1_tag(s)?;
                self.sound(t, &mut rep)
            }
            None => None,
        };
        parts.halo1 = vec![
            arsenal::halo1_ref(arsenal::PROJECTILE_DETONATION_STARTED, b"effe", started),
            arsenal::halo1_ref(arsenal::PROJECTILE_AIRBORNE_DETONATION, b"effe", detonation),
            arsenal::halo1_ref(arsenal::PROJECTILE_DETONATION, b"effe", detonation),
            arsenal::halo1_ref(arsenal::PROJECTILE_SUPER_DETONATION, b"effe", sup),
            arsenal::halo1_ref(arsenal::PROJECTILE_FLYBY_SOUND, b"snd!", flyby),
        ];
        if let Some(f) = self.h1_foot(b, &mut rep)? {
            parts.halo1.push(arsenal::halo1_ref(
                arsenal::PROJECTILE_FOOT,
                b"foot",
                Some(f),
            ));
        }
        let role = self.item_role(b"proj")?;
        let (root, roles) =
            crate::provenance::capture(|| arsenal::projectile(&p, b, &parts, &role));
        let root = root?;
        let root = self.halo1_attachments(*b"proj", "object", root, b, &p.object, &mut rep)?;
        let halo1 = &parts.halo1;
        for (path, g, id) in parts
            .halo2
            .iter()
            .filter(|r| !halo1.iter().any(|h| h.0 == r.0))
        {
            let n = self.base.name(*id).map_or("?", |n| n.1);
            rep.no_home
                .push(format!("Halo 2 reference {path}: {g} {n}"));
        }
        rep.no_home.push(format!(
            "{} Halo 1 material responses as {} (their effects null: Halo 1's effects are not converted)",
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
        self.tree.note_roles(id, roles);
        Ok(id)
    }

    pub(super) fn equipment(&mut self, tag: &Tag) -> Result<TagId> {
        let e = Equipment::read(self.h1, tag)?;
        let d = Defs::builtin().group(FourCC(*b"eqip")).expect("eqip");
        let cp = self.counterpart(b"eqip", &tag.name);
        let mut rep = Self::rebuilt_report(tag, cp.as_ref().map(|c| c.0), self.base);
        rep.physics = Some("none (rebuilt equipment borrows no phmo)".into());
        let hlmt = self.model(&mut rep, &e.object, None, &tag.name, true, None)?;
        let predicted = self.predicted(hlmt);
        let mut parts = arsenal::EquipmentParts {
            hlmt,
            predicted,
            halo2: Vec::new(),
            halo1: Vec::new(),
        };
        if let Some((_, root)) = &cp {
            parts.halo2 = arsenal::halo2_refs(self.base, d, root, &["item.obje.model"]);
        }
        let eb = h1::tag_bytes(self.h1, tag, b"eqip", h1::EQIP_SIZE)?;
        if let Some(f) = self.h1_foot(eb, &mut rep)? {
            parts
                .halo1
                .push(arsenal::halo1_ref(arsenal::ITEM_FOOT, b"foot", Some(f)));
        }
        let role = self.item_role(b"eqip")?;
        let (root, roles) = crate::provenance::capture(|| arsenal::equipment(&e, &parts, &role));
        let root: Block = root?;
        let halo1 = &parts.halo1;
        for (path, g, id) in parts
            .halo2
            .iter()
            .filter(|r| !halo1.iter().any(|h| h.0 == r.0))
        {
            let n = self.base.name(*id).map_or("?", |n| n.1);
            rep.no_home
                .push(format!("Halo 2 reference {path}: {g} {n}"));
        }
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"eqip"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(id)
    }
}
