//! `edit.collision.stage` and `.commit` through the executor, as a client
//! sends them, over a structure BSP in synthetic guest memory and the real
//! edit arena window. The BSP activation is a fake that runs at the next
//! tick. No game.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use agent_proto::hot::encode_base64;
use runtime::CpuAbi;
use runtime::guest::Permissions;
use serde_json::{Value, json};
use tags::collision::{BSP_BLOCKS_SIZE, Bsp, Child, Leaf, Node3d, Plane};

use super::*;
use crate::{GameStatus, Reader};

#[derive(Default)]
struct Level(GameStatus);

impl Reader for Level {
    fn status(&mut self) -> GameStatus {
        self.0.clone()
    }
    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(vec![])
    }
}

const HEADER: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const META: u32 = 0x1000_0000;
const SBSP_AT: u32 = META + 0x1000;
/// The structure's collision element: its eight pairs, then the map's
/// blocks.
const ELEMENT: u32 = META + 0x2000;
const LEAF_CLUSTERS_AT: u32 = META + 0x3000;
/// A point query's stack and record, for `sim_collision`.
const QUERY: u32 = 0x2000_0000;
const SCNR: u32 = 0xe000_0000;
const SBSP: u32 = 0xe111_0001;

/// One node over one plane `x = d`: behind it leaf 0, in front solid.
fn bsp(d: f32) -> Bsp {
    Bsp {
        nodes: vec![Node3d { plane: 0, back: Child::Leaf(0), front: Child::None }],
        planes: vec![Plane([1.0, 0.0, 0.0, d])],
        leaves: vec![Leaf { flags: 0, ref_count: 0, first_ref: 0xffff }],
        ..Bsp::default()
    }
}

/// [`bsp`] with a second plane and leaf: more of each, as an added solid
/// makes.
fn grown(d: f32) -> Bsp {
    let mut b = bsp(d);
    b.planes.push(Plane([0.0, 1.0, 0.0, 5.0]));
    b.leaves.push(Leaf { flags: 0, ref_count: 0, first_ref: 0xffff });
    b
}

fn entry(g: &mut Guest, slot: u32, group: &[u8; 4], datum: u32, addr: u32, size: u32) {
    let e = ENTRIES + slot * 16;
    g.set_u32(e, map::FourCC(*group).as_u32()).unwrap();
    g.set_u32(e + 4, datum).unwrap();
    g.set_u32(e + 8, addr).unwrap();
    g.set_u32(e + 12, size).unwrap();
}

/// A scenario and one structure BSP whose collision is [`bsp`]`(0)`, with
/// `pathfinding` elements of pathfinding data.
fn level(pathfinding: u32) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0020_0000, Permissions::RW).unwrap();
    g.map_zeroed(META, 0x0001_0000, Permissions::RW).unwrap();
    g.map_zeroed(QUERY, 0x3000, Permissions::RW).unwrap();
    g.set_u32(guest_layout::points::TAG_INDEX.base, HEADER).unwrap();
    g.set_u32(guest_layout::points::TAG_ENTRIES.base, ENTRIES).unwrap();
    g.set_u32(HEADER + hot_tags::index::HEADER_SCENARIO, SCNR).unwrap();
    g.set_u32(HEADER + hot_tags::index::HEADER_COUNT, 2).unwrap();
    entry(&mut g, 0, b"scnr", SCNR, META, 0x100);
    entry(&mut g, 1, b"sbsp", SBSP, SBSP_AT, 0x400);
    g.write_bytes(ELEMENT, &bsp(0.0).encode_blocks(ELEMENT), 1).unwrap();
    let pair = |g: &mut Guest, at: u32, count: u32, address: u32| {
        g.set_u32(at, count).unwrap();
        g.set_u32(at + 4, address).unwrap();
    };
    pair(&mut g, SBSP_AT + sbsp::MATERIALS, 1, META + 0x800);
    pair(&mut g, SBSP_AT + sbsp::COLLISION, 1, ELEMENT);
    pair(&mut g, SBSP_AT + sbsp::LEAVES, 1, LEAF_CLUSTERS_AT);
    pair(&mut g, SBSP_AT + sbsp::PATHFINDING, pathfinding, META + 0x900);
    g
}

/// The eight blocks of `b` as `edit.collision.stage` takes them.
fn blocks_of(b: &Bsp) -> Value {
    let enc = b.encode_blocks(0);
    let mut at = BSP_BLOCKS_SIZE;
    let mut out = serde_json::Map::new();
    for (i, name) in COLLISION_BLOCKS.iter().enumerate() {
        let count = u32::from_le_bytes(enc[i * 8..i * 8 + 4].try_into().unwrap());
        let len = count as usize * BSP_STRIDES[i];
        out.insert((*name).into(), json!({"count": count, "bytes": encode_base64(&enc[at..at + len])}));
        at += len;
    }
    Value::Object(out)
}

fn stage_of(b: &Bsp) -> Value {
    json!({
        "target": {"structure": format!("{SBSP:#010x}")},
        "blocks": blocks_of(b),
        "leaves": {"count": b.leaves.len(), "bytes": encode_base64(&vec![0u8; 8 * b.leaves.len()])},
    })
}

/// The pairs a commit names in `was`, as the guest holds them now.
fn pairs(g: &Guest) -> Value {
    let mut out = serde_json::Map::new();
    let mut put = |name: &str, at: u32| {
        out.insert(name.into(), json!({"count": g.u32(at).unwrap(), "address": g.u32(at + 4).unwrap()}));
    };
    for (i, name) in COLLISION_BLOCKS.iter().enumerate() {
        put(name, ELEMENT + 8 * i as u32);
    }
    put(COLLISION_LEAF_CLUSTERS, SBSP_AT + sbsp::LEAVES);
    put(COLLISION_MOPP, SBSP_AT + sbsp::MOPP);
    Value::Object(out)
}

/// An activation that runs at the first poll after it starts.
struct Switch(Arc<AtomicUsize>);

impl Activation for Switch {
    fn start(&mut self, _: &mut Guest) -> Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn poll(&mut self, _: &Guest) -> Option<Result<(), String>> {
        Some(Ok(()))
    }
}

struct Game {
    agent: Agent,
    guest: Arc<Mutex<Guest>>,
    conn: ConnId,
    rx: Receiver<String>,
    level: Level,
    switches: Arc<AtomicUsize>,
}

impl Game {
    fn new(guest: Guest, kind: agent_proto::sim::SessionKind, activation: bool) -> Game {
        let guest = Arc::new(Mutex::new(guest));
        let agent = Agent::new();
        let g = Arc::clone(&guest);
        super::super::add_edit_methods(
            &agent,
            Editor::new().with_arena(arena::shared()),
            Arc::new(move |f: &mut dyn FnMut(&mut Guest)| f(&mut g.lock().unwrap())),
            move |_| SessionView { kind, ..Default::default() },
            |_| None,
            false,
        );
        let switches = Arc::new(AtomicUsize::new(0));
        if activation {
            agent.set_collision_activation(Box::new(Switch(Arc::clone(&switches))));
        }
        let (tx, rx) = sync_channel(64);
        let conn = agent.connect(tx);
        Game { agent, guest, conn, rx, level: Level::default(), switches }
    }

    /// Sends a request and runs ticks until it is answered.
    fn call(&mut self, m: &str, p: Value) -> Value {
        self.agent.submit(
            self.conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
        );
        for _ in 0..4 {
            self.agent.on_tick(&mut self.level);
            if let Ok(line) = self.rx.try_recv() {
                return serde_json::from_str(&line).unwrap();
            }
        }
        serde_json::from_str(&self.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    fn ok(&mut self, m: &str, p: Value) -> Value {
        let r = self.call(m, p);
        assert!(r.get("error").is_none(), "{m}: {r}");
        r["result"].clone()
    }

    fn pairs(&self) -> Value {
        pairs(&self.guest.lock().unwrap())
    }

    fn commit(&mut self, stage: &Value, mode: &str, activate: bool) -> Value {
        let was = self.pairs();
        self.call(
            method::EDIT_COLLISION_COMMIT,
            json!({"stage": stage["stage"], "mode": mode, "activate": activate, "was": was}),
        )
    }

    /// The leaf `point_to_leaf` (`sim_collision`'s native point query)
    /// finds for `(1, 0, 0)` in the structure's collision.
    fn leaf_at_one(&self) -> u32 {
        let mut g = self.guest.lock().unwrap();
        g.set_u32(QUERY, 1.0f32.to_bits()).unwrap();
        let mut c = CpuAbi {
            ecx: ELEMENT,
            edx: QUERY,
            esp: QUERY + 0x1800,
            esi: QUERY + 32,
            mxcsr: 0x1fa0,
            ..CpuAbi::default()
        };
        let ran = engine::sim_collision::point_to_leaf(&mut c, &mut *g, &engine::sim_collision::WORLD).unwrap();
        assert!(ran.is_some(), "the native point query declined");
        c.eax
    }
}

fn addresses(blocks: &Value) -> Vec<u64> {
    let mut out: Vec<u64> = blocks
        .as_object()
        .unwrap()
        .values()
        .filter_map(|b| b["address"].as_u64().filter(|a| *a != 0))
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn a_commit_repoints_every_pair_at_one_boundary_or_none() {
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, false);
    let before = game.pairs();
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    // A stage writes only arena memory the game does not read.
    assert_eq!(game.pairs(), before);
    assert_eq!(staged["activation"], false);
    let planes = staged["blocks"]["planes"]["address"].as_u64().unwrap() as u32;
    assert!(arena::WINDOW.contains(&planes), "{staged}");
    assert_eq!(staged["blocks"]["planes"]["count"], 2);
    assert_eq!(staged["blocks"]["refs"], json!({"count": 0, "address": 0}));

    // One pair the game holds otherwise: nothing is written.
    let mut was = before.clone();
    was["surfaces"]["count"] = json!(5);
    let e = game.call(
        method::EDIT_COLLISION_COMMIT,
        json!({"stage": staged["stage"], "mode": "append", "was": was}),
    );
    assert_eq!(e["error"]["code"], code::MISMATCH, "{e}");
    assert_eq!(e["error"]["data"]["pairs"][0]["name"], "surfaces");
    assert_eq!(game.pairs(), before);

    // A was missing a staged pair is refused before anything is written.
    let mut partial = before.clone();
    partial.as_object_mut().unwrap().remove(COLLISION_LEAF_CLUSTERS);
    let e = game.call(
        method::EDIT_COLLISION_COMMIT,
        json!({"stage": staged["stage"], "mode": "append", "was": partial}),
    );
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");
    assert_eq!(game.pairs(), before);

    let done = game.ok(
        method::EDIT_COLLISION_COMMIT,
        json!({"stage": staged["stage"], "mode": "append", "was": before}),
    );
    assert_eq!(done["activated"], false);
    assert_eq!(done["freed"], json!([]));
    let now = game.pairs();
    for name in COLLISION_BLOCKS.iter().chain([&COLLISION_LEAF_CLUSTERS]) {
        assert_eq!(now[*name], staged["blocks"][*name], "{name}: {now}");
        assert_eq!(done["pairs"][*name], staged["blocks"][*name], "{name}");
    }
    // The MOPP was not staged, so its pair is the map's.
    assert_eq!(now[COLLISION_MOPP], before[COLLISION_MOPP]);
    // The new blocks read back as the staged BSP.
    let g = game.guest.lock().unwrap();
    let plane = g.f32(planes + 16 + 12).unwrap();
    assert_eq!(plane, 5.0);
    drop(g);

    // A commit uses its stage up.
    let e = game.commit(&staged, "append", false);
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");
}

#[test]
fn append_refuses_a_bsp_with_fewer_elements_than_it_replaces() {
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, false);
    game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    let first = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    let done = game.commit(&first, "append", false);
    assert!(done.get("error").is_none(), "{done}");
    let before = game.pairs();
    let smaller = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&bsp(0.0)));
    let e = game.commit(&smaller, "append", false);
    assert_eq!(e["error"]["code"], agent_proto::sim::REFUSED, "{e}");
    assert!(e["error"]["data"]["why"].as_str().unwrap().contains("index"), "{e}");
    assert_eq!(game.pairs(), before);
}

#[test]
fn sim_collision_answers_from_the_committed_blocks() {
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, false);
    // (1, 0, 0) is in front of x = 0: solid, no leaf.
    assert_ne!(game.leaf_at_one(), 0);
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(2.0)));
    // Staged, not committed: the query still reads the map's blocks.
    assert_ne!(game.leaf_at_one(), 0);
    let done = game.commit(&staged, "append", false);
    assert!(done.get("error").is_none(), "{done}");
    // Behind x = 2: leaf 0 of the new blocks, with no other notice than
    // the repointed pairs.
    assert_eq!(game.leaf_at_one(), 0);
}

#[test]
fn freed_extents_return_to_the_arena_after_the_next_activation() {
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, true);
    let first = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    assert_eq!(first["activation"], true);
    let used = game.ok(hot_tags_arena(), Value::Null)["used"].clone();
    // Staging the same target again replaces the uncommitted stage: its
    // extents go back first, so the new one takes the same addresses.
    let again = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    assert_eq!(addresses(&again["blocks"]), addresses(&first["blocks"]));
    assert_eq!(game.ok(hot_tags_arena(), Value::Null)["used"], used);

    // The map's blocks were never in the arena: nothing to free.
    let done = game.commit(&again, "append", true);
    assert!(done.get("error").is_none(), "{done}");
    assert_eq!(done["result"]["activated"], true);
    assert_eq!(done["result"]["freed"], json!([]));
    assert_eq!(game.switches.load(Ordering::SeqCst), 1);

    // A commit without activation keeps the extents it replaced...
    let quiet = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(1.0)));
    let done = game.commit(&quiet, "append", false);
    assert_eq!(done["result"]["freed"], json!([]), "{done}");
    // ...until the next activation frees them, and those of the commit
    // before it.
    let last = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(2.0)));
    let done = game.commit(&last, "append", true);
    assert_eq!(done["result"]["activated"], true, "{done}");
    let mut freed: Vec<u64> = done["result"]["freed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    freed.sort_unstable();
    let mut expected = addresses(&again["blocks"]);
    expected.extend(addresses(&quiet["blocks"]));
    expected.sort_unstable();
    assert_eq!(freed, expected);
    // Only the live blocks hold arena memory now.
    let arena = game.ok(hot_tags_arena(), Value::Null);
    let live = addresses(&last["blocks"]);
    let held: Vec<u64> = arena["moves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["address"].as_u64().unwrap())
        .collect();
    let mut held = held;
    held.sort_unstable();
    assert_eq!(held, live);
}

fn hot_tags_arena() -> &'static str {
    method::EDIT_ARENA
}

#[test]
fn rebuild_needs_an_activation_and_replaced_pathfinding() {
    // Pathfinding names surfaces by index; the stage did not replace it.
    let mut game = Game::new(level(1), agent_proto::sim::SessionKind::Local, true);
    let before = game.pairs();
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    let e = game.commit(&staged, "rebuild", false);
    assert_eq!(e["error"]["code"], agent_proto::sim::REFUSED, "{e}");
    assert!(e["error"]["data"]["why"].as_str().unwrap().contains("pathfinding"), "{e}");
    assert_eq!(game.pairs(), before);
    assert_eq!(game.switches.load(Ordering::SeqCst), 0);
    // Nor can a stage carry pathfinding yet.
    let mut with_paths = stage_of(&grown(0.0));
    with_paths["pathfinding"] = json!({"count": 1, "bytes": ""});
    let e = game.call(method::EDIT_COLLISION_STAGE, with_paths);
    assert_eq!(e["error"]["code"], agent_proto::sim::REFUSED, "{e}");

    // Without pathfinding, rebuild commits and always activates.
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, true);
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&bsp(3.0)));
    let done = game.commit(&staged, "rebuild", false);
    assert_eq!(done["result"]["activated"], true, "{done}");
    assert_eq!(game.switches.load(Ordering::SeqCst), 1);

    // A game with no activation refuses rebuild and append with activate,
    // and takes append alone.
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, false);
    let before = game.pairs();
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    for (mode, activate) in [("rebuild", false), ("append", true)] {
        let e = game.commit(&staged, mode, activate);
        assert_eq!(e["error"]["code"], agent_proto::sim::REFUSED, "{mode}: {e}");
        assert_eq!(game.pairs(), before);
    }
    let done = game.commit(&staged, "append", false);
    assert!(done.get("error").is_none(), "{done}");
}

#[test]
fn co_op_and_clients_refuse_collision_edits() {
    use agent_proto::sim::SessionKind::{CoopGuest, MpClient};
    for kind in [CoopGuest, MpClient] {
        let mut game = Game::new(level(0), kind, true);
        let e = game.call(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
        assert_eq!(e["error"]["code"], code::INVALID_REQUEST, "{kind:?}: {e}");
        assert_eq!(game.ok(hot_tags_arena(), Value::Null)["used"], 0);
    }
}

#[test]
fn a_committed_structure_replaces_the_debug_views_level_collision() {
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, false);
    let mut solid = grown(0.0);
    // A triangle the debug views can draw: three edges around one surface.
    solid.planes.push(Plane([0.0, 0.0, 1.0, 0.0]));
    solid.surfaces.push(tags::collision::Surface {
        plane: 2,
        first_edge: 0,
        flags: 0,
        breakable_surface: 0xff,
        material: 0,
    });
    for (i, p) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].into_iter().enumerate() {
        solid.vertices.push(tags::collision::Vertex { point: p, edge: i as u32 });
        let next = ((i + 1) % 3) as u16;
        solid.edges.push(tags::collision::Edge {
            start_vertex: i as u16,
            end_vertex: next,
            forward_edge: next,
            reverse_edge: next,
            left_surface: 0,
            right_surface: u16::MAX,
        });
    }
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&solid));
    let done = game.commit(&staged, "append", false);
    assert!(done.get("error").is_none(), "{done}");
    let edits = game.agent.collision_edits();
    let (generation, replaced) = edits.snapshot();
    assert!(generation > 0);
    assert_eq!(replaced.get(&SBSP).map(|b| b.edges.len()), Some(3));
}

/// The structure commits the listeners heard.
fn listen(agent: &Agent) -> Arc<Mutex<Vec<StructureCommit>>> {
    let heard: Arc<Mutex<Vec<StructureCommit>>> = Arc::default();
    let h = Arc::clone(&heard);
    agent.on_structure_collision(move |c| h.lock().unwrap().push(c.clone()));
    heard
}

#[test]
fn structure_listeners_hear_each_commit_once_with_its_bsp() {
    // The structure is the active one.
    let mut g = level(0);
    g.set_u32(engine::sim_collision::layout::ACTIVE_STRUCTURE, SBSP_AT).unwrap();
    let mut game = Game::new(g, agent_proto::sim::SessionKind::Local, true);
    let heard = listen(&game.agent);

    // A stage alone tells nobody.
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    assert!(heard.lock().unwrap().is_empty());

    // Without activation: at the commit.
    let done = game.commit(&staged, "append", false);
    assert!(done.get("error").is_none(), "{done}");
    {
        let h = heard.lock().unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].tag, h[0].address, h[0].active), (SBSP, SBSP_AT, true));
        assert_eq!(h[0].bsp.planes.len(), 2);
    }

    // With activation: once, after it ran.
    let mut more = grown(0.0);
    more.planes.push(Plane([0.0, 0.0, 1.0, 1.0]));
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&more));
    let done = game.commit(&staged, "append", true);
    assert_eq!(done["result"]["activated"], true, "{done}");
    assert_eq!(game.switches.load(Ordering::SeqCst), 1);
    let h = heard.lock().unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1].bsp.planes.len(), 3);
}

#[test]
fn a_structure_that_is_not_active_is_heard_as_such() {
    let mut game = Game::new(level(0), agent_proto::sim::SessionKind::Local, false);
    let heard = listen(&game.agent);
    let staged = game.ok(method::EDIT_COLLISION_STAGE, stage_of(&grown(0.0)));
    let done = game.commit(&staged, "append", false);
    assert!(done.get("error").is_none(), "{done}");
    let h = heard.lock().unwrap();
    assert_eq!(h.len(), 1);
    assert!(!h[0].active);
}

#[test]
fn the_queued_activation_is_one_reactivate_call_and_its_outcome() {
    let q = crate::sim::calls::shared();
    let mut a = QueuedActivation::new(Arc::clone(&q));
    let mut g = Guest::new();
    // No feature makes the calls: the start is refused, so the commit
    // puts its pairs back.
    assert!(a.start(&mut g).is_err());
    crate::sim::calls::lock(&q).arm();

    a.start(&mut g).unwrap();
    assert_eq!(a.poll(&g), None);
    let work = crate::sim::calls::lock(&q).drain();
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].1, crate::sim::calls::Call::ReactivateStructure);
    assert_eq!(a.poll(&g), None);
    crate::sim::calls::lock(&q).finish(work[0].0, Ok(json!({"bsp": 0})));
    assert_eq!(a.poll(&g), Some(Ok(())));
    // One outcome per start.
    assert!(matches!(a.poll(&g), Some(Err(_))));

    a.start(&mut g).unwrap();
    let work = crate::sim::calls::lock(&q).drain();
    crate::sim::calls::lock(&q).finish(
        work[0].0,
        Err(RpcError::new(REFUSED, "no structure BSP is loaded and active")),
    );
    assert_eq!(a.poll(&g), Some(Err("no structure BSP is loaded and active".into())));
}
