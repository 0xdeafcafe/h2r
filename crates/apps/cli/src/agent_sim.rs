//! `h2 agent sim ...`: simulated players, host writes, expectations and
//! lobby steps (docs/runtime/agent-api.md, "sim"). Each verb may be a line
//! of an `h2 agent run` script; `sim run` is `h2 agent run` that removes
//! the simulated players the script added when it ends.

use agent_proto::expect::Expect;
use agent_proto::sim::menu::{self, Lobby};
use agent_proto::sim::{self, Rpc, method};
use agent_proto::{RpcError, code};
use serde_json::{Map, Value, json};

pub const USAGE: &str = "  sim add phantom [script|recording|brain] xuid=X gamertag=G
        [team=N] [steps=JSON] [recording=NAME]
                         sim.player.add (default source: brain); a phantom's
                         xuid comes from the local embedded Live server
  sim remove <sim.N|all> sim.player.remove
  sim list               sim.player.list
  sim pad <sim.N> <JSON steps> [append]  sim.player.pad
  sim set <player.N|sim.N> key=JSON...  sim.state.set: pos=[x,y,z] (the
                         game's own teleport, answered after the next tick)
                         yaw=DEG [pitch=DEG] or facing=[x,y,z] (or forward=,
                         as world.query names it; turns the
                         unit and the player's view) team=N health=0..1
                         shield=0..3 grenades={\"frag\":N}
                         ammo={\"loaded\":N,\"reserve\":N}; host only, never
                         in a co-op session
  sim give <player.N|sim.N> <weap tag> [loaded=N] [reserve=N] [keep]
                         sim.give: the game makes the weapon and the unit
                         holds it (keep: added beside its weapons instead);
                         the tag is a datum or a name, / or \\ (edit tags weap),
                         looked for among weap tags; group:name gives the group
  sim spawn <tag> pos=[x,y,z] [yaw=DEG]
                         sim.spawn: the game makes the object (vehi, weap,
                         eqip, scen, bipd, ...) there; a name is looked for
                         among object groups (vehi:name to pick one)
  sim view [root...]     sim.view: at session ui player object count sim input
  sim expect <pointer> <op> [JSON]  sim.expect, judged in the game at one tick
                         (several: one quoted expectation per argument)
  sim until <ticks> <pointer> <op> [JSON]  sim.expect every 5 ticks until it
                         passes; fails with the last verdict after <ticks>
  sim session            sim.session: the session kind and whether writes are allowed
  sim lobby mode <campaign|xbox_live|split_screen|system_link>
  sim lobby host live_coop | invite [ROW] | start | press <b[+b...]> [JSON expect]
                         menu steps on player one's pad, checked on ui.screen
                         (H2_UI_WATCH=1)
  sim run <script|->     h2 agent run, then sim.player.remove for what it added";

fn bad(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

/// `key=JSON` words into an object (a bare word is a string).
fn pairs(words: &[String]) -> Result<Map<String, Value>, RpcError> {
    let mut m = Map::new();
    for w in words {
        let (k, v) = w
            .split_once('=')
            .ok_or_else(|| bad(format!("{w}: key=value")))?;
        let v = serde_json::from_str(v).unwrap_or_else(|_| Value::from(v));
        m.insert(k.to_owned(), v);
    }
    Ok(m)
}

/// Expectations from words: one per argument when an argument holds a
/// whole one (`'/at/phase == "menu"'`), else the words are one.
fn expectations(words: &[String]) -> Result<Vec<Expect>, RpcError> {
    if words.is_empty() {
        return Err(bad("an expectation: <pointer> <op> [JSON]"));
    }
    let whole = words[0].starts_with('/') && words[0].contains(char::is_whitespace);
    let lines: Vec<String> = if whole {
        words.to_vec()
    } else {
        vec![words.join(" ")]
    };
    lines
        .iter()
        .map(|l| Expect::parse(l).map_err(bad))
        .collect()
}

/// The params of a single-call verb.
pub fn params(words: &[String]) -> Result<(&'static str, Value), RpcError> {
    let verb = words.first().map(String::as_str).unwrap_or("");
    let rest = words.get(1..).unwrap_or(&[]);
    Ok(match verb {
        "add" => {
            match rest.first().map(String::as_str) {
                Some("phantom") => {}
                Some("local") => {
                    return Err(bad("controller seats are for people. Use sim add phantom"));
                }
                _ => return Err(bad("sim add phantom [script|recording|brain] xuid=X gamertag=G")),
            }
            let (source, more) = match rest.get(1) {
                Some(s) if !s.contains('=') => (s.as_str(), &rest[2..]),
                _ => ("brain", rest.get(1..).unwrap_or(&[])),
            };
            let mut p = pairs(more)?;
            let xuid = p.remove("xuid");
            let gamertag = p.remove("gamertag");
            if xuid.is_some() || gamertag.is_some() {
                let s = |v: Option<Value>| match v {
                    Some(Value::String(s)) => s,
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                p.insert(
                    "identity".into(),
                    json!({ "xuid": s(xuid), "gamertag": s(gamertag) }),
                );
            }
            p.insert("kind".into(), json!("phantom"));
            p.insert("source".into(), json!(source));
            (method::PLAYER_ADD, Value::Object(p))
        }
        "remove" => match rest {
            [w] if w == "all" => (method::PLAYER_REMOVE, json!({ "all": true })),
            [id] => (method::PLAYER_REMOVE, json!({ "id": id })),
            _ => return Err(bad("sim remove <sim.N|all>")),
        },
        "list" => (method::PLAYER_LIST, Value::Null),
        "session" => (method::SESSION, Value::Null),
        "pad" => {
            let (Some(id), Some(steps)) = (rest.first(), rest.get(1)) else {
                return Err(bad("sim pad <sim.N> <JSON steps> [append]"));
            };
            let steps: Value =
                serde_json::from_str(steps).map_err(|e| bad(format!("steps: {e}")))?;
            let append = match rest.get(2).map(String::as_str) {
                None => false,
                Some("append") => true,
                Some(w) => return Err(bad(format!("sim pad: unexpected {w}"))),
            };
            (
                method::PLAYER_PAD,
                json!({ "id": id, "steps": steps, "append": append }),
            )
        }
        "set" => {
            let who = rest
                .first()
                .ok_or_else(|| bad("sim set <player.N|sim.N> key=JSON..."))?;
            let mut p = pairs(&rest[1..])?;
            p.insert("player".into(), json!(who));
            (method::STATE_SET, Value::Object(p))
        }
        "give" => {
            let (Some(who), Some(weapon)) = (rest.first(), rest.get(1)) else {
                return Err(bad(
                    "sim give <player.N|sim.N> <weap tag> [loaded=N] [reserve=N] [keep]",
                ));
            };
            let mut keep = false;
            let mut kv = Vec::new();
            for word in &rest[2..] {
                match word.as_str() {
                    "keep" => keep = true,
                    _ => kv.push(word.clone()),
                }
            }
            let mut p = pairs(&kv)?;
            let mut ammo = Map::new();
            for k in ["loaded", "reserve"] {
                if let Some(v) = p.remove(k) {
                    ammo.insert(k.into(), v);
                }
            }
            if let Some(k) = p.keys().next() {
                return Err(bad(format!("sim give: unexpected {k}")));
            }
            let mut p = json!({ "player": who, "weapon": weapon });
            if !ammo.is_empty() {
                p["ammo"] = Value::Object(ammo);
            }
            if keep {
                p["keep"] = json!(true);
            }
            (method::GIVE, p)
        }
        "spawn" => {
            let tag = rest
                .first()
                .ok_or_else(|| bad("sim spawn <tag> pos=[x,y,z] [yaw=DEG]"))?;
            let mut p = pairs(&rest[1..])?;
            p.insert("tag".into(), json!(tag));
            (method::SPAWN, Value::Object(p))
        }
        "view" => (
            method::VIEW,
            if rest.is_empty() {
                Value::Null
            } else {
                json!({ "roots": rest })
            },
        ),
        "expect" => (method::EXPECT, json!({ "checks": expectations(rest)? })),
        other => return Err(bad(format!("sim {other}: not a sim verb"))),
    })
}

/// How often `sim until` asks.
const UNTIL_EVERY: u64 = 5;

/// Runs one `sim` verb (its words after `sim`), on `rpc`.
pub fn call(rpc: &mut dyn Rpc, words: &[String]) -> Result<Value, RpcError> {
    match words.first().map(String::as_str) {
        Some("until") => {
            let ticks: u64 = words
                .get(1)
                .and_then(|w| w.parse().ok())
                .ok_or_else(|| bad("sim until <ticks> <pointer> <op> [JSON]"))?;
            let checks = expectations(&words[2..])?;
            sim::until(rpc, &checks, ticks, UNTIL_EVERY)
        }
        Some("lobby") => menu::run(rpc, &lobby(&words[1..])?),
        _ => {
            let (m, p) = params(words)?;
            rpc.call(m, p)
        }
    }
}

/// A lobby step from its words.
pub fn lobby(words: &[String]) -> Result<Lobby, RpcError> {
    let word = |i: usize| words.get(i).map(String::as_str);
    let v = match (word(0), word(1)) {
        (Some("mode"), Some(m)) => json!({ "action": "mode", "mode": m }),
        (Some("host"), Some(via)) => json!({ "action": "host", "via": via }),
        (Some("invite"), row) => {
            let friend: u32 = row
                .map(|r| r.parse().map_err(|_| bad("invite [ROW]")))
                .transpose()?
                .unwrap_or(0);
            json!({ "action": "invite", "friend": friend })
        }
        (Some("start"), None) => json!({ "action": "start" }),
        (Some("press"), Some(b)) => {
            let mut v = json!({ "action": "press", "buttons": b.split('+').collect::<Vec<_>>() });
            if let Some(e) = word(2) {
                v["expect"] = serde_json::from_str(e).map_err(|e| bad(format!("expect: {e}")))?;
            }
            v
        }
        _ => {
            return Err(bad(
                "sim lobby mode <mode> | host live_coop | invite [ROW] | start | press <b[+b]> [JSON]",
            ));
        }
    };
    serde_json::from_value(v).map_err(|e| bad(e.to_string()))
}

/// The cleanup after `sim run`: removes every simulated player when the
/// script added any.
pub fn cleanup(rpc: &mut dyn Rpc) -> Value {
    match rpc.call(method::PLAYER_REMOVE, json!({ "all": true })) {
        Ok(v) => v,
        Err(e) => json!({ "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        crate::agent_cmd::split_words(s).unwrap()
    }

    #[test]
    fn verbs_become_calls() {
        let p = |s: &str| params(&w(s)).unwrap();
        assert_eq!(
            p("add phantom script xuid=0009000000000001 gamertag=Clanker team=1 steps=[{\"buttons\":[\"a\"]}]"),
            (
                method::PLAYER_ADD,
                json!({"kind": "phantom", "source": "script", "team": 1, "steps": [{"buttons": ["a"]}],
                       "identity": {"xuid": "0009000000000001", "gamertag": "Clanker"}})
            )
        );
        assert_eq!(
            p("add phantom team=0 xuid=0009000000000001 gamertag=Clanker"),
            (
                method::PLAYER_ADD,
                json!({"kind": "phantom", "source": "brain", "team": 0,
                       "identity": {"xuid": "0009000000000001", "gamertag": "Clanker"}})
            ),
            "an xuid keeps its zeros"
        );
        assert_eq!(
            p("set sim.1 health=0.5 pos=[1,2,3]"),
            (
                method::STATE_SET,
                json!({"player": "sim.1", "health": 0.5, "pos": [1, 2, 3]})
            )
        );
        assert_eq!(
            p("set player.0 yaw=90 pitch=-10"),
            (
                method::STATE_SET,
                json!({"player": "player.0", "yaw": 90, "pitch": -10})
            )
        );
        assert_eq!(
            p("give player.0 objects/weapons/rifle/smg/smg loaded=60 keep"),
            (
                method::GIVE,
                json!({"player": "player.0", "weapon": "objects/weapons/rifle/smg/smg",
                       "ammo": {"loaded": 60}, "keep": true})
            )
        );
        assert_eq!(
            p("give sim.1 0xe1230045"),
            (
                method::GIVE,
                json!({"player": "sim.1", "weapon": "0xe1230045"})
            )
        );
        assert_eq!(
            p("spawn objects/vehicles/warthog/warthog pos=[1,2,3] yaw=45"),
            (
                method::SPAWN,
                json!({"tag": "objects/vehicles/warthog/warthog", "pos": [1, 2, 3], "yaw": 45})
            )
        );
        assert_eq!(
            p("remove all"),
            (method::PLAYER_REMOVE, json!({"all": true}))
        );
        assert_eq!(
            p("view player at"),
            (method::VIEW, json!({"roots": ["player", "at"]}))
        );
        let (m, e) = p("expect /player/1/team == 1");
        assert_eq!(m, method::EXPECT);
        assert_eq!(e["checks"][0]["op"], "==");
        assert_eq!(
            p("pad sim.2 [{\"left\":[0,1],\"ticks\":30}] append").1["append"],
            true
        );
        for bad in [
            "add",
            "remove",
            "pad sim.1",
            "set",
            "set p health",
            "give player.0",
            "give player.0 0xe1230045 god=1",
            "spawn",
            "expect",
            "fly",
        ] {
            assert!(params(&w(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn sim_add_refuses_controller_seats() {
        let e = params(&w("add local script")).unwrap_err();
        assert!(e.message.contains("Use sim add phantom"));
    }

    #[test]
    fn several_whole_expectations_or_one_from_words() {
        let one = expectations(&w("/at/phase == \"gameplay\"")).unwrap();
        assert_eq!(one.len(), 1);
        let two = expectations(&[
            "/count/player >= 2".to_string(),
            "/sim/count == 1".to_string(),
        ])
        .unwrap();
        assert_eq!(two.len(), 2);
    }

    #[test]
    fn lobby_words() {
        assert_eq!(
            lobby(&w("mode system_link")).unwrap(),
            Lobby::Mode {
                mode: menu::Mode::SystemLink
            }
        );
        assert_eq!(lobby(&w("invite 2")).unwrap(), Lobby::Invite { friend: 2 });
        assert_eq!(lobby(&w("start")).unwrap(), Lobby::Start);
        assert!(matches!(
            lobby(&w("press down+a {\"main_menu\":true}")).unwrap(),
            Lobby::Press { .. }
        ));
        for bad in ["mode", "mode moon", "host", "start now", "invite x"] {
            assert!(lobby(&w(bad)).is_err(), "{bad}");
        }
    }

    /// The game for `call`: answers sim.expect from a counter.
    struct Fake(u32);

    impl Rpc for Fake {
        fn call(&mut self, m: &str, p: Value) -> Result<Value, RpcError> {
            self.0 += 1;
            match m {
                method::EXPECT => {
                    let p: sim::ExpectParams = serde_json::from_value(p).unwrap();
                    let view = json!({"at": {"tick": self.0}});
                    Ok(agent_proto::expect::verdict(&view, &p.checks))
                }
                _ => Ok(json!({})),
            }
        }
    }

    #[test]
    fn until_runs_in_a_script_line() {
        let v = call(&mut Fake(0), &w("until 100 /at/tick >= 5")).unwrap();
        assert_eq!(v["pass"], true);
        assert!(call(&mut Fake(0), &w("until 4 /at/tick > 100")).is_err());
    }
}
