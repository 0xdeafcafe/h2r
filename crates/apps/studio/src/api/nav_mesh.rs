//! `nav.pathfinding`, `nav.bots` and `nav.pick`: the Navigation layers'
//! data ([`scenario_edit::nav_view`]; docs/editor/editor.md, "Navigation").
//! Read only, and independent of the layers shown: an agent can ask about
//! a sector or a link the viewport hides. `nav.bots` starts the bots'
//! load ([`crate::app::bot_nav`]) when nothing has asked for it yet.

use agent_proto::RpcError;
use scenario_edit::geometry::{Ray, normalize};
use scenario_edit::layers::LayerId;
use scenario_edit::nav_view::{
    BotLinkKind, BotNav, LevelPathfinding, NavPick, Sector, SpotKind, SpotState,
};
use serde_json::{Map, Value, json};
use studio_proto::{code, method as m};

use super::scenario::{open, refused, tag_id};
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::Action;
use crate::app::bot_nav::BotNavState;

/// How far (world units) `nav.pick`'s point may be from what it names by
/// default.
const PICK_WITHIN: f32 = 0.3;

pub struct NavMesh;

impl Provider for NavMesh {
    fn methods(&self) -> Vec<MethodDef> {
        vec![
            def(
                m::NAV_PATHFINDING,
                "Each structure BSP's own AI pathfinding data (sbsp+0xc4): whether it has any, its sector, link, 2D node, vertex and ref counts, sectors whose ring does not close, and the sectors (index, flags, hint, links, ring). A map with none (retail multiplayer maps) answers empty with a note. Filter by BSP; offset and limit (200) page the sectors",
                r#"{"bsp?":"integer","offset?":"integer","limit?":"integer"}"#,
                r#"{"bsps":"[{bsp,tag,present,sectors,links,nodes,vertices,refs,orphan_links,broken}]","counts":"{sectors,links}","note":"string|null","matching":"integer","sectors":"[{index,bsp,sector,flags,hint,floor,links,ring,center}]","selected":"{kind,index}|null","errors":"[{bsp,error}]"}"#,
                r#"{"limit":10}"#,
            ),
            def(
                m::NAV_BOTS,
                "The bots' nav mesh and superbounce spots for the open map: read from <data>/bots (built in memory when the cache is missing or stale), starting the load if nothing has; wait blocks until it is done. Counts, links by kind, spots by kind and state, where each BSP's mesh came from, notes and errors. polys, links and spots list them (offset and limit, 200, page each)",
                r#"{"wait?":"boolean","polys?":"boolean","links?":"boolean","spots?":"boolean","offset?":"integer","limit?":"integer"}"#,
                r#"{"state":"no_map|idle|loading|ready|failed","map":"string|null","origins":"[string]","counts":"{polys,links,walls,spots}","links_by_kind":"{walk,drop,...}","spots_by_kind":"{bounce,strip,crouch}","spots_by_state":"{proven,failed,dead,learned,training,likely}","notes":"[string]","error":"string|null","polys?":"[{index,bsp,poly,center,verts}]","links?":"[{index,bsp,link,kind,from,to,a,b,one_way}]","spots?":"[{index,kind,source,state,stand,...}]","selected":"{kind,index}|null"}"#,
                r#"{"wait":true}"#,
            ),
            def(
                m::NAV_PICK,
                "The navigation thing a ray meets (origin and dir) or a point names (within 0.3 units): a bot spot or link first, else the nearer bot polygon or pathfinding sector, whatever layers are shown unless shown is set; layer limits it to pathfinding, bot_nav or bot_spots. select selects it (or clears the selection on a miss); kind and index select one directly",
                r#"{"origin?":"[x,y,z]","dir?":"[x,y,z]","point?":"[x,y,z]","within?":"number","layer?":"pathfinding|bot_nav|bot_spots","shown?":"boolean","select?":"boolean","kind?":"sector|poly|link|spot","index?":"integer","focus?":"boolean"}"#,
                r#"{"hit":"{kind,index,...}|null","distance":"number|null","selected":"{kind,index}|null"}"#,
                r#"{"point":[0,0,0]}"#,
            ),
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::NAV_PATHFINDING => pathfinding(app, &p),
            m::NAV_BOTS => bots(app, &p),
            m::NAV_PICK => pick(app, &p),
            _ => return None,
        })
    }
}

fn pick_json(pick: Option<NavPick>) -> Value {
    pick.map_or(
        Value::Null,
        |p| json!({"kind": p.kind(), "index": p.index()}),
    )
}

fn path_data(app: &App) -> Result<&LevelPathfinding, RpcError> {
    open(app)?;
    app.session
        .pathfinding()
        .ok_or_else(|| err(code::NO_DOCUMENT, "no map is open"))
}

pub fn sector_row(index: usize, s: &Sector) -> Value {
    json!({
        "index": index,
        "bsp": s.bsp,
        "sector": s.sector,
        "flags": s.flags,
        "hint": (s.hint != 0xffff).then_some(s.hint),
        "floor": s.floor(),
        "links": s.links,
        "ring": s.ring,
        "center": s.center(),
    })
}

fn pathfinding(app: &mut App, p: &P) -> Reply {
    let bsp = p.opt_usize("bsp")?;
    let offset = p.opt_usize("offset")?.unwrap_or(0);
    let limit = p.opt_usize("limit")?.unwrap_or(200);
    let l = path_data(app)?;
    let matching: Vec<(usize, &Sector)> = l
        .sectors
        .iter()
        .enumerate()
        .filter(|(_, s)| bsp.is_none_or(|b| s.bsp == b))
        .collect();
    Ok(json!({
        "bsps": l.bsps.iter().map(|b| json!({
            "bsp": b.bsp,
            "tag": tag_id(b.tag),
            "present": b.present,
            "sectors": b.sectors,
            "links": b.links,
            "nodes": b.nodes,
            "vertices": b.vertices,
            "refs": b.refs,
            "orphan_links": b.orphan_links,
            "broken": b.broken,
        })).collect::<Vec<_>>(),
        "counts": {"sectors": l.sectors.len(), "links": l.links.len()},
        "note": l.note(),
        "matching": matching.len(),
        "sectors": matching.iter().skip(offset).take(limit)
            .map(|&(i, s)| sector_row(i, s)).collect::<Vec<_>>(),
        "selected": pick_json(app.session.selected_nav()),
        "errors": l.errors.iter()
            .map(|(bsp, e)| json!({"bsp": bsp, "error": e}))
            .collect::<Vec<_>>(),
    }))
}

fn counts<T: Copy>(all: &[T], name: fn(T) -> &'static str, n: impl Fn(T) -> usize) -> Value {
    Value::Object(
        all.iter()
            .map(|&k| (name(k).to_owned(), json!(n(k))))
            .collect::<Map<_, _>>(),
    )
}

fn poly_row(i: usize, b: &BotNav) -> Value {
    let p = &b.polys[i];
    json!({
        "index": i, "bsp": p.bsp, "poly": p.poly, "center": p.center(), "verts": p.verts,
        "out_of_map": p.out_of_map,
    })
}

fn link_row(i: usize, b: &BotNav) -> Value {
    let l = &b.links[i];
    json!({
        "index": i, "bsp": l.bsp, "link": l.link, "kind": l.kind.name(),
        "from": l.from, "to": l.to, "a": l.a, "b": l.b, "one_way": l.one_way,
    })
}

fn spot_row(i: usize, b: &BotNav) -> Value {
    let s = &b.spots[i];
    json!({
        "index": i,
        "kind": s.kind().name(),
        "source": s.source,
        "state": s.state().name(),
        "stand": s.stand,
        "edge": s.edge,
        "lip": s.lip,
        "out": s.out,
        "score": s.score,
        "launch": s.launch,
        "fall": s.fall,
        "line": s.line,
        "tries": s.tries,
        "hits": s.hits,
        "trials": s.trials,
        "ok": s.ok,
        "ghost": s.ghost,
    })
}

fn bots(app: &mut App, p: &P) -> Reply {
    open(app)?;
    app.want_bot_nav();
    if p.bool("wait")? {
        app.wait_bot_nav();
    } else {
        app.poll_bot_nav();
    }
    let offset = p.opt_usize("offset")?.unwrap_or(0);
    let limit = p.opt_usize("limit")?.unwrap_or(200);
    let state = app.bot_nav_state();
    let mut out = json!({
        "state": state.name(),
        "map": Value::Null,
        "origins": [],
        "counts": {"polys": 0, "out_of_map": 0, "links": 0, "walls": 0, "spots": 0},
        "links_by_kind": counts(&BotLinkKind::ALL, BotLinkKind::name, |_| 0),
        "spots_by_kind": counts(&SpotKind::ALL, SpotKind::name, |_| 0),
        "spots_by_state": counts(&SpotState::ALL, SpotState::name, |_| 0),
        "notes": [],
        "error": app.bot_nav_error(),
        "selected": pick_json(app.session.selected_nav()),
    });
    let Some(b) = app
        .session
        .bot_nav()
        .filter(|_| state == BotNavState::Ready)
    else {
        return Ok(out);
    };
    out["map"] = json!(b.map);
    out["origins"] = json!(b.origins);
    out["counts"] = json!({
        "polys": b.polys.len(), "out_of_map": b.out_of_map_count(), "links": b.links.len(),
        "walls": b.walls, "spots": b.spots.len(),
    });
    out["links_by_kind"] = counts(&BotLinkKind::ALL, BotLinkKind::name, |k| b.link_count(k));
    out["spots_by_kind"] = counts(&SpotKind::ALL, SpotKind::name, |k| b.spot_kind_count(k));
    out["spots_by_state"] = counts(&SpotState::ALL, SpotState::name, |s| b.spot_count(s));
    out["notes"] = json!(b.notes);
    let page = |n: usize, row: &dyn Fn(usize) -> Value| -> Value {
        (offset..n).take(limit).map(row).collect::<Vec<_>>().into()
    };
    if p.bool("polys")? {
        out["polys"] = page(b.polys.len(), &|i| poly_row(i, b));
    }
    if p.bool("links")? {
        out["links"] = page(b.links.len(), &|i| link_row(i, b));
    }
    if p.bool("spots")? {
        out["spots"] = page(b.spots.len(), &|i| spot_row(i, b));
    }
    Ok(out)
}

/// What a pick names, as `nav.pick` reports it.
fn hit_row(app: &App, pick: NavPick) -> Value {
    let mut row = match pick {
        NavPick::Sector(i) => app
            .session
            .pathfinding()
            .and_then(|l| l.sectors.get(i))
            .map_or(Value::Null, |s| sector_row(i, s)),
        NavPick::BotPoly(i) => app
            .session
            .bot_nav()
            .map_or(Value::Null, |b| poly_row(i, b)),
        NavPick::BotLink(i) => app
            .session
            .bot_nav()
            .map_or(Value::Null, |b| link_row(i, b)),
        NavPick::Spot(i) => app
            .session
            .bot_nav()
            .map_or(Value::Null, |b| spot_row(i, b)),
    };
    if let Value::Object(o) = &mut row {
        o.insert("kind".into(), json!(pick.kind()));
        o.insert(
            "label".into(),
            json!(crate::viewport::nav::label(app, pick)),
        );
    }
    row
}

fn layer_param(p: &P) -> Result<Option<LayerId>, RpcError> {
    p.opt_str("layer")?
        .map(|name| {
            LayerId::NAVIGATION
                .into_iter()
                .find(|id| id.id() == name)
                .ok_or_else(|| {
                    err(
                        code::INVALID_PARAMS,
                        format!("layer: pathfinding, bot_nav or bot_spots, not {name:?}"),
                    )
                })
        })
        .transpose()
}

fn pick(app: &mut App, p: &P) -> Reply {
    open(app)?;
    // Select one by kind and index.
    if p.has("kind") || p.has("index") {
        if ["origin", "dir", "point"].iter().any(|k| p.has(k)) {
            return Err(err(
                code::INVALID_PARAMS,
                "give a kind and index, or a ray or a point, not both",
            ));
        }
        let selected = match (p.opt_str("kind")?, p.opt_usize("index")?) {
            (Some(kind), Some(index)) => {
                Some(NavPick::from_name(kind, index).ok_or_else(|| {
                    err(
                        code::INVALID_PARAMS,
                        format!("kind: sector, poly, link or spot, not {kind:?}"),
                    )
                })?)
            }
            (None, None) => None,
            _ => return Err(err(code::INVALID_PARAMS, "kind and index go together")),
        };
        if let Some(s) = selected.filter(|&s| !app.session.has_nav(s)) {
            return Err(err(
                code::INVALID_PARAMS,
                format!("no {} {} on this map", s.kind(), s.index()),
            ));
        }
        app.perform(Action::SelectNav(selected), false)
            .map_err(refused)?;
        if selected.is_some() && p.bool("focus")? {
            app.perform(Action::FocusSelected, false).map_err(refused)?;
        }
        let hit = selected.map(|s| hit_row(app, s)).unwrap_or(Value::Null);
        return Ok(json!({
            "hit": hit,
            "distance": selected.map(|_| 0.0),
            "selected": pick_json(app.session.selected_nav()),
        }));
    }
    let only = layer_param(p)?;
    let use_shown = p.bool("shown")?;
    let layers = app.layers.clone();
    let accept = |id: LayerId| only.is_none_or(|o| o == id) && (!use_shown || layers.visible(id));
    let hit = match (
        p.opt_vec3("origin")?,
        p.opt_vec3("dir")?,
        p.opt_vec3("point")?,
    ) {
        (Some(origin), Some(dir), None) => {
            if dir.iter().all(|&v| v == 0.0) {
                return Err(err(code::INVALID_PARAMS, "dir must not be zero"));
            }
            let ray = Ray {
                origin,
                dir: normalize(dir),
            };
            app.session.nav_hit(&ray, accept)
        }
        (None, None, Some(point)) => {
            let within = p.opt_f32("within")?.unwrap_or(PICK_WITHIN).max(0.0);
            app.session.nav_at(point, within, accept)
        }
        _ => {
            return Err(err(
                code::INVALID_PARAMS,
                "give a ray (origin and dir), a point, or a kind and index",
            ));
        }
    };
    if p.bool("select")? {
        app.perform(Action::SelectNav(hit.map(|(_, s)| s)), false)
            .map_err(refused)?;
    }
    Ok(json!({
        "hit": hit.map_or(Value::Null, |(_, s)| hit_row(app, s)),
        "distance": hit.map(|(d, _)| d),
        "selected": pick_json(app.session.selected_nav()),
    }))
}
