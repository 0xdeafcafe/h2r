//! A scripted fake game against a running server, for the web app and for
//! trying the API without the game:
//!
//! ```text
//! h2 live serve --db postgres://localhost/h2live_dev &
//! cargo run -p live-server --example fake_game -- http://127.0.0.1:8074 --seconds 60
//! ```
//!
//! It makes four accounts (Alpha, Bravo, Charlie, Delta, or `--prefix`ed
//! names), signs them in, stores an outline for `lockout`, plays a short
//! free-for-all on `midship` to the end, then hosts a Team Slayer game on
//! `lockout` over the ingest socket: frames at 10 Hz, a kill every two
//! seconds and presence pings every 30 for `--seconds`, left live at exit
//! (the server ends it as lost five minutes later and expires presence
//! after 90 seconds).

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

use live_proto::sign::{AccountRequest, EventBody, KeyPair, issue_token, sign_batch};
use live_proto::{ClientMessage, CreatedAccount, Discovery, IngestReply, lxm, streams, types};
use serde_json::{Value, json};

struct Player {
    key: KeyPair,
    account: CreatedAccount,
}

struct Server {
    base: String,
    id: String,
    agent: ureq::Agent,
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl Server {
    fn post(&self, path: &str, as_: Option<(&Player, &str)>, body: &str) -> Value {
        let mut req = self.agent.post(&format!("{}/v1{path}", self.base));
        if let Some((p, method)) = as_ {
            let token = issue_token(&p.key, &p.account.id, &self.id, method, now());
            req = req.set("authorization", &format!("Bearer {token}"));
        }
        self.send(req, body)
    }

    fn send(&self, req: ureq::Request, body: &str) -> Value {
        match req.send_string(body) {
            Ok(r) => r.into_json().unwrap_or(Value::Null),
            Err(ureq::Error::Status(code, r)) => {
                let url = r.get_url().to_string();
                panic!(
                    "HTTP {code} from {url}: {}",
                    r.into_string().unwrap_or_default()
                )
            }
            Err(e) => panic!("{e}"),
        }
    }

    fn event(&self, p: &Player, stream: &str, kind: &str, data: Value) {
        let body = EventBody::new(
            &p.key,
            &p.account.id,
            stream,
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        self.post("/events", Some((p, lxm::EVENTS_WRITE)), &body.sign(&p.key));
    }

    fn batch(&self, p: &Player, game_id: &str, events: &[(&str, Value)]) -> String {
        let at = time::OffsetDateTime::now_utc();
        let bodies: Vec<EventBody> = events
            .iter()
            .map(|(kind, data)| {
                EventBody::new(
                    &p.key,
                    &p.account.id,
                    &format!("game-{game_id}"),
                    kind,
                    data.clone(),
                    at,
                )
            })
            .collect();
        sign_batch(&p.key, &bodies)
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:8074".into());
    let seconds: u64 = arg(&args, "--seconds").map_or(30, |s| s.parse().expect("--seconds N"));
    let prefix = arg(&args, "--prefix").unwrap_or_default();
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    let discovery: Discovery = agent
        .get(&format!("{base}/.well-known/h2live.json"))
        .call()
        .expect("the server's discovery document")
        .into_json()
        .expect("discovery JSON");
    let server = Server {
        base: base.trim_end_matches('/').into(),
        id: discovery.id,
        agent,
    };

    let players: Vec<Player> = ["Alpha", "Bravo", "Charlie", "Delta"]
        .iter()
        .map(|name| {
            let key = KeyPair::generate();
            let tag = format!("{prefix}{name}");
            let req = AccountRequest::sign(&key, &tag, None, &server.id, now());
            let account: CreatedAccount =
                serde_json::from_value(server.post("/accounts", None, &req)).expect("an account");
            println!("account {} {}", account.gamertag, account.xuid);
            Player { key, account }
        })
        .collect();
    for p in &players {
        let xuid = p.account.xuid;
        server.event(
            p,
            &streams::presence(xuid),
            types::SIGNED_IN,
            json!({"xuid": xuid, "state": "online"}),
        );
    }
    let host = &players[0];

    let outline = json!({"map": "lockout", "bounds": {"min": [-20, -20, 0], "max": [20, 20, 12]},
        "layers": [{"z": [0, 6], "polygons": [[[-18, -18], [18, -18], [18, 18], [-18, 18]], [[-6, -6], [6, -6], [6, 6], [-6, 6]]]},
                   {"z": [6, 12], "polygons": [[[-10, -10], [10, -10], [10, 10], [-10, 10]]]}]});
    let req = server
        .agent
        .put(&format!("{}/v1/maps/lockout/outline", server.base))
        .set(
            "authorization",
            &format!(
                "Bearer {}",
                issue_token(
                    &host.key,
                    &host.account.id,
                    &server.id,
                    lxm::GAMES_WRITE,
                    now()
                )
            ),
        );
    match req.send_string(&outline.to_string()) {
        Ok(_) => println!("outline lockout"),
        Err(ureq::Error::Status(409, _)) => println!("outline lockout (already there)"),
        Err(e) => panic!("outline: {e}"),
    }

    let roster = |team_play: bool| -> Vec<Value> {
        players
            .iter()
            .enumerate()
            .map(|(i, p)| {
                json!({"index": i, "name": p.account.gamertag, "xuid": p.account.xuid,
                       "team": if team_play { (i % 2) as i32 } else { i as i32 }, "local": i == 0})
            })
            .collect()
    };
    let scores = |kills: &[u32; 4], deaths: &[u32; 4], team_play: bool| {
        let players: Vec<Value> = (0..4)
            .map(|i| json!({"index": i, "score": kills[i], "kills": kills[i], "deaths": deaths[i], "assists": 0}))
            .collect();
        let mut v = json!({"tick": 0, "players": players});
        if team_play {
            v["teams"] = json!([{"team": 0, "score": kills[0] + kills[2]}, {"team": 1, "score": kills[1] + kills[3]}]);
        }
        v
    };

    // A finished free-for-all.
    let ffa = server.post(
        "/games",
        Some((host, lxm::GAMES_WRITE)),
        &json!({"console": "Den", "map": "midship", "mode": "multiplayer", "variant": "Slayer", "team_play": false,
                "score_limit": 3, "players": roster(false)})
        .to_string(),
    )["game_id"]
        .as_str()
        .expect("a game id")
        .to_string();
    let (mut kills, mut deaths) = ([0u32; 4], [0u32; 4]);
    let mut events = Vec::new();
    for (tick, (killer, victim)) in [(0usize, 1usize), (2, 3), (0, 2), (1, 0), (0, 3)]
        .into_iter()
        .enumerate()
    {
        kills[killer] += 1;
        deaths[victim] += 1;
        let tick = 300 * (tick as u64 + 1);
        events.push((
            "PlayerKilled",
            json!({"tick": tick, "victim": victim, "killer": killer, "weapon": "battle_rifle",
            "kind": "kill", "pos": [3.0 * tick as f64 / 300.0, -2.0, 1.0]}),
        ));
        let mut s = scores(&kills, &deaths, false);
        s["tick"] = json!(tick);
        events.push(("ScoreChanged", s));
    }
    let mut end = scores(&kills, &deaths, false);
    end["reason"] = json!("finished");
    end["winner"] = json!({"player": 0});
    end["tick"] = json!(1800);
    events.push(("GameEnded", end));
    server.post(
        &format!("/games/{ffa}/events"),
        Some((host, lxm::GAMES_WRITE)),
        &server.batch(host, &ffa, &events),
    );
    println!("game {ffa} ended");

    // A live Team Slayer game over the ingest socket.
    let live_game = server.post(
        "/games",
        Some((host, lxm::GAMES_WRITE)),
        &json!({"console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Team Slayer", "team_play": true,
                "score_limit": 50, "players": roster(true)})
        .to_string(),
    )["game_id"]
        .as_str()
        .expect("a game id")
        .to_string();
    for p in &players[..2] {
        let xuid = p.account.xuid;
        server.event(
            p,
            &streams::presence(xuid),
            types::PRESENCE_CHANGED,
            json!({"xuid": xuid, "state": "playing", "game_id": live_game}),
        );
    }
    println!("game {live_game} live for {seconds}s");
    let ws_url = format!("{}/v1/ingest", server.base.replacen("http", "ws", 1));
    let (mut ws, _) = tungstenite::connect(&ws_url).expect("the ingest socket");
    let send = |ws: &mut tungstenite::WebSocket<_>, m: &ClientMessage| {
        ws.send(tungstenite::Message::Text(
            serde_json::to_string(m).unwrap().into(),
        ))
        .expect("sending");
    };
    let auth = || ClientMessage::Auth {
        token: issue_token(
            &host.key,
            &host.account.id,
            &server.id,
            lxm::GAMES_WRITE,
            now(),
        ),
    };
    send(&mut ws, &auth());
    let (mut kills, mut deaths) = ([0u32; 4], [0u32; 4]);
    let start = Instant::now();
    let mut last_kill = Instant::now();
    let mut last_auth = Instant::now();
    let mut last_ping = Instant::now();
    let mut tick = 0u64;
    let mut n = 0usize;
    while start.elapsed() < Duration::from_secs(seconds) {
        tick += 3;
        let t = tick as f64 / 30.0;
        let frame = live_proto::game::Frame {
            game_id: live_game.clone(),
            tick,
            players: (0..4)
                .map(|i| {
                    let a = t * 0.4 + i as f64 * TAU / 4.0;
                    let r = if i % 2 == 0 { 14.0 } else { 8.0 };
                    let yaw = a + TAU / 4.0;
                    live_proto::game::FramePlayer {
                        index: i,
                        alive: true,
                        pos: [r * a.cos(), r * a.sin(), if i == 3 { 8.0 } else { 1.0 }],
                        yaw,
                        vehicle: None,
                        species: Some(if i % 2 == 0 { "spartan" } else { "elite" }.into()),
                        aim: Some([yaw.cos(), yaw.sin(), 0.0]),
                        weapon: Some(
                            ["battle_rifle", "smg", "energy_blade", "rocket_launcher"][i as usize]
                                .into(),
                        ),
                        // A short burst every two seconds.
                        shots: (tick % 60 < 9 && i != 2).then_some(1),
                    }
                })
                .collect(),
        };
        send(
            &mut ws,
            &ClientMessage::Frame {
                frame: Box::new(frame),
            },
        );
        // Presence lasts 90 s without a ping.
        if last_ping.elapsed() > Duration::from_secs(30) {
            last_ping = Instant::now();
            for p in &players {
                server.post("/presence/ping", Some((p, lxm::EVENTS_WRITE)), "");
            }
        }
        if last_auth.elapsed() > Duration::from_secs(40) {
            send(&mut ws, &auth());
            last_auth = Instant::now();
        }
        if last_kill.elapsed() > Duration::from_secs(2) {
            last_kill = Instant::now();
            let (killer, victim) = [(0, 1), (1, 2), (2, 3), (3, 0), (0, 3), (2, 1)][n % 6];
            n += 1;
            kills[killer] += 1;
            deaths[victim] += 1;
            let mut s = scores(&kills, &deaths, true);
            s["tick"] = json!(tick);
            let jws = server.batch(host, &live_game, &[
                ("PlayerKilled", json!({"tick": tick, "victim": victim, "killer": killer, "weapon": "magnum", "kind": "kill",
                    "pos": [4.0 * (n as f64).cos(), 4.0 * (n as f64).sin(), 1.0]})),
                ("ScoreChanged", s),
            ]);
            send(
                &mut ws,
                &ClientMessage::Events {
                    game_id: live_game.clone(),
                    jws,
                },
            );
            match ws.read().expect("a reply") {
                tungstenite::Message::Text(t) => match serde_json::from_str::<IngestReply>(&t) {
                    Ok(IngestReply::Appended { position, .. }) => {
                        println!("kill {killer} -> {victim} at {position}")
                    }
                    other => println!("ingest: {other:?}"),
                },
                other => println!("ingest: {other:?}"),
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = ws.close(None);
    println!("done; {live_game} is still live");
}
