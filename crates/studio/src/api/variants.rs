//! `variant.*`: the game variant tool's open variant, through its actions.
//! Opening one is `doc.open {"variant": ...}`; saving is `doc.save`.

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use variants::SettingId;
use variants::VariantEditor;
use variants::panel::{Origin, strip_icon};

use super::scenario::refused;
use super::{P, Reply, err};
use crate::App;
use crate::app::Action;

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
            }))
        }
        m::VARIANT_SET => {
            need_open(app)?;
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
            run(app, Action::ExportVariant(path.clone()))?;
            Ok(json!({ "path": path.display().to_string() }))
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
