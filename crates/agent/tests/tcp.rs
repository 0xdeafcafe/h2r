//! TCP end to end (the Xbox app's transport; docs/runtime/xbox.md, "Remote
//! control"): a client connects over loopback, a "game thread" answers,
//! and host methods (pad input, the profiler) come back through it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent::{Access, Agent, GameStatus, Reader, Server};
use agent_proto::{CallError, Client, Endpoint, Object, PadParams, RpcError, code, method};
use serde_json::{Value, json};

struct Menu;

impl Reader for Menu {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(1),
            map: Some("mainmenu".into()),
            mode: Some("main_menu".into()),
            bsp: None,
        }
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(vec![])
    }
}

fn game(a: &Agent) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (s, a) = (Arc::clone(&stop), a.clone());
    let h = thread::spawn(move || {
        while !s.load(Ordering::Relaxed) {
            a.on_tick(&mut Menu);
            a.between_slices(&mut Menu, 0);
            thread::sleep(Duration::from_millis(1));
        }
    });
    (stop, h)
}

fn listen(a: &Agent) -> (Server, Endpoint) {
    // Port 0: the system picks one; the server says which.
    let server = Server::listen_on(&Endpoint::Tcp("127.0.0.1:0".into()), a.clone()).unwrap();
    let e = server.endpoint();
    assert!(
        matches!(&e, Endpoint::Tcp(addr) if !addr.ends_with(":0")),
        "{e}"
    );
    (server, e)
}

fn rpc_code(r: Result<Value, CallError>) -> i64 {
    match r {
        Err(CallError::Rpc(e)) => e.code,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_client_calls_over_tcp_and_host_methods_answer() {
    let a = Agent::new();
    let pads: Arc<Mutex<Vec<PadParams>>> = Arc::default();
    let p = Arc::clone(&pads);
    a.add_method(method::INPUT_PAD, Access::Act, move |v| {
        let params: PadParams = serde_json::from_value(v)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let n = params.steps.len();
        p.lock().unwrap().push(params);
        Ok(json!({ "queued": n }))
    });
    a.add_method(method::HOST_PERF, Access::Read, |_| {
        Ok(json!({ "line": "host perf: frames 100" }))
    });
    let (server, endpoint) = listen(&a);
    let (stop, h) = game(&a);

    // `Client::connect` takes the endpoint as a path-like string too, as
    // `h2 agent --socket tcp://...` passes it.
    let mut c = Client::connect(std::path::Path::new(&endpoint.to_string())).unwrap();
    c.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let s = c.call(method::STATUS, Value::Null).unwrap();
    assert_eq!(s["at"]["map"], "mainmenu");
    let methods = c.call(method::METHODS, Value::Null).unwrap();
    let methods: Vec<&str> = methods
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(methods.contains(&method::INPUT_PAD) && methods.contains(&method::STATUS));
    assert!(
        !methods.contains(&method::SAMPLE_START),
        "only registered host methods are listed"
    );

    let r = c
        .call(
            method::INPUT_PAD,
            json!({"steps": [{"buttons": ["a"], "ms": 100}, {"left": [0, 1]}]}),
        )
        .unwrap();
    assert_eq!(r["queued"], 2);
    assert_eq!(r["at"]["map"], "mainmenu", "results get the stamp");
    assert_eq!(pads.lock().unwrap()[0].steps[0].buttons, ["a"]);
    assert_eq!(
        rpc_code(c.call(method::INPUT_PAD, json!({"steps": 3}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        c.call(method::HOST_PERF, Value::Null).unwrap()["line"],
        "host perf: frames 100"
    );
    assert_eq!(
        rpc_code(c.call(method::SAMPLE_START, Value::Null)),
        code::METHOD_NOT_FOUND
    );

    // While another connection holds control, acting is refused; reading
    // is not.
    let mut other = Client::connect_to(&endpoint).unwrap();
    other.set_timeout(Some(Duration::from_secs(10))).unwrap();
    other.call(method::CLAIM, Value::Null).unwrap();
    assert_eq!(
        rpc_code(c.call(method::INPUT_PAD, json!({"steps": []}))),
        code::NOT_CONTROLLER
    );
    c.call(method::HOST_PERF, Value::Null).unwrap();
    other.call(method::INPUT_PAD, json!({"steps": []})).unwrap();
    drop(other);
    // Its disconnect releases control.
    let mut ok = false;
    for _ in 0..500 {
        if c.call(method::INPUT_PAD, json!({"steps": []})).is_ok() {
            ok = true;
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(ok, "control is released when its connection closes");

    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    drop(server);
}

#[test]
fn a_busy_port_is_an_error() {
    let a = Agent::new();
    let (_server, endpoint) = listen(&a);
    assert!(Server::listen_on(&endpoint, Agent::new()).is_err());
}

#[test]
#[should_panic(expected = "core method")]
fn core_methods_cannot_be_replaced() {
    Agent::new().add_method(method::STATUS, Access::Read, |_| Ok(Value::Null));
}
