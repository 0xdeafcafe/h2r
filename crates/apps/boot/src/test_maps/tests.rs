//! `maps.test.*` without a guest: the options steps against the level
//! boot's, recipe parameters, the list lookup, packages on disk, and the
//! methods through the agent's own dispatcher.

use super::*;
use crate::layout_glue::Gm;
use agent_proto::{Object, Request as RpcRequest};
use std::sync::mpsc::{Receiver, sync_channel};

/// Guest memory as a byte map, for the level boot's writes.
#[derive(Default, PartialEq, Debug)]
struct Mem(BTreeMap<u32, u8>);

impl Gm for Mem {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.0.get(&(addr + i as u32)).copied().unwrap_or(0);
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

fn listed() -> Listed {
    Listed {
        id: 0x0001_2345,
        path: "t:\\$C\\4d53006480012345\\m-00012345-0123456789abcdef0123".into(),
    }
}

#[test]
fn the_steps_are_the_level_boots_over_the_agents_options_block() {
    let req = request("spawn-grid", &listed());
    assert_eq!(req.kind, Kind::Multiplayer);
    assert_eq!(req.campaign, u32::MAX);
    let steps = game_steps(&req).unwrap();
    let buf = 0xa2f4_1000;
    // The calls, in order, are call_of's with the block's address.
    let calls: Vec<level_boot::GuestCall> = steps
        .iter()
        .filter_map(|s| match s {
            GameStep::Call {
                target,
                args,
                eax,
                esi,
                ..
            } => Some(level_boot::GuestCall {
                target: *target,
                args: args.iter().map(|a| a.resolve(buf)).collect(),
                eax: eax.map(|a| a.resolve(buf)),
                esi: esi.map(|a| a.resolve(buf)),
            }),
            GameStep::Write { .. } => None,
        })
        .collect();
    let want: Vec<level_boot::GuestCall> = level_boot::steps(Kind::Multiplayer)
        .into_iter()
        .filter_map(|s| level_boot::call_of(s, buf))
        .collect();
    assert_eq!(calls, want);
    // The writes are fill_options's, made after the options are cleared
    // and before the variant is built.
    let mut ours = Mem::default();
    for s in &steps {
        if let GameStep::Write { offset, bytes } = s {
            ours.wr(buf + offset, bytes);
        }
    }
    let mut theirs = Mem::default();
    assert!(level_boot::fill_options(&mut theirs, buf, &req));
    assert_eq!(ours, theirs);
    let first_write = steps.iter().position(|s| matches!(s, GameStep::Write { .. })).unwrap();
    let last_write = steps.iter().rposition(|s| matches!(s, GameStep::Write { .. })).unwrap();
    assert_eq!(first_write, 1);
    assert!(matches!(&steps[0], GameStep::Call { target, .. } if *target == level_boot::OPTIONS_NEW));
    assert!(matches!(
        &steps[last_write + 1],
        GameStep::Call { target, .. } if *target == level_boot::VARIANT_DEFAULTS
    ));
    // Only the options check is reported.
    let reported: Vec<&str> = steps
        .iter()
        .filter_map(|s| match s {
            GameStep::Call { report, .. } => *report,
            _ => None,
        })
        .collect();
    assert_eq!(reported, ["check"]);
    assert_eq!(ours.u32(buf + level_boot::OPT_MAP), Some(0x0001_2345));
    // A path the options can't hold is refused.
    let mut long = listed();
    long.path = "x".repeat(level_boot::OPT_PATH_LEN as usize);
    assert!(game_steps(&request("x", &long)).is_none());
}

#[test]
fn a_recipe_is_named_or_given_and_may_move_to_another_base() {
    let p = |v: Value| serde_json::from_value::<CreateParams>(v).unwrap();
    let r = recipe_of(&p(json!({"recipe": "spawn-grid"}))).unwrap();
    assert_eq!(r.name, "spawn-grid");
    let r = recipe_of(&p(json!({"recipe": "spawn-grid", "base": "lockout"}))).unwrap();
    assert_eq!((r.name.as_str(), r.base.as_str()), ("spawn-grid-lockout", "lockout"));
    let inline = json!({"recipe": {
        "name": "mine", "title": "Mine", "purpose": "p", "base": "lockout", "fidelity": "full",
        "place": [{"block": "starting_locations"}]
    }});
    assert_eq!(recipe_of(&p(inline)).unwrap().name, "mine");
    for bad in [
        json!({"recipe": "nope"}),
        json!({"recipe": 3}),
        json!({"recipe": {"name": "x"}}),
        json!({"recipe": "superbounce", "base": "lockout"}),
    ] {
        let e = recipe_of(&p(bad.clone())).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{bad}");
    }
}

#[test]
fn a_package_is_found_in_the_list_by_its_alias() {
    let maps = [
        crate::map_select::Map {
            id: 80,
            name: "Ascension".into(),
            sort: 10,
            path: "scenarios\\multi\\ascension\\ascension".into(),
        },
        crate::map_select::Map {
            id: listed().id,
            name: "Test: Spawn grid".into(),
            sort: 1000,
            path: listed().path,
        },
    ];
    let id = "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(listed_in(&maps, id), Some(listed()));
    assert_eq!(listed_in(&maps, &"f".repeat(64)), None);
    assert_eq!(listed_in(&maps, "short"), None);
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("boot-test-maps-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn test_maps_on_disk_are_the_test_packages() {
    let d = temp("disk");
    let maps = d.join("maps");
    let m = content::Manifest::retail("lockout", content::Sha256::of(b"invented"));
    for dir in ["test-spawn-grid", "lockout-edit"] {
        std::fs::create_dir_all(maps.join(dir)).unwrap();
        std::fs::write(
            maps.join(dir).join(content::manifest::MANIFEST_FILE),
            m.to_toml(),
        )
        .unwrap();
    }
    std::fs::create_dir_all(maps.join("test-broken")).unwrap();
    let found = on_disk(&maps);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "spawn-grid");
    assert_eq!(found[0].base, "lockout");
    assert_eq!(found[0].state, State::Ready);
    assert_eq!(found[0].map_id.as_deref(), Some(m.map_id().to_string().as_str()));
    std::fs::remove_dir_all(&d).unwrap();
}

struct Menu;

impl agent::Reader for Menu {
    fn status(&mut self) -> agent::GameStatus {
        agent::GameStatus::default()
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(Vec::new())
    }
}

struct Client {
    id: agent::ConnId,
    replies: Receiver<String>,
}

impl Client {
    fn connect(agent: &Agent) -> Self {
        let (tx, replies) = sync_channel(32);
        Self {
            id: agent.connect(tx),
            replies,
        }
    }
    /// One call, answered between slices.
    fn call(&self, agent: &Agent, id: u64, name: &str, params: Value) -> Value {
        let req = RpcRequest::new(id, name, params);
        agent.submit(self.id, &serde_json::to_string(&req).unwrap());
        agent.between_slices(&mut Menu, 0);
        let response: Value = serde_json::from_str(&self.replies.try_recv().unwrap()).unwrap();
        assert_eq!(response["id"], id);
        response
    }
}

#[test]
fn the_methods_list_build_and_remove_through_the_dispatcher() {
    let d = temp("methods");
    let agent = Agent::new();
    // No disc there: a build fails, and says why.
    install(&agent, None, &d.join("no-disc.iso"), &d.join("data"), None);
    let c = Client::connect(&agent);
    let methods = c.call(&agent, 1, method::METHODS, json!({}));
    for m in [
        method::MAPS_TEST_LIST,
        method::MAPS_TEST_CREATE,
        method::MAPS_TEST_LOAD,
        method::MAPS_TEST_REMOVE,
    ] {
        assert!(
            methods["result"].as_array().unwrap().iter().any(|n| n == m),
            "{m}"
        );
    }
    let list = c.call(&agent, 2, method::MAPS_TEST_LIST, json!({}));
    assert_eq!(
        list["result"]["recipes"].as_array().unwrap().len(),
        recipe::BUILT_IN.len()
    );
    assert_eq!(list["result"]["maps"], json!([]));
    let bad = c.call(&agent, 3, method::MAPS_TEST_CREATE, json!({"recipe": "nope"}));
    assert_eq!(bad["error"]["code"], code::INVALID_PARAMS);
    let made = c.call(&agent, 4, method::MAPS_TEST_CREATE, json!({"recipe": "empty-room"}));
    assert_eq!(made["result"]["name"], "empty-room");
    assert_eq!(made["result"]["dir"], "test-empty-room");
    let mut state = Value::Null;
    for i in 0..500 {
        let one = c.call(&agent, 10 + i, method::MAPS_TEST_LIST, json!({"name": "empty-room"}));
        state = one["result"]["maps"][0].clone();
        if state["state"] != "building" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(state["state"], "failed", "{state}");
    assert!(state["error"].as_str().unwrap().contains("disc"), "{state}");
    // Loading a failed map says why; an unknown one is a bad name.
    let load = c.call(&agent, 600, method::MAPS_TEST_LOAD, json!({"name": "nope"}));
    assert!(load["error"].is_object(), "{load}");
    let removed = c.call(&agent, 601, method::MAPS_TEST_REMOVE, json!({"name": "empty-room"}));
    assert_eq!(removed["result"]["removed"], false);
    let gone = c.call(&agent, 602, method::MAPS_TEST_LIST, json!({"name": "empty-room"}));
    assert_eq!(gone["error"]["code"], code::INVALID_PARAMS);
    std::fs::remove_dir_all(&d).unwrap();
}
