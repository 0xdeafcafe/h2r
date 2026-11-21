//! The studio-tags registry as the agent API's `tag.read`, `tag.set` and
//! `tag.block.*` ([`super::Provider`], registered in [`super::seams`]):
//! every field of any tag by its label, edited through the Tags tool's own
//! commands (`app::tag_api`, applied with `Action::Tag`), so an agent's
//! edit is the same undo step and the same saved patch as the window's.
//!
//! The scenario tag stays with the built-in methods: its placement blocks
//! are the Scenario tool's (`scenario.*`, `tag.set` paths
//! `<block>[<i>].<field>`), and the Tags tool reads it only.

use map::{FourCC, TagId};
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::registry::{Kind, Layout};

use super::scenario::refused;
use super::{METHODS, MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::tag_api;
use crate::app::{Action, BlockOp, NavAction, TagAction, ToolKind};
use crate::fields as panel;
use crate::fields::picker;
use crate::play::issues::IssueIndex;

/// The registry's provider.
#[derive(Default)]
pub struct TagFields;

const READ: MethodDef = MethodDef {
    name: m::TAG_READ,
    summary: "A tag as structured JSON: every field by its registry label (blocks as arrays of elements, data as hex), its access, and whether it is edited. `raw: true` gives the schema's structural view instead",
    params: r#"{"tag":"string (0x... id, group:name, name[.group], or a lone group)","depth?":"integer (raw view)","elements?":"integer (raw view)","hex?":"boolean (raw view)","raw?":"boolean"}"#,
    result: r#"{"id":"string","group":"string","name":"string","access":"edit|{read_only}|{none}","edited":"boolean","fields":"{label: value}"}"#,
    example: r#"{"tag":"itmc"}"#,
    planned: None,
};
const SET: MethodDef = MethodDef {
    name: m::TAG_SET,
    summary: "Set a field by its path from the tag's root (`parts[1].speed`, or `+0x14` for an unnamed one) to the JSON tag.read shows for it: numbers or arrays, enum names, flag-name lists, strings for string IDs, \"group:name\" or null for references. One undo step. The scenario's placements: <block>[<i>].<field>",
    params: r#"{"tag":"string","path":"string","value":"any"}"#,
    result: r#"{"value":"any"}"#,
    example: r#"{"tag":"itmc","path":"+0x8","value":7}"#,
    planned: None,
};

const FIELDS: MethodDef = def(
    m::TAG_FIELDS,
    "The field panel's rows for a tag or one block element, as the Tags tool and the inspector draw them (fields::rows): the named rows, then the Unnamed fields group's (padding last). Each row has the label in words, the type in words, the unit, whether the field is named, editable (else read_only says why), edited (file_value is the file's value), when a change takes effect, the chip the row shows (only with a game connected) and the tag.set path. `unnamed: false` leaves the unnamed rows out",
    r#"{"tag":"string|integer","path?":"string (block element, e.g. parts[0])","unnamed?":"boolean (default true)"}"#,
    r#"{"fields":"[{name,offset,kind,size,value,description,evidence,section,label,named,type,unit,editable,read_only,edited,file_value,effect:{id,text,why},chip,live:{kind,text}|null,path,summary,choices?:[{value,name}],bits?:[{bit,name}],groups?:[string],count?,capacity?,show_as?}]"}"#,
    r#"{"tag":"itmc"}"#,
);

fn fields(app: &mut App, p: &P) -> Reply {
    let tag = resolve(app, p)?;
    let path = p.opt_str("path")?.unwrap_or_default();
    let unnamed = match p.get("unnamed") {
        None => Unnamed::All,
        Some(v) => match v.as_bool() {
            Some(true) => Unnamed::All,
            Some(false) => Unnamed::Without,
            None => return Err(err(code::INVALID_PARAMS, "unnamed: a boolean")),
        },
    };
    let doc = app.shown_tags();
    let (at, layout) = tag_api::field_element(doc, tag, path)
        .map_err(|reason| err(code::REFUSED, reason))?;
    let (shown, _) = rows(app, tag, path, at, &layout, unnamed);
    Ok(json!({ "fields": shown }))
}

/// Which of an element's unnamed rows [`rows`] gives.
#[derive(Clone, Copy)]
enum Unnamed {
    All,
    Without,
    /// As the panel opens the group by itself (`fields::row::opens`), with
    /// the focused field's address.
    AsShown { focus: Option<u32> },
}

/// The panel's rows of the element at `at` as JSON, in the panel's order,
/// for the open map's game link; and how many unnamed rows are left out.
/// `path` is the element's path from the root (empty for it). An unnamed
/// value's row says how it is shown (`show_as`).
fn rows(
    app: &App,
    tag: TagId,
    path: &str,
    at: u32,
    layout: &Layout,
    unnamed: Unnamed,
) -> (Vec<Value>, usize) {
    let doc = app.shown_tags();
    let link = panel::Link::of(app);
    let issues = match (link, &app.pairing.link) {
        (panel::Link::Off, _) | (_, None) => IssueIndex::empty(),
        (_, Some(l)) => l.issues.as_ref(),
    };
    let prefix = match tag_api::named_path(doc, tag, path) {
        Ok(p) if !p.is_empty() => format!("{p}."),
        _ => String::new(),
    };
    let cx = panel::row::Context {
        link,
        issues,
        prefix: &prefix,
    };
    let all = panel::rows(doc, tag, at, layout, &cx);
    let (named, hidden) = panel::row::split(&all);
    let open = match unnamed {
        Unnamed::All => true,
        Unnamed::Without => false,
        Unnamed::AsShown { focus } => {
            panel::row::opens(&hidden, app.tag_fields.unnamed, named.is_empty(), focus)
        }
    };
    let group = doc.tag(tag).map_or(FourCC(*b"????"), |t| t.group);
    let mut out: Vec<Value> = named.iter().map(|r| r.json(doc, tag, at)).collect();
    if !open {
        return (out, hidden.len());
    }
    for r in &hidden {
        let mut row = r.json(doc, tag, at);
        if let Kind::Unknown { len } = r.field.kind
            && let Some(b) = doc.read(tag, r.address, len)
        {
            let v = b.iter().rev().fold(0u32, |v, x| v << 8 | u32::from(*x));
            let key = panel::show_as_key(group, &prefix, r.field.offset);
            row["show_as"] = json!(panel::shown_as(&app.tag_fields, &key, r.field, v).id());
        }
        out.push(row);
    }
    (out, 0)
}

const VIEW: MethodDef = def(
    m::TAG_VIEW,
    "Drive the Tags tool's field panel and read what it shows. `tag` selects a tag in the Tags tool; `field` (a tag.set path) focuses a field, opening its lists and its Unnamed fields group; `unnamed` sets Show unnamed fields; `show_as` sets how an unnamed value is shown (`as` null for the guess), kept for that field of every tag of the kind. With none it only reads. `rows` are the root's rows the panel shows, `unnamed_hidden` the unnamed rows in a folded group, `header` the header's lines as drawn",
    r#"{"tag?":"string|integer","field?":"string","unnamed?":"boolean","show_as?":"{path: string, as: decimal|whole_number|hex|null}"}"#,
    r#"{"tag":"string|null","path":"string","breadcrumbs":"[string]","field":"string|null","search":"null","changed":"boolean","unnamed":"boolean","rows":"[tag.fields row]","unnamed_hidden":"integer","header":"[string]"}"#,
    r#"{"tag":"itmc","unnamed":true}"#,
);

fn view(app: &mut App, p: &P) -> Reply {
    // `path`, `search` and `changed` are not in the params, so the
    // dispatcher refuses them: the panel shows a tag's root, unsearched.
    if p.has("tag") {
        let id = resolve(app, p)?;
        app.perform(Action::Tag(TagAction::Select(id)), false)
            .map_err(refused)?;
        app.dispatch(Action::SetTool(ToolKind::Tags));
    }
    let selected = app.shown_tags().selected;
    let need = |what: &str| {
        selected.ok_or_else(|| err(code::INVALID_PARAMS, format!("{what}: select a tag first")))
    };
    if let Some(field) = p.opt_str("field")? {
        let tag = need("field")?;
        if app.browsing().is_some() {
            return Err(err(code::REFUSED, "field: a browsed map's fields are shown, not focused"));
        }
        tag_api::field_focus(&app.tags, tag, field)
            .map_err(|e| err(code::INVALID_PARAMS, format!("{field}: {e}")))?;
        let go = NavAction::Go {
            tag,
            path: Some(field.to_owned()),
        };
        app.perform(Action::Navigate(go), false).map_err(refused)?;
        app.dispatch(Action::SetTool(ToolKind::Tags));
    }
    if let Some(v) = p.get("unnamed") {
        app.tag_fields.unnamed = v
            .as_bool()
            .ok_or_else(|| err(code::INVALID_PARAMS, "unnamed: a boolean"))?;
    }
    if let Some(choice) = p.get("show_as") {
        let tag = need("show_as")?;
        let (key, chosen) = show_as(app.shown_tags(), tag, choice)?;
        match chosen {
            Some(to) => app.tag_fields.show_as.insert(key, to),
            None => app.tag_fields.show_as.remove(&key),
        };
    }
    let Some(tag) = selected else {
        return Ok(json!({
            "tag": null, "path": "", "breadcrumbs": [], "field": null, "search": null,
            "changed": false, "unnamed": app.tag_fields.unnamed, "rows": [],
            "unnamed_hidden": 0, "header": [],
        }));
    };
    let doc = app.shown_tags();
    let field = match app.browsing() {
        None => {
            let location = app.nav_location();
            let here = location.tag() == Some(tag);
            location.path.filter(|_| here)
        }
        Some(_) => None,
    };
    let focus = field
        .as_deref()
        .and_then(|f| tag_api::field_focus(doc, tag, f).ok())
        .map(|f| f.address);
    let (at, layout) = tag_api::field_element(doc, tag, "")
        .map_err(|reason| err(code::REFUSED, reason))?;
    let (shown, unnamed_hidden) = rows(app, tag, "", at, &layout, Unnamed::AsShown { focus });
    Ok(json!({
        "tag": format!("{:#010x}", tag.0),
        "path": "",
        "breadcrumbs": [doc.tag_label(tag.0)],
        "field": field,
        "search": null,
        "changed": false,
        "unnamed": app.tag_fields.unnamed,
        "rows": shown,
        "unnamed_hidden": unnamed_hidden,
        "header": crate::ui::tags::header(app, tag),
    }))
}

/// A `show_as` param, `{path, as}`, as the settings key and the view (none
/// for the guess).
fn show_as(
    doc: &crate::app::TagDoc,
    tag: TagId,
    choice: &Value,
) -> Result<(String, Option<panel::View>), agent_proto::RpcError> {
    let bad = |why: String| err(code::INVALID_PARAMS, format!("show_as: {why}"));
    let path = choice
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("give the unnamed value's path".into()))?;
    let (_, _, f) = tag_api::resolve(doc, tag, path).map_err(|e| bad(format!("{path}: {e}")))?;
    if !matches!(f.kind, Kind::Unknown { .. }) {
        return Err(bad(format!("{path} is not an unnamed value")));
    }
    let chosen = match choice.get("as") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .and_then(panel::View::from_id)
                .ok_or_else(|| bad("as: decimal, whole_number, hex or null".into()))?,
        ),
    };
    let named = tag_api::named_path(doc, tag, path).map_err(bad)?;
    let prefix = named.rsplit_once('.').map_or(String::new(), |(e, _)| format!("{e}."));
    let group = doc.tag(tag).map_or(FourCC(*b"????"), |t| t.group);
    Ok((panel::show_as_key(group, &prefix, f.offset), chosen))
}

const CHOICES: MethodDef = def(
    m::TAG_CHOICES,
    "What the field panel's pick dialog lists for a field (fields::picker): an enum's named values (`values`); a switches field's bits, named or not, and whether each is on (`bits`); a tag reference's kinds and every tag it takes, with no cap (`kinds`, `tags`); or how an unnamed value can be shown (`views`, `shown`). `query` narrows values and tags as the dialog's search box does (words of the name, or of group:name); `kind` (a group code) narrows tags as the dialog's Kind row does; `limit` cuts the tags and sets `truncated`",
    r#"{"tag":"string|integer","path":"string (a tag.set path)","query?":"string","kind?":"string (a group code)","limit?":"integer"}"#,
    r#"{"values?":"[{value,name}]","bits?":"[{bit,name,label,on}]","kinds?":"[{group,name}]","tags?":"[{id,group,name}]","truncated?":"boolean","views?":"[{id,name}]","shown?":"string"}"#,
    r#"{"tag":"itmc","path":"+0xa"}"#,
);

fn choices(app: &mut App, p: &P) -> Reply {
    let tag = resolve(app, p)?;
    let path = p.str("path")?;
    let query = p.opt_str("query")?.unwrap_or_default();
    let limit = p.opt_usize("limit")?;
    let kind = match p.opt_str("kind")? {
        None => None,
        Some(k) => Some(
            tags::defs::parse_fourcc(k)
                .ok_or_else(|| err(code::INVALID_PARAMS, format!("kind: {k} is not a group code")))?,
        ),
    };
    let doc = app.shown_tags();
    let (at, _, f) = tag_api::resolve(doc, tag, path)
        .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))?;
    let a = at + f.offset;
    let now = doc.read(tag, a, f.kind.width()).unwrap_or_default();
    let word = |n: usize| now.iter().take(n).rev().fold(0u64, |v, x| v << 8 | u64::from(*x));
    match &f.kind {
        Kind::Enum { values, .. } => {
            let listed: Vec<Value> = picker::values(values, query)
                .into_iter()
                .map(|c| json!({ "value": c.value, "name": c.name }))
                .collect();
            Ok(json!({ "values": listed }))
        }
        Kind::Flags { bytes: width, bits } => {
            let n = usize::from(*width);
            let v = word(n);
            let listed: Vec<Value> = picker::bits(bits, 8 * n as u32)
                .into_iter()
                .map(|b| {
                    let on = v >> b.bit & 1 == 1;
                    json!({ "bit": b.bit, "name": b.name, "label": b.label, "on": on })
                })
                .collect();
            Ok(json!({ "bits": listed }))
        }
        Kind::TagRef { groups } | Kind::Datum { groups } => {
            let kinds: Vec<Value> = picker::kinds(doc, groups)
                .into_iter()
                .map(|g| {
                    let name = panel::row::kind_words(doc, g);
                    json!({ "group": tags::defs::fourcc_text(g), "name": name })
                })
                .collect();
            let all = picker::tags(doc, groups, kind, query);
            let shown = limit.unwrap_or(all.len()).min(all.len());
            let listed: Vec<Value> = all[..shown]
                .iter()
                .map(|t| {
                    json!({
                        "id": format!("{:#010x}", t.id.0),
                        "group": tags::defs::fourcc_text(t.group),
                        "name": t.name,
                    })
                })
                .collect();
            Ok(json!({ "kinds": kinds, "tags": listed, "truncated": shown < all.len() }))
        }
        Kind::Unknown { len } => {
            let group = doc.tag(tag).map_or(FourCC(*b"????"), |t| t.group);
            let named = tag_api::named_path(doc, tag, path)
                .map_err(|e| err(code::INVALID_PARAMS, e))?;
            let prefix = named.rsplit_once('.').map_or(String::new(), |(e, _)| format!("{e}."));
            let kept = panel::show_as_key(group, &prefix, f.offset);
            let v = word(4) as u32;
            let listed: Vec<Value> = picker::views(*len)
                .into_iter()
                .map(|to| json!({ "id": to.id(), "name": to.words() }))
                .collect();
            let shown = panel::shown_as(&app.tag_fields, &kept, &f, v).id();
            Ok(json!({ "views": listed, "shown": shown }))
        }
        other => Err(err(
            code::INVALID_PARAMS,
            format!(
                "{path}: a {} field has no choices",
                panel::row::type_words(other).to_ascii_lowercase()
            ),
        )),
    }
}

const BYTES: MethodDef = def(
    m::TAG_BYTES,
    "The bytes of a tag's element (the root, or a block element such as `parts[1]`) or of one field (a tag.set path), as the Data and bytes window shows them: the address, the size, the bytes now and in the loaded file as hex, and the spans of its fields at offsets from the address (an element's spans tile it)",
    r#"{"tag":"string|integer","path?":"string (an element or a field; default the root)"}"#,
    r#"{"address":"integer","size":"integer","hex":"string","file_hex":"string|null","spans":"[{offset,size,name,named}]"}"#,
    r#"{"tag":"itmc"}"#,
);

const RESET: MethodDef = def(
    m::TAG_RESET,
    "Put a field back to its value in the loaded file, as the Data and bytes window's Reset to file value does: one undo step, none when it already has that value. Lists and data are refused (put back their elements or bytes)",
    r#"{"tag":"string|integer","path":"string (a tag.set path)"}"#,
    r#"{"value":"any (the field's value now)"}"#,
    r#"{"tag":"itmc","path":"respawn_time"}"#,
);

fn bytes(app: &mut App, p: &P) -> Reply {
    let tag = resolve(app, p)?;
    let path = p.opt_str("path")?.unwrap_or_default();
    tag_api::bytes_of(app.shown_tags(), tag, path)
        .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))
}

fn reset(app: &mut App, p: &P) -> Reply {
    let tag = resolve(app, p)?;
    let path = p.str("path")?;
    if app.browsing().is_some() {
        return Err(err(code::REFUSED, "a browsed map's tags are shown, not edited"));
    }
    if app.session.scenario().is_some_and(|s| s.tag == tag) {
        return Err(err(code::REFUSED, "the scenario's fields are the Scenario tool's"));
    }
    let action = tag_api::reset(&app.tags, tag, path)
        .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))?;
    if let Some(action) = action {
        perform(app, action)?;
    }
    let (at, _, f) = tag_api::resolve(&app.tags, tag, path)
        .map_err(|e| err(code::REFUSED, e))?;
    Ok(json!({ "value": tag_api::value(&app.tags, tag, at, &f) }))
}

const CHAIN: MethodDef = MethodDef {
    name: m::TAG_CHAIN,
    summary: "The inspector's tag tree for a placement or tag, including in-memory tag edits",
    params: r#"{"block?":"string","index?":"integer","tag?":"string|integer","depth?":"integer (default 3, max 8)"}"#,
    result: r#"{"root":"{tag,group,name,via,label,where,cache,variant,cycle,children:[node]}|null","truncated":"boolean"}"#,
    example: r#"{"block":"weapons","index":0}"#,
    planned: None,
};

fn chain(app: &mut App, p: &P) -> Reply {
    let depth = p.opt_u64("depth")?.unwrap_or(tag_api::DEFAULT_DEPTH as u64);
    if depth > tag_api::MAX_DEPTH as u64 {
        return Err(err(code::INVALID_PARAMS, format!("depth: at most {}", tag_api::MAX_DEPTH)));
    }
    let (doc, root) = match (p.has("block"), p.has("index"), p.get("tag")) {
        (true, true, None) => {
            let (kind, index) = p.entry(app)?;
            sync_open_tags(app)?;
            let root = tag_api::placement_root(super::scenario::open(app)?, kind, index);
            (&app.tags, root)
        }
        (false, false, Some(want)) => {
            if app.browsing().is_none() {
                sync_open_tags(app)?;
            }
            let doc = app.shown_tags();
            let id = tag_api::lookup(doc, want).map_err(|e| err(code::INVALID_PARAMS, e))?;
            (doc, Some((id, "")))
        }
        _ => return Err(err(code::INVALID_PARAMS, "supply block and index, or tag")),
    };
    Ok(tag_api::chain(doc, root, depth as usize).json(doc))
}

pub(super) fn sync_open_tags(app: &mut App) -> Result<(), agent_proto::RpcError> {
    super::scenario::open(app)?;
    app.sync_tags();
    if let Some(e) = &app.tags.error {
        return Err(err(code::FAILED, format!("the tag editor could not read the map: {e}")));
    }
    Ok(())
}

fn block_def(name: &'static str, summary: &'static str, params: &'static str) -> MethodDef {
    let base = METHODS.iter().find(|d| d.name == name).expect("in the table");
    MethodDef {
        name,
        summary,
        params,
        result: r#"{"count":"integer|null (the block's elements now)"}"#,
        example: base.example,
        planned: None,
    }
}

impl Provider for TagFields {
    fn methods(&self) -> Vec<MethodDef> {
        vec![
            READ,
            SET,
            CHAIN,
            FIELDS,
            VIEW,
            CHOICES,
            BYTES,
            RESET,
            block_def(
                m::TAG_BLOCK_ADD,
                "Add an element to a tag block (path from the root, `parts` or `parts[0].bones`), where the block has room in its source; refused with the reason otherwise",
                r#"{"tag":"string","path":"string (a block)","like?":"integer"}"#,
            ),
            block_def(
                m::TAG_BLOCK_REMOVE,
                "Remove an element of a tag block",
                r#"{"tag":"string","path":"string (a block)","index":"integer"}"#,
            ),
            block_def(
                m::TAG_BLOCK_DUPLICATE,
                "Duplicate an element of a tag block",
                r#"{"tag":"string","path":"string (a block)","index":"integer"}"#,
            ),
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &serde_json::Value) -> Option<Reply> {
        let p = P(params);
        match method {
            m::TAG_FIELDS => return Some(fields(app, &p)),
            m::TAG_VIEW => return Some(view(app, &p)),
            m::TAG_CHOICES => return Some(choices(app, &p)),
            m::TAG_BYTES => return Some(bytes(app, &p)),
            m::TAG_RESET => return Some(reset(app, &p)),
            m::TAG_CHAIN => return Some(chain(app, &p)),
            _ => {}
        }
        if ![
            m::TAG_READ,
            m::TAG_SET,
            m::TAG_BLOCK_ADD,
            m::TAG_BLOCK_REMOVE,
            m::TAG_BLOCK_DUPLICATE,
        ]
        .contains(&method)
        {
            return None;
        }
        if method == m::TAG_READ && p.bool("raw").unwrap_or(false) {
            return None;
        }
        let id = match resolve(app, &p) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
        // The scenario tag's edits are the Scenario tool's (a browsed
        // map's tags are not edited at all: the Tags tool says so).
        if method != m::TAG_READ
            && app.browsing().is_none()
            && app.session.scenario().is_some_and(|s| s.tag == id)
        {
            return None;
        }
        Some(self.run(app, method, &p, id))
    }
}

/// The `tag` param: `group:name` as the Tags tool writes it, else as the
/// built-in methods take it. A browsed map's tags resolve in it.
pub(super) fn resolve(app: &mut App, p: &P) -> Result<TagId, agent_proto::RpcError> {
    if app.browsing().is_some() {
        let want = p.get("tag").cloned().unwrap_or(Value::Null);
        return tag_api::lookup(app.shown_tags(), &want).map_err(|e| err(code::INVALID_PARAMS, e));
    }
    sync_open_tags(app)?;
    if let Some(s) = p.get("tag").and_then(Value::as_str)
        && let Some(id) = tag_api::find_tag(&app.tags, s)
    {
        return Ok(id);
    }
    let map = super::tags::open_map(app)?;
    let want = p.get("tag").cloned().unwrap_or(Value::Null);
    Ok(super::tags::resolve(&map, &want)?.id)
}

impl TagFields {
    fn run(&self, app: &mut App, method: &str, p: &P, id: TagId) -> Reply {
        let failed = |e: String| err(code::REFUSED, e);
        match method {
            m::TAG_READ => tag_api::read_tag(app.shown_tags(), id).map_err(failed),
            m::TAG_SET => {
                let path = p.str("path")?;
                let value = p.get("value").cloned().unwrap_or(Value::Null);
                let action = tag_api::set_field(app.shown_tags(), id, path, &value)
                    .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))?;
                perform(app, action)?;
                let (at, _, f) = tag_api::resolve(&app.tags, id, path).map_err(failed)?;
                Ok(json!({ "value": tag_api::value(&app.tags, id, at, &f) }))
            }
            _ => {
                let path = p.str("path")?;
                let op = match method {
                    m::TAG_BLOCK_ADD => match p.opt_usize("like")? {
                        Some(i) => BlockOp::Duplicate(i as u32),
                        None => BlockOp::Add,
                    },
                    m::TAG_BLOCK_REMOVE => BlockOp::Remove(p.usize("index")? as u32),
                    _ => BlockOp::Duplicate(p.usize("index")? as u32),
                };
                let action = tag_api::block(app.shown_tags(), id, path, op)
                    .map_err(|e| err(code::REFUSED, format!("{path}: {e}")))?;
                perform(app, action)?;
                let count = tag_api::resolve(&app.tags, id, path)
                    .ok()
                    .and_then(|(at, _, f)| {
                        tag_api::value(&app.tags, id, at, &f)
                            .as_array()
                            .map(Vec::len)
                    });
                Ok(json!({ "count": count }))
            }
        }
    }
}

/// Applies a Tags tool command the way the window does, with its outcome.
pub(super) fn perform(app: &mut App, action: crate::app::TagAction) -> Result<(), agent_proto::RpcError> {
    app.perform(Action::Tag(action), false).map_err(|r| match r {
        crate::app::Refused::Failed(e) => err(code::REFUSED, e),
        other => refused(other),
    })?;
    // End the step: the next call is its own undo step.
    let _ = app.perform(Action::Tag(crate::app::TagAction::Seal), false);
    Ok(())
}
