//! The areas and shown flags a document gives the map it builds, as the
//! areas record Studio saves in a map's tail
//! (`game_markers::record`, docs/game/gametype-markers.md, "Areas saved
//! with a map"). A netgame flag has no bytes for either, so the record
//! carries them to the game that loads the map: it attaches each row to
//! its flag (`game::map_areas::attach`), so a match on the variant draws
//! the play form of every flag the document shows
//! (docs/game/gametype-marker-forms.md, "When a form shows").
use std::collections::{BTreeMap, BTreeSet};

use forge_doc::{BlockKind, ForgeDoc};
use game_markers::record::{self, Row};
use game_markers::{Form, Kind};
use scenario_edit::Scenario;

use crate::Error;

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn invalid(field: &str) -> Error {
    Error::Scenario(scenario_edit::Error::Invalid(match field {
        "type" => "a netgame flag has no type",
        "identifier" => "a netgame flag has no identifier",
        _ => "a netgame flag has no position",
    }))
}

/// Whether a flag labelled `text` has a play form to show: a flag whose
/// marker draws nothing in play (a drop, a base, a cover mark, a siege
/// line) has no use for the flag.
fn has_play_form(text: &str, area: bool) -> bool {
    let kind = if text == "hill_corner" {
        area.then(|| Kind::Hill(game_markers::Area::default()))
    } else {
        Kind::of_label(text)
    };
    kind.is_some_and(|k| {
        let mesh = k.mesh_in(Form::Play, [1.0; 3]);
        !mesh.lines.is_empty() || !mesh.tris.is_empty()
    })
}

/// The record of `doc`'s areas and shown flags over the finished
/// `scenario`: one row for each flag with an area, or shown and with a play
/// form to show, in flag order. None when there is no such flag, so a
/// document with none builds the bytes it always did.
pub(crate) fn record(
    doc: &ForgeDoc,
    base_counts: &BTreeMap<BlockKind, u32>,
    scenario: &Scenario,
) -> Result<Option<Vec<u8>>, Error> {
    let areas = doc.areas(base_counts)?;
    let labels = doc.labels(base_counts)?;
    let labelled = |index: u32| {
        labels
            .active
            .iter()
            .find(|l| l.block == FLAGS && l.index == index)
            .map(|l| l.label.text.clone())
    };
    let shown = doc.shown_flags(base_counts)?;
    let indices: BTreeSet<u32> = areas.keys().chain(shown.iter()).copied().collect();
    let flags = scenario.block(FLAGS).len() as u32;
    let mut rows = Vec::new();
    for index in indices.into_iter().filter(|&i| i < flags) {
        let int = |field: &str| {
            scenario
                .get(FLAGS, index as usize, field)
                .ok()
                .and_then(|v| v.as_int())
                .and_then(|v| i16::try_from(v).ok())
                .ok_or_else(|| invalid(field))
        };
        let at = scenario
            .get(FLAGS, index as usize, "position")
            .ok()
            .and_then(|v| v.as_vec3())
            .ok_or_else(|| invalid("position"))?;
        let flag_type = int("type")?;
        let area = areas.get(&index).cloned();
        let text = labelled(index).or_else(|| {
            u16::try_from(flag_type)
                .ok()
                .and_then(forge_doc::builtin_label)
                .map(|l| l.text)
        });
        let show = shown.contains(&index)
            && text.is_some_and(|text| has_play_form(&text, area.is_some()));
        if area.is_none() && !show {
            continue;
        }
        rows.push(Row {
            index,
            flag_type,
            identifier: int("identifier")?,
            at,
            area,
            show,
        });
    }
    Ok((!rows.is_empty()).then(|| record::encode(&rows)))
}
