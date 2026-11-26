//! A layer draw's items, typed (docs/graphics/framescene.md, slice 3):
//! the scene's world layers 1, 5, 7, 8 and 20 and the first person's
//! layers, each item with what it draws and, for an object's, its
//! palette rows and its owner's lighting. `scene_extract::extract::layer`
//! reads them from the layer's list when its draw runs, with the material
//! inputs that hold for the whole draw ([`LayerInputs`], slice 4).

use std::ops::Range;

use serde::Serialize;

use crate::object::{Defaults, ItemKey, Lighting};
use crate::scene::{InstanceKey, PartFlags, Subparts};

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
    /// What an object item's render record makes the geometry draw
    /// (`0x0004d0b0`) do beyond its material, as a layer-3 part's
    /// ([`PartFlags`]; the stencil mark only in layers 1 and 3); none for
    /// the structure's.
    pub flags: PartFlags,
    /// With [`PartFlags::SUBPARTS`], the subparts the emitter draws.
    pub subparts: Option<Subparts>,
    /// Why an object item's draw is not one the scene can reproduce: a
    /// named cause, the item's guest draw kept. `None` for one that is,
    /// and for the structure's.
    pub refusal: Option<LayerObjectRefusal>,
    /// Layer 5: the stream and constants the geometry draw adds for an
    /// object item whose model has a PRT block; `None` elsewhere, and
    /// where the draw adds none.
    pub prt: Option<PrtStream>,
}

/// What the geometry draw `0x0004d0b0` adds for an object item of layer
/// 5 whose model has a PRT block (`0x0004d220`: `0x00024040` sets the
/// constants, `0x00024490` points the model's PRT stream at the item's
/// section), as `scene_extract::layout::prt` reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrtStream {
    /// The stream header's type: its record in the stream format table
    /// (`0x0043f408`), which names the registers the stream feeds.
    pub kind: u8,
    /// Bytes per vertex (the header's `+1`).
    pub stride: u8,
    /// Where the item's section's vertices begin in the stream at the
    /// object's detail level: a guest address (the contiguous mapping of
    /// the vertex buffer's data, plus the offset `0x00024490` writes).
    pub data: u32,
    /// The vertex constants from [`PRT_FIRST_CONSTANT`] on: the object's
    /// entry's vector, the fixed vector of `c[69]`, then the object's
    /// coefficients.
    pub constants: Vec<[f32; 4]>,
}

/// The first vertex constant layer 5's draw sets for an object's PRT
/// stream (`c[68]`, hardware numbering).
pub const PRT_FIRST_CONSTANT: u8 = 68;

/// Why an object item of a layer draw keeps its guest draw: what the
/// geometry draw `0x0004d0b0` does for it that the scene does not carry
/// (**Code**, the module notes of `scene_extract::layout::object_walk`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum LayerObjectRefusal {
    /// The shader's flag bit 0 (`+0x16`) while the active structure has a
    /// parameter block (`[[0x004e0348] +0x214] > 0`): the prologue
    /// (`0x0004d640`) runs `0x0003c650` and `0x0003d000`, which set texture
    /// stages, the fog colour and `c[21..25]` from it. In layer 3, also a
    /// flagged record with a structure lightmap index, which the draw does
    /// not bind for the shader.
    ShaderFlag,
    /// The shader's flag bit 0 where the draw may call `0x0003bea0` (its
    /// program's flag bit 20, which a world layer's item does not carry)
    /// before the frame's structure target copy is made (`0x00509430`
    /// clear): that call clears the shader cache and copies and binds a
    /// target.
    ShaderFlagTarget,
    /// The item's `+0x14` or `+0x15` byte is set, or its argument's low
    /// byte is not its key's part: the draw would set up another pass of
    /// the layer or another level of the shader (`+0x14` and `+0x15` go
    /// to the shader begin `0x00016b90` through `0x0004d9c0` as the pass
    /// and level), which the scene's passes are not.
    Argument,
    /// Layer 5: the stream and constants the draw adds for an object's
    /// record whose model has a PRT block (`0x00024040`, `0x00024490`)
    /// do not read, or the draw leaves one of them as the draws before it
    /// left it (no entry for the object, no coefficients, or geometry not
    /// resident).
    Layer5Stream,
    /// The visible subparts do not read.
    Subparts,
    /// Bit 11, and the auxiliary state toggle's inputs do not read.
    AuxState,
    /// The part, its model or its record does not read.
    Unread,
}

impl LayerObjectRefusal {
    /// The cause's name, as `render.scene` counts it.
    pub fn name(self) -> &'static str {
        match self {
            LayerObjectRefusal::ShaderFlag => "object_shader_flag",
            LayerObjectRefusal::ShaderFlagTarget => "object_shader_flag_target",
            LayerObjectRefusal::Argument => "object_argument",
            LayerObjectRefusal::Layer5Stream => "object_layer5_stream",
            LayerObjectRefusal::Subparts => "object_subparts",
            LayerObjectRefusal::AuxState => "object_aux_state",
            LayerObjectRefusal::Unread => "object_unread",
        }
    }
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
