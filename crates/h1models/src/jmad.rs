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
    /// Animations with per-frame root movement in the output resource.
    pub movement: Vec<String>,
    /// Non-identity default channels of overlays and replacements, dropped.
    pub dropped_default_channels: usize,
    /// Zero quaternions in the source, written as identity.
    pub zero_rotations: usize,
    /// Suspension elements left out: their mass point has no marker on
    /// the converted model ([`GraphOptions::mass_point_markers`]).
    pub suspension_without_marker: Vec<String>,
}

/// What converting a graph needs from the object it animates.
#[derive(Clone, Debug, Default)]
pub struct GraphOptions {
    /// Per mass point of the object's Halo 1 physics (`phys`), the marker
    /// the converted model holds for it, `None` for none. A vehicle
    /// graph's suspension names its mass points by index; Halo 2's names a
    /// marker ([`VEHICLE_SUSPENSION_LEN`]).
    pub mass_point_markers: Vec<Option<String>>,
}

/// A vehicle graph's suspension element (graph `+0x3c`, `0x28` each). E:
/// `0x001dceb0` steps through the block and copies, per element, the
/// animation (`+4`, graph index and animation index as an action's), the
/// marker name (`+8`), a mass point offset (`+0xc`), the full extension
/// (`+0x10`) and full compression (`+0x14`) ground depths, a region name
/// (`+0x18`) and the same three numbers for when that region is
/// destroyed (`+0x1c`..`+0x24`). The vehicle's suspension update
/// (`0x000f6670`) finds the marker (`0x000b8d30`; no marker, no
/// suspension), casts from it (moved by the offset along the object's
/// up) down along the up by the full extension depth, and turns the hit
/// distance `d` into `1 - (d - compression) / (extension - compression)`,
/// clamped to 0..1; the animation update (`0x000f0760`) plays the
/// element's overlay at that fraction. So both depths are distances
/// below the marker, positive. V: the retail warthog's four name its
/// `left_back_tire` .. markers with 0.308 and 0.132, a zero offset, and
/// the same three numbers again for the destroyed wheel. Halo 1's depths
/// are negative ([`crate::h1::Suspension`]) and measured from the tire
/// mass points, where the converted model's markers sit, so they are
/// written negated.
pub const VEHICLE_SUSPENSION_LEN: usize = 0x28;
/// An object overlay (graph `+0x44`, `0x14` each): label string ID,
/// animation (graph index, animation index), `+8` function controls,
/// `+0xc` the function's name, `+0x10` zero. V: the two on Coagulation,
/// the chaingun's and the spectre turret's `firing`, name `barrel_spin`
/// with controls 0.
pub const OBJECT_OVERLAY_LEN: usize = 0x14;
/// The function a converted object overlay follows. E: the unit's import
/// names (`0x000c6b90`) answer `barrel_spin` (`0x000c6d41`) from the
/// unit's current weapon (`+0x212`, `+0x218`, through the weapon's own at
/// `0x000ff7b0`), so a vehicle's barrels spin with its gun. V: both
/// retail object overlays name it. S: Halo 1 drives the warthog's
/// `firing` by its D output, an additive (accumulating) function whose
/// input the vehicle leaves none; additivity is not converted
/// (`h1objects::functions`), so that output could not turn the barrels.
pub const BARREL_SPIN: &str = "barrel_spin";
const BLEND_SCREEN_LEN: usize = 0x1c;
/// Alignment of every animation's blob in the resource, and of a
/// blend-screen stream after a static one. E: codec 8's interpolated
/// rotation function (`0x0028c530`, the codec table's record 8 at
/// `0x47fc54`, slot `+0x1c`) loads each frame's four floats at
/// `+0x20 + i * [+0x14] + frame * 16` of the stream with `movaps` (`0x0028c589`),
/// which faults (#GP) on an address that is not a multiple of 16. Through
/// converter version 88 blobs were 4-aligned; from version 84, when the
/// vehicles' steering and aim overlays took codec 8, Blood Gulch stopped
/// there about 5 s into a game (`tags::anim::AnimationGraph::engine_problems`
/// checks it).
const STREAM_ALIGN: usize = 16;
/// Codec of an animation with a blend screen: float rotations (V: every
/// retail animation that names a screen, the vehicles' steering and aim
/// overlays included, is codec 8).
const CODEC_BLEND_SCREEN: u8 = 8;

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
/// and scales. Codec 8 (`blend_screen`, when `screen`) is the same with
/// float rotations, 16 bytes a frame (E: its rotation function
/// `0x0028c710` reads four floats at `+0x20 + i * [+0x14] + frame * 16`;
/// translation and scale are codec 3's functions).
fn animated_stream(frames: &[Vec<Transform>], ch: Channels, screen: bool) -> Vec<u8> {
    let n = frames[0].len();
    let f = frames.len();
    let [r, t, s] = ch.map(|m| ranks(m, n));
    let rot = if screen { 16 } else { 8 };
    let trans_at = 0x20 + rot * f * r.len();
    let scale_at = trans_at + 12 * f * t.len();
    let mut b = vec![0u8; scale_at + 4 * f * s.len()];
    b[0] = if screen {
        CODEC_BLEND_SCREEN
    } else {
        CODEC_QUANTIZED_ROTATION
    };
    b[1..4].copy_from_slice(&[r.len() as u8, t.len() as u8, s.len() as u8]);
    put32(&mut b, 0xc, trans_at as u32);
    put32(&mut b, 0x10, scale_at as u32);
    put32(&mut b, 0x14, (rot * f) as u32);
    put32(&mut b, 0x18, (12 * f) as u32);
    put32(&mut b, 0x1c, (4 * f) as u32);
    for (i, &node) in r.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            let at = 0x20 + i * rot * f + k * rot;
            if screen {
                for (a, x) in pose[node].rotation.iter().enumerate() {
                    put32(&mut b, at + 4 * a, x.to_bits());
                }
            } else {
                b[at..at + 8].copy_from_slice(&q16(pose[node].rotation));
            }
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
    screen: bool,
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
        animated_stream(&anim.frames, animated, screen)
    } else {
        Vec::new()
    };
    // A blend-screen stream must start 16-byte aligned in the resource
    // ([`STREAM_ALIGN`]); every blob does, so pad the static stream
    // before one. Its size field covers the padding, which no codec 1
    // read reaches (its offsets are in its header).
    let mut static_part = static_part;
    if screen && !static_part.is_empty() {
        static_part.resize(static_part.len().next_multiple_of(STREAM_ALIGN), 0);
    }
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
        if is_vehicle(g) {
            vehicle_aim_name(&mut name);
        }
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
///
/// A graph whose `fire-1` slot is empty but which has an animation
/// labelled `firing` ([`FIRST_PERSON_FIRING`]) also gets `fire_1` naming
/// it ([`first_person_fire`]).
pub(crate) fn first_person_slots(
    g: &Graph,
    tree: &mut BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>>>,
) {
    if !is_first_person(g) {
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
    first_person_fire(g, tree);
}

/// Only a weapon's first-person graph (every animation `first-person ...`)
/// is read as one.
pub(crate) fn is_first_person(g: &Graph) -> bool {
    !g.animations.is_empty()
        && g
            .animations
            .iter()
            .all(|a| a.name.split(' ').next() == Some(crate::names::FIRST_PERSON))
}

/// The label of the assault rifle's firing animation, which no slot holds.
pub const FIRST_PERSON_FIRING: &str = "first-person firing";

/// `fire_1` for a first-person graph that has none, naming its
/// [`FIRST_PERSON_FIRING`] animation. V (every first-person graph on the
/// disc's maps, Blood Gulch, a10, c10, d40): the assault rifle's is the
/// only one whose `fire-1` slot (2) is empty while it has a firing
/// animation, `first-person firing`, a 21-frame base animation no slot
/// holds; the flamethrower, ball and flag have neither. Halo 1 plays
/// first-person animations by slot, so it plays none for the assault
/// rifle's shots (I: no slot names it, and nothing else in the graph
/// does). Halo 2 gives every weapon that fires a `fire_1` (V: 13 of the
/// Chief's 17 retail first-person graphs on Coagulation have it; the four
/// without are the flag, ball, bomb and energy sword, which do not fire),
/// and without one a converted assault rifle fired with no recoil
/// animation.
pub(crate) fn first_person_fire(
    g: &Graph,
    tree: &mut BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>>>,
) {
    let slot = crate::h1::FIRST_PERSON_SLOTS[2];
    let fire = map_name(&format!("{} {slot}", crate::names::FIRST_PERSON));
    let Some(index) = g.animations.iter().position(|a| a.name == FIRST_PERSON_FIRING) else {
        return;
    };
    let actions = tree
        .entry(fire.mode)
        .or_default()
        .entry(fire.class)
        .or_default()
        .entry(fire.kind)
        .or_default();
    if !actions.iter().any(|(a, _)| *a == fire.action) {
        actions.push((fire.action, index));
    }
}

/// `(mode, class, type)` -> that type's overlay animations, `(name,
/// animation index)`.
pub(crate) type Overlays = BTreeMap<(String, String, String), Vec<(String, usize)>>;

/// A first-person graph's overlay animations (Halo 1 type 1: `moving`,
/// `overlays`, the needler's `ammunition`, the plasma pistol's
/// `overcharged-jitter`), taken out of the actions into the overlays of
/// their type (the type's `+0xc` block). The engine finds a first-person
/// overlay by name in that block, not among the actions: `0x001dbc80`
/// searches the type's `+0xc` (stride 8: string ID, graph index, animation
/// index; `0x001dbd01..0x001dbd12`) through `0x001dd560`, and the
/// first-person setup asks it for `overcharged_jitter`, `overlays`,
/// `ammunition` and `pitch_and_turn` (`0x0016821f..0x00168282`, through
/// `0x001cb520`/`0x001cb9d0`) and for `moving` (`0x001672d6`). V: every
/// retail first-person graph on Coagulation keeps `moving`, `overlays`,
/// `pitch_and_turn` (and the needler's `ammunition`, sprint's `sprint`) in
/// that block and none of them among its actions. Until converter version
/// 79 they were actions, so no Halo 1 weapon swayed as it moved. Halo 1's
/// `overlays` keeps its name; Halo 2 also asks for `pitch_and_turn`, which
/// Halo 1 has no animation for (S: Halo 1's 9-frame `overlays` is the one
/// the two have in common by name and frame count; which of Halo 2's two it
/// corresponds to is not established).
pub(crate) fn first_person_overlays(
    g: &Graph,
    tree: &mut BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<(String, usize)>>>>,
) -> Overlays {
    let mut out = Overlays::new();
    if !is_first_person(g) && !is_vehicle(g) {
        return out;
    }
    let overlay = |i: usize| g.animations.get(i).is_some_and(|a| a.kind == AnimType::Overlay);
    for (mode, classes) in tree.iter_mut() {
        for (class, types) in classes.iter_mut() {
            for (kind, actions) in types.iter_mut() {
                let (overlays, rest): (Vec<_>, Vec<_>) =
                    actions.drain(..).partition(|&(_, i)| overlay(i));
                *actions = rest;
                if !overlays.is_empty() {
                    out.insert((mode.clone(), class.clone(), kind.clone()), overlays);
                }
            }
        }
    }
    out
}

/// A vehicle graph: one with Halo 1's vehicles block (the warthog, ghost
/// and scorpion on Blood Gulch).
pub(crate) fn is_vehicle(g: &Graph) -> bool {
    g.vehicle.is_some()
}

/// Halo 2's aim overlays are `aim_still_up` and `aim_move_up` (E: the unit
/// aim update asks for them, `0x001134a3`; V: every retail vehicle and
/// turret graph names its aim overlay `combat:fixed:aim_still_up`, the
/// Halo 1 warthog's, ghost's and scorpion's are `stand fixed aim-still`).
/// Only vehicle graphs are renamed: a biped's aim is not converted here.
fn vehicle_aim_name(name: &mut H2Name) {
    let (head, var) = match name.action.split_once(':') {
        Some((h, v)) => (h.to_owned(), Some(v.to_owned())),
        None => (name.action.clone(), None),
    };
    let head = match head.as_str() {
        "aim_still" => "aim_still_up",
        "aim_move" => "aim_move_up",
        _ => return,
    };
    name.action = match var {
        Some(v) => format!("{head}:{v}"),
        None => head.to_owned(),
    };
}

/// What a vehicle graph lays out beside its name tree ([`vehicle_plan`]).
#[derive(Default)]
struct VehiclePlan {
    /// Animations kept out of the name tree: those the vehicle slots, the
    /// suspension and the object overlays name (V: no retail vehicle tree
    /// holds a `vehicle`, `suspension`, `tread` or `firing` mode).
    outside: std::collections::HashSet<usize>,
    /// The vehicle slots as overlays of mode, class and type `any` (V: the
    /// retail warthog's `steering`, `ground_speed`, `ground_speed_rear`,
    /// the ghost's `steering` are there).
    slot_overlays: Vec<(String, usize)>,
    /// Blend screens, `(label, the six fields)`, and each animation's.
    screens: Vec<(String, [u8; 0x18])>,
    screen_of: BTreeMap<usize, u8>,
    suspension: Vec<Block>,
    object_overlays: Vec<Block>,
    /// `(type, class)` pairs for graph `+0x54`.
    weapon_pairs: Vec<(String, String)>,
}

impl VehiclePlan {
    fn screen(&mut self, label: &str, bytes: [u8; 0x18]) -> u8 {
        if let Some(i) = self.screens.iter().position(|(_, b)| *b == bytes) {
            return i as u8;
        }
        self.screens.push((label.to_owned(), bytes));
        (self.screens.len() - 1) as u8
    }
}

/// Halo 1's vehicle tables as Halo 2 lays them out: the vehicle slots as
/// overlays of `any` (the engine asks for them by name through
/// `0x001cb9d0`, from `0x000f0760`), the steering screen on the steering
/// overlay and each weapon class's aiming screen on its aim overlays (an
/// animation's `+0x12`, a blend screen at graph `+0x24`), the suspension
/// ([`VEHICLE_SUSPENSION_LEN`]), the object overlays
/// ([`OBJECT_OVERLAY_LEN`]) and the weapon pairs. Halo 1 finds all of
/// these by slot. Until converter version 84 they were actions named for
/// their labels (`vehicle:steering`, `suspension:left_back`,
/// `combat:fixed:aim_still`) that no engine lookup asks for, so no
/// converted vehicle steered its wheels, sprang, aimed or spun its
/// barrels.
fn vehicle_plan(
    g: &Graph,
    names: &[(H2Name, bool)],
    opts: &GraphOptions,
    report: &mut GraphReport,
) -> VehiclePlan {
    let mut plan = VehiclePlan::default();
    let Some(v) = &g.vehicle else {
        return plan;
    };
    let valid = |i: i16| usize::try_from(i).ok().filter(|&i| i < g.animations.len());
    for (k, &slot) in v.slots.iter().enumerate() {
        let (Some(word), Some(i)) = (crate::h1::VEHICLE_SLOTS.get(k), valid(slot)) else {
            continue;
        };
        plan.outside.insert(i);
        if !plan.slot_overlays.iter().any(|(w, _)| w == word) {
            plan.slot_overlays.push(((*word).to_owned(), i));
        }
        if k == 0 && v.screen.is_set() {
            let s = plan.screen("steering", v.screen.bytes());
            plan.screen_of.insert(i, s);
        }
    }
    for u in &g.units {
        let mode = crate::names::mode_name(&u.label);
        for c in &u.classes {
            plan.weapon_pairs
                .extend(c.types.iter().map(|(t, _)| (t.clone(), c.label.clone())));
            if !c.screen.is_set() {
                continue;
            }
            for (i, (n, _)) in names.iter().enumerate() {
                let head = n.action.split(':').next().unwrap_or("");
                if n.mode == mode
                    && n.class == c.label
                    && matches!(head, "aim_still_up" | "aim_move_up")
                    && !plan.screen_of.contains_key(&i)
                {
                    let s = plan.screen("aiming", c.screen.bytes());
                    plan.screen_of.insert(i, s);
                }
            }
        }
    }
    plan.weapon_pairs.sort();
    plan.weapon_pairs.dedup();
    plan.weapon_pairs
        .insert(0, ("any".to_owned(), "any".to_owned()));
    for s in &v.suspension {
        let Some(i) = valid(s.animation) else {
            continue;
        };
        plan.outside.insert(i);
        let marker = usize::try_from(s.mass_point)
            .ok()
            .and_then(|m| opts.mass_point_markers.get(m).cloned().flatten());
        let Some(marker) = marker else {
            report
                .suspension_without_marker
                .push(g.animations[i].name.clone());
            continue;
        };
        let ext = -s.full_extension_ground_depth;
        let comp = -s.full_compression_ground_depth;
        let mut b = Block::zeroed(VEHICLE_SUSPENSION_LEN);
        b.string_id(0, &names[i].0.full())
            .put_i16(4, -1)
            .put_i16(6, i as i16)
            .string_id(8, &marker)
            .put_f32(0xc, 0.0)
            .put_f32(0x10, ext)
            .put_f32(0x14, comp)
            // No region; the destroyed numbers are the same, as retail's.
            .put_f32(0x1c, 0.0)
            .put_f32(0x20, ext)
            .put_f32(0x24, comp);
        plan.suspension.push(b);
    }
    for o in &g.objects {
        let Some(i) = valid(o.animation) else {
            continue;
        };
        plan.outside.insert(i);
        let label = g.animations[i]
            .name
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_");
        let mut b = Block::zeroed(OBJECT_OVERLAY_LEN);
        b.string_id(0, &label)
            .put_i16(4, -1)
            .put_i16(6, i as i16)
            .put_u32(8, u32::from(o.controls))
            .string_id(0xc, BARREL_SPIN);
        plan.object_overlays.push(b);
    }
    plan
}

/// An action or overlay entry (`ACTION_LEN`): its name, graph index -1
/// (this graph), the animation index.
fn action_block((action, index): &(String, usize)) -> Block {
    let mut b = Block::zeroed(ACTION_LEN);
    b.string_id(0, action).put_i16(4, -1).put_i16(6, *index as i16);
    b
}

/// Converts a Halo 1 animation graph. A graph without its own node block
/// takes `skeleton` (its model's nodes, [`AnimNode::from_model`]).
pub fn convert_graph(g: &Graph, skeleton: Option<&[AnimNode]>) -> Result<(Converted, GraphReport)> {
    convert_graph_with(g, skeleton, &GraphOptions::default())
}

/// [`convert_graph`] with what the animated object adds ([`GraphOptions`]).
pub fn convert_graph_with(
    g: &Graph,
    skeleton: Option<&[AnimNode]>,
    opts: &GraphOptions,
) -> Result<(Converted, GraphReport)> {
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
    let plan = vehicle_plan(g, &names, opts, &mut report);
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
            report.movement.push(anim.name.clone());
        }
        if !plan.outside.contains(&i) {
            tree.entry(name.mode)
                .or_default()
                .entry(name.class)
                .or_default()
                .entry(name.kind)
                .or_default()
                .push((name.action, i));
        }

        resource.resize(resource.len().next_multiple_of(STREAM_ALIGN), 0);
        let offset = resource.len();
        let screen = plan.screen_of.get(&i).copied();
        let (blob, static_size, animated_size) =
            encode(anim, nodes, screen.is_some(), &mut report)?;
        resource.extend(blob);
        let movement_size = i16::try_from(anim.frame_info.len())
            .map_err(|_| Error::Unsupported(format!("{}: root movement exceeds i16", anim.name)))?;
        resource.extend_from_slice(&anim.frame_info);

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
            .put_u8(0x11, anim.frame_info_type as u8)
            // The blend screen (aim screen) index, -1 for none: the
            // engine takes `blend_screens[+0x12]` (`+0x24`, `0x1c` each)
            // unless it is -1 (E `0x001daf9c`, from the unit aim update
            // `0x00112a51`, which reads the screen's `+0xc`). A converted
            // graph has no blend screens, so 0 named a screen past the end
            // of an empty block: the read faulted at address `0xc` as soon
            // as a graph found its animations (converter version 64).
            // A vehicle's steering and aim overlays name theirs
            // ([`vehicle_plan`]).
            .put_u8(0x12, screen.unwrap_or(0xff))
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
            .put_i16(0x3e, movement_size)
            .put_u16(0x42, static_size as u16)
            .put_u32(0x48, animated_size as u32);
        records.push(r);
    }
    borrow_slots(g, &names, &mut tree);
    first_person_slots(g, &mut tree);
    let mut overlays = first_person_overlays(g, &mut tree);
    if !plan.slot_overlays.is_empty() {
        let any = || "any".to_owned();
        tree.entry(any())
            .or_default()
            .entry(any())
            .or_default()
            .entry(any())
            .or_default();
        overlays
            .entry((any(), any(), any()))
            .or_default()
            .extend(plan.slot_overlays.iter().cloned());
    }
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
                            let actions = actions.iter().map(action_block).collect::<Vec<_>>();
                            let over = overlays
                                .get(&(mode.clone(), class.clone(), kind.clone()))
                                .map(|o| o.iter().map(action_block).collect::<Vec<_>>())
                                .unwrap_or_default();
                            let mut b = Block::zeroed(TYPE_LEN);
                            b.string_id(0, &kind);
                            b.block(4, actions.len() as u32, sorted_array(actions, ACTION_LEN));
                            if !over.is_empty() {
                                b.block(0xc, over.len() as u32, sorted_array(over, ACTION_LEN));
                            }
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
    if !plan.screens.is_empty() {
        let screens = plan
            .screens
            .iter()
            .map(|(label, bytes)| {
                let mut b = Block::zeroed(BLEND_SCREEN_LEN);
                b.string_id(0, label);
                b.bytes[4..BLEND_SCREEN_LEN].copy_from_slice(bytes);
                b
            })
            .collect::<Vec<_>>();
        h.block(0x24, screens.len() as u32, array(screens, BLEND_SCREEN_LEN));
    }
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
    } else if is_vehicle(g) {
        // `(any, any)` and each weapon type's `(type, class)` from Halo 1's
        // tables, as retail vehicle graphs hold (V: the ghost's `(any,
        // any)`, `(gun, fixed)`; the scorpion cannon's `(cannon, fixed)`,
        // `(cg, fixed)`). What the engine reads them for is S.
        let pairs = plan
            .weapon_pairs
            .iter()
            .map(|(t, c)| {
                let mut p = Block::zeroed(8);
                p.string_id(0, t).string_id(4, c);
                p
            })
            .collect::<Vec<_>>();
        h.block(0x54, pairs.len() as u32, sorted_array(pairs, 8));
    }
    if !plan.suspension.is_empty() {
        let n = plan.suspension.len() as u32;
        h.block(0x3c, n, array(plan.suspension, VEHICLE_SUSPENSION_LEN));
    }
    if !plan.object_overlays.is_empty() {
        let n = plan.object_overlays.len() as u32;
        h.block(0x44, n, array(plan.object_overlays, OBJECT_OVERLAY_LEN));
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
