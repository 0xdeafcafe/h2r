//! The `tag` builder: a new tag from `<path>.<group>.yaml`
//! (docs/editor/tag-workspace.md, "Authoring and importing").
//!
//! The group is the file name's second extension, the tag's path the item's
//! `tag`. The file starts the tag from a tag already in the map (`like`) or
//! from the group's defaults, then changes fields by name:
//!
//! ```yaml
//! like: 'jpt!:globals\damage\sample'   # optional, same group
//! block:                               # as an edit file's, applied first
//!   - {path: player_responses, op: add}
//! fields:                              # path: value, in order
//!   damage_lower_bound: 12.5
//!   "player_responses[0].duration": 0.5
//! ```
//!
//! A tag reference names a tag by `<group>:<path>`, and the tag must be in
//! the map by then: an item may name tags that the base has or that earlier
//! items added. A tag path that is taken stops the build.
//!
//! `borrow` takes fields from a tag the map already has, chosen by what it
//! holds, for a value no package can name (the first-person view of the
//! map's own weapon of a class):
//!
//! ```yaml
//! borrow:
//!   - from: weap                  # the tag's own group
//!     where:                      # alternatives, tried in order
//!       - {weapon_class: pistol, weapon_name: hp}
//!       - {weapon_class: pistol}
//!     take: [first_person]        # copied over, each a non-empty block
//! ```
//!
//! The first alternative that some tag of the group matches (text fields
//! equal) and that holds every `take` field with elements gives its values,
//! the first such tag in the map's order. No match stops the build.
//!
//! With `replaces = "<asset hash>"` the item overrides whole the asset its
//! `tag` names (docs/engine/asset-store.md, "Mods and packages"): the asset
//! is in the map and hashes to it, else the item is a refusal
//! ([`crate::Cx::replace_target`]). `like` may name that same asset, so an
//! override is the asset with fields changed. The asset's hash as the item
//! leaves it is kept for a later package's `was` (`Report::replaced`).
use std::path::Path;

use map::FourCC;
use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};
use tags::defs::{Record, Str, StructDef, Val, default_record, from_built, parse_fourcc};

use crate::build::{Cx, Failure};
use crate::builders::Job;
use crate::edit::{
    BlockLine, change, check_block_line, def_of, field_of, parse_group_ref, text_of, write_back,
    yaml_failure,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TagFile {
    #[serde(default)]
    like: Option<String>,
    #[serde(default)]
    block: Vec<BlockLine>,
    #[serde(default)]
    fields: Mapping,
    #[serde(default)]
    borrow: Vec<Borrow>,
}

/// One `borrow` entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Borrow {
    from: String,
    #[serde(default, rename = "where")]
    matching: Vec<Mapping>,
    take: Vec<String>,
}

/// The group in `<path>.<group>.yaml`.
pub fn group_of_source(source: &str) -> Result<FourCC, String> {
    let name = Path::new(source)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(source);
    let stem = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))
        .ok_or_else(|| format!("{source:?} is not <path>.<group>.yaml"))?;
    let (_, group) = stem
        .rsplit_once('.')
        .ok_or_else(|| format!("{source:?} is not <path>.<group>.yaml"))?;
    parse_fourcc(group).ok_or_else(|| format!("{group:?} in {source:?} is not a tag group"))
}

fn parse(job: &Job<'_>) -> Result<(FourCC, String, TagFile), Failure> {
    let source = job.item.source.as_deref().unwrap_or("");
    let group = group_of_source(source)?;
    let path = job
        .item
        .tag
        .as_deref()
        .ok_or("a tag item needs `tag`, the path of the tag it adds")?;
    let file: TagFile = serde_yaml_ng::from_str(text_of(job.source)?).map_err(yaml_failure)?;
    Ok((group, path.replace('/', "\\"), file))
}

fn sets(file: &TagFile) -> Result<Vec<(&str, &Value)>, String> {
    file.fields
        .iter()
        .map(|(k, v)| match k {
            Value::String(s) => Ok((s.as_str(), v)),
            _ => Err("a field is named by its path, a string".to_owned()),
        })
        .collect()
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    let (group, _, file) = parse(job)?;
    let def = def_of(group)?;
    if let Some(like) = &file.like {
        let (g, _) = parse_group_ref(like)?;
        if g != group {
            return Err(format!("`like` is a {} tag, this tag is not", fourcc(g)).into());
        }
    }
    for b in &file.block {
        check_block_line(def, b)?;
    }
    for (path, _) in sets(&file)? {
        field_of(def, path)?;
    }
    for b in &file.borrow {
        check_borrow(def, group, b)?;
    }
    Ok(())
}

fn check_borrow(def: &StructDef, group: FourCC, b: &Borrow) -> Result<(), String> {
    let from =
        parse_fourcc(&b.from).ok_or_else(|| format!("borrow from {:?}: not a group", b.from))?;
    if from != group {
        return Err(format!(
            "borrow from {}: a tag takes fields from tags of its own group, {}",
            b.from,
            fourcc(group)
        ));
    }
    if b.take.is_empty() {
        return Err("a borrow takes at least one field".into());
    }
    for path in &b.take {
        field_of(def, path).map_err(|e| format!("borrow take: {e}"))?;
    }
    for alt in &b.matching {
        for (k, v) in alt {
            let (Value::String(k), Value::String(_)) = (k, v) else {
                return Err("a `where` entry is a field path and its text".into());
            };
            field_of(def, k).map_err(|e| format!("borrow where: {e}"))?;
        }
    }
    Ok(())
}

/// Whether the string field at `path` is `want`: its text, or, for a tag the
/// base map lays out anew (whose string IDs are the map's own words), the
/// text the map's string table gives the word.
fn text_is(rec: &Record, path: &str, want: &str, strings: &[Option<String>]) -> bool {
    match rec.at(path) {
        Some(Val::String(Str::Text(t))) => t == want,
        Some(Val::String(Str::Index(w))) => {
            tags::StringId(*w).resolve(strings).ok().flatten() == Some(want)
        }
        _ => false,
    }
}

/// Take `b`'s fields into `rec` from a tag of the map.
fn borrow(
    cx: &Cx<'_>,
    def: &StructDef,
    group: FourCC,
    rec: &mut Record,
    b: &Borrow,
) -> Result<(), String> {
    let mut unreadable = 0usize;
    let mut candidates = Vec::new();
    for slot in &cx.set.slots {
        let Some(t) = slot.tag.as_ref().filter(|t| t.group == group) else {
            continue;
        };
        match cx
            .read_block(t.id)
            .and_then(|blk| from_built(def, &blk).map_err(|e| e.to_string()))
        {
            Ok(r) => candidates.push(r),
            Err(_) => unreadable += 1,
        }
    }
    let alternatives: Vec<&Mapping> = if b.matching.is_empty() {
        vec![]
    } else {
        b.matching.iter().collect()
    };
    let empty = Mapping::new();
    let tries: Vec<&Mapping> = if alternatives.is_empty() {
        vec![&empty]
    } else {
        alternatives
    };
    for alt in tries {
        let pairs: Vec<(&str, &str)> = alt
            .iter()
            .filter_map(|(k, v)| Some((k.as_str()?, v.as_str()?)))
            .collect();
        let found = candidates.iter().find(|r| {
            pairs.iter().all(|(k, v)| text_is(r, k, v, &cx.set.strings))
                && b.take.iter().all(|p| match r.at(p) {
                    Some(Val::Block(e)) => !e.is_empty(),
                    Some(_) => true,
                    None => false,
                })
        });
        if let Some(from) = found {
            for p in &b.take {
                let v = from.at(p).cloned().expect("checked above");
                *rec.at_mut(p)
                    .ok_or_else(|| format!("borrow take {p:?}: no such element"))? = v;
            }
            return Ok(());
        }
    }
    Err(format!(
        "borrow from {}: no tag of the map matches ({} read, {unreadable} that could not be laid out)",
        b.from,
        candidates.len()
    ))
}

fn fourcc(g: FourCC) -> String {
    tags::defs::fourcc_text(g)
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let (group, path, file) = parse(job)?;
    let key = Cx::key(group, &path);
    // An override takes the asset at `key`; anything else adds a free path.
    let over = match job.item.replaces.as_deref() {
        Some(want) => Some(cx.replace_target(&key, want)?),
        None => {
            cx.check_free(&key)?;
            None
        }
    };
    let def = def_of(group)?;
    let mut rec = match &file.like {
        Some(like) => {
            let (g, p) = parse_group_ref(like)?;
            if g != group {
                return Err(format!("`like` is a {} tag, this tag is not", fourcc(g)).into());
            }
            let from = Cx::key(g, &p);
            let id = cx
                .find(&from)
                .ok_or_else(|| format!("`like` names {from}, which the map does not have"))?;
            let block = cx.read_block(id)?;
            from_built(def, &block).map_err(|e| format!("reading {from}: {e}"))?
        }
        None => default_record(def),
    };
    let sets = sets(&file)?;
    change(def, &mut rec, &file.block, &sets).map_err(|why| format!("tag {key}: {why}"))?;
    for b in &file.borrow {
        borrow(cx, def, group, &mut rec, b).map_err(|why| format!("tag {key}: {why}"))?;
    }
    let block = write_back(cx, def, &rec, &key)?;
    match over {
        Some(id) => {
            // The asset's hash as this package leaves it, for a later
            // patch's `was`: its normal form when the build has the base's
            // assets, else its own. (A record that does not encode is the
            // latter.)
            let normal = tags::defs::encode(def, key.clone(), &rec, tags::repo::Region::Metadata)
                .ok()
                .and_then(|tag| cx.asset_hash_of(&tag).ok());
            cx.write_block(id, &key, block)?;
            // Its own hash is the tag as written, so it is read after.
            let hash = normal.unwrap_or_else(|| {
                crate::tag_hash(cx.set, id).expect("the tag is in the map")
            });
            cx.note_replaced(&key, hash);
        }
        None => {
            cx.add_tag(&key, block)?;
        }
    }
    Ok(())
}
