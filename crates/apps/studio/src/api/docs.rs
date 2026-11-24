//! `source.*` and `doc.*`: what is open, opening, saving, the diff a save
//! would write, undo and redo, for every tool's document.

use std::path::{Path, PathBuf};

use agent_proto::RpcError;
use editor_core::map_groups::MapGroup;
use scenario_edit::layout;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use variants::GameType;
use variants::panel::Origin;

use super::params::block_name;
use super::scenario::refused;
use super::{P, Reply, err};
use crate::App;
use crate::app::map_cards::MapEntry;
use crate::app::{Action, Doc, SaveTarget, ToolKind};

/// A `doc` param as a tool (default: the tool shown).
pub fn doc_kind(app: &App, p: &P) -> Result<ToolKind, RpcError> {
    match p.opt_str("doc")? {
        None => Ok(app.tool),
        Some(d) => parse_doc(d),
    }
}

pub fn parse_doc(d: &str) -> Result<ToolKind, RpcError> {
    Ok(match d {
        "map" | "scenario" => ToolKind::Scenario,
        "variant" | "variants" => ToolKind::Variants,
        "playlists" | "playlist" | "hopper" => ToolKind::Playlists,
        "ui" | "ui-screens" => ToolKind::UiScreens,
        "tags" => ToolKind::Tags,
        "hud" => ToolKind::Hud,
        "scripts" => ToolKind::Scripts,
        "bitmaps" | "textures" => ToolKind::Textures,
        other => {
            return Err(err(
                code::INVALID_PARAMS,
                format!("no document {other:?} (map, tags, scripts, bitmaps, variant, playlists, ui, hud)"),
            ));
        }
    })
}

pub fn doc_name(t: ToolKind) -> &'static str {
    match t {
        ToolKind::Scenario => "map",
        ToolKind::Variants => "variant",
        ToolKind::Playlists => "playlists",
        ToolKind::UiScreens => "ui",
        ToolKind::Tags => "tags",
        ToolKind::Hud => "hud",
        ToolKind::Scripts => "scripts",
        ToolKind::Textures => "bitmaps",
    }
}

fn is_open(app: &App, t: ToolKind) -> bool {
    match t {
        ToolKind::Scenario => app.session.is_open(),
        ToolKind::Variants => app.variants.as_ref().is_some_and(|e| e.current().is_some()),
        ToolKind::Playlists => app.hoppers.is_open(),
        ToolKind::UiScreens => app.ui_screens.pick.is_some(),
        ToolKind::Tags => app.session.is_open(),
        ToolKind::Hud => app.hud.session.is_some(),
        ToolKind::Scripts => app.scripts_loaded().is_ok(),
        ToolKind::Textures => app.session.is_open(),
    }
}

fn can(app: &App, t: ToolKind) -> (bool, bool) {
    match t {
        // A target in edit mode takes the map's undo (docs/editor/modeling.md).
        ToolKind::Scenario if app.modeling.session.active.is_some() => {
            let g = &app.modeling.session;
            (g.can_undo(), g.can_redo())
        }
        ToolKind::Scenario => (app.session.can_undo(), app.session.can_redo()),
        ToolKind::Variants => app
            .variants
            .as_ref()
            .map_or((false, false), |e| (e.can_undo(), e.can_redo())),
        ToolKind::Playlists => (app.hoppers.can_undo(), app.hoppers.can_redo()),
        ToolKind::UiScreens => (false, false),
        // The compiles into the map's scripts are the Scripts surface's stack.
        ToolKind::Scripts => app
            .scripts
            .edit
            .as_ref()
            .map_or((false, false), |d| {
                let (undo, redo) = d.depth();
                (undo > 0, redo > 0)
            }),
        ToolKind::Tags => (app.tags.can_undo(), app.tags.can_redo()),
        ToolKind::Textures => (app.textures.can_undo(), app.textures.can_redo()),
        ToolKind::Hud => app
            .hud
            .session
            .as_ref()
            .map_or((false, false), |s| (s.can_undo(), s.can_redo())),
    }
}

fn name_of(app: &App, t: ToolKind) -> Option<String> {
    match t {
        ToolKind::Scenario => app.loaded_map().map(str::to_owned),
        ToolKind::Variants => app
            .variants
            .as_ref()
            .and_then(|e| e.current())
            .map(|v| variants::panel::strip_icon(&v.name())),
        ToolKind::Playlists => app.hoppers.is_open().then(|| {
            app.hoppers
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "untitled playlists".into())
        }),
        ToolKind::UiScreens => app.ui_screens.pick.as_ref().map(|p| format!("{p:?}")),
        ToolKind::Tags => app.loaded_map().map(str::to_owned),
        ToolKind::Hud => app.hud.session.as_ref().map(|s| match s.hud() {
            Some(h) => format!("{} / {}", s.name, h.name),
            None => s.name.clone(),
        }),
        ToolKind::Scripts => app.scripts_loaded().ok().map(|l| l.map.clone()),
        ToolKind::Textures => app.loaded_map().map(str::to_owned),
    }
}

/// `doc.list`: each tool's document, then each tag tab's.
pub fn list(app: &App) -> Value {
    let tags = app.tabs().iter().filter_map(|tab| match tab.doc {
        Doc::Tag(_) => {
            let (u, r) = app.doc_can(tab.doc);
            Some(json!({
                "doc": app.doc_id(tab.doc),
                "open": true,
                "name": app.doc_title(tab.doc),
                "dirty": app.doc_dirty(tab.doc),
                "can_undo": u,
                "can_redo": r,
                "shown": app.front_doc() == Some(tab.doc) && app.tool == app.doc_tool(tab.doc),
            }))
        }
        _ => None,
    });
    Value::Array(
        ToolKind::ALL
            .into_iter()
            .map(|t| {
                let (u, r) = can(app, t);
                let mut v = json!({
                    "doc": doc_name(t),
                    "open": is_open(app, t),
                    "name": name_of(app, t),
                    "dirty": app.tool_dirty(t),
                    "can_undo": u,
                    "can_redo": r,
                    "shown": app.tool == t,
                });
                let path = match t {
                    ToolKind::Scenario => app.saved_path().map(Path::to_path_buf),
                    ToolKind::Playlists => app.hoppers.path().map(Path::to_path_buf),
                    ToolKind::Hud => app.hud.session.as_ref().and_then(|s| s.saved_path.clone()),
                    _ => None,
                };
                if let Some(p) = path {
                    v["path"] = json!(p.display().to_string());
                }
                v
            })
            .chain(tags)
            .collect(),
    )
}

/// A `doc` param naming an open tag tab (`group:path` or `0x...`).
fn tag_tab(app: &mut App, p: &P) -> Result<Option<Doc>, RpcError> {
    let Some(id) = p.opt_str("doc")? else {
        return Ok(None);
    };
    if parse_doc(id).is_ok() {
        return Ok(None);
    }
    app.sync_tabs();
    Ok(app.find_doc(id).filter(|d| matches!(d, Doc::Tag(_))))
}

fn run(app: &mut App, a: Action, discard: bool) -> Result<Vec<String>, RpcError> {
    app.perform(a, discard).map_err(refused)
}

/// Refuses an output path inside a Git checkout: what is drawn or patched
/// from the user's disc stays out of repositories.
pub fn outside_git(path: &Path) -> Result<(), RpcError> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or(Path::new("."));
    if let Some(repo) = crate::app::ui_screens::git_checkout(dir) {
        return Err(err(
            code::REFUSED,
            format!(
                "{} is inside the Git checkout {}: maps and renders from your disc stay out of repositories; pick a path outside it",
                path.display(),
                repo.display()
            ),
        ));
    }
    Ok(())
}

fn source_entries(entries: Vec<MapEntry<'_>>) -> Vec<Value> {
    entries
        .into_iter()
        .map(|entry| {
            json!({ "map": entry.stem, "label": entry.label, "group": entry.group.key() })
        })
        .collect()
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::SOURCE_OPEN => {
            let path = PathBuf::from(p.str("path")?);
            if !path.exists() {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("{}: no such file or folder", path.display()),
                ));
            }
            let log = run(app, Action::OpenSource(path), p.bool("discard")?)?;
            Ok(json!({ "maps": app.maps(), "loaded": app.loaded_map(), "log": log }))
        }
        m::SOURCE_MAPS => {
            app.poll_cards();
            Ok(json!({
                "source": app.source().map(|s| s.path().display().to_string()),
                "maps": app.maps(),
                "map_entries": source_entries(app.halo2_map_entries()),
                "halo1_maps": source_entries(app.halo1_map_entries()),
                "loaded": app.loaded_map(),
                "map_pack": app.map_pack_entries().into_iter().map(|m| json!({
                    "map": m.map, "label": m.name, "group": MapGroup::MapPack.key(),
                })).collect::<Vec<_>>(),
                "packages": app.packages().iter().map(|e| json!({
                    "key": e.key(), "label": e.label(), "map": e.map, "group": e.map_group().key(),
                })).collect::<Vec<_>>(),
            }))
        }
        m::DOC_LIST => Ok(list(app)),
        m::DOC_OPEN => open(app, p),
        m::DOC_SAVE => save(app, map_for_tags(doc_kind(app, p)?)),
        m::DOC_SAVE_AS => {
            let t = map_for_tags(doc_kind(app, p)?);
            save_as(app, t, PathBuf::from(p.str("path")?))
        }
        m::DOC_DIFF => diff(app, map_for_tags(doc_kind(app, p)?)),
        m::DOC_UNDO | m::DOC_REDO => {
            let undo = method == m::DOC_UNDO;
            if let Some(doc) = tag_tab(app, p)? {
                let before = app.doc_can(doc);
                let action = if undo {
                    Action::UndoDoc(doc)
                } else {
                    Action::RedoDoc(doc)
                };
                let log = app.perform(action, false).map_err(|r| match r {
                    crate::app::Refused::Failed(e) => err(code::REFUSED, e),
                    other => refused(other),
                })?;
                let done = if undo { before.0 } else { before.1 };
                let (u, r) = app.doc_can(doc);
                return Ok(json!({ "done": done, "can_undo": u, "can_redo": r, "log": log }));
            }
            let t = doc_kind(app, p)?;
            let before = can(app, t);
            let action = if undo {
                Action::UndoIn(t)
            } else {
                Action::RedoIn(t)
            };
            let log = run(app, action, false)?;
            let done = if undo { before.0 } else { before.1 };
            let (u, r) = can(app, t);
            Ok(json!({ "done": done, "can_undo": u, "can_redo": r, "log": log }))
        }
        m::DOC_CLOSE => {
            if let Some(doc) = tag_tab(app, p)? {
                run(app, Action::CloseDoc(doc), false)?;
                return Ok(json!({ "closed": true }));
            }
            let t = parse_doc(p.str("doc")?)?;
            if app.tool_dirty(t) && !p.bool("discard")? {
                return Err(err(
                    code::UNSAVED,
                    format!(
                        "{} has unsaved edits: save them, or pass \"discard\": true",
                        doc_name(t)
                    ),
                ));
            }
            app.close_tool(t);
            app.sync_tabs();
            Ok(json!({ "closed": true }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

fn open(app: &mut App, p: &P) -> Reply {
    let discard = p.bool("discard")?;
    let given: Vec<&str> = ["map", "path", "package", "variant", "playlists"]
        .into_iter()
        .filter(|k| p.has(k))
        .collect();
    let [what] = given[..] else {
        return Err(err(
            code::INVALID_PARAMS,
            "doc.open takes one of map, path, package, variant, playlists",
        ));
    };
    let (doc, log) = match what {
        "map" => {
            let name = p.str("map")?;
            if !app.maps().iter().any(|m| m == name) {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("the source has no map {name:?} (source.maps lists them)"),
                ));
            }
            (
                ToolKind::Scenario,
                run(app, Action::SelectMap(name.to_owned()), discard)?,
            )
        }
        "path" => {
            let path = PathBuf::from(p.str("path")?);
            (
                ToolKind::Scenario,
                run(app, Action::OpenSource(path), discard)?,
            )
        }
        "package" => {
            let key = p.str("package")?;
            (
                ToolKind::Scenario,
                run(app, Action::OpenPackageEntry(key.to_owned()), discard)?,
            )
        }
        "variant" => {
            let v = p.get("variant").expect("given");
            let q = P(v);
            let action = if let Some(i) = q.opt_usize("builtin")? {
                app.ensure_variants();
                let n = app.variants.as_ref().map_or(0, |e| e.builtins().len());
                if i >= n {
                    return Err(err(
                        code::INVALID_PARAMS,
                        format!("no built-in variant {i} (0..{n})"),
                    ));
                }
                Action::OpenVariant(Origin::Builtin(i))
            } else if let Some(i) = q.opt_usize("saved")? {
                app.ensure_variants();
                let dir = app
                    .variants
                    .as_ref()
                    .and_then(|e| e.saved().get(i))
                    .map(|s| s.dir.clone())
                    .ok_or_else(|| {
                        err(
                            code::INVALID_PARAMS,
                            format!("no saved variant {i} (variant.list)"),
                        )
                    })?;
                Action::OpenVariant(Origin::Saved(dir))
            } else if let Some(t) = q.opt_str("new")? {
                Action::NewVariant(game_type(t)?)
            } else if let Some(s) = q.opt_str("script")? {
                // A starter by name, or "open" for the script open in the
                // script editor: a new variant of it (`record_for`).
                let source = if s == "open" {
                    app.rules.text.clone()
                } else {
                    crate::app::rules_edit::find_starter(s)
                        .map(|(_, src)| src.to_string())
                        .ok_or_else(|| {
                            err(
                                code::INVALID_PARAMS,
                                format!("no built-in or example script named {s} (rules.get's starters), or \"open\""),
                            )
                        })?
                };
                Action::NewVariantFromScript(source)
            } else {
                return Err(err(
                    code::INVALID_PARAMS,
                    "variant: one of builtin, saved, new, script",
                ));
            };
            (ToolKind::Variants, run(app, action, discard)?)
        }
        "playlists" => {
            let action = match p.str("playlists")? {
                "default" => Action::NewDefaultHopper,
                "blank" => Action::NewHopper,
                path => Action::OpenHopper(PathBuf::from(path)),
            };
            (ToolKind::Playlists, run(app, action, discard)?)
        }
        _ => unreachable!(),
    };
    if !is_open(app, doc) {
        return Err(err(
            code::FAILED,
            format!("{} did not open: {}", doc_name(doc), log.join("; ")),
        ));
    }
    Ok(json!({ "doc": doc_name(doc), "name": name_of(app, doc), "log": log }))
}

pub fn game_type(t: &str) -> Result<GameType, RpcError> {
    let want = t.to_ascii_lowercase().replace([' ', '_', '-'], "");
    GameType::ALL
        .into_iter()
        .find(|g| {
            g.label().to_ascii_lowercase().replace([' ', '_', '-'], "") == want
                || format!("{g:?}").to_ascii_lowercase() == want
        })
        .ok_or_else(|| {
            let names: Vec<String> = GameType::ALL
                .iter()
                .map(|g| format!("{g:?}").to_ascii_lowercase())
                .collect();
            err(
                code::INVALID_PARAMS,
                format!("no game type {t:?} ({})", names.join(", ")),
            )
        })
}

/// The Scripts tool writes nothing (docs/editor/rich-editors.md, slice H3).
const SCRIPTS_READ_ONLY: &str = "the Scripts tool is read only: it saves nothing";

/// Tag edits save and diff with the map: the session writes them.
fn map_for_tags(t: ToolKind) -> ToolKind {
    match t {
        ToolKind::Tags => ToolKind::Scenario,
        t => t,
    }
}

fn need_open(app: &App, t: ToolKind) -> Result<(), RpcError> {
    if is_open(app, t) {
        Ok(())
    } else {
        Err(err(
            code::NO_DOCUMENT,
            format!("no {} is open", doc_name(t)),
        ))
    }
}

fn save(app: &mut App, t: ToolKind) -> Reply {
    need_open(app, t)?;
    match t {
        // The bitmaps are the map's: its save encodes the edited images
        // and writes them with the rest (`App::push_bitmap_changes`).
        ToolKind::Scenario | ToolKind::Tags | ToolKind::Textures => {
            let target = app.save_target();
            let log = run(app, Action::Save, false)?;
            match target {
                SaveTarget::File(path) => {
                    let bytes = std::fs::metadata(&path).map(|m| m.len()).ok();
                    Ok(json!({
                        "saved": true, "path": path.display().to_string(), "bytes": bytes, "log": log,
                    }))
                }
                SaveTarget::Package(dir) => Ok(json!({
                    "saved": true, "path": dir.display().to_string(), "log": log,
                })),
                SaveTarget::EditTarget(Some(id)) => Ok(json!({
                    "saved": true, "edit_target": id, "log": log,
                })),
                SaveTarget::EditTarget(None) => Err(err(
                    code::NO_TARGET,
                    crate::app::edit_target::NO_TARGET,
                )),
                SaveTarget::Nothing => Err(err(code::NO_DOCUMENT, "no map is open")),
            }
        }
        ToolKind::Variants => {
            let log = run(app, Action::SaveVariant, false)?;
            Ok(json!({ "saved": true, "log": log }))
        }
        ToolKind::Playlists => {
            let log = run(app, Action::SaveHopper, false)?;
            Ok(
                json!({ "saved": true, "path": app.hoppers.path().map(|p| p.display().to_string()), "log": log }),
            )
        }
        ToolKind::UiScreens => Err(err(
            code::REFUSED,
            "UI screens have nothing to save: ui.render writes a PNG",
        )),
        ToolKind::Scripts => Err(err(code::REFUSED, SCRIPTS_READ_ONLY)),
        ToolKind::Hud => match app.hud.session.as_ref().and_then(|s| s.saved_path.clone()) {
            Some(path) => save_as(app, t, path),
            None => Err(err(
                code::REFUSED,
                "the HUD edits have no file of their own yet: doc.save_as {\"doc\": \"hud\", \"path\": ...} (the source is never overwritten)",
            )),
        },
    }
}

fn save_as(app: &mut App, t: ToolKind, path: PathBuf) -> Reply {
    need_open(app, t)?;
    let action = match t {
        ToolKind::Hud => return super::hud::save_as(app, path),
        ToolKind::Scenario | ToolKind::Tags | ToolKind::Textures => {
            outside_git(&path)?;
            if let Some(src) = app.session.source()
                && same_file(&src.path, &path)
            {
                return Err(err(
                    code::REFUSED,
                    "that is the map's source: the editor never overwrites it",
                ));
            }
            Action::SaveMapAs(path.clone())
        }
        ToolKind::Playlists => Action::SaveHopperAs(path.clone()),
        ToolKind::Variants => Action::ExportVariant(path.clone()),
        ToolKind::UiScreens => {
            return Err(err(
                code::REFUSED,
                "UI screens save nothing: ui.render writes a PNG",
            ));
        }
        ToolKind::Scripts => return Err(err(code::REFUSED, SCRIPTS_READ_ONLY)),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
    }
    let log = run(app, action, false)?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).ok();
    Ok(json!({ "saved": true, "path": path.display().to_string(), "bytes": bytes, "log": log }))
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn diff(app: &mut App, t: ToolKind) -> Reply {
    need_open(app, t)?;
    let dirty = app.tool_dirty(t);
    match t {
        ToolKind::Scenario | ToolKind::Tags => {
            let s = super::scenario::open(app)?;
            let changes = s.changes();
            let mut patches: Vec<Value> = changes
                .patches
                .iter()
                .map(|(address, bytes)| {
                    let mut v = json!({
                        "address": format!("{address:#010x}"),
                        "len": bytes.len(),
                        "bytes": hex(bytes),
                    });
                    if let Some(b) = s.blocks.iter().find(|b| {
                        let end = b.address as u64 + b.def.stride as u64 * b.capacity() as u64;
                        b.capacity() > 0
                            && (*address as u64) >= b.address as u64
                            && (*address as u64) < end
                    }) {
                        let rel = address - b.address;
                        let (index, off) = (rel / b.def.stride, rel % b.def.stride);
                        let end = off + bytes.len() as u32;
                        v["block"] = json!(block_name(b.def.kind));
                        v["index"] = json!(index);
                        v["fields"] = json!(
                            layout::block(b.def.kind)
                                .fields
                                .iter()
                                .filter(|f| f.offset < end && off < f.offset + f.ty.size())
                                .map(|f| f.name)
                                .collect::<Vec<_>>()
                        );
                    } else if let Some(b) = s
                        .blocks
                        .iter()
                        .find(|b| s.address + b.def.offset == *address)
                    {
                        v["block"] = json!(block_name(b.def.kind));
                        v["fields"] = json!(["count"]);
                    } else if let Some(t) = app.tags.tags().iter().find(|t| {
                        t.bsp.is_none()
                            && *address >= t.address
                            && (*address as u64) < t.address as u64 + t.size as u64
                    }) {
                        // A Tags tool edit.
                        v["tag"] = json!(format!("{}:{}", tags::defs::fourcc_text(t.group), t.name));
                        v["offset"] = json!(format!("{:#x}", address - t.address));
                    }
                    v
                })
                .collect();
            patches.extend(changes.bsp_patches.iter().map(|patch| {
                json!({
                    "address": format!("{:#010x}", patch.address),
                    "bsp_region_offset": patch.space.file_offset,
                    "len": patch.bytes.len(),
                    "bytes": hex(&patch.bytes),
                })
            }));
            let grown: Vec<Value> = changes
                .grown
                .iter()
                .map(|g| json!({"block": block_name(g.kind), "count": g.count, "bytes": g.bytes.len()}))
                .collect();
            Ok(json!({ "doc": "map", "dirty": dirty, "patches": patches, "grown": grown }))
        }
        ToolKind::Playlists => {
            let (Some(now), Some(was)) = (app.hoppers.hoppers(), app.hoppers.baseline()) else {
                return Err(err(code::NO_DOCUMENT, "no playlist file is open"));
            };
            let changed: Vec<Value> = now
                .iter()
                .zip(was)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| {
                    let mut what = Vec::new();
                    if a.title != b.title {
                        what.push("title");
                    }
                    if a.description != b.description {
                        what.push("description");
                    }
                    if a.records != b.records {
                        what.push("entries");
                    }
                    json!({ "playlist": i, "changed": what, "entries": a.records.len(), "was_entries": b.records.len() })
                })
                .collect();
            Ok(json!({ "doc": "playlists", "dirty": dirty, "changed": changed }))
        }
        ToolKind::Variants => {
            let e = app.variants.as_ref().expect("open");
            let v = e.current().expect("open");
            let changed: Vec<Value> = e
                .schema()
                .diff_from_defaults(v)
                .into_iter()
                .map(|(id, default, now)| json!({"setting": id.0, "title": e.schema().title(id), "default": default, "value": now}))
                .collect();
            Ok(json!({ "doc": "variant", "dirty": dirty, "changed_from_defaults": changed }))
        }
        ToolKind::UiScreens => Ok(json!({ "doc": "ui", "dirty": false })),
        ToolKind::Scripts => Ok(json!({ "doc": "scripts", "dirty": false })),
        ToolKind::Textures => Ok(super::bitmaps::diff(app)),
        ToolKind::Hud => super::hud::diff(app),
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
