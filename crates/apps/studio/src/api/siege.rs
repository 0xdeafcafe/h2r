//! `siege.layout`: the Siege layout recipe over the open map
//! (`forge_doc::siege_layout`; docs/game/rules-engine.md, "Siege layouts";
//! docs/editor/agent-api.md, "Siege layouts"). It plans from the map as
//! edited (its CTF flag stands, territories and starting locations) and
//! answers the labels-only document `h2 siege layout` writes, and with
//! `out` writes it there. Nothing in the open map changes.

use forge_doc::BlockKind;
use forge_doc::siege_layout::{self, Barrier, Recipe, SiegeFlag, SiegeMap};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;

pub struct Siege;

impl Provider for Siege {
    fn methods(&self) -> Vec<MethodDef> {
        vec![def(
            m::SIEGE_LAYOUT,
            "Plan a Siege layout over the open map as edited, as `h2 siege layout` does: the attack line from the attackers' CTF flag stand (`attack` red, the default, or blue) to the defenders', the territories between them as sites two a phase, the defenders' stand as the siege_goal, and each phase's starting locations labelled siege_attack and siege_defend (`per_side` a side, default 6). `barriers` name objects that open in a phase (`crates:22=2`). Answers the plan and the labels-only document; `out` also writes the document there. The open map is not changed",
            r#"{"attack?":"red|blue","per_side?":"integer","barriers?":"[string]","out?":"string (a path)"}"#,
            r#"{"sites":"[{index,phase}]","goal":"integer","attack":"[{index,phase}]","defend":"[{index,phase}]","barriers":"[{block,index,phase}]","phases":"integer","document":"object","written":"string|null"}"#,
            r#"{"attack":"red","per_side":1}"#,
        )]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        (method == m::SIEGE_LAYOUT).then(|| layout(app, &P(params)))
    }
}

fn invalid(e: impl Into<String>) -> agent_proto::RpcError {
    err(code::INVALID_PARAMS, e.into())
}

fn layout(app: &mut App, p: &P) -> Reply {
    let mut recipe = Recipe::default();
    match p.opt_str("attack")? {
        None | Some("red") => {}
        Some("blue") => recipe.attack_team = 1,
        Some(other) => return Err(invalid(format!("attack is red or blue, not {other:?}"))),
    }
    if let Some(n) = p.opt_usize("per_side")? {
        recipe.per_side = n;
    }
    if let Some(list) = p.get("barriers") {
        let list = list.as_array().ok_or_else(|| invalid("barriers is a list of <block>:<index>=<phase>"))?;
        for b in list {
            let text = b.as_str().ok_or_else(|| invalid("barriers is a list of strings"))?;
            recipe.barriers.push(Barrier::parse(text).map_err(invalid)?);
        }
    }
    let out = p.opt_str("out")?.map(std::path::PathBuf::from);
    let scenario = app
        .session
        .scenario()
        .ok_or_else(|| err(code::NO_DOCUMENT, "no map is open"))?;
    let name = app.session.map_name().unwrap_or("map").to_ascii_lowercase();
    let map = map_of(scenario).map_err(|e| err(code::FAILED, e))?;
    let plan = siege_layout::plan(&map, &recipe).map_err(|e| err(code::REFUSED, e))?;
    let description = format!("Siege on {name}, labels only, planned in Studio.");
    let doc = siege_layout::document(&name, &plan, &description);
    let written = match out {
        Some(path) => {
            std::fs::write(&path, siege_layout::to_text(&doc))
                .map_err(|e| err(code::FAILED, format!("writing {}: {e}", path.display())))?;
            Some(path.display().to_string())
        }
        None => None,
    };
    let pairs = |list: &[(u32, i32)]| list.iter().map(|&(i, n)| json!({"index": i, "phase": n})).collect::<Vec<_>>();
    Ok(json!({
        "sites": pairs(&plan.sites),
        "goal": plan.goal,
        "attack": pairs(&plan.attack),
        "defend": pairs(&plan.defend),
        "barriers": plan.barriers.iter().map(|b| json!({"block": b.block.name(), "index": b.index, "phase": b.phase})).collect::<Vec<_>>(),
        "phases": plan.phases,
        "document": doc,
        "written": written,
    }))
}

/// The rows the recipe reads, from the open map's scenario as edited.
fn map_of(s: &scenario_edit::Scenario) -> Result<SiegeMap, String> {
    let flags = BlockKind::NetgameFlags;
    let starts = BlockKind::StartingLocations;
    let get = |kind: BlockKind, i: usize, field: &str| s.get(kind, i, field).map_err(|e| format!("{} {i}: {e}", kind.name()));
    let mut map = SiegeMap::default();
    for i in 0..s.block(flags).len() {
        map.flags.push(SiegeFlag {
            position: get(flags, i, "position")?.as_vec3().unwrap_or_default(),
            kind: get(flags, i, "type")?.as_int().unwrap_or(-1) as u16,
            team: get(flags, i, "team")?.as_int().unwrap_or(-1) as u16,
        });
    }
    for i in 0..s.block(starts).len() {
        map.starts.push(get(starts, i, "position")?.as_vec3().unwrap_or_default());
    }
    for kind in forge_doc::SOLID_BLOCKS {
        map.counts.insert(kind, s.block(kind).len() as u32);
    }
    Ok(map)
}
