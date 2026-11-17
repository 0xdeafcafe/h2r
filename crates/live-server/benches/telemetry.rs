//! The server side of the live telemetry pipeline, timed in process on a
//! SQLite store (docs/live/live-telemetry.md, docs/live/live-api.md):
//!
//! 1. ingest: packed frames through what the ingest socket does with a
//!    binary message (`pack::records` -> `Decoder::decode` ->
//!    `Live::ingest_frame`), every game sending at 15 Hz so each frame is
//!    fanned out and every 15th recorded, as in a real game; and the same
//!    frames back to back on one game, where all but 15 a second are
//!    throttled.
//! 2. events: signed batches of 1 and 32 game events through
//!    `Live::append_game_batch` (JWS verification, nonces, store append,
//!    waiting for the read model).
//! 3. fan-out: N stream readers (`/v1/stream`, packed and JSON) on
//!    loopback WebSockets while games send frames; the process CPU time
//!    above a run with no readers, per frame and reader. That includes
//!    the readers' own receiving (tungstenite), so it is an upper bound;
//!    the per-reader work of `run_stream` (visibility filter plus packing
//!    or serialising) is also timed alone.
//!
//! The frame is invented (no game data): 16 players, 64 vehicles (16 of
//! them turrets riding another), 100 actors, a few hits a frame, 12
//! machines and 40 moved or gone placements, all moving a little each
//! frame.
//!
//! `cargo bench -p live-server --bench telemetry` prints a summary; with
//! `TELEMETRY_BENCH_OUT=<dir>` it also writes `<dir>/server.json`. Env:
//! `TELEMETRY_BENCH_SECS` (each timed run, default 1.5),
//! `TELEMETRY_BENCH_RUNS` (runs per measurement, median taken, default 3).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use live_proto::game::{
    Frame, FrameActor, FrameHit, FrameMachine, FrameObject, FramePlayer, FrameVehicle, NewGame,
    ObjectKind,
};
use live_proto::sign::{AccountRequest, EventBody, KeyPair, issue_token, sign_batch};
use live_proto::{Author, StreamMessage, lxm, pack};
use live_server::games::FrameOutcome;
use live_server::sqlite::SqliteStore;
use live_server::{Live, Policy, ServerIdentity, Viewer, http};
use serde_json::{Value, json};

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn secs() -> Duration {
    Duration::from_secs_f64(env_f64("TELEMETRY_BENCH_SECS", 1.5))
}

fn runs() -> usize {
    env_f64("TELEMETRY_BENCH_RUNS", 3.0).max(1.0) as usize
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.total_cmp(b));
    xs[xs.len() / 2]
}

/// User plus system CPU time of the whole process.
fn cpu_time() -> Duration {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: getrusage writes the struct it is given.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(u.ru_utime) + tv(u.ru_stime)
}

/// The 1-minute load average: other work on the machine skews every
/// number here, so the output says how busy it was.
fn load_average() -> f64 {
    let mut l = [0f64; 3];
    // SAFETY: getloadavg writes at most the 3 samples it is given room for.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    if n > 0 { l[0] } else { f64::NAN }
}

// ---------------------------------------------------------------------------
// The frame.

fn round(v: f64) -> f64 {
    f64::from(v as f32)
}

fn r3(v: [f64; 3]) -> [f64; 3] {
    v.map(round)
}

fn dir(yaw: f64, pitch: f64) -> [f64; 3] {
    r3([
        yaw.cos() * pitch.cos(),
        yaw.sin() * pitch.cos(),
        pitch.sin(),
    ])
}

/// A deterministic mover: unit `i` circles its own centre.
fn at(i: u32, t: f64, extent: f64) -> ([f64; 3], f64) {
    let h = |k: u32| {
        let x = (i.wrapping_mul(2_654_435_761) ^ k.wrapping_mul(40_503)) as f64;
        (x.sin() * 43_758.545).fract().abs()
    };
    let (cx, cy, cz) = (
        (h(1) * 2.0 - 1.0) * extent,
        (h(2) * 2.0 - 1.0) * extent,
        h(3) * 6.0,
    );
    let radius = 2.0 + h(4) * 10.0;
    let a = h(5) * std::f64::consts::TAU + t * (1.0 + h(6) * 3.5) / radius;
    (
        r3([
            cx + radius * a.cos(),
            cy + radius * a.sin(),
            cz + 0.2 * (a * 3.0).sin(),
        ]),
        round(a + std::f64::consts::FRAC_PI_2),
    )
}

const WEAPONS: [&str; 6] = [
    "battle_rifle",
    "smg",
    "magnum",
    "plasma_rifle",
    "carbine",
    "sniper_rifle",
];
const SPECIES: [&str; 5] = ["grunt", "jackal", "elite", "marine", "brute"];
const VEHICLES: [&str; 4] = ["warthog", "ghost", "banshee", "scorpion"];

/// Frame `n` (15 Hz) of game `game_id`.
fn frame(game_id: &str, n: u64) -> Frame {
    let t = n as f64 / 15.0;
    let tick = 9000 + 2 * n;
    let players = (0..16u32)
        .map(|i| {
            let (pos, yaw) = at(i, t, 40.0);
            let firing = (n / 15 + u64::from(i)).is_multiple_of(3);
            FramePlayer {
                index: i,
                alive: true,
                pos,
                yaw,
                vehicle: (i < 4).then(|| format!("{:08x}", 0x9000_0000u32 + i)),
                species: Some(if i % 2 == 0 { "spartan" } else { "elite" }.into()),
                aim: Some(dir(yaw + 0.1 * (t * 2.0).sin(), 0.05)),
                weapon: Some(WEAPONS[i as usize % WEAPONS.len()].into()),
                shots: firing.then_some(2),
                stance: Some(if i < 4 { "vehicle" } else { "stand" }.into()),
                speed: Some(3.0),
                actions: (n + u64::from(i))
                    .is_multiple_of(40)
                    .then(|| vec!["reload".into()]),
                grenade: Some("frag".into()),
            }
        })
        .collect();
    // 48 hulls, 16 of them driven; 16 turrets riding the first 16.
    let mut vehicles: Vec<FrameVehicle> = (0..48u32)
        .map(|i| {
            let moving = i < 16;
            let (pos, yaw) = at(100 + i, if moving { t } else { 0.0 }, 60.0);
            FrameVehicle {
                id: format!("{:08x}", 0x9000_0000u32 + i),
                kind: VEHICLES[i as usize % VEHICLES.len()].into(),
                pos,
                forward: dir(yaw, 0.0),
                up: [0.0, 0.0, 1.0],
                occupied: moving,
                parent: None,
                aim: None,
            }
        })
        .collect();
    vehicles.extend((0..16u32).map(|i| FrameVehicle {
        id: format!("{:08x}", 0x9100_0000u32 + i),
        kind: "chaingun".into(),
        pos: [-0.6, 0.0, 0.75],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        occupied: true,
        parent: Some(format!("{:08x}", 0x9000_0000u32 + i)),
        aim: Some(dir(0.8 * (t * 1.7 + f64::from(i)).sin(), 0.1)),
    }));
    let actors = (0..100u32)
        .map(|i| {
            let guard = i % 4 == 0;
            let (pos, yaw) = at(200 + i, if guard { 0.0 } else { t }, 60.0);
            let species = SPECIES[i as usize % SPECIES.len()];
            FrameActor {
                id: format!("{:08x}", 0xe100_0000u32 + i),
                species: species.into(),
                pos,
                yaw,
                aim: Some(dir(yaw, 0.02)),
                weapon: Some(WEAPONS[i as usize % WEAPONS.len()].into()),
                shots: (n / 10 + u64::from(i)).is_multiple_of(7).then_some(2),
                team: Some(
                    if species == "marine" {
                        "human"
                    } else {
                        "covenant"
                    }
                    .into(),
                ),
                vehicle: None,
                stance: Some("stand".into()),
                speed: Some(if guard { 0.0 } else { 2.5 }),
                actions: None,
                grenade: Some("plasma".into()),
            }
        })
        .collect();
    let hits = (0..n % 5)
        .map(|k| FrameHit {
            from: Some(format!("p{}", (n + k) % 16)),
            to: Some(format!(
                "{:08x}",
                0xe100_0000u32 + ((n * 7 + k) % 100) as u32
            )),
            weapon: Some(WEAPONS[(k as usize) % WEAPONS.len()].into()),
            pos: at(200 + ((n * 7 + k) % 100) as u32, t, 60.0).0,
            amount: Some(0.25),
            kind: Some("bullet".into()),
        })
        .collect();
    let machines = (0..12u32)
        .filter(|i| n == 0 || *i < 3)
        .map(|i| {
            let travel = if i < 3 {
                ((t * 0.2 + f64::from(i)).sin() + 1.0) / 2.0
            } else {
                0.0
            };
            FrameMachine {
                node: Some(format!("door_{i}")),
                id: format!("{:08x}", 0xd200_0000u32 + i),
                kind: if i == 0 { "elevator" } else { "door" }.into(),
                pos: r3([f64::from(i) * 7.0, -20.0, 3.0 * travel]),
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                position: Some(round(travel)),
            }
        })
        .collect();
    let mut objects: Vec<FrameObject> = (0..30u32)
        .map(|i| {
            let (p, yaw) = at(400 + i, if i < 5 { t.min(2.0) } else { 0.0 }, 60.0);
            FrameObject {
                k: ObjectKind::Crate,
                i,
                p: Some(p),
                f: Some(dir(yaw, 0.0)),
                u: Some([0.0, 0.0, 1.0]),
                gone: false,
            }
        })
        .collect();
    objects.extend((0..10).map(|i| FrameObject {
        k: ObjectKind::Weapon,
        i,
        p: None,
        f: None,
        u: None,
        gone: true,
    }));
    objects.sort_by_key(|o| (o.k, o.i));
    Frame {
        game_id: game_id.into(),
        tick,
        players,
        vehicles,
        actors,
        hits,
        bsp: Some(0),
        machines,
        objects,
    }
}

/// One game's console: its frames packed as the runtime sends them.
struct Console {
    game_id: String,
    n: u64,
    packer: pack::Encoder,
    /// The server's side of this game's ingest socket.
    decoder: pack::Decoder,
    /// Messages made ahead, so making them stays out of a timed window.
    ready: std::collections::VecDeque<Vec<u8>>,
    /// When the server last took a frame of this game.
    last: Option<Instant>,
}

impl Console {
    fn new(game_id: String) -> Self {
        Console {
            game_id,
            n: 0,
            // As examples/fake_game.rs: a keyframe every 10 s at 15 Hz.
            packer: pack::Encoder::new(0, 150),
            decoder: pack::Decoder::new(),
            ready: Default::default(),
            last: None,
        }
    }

    /// Makes enough messages ahead for `window` at 15 Hz.
    fn prepare(&mut self, window: Duration) {
        let n = (window.as_secs_f64() * 15.0).ceil() as usize + 4;
        while self.ready.len() < n {
            let msg = self.make();
            self.ready.push_back(msg);
        }
    }

    /// The next frame, as a binary ingest message of one record.
    fn next_message(&mut self) -> Vec<u8> {
        self.ready.pop_front().unwrap_or_else(|| self.make())
    }

    fn make(&mut self) -> Vec<u8> {
        let f = frame(&self.game_id, self.n);
        self.n += 1;
        let (bytes, _) = self.packer.encode(&f).expect("packs");
        let mut msg = Vec::with_capacity(bytes.len() + 4);
        pack::push_record(&mut msg, &bytes);
        msg
    }
}

/// What the ingest socket does with one binary message
/// (`ingest::ingest_packed`): every record decoded and ingested.
async fn ingest_message(
    live: &Live,
    author: &Author,
    decoder: &mut pack::Decoder,
    msg: &[u8],
) -> FrameOutcome {
    let mut last = FrameOutcome::Throttled;
    for record in pack::records(msg) {
        let d = decoder.decode(record.expect("a record")).expect("decodes");
        last = live.ingest_frame(author, d.frame).await.expect("ingests");
    }
    last
}

// ---------------------------------------------------------------------------
// The server.

struct Server {
    live: Arc<Live>,
    key: KeyPair,
    author: Author,
    id: String,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Server {
    async fn start() -> Server {
        let dir = std::env::temp_dir().join(format!(
            "h2live-bench-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = ServerIdentity::open(&dir.join("id"), Some("live.test")).unwrap();
        let store = SqliteStore::open(&dir.join("live.db")).await.unwrap();
        let policy = Policy {
            // The bench is one author writing as fast as it can.
            write_rate: (1e9, u32::MAX),
            game_idle_s: 3600,
            ..Policy::default()
        };
        let live = Live::start(Arc::new(store), identity, policy)
            .await
            .unwrap();
        let key = KeyPair::generate();
        let created = live
            .create_account(&AccountRequest::sign(
                &key,
                "Host",
                None,
                "live.test",
                live_server::app::now(),
            ))
            .await
            .unwrap();
        Server {
            live,
            key,
            author: Author::parse(&created.id).unwrap(),
            id: created.id,
            dir,
        }
    }

    /// A 16-player game hosted by the bench's account.
    async fn start_game(&self) -> String {
        let players: Vec<Value> = (0..16)
            .map(|i| json!({"index": i, "name": format!("P{i}"), "team": i % 2}))
            .collect();
        let new: NewGame = serde_json::from_value(json!({
            "map": "lockout", "mode": "multiplayer", "team_play": true,
            "players": players,
        }))
        .unwrap();
        let token = issue_token(
            &self.key,
            &self.id,
            "live.test",
            lxm::GAMES_WRITE,
            live_server::app::now(),
        );
        self.live
            .start_game(Some(&token), new)
            .await
            .unwrap()
            .game_id
    }
}

// ---------------------------------------------------------------------------
// 1. Ingest.

/// Frame interval per game: just over the server's 15 Hz throttle
/// (66 ms), counted from when the server last took one.
const PACE: Duration = Duration::from_millis(67);

struct IngestRun {
    us_per_frame: f64,
    sent: u64,
    throttled: u64,
}

/// `games` consoles each sending at 15 Hz; only the ingest calls are
/// timed.
async fn ingest_paced(s: &Server, consoles: &mut [Console], window: Duration) -> IngestRun {
    let mut busy = Duration::ZERO;
    let (mut sent, mut throttled) = (0u64, 0u64);
    for c in consoles.iter_mut() {
        c.prepare(window);
    }
    let start = Instant::now();
    while start.elapsed() < window {
        for c in consoles.iter_mut() {
            // No sooner than the throttle allows after this game's last
            // frame (a slow round must not be followed by an early one).
            if let Some(due) = c.last.map(|l| l + PACE) {
                tokio::time::sleep_until(due.into()).await;
            }
            let msg = c.next_message();
            let t = Instant::now();
            let outcome = ingest_message(&s.live, &s.author, &mut c.decoder, &msg).await;
            let done = Instant::now();
            busy += done - t;
            c.last = Some(done);
            match outcome {
                FrameOutcome::Sent { .. } => sent += 1,
                FrameOutcome::Throttled => throttled += 1,
            }
        }
    }
    IngestRun {
        us_per_frame: busy.as_secs_f64() * 1e6 / (sent + throttled) as f64,
        sent,
        throttled,
    }
}

/// One console back to back: 15 frames a second fan out, the rest are
/// throttled (validated, their one-shot data folded into the next sent
/// frame). Only the ingest calls are timed.
async fn ingest_flat_out(s: &Server, c: &mut Console, window: Duration) -> f64 {
    let mut frames = 0u64;
    let mut busy = Duration::ZERO;
    while busy < window {
        let msg = c.next_message();
        let t = Instant::now();
        ingest_message(&s.live, &s.author, &mut c.decoder, &msg).await;
        busy += t.elapsed();
        frames += 1;
    }
    busy.as_secs_f64() * 1e6 / frames as f64
}

/// Times `f` over enough calls to fill `window`; µs per call.
fn per_call(window: Duration, mut f: impl FnMut()) -> f64 {
    for _ in 0..20 {
        f();
    }
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < window {
        for _ in 0..16 {
            f();
        }
        n += 16;
    }
    start.elapsed().as_secs_f64() * 1e6 / n as f64
}

/// The pieces of one admitted delta frame's path, each alone.
fn ingest_parts() -> Value {
    let window = Duration::from_millis(300);
    let id = "0123456789abcdef0123456789abcdef";
    let frames: Vec<Frame> = (0..40).map(|n| frame(id, n)).collect();
    let mut enc = pack::Encoder::new(0, 1000);
    let packed: Vec<Vec<u8>> = frames.iter().map(|f| enc.encode(f).unwrap().0).collect();
    // Decoding a delta needs the stream's state: decode the run once,
    // then time one delta against a decoder at the frame before it.
    let mut warm = pack::Decoder::new();
    for p in &packed[..39] {
        warm.decode(p).unwrap();
    }
    let decoded = warm.decode(&packed[39]).unwrap().frame;
    let decode_key = per_call(window, || {
        let mut d = pack::Decoder::new();
        std::hint::black_box(d.decode(&packed[0]).unwrap());
    });
    let validate = per_call(window, || {
        live_proto::game::validate_frame(std::hint::black_box(&decoded)).unwrap();
    });
    let clone = per_call(window, || {
        std::hint::black_box(decoded.clone());
    });
    let normalize = per_call(window, || {
        let mut f = decoded.clone();
        live_proto::game::normalize_frame(&mut f);
        std::hint::black_box(f);
    }) - clone;
    let encode_delta = per_call(window, || {
        let mut e = pack::Encoder::new(0, 1000);
        e.encode(&frames[38]).unwrap();
        std::hint::black_box(e.encode(&frames[39]).unwrap());
    }) - per_call(window, || {
        let mut e = pack::Encoder::new(0, 1000);
        std::hint::black_box(e.encode(&frames[38]).unwrap());
    });
    let encode_key = per_call(window, || {
        std::hint::black_box(pack::encode_keyframe(&decoded).unwrap());
    });
    let json = per_call(window, || {
        std::hint::black_box(serde_json::to_string(&decoded).unwrap());
    });
    json!({
        "decode_keyframe_us": decode_key,
        "validate_us": validate,
        "clone_us": clone,
        "normalize_us": normalize,
        "encode_delta_us": encode_delta,
        "encode_keyframe_us": encode_key,
        "json_us": json,
        "json_bytes": serde_json::to_string(&decoded).unwrap().len(),
        "keyframe_bytes": packed[0].len(),
        "delta_bytes": packed[1..].iter().map(Vec::len).sum::<usize>() / (packed.len() - 1),
    })
}

async fn bench_ingest(s: &Server) -> Value {
    const GAMES: usize = 64;
    let mut consoles = Vec::new();
    for _ in 0..GAMES {
        consoles.push(Console::new(s.start_game().await));
    }
    // Warm up: every game's keyframe and a few deltas.
    ingest_paced(s, &mut consoles, Duration::from_millis(500)).await;
    let mut paced = Vec::new();
    for _ in 0..runs() {
        paced.push(ingest_paced(s, &mut consoles, secs()).await);
    }
    let (sent, throttled) = paced
        .iter()
        .fold((0, 0), |(a, b), r| (a + r.sent, b + r.throttled));
    let us = median(paced.iter().map(|r| r.us_per_frame).collect());

    let mut flat_console = Console::new(s.start_game().await);
    ingest_flat_out(s, &mut flat_console, Duration::from_millis(300)).await;
    let mut flat = Vec::new();
    for _ in 0..runs() {
        flat.push(ingest_flat_out(s, &mut flat_console, secs()).await);
    }
    let flat_us = median(flat);

    let parts = ingest_parts();
    let delta = &consoles[0];
    let mut probe = Console::new(delta.game_id.clone());
    let key_bytes = probe.next_message().len();
    let delta_bytes = (0..15).map(|_| probe.next_message().len()).sum::<usize>() / 15;
    json!({
        "frames_per_s": 1e6 / us,
        "us_per_frame": us,
        "frame_bytes": delta_bytes,
        "keyframe_bytes": key_bytes,
        "games": GAMES,
        "sent": sent,
        "throttled": throttled,
        "throttled_path": {"frames_per_s": 1e6 / flat_us, "us_per_frame": flat_us},
        "parts": parts,
    })
}

// ---------------------------------------------------------------------------
// 2. Events.

async fn bench_events(s: &Server, batch: usize) -> Value {
    let game_id = s.start_game().await;
    let stream = format!("game-{game_id}");
    let mut tick = 100u64;
    let mut sign = |count: usize| -> Vec<String> {
        (0..count)
            .map(|_| {
                let at = time::OffsetDateTime::now_utc();
                let bodies: Vec<EventBody> = (0..batch)
                    .map(|k| {
                        tick += 1;
                        let (kind, data) = if k % 2 == 0 {
                            (
                                "PlayerKilled",
                                json!({"tick": tick, "victim": tick % 16, "killer": (tick + 1) % 16,
                                    "weapon": "battle_rifle", "kind": "kill", "pos": [1.5, -2.0, 3.25]}),
                            )
                        } else {
                            (
                                "ScoreChanged",
                                json!({"tick": tick,
                                    "players": [{"index": 0, "score": 1, "kills": 1, "deaths": 0, "assists": 0}],
                                    "teams": [{"team": 0, "score": 1}, {"team": 1, "score": 0}]}),
                            )
                        };
                        EventBody::new(&s.key, &s.id, &stream, kind, data, at)
                    })
                    .collect();
                sign_batch(&s.key, &bodies)
            })
            .collect()
    };
    let chunk = (256 / batch).max(8);
    let mut one_run = async |window: Duration| -> f64 {
        let mut busy = Duration::ZERO;
        let mut batches = 0u64;
        while busy < window {
            for jws in sign(chunk) {
                let t = Instant::now();
                s.live
                    .append_game_batch(&s.author, &game_id, &jws)
                    .await
                    .expect("appends");
                busy += t.elapsed();
                batches += 1;
            }
        }
        busy.as_secs_f64() * 1e6 / batches as f64
    };
    one_run(Duration::from_millis(300)).await;
    let mut per_batch = Vec::new();
    for _ in 0..runs() {
        per_batch.push(one_run(secs()).await);
    }
    let us = median(per_batch);
    // Opening and verifying the batch alone (what the signature costs).
    let jws = sign(1).pop().unwrap();
    let verify = per_call(Duration::from_millis(300), || {
        let (parsed, _) = live_proto::sign::SignedEvent::open_batch(&jws).unwrap();
        parsed.verify(s.key.public()).unwrap();
    });
    json!({
        "batch": batch,
        "events_per_s": batch as f64 * 1e6 / us,
        "us_per_batch": us,
        "us_per_event": us / batch as f64,
        "open_verify_us": verify,
        "jws_bytes": jws.len(),
    })
}

// ---------------------------------------------------------------------------
// 3. Fan-out.

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A stream reader that counts the frames it gets.
async fn reader(mut ws: Socket, packed: bool, frames: Arc<AtomicU64>) {
    use tokio_tungstenite::tungstenite::Message;
    while let Some(Ok(msg)) = ws.next().await {
        match msg {
            Message::Binary(b) if packed => {
                frames.fetch_add(pack::records(&b).count() as u64, Ordering::Relaxed);
            }
            Message::Text(t) if t.as_str().starts_with("{\"kind\":\"frame\"") => {
                frames.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

struct FanoutRun {
    cpu: Duration,
    frames: u64,
    received: u64,
}

/// `games` consoles at 15 Hz with `readers` stream readers attached (each
/// of every game, or with `split` each of one, round robin); the process
/// CPU time over the window.
async fn fanout_run(
    s: &Server,
    addr: std::net::SocketAddr,
    consoles: &mut [Console],
    readers: usize,
    packed: bool,
    split: bool,
    window: Duration,
) -> FanoutRun {
    let query = if packed {
        "?exclude=GameFrame&frames=packed"
    } else {
        "?exclude=GameFrame"
    };
    let count = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for i in 0..readers {
        let query = if split {
            format!("{query}&games={}", consoles[i % consoles.len()].game_id)
        } else {
            query.to_string()
        };
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream{query}"))
            .await
            .unwrap();
        // caught_up first.
        let first = ws.next().await.unwrap().unwrap();
        let text = first.into_text().unwrap();
        assert!(matches!(
            serde_json::from_str::<StreamMessage>(&text).unwrap(),
            StreamMessage::CaughtUp { .. }
        ));
        tasks.push(tokio::spawn(reader(ws, packed, count.clone())));
    }
    // Warm up.
    ingest_paced(s, consoles, Duration::from_millis(400)).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    for c in consoles.iter_mut() {
        c.prepare(window);
    }
    let received0 = count.load(Ordering::Relaxed);
    let cpu0 = cpu_time();
    let r = ingest_paced(s, consoles, window).await;
    // Let the last packed batches go out.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let cpu = cpu_time() - cpu0;
    let received = count.load(Ordering::Relaxed) - received0;
    for t in tasks {
        t.abort();
    }
    FanoutRun {
        cpu,
        frames: r.sent,
        received,
    }
}

/// The per-reader work `run_stream` does for each live frame, without the
/// socket: the visibility filter, then a record pushed onto the reader's
/// packed batch, or the frame cloned and serialised for a JSON reader.
fn per_reader_parts(live: &Live, f: &live_server::games::LiveFrame) -> Value {
    let window = Duration::from_millis(300);
    let viewer = Viewer::Anonymous;
    let games: Vec<String> = Vec::new();
    let filter = per_call(window, || {
        std::hint::black_box(
            (games.is_empty() || games.contains(&f.frame.game_id)) && live.frame_shows(f, viewer),
        );
    });
    let packed = f.packed.as_ref().expect("packs");
    let mut sent = std::collections::HashMap::<u64, u64>::new();
    let mut bytes = Vec::new();
    let pack = per_call(window, || {
        sent.insert(f.sid, f.seq);
        pack::push_record(&mut bytes, packed);
        if bytes.len() > 1 << 20 {
            bytes.clear();
        }
    });
    let json = per_call(window, || {
        std::hint::black_box(
            serde_json::to_string(&StreamMessage::Frame {
                frame: Box::new(f.frame.clone()),
            })
            .unwrap(),
        );
    });
    json!({"filter_us": filter, "packed_add_us": pack, "json_us": json, "packed_bytes": packed.len()})
}

async fn bench_fanout(s: &Server) -> Value {
    const GAMES: usize = 8;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, http::router(s.live.clone())).into_future());
    let mut consoles = Vec::new();
    for _ in 0..GAMES {
        consoles.push(Console::new(s.start_game().await));
    }

    // A delta frame as readers get it, for the per-reader parts.
    let mut rx = s.live.frames();
    ingest_paced(s, &mut consoles, Duration::from_millis(300)).await;
    let mut live_frame = None;
    while let Ok(f) = rx.try_recv() {
        if !f.keyframe {
            live_frame = Some(f);
        }
    }
    drop(rx);
    let parts = per_reader_parts(&s.live, &live_frame.expect("a delta frame"));

    let cpu_per_frame = |r: &FanoutRun| r.cpu.as_secs_f64() * 1e6 / r.frames as f64;
    let mut base = Vec::new();
    for _ in 0..runs() {
        base.push(cpu_per_frame(
            &fanout_run(s, addr, &mut consoles, 0, true, false, secs()).await,
        ));
    }
    let base_us = median(base);
    let mut out = serde_json::Map::new();
    for readers in [1usize, 10, 100] {
        let mut modes = serde_json::Map::new();
        for packed in [true, false] {
            let mut cpu = Vec::new();
            let mut delivered = Vec::new();
            for _ in 0..runs() {
                let r = fanout_run(s, addr, &mut consoles, readers, packed, false, secs()).await;
                cpu.push(cpu_per_frame(&r));
                delivered.push(r.received as f64 / (r.frames * readers as u64) as f64);
            }
            let total = median(cpu) - base_us;
            modes.insert(
                if packed { "packed" } else { "json" }.into(),
                json!({
                    "us_per_frame_total": total,
                    "us_per_frame_per_reader": total / readers as f64,
                    "delivered": median(delivered),
                }),
            );
        }
        let packed = modes["packed"].clone();
        let mut entry = json!({
            "readers": readers,
            "us_per_frame_per_reader": packed["us_per_frame_per_reader"],
            "us_per_frame_total": packed["us_per_frame_total"],
        });
        for (k, v) in modes {
            entry[k] = v;
        }
        out.insert(readers.to_string(), entry);
    }
    // Readers split across the games, each following one: packed.
    let mut split = serde_json::Map::new();
    for readers in [8usize, 80] {
        let mut cpu = Vec::new();
        let mut delivered = Vec::new();
        for _ in 0..runs() {
            let r = fanout_run(s, addr, &mut consoles, readers, true, true, secs()).await;
            cpu.push(cpu_per_frame(&r));
            delivered.push(r.received as f64 * GAMES as f64 / (r.frames * readers as u64) as f64);
        }
        let total = median(cpu) - base_us;
        split.insert(
            readers.to_string(),
            json!({
                "us_per_frame_total": total,
                "us_per_frame_per_reader": total / readers as f64,
                "delivered": median(delivered),
            }),
        );
    }
    out.insert("split".into(), Value::Object(split));
    out.insert("baseline_cpu_us_per_frame".into(), json!(base_us));
    out.insert("games".into(), json!(GAMES));
    out.insert("per_reader_parts".into(), parts);
    Value::Object(out)
}

// ---------------------------------------------------------------------------

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = Server::start().await;
        eprintln!("ingest ...");
        let ingest = bench_ingest(&s).await;
        eprintln!("events ...");
        let events = json!({
            "1": bench_events(&s, 1).await,
            "32": bench_events(&s, 32).await,
        });
        eprintln!("fan-out ...");
        let fanout = bench_fanout(&s).await;
        let notes = "In-process, release, SQLite store (WAL, synchronous=normal) in the temp dir. \
            ingest: decode packed record + Live::ingest_frame, 64 games each at 15 Hz (every \
            frame fanned out, one a second recorded as GameFrame); us_per_frame is the inline \
            time of those calls (the dispatcher applying GameFrame runs in the background, not \
            counted); throttled_path is one game back to back. events: append_game_batch, \
            sequential, includes waiting for the read model. fanout: loopback WebSocket readers \
            of /v1/stream (anonymous, all games, exclude=GameFrame), 8 games at 15 Hz; process \
            CPU above a no-reader run, so it includes the readers' own receive cost; top-level \
            numbers are packed readers, json readers under \"json\"; per_reader_parts times \
            run_stream's per-frame reader work without the socket.";
        let out = json!({
            "ingest": ingest,
            "events": events,
            "fanout": fanout,
            "notes": notes,
            "load_average": load_average(),
        });
        print_summary(&out);
        if let Ok(dir) = std::env::var("TELEMETRY_BENCH_OUT") {
            let path = std::path::Path::new(&dir).join("server.json");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
            println!("wrote {}", path.display());
        }
    });
}

fn print_summary(out: &Value) {
    let f = |v: &Value| v.as_f64().unwrap_or(f64::NAN);
    println!(
        "load average (1 min) at the end: {:.1}",
        f(&out["load_average"])
    );
    let i = &out["ingest"];
    println!(
        "ingest (packed -> ingest_frame, {} games at 15 Hz): {:.1} us/frame, {:.0} frames/s, \
         delta {} B, keyframe {} B (sent {}, throttled {})",
        i["games"],
        f(&i["us_per_frame"]),
        f(&i["frames_per_s"]),
        i["frame_bytes"],
        i["keyframe_bytes"],
        i["sent"],
        i["throttled"],
    );
    println!(
        "  throttled path (one game flat out): {:.1} us/frame, {:.0} frames/s",
        f(&i["throttled_path"]["us_per_frame"]),
        f(&i["throttled_path"]["frames_per_s"]),
    );
    let p = &i["parts"];
    println!(
        "  parts: decode key {:.1}, validate {:.1}, clone {:.1}, normalize {:.1}, encode delta {:.1}, \
         encode key {:.1}, json {:.1} us ({} B json)",
        f(&p["decode_keyframe_us"]),
        f(&p["validate_us"]),
        f(&p["clone_us"]),
        f(&p["normalize_us"]),
        f(&p["encode_delta_us"]),
        f(&p["encode_keyframe_us"]),
        f(&p["json_us"]),
        p["json_bytes"],
    );
    for b in ["1", "32"] {
        let e = &out["events"][b];
        println!(
            "events (batch {b}): {:.0} events/s, {:.1} us/batch, {:.1} us/event (open+verify {:.1} us, {} B)",
            f(&e["events_per_s"]),
            f(&e["us_per_batch"]),
            f(&e["us_per_event"]),
            f(&e["open_verify_us"]),
            e["jws_bytes"],
        );
    }
    let fo = &out["fanout"];
    println!(
        "fan-out ({} games at 15 Hz, baseline {:.1} us CPU/frame with no readers):",
        fo["games"],
        f(&fo["baseline_cpu_us_per_frame"]),
    );
    for n in ["1", "10", "100"] {
        let e = &fo[n];
        println!(
            "  {n:>3} readers: packed {:.1} us/frame/reader ({:.1} total, delivered {:.2}); \
             json {:.1} us/frame/reader ({:.1} total, delivered {:.2})",
            f(&e["packed"]["us_per_frame_per_reader"]),
            f(&e["packed"]["us_per_frame_total"]),
            f(&e["packed"]["delivered"]),
            f(&e["json"]["us_per_frame_per_reader"]),
            f(&e["json"]["us_per_frame_total"]),
            f(&e["json"]["delivered"]),
        );
    }
    for n in ["8", "80"] {
        let e = &fo["split"][n];
        println!(
            "  {n:>3} readers, one game each: packed {:.1} us/frame/reader ({:.1} total, delivered {:.2})",
            f(&e["us_per_frame_per_reader"]),
            f(&e["us_per_frame_total"]),
            f(&e["delivered"]),
        );
    }
    let pr = &fo["per_reader_parts"];
    println!(
        "  per-reader work alone: filter {:.2} us, packed add {:.2} us, json clone+serialise {:.1} us",
        f(&pr["filter_us"]),
        f(&pr["packed_add_us"]),
        f(&pr["json_us"]),
    );
}
