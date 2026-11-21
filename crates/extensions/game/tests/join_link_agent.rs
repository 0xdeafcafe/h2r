//! Exercise the agent and the process-wide launch inbox together.
use agent_proto::{code, method};
use live_hle::join_link::{self, Phase};
use serde_json::{Value, json};

#[test]
fn website_join_reports_its_request_and_terminal_outcome() {
    let glue = game::agent_glue::AgentGlue::local();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let connection = glue.agent.connect(tx);
    let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
    let call = |method: &str, params: Value| -> Value {
        let message = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        glue.agent.submit(connection, &message.to_string());
        glue.after_slice(&rt, 0);
        serde_json::from_str(&rx.try_recv().unwrap()).unwrap()
    };
    let status = || call(method::LIVE_JOIN_STATUS, json!({}))["result"]["join"].clone();
    assert_eq!(status(), Value::Null);
    assert_eq!(
        call(method::LIVE_JOIN_STATUS, json!({"unknown": 0}))["error"]["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        call(
            method::LIVE_JOIN_OPEN,
            json!({"url": "https://live.example/"})
        )["error"]["code"],
        code::INVALID_PARAMS
    );

    let url = "h2recomp://join?server=https%3A%2F%2Flive.example&session=8000000000000001";
    let open = |url: &str| call(method::LIVE_JOIN_OPEN, json!({"url": url}));
    let id = open(url)["result"]["id"].as_u64().unwrap();
    assert_eq!(open(url)["result"]["id"], id);
    assert_eq!(
        open(&url.replace("0001", "0002"))["error"]["code"],
        code::BUSY
    );
    assert_eq!(
        status(),
        json!({"id": id, "session": "8000000000000001", "server": "https://live.example", "phase": "queued"})
    );

    let request = join_link::take_pending().unwrap();
    assert_eq!(request.id, id);
    assert_eq!(status()["phase"], "resolving");
    join_link::set_phase(
        id,
        Phase::Confirm {
            host: Some("Example".into()),
        },
    );
    assert_eq!(status()["host"], "Example");
    join_link::set_phase(id, Phase::Cancelled);
    assert_eq!(status()["phase"], "cancelled");

    join_link::set_enabled(false);
    assert_eq!(open(url)["error"]["code"], code::UNAVAILABLE);
}
