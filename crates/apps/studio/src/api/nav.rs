//! Scenario inspector navigation over the open map's placement and tag documents.
use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err, params::block_name, scenario, tag_fields};
use crate::app::{Action, App, NavAction, Selection, tag_api};

const STATUS: &str = r#"{"selected":"{block,index}|{tag}|null","tag":"string|null","path":"string|null","breadcrumbs":"[placement or tag]","can_back":"boolean","can_forward":"boolean"}"#;

pub struct Navigation;

impl Provider for Navigation {
    fn methods(&self) -> Vec<MethodDef> {
        vec![
            def(
                m::NAV_STATUS,
                "The Scenario inspector's selection, tag, field and history",
                "{}",
                STATUS,
                "{}",
            ),
            def(
                m::NAV_BACK,
                "Go back in inspector history",
                "{}",
                STATUS,
                "{}",
            ),
            def(
                m::NAV_FORWARD,
                "Go forward in inspector history",
                "{}",
                STATUS,
                "{}",
            ),
            def(
                m::NAV_GO,
                "Show a tag or field in the Scenario inspector and keep the viewport selection",
                r#"{"tag":"string|integer","path?":"string"}"#,
                STATUS,
                r#"{"tag":"itmc"}"#,
            ),
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        let action = match method {
            m::NAV_STATUS => return Some(Ok(status(app))),
            m::NAV_BACK => Ok(NavAction::Back),
            m::NAV_FORWARD => Ok(NavAction::Forward),
            m::NAV_GO => open_tag(app, &p).and_then(|tag| {
                Ok(NavAction::Go {
                    tag,
                    path: p.opt_str("path")?.map(str::to_owned),
                })
            }),
            _ => return None,
        };
        Some(action.and_then(|action| {
            app.perform(Action::Navigate(action), false)
                .map_err(scenario::refused)?;
            Ok(status(app))
        }))
    }
}

pub(super) fn open_tag(app: &mut App, p: &P) -> Result<TagId, agent_proto::RpcError> {
    tag_fields::sync_open_tags(app)?;
    tag_api::lookup(&app.tags, p.get("tag").unwrap_or(&Value::Null))
        .map_err(|e| err(code::INVALID_PARAMS, e))
}

pub(super) fn selection_json(selection: Option<Selection>) -> Value {
    match selection {
        Some(Selection::Placement(kind, index)) => {
            json!({"block": block_name(kind), "index": index})
        }
        Some(Selection::Tag(id)) => json!({"tag": scenario::tag_id(id)}),
        None => Value::Null,
    }
}

fn status(app: &App) -> Value {
    let location = app.nav_location();
    let selected = location.selection();
    let mut breadcrumbs = Vec::new();
    if matches!(selected, Some(Selection::Placement(..))) {
        breadcrumbs.push(selection_json(selected));
    }
    breadcrumbs.extend(location.trail.iter().map(|&id| {
        let entry = app.tags.tag(id);
        json!({
            "tag": scenario::tag_id(id),
            "group": entry.map(|t| scenario::group_text(t.group)),
            "name": entry.map(|t| &t.name),
        })
    }));
    json!({
        "selected": selection_json(selected),
        "tag": location.tag().map(scenario::tag_id),
        "path": location.path,
        "breadcrumbs": breadcrumbs,
        "can_back": app.can_navigate(false),
        "can_forward": app.can_navigate(true),
    })
}
