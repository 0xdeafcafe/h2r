//! A candidate dynamic light and its guest draw, when selected
//! (docs/graphics/framescene.md, slice 3): the light, and the layers it
//! runs in the passes' order, each with the target it draws into and the
//! stencil mode set before it. `scene_extract::layout::light_passes`
//! derives the steps from the guest's inputs at the passes' entry.

use std::ops::Range;

use serde::Serialize;

/// The target a light step draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum LightTarget {
    /// The shadow map the layer-begin check `0x00044370` picks (layer 10).
    ShadowMap,
    /// The view's target (`[0x004858b8]`).
    View,
    /// An engine target by index (layer 14's is 1).
    Target(u8),
}

/// One layer of a light's passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightStep {
    pub layer: u8,
    /// The mode `0x00015370` sets before the layer's draw (0 is off);
    /// `None` when the passes set none before it.
    pub stencil: Option<u8>,
    pub target: LightTarget,
    /// Whether the layer's list holds entries for it: the layer draw runs.
    pub draws: bool,
}

/// One light of a view. The guest-selected lights preserve their draw order.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Light {
    /// The light datum.
    pub handle: u32,
    /// The `ligh` tag datum.
    pub definition: u32,
    /// The passes' flags (`[0x004ba014]` as the flag code sets them).
    pub flags: u32,
    /// The definition's flags word.
    pub definition_flags: u32,
    /// The additional casters' generate and apply pair (`0x00030ed0`)
    /// runs between layers 11 and 14.
    pub additional_casters: bool,
    /// Into [`crate::FrameScene::light_steps`], in the passes' order.
    pub steps: Range<u32>,
    /// Whether the guest ran this light's passes.
    pub guest_selected: bool,
    /// The guest's weight after its per-view history and fade.
    pub guest_weight: Option<f32>,
    /// View-candidate inputs and constants, when its record reads.
    pub candidate: Option<LightCandidate>,
    /// A host light (docs/game/forge-monitor-appearance.md, ML5): a light
    /// the host process adds after extraction, with no light datum and no
    /// `ligh` tag; the seat whose body carries it. `None` for the game's
    /// own lights.
    pub host: Option<u8>,
}

/// The datum word of a light that has none: a [`Light::host_light`]'s
/// `handle` and `definition`.
pub const NO_DATUM: u32 = u32::MAX;

impl Light {
    /// Whether the light casts its shadow (flag bit 2).
    pub fn shadowed(&self) -> bool {
        self.flags & 4 != 0
    }

    /// A host light at `origin`: a sphere of `radius` in `colour` (linear
    /// RGB, 0 to 1), carried by `seat`'s body. It has no datum, no `ligh`
    /// tag and no guest steps, casts no shadow (flag bit 2 clear) and is
    /// in its view by the host's choice, so it is always eligible. Its
    /// priority is zero: a renderer that ranks by score puts it after
    /// every game light.
    pub fn host_light(seat: u8, origin: [f32; 3], colour: [f32; 3], radius: f32) -> Light {
        Light {
            handle: NO_DATUM,
            definition: NO_DATUM,
            host: Some(seat),
            candidate: Some(LightCandidate {
                position: origin,
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                colour_a: colour,
                colour_b: colour,
                intensity: 1.0,
                bounds: (origin, radius),
                owner: None,
                definition_kind: 0,
                volume: Some(LightVolume::Sphere { origin, radius }),
                view_eligible: Some(true),
                priority: Some(0.0),
                weight: 1.0,
                gel_bitmap: None,
                gel_shader: None,
                constants: None,
            }),
            ..Default::default()
        }
    }
}

/// The guest pass record already used by the layer and shadow stages.
pub type SceneLight = Light;

/// The record and definition of a candidate, independent of the guest's trim.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightCandidate {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub colour_a: [f32; 3],
    pub colour_b: [f32; 3],
    pub intensity: f32,
    pub bounds: ([f32; 3], f32),
    pub owner: Option<crate::InstanceKey>,
    pub definition_kind: u16,
    /// The effective posed volume after any placement override.
    pub volume: Option<LightVolume>,
    /// Result of the pre-trim view tests, when their inputs read.
    pub view_eligible: Option<bool>,
    /// The view's projected-size priority and the record's unfaded weight.
    pub priority: Option<f32>,
    pub weight: f32,
    pub gel_bitmap: Option<u32>,
    pub gel_shader: Option<u32>,
    pub constants: Option<LightVariants>,
}

/// Shape-specific extents in the light's posed volume.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum LightVolume {
    Sphere {
        origin: [f32; 3],
        radius: f32,
    },
    Projected {
        kind: u16,
        origin: [f32; 3],
        axis: [f32; 3],
        near: f32,
        far: f32,
        far_extent: [f32; 2],
    },
}

impl LightVolume {
    pub fn kind(&self) -> u16 {
        match *self {
            Self::Sphere { .. } => 0,
            Self::Projected { kind, .. } => kind,
        }
    }
}

/// The constant sets for both shadow decisions, using the unfaded weight.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightVariants {
    pub unshadowed: LightConstants,
    pub shadowed: LightConstants,
    /// The projected light's shadow block, when its inputs read.
    pub projection: Option<LightProjection>,
}

impl LightVariants {
    /// Both lighting choices, including the projection if the shadowed
    /// pass needs one.
    pub fn complete(&self) -> bool {
        !self.shadowed.shadowed() || self.projection.is_some()
    }
}

/// The candidate's shadow constants for `c[40]..c[48]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightProjection {
    pub rows: [[u32; 4]; 9],
    pub mode: u8,
    pub size: [i32; 2],
}

/// One light's constants as its passes read them, words as the game
/// stores them (docs/graphics/native-lights-constants.md, "Engine
/// constants"); `engine::light_constants` and `engine::light_passes`
/// compute them without writing guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightConstants {
    /// The 36 dwords `0x0003a550` uploads to `c[31]`..`c[39]`.
    #[serde(serialize_with = "serialize_frame")]
    pub frame: [u32; 36],
    /// The per-light flags (`[0x004ba014]`, `engine::light_passes::flag`).
    pub flags: u32,
    /// Engine vectors 5 and 6: colours A and B as the frame holds them.
    pub colour_a: [u32; 3],
    pub colour_b: [u32; 3],
    /// The colour scale mode (`[0x00485a2c]`, integers 0 and 1).
    pub mode: u32,
    /// The definition's first dword (its first byte picks layer 12's
    /// implementation 4; bit 1 is integer 7), and its words `+0x8a` and
    /// `+0x80` (integers 2 and 3).
    pub definition_flags: u32,
    pub definition_8a: u16,
    pub definition_80: u16,
    /// Falloff word 0 (`[0x00485a30]`, scalar 16).
    pub falloff_0: u32,
    /// The `bitm` datum engine texture 10 (the gel) binds, image 0
    /// (`engine::light_passes::gel`). A shader-driven gel uses the live
    /// target-13 binding supplied separately by its producer.
    pub gel: Option<u32>,
    /// The constants setup also renders a shader into target 13.
    pub animated_gel: bool,
}

impl LightConstants {
    /// Whether the light casts its shadow (flag bit 2): its passes run
    /// layers 10 and 11 before the others (`light_passes`).
    pub fn shadowed(&self) -> bool {
        self.flags & 4 != 0
    }
}

fn serialize_frame<S: serde::Serializer>(frame: &[u32; 36], serializer: S) -> Result<S::Ok, S::Error> {
    frame.as_slice().serialize(serializer)
}
