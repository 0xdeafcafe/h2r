//! `h2 agent maps ...`: test maps (docs/game/test-maps.md). Lists the
//! built-in recipes and the maps built, builds one from a recipe (a
//! built-in's name, or a TOML or JSON file read here), loads one as the
//! next game and waits for it, and removes one. Each verb may be a line of
//! an `h2 agent -` or `h2 agent run` script.

use std::path::Path;
use std::time::{Duration, Instant};

use agent_proto::{CallError, Client, method};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const USAGE: &str = "  maps [list [NAME]]     maps.test.list: the built-in recipes, and the test
                         maps built (state, slot, what was placed where)
  maps create <RECIPE|FILE> [base=MAP] [replace] [nowait]
                         maps.test.create: a built-in recipe by name, or a
                         recipe file (.toml or .json) read here; base= builds
                         it on another disc map. Waits for the build (up to
                         5 min) unless nowait
  maps load <NAME> [nowait]  maps.test.load: queue it as the next game (a
                         local Slayer game, one player), then wait until it
                         is in gameplay (up to 2 min) unless nowait
  maps remove <NAME>     maps.test.remove: delete its package";

/// How long `create` waits for a build, and `load` for the map.
const BUILD_WAIT: Duration = Duration::from_secs(300);
const LOAD_WAIT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(250);

fn call(c: &mut Client, m: &str, p: Value) -> Result<Value> {
    c.call(m, p).map_err(|e| match e {
        CallError::Rpc(e) => anyhow::anyhow!(
            "{}",
            serde_json::to_string(&e).unwrap_or_else(|_| e.message.clone())
        ),
        e => anyhow::anyhow!("{m}: {e}"),
    })
}

/// A recipe argument: a file's recipe as JSON, or a built-in's name.
fn recipe_arg(arg: &str) -> Result<Value> {
    let path = Path::new(arg);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !(ext.eq_ignore_ascii_case("toml") || ext.eq_ignore_ascii_case("json")) {
        if path.is_file() {
            bail!("{arg}: a recipe file ends in .toml or .json");
        }
        return Ok(Value::String(arg.to_owned()));
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {arg}"))?;
    if ext.eq_ignore_ascii_case("json") {
        return serde_json::from_str(&text).with_context(|| format!("{arg}: not JSON"));
    }
    let t: toml::Value = toml::from_str(&text).with_context(|| format!("{arg}: not TOML"))?;
    serde_json::to_value(t).context("a TOML recipe as JSON")
}

/// The method and params of a verb, and whether to wait after it.
#[derive(Debug, PartialEq)]
struct Verb {
    method: &'static str,
    params: Value,
    wait: bool,
}

fn parse(words: &[String]) -> Result<Verb> {
    let verb = words.first().map_or("list", String::as_str);
    let rest = words.get(1..).unwrap_or(&[]);
    let (pos, flags): (Vec<&String>, Vec<&String>) = rest
        .iter()
        .partition(|w| !w.contains('=') && !matches!(w.as_str(), "replace" | "nowait"));
    let flag = |f: &str| flags.iter().any(|w| *w == f);
    let named = |k: &str| {
        flags
            .iter()
            .find_map(|w| w.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
    };
    let one = |what: &str| -> Result<String> {
        match pos.as_slice() {
            [n] => Ok((*n).clone()),
            [] => bail!("maps {verb} needs {what}"),
            _ => bail!("maps {verb}: one {what}"),
        }
    };
    let unknown = flags
        .iter()
        .find(|w| !matches!(w.as_str(), "replace" | "nowait") && !w.starts_with("base="));
    if let Some(w) = unknown {
        bail!("maps {verb}: unexpected {w}");
    }
    Ok(match verb {
        "list" => {
            if pos.len() > 1 {
                bail!("maps list: at most one name");
            }
            Verb {
                method: method::MAPS_TEST_LIST,
                params: match pos.first() {
                    Some(n) => json!({ "name": n }),
                    None => json!({}),
                },
                wait: false,
            }
        }
        "create" => {
            let mut p = json!({ "recipe": recipe_arg(&one("a recipe")?)? });
            if let Some(b) = named("base") {
                p["base"] = json!(b);
            }
            if flag("replace") {
                p["replace"] = json!(true);
            }
            Verb {
                method: method::MAPS_TEST_CREATE,
                params: p,
                wait: !flag("nowait"),
            }
        }
        "load" => Verb {
            method: method::MAPS_TEST_LOAD,
            params: json!({ "name": one("a test map's name")? }),
            wait: !flag("nowait"),
        },
        "remove" => Verb {
            method: method::MAPS_TEST_REMOVE,
            params: json!({ "name": one("a test map's name")? }),
            wait: false,
        },
        other => bail!("maps {other}: list, create, load or remove"),
    })
}

/// Polls `maps.test.list` until `name` is built or failed.
fn wait_built(c: &mut Client, name: &str) -> Result<Value> {
    let until = Instant::now() + BUILD_WAIT;
    loop {
        let v = call(c, method::MAPS_TEST_LIST, json!({ "name": name }))?;
        let m = v["maps"][0].clone();
        match m["state"].as_str() {
            Some("ready") => return Ok(m),
            Some("failed") => bail!(
                "{name} failed to build: {}",
                m["error"].as_str().unwrap_or("no reason given")
            ),
            _ if Instant::now() >= until => bail!("{name}: still building after {BUILD_WAIT:?}"),
            _ => std::thread::sleep(POLL),
        }
    }
}

/// Whether the stamp says a map that started at or after tick `from` is in
/// gameplay, past its first second.
fn loaded_since(at: &Value, from: u64) -> bool {
    let ticks = at["ticks"].as_u64().unwrap_or(0);
    let map_ticks = at["map_ticks"].as_u64().unwrap_or(u64::MAX);
    at["phase"] == "gameplay" && map_ticks >= 30 && ticks.saturating_sub(map_ticks) >= from
}

/// Polls `session.status` until a map loaded after tick `from` is in
/// gameplay.
fn wait_loaded(c: &mut Client, from: u64) -> Result<Value> {
    let until = Instant::now() + LOAD_WAIT;
    loop {
        let s = call(c, method::STATUS, Value::Null)?;
        if loaded_since(&s["at"], from) {
            return Ok(s["at"].clone());
        }
        if Instant::now() >= until {
            bail!("no new map in gameplay after {LOAD_WAIT:?}: {}", s["at"]);
        }
        std::thread::sleep(POLL);
    }
}

/// Runs a `maps` verb; its answer.
pub fn run(c: &mut Client, words: &[String]) -> Result<Value> {
    let v = parse(words)?;
    match v.method {
        method::MAPS_TEST_CREATE if v.wait => {
            let made = call(c, v.method, v.params)?;
            let name = made["name"].as_str().unwrap_or_default().to_owned();
            wait_built(c, &name)
        }
        method::MAPS_TEST_LOAD if v.wait => {
            let from = call(c, method::STATUS, Value::Null)?["at"]["ticks"]
                .as_u64()
                .unwrap_or(0);
            let mut loaded = call(c, v.method, v.params)?;
            loaded["at"] = wait_loaded(c, from)?;
            Ok(loaded)
        }
        m => call(c, m, v.params),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn verbs_become_calls() {
        let v = parse(&w("")).unwrap();
        assert_eq!((v.method, v.params), (method::MAPS_TEST_LIST, json!({})));
        let v = parse(&w("list spawn-grid")).unwrap();
        assert_eq!(v.params, json!({"name": "spawn-grid"}));
        let v = parse(&w("create spawn-grid base=lockout replace")).unwrap();
        assert_eq!(v.method, method::MAPS_TEST_CREATE);
        assert_eq!(
            v.params,
            json!({"recipe": "spawn-grid", "base": "lockout", "replace": true})
        );
        assert!(v.wait);
        assert!(!parse(&w("create spawn-grid nowait")).unwrap().wait);
        let v = parse(&w("load spawn-grid")).unwrap();
        assert_eq!(
            (v.method, v.params, v.wait),
            (method::MAPS_TEST_LOAD, json!({"name": "spawn-grid"}), true)
        );
        let v = parse(&w("remove spawn-grid")).unwrap();
        assert_eq!(v.method, method::MAPS_TEST_REMOVE);
        for bad in [
            "create",
            "load",
            "load a b",
            "remove",
            "fly",
            "create x colour=1",
            "list a b",
        ] {
            assert!(parse(&w(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_recipe_file_is_read_here_as_json() {
        let d = std::env::temp_dir().join(format!("h2-agent-maps-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let toml_path = d.join("mine.toml");
        std::fs::write(
            &toml_path,
            "name = \"mine\"\nbase = \"lockout\"\n[[place]]\nblock = \"starting_locations\"\nat = [1, 0, 0]\n",
        )
        .unwrap();
        let v = recipe_arg(toml_path.to_str().unwrap()).unwrap();
        assert_eq!(v["name"], "mine");
        assert_eq!(v["place"][0]["at"], json!([1, 0, 0]));
        let json_path = d.join("mine.json");
        std::fs::write(&json_path, r#"{"name": "mine"}"#).unwrap();
        assert_eq!(recipe_arg(json_path.to_str().unwrap()).unwrap()["name"], "mine");
        assert_eq!(recipe_arg("spawn-grid").unwrap(), json!("spawn-grid"));
        assert!(recipe_arg(d.join("missing.toml").to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_load_waits_for_a_map_started_after_it() {
        let at = |phase: &str, ticks: u64, map_ticks: u64| {
            json!({"phase": phase, "ticks": ticks, "map_ticks": map_ticks})
        };
        // The game that was running when the load was asked.
        assert!(!loaded_since(&at("gameplay", 1000, 900), 950));
        // The new map, loading, then in its first second, then playing.
        assert!(!loaded_since(&at("loading", 1010, 0), 950));
        assert!(!loaded_since(&at("gameplay", 1010, 10), 950));
        assert!(loaded_since(&at("gameplay", 1040, 40), 950));
        assert!(!loaded_since(&json!({}), 0));
    }
}
