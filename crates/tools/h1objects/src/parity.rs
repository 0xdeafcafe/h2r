//! Independent behaviour references from the owner's Halo 1 Xbox cache and executable.
use crate::{Error, Result, h1};
use h1map::{H1Map, Tag};
use std::collections::BTreeSet;

#[derive(Debug)]
pub struct Rules {
    pub hz: f32,
    pub press_mask: u8,
    body_scale: usize,
    shield_scale: usize,
    material_base: usize,
}

impl Rules {
    /// Follow the immediates in the retail firing and damage readers.
    /// Refuse an executable with different instructions at these sites.
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let x = xbe::Xbe::parse(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
        let operand = |va, prefix: &[u8], width| -> Result<usize> {
            let b = x
                .bytes_at(va, prefix.len() + width)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            immediate(b, prefix, width)
                .map_err(|e| Error::Invalid(format!("Halo 1 XBE {va:#x}: {e}")))
        };
        let address = operand(0xbdd14, &[0xd8, 0x3d], 4)? as u32;
        let hz = f32::from_bits(
            x.read_u32(address)
                .map_err(|e| Error::Invalid(e.to_string()))?,
        );
        if !hz.is_finite() || hz <= 0. || hz > 240. {
            return Err(Error::Invalid("invalid XBE tick rate".into()));
        }
        Ok(Self {
            hz,
            press_mask: operand(0xbdd69, &[0xf6, 0xc2], 1)? as u8,
            body_scale: operand(0x94f3f, &[0xd8, 0x4a], 1)?,
            shield_scale: operand(0x94a37, &[0xd8, 0x49], 1)?,
            material_base: operand(0x956ab, &[0x81, 0xc6], 4)?
                + operand(0x9505c, &[0xd8, 0x4c, 0x82], 1)?,
        })
    }
}

fn immediate(bytes: &[u8], prefix: &[u8], width: usize) -> Result<usize> {
    let op = bytes
        .strip_prefix(prefix)
        .ok_or_else(|| Error::Invalid("opcode mismatch".into()))?;
    match (width, op) {
        (1, [n]) => Ok(*n as usize),
        (4, [a, b, c, d]) => Ok(u32::from_le_bytes([*a, *b, *c, *d]) as usize),
        _ => Err(Error::Invalid("bad operand width".into())),
    }
}

fn float(b: &[u8], off: usize) -> Result<f32> {
    let n = f32::from_le_bytes(
        b.get(off..off + 4)
            .ok_or(Error::Truncated("parity reference"))?
            .try_into()
            .unwrap(),
    );
    if n.is_finite() {
        Ok(n)
    } else {
        Err(Error::Invalid(format!("non-finite reference at {off:#x}")))
    }
}

pub struct Gun {
    pub weapon: h1::Weapon,
    pub charge_seconds: Vec<f32>,
}

pub fn gun(map: &H1Map, tag: &Tag) -> Result<Gun> {
    let root = h1::tag_bytes(map, tag, b"weap", h1::WEAP_SIZE)?;
    let charge_seconds = h1::elements(map, root, 0x4fc, h1::TRIGGER_SIZE)?
        .iter()
        .map(|t| float(t, 0x48))
        .collect::<Result<_>>()?;
    Ok(Gun {
        weapon: h1::Weapon::read(map, tag)?,
        charge_seconds,
    })
}

#[derive(Debug)]
pub struct Pose {
    pub mode: String,
    pub class: String,
    pub kind: String,
    pub state: &'static str,
    pub action: &'static str,
    pub index: i16,
    pub clip: String,
    pub frames: u16,
    pub overlay: bool,
}

pub fn poses(map: &H1Map, graph: &Tag) -> Result<Vec<Pose>> {
    let g = h1_assets::models::h1::Graph::read(map, graph.data_address)?;
    let mut rows = Vec::new();
    let mut add = |mode: &str,
                   class: &str,
                   kind: &str,
                   state: &'static str,
                   action: &'static str,
                   slots: &[i16],
                   slot: usize,
                   overlay: bool|
     -> Result<()> {
        let Some(&index) = slots.get(slot) else {
            return Ok(());
        };
        if index == -1 {
            return Ok(());
        } // The source graph assigns no animation to this slot.
        let clip = g.animations.get(index as usize).ok_or_else(|| {
            Error::Invalid(format!("{}: slot {slot} refers to {index}", graph.name))
        })?;
        rows.push(Pose {
            mode: h1_assets::models::names::mode_name(mode),
            class: class.to_owned(),
            kind: kind.to_owned(),
            state,
            action,
            index,
            clip: clip.name.clone(),
            frames: clip.frame_count,
            overlay,
        });
        Ok(())
    };
    for u in &g.units {
        if !matches!(u.label.as_str(), "stand" | "crouch" | "alert") {
            continue;
        }
        add(
            &u.label,
            "any",
            "any",
            "die",
            "airborne_dead",
            &u.slots,
            0,
            false,
        )?;
        for c in &u.classes {
            let class = if c.label.is_empty() { "any" } else { &c.label };
            for (state, action, slot, overlay) in [
                (
                    if u.label == "crouch" {
                        "crouch"
                    } else {
                        "stand"
                    },
                    "idle",
                    0,
                    false,
                ),
                ("move", "move_front", 8, false),
                ("jump", "airborne", 16, false),
                ("throw", "throw_grenade", 20, false),
                ("aim", "aim_still_up", 25, true),
            ] {
                add(
                    &u.label, class, "any", state, action, &c.slots, slot, overlay,
                )?;
            }
            for (kind, slots) in &c.types {
                let kind = if kind.is_empty() { "any" } else { kind };
                for (state, action, slot) in [
                    ("reload", "reload_full", 0),
                    ("fire", "fire_1", 4),
                    ("melee", "melee", 8),
                ] {
                    add(&u.label, class, kind, state, action, slots, slot, false)?;
                }
            }
        }
    }
    Ok(rows)
}

#[derive(Debug)]
pub struct RegionRay {
    pub name: String,
    pub region: i16,
    pub node: usize,
    pub material: String,
    pub material_type: u16,
    pub center: [f32; 3],
    pub normal: [f32; 3],
    pub body_scale: f32,
    pub shield_scale: f32,
}

pub struct Target {
    pub graph: String,
    pub poses: Vec<Pose>,
    pub rays: Vec<RegionRay>,
    pub body: f32,
    pub shield: f32,
    pub shield_type: u16,
}

pub fn target(map: &H1Map, biped: &Tag, rules: &Rules) -> Result<Target> {
    let obj = h1::Object::read(map, h1::tag_bytes(map, biped, b"bipd", h1::BIPED_SIZE)?)?;
    let missing = |what| Error::Invalid(format!("{}: no {what}", biped.name));
    let graph = map.tag(obj.animation.ok_or_else(|| missing("animation graph"))?)?;
    let tag = map.tag(obj.collision.ok_or_else(|| missing("collision"))?)?;
    let model = map.tag(obj.model.ok_or_else(|| missing("render model"))?)?;
    let nodes = h1_assets::models::h1::Model::read(map, model.data_address)?.nodes;
    let coll = h1::Collision::read(map, tag)?;
    let root = map.tag_data(tag)?;
    let materials = h1::elements(map, root, 0x234, 0x48)?;
    let mut rays = Vec::new();
    for n in &coll.nodes {
        for bsp in n.bsps.iter().take(1) {
            // default permutation on a newly spawned biped
            let mut seen = BTreeSet::new();
            for (i, s) in bsp.surfaces.iter().enumerate() {
                if !seen.insert(s.material) {
                    continue;
                }
                let k = s.material as usize;
                let mat = materials
                    .get(k)
                    .ok_or_else(|| missing("surface material"))?;
                let plane = bsp
                    .planes
                    .get((s.plane & 0x7fff_ffff) as usize)
                    .ok_or_else(|| missing("surface plane"))?;
                let sign = if s.plane >> 31 == 0 { 1. } else { -1. };
                let normal = [plane[0] * sign, plane[1] * sign, plane[2] * sign];
                let mut center = [0.; 3];
                let mut edge = s.first_edge;
                let mut count = 0;
                loop {
                    let e = bsp
                        .edges
                        .get(edge as usize)
                        .ok_or_else(|| missing("surface edge"))?;
                    let (vertex, next) = if e.left_surface == i as u32 {
                        (e.start_vertex, e.forward_edge)
                    } else if e.right_surface == i as u32 {
                        (e.end_vertex, e.reverse_edge)
                    } else {
                        return Err(missing("surface edge incidence"));
                    };
                    let p = bsp
                        .vertices
                        .get(vertex as usize)
                        .ok_or_else(|| missing("surface vertex"))?
                        .point;
                    for j in 0..3 {
                        center[j] += p[j];
                    }
                    count += 1;
                    edge = next;
                    if edge == s.first_edge {
                        break;
                    }
                    if count >= bsp.edges.len() {
                        return Err(missing("closed edge ring"));
                    }
                }
                let node = nodes
                    .iter()
                    .position(|r| r.name == n.name)
                    .ok_or_else(|| missing("model node for collision node"))?;
                let region = coll
                    .regions
                    .get(n.region as usize)
                    .ok_or_else(|| missing("collision region"))?;
                rays.push(RegionRay {
                    name: region.name.clone(),
                    region: n.region,
                    node,
                    material: coll.materials[k].clone(),
                    material_type: coll.material_types[k],
                    center: center.map(|v| v / count as f32),
                    normal,
                    body_scale: float(mat, rules.body_scale)?,
                    shield_scale: float(mat, rules.shield_scale)?,
                });
            }
        }
    }
    Ok(Target {
        graph: graph.name.clone(),
        poses: poses(map, graph)?,
        rays,
        body: coll.body_vitality,
        shield: coll.shield_vitality,
        shield_type: u16::from_le_bytes(root[0xd2..0xd4].try_into().unwrap()),
    })
}

pub fn damage(map: &H1Map, effect: &Tag, material: u16, rules: &Rules) -> Result<[f32; 2]> {
    let b = h1::tag_bytes(map, effect, b"jpt!", 0x1dc)?;
    damage_bounds(b, material, rules.material_base)
}

fn damage_bounds(b: &[u8], material: u16, material_base: usize) -> Result<[f32; 2]> {
    let scale = float(b, material_base + usize::from(material) * 4)?;
    Ok([float(b, 0x1d4)? * scale, float(b, 0x1d8)? * scale])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_the_executable_operand_and_refuses_another_instruction() {
        assert_eq!(immediate(&[0xf6, 0xc2, 16], &[0xf6, 0xc2], 1).unwrap(), 16);
        assert!(immediate(&[0xf6, 0xc1, 8], &[0xf6, 0xc2], 1).is_err());
        assert!(float(&[0; 3], 0).is_err());
    }
    #[test]
    fn source_damage_uses_the_selected_material_and_preserves_a_zero_multiplier() {
        let mut b = [0_u8; 0x20c];
        for (o, v) in [(0x1d4, 7.0_f32), (0x1d8, 9.), (0x204, 2.5)] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(damage_bounds(&b, 1, 0x200).unwrap(), [17.5, 22.5]);
        assert_eq!(damage_bounds(&b, 0, 0x200).unwrap(), [0., 0.]);
        assert!(damage_bounds(&b, 3, 0x200).is_err());
    }
    #[test]
    fn independent_weapon_reference_follows_the_trigger_block() {
        use h1map::testing::{BASE, Opts, Spec, build_with_extra_tags, put};
        let spec = Spec {
            group: b"weap",
            parent: None,
            name: "test rifle",
            data: Some(vec![0; h1::WEAP_SIZE + h1::TRIGGER_SIZE]),
        };
        let mut cache = build_with_extra_tags(&Opts::default(), vec![spec]);
        let root = cache.data_at.last().copied().flatten().unwrap();
        let trigger = root + h1::WEAP_SIZE;
        // An unset reference is a null id (all ones), not the zeroed field: the
        // rifle has no HUD, and a zero id would name a tag the map lacks.
        put(&mut cache.file, root + 0x480 + 0xc, h1map::TagId::NONE.0);
        put(&mut cache.file, root + 0x4fc, 1);
        put(
            &mut cache.file,
            root + 0x500,
            BASE + (trigger - h1map::HEADER_LEN) as u32,
        );
        for (offset, value) in [
            (4, 3.5_f32),
            (8, 7.),
            (0xc, 0.75),
            (0x48, 0.25),
            (0x7c, 0.125),
            (0x80, 0.5),
        ] {
            put(&mut cache.file, trigger + offset, value.to_bits());
        }
        cache.file[trigger + 0x6e..trigger + 0x70].copy_from_slice(&9_i16.to_le_bytes());
        let map = H1Map::from_decompressed(cache.file).unwrap();
        let gun = gun(
            &map,
            map.find(h1map::Group::new(b"weap"), "test rifle").unwrap(),
        )
        .unwrap();
        let t = &gun.weapon.triggers[0];
        assert_eq!(t.rounds_per_second, [3.5, 7.]);
        assert_eq!(t.firing_acceleration_time, 0.75);
        assert_eq!(t.projectiles_per_shot, 9);
        assert_eq!(t.error_angle, [0.125, 0.5]);
        assert_eq!(gun.charge_seconds, [0.25]);
    }
}
