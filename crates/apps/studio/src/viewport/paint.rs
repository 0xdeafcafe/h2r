//! The viewport follows the Textures tool's painted pixels
//! (docs/editor/realtime-edits.md, R15): an open bitmap whose pixels
//! changed is drawn again as the base map of every material of the level
//! that took it, as the game follows the same pixels through R5 and R11.
//!
//! What it covers is the level's base look (image 0 of the bitmap the
//! shader names as its base map). The look's size and level count are the
//! upload's: an image of another size stays as it was until the map is
//! opened again.

use std::collections::HashMap;

use map::TagId;
use scenario_edit::render::Viewport;

use crate::app::App;

/// What the viewport has drawn of each open bitmap.
#[derive(Default)]
pub(super) struct Painted {
    /// The Textures tool's `(generation, pixel revision)` of each bitmap
    /// last drawn.
    seen: HashMap<TagId, (u64, u64)>,
}

/// What [`Painted::sync`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Synced {
    /// Textures written.
    pub written: usize,
    /// A bitmap drawn painted is no longer open (its edits were dropped):
    /// the textures hold pixels no document has, and the level is
    /// uploaded again.
    pub reload: bool,
}

impl Painted {
    /// The level was uploaded again, from the map as saved.
    pub fn reset(&mut self) {
        self.seen.clear();
    }

    /// Draws every open bitmap that changed since the last call. A bitmap
    /// opened and never painted is not drawn: the level has its pixels.
    pub fn sync(&mut self, vp: &mut Viewport, queue: &wgpu::Queue, app: &App) -> Synced {
        let mut out = Synced::default();
        let textures = &app.textures;
        if self.seen.keys().any(|t| !textures.open.contains_key(t)) {
            out.reload = true;
            return out;
        }
        let mut tags: Vec<&TagId> = textures.open.keys().collect();
        tags.sort_unstable_by_key(|t| t.0);
        for &tag in tags {
            let open = &textures.open[&tag];
            let stamp = (textures.generation, open.bitmap.rev());
            if self.seen.get(&tag) == Some(&stamp) {
                continue;
            }
            if !open.bitmap.is_edited() && !self.seen.contains_key(&tag) {
                continue;
            }
            let Some(layer) = open.bitmap.layer(bitmap_edit::LayerId::new(0, 0, 0)) else {
                continue;
            };
            let (width, height) = (layer.canvas.width(), layer.canvas.height());
            out.written += vp.set_native_base_map(queue, tag, width, height, &layer.canvas.to_rgba());
            self.seen.insert(tag, stamp);
        }
        out
    }
}
