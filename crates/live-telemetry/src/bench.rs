//! Guest-thread cost of telemetry (docs/live/live-telemetry.md,
//! "Performance"): synthetic scenes in fixture memory driven tick by tick
//! through the same entry points the hooks call, timed and allocation
//! counted on the calling (game) thread. Also the sink worker's packed
//! encode and the wire size of the frames.
//!
//! `scripts/telemetry-bench.sh` runs this with the server and web benches;
//! alone: `cargo test --release -p live-telemetry --lib bench -- --ignored
//! --nocapture --test-threads=1`. With `TELEMETRY_BENCH_OUT=<dir>` the
//! numbers also go to `<dir>/guest.json`.
//!
//! The golden test (not ignored) runs each scene for a few ticks and
//! compares a hash of every event and frame the session produced, so a
//! speed-up that changes the output fails. Another (not ignored) fails if
//! the game thread's tick allocates once steady:
//! `cargo test -p live-telemetry --lib bench::the_game_threads_tick_does_not_allocate`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use live_proto::game::Frame;
use live_proto::pack;
use runtime::Guest;

use crate::event::{HitRecord, Output};
use crate::hooks::Telemetry;
use crate::layout;
use crate::memory::fixture::{self, body, object_datum};
use crate::session::Session;
use crate::sink::Sink;
use crate::tags::TagNames;

// Allocations made by this thread (the guest thread in the bench), so the
// sink worker's are not counted.
thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static ALLOC_BYTES: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

fn count(bytes: usize) {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
    let _ = ALLOC_BYTES.try_with(|c| c.set(c.get() + bytes as u64));
}

// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> (u64, u64) {
    (ALLOCS.with(Cell::get), ALLOC_BYTES.with(Cell::get))
}

/// The sink of the bench: drops everything (the server sink's own cost is
/// the encode stage, measured apart).
struct NullSink;

impl Sink for NullSink {
    fn event(&mut self, _: crate::event::GameEvent) -> Result<(), String> {
        Ok(())
    }
    fn frame(&mut self, _: Frame) -> Result<(), String> {
        Ok(())
    }
}

/// The sink of the golden output: keeps everything the worker sends.
struct Capture(std::sync::Arc<std::sync::Mutex<Vec<Output>>>);

impl Sink for Capture {
    fn event(&mut self, event: crate::event::GameEvent) -> Result<(), String> {
        self.0.lock().unwrap().push(Output::Event(event));
        Ok(())
    }
    fn frame(&mut self, frame: Frame) -> Result<(), String> {
        self.0.lock().unwrap().push(Output::Frame(frame));
        Ok(())
    }
}

const BIPED: u32 = 0xe001_0010;
const WEAPON: u32 = 0xe002_0020;
const VEHICLE: u32 = 0xe003_0030;
const TURRET: u32 = 0xe004_0040;
const MACHINE: u32 = 0xe005_0050;
const ELITE: u32 = 0xe006_0060;
/// Scenery's object type (the type table's order, `memory::object_kind`).
const SCENERY: u8 = 6;

fn tag_names() -> TagNames {
    TagNames::new(
        0,
        [
            (
                BIPED,
                *b"bipd",
                "objects\\characters\\masterchief\\masterchief_mp".to_string(),
            ),
            (
                ELITE,
                *b"bipd",
                "objects\\characters\\elite\\elite".to_string(),
            ),
            (
                WEAPON,
                *b"weap",
                "objects\\weapons\\rifle\\battle_rifle\\battle_rifle".to_string(),
            ),
            (
                VEHICLE,
                *b"vehi",
                "objects\\vehicles\\warthog\\warthog".to_string(),
            ),
            (
                TURRET,
                *b"vehi",
                "objects\\vehicles\\warthog\\turrets\\chaingun\\chaingun".to_string(),
            ),
            (
                MACHINE,
                *b"mach",
                "scenarios\\objects\\machines\\lift\\lift".to_string(),
            ),
        ],
    )
}

/// What a scene puts in the world.
#[derive(Clone, Copy)]
pub(crate) struct Scene {
    pub name: &'static str,
    pub campaign: bool,
    pub players: u32,
    /// Players riding vehicles (the first of them a turret's gunner).
    pub riders: u32,
    pub actors: u32,
    /// Actors riding vehicles.
    pub actor_riders: u32,
    pub bodies: u32,
    pub vehicles: u32,
    /// Vehicles with a turret (each turret is one more vehicle object).
    pub turrets: u32,
    pub machines: u32,
    /// Scenery from scenario placements (one in twenty moved off it).
    pub scenery: u32,
    /// Weapons from scenario placements (one in four picked up: gone).
    pub placed_weapons: u32,
    pub hits_per_tick: u32,
    pub shots_per_tick: u32,
}

pub(crate) const SCENES: [Scene; 3] = [
    // A 16-player slayer: ~300 objects.
    Scene {
        name: "mp16",
        campaign: false,
        players: 16,
        riders: 5,
        actors: 0,
        actor_riders: 0,
        bodies: 4,
        vehicles: 12,
        turrets: 4,
        machines: 10,
        scenery: 200,
        placed_weapons: 24,
        hits_per_tick: 3,
        shots_per_tick: 8,
    },
    // The same with ~1000 objects.
    Scene {
        name: "mp16_1000",
        campaign: false,
        players: 16,
        riders: 5,
        actors: 0,
        actor_riders: 0,
        bodies: 8,
        vehicles: 24,
        turrets: 8,
        machines: 20,
        scenery: 840,
        placed_weapons: 48,
        hits_per_tick: 3,
        shots_per_tick: 8,
    },
    // Campaign: one player, a hundred actors, ~1000 objects.
    Scene {
        name: "campaign100",
        campaign: true,
        players: 1,
        riders: 0,
        actors: 100,
        actor_riders: 6,
        bodies: 30,
        vehicles: 16,
        turrets: 6,
        machines: 30,
        scenery: 600,
        placed_weapons: 60,
        hits_per_tick: 4,
        shots_per_tick: 12,
    },
];

/// A scene in guest memory, and the slots that move each tick.
pub(crate) struct World {
    pub guest: Guest,
    units: Vec<u32>,
    actors: Vec<u32>,
    vehicles: Vec<u32>,
    turrets: Vec<u32>,
    machines: Vec<u32>,
    weapons: Vec<u32>,
}

/// A point on the unit circle that goes round as `a` grows (one turn every
/// four units), from + − × ÷ alone. Not `sin`/`cos`: the optimiser may
/// fuse a sin and a cos into one `sincos`, whose last bit can differ from
/// the two calls', and the scenes must be the same in every build profile
/// for the golden test.
fn circle(a: f32) -> (f32, f32) {
    // tan of a quarter of the angle, -1..1, then the rational parametrisation.
    let u = (a % 4.0) / 2.0 - 1.0;
    let d = 1.0 + u * u;
    ((1.0 - u * u) / d, 2.0 * u / d)
}

fn set_vec(guest: &mut Guest, at: u32, v: [f32; 3]) {
    for (i, c) in v.into_iter().enumerate() {
        guest.set_f32(at + 4 * i as u32, c).unwrap();
    }
}

impl World {
    pub fn new(scene: &Scene) -> Self {
        let mut guest = fixture::large_memory();
        let options = fixture::GLOBALS + layout::OPTIONS;
        guest
            .set_u32(
                options + layout::OPTIONS_TYPE,
                if scene.campaign { 1 } else { 2 },
            )
            .unwrap();
        guest
            .set_u16(options + layout::OPTIONS_TICK_RATE, 30)
            .unwrap();
        if scene.campaign {
            guest
                .set_u32(fixture::ENGINE + layout::ENGINE_STATBORG, 0)
                .unwrap();
        }
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 1).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        let mut next = 0;
        let mut slot = || {
            next += 1;
            next - 1
        };
        let mut world = World {
            guest,
            units: Vec::new(),
            actors: Vec::new(),
            vehicles: Vec::new(),
            turrets: Vec::new(),
            machines: Vec::new(),
            weapons: Vec::new(),
        };
        let g = &mut world.guest;
        // Vehicles first, so riders can name them.
        for v in 0..scene.vehicles {
            let s = slot();
            fixture::put_vehicle(g, s);
            g.set_u32(body(s) + layout::BODY_DEFINITION, VEHICLE)
                .unwrap();
            set_vec(g, body(s) + layout::BODY_FORWARD, [1.0, 0.0, 0.0]);
            set_vec(
                g,
                body(s) + layout::BODY_POSITION,
                [v as f32 * 8.0, 0.0, 1.0],
            );
            world.vehicles.push(s);
            if v < scene.turrets {
                let t = slot();
                fixture::put_vehicle(g, t);
                g.set_u32(body(t) + layout::BODY_DEFINITION, TURRET)
                    .unwrap();
                g.set_u32(body(t) + layout::BODY_PARENT, object_datum(s))
                    .unwrap();
                set_vec(g, body(t) + layout::BODY_POSITION, [-0.5, 0.0, 0.8]);
                set_vec(g, body(t) + layout::BODY_FORWARD, [1.0, 0.0, 0.0]);
                set_vec(g, body(t) + layout::UNIT_AIM, [1.0, 0.0, 0.0]);
                world.turrets.push(t);
            }
        }
        let ride = |n: u32, turret_first: bool, world: &World| -> u32 {
            if turret_first && n == 0 && !world.turrets.is_empty() {
                object_datum(world.turrets[0])
            } else {
                object_datum(world.vehicles[n as usize % world.vehicles.len()])
            }
        };
        for p in 0..scene.players {
            let unit = slot();
            fixture::put_object(&mut world.guest, unit, u32::MAX);
            fixture::put_unit(&mut world.guest, unit, BIPED, [1.0, 0.0, 0.0], 0);
            let g = &mut world.guest;
            g.set_u32(body(unit) + layout::UNIT_PLAYER, 0x8001_0000 + p)
                .unwrap();
            g.set_u16(body(unit) + layout::UNIT_TEAM, (p % 2) as u16)
                .unwrap();
            set_vec(
                g,
                body(unit) + layout::BODY_POSITION,
                [p as f32 * 3.0, 5.0, 0.0],
            );
            for i in 0..2 {
                let w = slot();
                fixture::put_weapon(g, w, unit, i, WEAPON);
                world.weapons.push(w);
            }
            if p < scene.riders {
                let parent = ride(p, true, &world);
                world
                    .guest
                    .set_u32(body(unit) + layout::BODY_PARENT, parent)
                    .unwrap();
            }
            fixture::put_player(
                &mut world.guest,
                p,
                &format!("Player {p}"),
                (p % 2) as i8,
                p == 0,
                object_datum(unit),
            );
            world.units.push(unit);
        }
        for a in 0..scene.actors {
            let s = slot();
            let at = [(a % 10) as f32 * 4.0, (a / 10) as f32 * 4.0, 0.0];
            fixture::put_actor(&mut world.guest, s, (a % 3) as i16, at);
            fixture::put_unit(&mut world.guest, s, ELITE, [0.0, 1.0, 0.0], 0);
            let w = slot();
            fixture::put_weapon(&mut world.guest, w, s, 0, WEAPON);
            world.weapons.push(w);
            if a < scene.actor_riders {
                let parent = ride(a, false, &world);
                world
                    .guest
                    .set_u32(body(s) + layout::BODY_PARENT, parent)
                    .unwrap();
            }
            world.actors.push(s);
        }
        for b in 0..scene.bodies {
            let s = slot();
            fixture::put_actor(&mut world.guest, s, 2, [b as f32, -10.0, 0.0]);
            fixture::put_unit(&mut world.guest, s, ELITE, [0.0, 1.0, 0.0], -1);
            world
                .guest
                .set_u16(body(s) + layout::BODY_DAMAGE_FLAGS, layout::BODY_DEPLETED)
                .unwrap();
        }
        for m in 0..scene.machines {
            let s = slot();
            fixture::put_machine(&mut world.guest, s, m as i16, 0.0);
            world
                .guest
                .set_u32(body(s) + layout::BODY_DEFINITION, MACHINE)
                .unwrap();
            world.machines.push(s);
        }
        // Placements: scenery and weapons, records at the scenario.
        let mut records = fixture::SCENARIO + 0x1000;
        for (object_type, count, every) in [
            (SCENERY, scene.scenery, 20),
            (layout::OBJECT_TYPE_WEAPON, scene.placed_weapons, 4),
        ] {
            let &(_, offset, stride) = layout::PLACEMENT_BLOCKS
                .iter()
                .find(|(t, _, _)| *t == object_type)
                .unwrap();
            world
                .guest
                .set_u32(fixture::SCENARIO + offset, count)
                .unwrap();
            world
                .guest
                .set_u32(fixture::SCENARIO + offset + 4, records)
                .unwrap();
            for i in 0..count {
                let pos = [i as f32 * 2.0, 40.0, 0.5];
                let record = records + i * stride;
                set_vec(&mut world.guest, record + layout::PLACEMENT_POSITION, pos);
                // A placed weapon picked up is gone from its spawn.
                if object_type == layout::OBJECT_TYPE_WEAPON && i % every == 0 {
                    continue;
                }
                let s = slot();
                let moved = object_type == SCENERY && i % every == 0;
                let at = if moved {
                    [pos[0], pos[1] + 3.0, pos[2]]
                } else {
                    pos
                };
                fixture::put_placed(&mut world.guest, s, object_type, i as i16, -1, at, u32::MAX);
                set_vec(
                    &mut world.guest,
                    body(s) + layout::BODY_FORWARD,
                    [1.0, 0.0, 0.0],
                );
            }
            records += count * stride;
        }
        world
            .guest
            .set_u32(layout::SCENARIO_PTR, fixture::SCENARIO)
            .unwrap();
        world
            .guest
            .set_u16(fixture::GLOBALS + layout::GLOBALS_ACTIVE_BSP, 0)
            .unwrap();
        world.guest.prepare_fast_pages();
        world
    }

    pub fn objects(&self) -> u32 {
        self.guest
            .u32(fixture::OBJECTS + layout::DATUM_HIGH_WATER)
            .unwrap()
    }

    /// Moves everything that moves to where it is at `tick`.
    pub fn advance(&mut self, tick: u32) {
        let t = tick as f32 / 30.0;
        let g = &mut self.guest;
        g.set_u32(fixture::TIME + layout::TIME_TICK, tick).unwrap();
        for (i, s) in self.units.iter().chain(&self.actors).enumerate() {
            let (cos, sin) = circle(t + i as f32);
            let at = body(*s);
            set_vec(
                g,
                at + layout::BODY_POSITION,
                [i as f32 * 3.0 + cos * 4.0, 5.0 + sin * 4.0, 0.0],
            );
            set_vec(g, at + layout::BODY_FORWARD, [-sin, cos, 0.0]);
            set_vec(g, at + layout::UNIT_AIM, [-sin, cos, 0.0]);
            // World units a second (layout::BODY_VELOCITY), about the
            // speed of the circle's four units a second.
            set_vec(g, at + layout::BODY_VELOCITY, [-sin * 4.0, cos * 4.0, 0.0]);
            g.set_f32(at + layout::BODY_VITALITY, 1.0).unwrap();
            g.set_f32(at + layout::BODY_SHIELD_VITALITY, 1.0).unwrap();
        }
        for (i, s) in self.vehicles.iter().enumerate() {
            let at = body(*s);
            set_vec(
                g,
                at + layout::BODY_POSITION,
                [i as f32 * 8.0 + t * 6.0, 0.0, 1.0],
            );
            set_vec(g, at + layout::BODY_VELOCITY, [6.0, 0.0, 0.0]);
        }
        for (i, s) in self.turrets.iter().enumerate() {
            let (cos, sin) = circle(t * 2.0 + i as f32);
            set_vec(g, body(*s) + layout::UNIT_AIM, [cos, sin, 0.0]);
        }
        // Two machines move (lifts); the rest stand.
        for s in self.machines.iter().take(2) {
            g.set_f32(body(*s) + layout::DEVICE_POSITION, (t * 0.1).fract())
                .unwrap();
        }
    }

    /// The hooks between two ticks: weapons firing and damage reports.
    pub fn hooks(&self, telemetry: &Telemetry, scene: &Scene, tick: u32) {
        for n in 0..scene.shots_per_tick {
            let w = self.weapons[((tick * 7 + n) as usize) % self.weapons.len()];
            telemetry.fired(object_datum(w));
        }
        for n in 0..scene.hits_per_tick {
            let victims = if self.actors.is_empty() {
                &self.units
            } else {
                &self.actors
            };
            let victim = victims[((tick + n) as usize) % victims.len()];
            let attacker = self.units[(n as usize) % self.units.len()];
            telemetry.damaged(HitRecord {
                code: 7,
                effect: 0,
                owner_player: if self.actors.is_empty() {
                    0x8001_0000 + n % scene.players
                } else {
                    u32::MAX
                },
                owner_object: object_datum(attacker),
                victim: object_datum(victim),
                pos: Some([1.0, 2.0, 3.0]),
                body: 0.1,
                shield: 0.05,
                tick: None,
            });
        }
    }
}

fn start(world: &World, sink: impl Sink) -> Telemetry {
    let telemetry = Telemetry::new(Session::new("bench", None), sink);
    telemetry.start(&world.guest).unwrap();
    telemetry.configure(|s| s.set_tag_names(tag_names()));
    telemetry.pipeline.settle();
    telemetry
}

/// Every output of `ticks` ticks of `scene` (after its start), the game id
/// replaced by a fixed one. The worker finishes each tick before the next is read, so
/// none is folded.
pub(crate) fn outputs(scene: &Scene, ticks: u32) -> (Vec<Output>, String) {
    let mut world = World::new(scene);
    let out = std::sync::Arc::default();
    let telemetry = start(&world, Capture(std::sync::Arc::clone(&out)));
    // The ticks' output only (the golden predates the start's).
    out.lock().unwrap().clear();
    for tick in 2..2 + ticks {
        world.hooks(&telemetry, scene, tick);
        world.advance(tick);
        assert!(telemetry.observe_tick(&world.guest).unwrap());
        telemetry.pipeline.settle();
    }
    assert_eq!(telemetry.pipeline.folded_ticks(), 0);
    let game_id = telemetry.pipeline.session().game_id().unwrap().to_owned();
    drop(telemetry);
    let out = std::mem::take(&mut *out.lock().unwrap());
    (out, game_id)
}

fn output_json(outputs: &[Output], game_id: &str) -> String {
    let mut text = String::new();
    for o in outputs {
        let line = match o {
            Output::Event(e) => {
                serde_json::json!({"kind": e.kind, "game_id": e.game_id, "data": e.data})
            }
            Output::Frame(f) => serde_json::json!({"frame": f}),
        };
        text.push_str(&line.to_string());
        text.push('\n');
    }
    text.replace(game_id, "GAME")
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// Each scene's output for its first 12 ticks, as hashed by [`fnv`] over
/// [`output_json`]: what a change to the guest-side path must keep.
const GOLDEN: [(&str, u64); 3] = [
    ("mp16", 0x5f87e00e72613693),
    ("mp16_1000", 0xf840a9fe4aec3427),
    ("campaign100", 0xd8a08e706180ce67),
];

#[test]
fn scenes_produce_the_golden_output() {
    let update = std::env::var_os("TELEMETRY_GOLDEN_UPDATE").is_some();
    for (scene, (name, golden)) in SCENES.iter().zip(GOLDEN) {
        assert_eq!(scene.name, name);
        let (outputs, game_id) = outputs(scene, 12);
        let frames = outputs
            .iter()
            .filter(|o| matches!(o, Output::Frame(_)))
            .count();
        assert_eq!(frames, 6, "{name}: a frame every second tick");
        let text = output_json(&outputs, &game_id);
        if update {
            let path = std::env::temp_dir().join(format!("telemetry-golden-{name}.jsonl"));
            std::fs::write(&path, &text).unwrap();
            println!("(\"{name}\", {:#018x}), // {}", fnv(&text), path.display());
        } else {
            assert_eq!(
                fnv(&text),
                golden,
                "{name}: output changed (TELEMETRY_GOLDEN_UPDATE=1 writes it to $TMPDIR)"
            );
        }
    }
}

/// The scenes' frames carry what capture fills (docs/live/live-telemetry.md,
/// "Velocity, seat, vitality and fire"), so the wire numbers count it.
#[test]
fn scenes_fill_velocity_and_vitality() {
    for scene in &SCENES {
        let (outputs, _) = outputs(scene, 4);
        let frames: Vec<&Frame> = outputs
            .iter()
            .filter_map(|o| match o {
                Output::Frame(f) => Some(f),
                Output::Event(_) => None,
            })
            .collect();
        assert!(!frames.is_empty(), "{}", scene.name);
        for f in frames {
            for p in &f.players {
                assert!(p.vel.is_some(), "{}: player {} vel", scene.name, p.index);
                assert_eq!(p.body, Some(1.0), "{}", scene.name);
                assert_eq!(p.shield, Some(1.0), "{}", scene.name);
                assert_eq!(p.seat.is_some(), p.vehicle.is_some(), "{}", scene.name);
            }
        }
    }
}

/// The game thread's half of a tick (the weapon-fire and damage hooks'
/// entry points, the tick's guest reads and the queue push) allocates
/// nothing once its lists have grown, in every scene, frame ticks and the
/// ticks between alike. The worker's allocations are its own thread's and
/// are not counted (the counter is per thread).
#[test]
fn the_game_threads_tick_does_not_allocate() {
    // Long enough for every recycled list to have grown.
    const WARM: u32 = 40;
    const COUNTED: u32 = 60;
    for scene in &SCENES {
        let mut world = World::new(scene);
        let telemetry = start(&world, NullSink);
        let mut allocating = Vec::new();
        for tick in 2..2 + WARM + COUNTED {
            world.advance(tick);
            let (a0, b0) = allocs();
            world.hooks(&telemetry, scene, tick);
            let observed = telemetry.observe_tick(&world.guest);
            let (a1, b1) = allocs();
            assert_eq!(observed, Ok(true), "{}: tick {tick}", scene.name);
            telemetry.pipeline.settle();
            if tick >= 2 + WARM && a1 > a0 {
                allocating.push((tick, a1 - a0, b1 - b0));
            }
        }
        // An unchanged tick reads only the clock.
        let (a0, _) = allocs();
        assert_eq!(telemetry.observe_tick(&world.guest), Ok(false));
        assert_eq!(
            allocs().0,
            a0,
            "{}: an unchanged tick allocated",
            scene.name
        );
        assert!(
            allocating.is_empty(),
            "{}: ticks that allocated on the game thread (tick, allocations, bytes): {allocating:?}",
            scene.name
        );
        assert_eq!(telemetry.pipeline.folded_ticks(), 0);
        assert!(!telemetry.disabled());
    }
}

#[derive(serde::Serialize, Default)]
struct Stats {
    n: usize,
    mean_us: f64,
    p50_us: f64,
    p99_us: f64,
    max_us: f64,
}

fn stats(mut ns: Vec<u64>) -> Stats {
    if ns.is_empty() {
        return Stats::default();
    }
    ns.sort_unstable();
    let us = |v: u64| v as f64 / 1000.0;
    let at = |q: f64| us(ns[((ns.len() - 1) as f64 * q).round() as usize]);
    Stats {
        n: ns.len(),
        mean_us: us(ns.iter().sum::<u64>()) / ns.len() as f64,
        p50_us: at(0.5),
        p99_us: at(0.99),
        max_us: us(*ns.last().unwrap()),
    }
}

fn time<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_nanos() as u64)
}

const WARMUP: u32 = 120;
/// A minute of 30 Hz ticks.
const TICKS: u32 = 1800;

fn bench_scene(scene: &Scene) -> serde_json::Value {
    let mut world = World::new(scene);
    let telemetry = start(&world, NullSink);
    let mut frame_ticks = (Vec::new(), Vec::new(), Vec::new(), 0u64, 0u64);
    let mut other_ticks = (Vec::new(), Vec::new(), Vec::new(), 0u64, 0u64);
    for tick in 2..2 + WARMUP + TICKS {
        world.advance(tick);
        let (a0, b0) = allocs();
        let (_, hooks) = time(|| world.hooks(&telemetry, scene, tick));
        let w0 = telemetry.pipeline.work_ns();
        let (observed, observe) = time(|| telemetry.observe_tick(&world.guest).unwrap());
        assert!(observed);
        let (a1, b1) = allocs();
        // Real ticks are 33 ms apart: the worker is done long before the
        // next. Its time is its own thread's, not the game thread's.
        telemetry.pipeline.settle();
        let worker = telemetry.pipeline.work_ns() - w0;
        if tick < 2 + WARMUP {
            continue;
        }
        let bucket = if tick.is_multiple_of(2) {
            &mut frame_ticks
        } else {
            &mut other_ticks
        };
        bucket.0.push(hooks + observe);
        bucket.1.push(observe);
        bucket.2.push(worker);
        bucket.3 += a1 - a0;
        bucket.4 += b1 - b0;
    }
    assert!(!telemetry.disabled());
    // The readers alone, on the final state.
    let reads = |f: &dyn Fn()| {
        let ns: Vec<u64> = (0..200).map(|_| time(f).1).collect();
        stats(ns)
    };
    let snapshot = reads(&|| {
        crate::memory::snapshot(&world.guest).unwrap();
    });
    // As the game thread scans: into last time's lists.
    let scan = std::cell::RefCell::new((
        crate::memory::ScanBuffers::default(),
        crate::event::World::default(),
    ));
    let world_scan = reads(&|| {
        let (buffers, out) = &mut *scan.borrow_mut();
        crate::memory::world_into(&world.guest, buffers, out).unwrap();
    });
    let summary = |b: &(Vec<u64>, Vec<u64>, Vec<u64>, u64, u64)| {
        let n = b.0.len() as f64;
        serde_json::json!({
            "total": stats(b.0.clone()),
            "observe": stats(b.1.clone()),
            "worker": stats(b.2.clone()),
            "allocs_per_tick": b.3 as f64 / n,
            "alloc_bytes_per_tick": b.4 as f64 / n,
        })
    };
    let frame = summary(&frame_ticks);
    let other = summary(&other_ticks);
    let per_second_us = |hz: f64| {
        let f = frame["total"]["p50_us"].as_f64().unwrap();
        let o = other["total"]["p50_us"].as_f64().unwrap();
        (f + o) / 2.0 * hz
    };
    let wire = wire(scene);
    serde_json::json!({
        "scene": scene.name,
        "objects": world.objects(),
        "frame_tick": frame,
        "other_tick": other,
        "readers": {"snapshot": snapshot, "world": world_scan},
        "guest_us_per_second_30hz": per_second_us(30.0),
        "guest_us_per_second_60hz": per_second_us(60.0),
        "folded_ticks": telemetry.pipeline.folded_ticks(),
        "wire": wire,
    })
}

/// The sink worker's half: frames packed as the server sink packs them,
/// timed, and their sizes against JSON.
fn wire(scene: &Scene) -> serde_json::Value {
    let (outputs, _) = outputs(scene, TICKS);
    let frames: Vec<Frame> = outputs
        .into_iter()
        .filter_map(|o| match o {
            Output::Frame(f) => Some(f),
            Output::Event(_) => None,
        })
        .collect();
    let mut encoder = pack::Encoder::new(0, crate::server::FRAME_KEYFRAME_EVERY);
    let (mut packed, mut keyframes, mut key_bytes, mut json) = (0usize, 0usize, 0usize, 0usize);
    let mut ns = Vec::new();
    for frame in &frames {
        let ((record, key), t) = time(|| encoder.encode(frame).unwrap());
        ns.push(t);
        // The ingest container's length prefix.
        let mut bytes = Vec::new();
        pack::push_record(&mut bytes, &record);
        packed += bytes.len();
        if key {
            keyframes += 1;
            key_bytes += bytes.len();
        }
        json += serde_json::to_vec(frame).unwrap().len();
    }
    let n = frames.len();
    let minute = n as f64 / (TICKS as f64 / 30.0 / 60.0);
    serde_json::json!({
        "frames": n,
        "encode": stats(ns),
        "packed_bytes_per_frame": packed as f64 / n as f64,
        "keyframe_bytes": key_bytes as f64 / keyframes.max(1) as f64,
        "delta_bytes": (packed - key_bytes) as f64 / (n - keyframes).max(1) as f64,
        "packed_kib_per_minute": packed as f64 / n as f64 * minute / 1024.0,
        "json_bytes_per_frame": json as f64 / n as f64,
        "json_kib_per_minute": json as f64 / n as f64 * minute / 1024.0,
    })
}

#[test]
#[ignore = "benchmark: cargo test --release -p live-telemetry --lib bench -- --ignored --nocapture"]
fn bench_guest_tick() {
    // A loaded machine (other builds) skews single runs: keep each scene's
    // run with the lowest median frame tick.
    let runs: usize = std::env::var("TELEMETRY_BENCH_RUNS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(5);
    let p50 = |r: &serde_json::Value| r["frame_tick"]["total"]["p50_us"].as_f64().unwrap();
    let results: Vec<serde_json::Value> = SCENES
        .iter()
        .map(|scene| {
            (0..runs.max(1))
                .map(|_| bench_scene(scene))
                .min_by(|a, b| p50(a).total_cmp(&p50(b)))
                .unwrap()
        })
        .collect();
    for r in &results {
        println!(
            "{:<12} objects {:>4}  frame tick p50 {:>6.1} µs (p99 {:>7.1}) {:>4.0} allocs  other tick p50 {:>5.1} µs {:>3.0} allocs  | worker frame {:>5.1} other {:>4.1} µs | snapshot {:>5.1} µs world {:>6.1} µs | encode {:>5.1} µs  {:>5.0} B/frame packed ({:>6.0} JSON) {:>5.0} KiB/min",
            r["scene"].as_str().unwrap(),
            r["objects"],
            r["frame_tick"]["total"]["p50_us"].as_f64().unwrap(),
            r["frame_tick"]["total"]["p99_us"].as_f64().unwrap(),
            r["frame_tick"]["allocs_per_tick"].as_f64().unwrap(),
            r["other_tick"]["total"]["p50_us"].as_f64().unwrap(),
            r["other_tick"]["allocs_per_tick"].as_f64().unwrap(),
            r["frame_tick"]["worker"]["p50_us"].as_f64().unwrap(),
            r["other_tick"]["worker"]["p50_us"].as_f64().unwrap(),
            r["readers"]["snapshot"]["p50_us"].as_f64().unwrap(),
            r["readers"]["world"]["p50_us"].as_f64().unwrap(),
            r["wire"]["encode"]["p50_us"].as_f64().unwrap(),
            r["wire"]["packed_bytes_per_frame"].as_f64().unwrap(),
            r["wire"]["json_bytes_per_frame"].as_f64().unwrap(),
            r["wire"]["packed_kib_per_minute"].as_f64().unwrap(),
        );
    }
    if let Some(dir) = std::env::var_os("TELEMETRY_BENCH_OUT") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let text = serde_json::to_string_pretty(&serde_json::json!({ "scenes": results })).unwrap();
        std::fs::write(dir.join("guest.json"), text).unwrap();
    }
}
