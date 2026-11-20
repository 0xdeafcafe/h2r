//! A layer draw's items, typed (docs/graphics/framescene.md, slice 3):
//! the scene's world layers 1, 5, 7, 8 and 20 and the first person's
//! layers, each item with what it draws and, for an object's, its
//! palette rows and its owner's lighting. `scene_extract::extract::layer`
//! reads them from the layer's list when its draw runs, with the material
//! inputs that hold for the whole draw ([`LayerInputs`], slice 4).

use std::ops::Range;

use serde::Serialize;

use crate::object::{Defaults, ItemKey, Lighting};
use crate::scene::InstanceKey;

/// What a layer item's render record draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum LayerOwner {
    /// A structure cluster's section.
    Cluster(u16),
    /// An instanced-geometry placement.
    Placement(u16),
    /// An object's model, by the owner's handle.
    Object(InstanceKey),
}

/// One item of a layer draw's list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayerItem {
    /// The item's index in the list.
    pub slot: u32,
    pub key: ItemKey,
    pub owner: LayerOwner,
    /// The render record's model (`+4`; for a first-person item its
    /// first-person mode); `u32::MAX` for the structure's.
    pub model: u32,
    /// An object item's palette rows (into [`crate::FrameScene::rows`]);
    /// empty for the structure's.
    pub rows: Range<u32>,
    /// An object item's owner's lighting (the scene's and the first
    /// person's); `None` for the structure's, whose draws hold no
    /// lighting (`[0x00485a58]` is 0). The lightmap cycle's coefficients
    /// reach a layer's draws only through it ([`LayerInputs`]).
    pub lighting: Option<Lighting>,
}

/// One layer draw of a view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayerPass {
    pub layer: u8,
    /// The stencil-off flag bytes were set (`0x00485a74`): every item
    /// draws in stencil mode 0.
    pub stencil_off: bool,
    /// The texture accumulation target was there to bind.
    pub texaccum: bool,
    /// Into [`crate::FrameScene::layer_items`], in the list's order.
    pub items: Range<u32>,
    /// Items extraction could not read: the guest still draws them.
    pub remainder: u32,
    /// The material inputs every item of the draw reads alike.
    pub inputs: LayerInputs,
}

/// Engine inputs: the scalar and vector inputs number 64 each, the
/// integer inputs 8 (`0x000336f0`, `0x00033980`, `0x00034060`).
pub const SCALAR_INPUTS: usize = 64;
pub const VECTOR_INPUTS: usize = 64;
pub const INTEGER_INPUTS: usize = 8;

/// What a layer draw's material passes read that is the same for every
/// item of the draw, by value, read when the draw begins
/// (docs/graphics/framescene.md, section 4.3 and slice 4):
///
/// - **Material animation.** An animated instance property driven by
///   game time (`shad +0x3c`, input 0) takes [`Self::time`] over its
///   period (`0x0001b230`); one driven by an object function goes
///   through the owner's callbacks and is not carried.
/// - **The engine inputs that do not read the item.** The render
///   context's (`0x00485600`, copied per view by `0x000132d0`), the
///   view's vectors, the fog's, and the light block's that hold at the
///   draw, evaluated as the selectors evaluate them, raw `f32` bits. Bit
///   `n` of a mask says input `n` is given. The inputs that read the
///   item's lighting (`scene_extract::layout::object_lighting`) and the
///   shader's own (integers 4 and 5, scalar 21) are not here.
/// - **The defaults** the lighting inputs fall back on without a
///   lighting: a structure item's.
/// - **The lightmap cycle.** `0x00022850` (each view's begin,
///   `0x00012a50`) steps the probe coefficients the object lighting
///   `0x00023690` samples through `0x00022cf0`; nothing else reads them.
///   So the cycle reaches a layer's draws only through each object
///   item's [`LayerItem::lighting`], read at the draw, after the cycle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LayerInputs {
    /// The render time, seconds (`0x004858a0`, a double); `None` when it
    /// does not read.
    pub time: Option<f64>,
    #[serde(skip)]
    pub scalars: [u32; SCALAR_INPUTS],
    pub scalars_given: u64,
    #[serde(skip)]
    pub vectors: [[u32; 3]; VECTOR_INPUTS],
    pub vectors_given: u64,
    #[serde(skip)]
    pub ints: [u32; INTEGER_INPUTS],
    pub ints_given: u8,
    pub defaults: Option<Defaults>,
}

impl Default for LayerInputs {
    fn default() -> Self {
        Self {
            time: None,
            scalars: [0; SCALAR_INPUTS],
            scalars_given: 0,
            vectors: [[0; 3]; VECTOR_INPUTS],
            vectors_given: 0,
            ints: [0; INTEGER_INPUTS],
            ints_given: 0,
            defaults: None,
        }
    }
}

impl LayerInputs {
    /// Scalar input `n`, if given.
    pub fn scalar(&self, n: usize) -> Option<f32> {
        (n < SCALAR_INPUTS && self.scalars_given >> n & 1 != 0).then(|| f32::from_bits(self.scalars[n]))
    }

    /// Vector input `n`, if given.
    pub fn vector(&self, n: usize) -> Option<[f32; 3]> {
        (n < VECTOR_INPUTS && self.vectors_given >> n & 1 != 0).then(|| self.vectors[n].map(f32::from_bits))
    }

    /// Integer input `n`, if given.
    pub fn int(&self, n: usize) -> Option<i32> {
        (n < INTEGER_INPUTS && self.ints_given >> n & 1 != 0).then(|| self.ints[n] as i32)
    }

    /// Gives scalar input `n` the value `bits`.
    pub fn set_scalar(&mut self, n: usize, bits: u32) {
        if n < SCALAR_INPUTS {
            self.scalars[n] = bits;
            self.scalars_given |= 1 << n;
        }
    }

    /// Gives vector input `n` the value `bits`.
    pub fn set_vector(&mut self, n: usize, bits: [u32; 3]) {
        if n < VECTOR_INPUTS {
            self.vectors[n] = bits;
            self.vectors_given |= 1 << n;
        }
    }

    /// Gives integer input `n` the value `word`.
    pub fn set_int(&mut self, n: usize, word: u32) {
        if n < INTEGER_INPUTS {
            self.ints[n] = word;
            self.ints_given |= 1 << n;
        }
    }
}
