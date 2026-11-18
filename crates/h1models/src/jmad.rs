//! Halo 1 `antr` -> Halo 2 `jmad` (animation graph).
//!
//! Every animation's data goes into one raw resource. Base animations get a
//! static stream (codec 1, `uncompressed_static_data`) holding the channels
//! Halo 1 keeps in its default data, and an animated stream (codec 3,
//! `8byte_quantized_rotation_only`: i16 quaternions like Halo 1's, float
//! translations and scales) holding the channels Halo 1 animates. Overlays and
//! replacements carry only their animated channels. Names and the
//! mode/class/type/action tree come from [`crate::names`].
//!
//! Layouts are those `crates/tags` decodes (`docs/formats/animation.md`).

use std::collections::BTreeMap;

use map::build::{Block, RawRef};

use crate::h1::{AnimNode, AnimType, Animation, Graph, Transform};
use crate::names::{H2Name, map_name};
use crate::out::{BlockExt, Converted, SELF_ID, array};
use crate::{Error, Result};

const HEADER_LEN: usize = 0xbc;
const NODE_LEN: usize = 0x20;
const ANIMATION_LEN: usize = 0x6c;
const MODE_LEN: usize = 0x14;
const CLASS_LEN: usize = 0x14;
const TYPE_LEN: usize = 0x34;
const ACTION_LEN: usize = 8;
const RESOURCE_LEN: usize = 0x14;
/// Animation flag: data lives in a resource.
const IN_RESOURCE: u16 = 0x10;
const CODEC_STATIC: u8 = 1;
const CODEC_QUANTIZED_ROTATION: u8 = 3;

/// What the conversion did with each animation.
#[derive(Clone, Debug, Default)]
pub struct GraphReport {
    /// `(Halo 1 name, Halo 2 name)` per animation, in order.
    pub names: Vec<(String, String)>,
    /// Animations whose Halo 2 name an earlier one already took (Halo 1
    /// permutations); they get a `:var<n>` action suffix.
    pub variants: Vec<String>,
    /// Animations with root movement (frame info) data, which is dropped.
    pub dropped_movement: Vec<String>,
    /// Non-identity default channels of overlays and replacements, dropped.
    pub dropped_default_channels: usize,
    /// Zero quaternions in the source, written as identity.
    pub zero_rotations: usize,
}

/// Bytes of one per-node bit array (three per flag set).
pub fn flag_array_bytes(nodes: usize) -> usize {
    ((nodes + 31) >> 3) & !3
}

fn q16(q: [f32; 4]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, x) in q.iter().enumerate() {
        let v = (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
    }
    out
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Node channel sets: `[rotation, translation, scale]` bit masks.
type Channels = [u64; 3];

fn ranks(mask: u64, nodes: usize) -> Vec<usize> {
    (0..nodes).filter(|&n| mask >> n & 1 != 0).collect()
}

/// Codec 1: one value per channel.
fn static_stream(pose: &[Transform], ch: Channels) -> Vec<u8> {
    let n = pose.len();
    let [r, t, s] = ch.map(|m| ranks(m, n));
    let trans_at = 0x20 + 8 * r.len();
    let scale_at = trans_at + 12 * t.len();
    let mut b = vec![0u8; scale_at + 4 * s.len()];
    b[0] = CODEC_STATIC;
    b[1..4].copy_from_slice(&[r.len() as u8, t.len() as u8, s.len() as u8]);
    put32(&mut b, 0xc, trans_at as u32);
    put32(&mut b, 0x10, scale_at as u32);
    for (i, &node) in r.iter().enumerate() {
        b[0x20 + 8 * i..0x28 + 8 * i].copy_from_slice(&q16(pose[node].rotation));
    }
    for (i, &node) in t.iter().enumerate() {
        for a in 0..3 {
            put32(
                &mut b,
                trans_at + 12 * i + 4 * a,
                pose[node].translation[a].to_bits(),
            );
        }
    }
    for (i, &node) in s.iter().enumerate() {
        put32(&mut b, scale_at + 4 * i, pose[node].scale.to_bits());
    }
    b
}

/// Codec 3: per channel, every frame; i16 rotations, float translations
/// and scales.
fn animated_stream(frames: &[Vec<Transform>], ch: Channels) -> Vec<u8> {
    let n = frames[0].len();
    let f = frames.len();
    let [r, t, s] = ch.map(|m| ranks(m, n));
    let trans_at = 0x20 + 8 * f * r.len();
    let scale_at = trans_at + 12 * f * t.len();
    let mut b = vec![0u8; scale_at + 4 * f * s.len()];
    b[0] = CODEC_QUANTIZED_ROTATION;
    b[1..4].copy_from_slice(&[r.len() as u8, t.len() as u8, s.len() as u8]);
    put32(&mut b, 0xc, trans_at as u32);
    put32(&mut b, 0x10, scale_at as u32);
    put32(&mut b, 0x14, (8 * f) as u32);
    put32(&mut b, 0x18, (12 * f) as u32);
    put32(&mut b, 0x1c, (4 * f) as u32);
    for (i, &node) in r.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            let at = 0x20 + i * 8 * f + k * 8;
            b[at..at + 8].copy_from_slice(&q16(pose[node].rotation));
        }
    }
    for (i, &node) in t.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            for a in 0..3 {
                let at = trans_at + i * 12 * f + k * 12 + 4 * a;
                put32(&mut b, at, pose[node].translation[a].to_bits());
            }
        }
    }
    for (i, &node) in s.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            put32(
                &mut b,
                scale_at + i * 4 * f + k * 4,
                pose[node].scale.to_bits(),
            );
        }
    }
    b
}

fn flag_set(ch: Channels, nodes: usize) -> Vec<u8> {
    let a = flag_array_bytes(nodes);
    let mut b = vec![0u8; 3 * a];
    for (k, mask) in ch.iter().enumerate() {
        for n in ranks(*mask, nodes) {
            b[k * a + n / 8] |= 1 << (n % 8);
        }
    }
    b
}

/// One animation's blob and its size fields: `(blob, static size,
/// animated size)`.
fn encode(
    anim: &Animation,
    nodes: usize,
    report: &mut GraphReport,
) -> Result<(Vec<u8>, usize, usize)> {
    let all = if nodes >= 64 {
        u64::MAX
    } else {
        (1u64 << nodes) - 1
    };
    let animated: Channels = anim.animated;
    let defaults: Channels = animated.map(|m| !m & all);
    let static_part = if anim.kind == AnimType::Base && defaults.iter().any(|&m| m != 0) {
        static_stream(&anim.frames[0], defaults)
    } else {
        if anim.kind != AnimType::Base {
            let p = &anim.frames[0];
            for n in 0..nodes {
                let t = p[n];
                let rot_id = (t.rotation[3].abs() - 1.0).abs() < 1e-4;
                let trans_id = t.translation.iter().all(|v| v.abs() < 1e-6);
                let scale_id = (t.scale - 1.0).abs() < 1e-6;
                for (c, id) in [rot_id, trans_id, scale_id].into_iter().enumerate() {
                    if defaults[c] >> n & 1 != 0 && !id {
                        report.dropped_default_channels += 1;
                    }
                }
            }
        }
        Vec::new()
    };
    let animated_part = if animated.iter().any(|&m| m != 0) {
        animated_stream(&anim.frames, animated)
    } else {
        Vec::new()
    };
    if static_part.len() >= 0x8000 {
        return Err(Error::Unsupported(format!(
            "{}: static stream too large",
            anim.name
        )));
    }
    let mut blob = Vec::new();
    blob.extend(&static_part);
    blob.extend(&animated_part);
    let static_flags = if static_part.is_empty() {
        [0; 3]
    } else {
        defaults
    };
    blob.extend(flag_set(static_flags, nodes));
    blob.extend(flag_set(animated, nodes));
    Ok((blob, static_part.len(), animated_part.len()))
}

/// Each animation's Halo 2 name, in order, and whether it is a variant: Halo 1
/// repeats a name for random permutations of one animation, and later ones
/// become variants of the first (`:var<n>` on the action). A script that
/// names a Halo 1 animation of this graph names the Halo 2 animation at the
/// same index.
pub fn animation_names(g: &Graph) -> Vec<(H2Name, bool)> {
    let mut taken = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(g.animations.len());
    for anim in &g.animations {
        let mut name: H2Name = map_name(&anim.name);
        let variant = taken.contains(&name.full());
        if variant {
            let action = name.action.clone();
            let mut k = 1;
            while taken.contains(&name.full()) {
                name.action = format!("{action}:var{k}");
                k += 1;
            }
        }
        taken.insert(name.full());
        out.push((name, variant));
    }
    out
}

/// Halo 1 finds a unit's animation through slot tables: per unit, weapon
/// class and type, a list of animation indices. A slot may name another
/// class's animation; the cyborg's pistol, missile and unarmed classes all
/// throw grenades with `stand rifle throw-grenade`. Halo 2 finds animations
/// by name, so each borrowed slot becomes an action of the borrowing class
/// (or type) naming the same animation, unless that path already has the
/// action.
pub(crate) fn borrow_slots(
    g: &Graph,
    names: &[(H2Name, bool)],
    tree: &mut BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>>>,
) {
    let any = |s: &str| if s.is_empty() { "any".to_owned() } else { s.to_owned() };
    for unit in &g.units {
        let mode = crate::names::mode_name(&unit.label);
        for class in &unit.classes {
            // A class the names don't keep never reaches Halo 2's tree.
            if !crate::names::CLASSES.contains(&class.label.as_str()) {
                continue;
            }
            let lists = std::iter::once(("any".to_owned(), &class.slots))
                .chain(class.types.iter().map(|(t, s)| (any(t), s)));
            for (kind, slots) in lists {
                for &slot in slots {
                    let Some((name, _)) = usize::try_from(slot).ok().and_then(|i| names.get(i))
                    else {
                        continue;
                    };
                    let here = (&name.mode, &name.class, &name.kind) == (&mode, &class.label, &kind);
                    if here || name.mode == crate::names::FIRST_PERSON_MODE {
                        continue;
                    }
                    let actions = tree
                        .entry(mode.clone())
                        .or_default()
                        .entry(class.label.clone())
                        .or_default()
                        .entry(kind.clone())
                        .or_default();
                    if !actions.iter().any(|(a, _)| *a == name.action) {
                        actions.push((name.action.clone(), slot as usize));
                    }
                }
            }
        }
    }
}

/// A first-person graph's slot table ([`crate::h1::FIRST_PERSON_SLOTS`]) as
/// actions of mode, class and type `any`: each slot that names an
/// animation becomes the action its slot word maps to (`reload-empty` ->
/// `reload_empty`, `melee` -> `melee_strike_1`), naming that animation,
/// unless an animation labelled so already took the action. Halo 1 plays
/// first-person animations by slot; Halo 2 looks the action up by name
/// (`reload_empty`, `fire_1`, ...), so without this a slot filled by an
/// animation labelled for another (the assault rifle's `reload-empty` slot
/// holds its `reload-full`) has no action, and its animation never plays.
pub(crate) fn first_person_slots(
    g: &Graph,
    tree: &mut BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>>>,
) {
    // Only a weapon's first-person graph (every animation `first-person
    // ...`) is read this way.
    let first_person = |a: &crate::h1::Animation| {
        a.name.split(' ').next() == Some(crate::names::FIRST_PERSON)
    };
    if g.animations.is_empty() || !g.animations.iter().all(first_person) {
        return;
    }
    for (k, &slot) in g.first_person_slots.iter().enumerate() {
        let (Some(word), Ok(index)) = (crate::h1::FIRST_PERSON_SLOTS.get(k), usize::try_from(slot))
        else {
            continue;
        };
        if index >= g.animations.len() {
            continue;
        }
        let name = map_name(&format!("{} {word}", crate::names::FIRST_PERSON));
        let actions = tree
            .entry(name.mode)
            .or_default()
            .entry(name.class)
            .or_default()
            .entry(name.kind)
            .or_default();
        if !actions.iter().any(|(a, _)| *a == name.action) {
            actions.push((name.action, index));
        }
    }
}

/// Converts a Halo 1 animation graph. A graph without its own node block
/// takes `skeleton` (its model's nodes, [`AnimNode::from_model`]).
pub fn convert_graph(g: &Graph, skeleton: Option<&[AnimNode]>) -> Result<(Converted, GraphReport)> {
    let node_list: &[AnimNode] = match (g.nodes.is_empty(), skeleton) {
        (false, _) => &g.nodes,
        (true, Some(s)) => s,
        (true, None) => {
            return Err(Error::Invalid(
                "graph has no nodes of its own; pass its model's skeleton".into(),
            ));
        }
    };
    let nodes = node_list.len();
    if let Some(a) = g
        .animations
        .iter()
        .find(|a| usize::from(a.node_count) != nodes)
    {
        return Err(Error::Invalid(format!(
            "{}: {} nodes, the skeleton has {nodes}",
            a.name, a.node_count
        )));
    }
    if nodes == 0 || nodes > 64 {
        return Err(Error::Unsupported(format!("{nodes} nodes")));
    }
    let a = flag_array_bytes(nodes);
    let mut report = GraphReport::default();
    let mut resource = Vec::new();
    let mut records = Vec::new();
    let mut tree: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>>> =
        BTreeMap::new();
    let names = animation_names(g);
    for (i, anim) in g.animations.iter().enumerate() {
        if anim.frame_count == 0 {
            return Err(Error::Unsupported(format!("{}: no frames", anim.name)));
        }
        let (name, variant) = names[i].clone();
        if variant {
            report.variants.push(anim.name.clone());
        }
        let full = name.full();
        report.names.push((anim.name.clone(), full.clone()));
        report.zero_rotations += anim.zero_rotations;
        if anim.frame_info_type != 0 {
            report.dropped_movement.push(anim.name.clone());
        }
        tree.entry(name.mode)
            .or_default()
            .entry(name.class)
            .or_default()
            .entry(name.kind)
            .or_default()
            .push((name.action, i));

        while resource.len() % 4 != 0 {
            resource.push(0);
        }
        let offset = resource.len();
        let (blob, static_size, animated_size) = encode(anim, nodes, &mut report)?;
        resource.extend(blob);

        let mut r = Block::zeroed(ANIMATION_LEN);
        r.string_id(0, &full)
            .put_u32(4, anim.node_list_checksum)
            .put_u8(
                0x10,
                match anim.kind {
                    AnimType::Base => 0,
                    AnimType::Overlay => 1,
                    AnimType::Replacement => 2,
                },
            )
            // The blend screen (aim screen) index, -1 for none: the
            // engine takes `blend_screens[+0x12]` (`+0x24`, `0x1c` each)
            // unless it is -1 (E `0x001daf9c`, from the unit aim update
            // `0x00112a51`, which reads the screen's `+0xc`). A converted
            // graph has no blend screens, so 0 named a screen past the end
            // of an empty block: the read faulted at address `0xc` as soon
            // as a graph found its animations (converter version 64).
            .put_u8(0x12, 0xff)
            .put_u8(0x13, nodes as u8)
            .put_i16(0x14, anim.frame_count as i16)
            .put_u16(0x16, IN_RESOURCE)
            .put_u32(0x24, 0)
            .put_u32(0x28, offset as u32)
            .put_i16(0x2c, -1)
            .put_i16(0x30, -1)
            .put_i16(0x32, -1)
            .put_u8(0x3c, (3 * a) as u8)
            .put_u8(0x3d, (3 * a) as u8)
            .put_u16(0x42, static_size as u16)
            .put_u32(0x48, animated_size as u32);
        records.push(r);
    }
    borrow_slots(g, &names, &mut tree);
    first_person_slots(g, &mut tree);
    while resource.len() % 0x200 != 0 {
        resource.push(0);
    }

    let node_blocks = node_list
        .iter()
        .map(|n| {
            let mut b = Block::zeroed(NODE_LEN);
            b.string_id(0, &n.name)
                .put_i16(4, n.next_sibling)
                .put_i16(6, n.first_child)
                .put_i16(8, n.parent)
                .put_u32(0xc, n.joint_flags)
                .put_f32s(0x10, &n.base_vector)
                .put_f32(0x1c, n.vector_range);
            b
        })
        .collect::<Vec<_>>();

    // Each level of the tree in ascending string-ID order, as retail: the
    // engine finds a mode, class, type and action by binary search
    // (`0x001dd560`, from the graph lookup `0x001db170`), so a level in
    // name order hides names (no `rifle` class: the cyborg refused the
    // assault rifle). String IDs are known only when the map is built.
    let sorted_array = |e: Vec<Block>, stride: usize| array(e, stride).sorted_by_string_id(stride as u32);
    let modes_any: Vec<String> = tree.keys().cloned().collect();
    let modes = tree
        .into_iter()
        .map(|(mode, classes)| {
            let classes = classes
                .into_iter()
                .map(|(class, types)| {
                    let types = types
                        .into_iter()
                        .map(|(kind, actions)| {
                            let actions = actions
                                .into_iter()
                                .map(|(action, index)| {
                                    let mut b = Block::zeroed(ACTION_LEN);
                                    b.string_id(0, &action)
                                        .put_i16(4, -1)
                                        .put_i16(6, index as i16);
                                    b
                                })
                                .collect::<Vec<_>>();
                            let mut b = Block::zeroed(TYPE_LEN);
                            b.string_id(0, &kind);
                            b.block(4, actions.len() as u32, sorted_array(actions, ACTION_LEN));
                            b
                        })
                        .collect::<Vec<_>>();
                    let mut b = Block::zeroed(CLASS_LEN);
                    b.string_id(0, &class);
                    b.block(4, types.len() as u32, sorted_array(types, TYPE_LEN));
                    b
                })
                .collect::<Vec<_>>();
            let mut b = Block::zeroed(MODE_LEN);
            b.string_id(0, &mode);
            b.block(4, classes.len() as u32, sorted_array(classes, CLASS_LEN));
            b
        })
        .collect::<Vec<_>>();

    let mut res = Block::zeroed(RESOURCE_LEN);
    res.tag_id(0, SELF_ID)
        .put_u32(4, resource.len() as u32)
        .raw(8, RawRef::Local(map::build::RawId(0)))
        .put_u32(0xc, u32::MAX)
        .put_u32(0x10, 0);

    let mut h = Block::zeroed(HEADER_LEN);
    h.tag_ref(0, map::TagId::NULL);
    h.block(0x0c, node_blocks.len() as u32, array(node_blocks, NODE_LEN));
    h.block(0x2c, records.len() as u32, array(records, ANIMATION_LEN));
    // A first-person graph (every tree mode `any`): one string-ID pair at
    // `+0x54`, `(any, any)`, as every retail first-person graph holds (V3:
    // tests/weapon_rebuild_disc.rs in h1objects). Other graphs leave it
    // empty, as before (its meaning in unit graphs is not established).
    let first_person = !modes_any.is_empty() && modes_any.iter().all(|m| m == crate::names::FIRST_PERSON_MODE);
    h.block(0x34, modes.len() as u32, sorted_array(modes, MODE_LEN));
    if first_person {
        let mut p = Block::zeroed(8);
        p.string_id(0, crate::names::FIRST_PERSON_MODE)
            .string_id(4, crate::names::FIRST_PERSON_MODE);
        h.block(0x54, 1, p);
    }
    h.block(0xac, 1, res);
    Ok((
        Converted {
            group: map::FourCC(*b"jmad"),
            root: h,
            raw: vec![(resource, 0x200)],
        },
        report,
    ))
}
