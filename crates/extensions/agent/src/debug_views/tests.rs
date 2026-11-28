use super::*;
use crate::ConnId;
use guest_layout::testing::FakeGuest;
use serde_json::{Value, json};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::{Duration, Instant};

const EDGE: Edge = [[-1.0, 3.0, 0.0], [1.0, 3.0, 0.0]];
const CHECKSUM: u32 = 41;
const CACHE_CHECKSUM: u32 = guest_layout::global::CACHE_MAP_NAME.base - 0x198 + 0x2d0;
const GLOBALS: u32 = 0x12000;

#[derive(Default)]
struct Maps(AtomicUsize);
impl DebugMaps for Maps {
    fn generation(&self) -> u64 {
        0
    }
    fn bsp(&self, name: &str, checksum: Option<u32>, index: usize) -> Result<DebugBsp, String> {
        assert_eq!((name, checksum, index), ("synthetic", Some(CHECKSUM), 0));
        self.0.fetch_add(1, Ordering::Relaxed);
        let mut bsp = DebugBsp::default();
        bsp.geometry_edges.push(EDGE);
        Ok(bsp)
    }
    fn hitboxes(&self, _: &str, _: Option<u32>) -> Result<DebugHitboxes, String> {
        panic!("hitboxes are off")
    }
}

struct Game(FakeGuest);

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(22),
            map: Some("synthetic".into()),
            bsp: Some(0),
            ..Default::default()
        }
    }
    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(vec![])
    }
    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
        f(&mut self.0);
        true
    }
}

fn rpc(
    agent: &Agent,
    conn: ConnId,
    replies: &Receiver<String>,
    game: &mut Game,
    method: &str,
    params: Value,
) -> Value {
    agent.submit(
        conn,
        &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
    );
    agent.on_tick(game);
    serde_json::from_str(&replies.try_recv().unwrap()).unwrap()
}

#[test]
fn rpc_toggles_reach_new_images_and_leave_held_images_immutable() {
    let mut game = Game(FakeGuest::new());
    game.0
        .put_u32(guest_layout::global::GAME_GLOBALS.base, GLOBALS)
        .put_u16(GLOBALS + GameGlobals::ACTIVE_BSP.offset, 0)
        .put_u32(CACHE_CHECKSUM, CHECKSUM);
    let maps = Arc::new(Maps::default());
    let views = Arc::new(DebugViews::default());
    let agent = Agent::new();
    install(&agent, Some(views.clone()), Some(maps.clone()), None);
    let (tx, replies) = sync_channel(16);
    let conn = agent.connect(tx);
    let methods = rpc(
        &agent,
        conn,
        &replies,
        &mut game,
        agent_proto::method::METHODS,
        Value::Null,
    );
    for method in [method::VIEWS, method::VIEW] {
        assert!(
            methods["result"]
                .as_array()
                .unwrap()
                .contains(&json!(method))
        );
    }
    assert!(views.capture(&game.0).is_none());
    let set = rpc(
        &agent,
        conn,
        &replies,
        &mut game,
        method::VIEW,
        json!({"name":"geometry_edges","on":true}),
    );
    assert_eq!(set["result"]["views"][0]["on"], true, "{set}");
    // The loader owns another thread. Run the normal observer until it has
    // published the result; give a failing load a bounded wait.
    let deadline = Instant::now() + Duration::from_secs(5);
    let held = loop {
        agent.on_tick(&mut game);
        if let Some(image) = views.capture(&game.0) {
            break image;
        }
        assert!(
            Instant::now() < deadline,
            "synthetic BSP never reached the image"
        );
        std::thread::yield_now();
    };
    // A render owner on another BSP may still receive the previous tick's cache.
    game.0.put_u16(GLOBALS + GameGlobals::ACTIVE_BSP.offset, 1);
    assert!(views.capture(&game.0).is_none());
    game.0.put_u16(GLOBALS + GameGlobals::ACTIVE_BSP.offset, 0);
    game.0.put_u32(CACHE_CHECKSUM, CHECKSUM.wrapping_add(1));
    assert!(views.capture(&game.0).is_none());
    game.0.put_u32(CACHE_CHECKSUM, CHECKSUM);
    let list = rpc(&agent, conn, &replies, &mut game, method::VIEWS, json!({}));
    assert_eq!(list["result"]["views"][0]["on"], true);
    let off = rpc(
        &agent,
        conn,
        &replies,
        &mut game,
        method::VIEW,
        json!({"name":"geometry_edges","on":false}),
    );
    assert_eq!(off["result"]["views"][0]["on"], false);
    assert!(!views.active());
    assert!(views.capture(&game.0).is_none());
    let mut lines = Vec::new();
    held.lines(&mut |edges, color, dashed| {
        assert_eq!(color, View::GeometryEdges.color());
        assert!(!dashed);
        lines.extend_from_slice(edges);
    });
    assert_eq!(lines, [EDGE]);
    assert_eq!(maps.0.load(Ordering::Relaxed), 1);
}

#[test]
fn refusal_leaves_all_views_off() {
    let agent = Agent::new();
    let mut game = Game(FakeGuest::new());
    let (tx, replies) = sync_channel(16);
    let conn = agent.connect(tx);
    let why = "no presenter in this mode";
    install(&agent, None, None, Some(why));
    let response = rpc(
        &agent,
        conn,
        &replies,
        &mut game,
        method::VIEW,
        json!({"name":"geometry_edges","on":true}),
    );
    assert_eq!(response["error"]["code"], code::UNAVAILABLE);
    assert_eq!(response["error"]["message"], why);
    let list = rpc(&agent, conn, &replies, &mut game, method::VIEWS, json!({}));
    assert!(
        list["result"]["views"]
            .as_array()
            .unwrap()
            .iter()
            .all(|view| view["on"] == false && view["why"] == why)
    );
}

#[test]
fn range_and_cap_share_one_layer_between_disjoint_seats() {
    let west = [[-100.0, 0.0, 0.0], [-99.0, 0.0, 0.0]];
    let east = [[100.0, 0.0, 0.0], [101.0, 0.0, 0.0]];
    let eyes = [west[0], east[0]];
    let mut edges = vec![EDGE; 100]; // Too far from both users.
    edges.extend(std::iter::repeat_n(west, 100));
    edges.extend(std::iter::repeat_n(east, 2));
    assert_eq!(select(&edges, &eyes, 5.0, 4), [west, east, west, east]);
    assert_eq!(
        select(&edges, &eyes, 5.0, 5),
        [west, east, west, east, west]
    );
    // A crossing segment is in range even when its endpoints are distant.
    assert_eq!(
        nearest([[-500.0, 0.0, 0.0], [500.0, 0.0, 0.0]], &eyes, 5.0),
        Some(0)
    );
    assert_eq!(select(&edges, &[], 5.0, 4), [EDGE; 4]);
}

#[test]
fn reads_only_mapped_split_seats_even_when_a_listener_stays_active() {
    use guest_layout::global::PLAYER_GLOBALS;
    let mut guest = FakeGuest::new();
    let players = 0x20000;
    let sound = 0x30000;
    guest
        .put_u32(PLAYER_GLOBALS.base, players)
        .put_u32(guest_layout::points::SOUND_GLOBALS.base, sound);
    for (seat, datum) in [u32::MAX, 0xa000_0001, u32::MAX, 0xb000_0002]
        .into_iter()
        .enumerate()
    {
        guest.put_u32(
            players + PlayerGlobals::LOCAL_PLAYERS.offset + 4 * seat as u32,
            datum,
        );
        let listener = SoundListener::for_user(sound, seat).unwrap();
        guest
            .put_u8(listener.addr + SoundListener::ACTIVE.offset, 1)
            .put_f32(listener.addr + SoundListener::POSITION.offset, seat as f32);
    }
    assert_eq!(seat_positions(&guest), [[1.0, 0.0, 0.0], [3.0, 0.0, 0.0]]);
}

#[test]
fn recent_hits_draw_the_requested_ray_and_hit_cross_while_misses_dash() {
    let hit = CollisionTrace {
        tick: 20,
        start: [0.; 3],
        end: [7., 0., 0.],
        hit: Some([3., 0., 0.]),
    };
    let mut image = DebugImage::default();
    image.add_traces(
        &[
            CollisionTrace { tick: 15, ..hit },
            hit,
            CollisionTrace { hit: None, ..hit },
        ],
        25,
        10,
        &[[0.; 3]],
        1.,
    );
    let mut batches = Vec::new();
    image.lines(&mut |lines, color, dashed| {
        assert_eq!(color, View::Traces.color().map(|v| v * 0.5));
        batches.push((lines.to_vec(), dashed));
    });
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].0.len(), 4);
    assert!(!batches[0].1);
    assert_eq!(batches[0].0[0], [hit.start, hit.end]);
    for (i, edge) in batches[0].0[1..].iter().enumerate() {
        let middle = hit.hit.unwrap()[i];
        assert!(edge[0][i] < middle && middle < edge[1][i]);
    }
    assert_eq!(batches[1], (vec![[hit.start, hit.end]], true));
}
