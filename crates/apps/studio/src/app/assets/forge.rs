//! The **Forge items** kind (docs/editor/studio-flow.md, "Forge variants
//! and items"): the Forge palette as Browser rows. An item is a tag, so
//! its id is `tag:<group>:<path>` and **Where used** lists the map
//! variants that place it (`app::forge_variants::users`).
//!
//! | Source | Rows | `forge.source` |
//! |---|---|---|
//! | the tags tree | every tag of a group Forge places ([`MENUS`]) | `disc` |
//! | the packages that are on | the tags they add in those groups: the custom items | `package` |
//! | the kit's block-out set (`forge_shapes::starter`) | one `bloc` under `forge\blockout\` per definition | `blockout` |
//!
//! A custom item (`forge.custom`) is one the disc does not have: a
//! package's or a block-out definition.
use super::*;
use crate::app::forge_variants::{BLOCKOUT, ITEMS_KIND};

/// The groups Forge places, and the menu of the Objects tool each is
/// offered in (docs/game/forge-features.md, "Objects").
pub const MENUS: &[(&str, &str)] = &[
    ("weap", "Weapons"),
    ("vehi", "Vehicles"),
    ("eqip", "Equipment"),
    ("scen", "Scenery"),
    ("bloc", "Crates"),
    ("mach", "Machines"),
    ("ctrl", "Controls"),
    ("itmc", "Item spawners"),
    ("vehc", "Item spawners"),
];

/// The menu a group's items are offered in, when Forge places the group.
pub fn menu_of(group: &str) -> Option<&'static str> {
    MENUS
        .iter()
        .find(|(g, _)| *g == group.trim())
        .map(|(_, menu)| *menu)
}

/// The rows of the kit's block-out set: a definition is a crate the kit
/// makes, at `forge\blockout\<name>`.
fn blockout_rows() -> Vec<Value> {
    forge_shapes::starter()
        .iter()
        .map(|def| {
            let name = def.name();
            let path = format!("{BLOCKOUT}{name}");
            json!({
                "asset": format!("tag:bloc:{path}"),
                // Not a tag of the tree: the row lists and answers Where
                // used, and has no editor to open.
                "root": "forge",
                "kind": ITEMS_KIND,
                "group": "bloc",
                "path": path,
                "name": name,
                "origin": "forge",
                "new": false,
                "badge": "Forge block-out",
                "packages": [],
                "maps": [],
                "variants": [],
                "used": Value::Null,
                "forge": { "menu": "Block-out", "custom": true, "source": "blockout" },
            })
        })
        .collect()
}

/// `asset.list {kind: "Forge items"}`: the Forge palette, the custom
/// items included. A missing tags tree is named in `problems` and the
/// custom items are still listed. `map` keeps what one map holds, and
/// every word of the query must match the path, the group, the menu, the
/// package or `custom`.
pub fn forge_items(tags_root: &Path, set: &GameSet, q: &Query) -> Result<Value, String> {
    let words: Vec<String> = q
        .text
        .split_whitespace()
        .map(|w| w.to_lowercase().replace('/', "\\"))
        .collect();
    let o = overlay(set);
    let mut problems = o.problems.clone();
    let (catalogue, problem) = current_catalogue(tags_root);
    problems.extend(problem);
    let all = match tree(tags_root) {
        Ok(all) => all,
        Err(e) => {
            problems.push(e);
            BTreeMap::new()
        }
    };
    let maps = TagRepo::open(tags_root)
        .and_then(|r| r.maps())
        .unwrap_or_default();
    let in_map = match &q.map {
        Some(m) => Some((m.clone(), map_tags(tags_root, m)?)),
        None => None,
    };
    let mut keys: BTreeSet<&TagKey> = all.keys().collect();
    keys.extend(o.added.keys());
    let mut rows: Vec<Value> = Vec::new();
    for key in keys {
        let group = fourcc_text(key.group);
        let Some(menu) = menu_of(&group) else {
            continue;
        };
        let versions = all.get(key);
        if let Some((name, (tags, scope))) = &in_map {
            let there = tags.contains(key)
                || (versions.is_none()
                    && o.added
                        .get(key)
                        .is_some_and(|l| l.iter().any(|(_, m)| applies(m, name, Some(*scope)))));
            if !there {
                continue;
            }
        }
        let custom = versions.is_none();
        let mut row = tag_row(key, versions, &o, catalogue.as_deref());
        row["kind"] = json!(ITEMS_KIND);
        row["forge"] = json!({
            "menu": menu,
            "custom": custom,
            "source": if custom { "package" } else { "disc" },
        });
        let hay = format!(
            "{} {} {} {} {}",
            key.path,
            group,
            menu,
            row["packages"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
            if custom { "custom" } else { "" }
        )
        .to_lowercase();
        if words_match(&words, &hay) {
            rows.push(row);
        }
    }
    if in_map.is_none() {
        for row in blockout_rows() {
            let hay = format!("{} bloc block-out blockout custom", row["path"].as_str().unwrap_or(""))
                .to_lowercase();
            if words_match(&words, &hay) {
                rows.push(row);
            }
        }
    }
    let total = rows.len();
    let custom = rows.iter().filter(|r| r["forge"]["custom"] == true).count();
    rows.truncate(q.limit);
    Ok(json!({
        "root": "tags",
        "rows": rows,
        "total": total,
        "custom": custom,
        "limit": q.limit,
        "truncated": total > q.limit,
        "maps": maps,
        "kinds": kind_names(),
        "problems": problems,
    }))
}

impl App {
    /// The rows of a kind the tree's own list does not answer: the
    /// **Voices**, the **Forge variants** and the **Forge items**. `None`
    /// for any other kind and any other root.
    pub fn kind_list(&self, q: &Query) -> Option<Result<Value, String>> {
        if q.root != Root::Tags {
            return None;
        }
        let kind = q.kind.as_deref()?.trim();
        if kind.eq_ignore_ascii_case(VOICES_KIND) {
            Some(Ok(self.voice_asset_list(&q.text, q.limit)))
        } else if kind.eq_ignore_ascii_case(crate::app::forge_variants::VARIANTS_KIND) {
            Some(self.forge_variant_list(&q.text, q.limit))
        } else if kind.eq_ignore_ascii_case(ITEMS_KIND) {
            let set = read_set(&self.data_root());
            Some(forge_items(&self.tags_root(), &set, q))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_places_the_object_groups_and_the_spawner_collections() {
        assert_eq!(menu_of("weap"), Some("Weapons"));
        assert_eq!(menu_of("bloc"), Some("Crates"));
        assert_eq!(menu_of("vehc"), Some("Item spawners"));
        assert_eq!(menu_of("snd!"), None);
        assert_eq!(menu_of("bitm"), None);
    }

    #[test]
    fn the_block_out_set_lists_as_custom_crates_under_the_forge_folder() {
        let rows = blockout_rows();
        assert_eq!(rows.len(), forge_shapes::starter().len());
        for row in &rows {
            assert!(
                row["asset"].as_str().unwrap().starts_with("tag:bloc:forge\\blockout\\"),
                "{row}"
            );
            assert_eq!(row["forge"]["custom"], true);
            assert_eq!(row["kind"], ITEMS_KIND);
        }
    }
}
