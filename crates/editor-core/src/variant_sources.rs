//! Where the game-variant and playlist tools get their variants: the
//! setting schema from the source's `mainmenu` map (the built-in model
//! without one), save names from the XBE (`H2_XBE`, else `default.xbe`
//! beside the disc image), and the runtime's save storage under the data
//! root (docs/editor/variants.md).

use std::path::{Path, PathBuf};

use variants::save::{SaveStore, XbeNames};
use variants::{Schema, VariantEditor};

use crate::source::MapSource;

const LANGUAGE: tags::Language = tags::Language::English;

/// The save storage root: `U:` under the data root (`None`: the default
/// data root, as `h2 play` uses).
pub fn save_root(data_root: Option<&Path>) -> PathBuf {
    match data_root {
        Some(d) => SaveStore::root_under(d),
        None => SaveStore::root_under(&content::store::default_data_root()),
    }
}

/// The schema from the source's main menu map, else the fallback. The bool
/// says whether it came from the disc.
pub fn schema(source: Option<&MapSource>) -> (Schema, bool) {
    let from_disc = source
        .and_then(|s| s.open_map("mainmenu").ok())
        .and_then(|mut m| Schema::from_map(&mut m, LANGUAGE).ok());
    match from_disc {
        Some(s) => (s, true),
        None => (Schema::fallback(), false),
    }
}

/// The XBE to read save names from: `H2_XBE`, else `default.xbe` beside
/// the source.
pub fn xbe_path(source: Option<&MapSource>) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("H2_XBE") {
        return Some(PathBuf::from(p));
    }
    let beside = source?.path().parent()?.join("default.xbe");
    beside.is_file().then_some(beside)
}

/// The save names (per game type) from the XBE, when there is one.
pub fn names(source: Option<&MapSource>) -> Option<XbeNames> {
    let bytes = std::fs::read(xbe_path(source)?).ok()?;
    let xbe = xbe::Xbe::parse(&bytes).ok()?;
    XbeNames::read(&xbe, LANGUAGE).ok()
}

/// The save store under `data_root` with the source's save names.
pub fn store(source: Option<&MapSource>, data_root: Option<&Path>) -> SaveStore {
    SaveStore::new(save_root(data_root), names(source))
}

/// A variant editor for `source`, and what the user should know about how
/// it was built (no disc, no XBE), if anything.
pub fn editor(
    source: Option<&MapSource>,
    data_root: Option<&Path>,
) -> (VariantEditor, Option<&'static str>) {
    let (schema, from_disc) = schema(source);
    let store = store(source, data_root);
    let note = if !from_disc {
        Some("no disc: using the built-in setting model")
    } else if store.names.is_none() {
        Some("no XBE found (set H2_XBE): saved variants can be edited, but not created")
    } else {
        None
    };
    (VariantEditor::new(schema, Some(store)), note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_save_root_is_under_the_data_root() {
        assert_eq!(
            save_root(Some(Path::new("/d"))),
            SaveStore::root_under(Path::new("/d"))
        );
        assert_eq!(
            save_root(None),
            SaveStore::root_under(&content::store::default_data_root())
        );
    }

    #[test]
    fn without_a_source_the_editor_uses_the_built_in_model() {
        let dir = std::env::temp_dir().join(format!("editor-core-vs-{}", std::process::id()));
        let (e, note) = editor(None, Some(&dir));
        assert_eq!(note, Some("no disc: using the built-in setting model"));
        assert!(e.current().is_none() && !e.is_dirty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
