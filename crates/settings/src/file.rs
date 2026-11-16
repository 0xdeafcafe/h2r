//! `settings.toml`: the one reader and the one writer.

use std::path::{Path, PathBuf};

use crate::def::{Def, Value};

/// The host settings file, under `h2 play`'s data root (`--data`), next to
/// the guest's `disk/` tree but not in it.
pub const SETTINGS_FILE: &str = "settings.toml";

/// The settings file under the data root `data`: `<data>/settings.toml`.
pub fn path_in(data: &Path) -> PathBuf {
    data.join(SETTINGS_FILE)
}

/// The file's table: `None` when there is no file. A file that cannot be
/// read or is not TOML is an error naming it.
pub(crate) fn read_table(path: &Path) -> Result<Option<toml::Table>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    text.parse()
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `def`'s value in `table`: `Ok(None)` when absent.
pub(crate) fn lookup(table: &toml::Table, def: &Def) -> Result<Option<Value>, String> {
    let (t, k) = def.table_key();
    match table.get(t) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => match t.get(k) {
            None => Ok(None),
            Some(v) => def
                .from_toml(v)
                .map(Some)
                .map_err(|e| format!("{} {e}", def.key)),
        },
        Some(_) => Err(format!("{t} must be a table")),
    }
}

/// Writes `def = v`, keeping every other key in the file. Atomic: a
/// temporary file next to it, then a rename. A file that cannot be read,
/// is not TOML, or whose table is not a table is not overwritten.
pub fn write_value(path: &Path, def: &Def, v: Value) -> Result<(), String> {
    let v = def.check(v)?;
    let mut table = match read_table(path) {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => return Err(format!("{e}; not overwritten")),
    };
    let (t, k) = def.table_key();
    let entry = table
        .entry(t)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(entry) = entry else {
        return Err(format!(
            "{}: {t} must be a table; not overwritten",
            path.display()
        ));
    };
    entry.insert(k.into(), def.to_toml(v));
    let text = toml::to_string(&table).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}
