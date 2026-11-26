//! The viewport's sky and fog follow the `sky ` tag in the tag document
//! (docs/editor/live-editors.md, "Sky and fog"). A sun direction, a
//! light's colour or the fog's density written to the sky is drawn on
//! the next frame: [`live_atmosphere`] reads the first of the scenario's
//! skies from the document's bytes, edits applied, and
//! [`apply`] hands it to the viewport, which re-uploads nothing.
//!
//! The fan-out has no delivery into this view: the viewport calls
//! [`apply`] whenever the document's revision moves, so every write path
//! (field writes, undo, redo) is covered. The own game's sky is read by the engine from
//! the same bytes the Field route writes.
use map::FourCC;
use scenario_edit::render::Viewport;
use scenario_edit::scene::{self, Atmosphere};

use crate::app::TagDoc;

const SCNR: FourCC = FourCC(*b"scnr");
const SKY: FourCC = FourCC(*b"sky ");

/// The sun and fog of the first of the open scenario's skies that
/// decodes, read from the tag document with its edits applied. `None`
/// when the document is closed or no sky reads (a sky whose payload is
/// an alias of another's, or a tag the map has no data for): the scene's
/// own atmosphere stands.
pub fn live_atmosphere(tags: &TagDoc) -> Option<Atmosphere> {
    let scnr = tags
        .tags()
        .iter()
        .find(|t| t.group == SCNR && t.size != 0)?;
    let payload = tags.read(scnr.id, scnr.address, scnr.size)?;
    scene::atmosphere_in_doc(
        &payload,
        scnr.address,
        &mut |id| {
            let t = tags.tag(id).filter(|t| t.group == SKY && t.size != 0)?;
            Some((t.name.clone(), t.address, tags.read(id, t.address, t.size)?))
        },
        &mut |id, at, n| tags.read(id, at, n),
    )
}

/// Draw the viewport with the document's sky: its live atmosphere, else
/// the scene's. A viewport with no scene draws no sun or fog and keeps
/// none.
pub fn apply(vp: &mut Viewport, scene: Option<&Atmosphere>, tags: &TagDoc) {
    let Some(scene) = scene else { return };
    let atmosphere = live_atmosphere(tags);
    vp.set_atmosphere(atmosphere.as_ref().unwrap_or(scene));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_document_has_no_live_atmosphere() {
        let tags = TagDoc::new(crate::app::tag_tests::registry());
        assert_eq!(live_atmosphere(&tags), None);
    }
}
