//! A layer draw's items, typed (docs/graphics/framescene.md, slice 3):
//! the scene's world layers 1, 5, 7, 8 and 20 and the first person's
//! layers, each item with what it draws and, for an object's, its
//! palette rows and its owner's lighting. `scene_extract::extract::layer`
//! reads them from the layer's list when its draw runs.

use std::ops::Range;

use serde::Serialize;

use crate::object::{ItemKey, Lighting};
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
    /// The owner's lighting, for a first-person item.
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
}
