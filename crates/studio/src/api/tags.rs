//! `tag.*`: the open map's tags. Listing, reading and following references
//! are built here over the map's index and the layout schema
//! ([`tags::schema`]: blocks, references, string IDs; no names). Field
//! names and edits for every group are the studio-tags lane's registry,
//! which plugs in as a [`super::Provider`] ([`super::seams`]); until then
//! `tag.set` and `tag.block.*` edit the scenario tag's placement blocks
//! through the scenario's own commands and answer NOT_YET for the rest.

use std::collections::HashMap;

use agent_proto::RpcError;
use map::{FourCC, Map, Tag, TagId};
use scenario_edit::save::MapReader;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::schema::{Field, Schema, Struct};

use super::params::{block_kind, block_name};
use super::scenario::{group_text, parse_tag_id, tag_id};
use super::{P, Reply, err};
use crate::App;

/// The map open in the scenario tool, read from its source (edits are not
/// in it: the scenario's show through `scenario.*`).
pub fn open_map(app: &App) -> Result<Map<MapReader>, RpcError> {
    let src = app.session.source().ok_or_else(|| {
        err(
            code::NO_DOCUMENT,
            "no map is open: source.open a map, or doc.open {\"map\": ...}",
        )
    })?;
    src.open()
        .map_err(|e| err(code::FAILED, format!("reading the map: {e}")))
}

fn tag_json(t: &Tag) -> Value {
    json!({
        "id": tag_id(t.id),
        "group": group_text(t.group),
        "name": t.name,
        "size": t.location.map(|l| l.size),
        "address": t.location.map(|l| format!("{:#010x}", l.address)),
    })
}

/// A `tag` param: `0x...` (or a number), a full name, `name.group`, or a
/// group alone when the map has one tag of it (`scnr`, `matg`).
pub fn resolve<'m>(map: &'m Map<MapReader>, want: &Value) -> Result<&'m Tag, RpcError> {
    if let Some(id) = parse_tag_id(want) {
        return map
            .tags()
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("no tag {}", tag_id(id))));
    }
    let s = want.as_str().ok_or_else(|| {
        err(
            code::INVALID_PARAMS,
            format!("tag: an id or a name, not {want}"),
        )
    })?;
    let by_name: Vec<&Tag> = map
        .tags()
        .iter()
        .filter(|t| {
            t.name.as_deref().is_some_and(|n| {
                n.eq_ignore_ascii_case(s)
                    || format!("{n}.{}", group_text(t.group)).eq_ignore_ascii_case(s)
            })
        })
        .collect();
    let found: Vec<&Tag> = if by_name.is_empty() {
        map.tags()
            .iter()
            .filter(|t| group_text(t.group).eq_ignore_ascii_case(s))
            .collect()
    } else {
        by_name
    };
    match found[..] {
        [one] => Ok(one),
        [] => Err(err(
            code::INVALID_PARAMS,
            format!("no tag named {s:?} (tag.list finds them)"),
        )),
        _ => Err(err(
            code::INVALID_PARAMS,
            format!(
                "{s:?} names {} tags; use an id: {}",
                found.len(),
                found
                    .iter()
                    .take(8)
                    .map(|t| format!("{} {}", tag_id(t.id), group_text(t.group)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// What a structural read needs about the map.
struct Ctx<'a> {
    names: HashMap<TagId, (FourCC, Option<String>)>,
    strings: &'a [Option<String>],
    /// The tag's bytes and its address.
    bytes: &'a [u8],
    base: u32,
    elements: usize,
    hex: bool,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
}

impl Ctx<'_> {
    fn tag(&self, id: TagId) -> Value {
        match self.names.get(&id) {
            Some((g, n)) => json!({ "id": tag_id(id), "group": group_text(*g), "name": n }),
            None if id == TagId::NULL => Value::Null,
            None => json!({ "id": tag_id(id), "missing": true }),
        }
    }

    /// The element of `s` at byte `at` of the tag, `depth` levels of
    /// blocks deep.
    fn element(&self, s: &Struct, at: usize, depth: u32) -> Value {
        let mut fields = Vec::new();
        for (&o, f) in &s.fields {
            let p = at + o as usize;
            let off = format!("{o:#x}");
            let w0 = u32_at(self.bytes, p).unwrap_or(0);
            let w1 = u32_at(self.bytes, p + 4).unwrap_or(0);
            fields.push(match f {
                Field::Block(child) => {
                    let mut v = json!({
                        "offset": off, "kind": "block", "count": w0,
                        "address": format!("{w1:#010x}"), "stride": child.size,
                    });
                    if depth > 0 && w0 > 0 {
                        let start = w1.wrapping_sub(self.base) as usize;
                        let n = (w0 as usize).min(self.elements);
                        let fits = w1 >= self.base
                            && start + child.size as usize * w0 as usize <= self.bytes.len();
                        if fits {
                            v["elements"] = Value::Array(
                                (0..n)
                                    .map(|i| self.element(child, start + i * child.size as usize, depth - 1))
                                    .collect(),
                            );
                        } else {
                            v["outside_tag"] = json!(true);
                        }
                    }
                    v
                }
                Field::TagRef => {
                    json!({ "offset": off, "kind": "tag_ref", "group": group_text(FourCC::from_u32(w0)), "tag": self.tag(TagId(w1)) })
                }
                Field::TagId => json!({ "offset": off, "kind": "tag_id", "tag": self.tag(TagId(w0)) }),
                Field::StringId => {
                    let s = self.strings.get((w0 & 0x00ff_ffff) as usize).cloned().flatten();
                    json!({ "offset": off, "kind": "string_id", "value": format!("{w0:#010x}"), "string": s })
                }
                Field::Raw => json!({ "offset": off, "kind": "raw", "value": format!("{w0:#010x}") }),
                Field::RawSize(_) => json!({ "offset": off, "kind": "raw_size", "value": w0 }),
                Field::Word => json!({ "offset": off, "kind": "word", "value": format!("{w0:#010x}") }),
                Field::Bsp => json!({ "offset": off, "kind": "bsp" }),
            });
        }
        let mut v = json!({ "fields": fields });
        if self.hex
            && let Some(b) = self.bytes.get(at..at + s.size as usize)
        {
            v["hex"] = json!(super::docs::hex(b));
        }
        v
    }

    /// Every tag reference under the element at `at`, with its path.
    fn refs(
        &self,
        s: &Struct,
        at: usize,
        path: &str,
        out: &mut Vec<Value>,
        limit: usize,
        depth: u32,
    ) {
        if depth > 32 {
            return;
        }
        for (&o, f) in &s.fields {
            if out.len() >= limit {
                return;
            }
            let p = at + o as usize;
            let here = if path.is_empty() {
                format!("{o:#x}")
            } else {
                format!("{path}.{o:#x}")
            };
            let w0 = u32_at(self.bytes, p).unwrap_or(0);
            let w1 = u32_at(self.bytes, p + 4).unwrap_or(0);
            match f {
                Field::TagRef | Field::TagId => {
                    let id = TagId(if matches!(f, Field::TagRef) { w1 } else { w0 });
                    if id != TagId::NULL && id.0 != 0 {
                        let mut v = self.tag(id);
                        v["path"] = json!(here);
                        out.push(v);
                    }
                }
                Field::Block(child) if w0 > 0 && w1 >= self.base => {
                    let start = (w1 - self.base) as usize;
                    if start + child.size as usize * w0 as usize > self.bytes.len() {
                        continue;
                    }
                    for i in 0..w0 as usize {
                        self.refs(
                            child,
                            start + i * child.size as usize,
                            &format!("{here}[{i}]"),
                            out,
                            limit,
                            depth + 1,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Reads a tag and calls `f` with its structural reader and root layout.
fn with_tag<T>(
    app: &App,
    p: &P,
    elements: usize,
    hex: bool,
    f: impl FnOnce(&Tag, Option<(&Ctx, &Struct)>) -> T,
) -> Result<T, RpcError> {
    let mut map = open_map(app)?;
    let want = p.get("tag").cloned().unwrap_or(Value::Null);
    let tag = resolve(&map, &want)?.clone();
    let names = map
        .tags()
        .iter()
        .map(|t| (t.id, (t.group, t.name.clone())))
        .collect();
    let strings = map.strings().to_vec();
    let bytes = match tag.location {
        Some(l) if l.size > 0 => map
            .read_tag_bytes(tag.id)
            .map_err(|e| err(code::FAILED, format!("reading {}: {e}", tag_id(tag.id))))?,
        _ => Vec::new(),
    };
    let root = Schema::builtin().groups.get(&tag.group);
    let ctx = Ctx {
        names,
        strings: &strings,
        bytes: &bytes,
        base: tag.location.map_or(0, |l| l.address),
        elements,
        hex,
    };
    let usable = root.filter(|r| !bytes.is_empty() && r.size as usize <= bytes.len());
    Ok(f(&tag, usable.map(|r| (&ctx, r))))
}

fn refs_of(app: &App, p: &P) -> Result<(Tag, Vec<Value>), RpcError> {
    let limit = p.opt_usize("limit")?.unwrap_or(500);
    with_tag(app, p, usize::MAX, false, |tag, r| {
        let mut out = Vec::new();
        if let Some((ctx, root)) = r {
            ctx.refs(root, 0, "", &mut out, limit, 0);
        }
        (tag.clone(), out)
    })
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::TAG_LIST => {
            let map = open_map(app)?;
            let group = p.opt_str("group")?.map(str::to_ascii_lowercase);
            let name = p.opt_str("name")?.map(str::to_ascii_lowercase);
            let limit = p.opt_usize("limit")?.unwrap_or(200);
            let hits: Vec<&Tag> = map
                .tags()
                .iter()
                .filter(|t| {
                    group
                        .as_deref()
                        .is_none_or(|g| group_text(t.group).eq_ignore_ascii_case(g))
                })
                .filter(|t| {
                    name.as_deref().is_none_or(|n| {
                        t.name
                            .as_deref()
                            .is_some_and(|x| x.to_ascii_lowercase().contains(n))
                    })
                })
                .collect();
            Ok(json!({
                "total": hits.len(),
                "tags": hits.iter().take(limit).map(|t| tag_json(t)).collect::<Vec<_>>(),
            }))
        }
        m::TAG_READ => {
            let depth = p.opt_u64("depth")?.unwrap_or(1) as u32;
            let elements = p.opt_usize("elements")?.unwrap_or(8);
            let hex = p.bool("hex")?;
            let scenario_tag = app.session.scenario().map(|s| s.tag);
            with_tag(app, p, elements, hex, |tag, r| {
                let mut v = tag_json(tag);
                match r {
                    Some((ctx, root)) => {
                        v["layout"] = json!("schema");
                        v["root"] = ctx.element(root, 0, depth);
                    }
                    None => {
                        v["layout"] = Value::Null;
                        v["note"] = json!(
                            "no layout for this group in the schema (or the tag has no data here)"
                        );
                    }
                }
                if Some(tag.id) == scenario_tag {
                    v["placements"] = json!(
                        "named and edited through scenario.* (and tag.set with <block>[<i>].<field>)"
                    );
                }
                v["names"] = json!(
                    "field names land with the studio-tags registry; until then fields are labelled by offset"
                );
                v
            })
        }
        m::TAG_REFS => Ok(Value::Array(refs_of(app, p)?.1)),
        m::TAG_FOLLOW => {
            let (_, refs) = refs_of(app, p)?;
            let want = p.get("ref").cloned().unwrap_or(Value::Null);
            let hit = match &want {
                Value::Number(n) => n.as_u64().and_then(|i| refs.get(i as usize)),
                Value::String(s) => refs.iter().find(|r| r["path"] == *s),
                _ => None,
            }
            .ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("no reference {want} (tag.refs lists them)"),
                )
            })?;
            if hit.get("missing").is_some() {
                return Err(err(
                    code::FAILED,
                    format!(
                        "the reference names {}, which is not in this map",
                        hit["id"]
                    ),
                ));
            }
            let map = open_map(app)?;
            let target = resolve(&map, &hit["id"])?;
            Ok(tag_json(target))
        }
        m::TAG_SET | m::TAG_BLOCK_ADD | m::TAG_BLOCK_REMOVE | m::TAG_BLOCK_DUPLICATE => {
            edit(app, method, p)
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

/// `tag.set` and `tag.block.*` on the scenario tag's placement blocks, as
/// the matching `scenario.*` calls; NOT_YET for everything else.
fn edit(app: &mut App, method: &str, p: &P) -> Reply {
    let map = open_map(app)?;
    let tag = resolve(&map, p.get("tag").unwrap_or(&Value::Null))?;
    let is_scenario = app.session.scenario().is_some_and(|s| s.tag == tag.id);
    let path = p.str("path")?;
    if !is_scenario {
        let mut e = err(
            code::NOT_YET,
            format!(
                "{method} on a {} tag needs the studio-tags field registry, which is not in yet; the scenario's placements edit now",
                group_text(tag.group)
            ),
        );
        e.data = Some(json!({ "lane": "studio-tags", "method": method }));
        return Err(e);
    }
    let not_placement = || {
        err(
            code::NOT_YET,
            format!(
                "{path}: only the placement blocks of the scenario edit here yet (<block>[<i>].<field>, scenario.blocks lists them)"
            ),
        )
    };
    // `<block>[<i>].<field>` or `<block>`.
    let (block, rest) = match path.split_once('[') {
        Some((b, r)) => (b, Some(r)),
        None => (path, None),
    };
    let kind = block_kind(block).map_err(|_| not_placement())?;
    let index = rest
        .map(|r| {
            let (i, tail) = r.split_once(']').ok_or_else(not_placement)?;
            let i: usize = i.parse().map_err(|_| not_placement())?;
            Ok::<_, RpcError>((i, tail.strip_prefix('.').filter(|f| !f.is_empty())))
        })
        .transpose()?;
    let mut q = json!({ "block": block_name(kind) });
    match (method, index) {
        (m::TAG_SET, Some((i, Some(field)))) => {
            q["index"] = json!(i);
            q["field"] = json!(field);
            q["value"] = p.get("value").cloned().unwrap_or(Value::Null);
            super::scenario::call(app, m::SCENARIO_SET, &P(&q))
        }
        (m::TAG_BLOCK_ADD, None) => {
            let like = p.opt_usize("like")?;
            super::scenario::add(app, kind, like, None)
        }
        (m::TAG_BLOCK_REMOVE | m::TAG_BLOCK_DUPLICATE, None) => {
            q["index"] = json!(p.usize("index")?);
            let to = if method == m::TAG_BLOCK_REMOVE {
                m::SCENARIO_DELETE
            } else {
                m::SCENARIO_DUPLICATE
            };
            super::scenario::call(app, to, &P(&q))
        }
        _ => Err(err(
            code::INVALID_PARAMS,
            format!(
                "{method}: path {path:?} should be {}",
                if method == m::TAG_SET {
                    "<block>[<i>].<field>"
                } else {
                    "a block name"
                }
            ),
        )),
    }
}
