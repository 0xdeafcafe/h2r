//! `variant.*`: the game variant tool's open variant, through its actions.
//! Opening one is `doc.open {"variant": ...}`; saving is `doc.save`.

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use variants::SettingId;
use variants::VariantEditor;
use variants::panel::{OptionKind, Origin, script_view, strip_icon};

use super::scenario::refused;
use super::{P, Reply, err};
use crate::App;
use crate::app::{Action, rules_edit};

fn editor(app: &mut App) -> &VariantEditor {
    app.ensure_variants();
    app.variants.as_ref().expect("built")
}

fn need_open(app: &mut App) -> Result<(), RpcError> {
    match editor(app).current() {
        Some(_) => Ok(()),
        None => Err(err(
            code::NO_DOCUMENT,
            "no variant is open: doc.open {\"variant\": {\"builtin\": 0}} (or saved, or new)",
        )),
    }
}

/// This machine's map variants a game variant may suggest
/// (`forge::files::variants::suggestable` under the data root).
fn suggestable(app: &App) -> Result<Vec<forge::files::variants::Suggestable>, RpcError> {
    forge::files::variants::suggestable(&app.data_root()).map_err(|e| err(code::FAILED, e.to_string()))
}

/// The suggestions for map variant `ids` (document ids), in the order
/// given: each must be on this machine, or the id is refused by name.
fn suggestions(app: &App, ids: &[String]) -> Result<Vec<variants::portable::MapSuggestion>, RpcError> {
    let list = suggestable(app)?;
    ids.iter()
        .map(|id| {
            list.iter()
                .find(|s| s.id.to_string() == *id)
                .map(|s| variants::portable::MapSuggestion {
                    base: s.base.to_string(),
                    variant: s.id.to_string(),
                    name: s.name.clone(),
                })
                .ok_or_else(|| {
                    err(code::INVALID_PARAMS, format!("no map variant {id} on this machine (variant.maps lists them)"))
                })
        })
        .collect()
}

fn origin(o: Option<&Origin>) -> Value {
    match o {
        Some(Origin::Builtin(i)) => json!({ "builtin": i }),
        Some(Origin::Saved(d)) => json!({ "saved": d.display().to_string() }),
        Some(Origin::New) => json!("new"),
        None => Value::Null,
    }
}

fn run(app: &mut App, a: Action) -> Result<Vec<String>, RpcError> {
    app.perform(a, false).map_err(|r| match r {
        crate::app::Refused::Failed(e) if e.contains("built-in") || e.contains("read") => {
            err(code::REFUSED, e)
        }
        other => refused(other),
    })
}

/// The Script section of the open variant, as `variant.get` reports it:
/// `null` when the record names no script, `{prefix, found: false}` when
/// this editor lacks it, else its name, description, options with their
/// values and the Quick Options rows (`variants::panel::script_view`).
fn script_json(app: &App) -> Value {
    let Some(e) = app.variants.as_ref() else {
        return Value::Null;
    };
    let (Some(prefix), Some(v)) = (e.script_prefix(), e.current()) else {
        return Value::Null;
    };
    let Some(ir) = rules_edit::find_script(prefix, &app.rules) else {
        return json!({ "prefix": rules_edit::hex_prefix(prefix), "found": false });
    };
    let view = script_view(&ir, v);
    json!({
        "prefix": rules_edit::hex_prefix(prefix),
        "found": true,
        "name": view.name,
        "about": view.about,
        "scoring_carrier": view.carrier_scoring,
        "options": view.options.iter().map(|o| json!({
            "index": o.index,
            "name": o.name,
            "label": o.label,
            "description": o.description,
            "kind": match o.kind {
                OptionKind::Bool => "bool",
                OptionKind::Int { .. } => "int",
                OptionKind::Enum { .. } => "enum",
            },
            "value": o.value,
            "text": o.text(o.value),
            "default": o.default,
            "quick": o.quick,
        })).collect::<Vec<_>>(),
        "quick": view.marks.iter().map(|m| json!({
            "label": m.label, "option": m.option, "text": m.text,
        })).collect::<Vec<_>>(),
    })
}

/// `variant.set {option, value}`: one option of the script the open
/// variant names, by its name in the script, as one undo step.
fn set_option(app: &mut App, name: &str, value: i32) -> Reply {
    let (read_only, prefix) = {
        let e = editor(app);
        (e.is_read_only(), e.script_prefix())
    };
    if read_only {
        return Err(err(
            code::REFUSED,
            "built-in variants cannot be edited: variant.copy {\"name\": ...} first",
        ));
    }
    let Some(prefix) = prefix else {
        return Err(err(
            code::INVALID_PARAMS,
            "the open variant names no script (variant.get's script is null)",
        ));
    };
    let Some(ir) = rules_edit::find_script(prefix, &app.rules) else {
        return Err(err(
            code::INVALID_PARAMS,
            format!(
                "this editor does not have the script {} (open it with rules.open)",
                rules_edit::hex_prefix(prefix)
            ),
        ));
    };
    let Some(index) = (0..ir.options.len()).find(|&i| ir.option_name(i) == name) else {
        let names: Vec<&str> = (0..ir.options.len()).map(|i| ir.option_name(i)).collect();
        return Err(err(
            code::INVALID_PARAMS,
            format!("no option {name} (the script has: {})", names.join(", ")),
        ));
    };
    run(app, Action::SetScriptOption(index, value))?;
    let v = editor(app).current().expect("open");
    let view = script_view(&ir, v);
    let o = &view.options[index];
    Ok(json!({ "option": name, "value": o.value, "text": o.text(o.value) }))
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::VARIANT_LIST => {
            let e = editor(app);
            Ok(json!({
                "builtins": e.builtins().iter().enumerate().map(|(i, v)| json!({
                    "index": i, "name": strip_icon(&v.name()), "type": v.game_type().label(),
                })).collect::<Vec<_>>(),
                "saved": e.saved().iter().enumerate().map(|(i, s)| json!({
                    "index": i, "name": s.contents.variant.name(), "save_name": s.save_name,
                    "dir": s.dir.display().to_string(),
                })).collect::<Vec<_>>(),
                "open": e.current().map(|v| json!({ "name": strip_icon(&v.name()), "origin": origin(e.origin()) })),
            }))
        }
        m::VARIANT_GET => {
            need_open(app)?;
            let dirty = app.tool_dirty(crate::ToolKind::Variants);
            let script = script_json(app);
            let e = editor(app);
            let v = e.current().expect("open");
            let schema = e.schema();
            let settings: Vec<Value> = v
                .known_settings()
                .into_iter()
                .filter(|(id, _)| variants::schema::applies(*id, v.game_type()))
                .map(|(id, value)| {
                    json!({
                        "id": id.0,
                        "title": schema.title(id),
                        "value": value,
                        "label": e.value_text(id, value),
                        "choices": schema.settings.get(&id).map(|d| d.choices.iter().map(|c| json!({"value": c.value, "label": c.label})).collect::<Vec<_>>()),
                    })
                })
                .collect();
            Ok(json!({
                "name": strip_icon(&v.name()),
                "type": v.game_type().label(),
                "origin": origin(e.origin()),
                "read_only": e.is_read_only(),
                "dirty": dirty,
                "settings": settings,
                "issues": e.issues().iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                "script": script,
            }))
        }
        m::VARIANT_SET => {
            need_open(app)?;
            if let Some(name) = p.opt_str("option")? {
                let value = i32::try_from(p.i64("value")?)
                    .map_err(|_| err(code::INVALID_PARAMS, "value: out of range"))?;
                return set_option(app, name, value);
            }
            let id = SettingId(
                u32::try_from(p.u64("setting")?)
                    .map_err(|_| err(code::INVALID_PARAMS, "setting: a setting id"))?,
            );
            let value = i32::try_from(p.i64("value")?)
                .map_err(|_| err(code::INVALID_PARAMS, "value: out of range"))?;
            let e = editor(app);
            if e.is_read_only() {
                return Err(err(
                    code::REFUSED,
                    "built-in variants cannot be edited: variant.copy {\"name\": ...} first",
                ));
            }
            let Some(d) = e.schema().settings.get(&id) else {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("no setting {} (variant.get lists them)", id.0),
                ));
            };
            if !d.choices.is_empty() && !d.choices.iter().any(|c| c.value == value) {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!(
                        "{}: {value} is not a choice ({})",
                        d.title,
                        d.choices
                            .iter()
                            .map(|c| format!("{} {}", c.value, c.label))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            run(app, Action::SetSetting(id, value))?;
            let e = editor(app);
            let now = e.current().and_then(|v| v.get(id)).unwrap_or(value);
            Ok(json!({ "setting": id.0, "value": now, "label": e.value_text(id, now) }))
        }
        m::VARIANT_RENAME => {
            need_open(app)?;
            let name = p.str("name")?.to_owned();
            run(app, Action::SetVariantName(name))?;
            Ok(json!({ "name": editor(app).current().map(|v| v.name()) }))
        }
        m::VARIANT_COPY => {
            need_open(app)?;
            let name = p.str("name")?.to_owned();
            run(app, Action::CopyBuiltin(name))?;
            Ok(json!({ "name": editor(app).current().map(|v| v.name()) }))
        }
        m::VARIANT_EXPORT => {
            need_open(app)?;
            let path = std::path::PathBuf::from(p.str("path")?);
            // As doc.save_as: the folder is made.
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)
                    .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
            }
            let ids: Vec<String> = match p.0.get("maps") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(|| err(code::INVALID_PARAMS, "maps takes variant ids (strings)"))?,
                Some(_) => return Err(err(code::INVALID_PARAMS, "maps takes a list of variant ids")),
            };
            let maps = if ids.is_empty() {
                Vec::new()
            } else {
                suggestions(app, &ids)?
            };
            let n = maps.len();
            let action = if maps.is_empty() {
                Action::ExportVariant(path.clone())
            } else {
                Action::ExportVariantWith(path.clone(), maps)
            };
            run(app, action)?;
            Ok(json!({ "path": path.display().to_string(), "maps": n }))
        }
        m::VARIANT_MAPS => {
            let list = suggestable(app)?;
            Ok(json!({ "maps": list.iter().map(|s| json!({
                "map": s.map,
                "name": s.name,
                "base": s.base.to_string(),
                "variant": s.id.to_string(),
                "source": match s.source {
                    forge::files::variants::Source::Saved => "saved",
                    forge::files::variants::Source::Live => "live",
                },
            })).collect::<Vec<_>>() }))
        }
        m::VARIANT_IMPORT => {
            let action = match (p.opt_str("path")?, p.opt_str("text")?) {
                (Some(path), None) => Action::ImportVariantFile(path.into()),
                (None, Some(text)) => Action::ImportVariantText(text.to_owned()),
                _ => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        "variant.import takes path or text",
                    ));
                }
            };
            app.ensure_variants();
            app.perform(action, p.bool("discard")?).map_err(refused)?;
            Ok(json!({ "name": editor(app).current().map(|v| v.name()) }))
        }
        m::VARIANT_DELETE => {
            need_open(app)?;
            run(app, Action::DeleteVariant)?;
            Ok(json!({ "deleted": true }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
