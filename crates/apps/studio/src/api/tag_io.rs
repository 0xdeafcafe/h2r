//! The Tags tool's browsing, resources and tag bundles as the agent API
//! ([`super::Provider`], registered in [`super::seams`]): `tag.list` over
//! the document the Tags tool shows, `tag.browse` (another map of the
//! source, read-only: the shared caches), `tag.resources`, and
//! `tag.export` / `tag.import` through the window's own actions
//! (`Action::TagExport`, `Action::TagImport`; `app::tag_bundle`).

use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::tag_bundle::{ExportOptions, ImportOptions};
use crate::app::tag_doc::{Access, TagDoc};
use crate::app::{Action, Refused, tag_api, tag_tree};

/// The provider.
#[derive(Default)]
pub struct TagIo;

const LIST: MethodDef = MethodDef {
    name: m::TAG_LIST,
    summary: "The tags of the map the Tags tool shows (the open map, or one browsed with tag.browse): by group, name, words of group:name or a folder of their paths, with where each lives (`elsewhere`: the shared cache a tag with no data here has its data in; with `homes`, its raw pointers counted by map)",
    params: r#"{"group?":"string","name?":"string (part of the path)","query?":"string (words, each in group:name)","path?":"string (a folder: objects\\weapons)","limit?":"integer (default 200)","homes?":"boolean"}"#,
    result: r#"{"map":"string|null","browsing":"boolean","total":"integer","groups":"{group: count}","tags":"[{id,group,name,size,address,access,elsewhere?,raw?}]"}"#,
    example: r#"{"group":"scnr"}"#,
    planned: None,
};
const BROWSE: MethodDef = MethodDef {
    name: m::TAG_BROWSE,
    summary: "Show another map of the source in the Tags tool, read-only (shared, single_player_shared, mainmenu or any other); without `map`, or with the open map's name, the open map again. tag.list, tag.read, tag.resources and tag.export then read it",
    params: r#"{"map?":"string"}"#,
    result: r#"{"map":"string|null","browsing":"boolean","total":"integer","groups":"{group: count}"}"#,
    example: r#"{"map":"shared"}"#,
    planned: None,
};
const RESOURCES: MethodDef = MethodDef {
    name: m::TAG_RESOURCES,
    summary: "A tag's data references and raw-resource pointers with their paths, files (local, mainmenu, shared, single_player_shared), offsets and sizes; for a bitmap its images, for a sound its pitch ranges, permutations and chunks",
    params: r#"{"tag":"string"}"#,
    result: r#"{"data":"[{path,size,address}]","raw":"[{path,file,offset,size}]","truncated":"boolean","media":"{bitmap}|{sound}|null","media_error?":"string"}"#,
    example: r#"{"tag":"itmc"}"#,
    planned: None,
};
const EXPORT: MethodDef = MethodDef {
    name: m::TAG_EXPORT,
    summary: "Write a tag of the map shown as a tag bundle (.h2tag, JSON: bytes, references by name, string IDs by text, blocks, data; docs/editor/editor.md): to `out`, else under the data root's tag-bundles/; never into a Git checkout. `dependencies`: the tags it references too; `resources`: raw resources as blobs",
    params: r#"{"tag":"string","out?":"string","dependencies?":"boolean","resources?":"boolean"}"#,
    result: r#"{"path":"string","tags":"[group:name]","blobs":"integer","blob_bytes":"integer","left_out":"[string]"}"#,
    example: r#"{"tag":"itmc","out":"$OUT/kit.h2tag"}"#,
    planned: None,
};
const IMPORT: MethodDef = MethodDef {
    name: m::TAG_IMPORT,
    summary: "Replace tags of the open map with a tag bundle's (each the tag of the same group and name; `tag`: the one a bundle of one tag replaces), as one undo step of the tags document, saved with the map. The map writer replaces tags and does not add them: a tag the map lacks refuses the import unless `skip_missing`",
    params: r#"{"path":"string","tag?":"string","skip_missing?":"boolean"}"#,
    result: r#"{"replaced":"[group:name]","skipped":"[group:name]","raw_kept":"integer","nulled":"[string]","strings_added":"integer"}"#,
    example: r#"{"path":"$OUT/kit.h2tag"}"#,
    planned: None,
};

impl Provider for TagIo {
    fn methods(&self) -> Vec<MethodDef> {
        vec![LIST, BROWSE, RESOURCES, EXPORT, IMPORT]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::TAG_LIST => list(app, &p),
            m::TAG_BROWSE => browse(app, &p),
            m::TAG_RESOURCES => resources(app, &p),
            m::TAG_EXPORT => export(app, &p),
            m::TAG_IMPORT => import(app, &p),
            _ => return None,
        })
    }
}

fn perform(app: &mut App, a: Action) -> Result<Vec<String>, agent_proto::RpcError> {
    app.perform(a, false).map_err(|r| match r {
        Refused::Failed(e) => err(code::REFUSED, e),
        other => refused(other),
    })
}

/// The document shown, read: the browsed map's, else the open map's
/// (which needs a map open).
fn shown(app: &mut App) -> Result<&TagDoc, agent_proto::RpcError> {
    if app.browsing().is_none() {
        super::scenario::open(app)?;
        app.sync_tags();
        if let Some(e) = &app.tags.error {
            return Err(err(
                code::FAILED,
                format!("the tag editor could not read the map: {e}"),
            ));
        }
    }
    Ok(app.shown_tags())
}

fn access(a: &Access) -> &'static str {
    match a {
        Access::Edit => "edit",
        Access::Read(_) => "read_only",
        Access::None(_) => "none",
    }
}

fn group_counts(doc: &TagDoc, hits: &[usize]) -> Value {
    Value::Object(
        tag_tree::by_group(doc.tags(), hits)
            .into_iter()
            .map(|(g, list)| (g, json!(list.len())))
            .collect(),
    )
}

fn list(app: &mut App, p: &P) -> Reply {
    let group = p.opt_str("group")?.map(str::to_owned);
    let name = p.opt_str("name")?.map(str::to_ascii_lowercase);
    let query = p.opt_str("query")?.unwrap_or_default().to_owned();
    let folder = p.opt_str("path")?.map(str::to_owned);
    let limit = p.opt_usize("limit")?.unwrap_or(200);
    let homes = p.bool("homes")?;
    let browsing = app.browsing().is_some();
    let map = app.shown_map_name().map(str::to_owned);
    let doc = shown(app)?;
    let mut hits: Vec<usize> = tag_tree::search(doc.tags(), &query)
        .into_iter()
        .filter(|&i| {
            let t = &doc.tags()[i];
            group
                .as_deref()
                .is_none_or(|g| tags::defs::fourcc_text(t.group).eq_ignore_ascii_case(g))
                && name
                    .as_deref()
                    .is_none_or(|n| t.name.to_ascii_lowercase().contains(n))
        })
        .collect();
    if let Some(f) = &folder {
        let tree = tag_tree::by_path(doc.tags(), &hits);
        hits = tree.find(f).map(tag_tree::under).unwrap_or_default();
    }
    let tags: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|&i| {
            let t = &doc.tags()[i];
            let mut v = json!({
                "id": format!("{:#010x}", t.id.0),
                "group": tags::defs::fourcc_text(t.group),
                "name": t.name,
                "size": (t.size > 0).then_some(t.size),
                "address": (t.size > 0).then(|| format!("{:#010x}", t.address)),
                "access": access(&t.access),
            });
            if let Some(m) = &t.elsewhere {
                v["elsewhere"] = json!(m);
            }
            if homes && let Some(h) = doc.homes(t.id) {
                v["raw"] = h.json();
            }
            v
        })
        .collect();
    Ok(json!({
        "map": map,
        "browsing": browsing,
        "total": hits.len(),
        "groups": group_counts(doc, &hits),
        "tags": tags,
    }))
}

fn browse(app: &mut App, p: &P) -> Reply {
    let to = p.opt_str("map")?.map(str::to_owned);
    perform(app, Action::BrowseTags(to))?;
    let browsing = app.browsing().is_some();
    let map = app.shown_map_name().map(str::to_owned);
    let doc = app.shown_tags();
    let all: Vec<usize> = (0..doc.tags().len()).collect();
    Ok(json!({
        "map": map,
        "browsing": browsing,
        "total": all.len(),
        "groups": group_counts(doc, &all),
    }))
}

fn resources(app: &mut App, p: &P) -> Reply {
    let id = super::tag_fields::resolve(app, p)?;
    let mut v = tag_api::resources(app.shown_tags(), id).map_err(|e| err(code::REFUSED, e))?;
    match app.tag_media(id) {
        Ok(media) => v["media"] = media,
        Err(e) => {
            v["media"] = Value::Null;
            v["media_error"] = json!(e);
        }
    }
    Ok(v)
}

fn export(app: &mut App, p: &P) -> Reply {
    let id = super::tag_fields::resolve(app, p)?;
    let out = p.opt_str("out")?.map(PathBuf::from);
    if let Some(o) = &out {
        super::docs::outside_git(o)?;
    }
    let options = ExportOptions {
        dependencies: p.bool("dependencies")?,
        resources: p.bool("resources")?,
    };
    perform(
        app,
        Action::TagExport {
            tags: vec![id],
            out,
            options,
        },
    )?;
    let (path, r) = app
        .last_tag_export
        .clone()
        .ok_or_else(|| err(code::FAILED, "the export wrote nothing"))?;
    Ok(json!({
        "path": path.display().to_string(),
        "tags": r.tags,
        "blobs": r.blobs,
        "blob_bytes": r.blob_bytes,
        "left_out": r.left_out,
    }))
}

fn import(app: &mut App, p: &P) -> Reply {
    let path = PathBuf::from(p.str("path")?);
    if app.browsing().is_some() {
        return Err(err(
            code::REFUSED,
            "a browsed map is read-only: tag.browse {} back to the open map, then import",
        ));
    }
    let target = if p.has("tag") {
        Some(super::tag_fields::resolve(app, p)?)
    } else {
        super::scenario::open(app)?;
        None
    };
    let options = ImportOptions {
        target,
        skip_missing: p.bool("skip_missing")?,
    };
    perform(app, Action::TagImport { path, options })?;
    let r = app
        .last_tag_import
        .clone()
        .ok_or_else(|| err(code::FAILED, "the import did nothing"))?;
    Ok(json!({
        "replaced": r.replaced,
        "skipped": r.skipped,
        "raw_kept": r.raw_kept,
        "nulled": r.nulled,
        "strings_added": r.strings_added,
    }))
}
