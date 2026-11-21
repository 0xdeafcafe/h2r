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
            None => {
                let Some(scale) = def.legacy_scale() else {
                    return Ok(None);
                };
                let Some((legacy_table, key)) = scale.key.split_once('.') else {
                    return Ok(None);
                };
                let Some(v) = table.get(legacy_table).and_then(|t| t.get(key)) else {
                    return Ok(None);
                };
                let n = v
                    .as_integer()
                    .ok_or_else(|| format!("{} must be a whole number", scale.key))?;
                scale
                    .parse(&n.to_string())
                    .map(|r| Some(r.value()))
                    .map_err(|e| format!("{}: {e}", scale.key))
            }
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

/// The table per-profile values live under: `[profiles."<name>".<table>]`.
pub const PROFILES: &str = "profiles";

/// Every profile's values of `defs` in `table`. A value that is not valid
/// for its setting, or a profile entry that is not a table, is reported
/// through `report` and left out; keys no setting has are ignored (a later
/// version's settings survive an older run, as the file keeps them).
pub(crate) fn profile_values(
    table: &toml::Table,
    defs: &[&'static Def],
    report: &mut dyn FnMut(String),
) -> std::collections::BTreeMap<String, std::collections::BTreeMap<&'static str, Value>> {
    let mut out = std::collections::BTreeMap::new();
    let Some(profiles) = table.get(PROFILES) else {
        return out;
    };
    let Some(profiles) = profiles.as_table() else {
        report(format!("{PROFILES} must be a table; ignored"));
        return out;
    };
    for (name, t) in profiles {
        let Some(t) = t.as_table() else {
            report(format!("{PROFILES}.{name:?} must be a table; ignored"));
            continue;
        };
        let mut values = std::collections::BTreeMap::new();
        for def in defs {
            match lookup(t, def) {
                Ok(Some(v)) => {
                    values.insert(def.key, v);
                }
                Ok(None) => {}
                Err(e) => report(format!("{PROFILES}.{name:?}: {e}; ignored")),
            }
        }
        if !values.is_empty() {
            out.insert(name.clone(), values);
        }
    }
    out
}

/// Writes `profile`'s `def = v` under [`PROFILES`], keeping every other key
/// in the file, as [`write_value`] does.
pub fn write_profile_value(path: &Path, profile: &str, def: &Def, v: Value) -> Result<(), String> {
    let v = def.check(v)?;
    let mut table = match read_table(path) {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => return Err(format!("{e}; not overwritten")),
    };
    let (t, k) = def.table_key();
    let mut at = &mut table;
    for name in [PROFILES, profile, t] {
        let next = at
            .entry(name)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let toml::Value::Table(next) = next else {
            return Err(format!(
                "{}: {name} must be a table; not overwritten",
                path.display()
            ));
        };
        at = next;
    }
    at.insert(k.into(), def.to_toml(v));
    let text = toml::to_string(&table).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}
