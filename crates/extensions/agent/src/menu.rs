//! The debug menu on the agent (docs/runtime/debug-menu.md): the menu is
//! an in-process connection ([`connect`]), so every row it shows is an
//! agent answer and every change an agent call, answered on the game's
//! thread like any client's. What `h2 agent` can read or change, the menu
//! can, and the other way round.
//!
//! Also the host methods the menu added beside `render.*` and `edit.*`
//! (names in `debug_menu::sources::method`):
//!
//! * `settings.list {prefix?}` and `settings.set {key, value}` over the
//!   settings store (`settings.toml`): each setting with how it applies
//!   (`live`, `next_map`, `restart`) and what locks it;
//! * `perf.overlay {on?}`: the perf overlay (the watermark) on or off.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};

use agent_proto::{RpcError, code};
use debug_menu::sources::method;
use debug_menu::{Call, Rpc, Shared};
use serde_json::{Map, Value, json};
use settings::{Apply, Def, Kind, Scope, Store};

use crate::{Access, Agent, ConnId};

/// Lines a connection may have queued before the agent drops it; the menu
/// drains them whenever it draws or reads input.
const QUEUE: usize = 1024;

/// The menu's connection to the agent.
pub struct MenuRpc {
    agent: Agent,
    conn: ConnId,
    rx: Receiver<String>,
}

impl MenuRpc {
    pub fn new(agent: &Agent) -> Self {
        let (tx, rx) = sync_channel(QUEUE);
        MenuRpc {
            agent: agent.clone(),
            conn: agent.connect(tx),
            rx,
        }
    }
}

impl Rpc for MenuRpc {
    fn send(&mut self, id: u64, call: &Call) {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": call.method,
            "params": call.params,
        });
        self.agent.submit(self.conn, &line.to_string());
    }

    fn recv(&mut self) -> Vec<(u64, Result<Value, String>)> {
        self.rx.try_iter().filter_map(|l| answer(&l)).collect()
    }
}

/// A response line as (id, result); events and other ids are skipped.
pub fn answer(line: &str) -> Option<(u64, Result<Value, String>)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let id = v.get("id")?.as_u64()?;
    if let Some(e) = v.get("error") {
        let msg = e["message"].as_str().unwrap_or("error").to_owned();
        return Some((id, Err(msg)));
    }
    Some((id, Ok(v.get("result").cloned().unwrap_or(Value::Null))))
}

/// Connects `menu` to `agent`.
pub fn connect(menu: &Shared, agent: &Agent) {
    menu.connect(Box::new(MenuRpc::new(agent)));
}

fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, m)
}

fn obj(p: &Value) -> Result<Map<String, Value>, RpcError> {
    match p {
        Value::Null => Ok(Map::new()),
        Value::Object(o) => Ok(o.clone()),
        _ => Err(invalid("params must be an object")),
    }
}

/// A setting as `settings.list` lists it.
pub fn setting_json(store: &Store, def: &Def) -> Value {
    let state = store.state(def.key);
    let value = |v: settings::Value| match v {
        settings::Value::Bool(b) => json!(b),
        settings::Value::Int(i) => json!(i),
    };
    let mut o = json!({
        "key": def.key,
        "label": def.label,
        "help": def.help,
        "apply": match def.apply {
            Apply::Live => "live",
            Apply::NextMap => "next_map",
            Apply::Restart => "restart",
        },
        "scope": match def.scope {
            Scope::Host => "host",
            Scope::View => "view",
            Scope::Sim => "sim",
        },
    });
    match &def.kind {
        Kind::Bool => o["kind"] = json!("bool"),
        Kind::Int { valid, steps, .. } => {
            o["kind"] = json!("int");
            o["min"] = json!(valid.start());
            o["max"] = json!(valid.end());
            o["steps"] = steps
                .iter()
                .map(|s| json!({ "value": s, "label": def.label(settings::Value::Int(*s)) }))
                .collect();
        }
        Kind::Choice(c) => {
            o["kind"] = json!("choice");
            o["choices"] = c
                .iter()
                .map(|c| json!({ "value": c.value, "name": c.name, "label": c.label }))
                .collect();
        }
    }
    if let Some(s) = state {
        o["value"] = value(s.shown);
        o["running"] = value(s.value);
        o["pending"] = json!(s.pending);
        if s.locked {
            o["locked"] = json!(s.source.to_string());
        }
    }
    o
}

/// A JSON value as `def` holds it: a bool, a whole number, or a choice's
/// number or name.
pub fn setting_value(def: &Def, v: &Value) -> Result<settings::Value, String> {
    let parsed = match (&def.kind, v) {
        (Kind::Bool, Value::Bool(b)) => settings::Value::Bool(*b),
        (Kind::Int { .. } | Kind::Choice(_), Value::Number(n)) => settings::Value::Int(
            n.as_i64()
                .ok_or_else(|| format!("{n} is not a whole number"))?,
        ),
        (_, Value::String(s)) => def.parse(s)?,
        _ => return Err(format!("{}: expected {}", def.key, def.expected())),
    };
    def.check(parsed)
}

/// Adds `settings.list` and `settings.set` over `store`.
pub fn add_settings_methods(agent: &Agent, store: Arc<Store>) {
    let s = Arc::clone(&store);
    agent.add_method(method::SETTINGS_LIST, Access::Read, move |p| {
        let o = obj(&p)?;
        let prefix = match o.get("prefix") {
            None | Some(Value::Null) => "",
            Some(Value::String(x)) => x.as_str(),
            Some(_) => return Err(invalid("prefix: a string (\"graphics.\")")),
        };
        let list: Vec<Value> = s
            .defs()
            .into_iter()
            .filter(|d| d.key.starts_with(prefix))
            .map(|d| setting_json(&s, d))
            .collect();
        Ok(json!({ "settings": list }))
    });
    agent.add_method(method::SETTINGS_SET, Access::Act, move |p| {
        let o = obj(&p)?;
        let key = o
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("key: the setting (settings.list lists them)"))?;
        let def = store
            .def(key)
            .ok_or_else(|| invalid(format!("no setting {key:?}")))?;
        let v = o
            .get("value")
            .ok_or_else(|| invalid("value: what to set"))?;
        let v = setting_value(def, v).map_err(invalid)?;
        let out = store.set(key, v).map_err(|e| invalid(e.to_string()))?;
        if out.state.locked {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                format!("{key}: set by {} for this run", out.state.source),
            ));
        }
        let mut r = setting_json(&store, def);
        r["changed"] = json!(out.changed);
        r["said"] = json!(out.describe(def));
        if let Some(why) = out.not_saved {
            r["not_saved"] = json!(why);
        }
        Ok(r)
    });
}

/// Adds `perf.overlay` over the overlay's switch.
pub fn add_perf_overlay(agent: &Agent, on: Arc<AtomicBool>) {
    agent.add_method(method::PERF_OVERLAY, Access::Act, move |p| {
        let o = obj(&p)?;
        match o.get("on") {
            None | Some(Value::Null) => {}
            Some(Value::Bool(b)) => on.store(*b, Ordering::Relaxed),
            Some(other) => return Err(invalid(format!("on: {other} is not true or false"))),
        }
        Ok(json!({ "on": on.load(Ordering::Relaxed) }))
    });
}

#[cfg(all(test, feature = "render-overrides"))]
mod tests;
