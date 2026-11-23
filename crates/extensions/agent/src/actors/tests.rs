use agent_proto::Object;
use serde_json::json;

use super::*;
use crate::{GameStatus, Reader};

fn host() -> Report {
    Report {
        role: Role::Host,
        tick: Some(300),
        peers: vec![
            Peer {
                peer: 2,
                bytes_per_second: 640,
                packets: 40,
                bytes: 6000,
                keys: 2,
                digests: 10,
                records: 120,
                dropped: 3,
            },
            Peer {
                peer: 5,
                bytes_per_second: 120,
                ..Peer::default()
            },
        ],
        client: None,
    }
}

#[test]
fn host_reports_per_client_rates() {
    let r = reply(&Value::Null, host()).unwrap();
    assert_eq!(r["role"], "host");
    assert_eq!(r["tick"], 300);
    assert!(r["client"].is_null());
    assert_eq!(
        r["peers"][0],
        json!({
            "peer": 2,
            "bytes_per_second": 640,
            "packets": 40,
            "bytes": 6000,
            "keys": 2,
            "digests": 10,
            "records": 120,
            "dropped": 3,
        })
    );
    assert_eq!(r["peers"][1]["bytes_per_second"], 120);
}

#[test]
fn client_reports_puppets_and_waiting() {
    let report = Report {
        role: Role::Client,
        tick: Some(77),
        peers: Vec::new(),
        client: Some(Client {
            actors: 3,
            puppets: vec![Puppet {
                entity: 0x4400_0003,
                character: 5,
                squad: 7,
                team: 2,
                posture: 0x86,
                unit: 0xe1a4_0002,
            }],
            waiting: 2,
            lines_waiting: 1,
            mode_state_bytes: 20,
            digest_misses: 1,
            key_requested: true,
            diverged: false,
            host_log_head: Some(hex(&[0xab; 32])),
        }),
    };
    let r = reply(&json!({}), report).unwrap();
    assert_eq!(r["role"], "client");
    assert_eq!(r["peers"], json!([]));
    let c = &r["client"];
    assert_eq!(c["actors"], 3);
    assert_eq!(c["waiting"], 2);
    assert_eq!(c["lines_waiting"], 1);
    assert_eq!(c["puppets"][0]["entity"], 0x4400_0003u32);
    assert_eq!(c["puppets"][0]["unit"], 0xe1a4_0002u32);
    assert_eq!(c["digest_misses"], 1);
    assert_eq!(c["key_requested"], true);
    assert_eq!(c["diverged"], false);
    assert_eq!(c["host_log_head"], "ab".repeat(32));
}

#[test]
fn outside_a_distributed_game_the_role_is_off() {
    let r = reply(&Value::Null, Report::default()).unwrap();
    assert_eq!(
        r,
        json!({"role": "off", "tick": null, "peers": [], "client": null})
    );
}

#[test]
fn parameters_are_refused() {
    let e = reply(&json!({"peer": 2}), host()).unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

#[test]
fn the_method_is_registered() {
    let a = Agent::new();
    install(&a, Report::default);
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let conn = a.connect(tx);
    struct NoGuest;
    impl Reader for NoGuest {
        fn status(&mut self) -> GameStatus {
            GameStatus::default()
        }
        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(Vec::new())
        }
    }
    a.submit(
        conn,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#,
    );
    a.between_slices(&mut NoGuest, 0);
    let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(line.contains(METHOD), "{line}");
}
