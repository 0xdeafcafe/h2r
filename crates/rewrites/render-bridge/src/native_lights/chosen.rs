//! E3's chosen lights beyond the guest's (docs/graphics/e3-lighting.md,
//! "Lights: which, and how many", slice L6). Once the main view's light
//! pass is over and the frame step has put the view's candidates in the
//! scene, the view is chosen as the render thread chooses it
//! (`native_render::light_choice::choose_view`, its share of the cap from
//! the views of this frame and the frame before), and each lit game light
//! the guest did not draw is drawn here: its layers 12 and 13 over the
//! structure its bounds meet, with the constants extraction made for it
//! without a shadow (`LightCandidate::constants`, `unshadowed`). The host
//! records each as a scene stage of E3 alone
//! (`native_render::Stage::lighting_only`): the frame graph draws it in an
//! E3 frame and skips it in any other, so the scene and the guest's own
//! work are the same whatever the mode.
//!
//! A chosen light the renderer shadows (slice L7: a cone it chooses
//! for one of the view's shadow slots, `native_render::cone_shadows::view_shadows`)
//! is drawn from its shadowed constants with the shadow flag set
//! ([`SHADOW_FLAGS`]): the guest's flag code clears that flag for a light
//! the content flagged no-shadow. Its parts then take the shadowed
//! implementations, which read layer 11's term from the destination
//! alpha, and its pass names the light
//! (its stage's cone shadow) so the frame graph writes the cone's
//! term there just before it.
//!
//! A light is left undrawn, and counted unread, when its candidate holds
//! no constants or its gel is drawn by a shader into target 13 (that
//! producer is the guest's passes', which did not run for it). The
//! first-person parts are lit by [`first_person`], when retail's
//! first-person pass begins.

pub mod first_person;

use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_passes::flag;
use frame_scene::SceneLight;
use native_render::{cone_shadows, light_choice};

use super::{LightConstants, Stage, VIEW_TARGET};

/// The flags a shadowed chosen light's constants carry: the shadow bit
/// and the bit the flag code sets with it (`engine::light_passes::flag`).
pub const SHADOW_FLAGS: u32 = flag::SHADOW | flag::SHADOW_B;

/// One chosen light ready to draw: its datum, the sphere its passes
/// light, its constants, and whether the renderer shadows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChosenLight {
    pub handle: u32,
    pub origin: [f32; 3],
    pub radius: f32,
    pub constants: LightConstants,
    /// Drawn with the shadowed implementations over the renderer's cone
    /// shadow; `constants` are the shadowed set with [`SHADOW_FLAGS`].
    pub shadowed: bool,
}

/// What the host is asked to draw: each light's layers 12 and 13, in the
/// order given, in the view whose target is `view_target`, as scene
/// stages of E3 alone.
pub struct ChosenDraw<'a> {
    pub lights: &'a [ChosenLight],
    pub view_target: [u32; 8],
}

/// `light`'s draw, `shadowed` when the renderer shadows it, or `None`
/// when its constants are missing or its gel needs the guest's target-13
/// producer.
pub fn ready(light: &SceneLight, shadowed: bool) -> Option<ChosenLight> {
    let c = light.candidate.as_ref()?;
    let variants = c.constants.as_ref()?;
    let constants = if shadowed {
        LightConstants {
            flags: variants.shadowed.flags | SHADOW_FLAGS,
            ..variants.shadowed
        }
    } else {
        variants.unshadowed
    };
    let (origin, radius) = c.bounds;
    if constants.animated_gel || !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    Some(ChosenLight {
        handle: light.handle,
        origin,
        radius,
        constants,
        shadowed,
    })
}

/// The view's lights beyond the guest's under `cap`, `lights` the open
/// view's, with `slots` its share of the shadow slots
/// (`cone_shadows::slot_share_of`): the ones ready to draw, in draw
/// order, each shadowed when the renderer shadows it, and how many were
/// not ready.
pub fn beyond_guest(lights: &[SceneLight], cap: usize, slots: usize) -> (Vec<ChosenLight>, usize) {
    let choice = light_choice::choose_view(lights, 0, cap);
    let shadows = cone_shadows::view_shadows(lights, 0, &choice.lit, slots);
    let mut unread = 0;
    let made = choice
        .beyond_guest
        .iter()
        .filter_map(|&i| {
            let made = ready(&lights[i as usize], shadows.contains(&i));
            unread += usize::from(made.is_none());
            made
        })
        .collect();
    (made, unread)
}

/// Draws `lights` (from [`beyond_guest`]) for the view whose light pass is
/// over and counts the outcome. Returns the lights the host drew.
pub fn draw(
    mem: &mut dyn engine::memory::GuestMemory,
    lights: &[ChosenLight],
    unread: usize,
) -> usize {
    CHOSEN_UNREAD.fetch_add(unread as u64, Ordering::Relaxed);
    if lights.is_empty() || Stage::Lights.standing_down() {
        return 0;
    }
    CHOSEN_SHADOWED.fetch_add(
        lights.iter().filter(|l| l.shadowed).count() as u64,
        Ordering::Relaxed,
    );
    let Some(view_target) = VIEW_TARGET.get() else {
        super::refused("light view target unavailable");
        return 0;
    };
    let Some(host) = super::HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    else {
        return 0;
    };
    let drawn = host.draw_chosen(
        mem,
        ChosenDraw {
            lights,
            view_target,
        },
    );
    CHOSEN_DRAWN.fetch_add(drawn as u64, Ordering::Relaxed);
    CHOSEN_KEPT.fetch_add(
        (lights.len() - drawn.min(lights.len())) as u64,
        Ordering::Relaxed,
    );
    drawn
}

static CHOSEN_DRAWN: AtomicU64 = AtomicU64::new(0);
static CHOSEN_KEPT: AtomicU64 = AtomicU64::new(0);
static CHOSEN_UNREAD: AtomicU64 = AtomicU64::new(0);
static CHOSEN_SHADOWED: AtomicU64 = AtomicU64::new(0);

/// Chosen lights handed to the host with their shadowed implementations
/// (`scene_e3_lights_shadowed`).
pub fn shadowed() -> u64 {
    CHOSEN_SHADOWED.load(Ordering::Relaxed)
}

/// Chosen lights the host queued as E3 stages (`scene_e3_lights_drawn`).
pub fn drawn() -> u64 {
    CHOSEN_DRAWN.load(Ordering::Relaxed)
}

/// Chosen lights ready to draw that the host did not queue
/// (`scene_e3_lights_kept`).
pub fn kept() -> u64 {
    CHOSEN_KEPT.load(Ordering::Relaxed)
}

/// Chosen lights with no constants to draw from (`scene_e3_lights_unread`).
pub fn unread() -> u64 {
    CHOSEN_UNREAD.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{LightCandidate, LightVariants};

    fn constants(gel: bool) -> LightConstants {
        LightConstants {
            frame: [0; 36],
            flags: 1,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: gel,
        }
    }

    /// A candidate: datum `index`, its priority, and constants unless
    /// `bare`.
    fn light(index: u32, priority: f32, guest: bool, bare: bool) -> SceneLight {
        SceneLight {
            handle: 0x0001_0000 | index,
            guest_selected: guest,
            candidate: Some(LightCandidate {
                position: [index as f32, 0.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                colour_a: [1.0; 3],
                colour_b: [1.0; 3],
                intensity: 1.0,
                bounds: ([index as f32, 0.0, 0.0], 3.0),
                owner: None,
                definition_kind: 0,
                volume: None,
                view_eligible: Some(true),
                priority: Some(priority),
                weight: 1.0,
                gel_bitmap: None,
                gel_shader: None,
                constants: (!bare).then(|| LightVariants {
                    unshadowed: constants(false),
                    shadowed: LightConstants {
                        flags: 5,
                        ..constants(false)
                    },
                    projection: None,
                }),
            }),
            ..Default::default()
        }
    }

    /// Of six candidates under a cap of four, the guest drew the best; the
    /// next three are drawn here, highest first, with the unshadowed
    /// constants and the bounds' sphere; the two over the cap are not.
    #[test]
    fn the_lit_lights_the_guest_did_not_draw_are_drawn_unshadowed() {
        let lights: Vec<SceneLight> = (0..6)
            .map(|k| light(k, 1.0 + k as f32, k == 5, false))
            .collect();
        let (made, unread) = beyond_guest(&lights, 4, 4);
        assert_eq!(unread, 0);
        assert_eq!(
            made.iter().map(|l| l.handle & 0xffff).collect::<Vec<_>>(),
            [4, 3, 2]
        );
        assert_eq!(made[0].origin, [4.0, 0.0, 0.0]);
        assert_eq!(made[0].radius, 3.0);
        assert_eq!(made[0].constants.flags, 1, "no shadow");
        assert!(made.iter().all(|l| !l.shadowed), "spheres cast no shadow");
    }

    /// `light` as a cone the content flagged no-shadow: a frustum down
    /// -z from one unit above its position, ten deep.
    fn cone(mut light: SceneLight) -> SceneLight {
        light.definition_flags |= cone_shadows::NO_SHADOW;
        let c = light.candidate.as_mut().unwrap();
        c.definition_kind = cone_shadows::FRUSTUM;
        let at = c.position;
        c.forward = [0.0, 0.0, -1.0];
        c.up = [1.0, 0.0, 0.0];
        c.volume = Some(frame_scene::LightVolume::Projected {
            kind: cone_shadows::FRUSTUM,
            origin: [at[0], at[1], at[2] + 1.0],
            axis: [0.0, 0.0, -1.0],
            near: 0.0,
            far: 10.0,
            far_extent: [5.0, 5.0],
        });
        light
    }

    /// Three no-shadow cones beyond the guest's with one shadow slot: the
    /// highest in score is drawn from its shadowed constants with the
    /// shadow flag set again, the others unshadowed.
    #[test]
    fn the_cone_the_renderer_shadows_is_drawn_shadowed() {
        let lights: Vec<SceneLight> = (0..3)
            .map(|k| cone(light(k, 1.0 + k as f32, false, false)))
            .collect();
        let (made, unread) = beyond_guest(&lights, 16, 1);
        assert_eq!(unread, 0);
        let got: Vec<(u32, bool, u32)> = made
            .iter()
            .map(|l| (l.handle & 0xffff, l.shadowed, l.constants.flags))
            .collect();
        assert_eq!(
            got,
            [(2, true, 5 | SHADOW_FLAGS), (1, false, 1), (0, false, 1)]
        );
        let (none, _) = beyond_guest(&lights, 16, 0);
        assert!(none.iter().all(|l| !l.shadowed), "no slot, no shadow");
    }

    /// A lit light with no constants, or with a shader-drawn gel, is
    /// counted unread and not drawn; a host light is never beyond the
    /// guest's.
    #[test]
    fn a_light_without_constants_or_with_a_shader_gel_is_unread() {
        let mut gel = light(1, 3.0, false, false);
        gel.candidate
            .as_mut()
            .unwrap()
            .constants
            .as_mut()
            .unwrap()
            .unshadowed
            .animated_gel = true;
        let lights = vec![
            light(0, 4.0, false, true),
            gel,
            light(2, 2.0, false, false),
            SceneLight::host_light(1, [0.0; 3], [1.0; 3], 2.0),
        ];
        let (made, unread) = beyond_guest(&lights, 16, 4);
        assert_eq!(unread, 2);
        assert_eq!(
            made.iter().map(|l| l.handle & 0xffff).collect::<Vec<_>>(),
            [2]
        );
    }
}
