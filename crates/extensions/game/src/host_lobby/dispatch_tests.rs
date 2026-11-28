//! Exercise the actual agent dispatcher, including method discovery, access
//! control and the deferred response path, without booting the guest.

use super::*;
use agent_proto::{Object, Request};
use std::sync::mpsc::{Receiver, sync_channel};

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

    fn send(&self, agent: &Agent, id: u64, name: &str, params: Value) {
        let req = Request::new(id, name, params);
        agent.submit(self.id, &serde_json::to_string(&req).unwrap());
    }

    fn response(&self, id: u64) -> Value {
        let response: Value = serde_json::from_str(&self.replies.try_recv().unwrap()).unwrap();
        assert_eq!(response["id"], id);
        response
    }
}

#[test]
fn session_methods_lists_only_installed_lobby_methods() {
    let agent = Agent::new();
    let client = Client::connect(&agent);
    let names = [
        method::HOST_LOBBY_OPEN,
        method::HOST_LOBBY_STATE,
        method::HOST_LOBBY_START,
        method::HOST_LOBBY_CLOSE,
    ];
    for install in [false, true] {
        if install {
            methods(
                &agent,
                &Shared::default(),
                Catalogue::default(),
                Variants::default(),
                false,
            );
        }
        client.send(&agent, 1, method::METHODS, json!({}));
        agent.between_slices(&mut Menu, 0);
        let result = client.response(1);
        let listed = result["result"].as_array().unwrap();
        for name in names {
            assert_eq!(
                listed.iter().filter(|n| n.as_str() == Some(name)).count(),
                usize::from(install)
            );
        }
    }
}

#[test]
fn malformed_payloads_never_enqueue_a_transition() {
    let agent = Agent::new();
    let client = Client::connect(&agent);
    let shared = Shared::default();
    methods(
        &agent,
        &shared,
        Catalogue::default(),
        Variants::default(),
        false,
    );

    for (name, params) in [
        (method::HOST_LOBBY_CLOSE, json!([])),
        (method::HOST_LOBBY_START, json!({"force": true})),
        (method::HOST_LOBBY_STATE, json!(true)),
        (method::HOST_LOBBY_OPEN, json!(["lockout", null, 1, false])),
        (
            method::HOST_LOBBY_OPEN,
            json!({"map":"lockout","max_players":1}),
        ),
    ] {
        client.send(&agent, 1, name, params);
        agent.between_slices(&mut Menu, 0);
        assert_eq!(client.response(1)["error"]["code"], code::INVALID_PARAMS);
    }
    let q = shared.lock().unwrap();
    assert!(q.job.is_none());
    assert!(q.readers.is_empty());
}

#[test]
fn close_serializes_with_open_or_start_and_observers_can_still_read() {
    let agent = Agent::new();
    let controller = Client::connect(&agent);
    let observer = Client::connect(&agent);
    let shared = Shared::default();
    methods(
        &agent,
        &shared,
        Catalogue::default(),
        Variants::default(),
        false,
    );
    controller.send(&agent, 1, method::CLAIM, json!({}));
    agent.between_slices(&mut Menu, 0);
    assert_eq!(controller.response(1)["result"]["controller"], true);

    observer.send(&agent, 2, method::HOST_LOBBY_CLOSE, json!({}));
    controller.send(&agent, 3, method::HOST_LOBBY_CLOSE, json!({}));
    controller.send(&agent, 4, method::HOST_LOBBY_START, json!({}));
    observer.send(&agent, 5, method::HOST_LOBBY_STATE, json!({}));
    agent.between_slices(&mut Menu, 0);
    assert_eq!(observer.response(2)["error"]["code"], code::NOT_CONTROLLER);
    assert_eq!(controller.response(4)["error"]["code"], code::BUSY);
    assert!(controller.replies.try_recv().is_err());
    assert!(observer.replies.try_recv().is_err());

    {
        let mut q = shared.lock().unwrap();
        let job = q.job.take().unwrap();
        assert!(matches!(job.cmd, Command::Close));
        job.reply.send(Ok(json!({}))).unwrap();
        assert_eq!(q.readers.len(), 1);
        q.readers
            .pop()
            .unwrap()
            .send(Ok(json!({
                "phase": "menu", "players": [], "ready": false,
            })))
            .unwrap();
    }
    agent.on_tick(&mut Menu);
    assert!(controller.response(3)["result"]["at"].is_object());
    assert_eq!(observer.response(5)["result"]["phase"], "menu");
}

#[test]
fn host_tick_and_agent_guest_calls_can_share_a_feature_plan() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    let calls = agent::sim::calls::CallsFeature(agent::sim::calls::shared());
    registry.add(&calls).unwrap();
    let settings = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    let skulls = crate::skulls_custom::Custom::new(settings, None, None, false);
    registry
        .add(&crate::skulls_custom::SkullsCustom(Arc::new(skulls)))
        .unwrap();
    registry
        .add(&Lobby {
            q: Shared::default(),
            live: None,
            machine_start: false,
        })
        .unwrap();
    let plan = registry.build(&hooks::Base).unwrap();
    assert!(plan.is_on("host-lobby"));
}

#[test]
fn menu_state_is_read_at_a_slice_even_before_the_first_game_tick() {
    struct Reader(guest_layout::testing::FakeGuest);
    impl agent::Reader for Reader {
        fn status(&mut self) -> agent::GameStatus {
            agent::GameStatus::default()
        }

        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(Vec::new())
        }

        fn with_guest(&mut self, visit: &mut dyn FnMut(&mut dyn guest_layout::GuestMemory)) -> bool {
            visit(&mut self.0);
            true
        }
    }

    let agent = Agent::new();
    let client = Client::connect(&agent);
    methods(
        &agent,
        &Shared::default(),
        Catalogue::default(),
        Variants::default(),
        false,
    );
    client.send(&agent, 1, method::HOST_LOBBY_STATE, json!({}));
    let mut menu = guest_layout::testing::FakeGuest::new();
    let game = 0x8200_0000;
    menu.put_u32(points::GAME_GLOBALS.base, game)
        .put_u8(game + 0x1120, 1)
        .put_u32(
            game + crate::telemetry::layout::OPTIONS,
            crate::level_boot::MODE_MAINMENU,
        );
    agent.between_slices(&mut Reader(menu), 0);
    let out = client.response(1);
    assert_eq!(out["result"]["phase"], "menu");
    assert_eq!(out["result"]["players"], json!([]));
    assert_eq!(out["result"]["ready"], false);
    assert_eq!(out["result"]["at"]["boundary"], "slice");
}
