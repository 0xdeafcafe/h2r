//! The cyborg's animation graph. Halo 1 selects an animation by slot even
//! when its label names another stance, weapon, direction or permutation.
//! Halo 2 searches actions, overlays and damage tables independently.

use super::*;
use crate::h1::AimScreen;

type Markers = Vec<(String, String)>;

#[derive(Default)]
pub(super) struct PlayerData {
    damage: Vec<Block>,
    ik: BTreeMap<(String, String), Markers>,
}

// The populated Blood Gulch unit, class and type slots. Positions come
// from the Halo 1 XBE's three label tables. The strings are the keys used
// by the retail H2 Chief. walk and move share H1's one directional cycle.
const UNIT: &[(usize, &[&str])] = &[
    (0, &["airborne_dead"]),
    (1, &["landing_dead"]),
    (2, &["acc_front_back"]),
    (3, &["acc_left_right"]),
    (4, &["acc_up_down"]),
    (5, &["push"]),
    (7, &["enter"]),
    (8, &["exit"]),
    (9, &["look"]),
];
const CLASS: &[(usize, &[&str])] = &[
    (0, &["idle"]),
    (2, &["turn_left"]),
    (3, &["turn_right"]),
    (8, &["move_front", "walk_front"]),
    (9, &["move_back", "walk_back"]),
    (10, &["move_left", "walk_left"]),
    (11, &["move_right", "walk_right"]),
    (16, &["airborne"]),
    (17, &["land_soft"]),
    (18, &["land_hard"]),
    (20, &["throw_grenade"]),
    (23, &["ready"]),
    (25, &["aim_still_up", "aim_still_down"]),
    (26, &["aim_move_up", "aim_move_down"]),
    (35, &["stunned_front"]),
    (36, &["stunned_back"]),
    (37, &["stunned_left"]),
    (38, &["stunned_right"]),
];
const WEAPON: &[(usize, &[&str])] = &[
    (0, &["reload_1"]),
    (1, &["reload_2"]),
    (2, &["chamber_1"]),
    (3, &["chamber_2"]),
    (4, &["fire_1"]),
    (5, &["fire_2"]),
    (6, &["charged_1"]),
    (7, &["charged_2"]),
    (8, &["melee_strike_1", "melee"]),
    (9, &["overheat"]),
];

fn index(g: &Graph, value: i16) -> Result<Option<usize>> {
    match value {
        -1 => Ok(None),
        i if i >= 0 && (i as usize) < g.animations.len() => Ok(Some(i as usize)),
        _ => Err(Error::Invalid(format!(
            "cyborg animation index {value} outside graph"
        ))),
    }
}

struct Bindings<'a> {
    graph: &'a Graph,
    tree: &'a mut Actions,
    overlays: &'a mut Overlays,
    plan: &'a mut VehiclePlan,
}

impl Bindings<'_> {
    fn slots(
        &mut self,
        path: [&str; 3],
        slots: &[i16],
        keys: &[(usize, &[&str])],
        screen: Option<&AimScreen>,
    ) -> Result<()> {
        let [mode, class, kind] = path;
        let actions = self
            .tree
            .entry(mode.into())
            .or_default()
            .entry(class.into())
            .or_default()
            .entry(kind.into())
            .or_default();
        for (k, &value) in slots.iter().enumerate() {
            let Some(i) = index(self.graph, value)? else {
                continue;
            };
            let Some((_, names)) = keys.iter().find(|(slot, _)| *slot == k) else {
                return Err(Error::Unsupported(format!(
                    "cyborg {path:?} populated slot {k}"
                )));
            };
            if matches!(names[0], "look" | "aim_still_up" | "aim_move_up") {
                let s = screen
                    .ok_or_else(|| Error::Invalid(format!("cyborg {path:?} missing screen")))?;
                let count = |a: i16, b: i16| (1 + i32::from(a) + i32::from(b)) as i64;
                let cells =
                    count(s.left_frames, s.right_frames) * count(s.up_frames, s.down_frames);
                // The warthog gunner has 16 frames for a 5-by-3 grid.
                // Both engines address cells by the screen dimensions.
                if !s.is_set()
                    || [s.left_frames, s.right_frames, s.up_frames, s.down_frames]
                        .iter()
                        .any(|&n| n < 0)
                    || cells > i64::from(self.graph.animations[i].frame_count)
                {
                    return Err(Error::Invalid(format!(
                        "cyborg {path:?} animation {i}: {cells} screen cells"
                    )));
                }
                let screen = self.plan.screen(
                    if names[0] == "look" {
                        "looking"
                    } else {
                        "aiming"
                    },
                    s.bytes(),
                );
                if self
                    .plan
                    .screen_of
                    .insert(i, screen)
                    .is_some_and(|old| old != screen)
                {
                    return Err(Error::Invalid(format!(
                        "cyborg animation {i} has conflicting screens"
                    )));
                }
            }
            // 0x113e90 tries the overlay, then a whole-body action. The
            // cyborg's grenade throw is a base clip; reload and melee are
            // replacements, and aim and fire are additive overlays.
            let target = if self.graph.animations[i].kind == AnimType::Base {
                &mut *actions
            } else {
                self.overlays
                    .entry((mode.into(), class.into(), kind.into()))
                    .or_default()
            };
            // The turret gunner repeats the unarmed class with the same
            // borrowed slots. Emit one key for each distinct binding.
            for &name in *names {
                if let Some((_, previous)) = target.iter().find(|(key, _)| key == name) {
                    if *previous != i {
                        return Err(Error::Invalid(format!(
                            "cyborg {path:?}: conflicting {name} slots"
                        )));
                    }
                } else {
                    target.push((name.into(), i));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn bindings(
    g: &Graph,
    tree: &mut Actions,
    overlays: &mut Overlays,
    plan: &mut VehiclePlan,
) -> Result<PlayerData> {
    let mut data = PlayerData::default();
    let mut bind = Bindings {
        graph: g,
        tree,
        overlays,
        plan,
    };
    for unit in &g.units {
        // Coagulation's Chief uses patrol as a locomotion override: its
        // sole type holds move_front and the moving aim overlay.
        if unit.label == "alert" {
            if let Some(c) = unit.classes.iter().find(|c| c.label == "rifle") {
                let path = ["patrol", "rifle", "any"];
                for (k, names) in [(8, &["move_front"][..]), (26, &["aim_move_up"][..])] {
                    if let Some(&slot) = c.slots.get(k) {
                        bind.slots(path, &[slot], &[(0, names)], Some(&c.screen))?;
                    }
                }
            }
            continue;
        }
        let mode_name = crate::names::mode_name(&unit.label);
        let mode = mode_name.as_str();
        bind.slots([mode, "any", "any"], &unit.slots, UNIT, Some(&unit.screen))?;
        for class in &unit.classes {
            let c = class.label.as_str();
            bind.slots([mode, c, "any"], &class.slots, CLASS, Some(&class.screen))?;
            if !class.ik.is_empty() {
                data.ik
                    .entry((mode.into(), c.into()))
                    .or_default()
                    .extend(class.ik.iter().cloned());
            }
            for (ty, slots) in &class.types {
                if ty.is_empty() && slots.is_empty() {
                    continue;
                }
                let ty = if ty.is_empty() { "any" } else { ty };
                bind.slots([mode, c, ty], slots, WEAPON, None)?;
            }
        }
    }
    plan.weapon_pairs = crate::h1::player_weapon_pairs(&g.units)?;
    plan.weapon_pairs.push(("any".into(), "any".into()));
    plan.weapon_pairs.sort();
    plan.weapon_pairs.dedup();
    data.damage = damage(g)?;
    if !data.damage.is_empty() {
        tree.entry("any".into())
            .or_default()
            .entry("any".into())
            .or_default()
            .entry("any".into())
            .or_default();
    }
    Ok(data)
}

// H1 indexes 4 damage kinds × 4 directions × 11 regions. H2 searches
// the damage kind by SID at type +0x14, then indexes direction and region.
fn damage(g: &Graph) -> Result<Vec<Block>> {
    const NAMES: [&str; 4] = ["s_ping", "h_ping", "s_kill", "h_kill"];
    if g.damage_slots.len() > 4 * 4 * 11 {
        return Err(Error::Invalid(format!(
            "cyborg has {} damage slots",
            g.damage_slots.len()
        )));
    }
    let mut out = Vec::new();
    for (name, slots) in NAMES.into_iter().zip(g.damage_slots.chunks(44)) {
        if slots.iter().all(|&s| s == -1) {
            continue;
        }
        let mut directions = Vec::new();
        for direction in 0..4 {
            let mut regions = Vec::new();
            for region in 0..11 {
                let slot = slots.get(11 * direction + region).copied().unwrap_or(-1);
                let mut r = Block::zeroed(4);
                // Retail encodes an unused region as (0, -1).
                if index(g, slot)?.is_some() {
                    r.put_i16(0, -1);
                }
                r.put_i16(2, slot);
                regions.push(r);
            }
            let mut d = Block::zeroed(8);
            d.block(0, 11, array(regions, 4));
            directions.push(d);
        }
        let mut d = Block::zeroed(12);
        d.string_id(0, name).block(4, 4, array(directions, 8));
        out.push(d);
    }
    Ok(out)
}

impl PlayerData {
    pub(super) fn write_damage(&mut self, ty: &mut Block) {
        if !self.damage.is_empty() {
            ty.block(
                0x14,
                self.damage.len() as u32,
                array(std::mem::take(&mut self.damage), 12).sorted_by_string_id(12),
            );
        }
    }

    pub(super) fn write_ik(&self, mode: &str, class: &str, out: &mut Block) {
        if let Some(pairs) = self.ik.get(&(mode.into(), class.into())) {
            let markers: Vec<_> = pairs
                .iter()
                .map(|(hand, grip)| {
                    let mut m = Block::zeroed(8);
                    m.string_id(0, hand).string_id(4, grip);
                    m
                })
                .collect();
            if !markers.is_empty() {
                out.block(0xc, markers.len() as u32, array(markers, 8));
            }
        }
    }
}

// The retail Chief's model nodes all carry bit 0, the pelvis bit 2,
// left and right hands bits 3/4, and the left arm chain bit 5. The two
// graph masks partition those nodes (0x1c7013..0x1c703a selects one for
// channel sampling). Construct both in the cyborg's own node order.
pub(super) fn node_flags(node: &AnimNode) -> u16 {
    1 | match node.name.as_str() {
        "bip01 pelvis" => 4,
        "bip01 l hand" => 8 | 32,
        "bip01 r hand" => 16,
        "bip01 l clavicle" | "bip01 l upperarm" | "bip01 l forearm" => 32,
        _ => 0,
    }
}

pub(super) fn write_masks(nodes: &[AnimNode], h: &mut Block) {
    for (i, node) in nodes.iter().enumerate() {
        let start = if node_flags(node) & 32 != 0 {
            0x5c
        } else {
            0x7c
        };
        h.bytes[start + i / 8] |= 1 << (i % 8);
    }
}

/// H1 0xa318e..0xa31aa loops to the lesser of the authored positive
/// start and the last frame. H2 0x1c6810 consumes the start directly.
pub(super) fn playback(anim: &Animation, out: &mut Block) -> Result<()> {
    let start = (anim.loop_frame as i16).max(0) as u16;
    out.put_u16(0x2e, start.min(anim.frame_count - 1));
    let mut events = Vec::new();
    let mut add = |kind: i16, frame: i16| {
        let mut event = Block::zeroed(4);
        event.put_i16(0, kind).put_i16(2, frame);
        events.push(event);
    };
    // The primary and secondary keys are event types 0 and 1.
    // Both games' third-person grenade release is frame 8.
    for (kind, frame) in anim.key_frames.into_iter().enumerate() {
        if frame <= 0 {
            continue;
        }
        if frame as u16 >= anim.frame_count {
            return Err(Error::Invalid(format!(
                "{}: key frame {frame} outside clip",
                anim.name
            )));
        }
        add(kind as i16, frame);
    }
    // Coagulation Chief's left/right turn events are (2,7)/(3,14)
    // and (2,14)/(3,7), the Blood Gulch cyborg's two contact bytes.
    // Zero can be a contact when paired with a later frame (move_front).
    if anim.foot_frames != [0, 0] {
        for (side, frame) in anim.foot_frames.into_iter().enumerate() {
            add(2 + side as i16, i16::from(frame));
        }
    }
    if !events.is_empty() {
        out.block(0x4c, events.len() as u32, array(events, 4));
    }
    Ok(())
}

/// 0x1dd6b9 compares the draw with the cumulative weight. +0x32
/// advances the chain and +0x30 names the head on its later members.
pub(super) fn permutation(g: &Graph, i: usize, out: &mut Block) -> Result<()> {
    let a = &g.animations[i];
    let first = index(g, a.first_permutation)?;
    let next = index(g, a.next_permutation)?;
    if next.is_some_and(|n| n <= i) || !a.cumulative_weight.is_finite() {
        return Err(Error::Invalid(format!(
            "{}: invalid cyborg permutation chain",
            a.name
        )));
    }
    if first.is_some_and(|first| first != i) {
        out.put_i16(0x30, a.first_permutation);
    }
    out.put_i16(0x32, a.next_permutation)
        .put_f32(0x1c, a.cumulative_weight);
    Ok(())
}
