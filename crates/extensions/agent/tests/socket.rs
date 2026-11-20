//! The socket end to end: a client connects, a "game thread" answers.
#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agent::{Agent, GameStatus, Reader, Server};
use agent_proto::{Client, EventKind, Object, code, method};
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

/// A unique socket path (Unix socket paths must stay short).
fn socket(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("h2a-{}-{name}.sock", std::process::id()))
}

/// Ticks every millisecond until stopped.
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

#[test]
fn a_client_calls_through_the_socket() {
    let path = socket("calls");
    let a = Agent::new();
    let server = Server::listen(&path, a.clone()).unwrap();
    let (stop, h) = game(&a);
    let mut c = Client::connect(&path).unwrap();
    c.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let s = c.call(method::STATUS, Value::Null).unwrap();
    assert_eq!(s["at"]["phase"], "menu");
    assert_eq!(s["at"]["map"], "mainmenu");
    let n = c.call(method::WORLD_COUNT, Value::Null).unwrap();
    assert_eq!(n["total"], 0);
    match c.call(method::PAUSE, Value::Null) {
        Err(agent_proto::CallError::Rpc(e)) => assert_eq!(e.code, code::NOT_CONTROLLER),
        other => panic!("{other:?}"),
    }
    // Pause, see the event, step, and let go by disconnecting.
    c.call(method::SUBSCRIBE, json!({"events": ["pause"]}))
        .unwrap();
    c.call(method::CLAIM, Value::Null).unwrap();
    c.call(method::PAUSE, Value::Null).unwrap();
    assert_eq!(c.next_event().unwrap().kind(), EventKind::Pause);
    let stepped = c.call(method::STEP, json!({"ticks": 2})).unwrap();
    assert_eq!(stepped["ticks"], 2);
    assert!(a.is_paused());
    drop(c);
    for _ in 0..500 {
        if !a.is_paused() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!a.is_paused(), "a dropped controller releases its pause");
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    drop(server);
    assert!(!path.exists(), "the socket file is removed");
}

#[test]
fn a_live_socket_is_not_taken_over_but_a_stale_one_is() {
    let path = socket("stale");
    let first = Server::listen(&path, Agent::new()).unwrap();
    assert!(Server::listen(&path, Agent::new()).is_err());
    drop(first);
    // A file left behind by a crashed run.
    std::os::unix::net::UnixListener::bind(&path).unwrap();
    let second = Server::listen(&path, Agent::new()).unwrap();
    drop(second);
}

#[test]
fn a_bad_line_gets_a_parse_error_and_the_connection_stays() {
    use std::io::{BufRead, BufReader, Write};
    let path = socket("parse");
    let a = Agent::new();
    let _server = Server::listen(&path, a.clone()).unwrap();
    let (stop, h) = game(&a);
    let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    s.write_all(b"nonsense\n{\"id\":2,\"method\":\"session.methods\"}\n")
        .unwrap();
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["error"]["code"], code::PARSE);
    line.clear();
    r.read_line(&mut line).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], 2);
    assert!(v["result"].as_array().unwrap().len() > 5);
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
}

#[test]
fn a_half_closed_client_gets_its_answer() {
    use std::io::{Read, Write};
    let path = socket("half");
    let a = Agent::new();
    let _server = Server::listen(&path, a.clone()).unwrap();
    let (stop, h) = game(&a);
    let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"{\"id\":1,\"method\":\"session.status\"}\n")
        .unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let mut out = String::new();
    // The server closes once it has answered.
    s.read_to_string(&mut out).unwrap();
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["result"]["at"]["map"], "mainmenu");
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
}

#[test]
fn a_file_that_is_not_a_socket_is_left_alone() {
    let path = socket("file");
    std::fs::write(&path, "notes").unwrap();
    assert!(Server::listen(&path, Agent::new()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "notes");
    std::fs::remove_file(&path).unwrap();
}
