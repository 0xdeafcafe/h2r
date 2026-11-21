//! The compiled records (`hud_kit_core::records`: the same bytes the game
//! is given), and the widgets read back as the HUD editor reads a map's,
//! for the preview's draw.

use hud_edit::model::{self as hm, WidgetKey};
pub use hud_kit_core::records::{
    Compiled, Error, KIT_INPUT, NO_FONT, Widget, WidgetKind, compile, placed,
};

/// The HUD editor's kind for the core's.
pub fn edit_kind(k: WidgetKind) -> hm::WidgetKind {
    match k {
        WidgetKind::Bitmap => hm::WidgetKind::Bitmap,
        WidgetKind::Text => hm::WidgetKind::Text,
    }
}

/// A [`Compiled`] HUD as the HUD editor's model.
pub trait AsHudEdit {
    /// The widgets read back as `hud_edit` reads a map's, at metadata
    /// address `base` (bitmaps, then texts), named by their element ids.
    fn as_hud_edit(&self, base: u32) -> hm::Hud;
}

impl AsHudEdit for Compiled {
    fn as_hud_edit(&self, base: u32) -> hm::Hud {
        let mut widgets = Vec::new();
        let mut address = base;
        for kind in [WidgetKind::Bitmap, WidgetKind::Text] {
            for (index, w) in self.of(kind).enumerate() {
                widgets.push(hm::Widget {
                    key: WidgetKey {
                        kind: edit_kind(kind),
                        index,
                    },
                    address,
                    bytes: w.bytes.clone(),
                    original: w.bytes.clone(),
                    name: w.id.to_owned(),
                });
                address += w.bytes.len() as u32;
            }
        }
        hm::Hud {
            id: map::TagId::NULL,
            name: self.hud.to_owned(),
            widgets,
        }
    }
}
