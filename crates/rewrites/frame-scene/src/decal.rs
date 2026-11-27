//! The decals a view draws, typed (docs/graphics/native-decals-research.md):
//! one item per decal record the guest's decal callback `0x00044040` would
//! draw, in its order, with the record's bitmap, tint and quad-list
//! vertices. `scene_extract::decals` reads them from the decal datums;
//! `native_render::decal_scene` draws them. Each item draws in the slot
//! of its mode ([`DecalItem::slot`]): mode 0 in slot 2, mode 1 in slot 5,
//! modes 2 to 6 in slot 7.

use std::ops::Range;

use serde::Serialize;

use crate::builder::{BuildError, SceneBuilder};
use crate::scene::{FrameScene, View};
use crate::view_slot::ViewSlot;

/// Bytes of one guest decal vertex: float3 position, then a packed
/// 11:11:10 signed normalised word.
pub const VERTEX_SIZE: u32 = 16;

/// One decal vertex as the guest stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DecalVertex {
    /// The three position floats, by their bits.
    pub position: [u32; 3],
    /// The `NORMPACKED3` word.
    pub packed: u32,
}

impl DecalVertex {
    pub fn position(&self) -> [f32; 3] {
        self.position.map(f32::from_bits)
    }
}

/// One decal record's draw: the callback's one array draw of `quads`
/// quads, after its bitmap and tint setup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecalItem {
    /// The decal pass mode, 0 to 6.
    pub mode: u8,
    /// The visible cluster whose list the record is in.
    pub cluster: u16,
    /// The record's index in the decal datum array.
    pub record: u16,
    /// The bitmap tag datum (`+0x2c`) and the signed image selector
    /// (`+0x29`).
    pub bitmap: u32,
    pub image: i8,
    /// The record's tint (`+0x24`), the vertex default `v10` as four bytes.
    pub colour: u32,
    /// Into [`FrameScene::decal_vertices`]: `4 * quads` vertices of a quad list.
    pub vertices: Range<u32>,
}

impl DecalItem {
    /// The slot the item draws in; `None` for a mode the guest does not have.
    pub fn slot(&self) -> Option<ViewSlot> {
        ViewSlot::decal(self.mode)
    }

    pub fn quads(&self) -> u32 {
        self.vertices.len() as u32 / 4
    }
}

impl FrameScene {
    /// The view's decals, in the guest's order: by mode, then the cluster
    /// list's order, then each bucket's chain.
    pub fn decals_of(&self, view: &View) -> &[DecalItem] {
        self.decals.get(view.decals.start as usize..view.decals.end as usize).unwrap_or(&[])
    }

    pub fn decal_vertices_of(&self, item: &DecalItem) -> &[DecalVertex] {
        self.decal_vertices.get(item.vertices.start as usize..item.vertices.end as usize).unwrap_or(&[])
    }
}

/// Every view's decals inside the array and after the view before's,
/// every item's vertices inside theirs, in whole quads.
pub(crate) fn check(scene: &FrameScene) -> Result<(), String> {
    let mut last = 0;
    for (n, v) in scene.views.iter().enumerate() {
        let r = &v.decals;
        if r.start > r.end || r.end as usize > scene.decals.len() {
            return Err(format!("view {n} decals {r:?} outside 0..{}", scene.decals.len()));
        }
        if r.start < last {
            return Err(format!("view {n} decals {r:?} overlap the view before"));
        }
        last = r.end;
    }
    for (n, d) in scene.decals.iter().enumerate() {
        let r = &d.vertices;
        if r.start > r.end || r.end as usize > scene.decal_vertices.len() || r.len() % 4 != 0 {
            return Err(format!("decal {n} vertices {r:?} outside 0..{} or not whole quads", scene.decal_vertices.len()));
        }
    }
    Ok(())
}

/// The view's decals as summary-hash words, in order.
pub(crate) fn summary_words(scene: &FrameScene, view: &View, word: &mut dyn FnMut(u32)) {
    let items = scene.decals_of(view);
    word(items.len() as u32);
    for d in items {
        word(u32::from(d.mode) | u32::from(d.cluster) << 8 | u32::from(d.record) << 24);
        word(d.bitmap);
        word(u32::from(d.image as u8));
        word(d.colour);
        for v in scene.decal_vertices_of(d) {
            v.position.iter().for_each(|&x| word(x));
            word(v.packed);
        }
    }
    word(view.decal_remainder);
}

impl SceneBuilder {
    /// Adds a decal item with its vertices to the open view, after its
    /// decals so far; its index in the scene. The builder sets `item.vertices`.
    pub fn push_decal(&mut self, item: DecalItem, vertices: &[DecalVertex]) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let first = scene.decal_vertices.len() as u32;
        scene.decal_vertices.extend_from_slice(vertices);
        let index = scene.decals.len() as u32;
        scene.decals.push(DecalItem { vertices: first..scene.decal_vertices.len() as u32, ..item });
        scene.views[open].decals.end = index + 1;
        Ok(index)
    }

    /// Counts decal records the guest still has: extraction could not read
    /// them (named by `scene_dropped_decal_*` where they were to be drawn).
    pub fn add_decal_remainder(&mut self, items: u32) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].decal_remainder += items;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{SceneId, Stamp, ViewKey, ViewKind};
    use crate::test_camera::camera;

    fn key(view: u8) -> ViewKey {
        ViewKey { kind: ViewKind::Game, player: 0, view }
    }

    fn item(mode: u8, record: u16) -> DecalItem {
        DecalItem { mode, cluster: 3, record, bitmap: 0xe600_0001, image: -1, colour: 0xff80_4020, vertices: 0..0 }
    }

    fn quad(x: f32) -> Vec<DecalVertex> {
        (0..4).map(|i| DecalVertex { position: [x.to_bits(), (i as f32).to_bits(), 0], packed: i }).collect()
    }

    #[test]
    fn a_views_decals_keep_their_order_and_slots() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(b.push_decal(item(0, 0), &[]).unwrap_err(), BuildError::NoView);
        b.open_view(key(0), camera()).unwrap();
        b.push_decal(item(2, 9), &quad(1.0)).unwrap();
        b.push_decal(item(0, 4), &quad(2.0)).unwrap();
        b.add_decal_remainder(1).unwrap();
        b.open_view(key(1), camera()).unwrap();
        b.push_decal(item(6, 1), &quad(3.0)).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let first = s.decals_of(&s.views[0]);
        assert_eq!(first.iter().map(|d| d.record).collect::<Vec<_>>(), [9, 4]);
        assert_eq!(s.decal_vertices_of(&first[1])[0].position()[0], 2.0);
        assert_eq!(first[1].quads(), 1);
        assert_eq!(first.iter().map(|d| d.slot().and_then(ViewSlot::number)).collect::<Vec<_>>(), [Some(7), Some(2)]);
        assert_eq!(s.views[0].decal_remainder, 1);
        assert_eq!(s.decals_of(&s.views[1]).len(), 1);
        let mut bad = (*s).clone();
        bad.decals[0].vertices = 0..3;
        assert!(bad.check().unwrap_err().contains("whole quads"));
    }
}
