//! Structural tag reads, the scenario tree and reference navigation.
use std::collections::HashMap;

use agent_proto::RpcError;
use map::{FourCC, Map, Tag, TagId};
use scenario_edit::save::MapReader;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::schema::{Field, Schema, Struct};

use super::params::{block_kind, block_name};
use super::scenario::{group_text, parse_tag_id, tag_id};
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::{
    TagDoc, tag_api,
    tag_tree::{self, Row, ScenarioRoots},
};

const TREE: MethodDef = def(
    m::TAG_TREE,
    "The open scenario's tag tree and the tags it cannot reach",
    r#"{"root?":"string (scenario)","depth?":"integer (references below palettes, default 3, max 8)"}"#,
    r#"{"root":"{label,tag,group,name,placed,via,cycle,children:[row]}","unreachable":"[row]","incomplete":"boolean","truncated":"boolean"}"#,
    "{}",
);

const REFS: MethodDef = def(
    m::TAG_REFS,
    "Direct links from a tag's fields or an aligned reference scan",
    r#"{"tag":"string|integer","limit?":"integer (default 500)"}"#,
    r#"[{"to":"string","via":"string","found":"field|scan","path":"string","id":"string","group":"string|null","name":"string|null","missing?":"boolean"}]"#,
    r#"{"tag":"scnr"}"#,
);

const KINDS: MethodDef = def(
    m::TAG_KINDS,
    "The kind registry: each tag group's editor, preview, subject and the classes its edits take",
    r#"{"group?":"string (one group, as tag.list writes it)"}"#,
    r#"{"kinds":"[{label,groups,editor,preview,subject,classes,missing}]","default":"{label,editor,preview,subject,classes,missing}","groups":"[{group,kind,default,editor,preview,subject,classes}]","target":"off|own|attached|attached_edit"}"#,
    r#"{"group":"weap"}"#,
);

pub(super) struct TagGraph;

impl Provider for TagGraph {
    fn methods(&self) -> Vec<MethodDef> {
        vec![TREE, REFS, KINDS]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        match method {
            m::TAG_TREE => Some(tree(app, &p)),
            m::TAG_REFS => Some(refs(app, &p)),
            m::TAG_KINDS => Some(kinds_list(app, params)),
            _ => None,
        }
    }
}

/// `tag.kinds`: the rows, the default, and every group the registry or
/// the open map has with the row it gets; with `group`, that group's line
/// alone (an unknown group gets the default).
fn kinds_list(app: &App, params: &Value) -> Reply {
    use crate::app::kinds::{self, DEFAULT, KINDS};
    use crate::app::live::Target;
    if let Some(want) = params.get("group").filter(|v| !v.is_null()) {
        let text = want
            .as_str()
            .filter(|t| !t.is_empty() && t.len() <= 4)
            .ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    "group: up to four characters, as tag.list writes them",
                )
            })?;
        let mut bytes = *b"    ";
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        return Ok(kinds::group_json(FourCC(bytes)));
    }
    let mut groups: std::collections::BTreeSet<[u8; 4]> =
        app.tags.registry().groups.keys().map(|g| g.0).collect();
    groups.extend(app.tags.tags().iter().map(|t| t.group.0));
    let target = match app.live.target() {
        Target::Off => "off",
        Target::Own => "own",
        Target::Attached { edit: false } => "attached",
        Target::Attached { edit: true } => "attached_edit",
    };
    Ok(json!({
        "kinds": KINDS.iter().map(|k| k.json()).collect::<Vec<_>>(),
        "default": DEFAULT.json(),
        "groups": groups
            .into_iter()
            .map(|g| kinds::group_json(FourCC(g)))
            .collect::<Vec<_>>(),
        "target": target,
    }))
}

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
}

/// Reads a tag and calls `f` with its structural reader and root layout.
fn with_tag<T>(
    app: &App,
    want: impl Into<Value>,
    elements: usize,
    hex: bool,
    f: impl FnOnce(&Tag, Option<(&Ctx, &Struct)>) -> T,
) -> Result<T, RpcError> {
    let _scope = mem_watch::scope("assets.tag_load");
    let mut map = open_map(app)?;
    let tag = resolve(&map, &want.into())?.clone();
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

fn row_json(doc: &TagDoc, row: &Row) -> Value {
    let entry = row.tag.and_then(|id| doc.tag(id));
    json!({
        "label": row.label,
        "tag": row.tag.map(tag_id),
        "group": entry.map(|t| group_text(t.group)),
        "name": entry.map(|t| &t.name),
        "placed": row.placements.as_ref().map(Vec::len),
        "via": row.via,
        "cycle": row.cycle,
        "edited": entry.is_some_and(|t| doc.is_changed(t.id)),
        "cache": entry.and_then(|t| t.elsewhere.as_deref()),
        "children": row.children.iter().map(|child| row_json(doc, child)).collect::<Vec<_>>(),
    })
}

fn tree(app: &mut App, p: &P) -> Reply {
    if p.opt_str("root")?.is_some_and(|r| r != "scenario") {
        return Err(err(code::INVALID_PARAMS, "root: expected scenario"));
    }
    let depth = p.opt_u64("depth")?.unwrap_or(tag_api::DEFAULT_DEPTH as u64);
    if depth > tag_api::MAX_DEPTH as u64 {
        return Err(err(
            code::INVALID_PARAMS,
            format!("depth: at most {}", tag_api::MAX_DEPTH),
        ));
    }
    super::tag_fields::sync_open_tags(app)?;
    let scenario = super::scenario::open(app)?;
    let doc = &app.tags;
    let roots = ScenarioRoots::read(&mut open_map(app)?);
    let tree = tag_tree::scenario_tree(doc, scenario, &roots, depth as usize);
    let mut root = row_json(doc, &tree.root);
    root["edited"] = json!(scenario.is_modified() || root["edited"] == true);
    Ok(json!({
        "root": root,
        "unreachable": tree.unreachable.iter().map(|&i| {
            row_json(doc, &Row { tag: Some(doc.tags()[i].id), ..Row::default() })
        }).collect::<Vec<_>>(),
        "incomplete": tree.incomplete,
        "truncated": tree.truncated,
    }))
}

fn refs_of(app: &mut App, p: &P, limit: usize) -> Result<Vec<Value>, RpcError> {
    super::tag_fields::sync_open_tags(app)?;
    let doc = &app.tags;
    let id = tag_api::lookup(doc, p.get("tag").unwrap_or(&Value::Null))
        .map_err(|e| err(code::INVALID_PARAMS, e))?;
    let refs = doc.tag_refs(id, app.session.scenario());
    Ok(refs
        .links
        .into_iter()
        .take(limit)
        .map(|link| {
            let to = tag_id(link.tag);
            let target = doc.tag(link.tag);
            let mut out = json!({
                "to": to,
                "via": link.via,
                "found": link.found,
                "path": link.path,
                "id": to,
                "group": target.map(|t| group_text(t.group)),
                "name": target.map(|t| &t.name),
            });
            if target.is_none() {
                out["missing"] = json!(true);
            }
            out
        })
        .collect())
}

fn refs(app: &mut App, p: &P) -> Reply {
    let limit = p.opt_usize("limit")?.unwrap_or(500);
    Ok(Value::Array(refs_of(app, p, limit)?))
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
            let want = p.get("tag").cloned().unwrap_or(Value::Null);
            with_tag(app, want, elements, hex, |tag, r| {
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
        m::TAG_FOLLOW => {
            let refs = refs_of(app, p, 500)?;
            let want = p.get("ref").cloned().unwrap_or(Value::Null);
            let hit = match &want {
                Value::Number(n) => n.as_u64().and_then(|i| refs.get(i as usize)),
                Value::String(s) => refs.iter().find(|r| r["path"] == *s || r["via"] == *s),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Api;
    use crate::app::{Action, NavAction, Selection};
    use map::{
        CacheType,
        build::{Block, BspField, FixupKind, TagPayload, TagSet},
    };
    use scenario_edit::{BlockKind, MapSource};

    struct Fixture {
        app: App,
        weapon: TagId,
        sky: TagId,
        bsp: TagId,
        bsp_asset: TagId,
        lightmap: TagId,
        globals: TagId,
        scan: TagId,
        child: TagId,
        ui_only: TagId,
        globals_child: TagId,
        other: TagId,
        old_item: TagId,
        new_item: TagId,
    }

    fn add(set: &mut TagSet, group: &[u8; 4], name: &str, block: Block) -> TagId {
        set.add_tag(FourCC(*group), name, TagPayload::Built(block))
            .unwrap()
    }

    fn fixture() -> Fixture {
        fixture_with_other_rows(0)
    }

    fn fixture_with_other_rows(rows: u32) -> Fixture {
        let mut set = TagSet::new(CacheType::Multiplayer, "tree", "test\\tree");
        set.salt_base = 0;
        for group in [
            b"scnr", b"matg", b"sky ", b"sbsp", b"ltmp", b"weap", b"xref", b"base", b"wgit",
            b"more", b"itmc",
        ] {
            set.add_group(FourCC(*group), None, None);
        }
        set.add_group(FourCC(*b"chil"), Some(FourCC(*b"base")), None);

        let sky = add(&mut set, b"sky ", "test\\sky", Block::zeroed(8));
        let ui_only = add(&mut set, b"more", "test\\menu_content", Block::zeroed(8));
        let mut ui = Block::zeroed(8);
        ui.tag_ref(0, ui_only);
        add(&mut set, b"wgit", "test\\menu", ui);

        let child = add(&mut set, b"chil", "test\\child", Block::zeroed(8));
        let mut other = Block::zeroed(8);
        if rows != 0 {
            let mut entries = Block::zeroed(rows as usize * 8);
            entries.tag_ref((rows - 1) * 8, ui_only);
            other.block(0, rows, entries);
        }
        let other = add(&mut set, b"more", "test\\other", other);
        let mut bytes = Block::zeroed(0x38);
        bytes
            .tag_ref(0, child)
            .put_u32(8, FourCC(*b"base").as_u32())
            .put_u32(12, child.0)
            .put_u32(16, FourCC(*b"weap").as_u32())
            .put_u32(20, child.0)
            .put_u32(24, FourCC(*b"chil").as_u32())
            .put_u32(28, child.0 ^ 0x0001_0000)
            .put_u32(0x2c, FourCC(*b"chil").as_u32())
            .tag_ref(0x30, sky);
        bytes.bytes[0x21..0x25].copy_from_slice(&FourCC(*b"chil").as_u32().to_le_bytes());
        bytes.bytes[0x25..0x29].copy_from_slice(&child.0.to_le_bytes());
        let scan = add(&mut set, b"xref", "test\\scan", bytes);
        let mut back = Block::zeroed(8);
        back.tag_ref(0, scan);
        set.tag_mut(child).unwrap().payload = TagPayload::Built(back);

        let mut weapon = Block::zeroed(8);
        weapon.tag_ref(0, scan);
        let weapon = add(&mut set, b"weap", "test\\blaster", weapon);
        let bsp_asset = add(&mut set, b"more", "test\\bsp_asset", Block::zeroed(8));
        let mut material = Block::zeroed(0x88);
        material.tag_ref(0x20, bsp_asset);
        let mut bsp = Block::zeroed(0x23c);
        bsp.block(0x84, 1, material);
        let bsp = add(&mut set, b"sbsp", "test\\room", bsp);
        let lightmap = add(&mut set, b"ltmp", "test\\lightmap", Block::zeroed(0x104));
        let old_item = add(&mut set, b"itmc", "test\\old_item", Block::zeroed(8));
        let new_item = add(&mut set, b"itmc", "test\\new_item", Block::zeroed(8));
        let globals_child = add(&mut set, b"more", "test\\global_content", Block::zeroed(8));
        let mut global_data = Block::zeroed(8);
        global_data.tag_ref(0, globals_child);
        let globals = add(&mut set, b"matg", "test\\globals", global_data);
        add(&mut set, b"matg", "test\\other_globals", Block::zeroed(8));

        let section = set.add_bsp(bsp, Some(lightmap));
        let mut descriptor = Block::zeroed(0x44);
        for (at, field) in [
            (0, BspField::FileOffset),
            (4, BspField::Size),
            (8, BspField::Address),
        ] {
            descriptor.fixup(at, FixupKind::Bsp { section, field });
        }
        descriptor.tag_ref(0x10, bsp).tag_ref(0x18, lightmap);

        let def = scenario_edit::layout::block(BlockKind::Weapons);
        let palette = def.palette.as_ref().unwrap();
        let mut entries = Block::zeroed(3 * palette.stride as usize);
        entries
            .tag_ref(0, weapon)
            .tag_ref(palette.stride, weapon)
            .tag_ref(palette.stride * 2, weapon);
        let mut placed = Block::zeroed(4 * def.stride as usize);
        for (i, slot) in [0, 1, 0, u16::MAX].into_iter().enumerate() {
            placed.put_u16(i * def.stride as usize, slot);
        }
        let mut skies = Block::zeroed(8);
        skies.tag_ref(0, sky);

        let equipment = scenario_edit::layout::block(BlockKind::NetgameEquipment);
        let mut item = Block::zeroed(equipment.stride as usize);
        item.tag_ref(equipment.field("item").unwrap().offset, old_item);

        let mut scenario = Block::zeroed(0x3e0);
        scenario
            .block(0x8, 1, skies)
            .block(def.offset, 4, placed)
            .block(equipment.offset, 1, item)
            .block(palette.offset, 3, entries)
            .block(0x210, 1, descriptor)
            .tag_ref(0x3d0, other);
        set.scenario = add(&mut set, b"scnr", "test\\tree", scenario);
        set.globals = globals;

        let mut app = App::new();
        app.session.build_scenes = false;
        static REGISTRY: std::sync::OnceLock<tags::registry::Registry> = std::sync::OnceLock::new();
        app.tags = TagDoc::new(REGISTRY.get_or_init(|| {
            let schema = Schema::parse(concat!(
                "group weap 0x8 align 4\n  0x0 tagref\n",
                "group more 0x8 align 4\n  0x0 block 0x8 align 4\n    0x0 tagref\n",
                "group sbsp 0x23c align 4\n  0x84 block 0x88 align 4\n    0x20 tagref\n",
                "group ltmp 0x104 align 4\n",
                "group scnr 0x3e0 align 4\n  0x120 block 0x90 align 4\n    0x58 tagref\n",
            ))
            .unwrap();
            tags::registry::Registry::build(&schema, &tags::defs::Defs::default())
        }));
        let (bytes, _) = set.build().unwrap();
        let mut source = MapSource::file(
            std::env::temp_dir()
                .join("h2-invented-tag-tree")
                .join("tree.map"),
        );
        source.bytes = Some(bytes.into());
        app.session.open(source).unwrap();
        app.sync_tags();
        Fixture {
            app,
            weapon,
            sky,
            bsp,
            bsp_asset,
            lightmap,
            globals,
            scan,
            child,
            ui_only,
            globals_child,
            other,
            old_item,
            new_item,
        }
    }

    fn section<'a>(row: &'a Value, label: &str) -> &'a Value {
        row["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["label"] == label)
            .unwrap_or_else(|| panic!("no {label} in {row}"))
    }

    #[test]
    fn scenario_tree_api_names_the_roots_and_uses_full_reachability() {
        let mut f = fixture();
        let mut api = Api::new();
        let call = |api: &mut Api, app: &mut App, depth| {
            api.call(app, m::TAG_TREE, json!({"root":"scenario", "depth": depth}))
                .unwrap()
        };
        let tree = call(&mut api, &mut f.app, 3);
        let root = &tree["root"];
        assert_eq!(root["group"], "scnr");
        assert_eq!(tree["incomplete"], false);
        let bsp = section(section(root, "Structure"), "BSP 0");
        assert_eq!(bsp["tag"], tag_id(f.bsp));
        assert_eq!(section(bsp, "Lightmap")["tag"], tag_id(f.lightmap));
        assert_eq!(section(root, "Skies")["children"][0]["tag"], tag_id(f.sky));
        assert_eq!(section(root, "Globals")["tag"], tag_id(f.globals));
        assert_eq!(
            section(root, "Other references (2)")["children"][0]["tag"],
            tag_id(f.other)
        );

        let palette = &section(section(root, "Objects"), "Weapons (3)")["children"];
        for (slot, count) in [2, 1, 0].into_iter().enumerate() {
            assert_eq!(palette[slot]["tag"], tag_id(f.weapon));
            assert_eq!(palette[slot]["placed"], count);
        }
        let chain = &palette[0]["children"][0];
        assert_eq!(chain["tag"], tag_id(f.scan));
        assert_eq!(chain["children"][0]["tag"], tag_id(f.child));
        assert_eq!(chain["children"][0]["children"][0]["cycle"], true);

        let menu_refs = api
            .call(&mut f.app, m::TAG_REFS, json!({"tag":"wgit"}))
            .unwrap();
        assert_eq!(menu_refs[0]["to"], tag_id(f.ui_only));
        let unreachable = tree["unreachable"].as_array().unwrap();
        assert!(
            unreachable
                .iter()
                .any(|row| row["tag"] == tag_id(f.ui_only))
        );
        assert!(
            unreachable
                .iter()
                .any(|row| row["name"] == "test\\other_globals")
        );
        assert!(unreachable.iter().all(|row| row["tag"] != tag_id(f.child)));
        assert!(
            unreachable
                .iter()
                .all(|row| row["tag"] != tag_id(f.globals_child))
        );
        let shallow = call(&mut api, &mut f.app, 0);
        assert_eq!(shallow["unreachable"], tree["unreachable"]);
        assert_eq!(
            section(section(&shallow["root"], "Objects"), "Weapons (3)")["children"][0]["children"],
            json!([]),
        );
        assert_eq!(
            api.call(&mut f.app, m::TAG_TREE, json!({"root":"globals"}))
                .unwrap_err()
                .code,
            code::INVALID_PARAMS
        );
        assert_eq!(
            api.call(&mut f.app, m::TAG_TREE, json!({"depth":9}))
                .unwrap_err()
                .code,
            code::INVALID_PARAMS
        );
        assert_eq!(
            api.call(&mut App::new(), m::TAG_TREE, json!({}))
                .unwrap_err()
                .code,
            code::NO_DOCUMENT
        );
    }

    #[test]
    fn scenario_tree_scan_checks_full_datum_group_and_alignment() {
        let mut f = fixture();
        let doc = &f.app.tags;
        assert!(doc.layout(f.scan).is_none());
        let refs = doc.scan_refs(f.scan);
        assert_eq!(
            refs.iter()
                .map(|r| (r.tag, r.via.as_str()))
                .collect::<Vec<_>>(),
            [(f.child, "+0x0"), (f.child, "+0x8"), (f.sky, "+0x30")]
        );

        let mut api = Api::new();
        let refs = api
            .call(&mut f.app, m::TAG_REFS, json!({"tag": f.scan.0}))
            .unwrap();
        assert_eq!(refs.as_array().unwrap().len(), 3);
        assert_eq!(refs[1]["to"], tag_id(f.child));
        assert_eq!(refs[1]["found"], "scan");
        assert_eq!(refs[1]["via"], "+0x8");
        let target = api
            .call(
                &mut f.app,
                m::TAG_FOLLOW,
                json!({
                    "tag": tag_id(f.scan), "ref": refs[1]["path"],
                }),
            )
            .unwrap();
        assert_eq!(target["id"], tag_id(f.child));
        let refs = api
            .call(&mut f.app, m::TAG_REFS, json!({"tag": "weap"}))
            .unwrap();
        assert_eq!(refs[0]["to"], tag_id(f.scan));
        assert_eq!(refs[0]["found"], "field");
        assert_eq!(refs[0]["path"], "0x0");
    }

    fn unreachable(tree: &Value, id: TagId) -> bool {
        tree["unreachable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["tag"] == tag_id(id))
    }

    #[test]
    fn scenario_tree_and_tag_refs_follow_bsp_region_links() {
        let mut f = fixture();
        let mut api = Api::new();
        let refs = api
            .call(&mut f.app, m::TAG_REFS, json!({"tag": f.bsp.0}))
            .unwrap();
        assert_eq!(refs[0]["to"], tag_id(f.bsp_asset));
        assert_eq!(refs[0]["path"], "0x84[0].0x20");
        let tree = api.call(&mut f.app, m::TAG_TREE, json!({})).unwrap();
        assert_eq!(tree["incomplete"], false);
        assert!(!unreachable(&tree, f.bsp_asset));
        assert!(unreachable(&tree, f.ui_only));
    }

    #[test]
    fn scenario_tree_omits_unreachable_tags_when_the_walk_exhausts_its_budget() {
        // The final row's link is just outside the 100,000-field walk budget.
        let mut f = fixture_with_other_rows(100_000);
        let tree = Api::new()
            .call(&mut f.app, m::TAG_TREE, json!({"depth": 0}))
            .unwrap();
        assert_eq!(tree["incomplete"], true);
        assert_eq!(tree["unreachable"], json!([]));
        assert_eq!(tree["truncated"], false);
    }

    #[test]
    fn scenario_tree_and_tag_refs_use_unsaved_scenario_item_edits() {
        let mut f = fixture();
        let mut api = Api::new();
        let before = api.call(&mut f.app, m::TAG_TREE, json!({})).unwrap();
        assert_eq!(before["incomplete"], false);
        assert!(!unreachable(&before, f.old_item));
        assert!(unreachable(&before, f.new_item));

        api.call(
            &mut f.app,
            m::SCENARIO_SET,
            json!({
                "block": "netgame_equipment", "index": 0, "field": "item",
                "value": {"id": tag_id(f.new_item)},
            }),
        )
        .unwrap();

        let after = api.call(&mut f.app, m::TAG_TREE, json!({})).unwrap();
        assert_eq!(after["incomplete"], false);
        assert!(unreachable(&after, f.old_item));
        assert!(!unreachable(&after, f.new_item));
        let other = section(&after["root"], "Other references (2)");
        assert!(
            other["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["tag"] == tag_id(f.new_item))
        );
        let refs = api
            .call(&mut f.app, m::TAG_REFS, json!({"tag": "scnr"}))
            .unwrap();
        let item = refs
            .as_array()
            .unwrap()
            .iter()
            .find(|link| link["path"] == "0x120[0].0x58")
            .unwrap();
        assert_eq!(item["to"], tag_id(f.new_item));
    }

    #[test]
    fn scenario_tree_and_scan_accept_slot_zero_with_salt_zero() {
        let mut f = fixture();
        assert_eq!(f.sky, TagId(0));
        let mut api = Api::new();
        let tree = api.call(&mut f.app, m::TAG_TREE, json!({})).unwrap();
        assert_eq!(
            section(&tree["root"], "Skies")["children"][0]["tag"],
            tag_id(f.sky)
        );
        let refs = api
            .call(&mut f.app, m::TAG_REFS, json!({"tag": f.scan.0}))
            .unwrap();
        assert_eq!(refs[2]["found"], "scan");
        assert_eq!(refs[2]["to"], tag_id(f.sky));
    }

    #[test]
    fn scenario_tree_palette_selection_keeps_the_tag_and_outlines_its_placements() {
        let mut f = fixture();
        let mut map = open_map(&f.app).unwrap();
        let roots = ScenarioRoots::read(&mut map);
        let tree =
            tag_tree::scenario_tree(&f.app.tags, f.app.session.scenario().unwrap(), &roots, 0);
        let objects = tree
            .root
            .children
            .iter()
            .find(|row| row.label == "Objects")
            .unwrap();
        let weapons = objects
            .children
            .iter()
            .find(|row| row.label == "Weapons (3)")
            .unwrap();
        let first = &weapons.children[0];
        let placed = first.placements.clone().unwrap();
        assert_eq!(placed, [(BlockKind::Weapons, 0), (BlockKind::Weapons, 2)]);
        f.app.dispatch(Action::Navigate(NavAction::SelectPalette {
            tag: f.weapon,
            placements: placed.clone(),
        }));
        assert_eq!(f.app.selection(), Some(Selection::Tag(f.weapon)));
        assert_eq!(f.app.session.selection(), placed);
        f.app.dispatch(Action::Navigate(NavAction::Back));
        assert!(f.app.session.selection().is_empty());
        f.app.dispatch(Action::Navigate(NavAction::Forward));
        assert_eq!(f.app.session.selection(), placed);
        assert_eq!(f.app.selection(), Some(Selection::Tag(f.weapon)));

        let mut api = Api::new();
        api.call(
            &mut f.app,
            m::SCENARIO_SET,
            json!({
                "block": "weapons", "index": 0, "field": "palette index", "value": 1,
            }),
        )
        .unwrap();
        let updated = api.call(&mut f.app, m::TAG_TREE, json!({})).unwrap();
        let entries = &section(section(&updated["root"], "Objects"), "Weapons (3)")["children"];
        assert_eq!(entries[0]["placed"], 1);
        assert_eq!(entries[1]["placed"], 2);
        assert_eq!(updated["root"]["edited"], true);
    }

    /// `tag.kinds` lists every group the registry knows with its editor
    /// and preview, the rows and the default, and one group's line.
    #[test]
    fn tag_kinds_lists_every_group_with_its_editor_and_preview() {
        let mut app = App::new();
        let mut api = Api::new();
        let all = api.call(&mut app, m::TAG_KINDS, Value::Null).unwrap();
        let groups = all["groups"].as_array().unwrap();
        let registry = tags::registry::Registry::builtin();
        assert_eq!(groups.len(), registry.groups.len());
        for g in registry.groups.keys() {
            let id = crate::app::kinds::group_id(*g);
            let line = groups
                .iter()
                .find(|l| l["group"] == id)
                .unwrap_or_else(|| panic!("{id} is listed"));
            assert!(line["editor"].is_string() && line["preview"].is_string(), "{line}");
            assert!(!line["classes"].as_array().unwrap().is_empty());
        }
        assert_eq!(all["kinds"].as_array().unwrap().len(), crate::app::kinds::KINDS.len());
        assert_eq!(all["default"]["editor"], "field_panel");
        assert_eq!(all["target"], "off");
        let bitm = api
            .call(&mut app, m::TAG_KINDS, json!({"group": "bitm"}))
            .unwrap();
        assert_eq!(
            (bitm["editor"].clone(), bitm["preview"].clone()),
            (json!("textures"), json!("thumbnails"))
        );
        assert_eq!(bitm["classes"], json!(["field", "raw", "shape"]));
        let sky = api
            .call(&mut app, m::TAG_KINDS, json!({"group": "sky"}))
            .unwrap();
        assert_eq!((sky["group"].clone(), sky["kind"].clone()), (json!("sky"), json!("Sky and fog")));
        let unknown = api
            .call(&mut app, m::TAG_KINDS, json!({"group": "zzzz"}))
            .unwrap();
        assert_eq!(unknown["default"], true);
        assert!(api.call(&mut app, m::TAG_KINDS, json!({"group": 3})).is_err());
    }
}
