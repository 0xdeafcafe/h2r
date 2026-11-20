//! A dynamic light as the light passes `0x00031170` draw it
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

/// One light of a view, in the guest's light order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct SceneLight {
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
}

impl SceneLight {
    /// Whether the light casts its shadow (flag bit 2).
    pub fn shadowed(&self) -> bool {
        self.flags & 4 != 0
    }
}
