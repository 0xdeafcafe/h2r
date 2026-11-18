//! The studio-tags registry as the agent API's `tag.read`, `tag.set` and
//! `tag.block.*` ([`super::Provider`], registered in [`super::seams`]):
//! every field of any tag by its label, edited through the Tags tool's own
//! commands (`app::tag_api`, applied with `Action::Tag`), so an agent's
//! edit is the same undo step and the same saved patch as the window's.
//!
//! The scenario tag stays with the built-in methods: its placement blocks
//! are the Scenario tool's (`scenario.*`, `tag.set` paths
//! `<block>[<i>].<field>`), and the Tags tool reads it only.

use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{METHODS, MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::tag_api;
use crate::app::{Action, BlockOp};

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
        let p = P(params);
        if method == m::TAG_READ && p.bool("raw").unwrap_or(false) {
            return None;
        }
        let id = match resolve(app, &p) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
        // The scenario tag's edits are the Scenario tool's.
        if method != m::TAG_READ && app.session.scenario().is_some_and(|s| s.tag == id) {
            return None;
        }
        Some(self.run(app, method, &p, id))
    }
}

/// The `tag` param: `group:name` as the Tags tool writes it, else as the
/// built-in methods take it.
fn resolve(app: &mut App, p: &P) -> Result<TagId, agent_proto::RpcError> {
    super::scenario::open(app)?;
    app.sync_tags();
    if let Some(e) = &app.tags.error {
        return Err(err(code::FAILED, format!("the tag editor could not read the map: {e}")));
    }
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
            m::TAG_READ => tag_api::read_tag(&app.tags, id).map_err(failed),
            m::TAG_SET => {
                let path = p.str("path")?;
                let value = p.get("value").cloned().unwrap_or(Value::Null);
                let action = tag_api::set_field(&app.tags, id, path, &value)
                    .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))?;
                perform(app, action)?;
                let (at, _, f) = tag_api::resolve(&app.tags, id, path).map_err(failed)?;
                Ok(json!({ "value": tag_api::value(&app.tags, at, &f) }))
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
                let action = tag_api::block(&app.tags, id, path, op)
                    .map_err(|e| err(code::REFUSED, format!("{path}: {e}")))?;
                perform(app, action)?;
                let count = tag_api::resolve(&app.tags, id, path)
                    .ok()
                    .and_then(|(at, _, f)| tag_api::value(&app.tags, at, &f).as_array().map(Vec::len));
                Ok(json!({ "count": count }))
            }
        }
    }
}

/// Applies a Tags tool command the way the window does, with its outcome.
fn perform(app: &mut App, action: crate::app::TagAction) -> Result<(), agent_proto::RpcError> {
    app.perform(Action::Tag(action), false).map_err(|r| match r {
        crate::app::Refused::Failed(e) => err(code::REFUSED, e),
        other => refused(other),
    })?;
    // End the step: the next call is its own undo step.
    let _ = app.perform(Action::Tag(crate::app::TagAction::Seal), false);
    Ok(())
}
