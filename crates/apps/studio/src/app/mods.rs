//! Mods in Studio (docs/engine/asset-store.md, "In Studio"; slice AS7):
//! what the packages do to one asset, in load order, and the calls that
//! change it.
//!
//! - [`changed_by`]: every package with a patch on a tag, in load order,
//!   with each field's value and whether it is the one that wins. The
//!   inspector's **Changed by** row and `asset.links` read it.
//! - [`fork`] and [`revert`] (`asset.fork`, `asset.revert`): a fork is the
//!   tag's edit lines in an `edits` item whose `maps` names one map
//!   (`edits/<map>.yaml` of the package). It is never a copy of the tag;
//!   every other level is as it was. A revert takes the lines out, and the
//!   item with them when no other tag has a line in its file.
//! - [`diff`] (`package.diff`): per asset, the fields the package sets
//!   (with the value a line's `from` names as before), the assets it adds,
//!   overrides whole or patches, which of them are forks, and the levels
//!   touched. With a map and its file, the set is built over that map
//!   (`workspace::build_map_settled`) and the conflict table's rows come
//!   with it ([`Conflict`]): the Problems list shows the last ones read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ::workspace::{
    GameSet, Loaded, SetLine, TagEdit, Target, added_tag, build_map_settled, changes_map,
    edits_text, group_of_source, parse_edits,
};
use content::package::{Item, Kind, MapScope, Maps};
use serde_json::{Value, json};
use tags::defs::fourcc_text;
use tags::repo::TagKey;

use super::App;
use super::assets::{AssetId, group_ref, tag_id};
use super::package_doc::{self as doc, BaseMap, Refusal};

type Res<T = Value> = Result<T, Refusal>;

/// `<group>:<path>`, the form an edit line names a tag by.
fn tag_text(key: &TagKey) -> String {
    format!("{}:{}", fourcc_text(key.group), key.path)
}

/// A `maps` value in words: the scope, or the names.
pub fn maps_word(maps: &Maps) -> String {
    match maps {
        Maps::Scope(s) => s.word().to_owned(),
        Maps::Names(n) => n.join(", "),
    }
}

/// Whether an item with `maps` reaches `map`, of class `scope` when that
/// is known. With no map every item does; with a map of unknown class a
/// scope word is taken to reach it.
pub fn applies(maps: &Maps, map: Option<&str>, scope: Option<MapScope>) -> bool {
    let Some(map) = map else {
        return true;
    };
    match maps {
        Maps::Scope(MapScope::All) => true,
        Maps::Scope(s) => scope.is_none_or(|x| x == *s),
        Maps::Names(n) => n.iter().any(|n| n.eq_ignore_ascii_case(map)),
    }
}

/// The entries of an `edits` item's file.
fn edits_of(loaded: &Loaded, item: &Item) -> Result<Vec<TagEdit>, String> {
    let source = item
        .source
        .as_deref()
        .ok_or_else(|| "an edits item has no source".to_owned())?;
    let bytes = loaded
        .files
        .get(source)
        .ok_or_else(|| format!("{source}: not in the package"))?;
    let text = String::from_utf8_lossy(bytes);
    parse_edits(&text)
        .map(|list| list.into_iter().map(|(e, _)| e).collect())
        .map_err(|f| format!("{source}: {}", f.why))
}

/// The tag a `tag-delta` item patches: its `tag`, in the group its source
/// names.
fn delta_key(item: &Item) -> Option<TagKey> {
    let path = item.tag.as_deref()?.replace('/', "\\");
    let group = group_of_source(item.source.as_deref()?).ok()?;
    Some(TagKey::new(group, path))
}

// ---- Changed by ----

/// One field a package sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    pub path: String,
    /// The value the line says it replaces, when it names one.
    pub from: Option<Value>,
    pub value: Value,
    /// Whether this is the value the field ends with: no later package sets
    /// it and no later package overrides the asset whole.
    pub wins: bool,
}

/// One package's patch on an asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedBy {
    pub package: String,
    /// The item's place in the package's manifest.
    pub item: usize,
    /// `edit` (set and block lines), `patch` (a `tag-delta`) or `replaces`
    /// (an authored override of the whole asset).
    pub change: &'static str,
    /// The item's `maps`, in words.
    pub maps: String,
    /// The item names maps, not a class: it is a fork.
    pub forked: bool,
    /// The `block` lines of the entry.
    pub blocks: usize,
    pub fields: Vec<FieldChange>,
}

impl ChangedBy {
    pub fn json(&self) -> Value {
        json!({
            "package": self.package,
            "item": self.item,
            "asset": format!("item:{}:{}", self.package, self.item),
            "change": self.change,
            "maps": self.maps,
            "forked": self.forked,
            "blocks": self.blocks,
            "fields": self.fields.iter().map(|f| {
                let mut v = json!({"path": f.path, "value": f.value, "wins": f.wins});
                if let Some(from) = &f.from {
                    v["from"] = from.clone();
                }
                v
            }).collect::<Vec<_>>(),
        })
    }
}

/// Mark which field values win, over `rows` in load order.
fn settle(rows: &mut [ChangedBy]) {
    let whole = rows.iter().rposition(|r| r.change == "replaces");
    let mut last: BTreeMap<String, usize> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        for f in &r.fields {
            last.insert(f.path.clone(), i);
        }
    }
    for (i, r) in rows.iter_mut().enumerate() {
        let overridden = whole.is_some_and(|w| i < w);
        for f in &mut r.fields {
            f.wins = !overridden && last.get(&f.path) == Some(&i);
        }
    }
}

/// The packages on that patch or override `key`, in load order, for `map`
/// (of class `scope` when known) or for every map. An item that adds the
/// tag is not a patch: `asset.links` names it under `used_by`.
pub fn changed_by(
    set: &GameSet,
    key: &TagKey,
    map: Option<&str>,
    scope: Option<MapScope>,
) -> Vec<ChangedBy> {
    let mut out = Vec::new();
    for entry in &set.on {
        let manifest = &entry.loaded.package.manifest;
        for (i, item) in manifest.items.iter().enumerate() {
            if !applies(&item.maps, map, scope) {
                continue;
            }
            let row = |change: &'static str, blocks: usize, fields: Vec<FieldChange>| ChangedBy {
                package: manifest.package.id.clone(),
                item: i,
                change,
                maps: maps_word(&item.maps),
                forked: matches!(item.maps, Maps::Names(_)),
                blocks,
                fields,
            };
            match item.kind {
                Kind::Edits => {
                    let Ok(list) = edits_of(&entry.loaded, item) else {
                        continue;
                    };
                    for edit in list
                        .into_iter()
                        .filter(|e| group_ref(&e.tag).as_ref() == Some(key))
                    {
                        let fields = edit
                            .set
                            .iter()
                            .map(|l| FieldChange {
                                path: l.path.clone(),
                                from: l
                                    .from
                                    .as_ref()
                                    .map(|v| serde_json::to_value(v).unwrap_or(Value::Null)),
                                value: serde_json::to_value(&l.value).unwrap_or(Value::Null),
                                wins: true,
                            })
                            .collect();
                        out.push(row("edit", edit.block.len(), fields));
                    }
                }
                Kind::TagDelta if delta_key(item).as_ref() == Some(key) => {
                    out.push(row("patch", 0, Vec::new()));
                }
                Kind::Tag if item.replaces.is_some() && added_tag(item).as_ref() == Some(key) => {
                    out.push(row("replaces", 0, Vec::new()));
                }
                _ => {}
            }
        }
    }
    settle(&mut out);
    out
}

// ---- fork and revert ----

fn check_map_name(map: &str) -> Res<()> {
    if map.is_empty()
        || !map
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Refusal::Params(format!(
            "map {map:?} cannot name a fork: letters, digits, '_' and '-'"
        )));
    }
    Ok(())
}

/// A tag as `asset.fork` and `asset.revert` take it: `<group>:<path>` or
/// `tag:<group>:<path>`.
pub fn parse_tag(text: &str) -> Res<TagKey> {
    let id = if text.starts_with("tag:") {
        text.to_owned()
    } else {
        format!("tag:{text}")
    };
    match AssetId::parse(&id) {
        Ok(AssetId::Tag(key)) => Ok(key),
        Ok(_) => Err(Refusal::Params(format!("{text:?} is not a tag"))),
        Err(e) => Err(Refusal::Params(e)),
    }
}

/// `asset.fork`: in `package` (a local one), the lines of `key` as they
/// are for `map`, in `edits/<map>.yaml`, an `edits` item that names that
/// map alone. With `set`, those are the lines. Without, they are the
/// lines of the package's other `edits` items that reach `map` (the last
/// one to set a path wins), and a package with none is refused. A tag
/// already forked for the map is refused without `set`; with it, the
/// lines are put in by path.
pub fn fork(data: &Path, package: &str, key: &TagKey, map: &str, set: Option<Vec<SetLine>>) -> Res {
    check_map_name(map)?;
    let (loaded, _) = doc::loaded_local(data, package)?;
    let own = doc::edits_file(map);
    let has_own = loaded
        .package
        .manifest
        .items
        .iter()
        .any(|i| i.kind == Kind::Edits && i.source.as_deref() == Some(own.as_str()));
    let mut mine: Vec<TagEdit> = if has_own {
        let item = loaded
            .package
            .manifest
            .items
            .iter()
            .find(|i| i.kind == Kind::Edits && i.source.as_deref() == Some(own.as_str()))
            .ok_or_else(|| Refusal::Failed(format!("{own} has no item")))?;
        edits_of(&loaded, item).map_err(Refusal::Refused)?
    } else {
        Vec::new()
    };
    let at = mine
        .iter()
        .position(|e| group_ref(&e.tag).as_ref() == Some(key));
    let mut from_items: Vec<usize> = Vec::new();
    let (block, sets) = match set {
        Some(lines) => {
            if lines.is_empty() {
                return Err(Refusal::Params(
                    "set is a list of {path, value} lines: it is empty".into(),
                ));
            }
            (Vec::new(), lines)
        }
        None => {
            if at.is_some() {
                return Err(Refusal::Refused(format!(
                    "{} is forked for {map} in {package} already: asset.revert takes the fork out, or pass `set` to change its lines",
                    tag_id(key)
                )));
            }
            let mut block = Vec::new();
            let mut sets: Vec<SetLine> = Vec::new();
            for (i, item) in loaded.package.manifest.items.iter().enumerate() {
                if item.kind != Kind::Edits
                    || item.source.as_deref() == Some(own.as_str())
                    || !applies(&item.maps, Some(map), None)
                {
                    continue;
                }
                let list = edits_of(&loaded, item).map_err(Refusal::Refused)?;
                for edit in list
                    .into_iter()
                    .filter(|e| group_ref(&e.tag).as_ref() == Some(key))
                {
                    from_items.push(i);
                    block.extend(edit.block.iter().cloned());
                    for line in edit.set {
                        sets.retain(|x| x.path != line.path);
                        sets.push(line);
                    }
                }
            }
            if sets.is_empty() && block.is_empty() {
                return Err(Refusal::Refused(format!(
                    "{package} has no edit line for {} that reaches {map}: pass `set` with the lines to fork",
                    tag_id(key)
                )));
            }
            (block, sets)
        }
    };
    let lines = sets.len() + block.len();
    match at {
        Some(n) => {
            let entry = &mut mine[n];
            for line in sets {
                entry.set.retain(|x| x.path != line.path);
                entry.set.push(line);
            }
        }
        None => mine.push(TagEdit {
            tag: tag_text(key),
            was: None,
            block,
            set: sets,
        }),
    }
    mine.sort_by(|a, b| a.tag.cmp(&b.tag));
    let text = edits_text(&mine).map_err(|e| Refusal::Failed(format!("edit lines: {e}")))?;
    doc::put_edits(data, package, map, &text)?;
    let (after, _) = doc::loaded_local(data, package)?;
    let item = after
        .package
        .manifest
        .items
        .iter()
        .position(|i| i.kind == Kind::Edits && i.source.as_deref() == Some(own.as_str()));
    Ok(json!({
        "package": package,
        "tag": tag_id(key),
        "map": map,
        "file": own,
        "item": item,
        "lines": lines,
        "from_items": from_items,
        "created": !has_own,
    }))
}

/// `asset.revert`: take the lines of `key` out of `package`'s `edits`
/// items. With `map`, only the item that names that map alone (the fork);
/// without, every `edits` item with an entry for the tag. An item left with
/// no entry goes with its file. A tag with no line is refused.
pub fn revert(data: &Path, package: &str, key: &TagKey, map: Option<&str>) -> Res {
    if let Some(m) = map {
        check_map_name(m)?;
    }
    let (loaded, _) = doc::loaded_local(data, package)?;
    let mut removed: Vec<Value> = Vec::new();
    for (i, item) in loaded.package.manifest.items.iter().enumerate() {
        if item.kind != Kind::Edits {
            continue;
        }
        let Some(source) = item.source.clone() else {
            continue;
        };
        if let Some(m) = map {
            let own = matches!(&item.maps, Maps::Names(n) if n.len() == 1 && n[0].eq_ignore_ascii_case(m));
            if !own {
                continue;
            }
        }
        let list = edits_of(&loaded, item).map_err(Refusal::Refused)?;
        let (gone, kept): (Vec<TagEdit>, Vec<TagEdit>) = list
            .into_iter()
            .partition(|e| group_ref(&e.tag).as_ref() == Some(key));
        if gone.is_empty() {
            continue;
        }
        let text = edits_text(&kept).map_err(|e| Refusal::Failed(format!("edit lines: {e}")))?;
        doc::put_edits_at(data, package, &source, item.maps.clone(), &text)?;
        removed.push(json!({
            "item": i,
            "file": source,
            "maps": maps_word(&item.maps),
            "lines": gone.iter().map(|e| e.set.len() + e.block.len()).sum::<usize>(),
            "item_removed": kept.is_empty(),
        }));
    }
    if removed.is_empty() {
        return Err(Refusal::Refused(match map {
            Some(m) => format!("{package} has no fork of {} for {m}", tag_id(key)),
            None => format!("{package} has no edit line for {}", tag_id(key)),
        }));
    }
    Ok(json!({
        "package": package,
        "tag": tag_id(key),
        "map": map,
        "removed": removed,
    }))
}

// ---- package.diff ----

/// One row of the conflict table for a level, as a build over the set
/// read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub map: String,
    /// `refused` (the package is off for the level) or `override` (a later
    /// package's value for a field, or a whole asset, wins).
    pub kind: &'static str,
    /// The package that is off, or the later one.
    pub package: String,
    /// The earlier package of an override.
    pub other: Option<String>,
    /// `<group>:<path>` of an override's asset.
    pub tag: Option<String>,
    pub path: Option<String>,
    /// The line the table gives.
    pub why: String,
}

impl Conflict {
    pub fn json(&self, id: &str) -> Value {
        let mut v = json!({
            "map": self.map,
            "kind": self.kind,
            "package": self.package,
            "why": self.why,
            "involves": self.package == id || self.other.as_deref() == Some(id),
        });
        for (k, s) in [
            ("other", &self.other),
            ("tag", &self.tag),
            ("path", &self.path),
        ] {
            if let Some(s) = s {
                v[k] = json!(s);
            }
        }
        v
    }
}

/// The conflict table's rows for `map` from a build's report.
fn conflicts_of(map: &str, report: &::workspace::Report) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = report
        .refused
        .iter()
        .map(|r| Conflict {
            map: map.to_owned(),
            kind: "refused",
            package: r.package.clone(),
            other: None,
            tag: None,
            path: None,
            why: r.why.clone(),
        })
        .collect();
    out.extend(report.overrides.iter().map(|o| Conflict {
        map: map.to_owned(),
        kind: "override",
        package: o.later.clone(),
        other: Some(o.earlier.clone()),
        tag: Some(tag_text(&o.tag)),
        path: Some(o.path.clone()),
        why: o.to_string(),
    }));
    out
}

/// `package.diff`: what package `id` does, per asset, for `map` or every
/// map. Rows are `{asset, item, change, maps, forked, ...}`; `change` is
/// `edit` (with `fields` `{path, from?, to}` and `blocks`), `patch` (a
/// `tag-delta`, with `was`), `override` (a `replaces`, with its hash) or
/// `added`. `levels` are the maps the package changes: with the source's
/// `bases` read for their class, else the maps its items name. With `map`
/// and its base, the whole set is built over that map and `conflicts` holds
/// the table's rows, `involves` marking the ones that name this package;
/// they are also returned for the Problems list.
pub fn diff(
    data: &Path,
    id: &str,
    bases: &[BaseMap],
    map: Option<&str>,
) -> Res<(Value, Vec<Conflict>)> {
    let loaded = doc::loaded(data, id)?;
    let manifest = &loaded.package.manifest;
    let base = map.and_then(|m| bases.iter().find(|b| b.name.eq_ignore_ascii_case(m)));
    let scope = base.and_then(|b| doc::scope_of_base(b).ok());
    let mut problems: Vec<String> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    for (i, item) in manifest.items.iter().enumerate() {
        if !applies(&item.maps, map, scope) {
            continue;
        }
        let common = |asset: String, change: &str| {
            json!({
                "asset": asset,
                "item": i,
                "item_asset": format!("item:{id}:{i}"),
                "change": change,
                "maps": maps_word(&item.maps),
                "forked": false,
            })
        };
        match item.kind {
            Kind::Edits => match edits_of(&loaded, item) {
                Ok(list) => {
                    for edit in list {
                        let asset = group_ref(&edit.tag)
                            .map(|k| tag_id(&k))
                            .unwrap_or_else(|| format!("tag:{}", edit.tag));
                        let mut r = common(asset, "edit");
                        r["forked"] = json!(matches!(item.maps, Maps::Names(_)));
                        r["fields"] = json!(
                            edit.set
                                .iter()
                                .map(|l| {
                                    let mut f = json!({
                                        "path": l.path,
                                        "to": serde_json::to_value(&l.value)
                                            .unwrap_or(Value::Null),
                                    });
                                    if let Some(from) = &l.from {
                                        f["from"] =
                                            serde_json::to_value(from).unwrap_or(Value::Null);
                                    }
                                    f
                                })
                                .collect::<Vec<_>>()
                        );
                        r["blocks"] = json!(
                            edit.block
                                .iter()
                                .map(|b| json!({"path": b.path, "op": format!("{:?}", b.op).to_lowercase()}))
                                .collect::<Vec<_>>()
                        );
                        if let Some(was) = &edit.was {
                            r["was"] = json!(was);
                        }
                        rows.push(r);
                    }
                }
                Err(e) => problems.push(format!("item {i}: {e}")),
            },
            Kind::TagDelta => match delta_key(item) {
                Some(key) => {
                    let mut r = common(tag_id(&key), "patch");
                    r["forked"] = json!(matches!(item.maps, Maps::Names(_)));
                    if let Some(was) = &item.was {
                        r["was"] = json!(was);
                    }
                    rows.push(r);
                }
                None => problems.push(format!("item {i}: a tag-delta item names no tag")),
            },
            Kind::Tag | Kind::Bitmap | Kind::Model | Kind::Sound => {
                if let Some(key) = added_tag(item) {
                    let mut r = match &item.replaces {
                        Some(hash) => {
                            let mut r = common(tag_id(&key), "override");
                            r["replaces"] = json!(hash);
                            r
                        }
                        None => common(tag_id(&key), "added"),
                    };
                    r["forked"] =
                        json!(item.replaces.is_some() && matches!(item.maps, Maps::Names(_)));
                    rows.push(r);
                }
            }
            _ => {}
        }
    }
    let count = |c: &str| rows.iter().filter(|r| r["change"] == c).count();
    let counts = json!({
        "edit": count("edit"),
        "patch": count("patch"),
        "override": count("override"),
        "added": count("added"),
        "forked": rows.iter().filter(|r| r["forked"] == true).count(),
    });
    let levels: Vec<String> = if bases.is_empty() {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for item in manifest.items.iter().filter(|i| changes_map(i)) {
            if let Maps::Names(n) = &item.maps {
                names.extend(n.iter().cloned());
            }
        }
        names.into_iter().collect()
    } else {
        bases
            .iter()
            .filter(|b| {
                doc::scope_of_base(b).is_ok_and(|scope| {
                    let target = Target {
                        name: &b.name,
                        scope,
                    };
                    manifest
                        .items
                        .iter()
                        .any(|i| changes_map(i) && target.takes(&i.maps))
                })
            })
            .map(|b| b.name.clone())
            .collect()
    };
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut built = false;
    if let (Some(base), Some(scope)) = (base, scope) {
        match doc::in_order(data) {
            Ok(packages) => {
                let bytes = scenario_edit::MapSource::region(&base.path, base.region)
                    .read_base()
                    .map_err(|e| format!("{}: {e}", base.path.display()));
                let target = Target {
                    name: &base.name,
                    scope,
                };
                match bytes
                    .and_then(|b| build_map_settled(&b, &packages, &target, &BTreeSet::new(), None))
                {
                    Ok(b) => {
                        built = true;
                        conflicts = conflicts_of(&base.name, &b.report);
                    }
                    Err(e) => problems.push(format!("building {}: {e}", base.name)),
                }
            }
            Err(e) => problems.push(e.to_string()),
        }
    }
    let v = json!({
        "id": id,
        "map": map,
        "assets": rows,
        "counts": counts,
        "levels": levels,
        "built": built,
        "conflicts": conflicts.iter().map(|c| c.json(id)).collect::<Vec<_>>(),
        "problems": problems,
    });
    Ok((v, conflicts))
}

// ---- the window ----

impl App {
    /// [`super::Action::ForkAsset`]: **Fork for this map** on the open
    /// map, into the edit target.
    pub(super) fn fork_asset(&mut self, asset: &str) {
        let data = self.data_root();
        let Some(package) = doc::target(&data) else {
            return self.error(super::edit_target::NO_TARGET);
        };
        let Some(map) = self.session.map_name().map(str::to_owned) else {
            return self.error("no map is open to fork the asset for");
        };
        let key = match parse_tag(asset) {
            Ok(k) => k,
            Err(e) => return self.error(e.to_string()),
        };
        match fork(&data, &package, &key, &map, None) {
            Ok(v) => self.info(format!(
                "{} is forked for {map} in {package} ({} lines)",
                asset,
                v["lines"].as_u64().unwrap_or(0)
            )),
            Err(e) => self.error(e.to_string()),
        }
    }

    /// [`super::Action::RevertAsset`]: take the open map's fork of the
    /// asset out of the edit target.
    pub(super) fn revert_asset(&mut self, asset: &str) {
        let data = self.data_root();
        let Some(package) = doc::target(&data) else {
            return self.error(super::edit_target::NO_TARGET);
        };
        let map = self.session.map_name().map(str::to_owned);
        let key = match parse_tag(asset) {
            Ok(k) => k,
            Err(e) => return self.error(e.to_string()),
        };
        match revert(&data, &package, &key, map.as_deref()) {
            Ok(_) => self.info(format!(
                "{asset} is back to what the level had in {package}"
            )),
            Err(e) => self.error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(package: &str, change: &'static str, fields: &[(&str, i64)]) -> ChangedBy {
        ChangedBy {
            package: package.to_owned(),
            item: 0,
            change,
            maps: "all".to_owned(),
            forked: false,
            blocks: 0,
            fields: fields
                .iter()
                .map(|(path, v)| FieldChange {
                    path: (*path).to_owned(),
                    from: None,
                    value: json!(v),
                    wins: true,
                })
                .collect(),
        }
    }

    fn wins(rows: &[ChangedBy]) -> Vec<Vec<bool>> {
        rows.iter()
            .map(|r| r.fields.iter().map(|f| f.wins).collect())
            .collect()
    }

    #[test]
    fn the_later_package_wins_a_field_and_both_win_different_ones() {
        let mut rows = vec![
            row("a", "edit", &[("x", 1), ("y", 2)]),
            row("b", "edit", &[("x", 3)]),
        ];
        settle(&mut rows);
        assert_eq!(wins(&rows), vec![vec![false, true], vec![true]]);
    }

    #[test]
    fn a_whole_override_takes_every_field_an_earlier_package_set() {
        let mut rows = vec![
            row("a", "edit", &[("x", 1)]),
            row("b", "replaces", &[]),
            row("c", "edit", &[("x", 4)]),
        ];
        settle(&mut rows);
        assert_eq!(wins(&rows), vec![vec![false], vec![], vec![true]]);
    }

    #[test]
    fn an_item_reaches_a_map_by_its_name_or_its_class() {
        let all = Maps::Scope(MapScope::All);
        let mp = Maps::Scope(MapScope::Multiplayer);
        let names = Maps::Names(vec!["Lockout".to_owned()]);
        assert!(applies(&mp, None, None));
        assert!(applies(&all, Some("lockout"), None));
        assert!(
            applies(&mp, Some("lockout"), None),
            "unknown class: reaches"
        );
        assert!(applies(&mp, Some("lockout"), Some(MapScope::Multiplayer)));
        assert!(!applies(&mp, Some("lockout"), Some(MapScope::Campaign)));
        assert!(applies(&names, Some("lockout"), Some(MapScope::Campaign)));
        assert!(!applies(&names, Some("zanzibar"), None));
    }

    #[test]
    fn a_tag_names_parse_with_or_without_the_asset_prefix() {
        let a = parse_tag("weap:objects/weapons/rifle").unwrap();
        let b = parse_tag("tag:weap:objects\\weapons\\rifle").unwrap();
        assert_eq!(a, b);
        assert!(matches!(parse_tag("rifle"), Err(Refusal::Params(_))));
        assert!(matches!(parse_tag("package:x"), Err(Refusal::Params(_))));
    }
}
