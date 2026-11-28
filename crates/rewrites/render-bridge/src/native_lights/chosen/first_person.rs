//! E3's first-person pass for the chosen lights the guest did not record
//! (docs/graphics/e3-lighting.md, "The flashlight and the first-person
//! pass"). Retail's first-person pass (`0x000404b0`) replays only the
//! lights recorded in its slots (`engine::render::light_gather::first_person::recorded`);
//! a light E3 chooses that has no slot has no `GroupSite::FirstPerson`
//! groups to draw from. When that pass begins, the frame step takes the
//! first `light_choice::FIRST_PERSON_CAP` of the game view's lit lights
//! ([`light_choice::ViewChoice::first_person`]), keeps the ones with no
//! slot whose sphere reaches the first-person camera ([`REACH`]), and
//! hands them to the host with the first-person view's object parts
//! ([`parts`]), each posed by its node rows. The host draws each light's
//! layers 12 and 13 over those parts from the light's constants and each
//! part's material, as a scene stage of E3 alone
//! (`native_render::Stage::lighting_only`).
//!
//! The constants are the candidate's unshadowed set, with the flags
//! retail's pass sets ([`flags`]): bit 0 for a spherical volume, bit 1
//! for a colour B component above zero, the shadow bits clear. The
//! first-person model stays outside the shadow maps.

use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_passes::flag;
use frame_scene::{
    LayerItem, LayerObjectRefusal, LayerOwner, LightConstants, PartFlags, Row, SceneLight,
};
use native_render::light_choice;

use super::ChosenLight;
use crate::native_lights::{Stage, VIEW_TARGET};

/// How far past a light's sphere the first-person parts may lie, in
/// world units, from the first-person camera. The weapon and arms are
/// drawn within this of the eye.
pub const REACH: f32 = 0.5;

/// One object part of the first-person view: the model (the
/// first-person mode datum), its section and part, the shader its item
/// binds, and the node rows that pose it.
#[derive(Clone, Debug, PartialEq)]
pub struct FirstPersonPart {
    pub model: u32,
    pub section: u16,
    pub part: u8,
    pub shader: u32,
    pub rows: Vec<Row>,
}

/// What the host is asked to draw: each light's layers 12 and 13 over
/// `parts`, in the order given, at the view whose target is
/// `view_target`, as scene stages of E3 alone.
pub struct FirstPersonDraw<'a> {
    pub lights: &'a [ChosenLight],
    pub parts: &'a [FirstPersonPart],
    pub view_target: [u32; 8],
}

/// The flags retail's first-person pass sets for a light
/// (`0x000404b0`, step 4): bit 0 for a spherical volume, as the light's
/// own pass found it; bit 1 for a colour B component above zero (a NaN
/// is not); bits 2 to 5, the shadow's, clear.
pub fn flags(c: &LightConstants) -> u32 {
    let specular = c.colour_b.iter().any(|&w| f32::from_bits(w) > 0.0);
    (c.flags & flag::CONTEXT_ZERO) | if specular { flag::SPECULAR } else { 0 }
}

/// Whether a light's sphere reaches the first-person parts: its centre
/// within its radius and [`REACH`] of the eye.
pub fn reaches(origin: [f32; 3], radius: f32, eye: [f32; 3]) -> bool {
    let d: f32 = (0..3).map(|i| (origin[i] - eye[i]).powi(2)).sum();
    let r = radius + REACH;
    d.is_finite() && d <= r * r
}

/// The game view's first-person lights this pass draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FirstPersonSet {
    /// In the choice's order, unshadowed with [`flags`].
    pub lights: Vec<ChosenLight>,
    /// Lights with no constants to draw from, or a shader gel.
    pub unread: usize,
    /// Lights whose sphere does not reach the eye.
    pub far: usize,
}

/// The first of the game view's lit lights (`lights` its slice, `cap` its
/// share) that retail's first-person pass did not record (`recorded`, the
/// slots' datums): host lights left out, each ready to draw unshadowed
/// with [`flags`], and only those that reach `eye`.
pub fn choose(
    lights: &[SceneLight],
    cap: usize,
    recorded: &[u32],
    eye: [f32; 3],
) -> FirstPersonSet {
    let choice = light_choice::choose_view(lights, 0, cap);
    let mut set = FirstPersonSet::default();
    for &i in &choice.first_person {
        let light = &lights[i as usize];
        if light.host.is_some() || recorded.contains(&light.handle) {
            continue;
        }
        let Some(mut made) = super::ready(light, false) else {
            set.unread += 1;
            continue;
        };
        if !reaches(made.origin, made.radius, eye) {
            set.far += 1;
            continue;
        }
        made.constants.flags = flags(&made.constants);
        set.lights.push(made);
    }
    set
}

/// Whether a first-person item's part can be lit from the scene: an
/// object's, posed by rows, drawing all its indices with its view's
/// projection (no subparts, no auxiliary depth), and read.
fn lit(item: &LayerItem, rows: &[Row]) -> bool {
    let whole = !item.flags.contains(PartFlags::SUBPARTS)
        && !item.flags.contains(PartFlags::AUX_DEPTH)
        && !matches!(
            item.refusal,
            Some(
                LayerObjectRefusal::Subparts
                    | LayerObjectRefusal::AuxState
                    | LayerObjectRefusal::Unread
            )
        );
    matches!(item.owner, LayerOwner::Object(_)) && !rows.is_empty() && whole
}

/// The first-person view's object parts, each once in the order its
/// layers first list it, from `items` (each with its rows), and how many
/// object parts are left out: drawn as subparts, with auxiliary depth, or
/// with no rows. The structure's items are not the first person's.
pub fn parts<'a>(
    items: impl IntoIterator<Item = (&'a LayerItem, &'a [Row])>,
) -> (Vec<FirstPersonPart>, usize) {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    let mut left = 0;
    for (item, rows) in items {
        let LayerOwner::Object(owner) = item.owner else {
            continue;
        };
        let key = (
            owner,
            item.model,
            item.key.record,
            item.key.section,
            item.key.part,
        );
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if !lit(item, rows) {
            left += 1;
            continue;
        }
        out.push(FirstPersonPart {
            model: item.model,
            section: item.key.section,
            part: item.key.part,
            shader: item.key.shader,
            rows: rows.to_vec(),
        });
    }
    (out, left)
}

/// What the frame step found when retail's first-person pass began
/// (`scene_step::first_person_chosen`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FirstPersonPass {
    pub set: FirstPersonSet,
    pub parts: Vec<FirstPersonPart>,
    /// Object parts left out ([`parts`]).
    pub left: usize,
}

/// Retail's first-person pass begins: draws the chosen lights it did not
/// record over the first-person parts, and counts. Returns the lights the
/// host queued.
pub fn at_pass(mem: &mut dyn engine::memory::GuestMemory) -> usize {
    match crate::scene_step::first_person_chosen(&*mem) {
        Some(pass) => draw(mem, &pass),
        None => 0,
    }
}

/// Draws `pass`'s lights over its parts and counts the outcome.
pub fn draw(mem: &mut dyn engine::memory::GuestMemory, pass: &FirstPersonPass) -> usize {
    UNREAD.fetch_add(pass.set.unread as u64, Ordering::Relaxed);
    FAR.fetch_add(pass.set.far as u64, Ordering::Relaxed);
    PARTS_LEFT.fetch_add(pass.left as u64, Ordering::Relaxed);
    let lights = &pass.set.lights;
    if lights.is_empty() || Stage::Lights.standing_down() {
        return 0;
    }
    PARTS.fetch_add(pass.parts.len() as u64, Ordering::Relaxed);
    let host = super::super::HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let drawn = match (VIEW_TARGET.get(), host, pass.parts.is_empty()) {
        (Some(view_target), Some(host), false) => host.draw_first_person(
            mem,
            FirstPersonDraw {
                lights,
                parts: &pass.parts,
                view_target,
            },
        ),
        _ => 0,
    };
    DRAWN.fetch_add(drawn as u64, Ordering::Relaxed);
    KEPT.fetch_add(
        (lights.len() - drawn.min(lights.len())) as u64,
        Ordering::Relaxed,
    );
    drawn
}

static DRAWN: AtomicU64 = AtomicU64::new(0);
static KEPT: AtomicU64 = AtomicU64::new(0);
static UNREAD: AtomicU64 = AtomicU64::new(0);
static FAR: AtomicU64 = AtomicU64::new(0);
static PARTS: AtomicU64 = AtomicU64::new(0);
static PARTS_LEFT: AtomicU64 = AtomicU64::new(0);

/// First-person lights the host queued as E3 stages
/// (`scene_e3_first_person_lights_drawn`).
pub fn drawn() -> u64 {
    DRAWN.load(Ordering::Relaxed)
}

/// First-person lights ready to draw that the host did not queue, or
/// that found no part (`scene_e3_first_person_lights_kept`).
pub fn kept() -> u64 {
    KEPT.load(Ordering::Relaxed)
}

/// First-person lights with no constants to draw from
/// (`scene_e3_first_person_lights_unread`).
pub fn unread() -> u64 {
    UNREAD.load(Ordering::Relaxed)
}

/// First-person lights whose sphere does not reach the eye
/// (`scene_e3_first_person_lights_far`).
pub fn far() -> u64 {
    FAR.load(Ordering::Relaxed)
}

/// First-person parts offered to the host with the lights
/// (`scene_e3_first_person_parts`).
pub fn parts_offered() -> u64 {
    PARTS.load(Ordering::Relaxed)
}

/// First-person object parts left out ([`parts`];
/// `scene_e3_first_person_parts_left`).
pub fn parts_left() -> u64 {
    PARTS_LEFT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{InstanceKey, ItemKey, LightCandidate, LightVariants};

    fn constants(flags: u32, colour_b: f32) -> LightConstants {
        LightConstants {
            frame: [0; 36],
            flags,
            colour_a: [0; 3],
            colour_b: [colour_b.to_bits(); 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: false,
        }
    }

    /// A lit candidate: datum `index` at `x` on the x axis, radius one,
    /// with its score, and constants unless `bare`.
    fn light(index: u32, priority: f32, x: f32, bare: bool) -> SceneLight {
        SceneLight {
            handle: 0x0002_0000 | index,
            candidate: Some(LightCandidate {
                position: [x, 0.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                colour_a: [1.0; 3],
                colour_b: [1.0; 3],
                intensity: 1.0,
                bounds: ([x, 0.0, 0.0], 1.0),
                owner: None,
                definition_kind: 0,
                volume: None,
                view_eligible: Some(true),
                priority: Some(priority),
                weight: 1.0,
                gel_bitmap: None,
                gel_shader: None,
                constants: (!bare).then(|| LightVariants {
                    // The light's own pass: spherical, specular over its
                    // threshold, shadowed bits set.
                    unshadowed: constants(
                        flag::CONTEXT_ZERO | flag::SPECULAR | flag::TARGET_10,
                        0.5,
                    ),
                    shadowed: constants(flag::SHADOW | flag::SHADOW_B, 0.5),
                    projection: None,
                }),
            }),
            ..Default::default()
        }
    }

    /// Ten lights in the game view; the first-person set is the eight
    /// highest. The two retail recorded are not drawn again, the one far
    /// from the eye is counted far, the one without constants unread, and
    /// the rest are drawn in the choice's order, unshadowed with the
    /// first-person flags.
    #[test]
    fn the_first_person_set_the_guest_did_not_record_is_drawn_over_the_eye() {
        let mut lights: Vec<SceneLight> = (0..10)
            .map(|k| light(k, 10.0 - k as f32, 0.0, false))
            .collect();
        lights[2] = light(2, 8.0, 40.0, false);
        lights[3] = light(3, 7.0, 0.0, true);
        let recorded = [lights[0].handle, lights[5].handle];
        let set = choose(&lights, 16, &recorded, [0.5, 0.0, 0.0]);
        assert_eq!(set.far, 1);
        assert_eq!(set.unread, 1);
        assert_eq!(
            set.lights
                .iter()
                .map(|l| l.handle & 0xffff)
                .collect::<Vec<_>>(),
            [1, 4, 6, 7],
            "the first eight lit, less the recorded, the far and the unread"
        );
        assert!(set.lights.iter().all(|l| !l.shadowed));
        assert!(
            set.lights
                .iter()
                .all(|l| l.constants.flags == flag::CONTEXT_ZERO | flag::SPECULAR),
            "the first-person flags: no shadow or target-10 bits"
        );
    }

    #[test]
    fn retails_first_person_flags_keep_the_sphere_and_take_colour_b_above_zero() {
        assert_eq!(
            flags(&constants(flag::CONTEXT_ZERO, 0.0)),
            flag::CONTEXT_ZERO
        );
        assert_eq!(flags(&constants(0, 0.25)), flag::SPECULAR);
        assert_eq!(flags(&constants(0, f32::NAN)), 0, "a NaN is not above zero");
        assert_eq!(
            flags(&constants(flag::SHADOW | flag::SHADOW_FALLOFF, -1.0)),
            0
        );
    }

    #[test]
    fn a_light_reaches_the_eye_within_its_radius_and_the_reach() {
        assert!(reaches([2.0, 0.0, 0.0], 1.5, [0.0; 3]));
        assert!(!reaches([2.1, 0.0, 0.0], 1.5, [0.0; 3]));
        assert!(!reaches([f32::NAN, 0.0, 0.0], 1.5, [0.0; 3]));
    }

    fn item(owner: LayerOwner, record: u16, part: u8, flags: PartFlags) -> LayerItem {
        LayerItem {
            slot: 0,
            key: ItemKey {
                record,
                shader: 0x00e0_0000 | u32::from(part),
                section: 2,
                part,
                implementation: 0,
            },
            level: 0, pass: 0, owner,
            model: 0x00f0_0001,
            rows: 0..0,
            lighting: None,
            flags,
            subparts: None,
            refusal: None,
            prt: None,
        }
    }

    /// The first-person view lists its weapon's parts in several layers:
    /// each is drawn once with its rows, in first-listed order; the
    /// structure's items are not the first person's, and a part drawn as
    /// subparts or with auxiliary depth, or without rows, is left out.
    #[test]
    fn the_first_person_parts_are_its_object_parts_once_with_their_rows() {
        let weapon = LayerOwner::Object(InstanceKey(7));
        let rows: Vec<Row> = (0..6).map(|k| [k as f32; 4]).collect();
        let items = [
            (item(weapon, 1, 0, PartFlags::NONE), &rows[0..3]),
            (
                item(LayerOwner::Cluster(4), 0, 0, PartFlags::NONE),
                &rows[0..0],
            ),
            (item(weapon, 1, 1, PartFlags::NONE), &rows[3..6]),
            (item(weapon, 1, 0, PartFlags::NONE), &rows[0..3]),
            (item(weapon, 1, 2, PartFlags::SUBPARTS), &rows[0..3]),
            (item(weapon, 1, 3, PartFlags::AUX_DEPTH), &rows[0..3]),
            (item(weapon, 1, 4, PartFlags::NONE), &rows[0..0]),
        ];
        let (got, left) = parts(items.iter().map(|(i, r)| (i, *r)));
        assert_eq!(left, 3);
        assert_eq!(got.iter().map(|p| p.part).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(got[1].rows, rows[3..6].to_vec());
        assert_eq!(
            (got[0].model, got[0].section, got[0].shader),
            (0x00f0_0001, 2, 0x00e0_0000)
        );
    }
}
