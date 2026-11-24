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
use std::path::Path;

use map::FourCC;
use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};
use tags::defs::{default_record, from_built, parse_fourcc};

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
    Ok(())
}

fn fourcc(g: FourCC) -> String {
    tags::defs::fourcc_text(g)
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let (group, path, file) = parse(job)?;
    let key = Cx::key(group, &path);
    cx.check_free(&key)?;
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
    let block = write_back(cx, def, &rec, &key)?;
    cx.add_tag(&key, block)?;
    Ok(())
}
