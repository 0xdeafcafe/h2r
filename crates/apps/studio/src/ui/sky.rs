//! The viewport's sky and fog follow the `sky ` tag in the tag document
//! (docs/editor/live-editors.md, "Sky and fog"). A sun direction, a
//! light's colour or the fog's density written to the sky is drawn on
//! the next frame: [`live_atmosphere`] reads the first of the scenario's
//! skies from the document's bytes, edits applied, and
//! [`apply`] hands it to the viewport, which re-uploads nothing.
//!
//! The one place the fan-out meets this view is [`touches_sky`]: the
//! viewport calls [`apply`] whenever the document's revision moves, so
//! every write path (field writes, undo, redo) is covered without a
//! delivery of its own. The own game's sky is read by the engine from
//! the same bytes the Field route writes.
use map::{FourCC, TagId};
use scenario_edit::render::Viewport;
use scenario_edit::scene::{self, Atmosphere};

use crate::app::{TagDoc, live::Change};

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

/// Whether a change writes a sky or fog tag, so the viewport's sun and
/// fog must be read again. The fan-out's hookup (LE1): a Preview that
/// re-reads the document on every revision may call this to skip the
/// read for other tags. `tag` is the changed tag's group, looked up in
/// the document by the caller.
pub fn touches_sky(change: &Change, group_of: impl Fn(TagId) -> Option<FourCC>) -> bool {
    change
        .tag
        .and_then(group_of)
        .is_some_and(|g| g == SKY || g == FourCC(*b"fog "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::live::Class;

    #[test]
    fn a_change_to_a_sky_or_fog_tag_touches_the_sky() {
        let groups = |id: TagId| match id.0 {
            1 => Some(SKY),
            2 => Some(FourCC(*b"fog ")),
            3 => Some(FourCC(*b"weap")),
            _ => None,
        };
        let change = |tag: Option<TagId>| Change::new(Class::Field, tag, vec![(0, 4)], "edit");
        assert!(touches_sky(&change(Some(TagId(1))), groups));
        assert!(touches_sky(&change(Some(TagId(2))), groups));
        assert!(!touches_sky(&change(Some(TagId(3))), groups));
        assert!(!touches_sky(&change(Some(TagId(9))), groups));
        assert!(!touches_sky(&change(None), groups));
    }

    #[test]
    fn a_closed_document_has_no_live_atmosphere() {
        let tags = TagDoc::new(crate::app::tag_tests::registry());
        assert_eq!(live_atmosphere(&tags), None);
    }
}
