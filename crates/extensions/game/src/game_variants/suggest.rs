//! The map variants a game variant may suggest, from this machine
//! (docs/game/forge-map-variants.md, "How a pick names a map variant"):
//! the choices a "Suggest map variants" step offers, and the suggestions a
//! pick of them makes, for [`super::store::Store::set_maps`] or
//! `Bundle::with_maps`.

use std::path::Path;

use ::variants::portable::{MAX_MAPS, MapSuggestion};
use forge::files::variants::Suggestable;

/// The choices: every map variant under the data root a lobby could pick,
/// saved then fetched, by map and name. Empty when there are none or the
/// directory cannot be read.
pub fn choices(data: &Path) -> Vec<Suggestable> {
    forge::files::variants::suggestable(data).unwrap_or_default()
}

/// The suggestions for the choices `picked` (their document ids), in the
/// order picked. An id that is not among `choices` is refused by name, as
/// is a pick of more than [`MAX_MAPS`].
pub fn suggestions(
    choices: &[Suggestable],
    picked: &[String],
) -> Result<Vec<MapSuggestion>, String> {
    if picked.len() > MAX_MAPS {
        return Err(format!(
            "at most {MAX_MAPS} map variants are suggested, not {}",
            picked.len()
        ));
    }
    picked
        .iter()
        .map(|id| {
            choices
                .iter()
                .find(|c| c.id.to_string() == *id)
                .map(|c| MapSuggestion {
                    base: c.base.to_string(),
                    variant: c.id.to_string(),
                    name: c.name.clone(),
                })
                .ok_or_else(|| format!("no map variant {id} on this machine"))
        })
        .collect()
}
