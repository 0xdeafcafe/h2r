//! The walk table on synthetic state: what it holds, which bodies run
//! whatever the float environment, and its counters.

use super::*;
use runtime::guest::Permissions;
use std::sync::Mutex;

/// The counters are process-wide: one test at a time reads them.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn of(entry: u32) -> Stats {
    stats().into_iter().find(|s| s.entry == entry).expect("a body in the table")
}

#[test]
fn the_table_holds_the_walks_bd090_and_the_multiplayer_type_callbacks_and_nothing_else() {
    assert_eq!(TABLE.bodies().len(), POINTS.len() + TYPES.len());
    for point in POINTS.iter().chain(TYPES) {
        assert!(TABLE.owns(point.base), "{}", point.name);
    }
    assert!(POINTS.iter().all(|p| !TYPES.iter().any(|t| t.base == p.base)), "listed once");
    assert!(TYPES.iter().enumerate().all(|(i, a)| TYPES[i + 1..].iter().all(|b| b.base != a.base)), "listed once");
    assert!(POINTS.iter().any(|p| p.base == points::OBJECTS_FINALIZE.base));
    for point in [
        &points::UNIT_UPDATE,
        &points::BIPED_UPDATE,
        &points::BIPED_POST_UPDATE,
        &points::WEAPON_UPDATE,
        &points::ITEM_UPDATE,
        &points::VEHICLE_UPDATE,
        &points::PROJECTILE_UPDATE,
        &points::SCENERY_UPDATE,
        &points::DEVICE_UPDATE,
        &points::MACHINE_UPDATE,
        &points::CRATE_UPDATE,
        &points::CREATURE_UPDATE,
        &points::CREATURE_POST_UPDATE,
    ] {
        assert!(TYPES.iter().any(|t| t.base == point.base), "{}", point.name);
    }
    // Every body the unit, biped and weapon trees fuse in the full table
    // is in this one: a call between two of them stays Rust. The unit
    // leaves the biped and vehicle trees call are among them.
    for point in [
        &points::UNIT_UPDATE_CONTROL,
        &points::UNIT_UPDATE_STATE,
        &points::UNIT_IDLE_TEST,
        &points::UNIT_MOVEMENT,
        &points::UNIT_CLAMP_DIRECTION,
        &points::UNIT_AIM_UPDATE,
        &points::UNIT_POWERED_SEATS,
        &points::UNIT_CONTROL_REFRESH,
        &points::UNIT_TIMERS,
        &points::UNIT_DIALOGUE_UPDATE,
        &points::UNIT_TEAM_UPDATE,
        &points::UNIT_SEATS_BEGIN,
        &points::UNIT_SEATS_NEXT,
        &points::UNIT_CAMO_UPDATE,
        &points::UNIT_LIGHT_UPDATE,
        &points::UNIT_ANIMATION_STATE,
    ] {
        assert!(TABLE.owns(point.base), "{} is in the unit tree", point.name);
    }
    // The bodies of the other types and the shell stay the guest's.
    for other in [
        &points::MACHINE_SLOT_48,
        &points::OBJECT_DAMAGE_UPDATE,
        &points::OBJECT_FUNCTION_VALUE,
        &points::UNIT_FUNCTION_IMPORT,
        &points::OBJECT_CHANGE_COLORS,
        &points::OBJECT_BOUNDS,
        &points::OBJECT_WORLD_BASIS,
        &points::OBJECT_ROOT_VELOCITY,
        &points::LIGHTS_UPDATE,
    ] {
        assert!(!TABLE.owns(other.base), "{} is the guest's", other.name);
    }
    // Every float-free entry and every replacing entry is in the table.
    for &entry in FLOAT_FREE.iter().chain(REPLACES_GUEST) {
        assert!(TABLE.owns(entry), "{entry:#x}");
    }
    assert!(REPLACES_GUEST.iter().all(|e| FLOAT_FREE.contains(e)), "a replacing body never refuses");
    assert!(!FLOAT_FREE.contains(&points::OBJECT_NODE_MATRICES.base));
    assert!(TYPES.iter().all(|t| !FLOAT_FREE.contains(&t.base) && !REPLACES_GUEST.contains(&t.base)));
}

#[test]
fn without_the_aim_update_the_table_is_the_same_but_c7840() {
    let aim = points::UNIT_AIM_UPDATE.base;
    assert!(TABLE.owns(aim) && !WITHOUT_AIM.owns(aim));
    assert_eq!(WITHOUT_AIM.bodies().len(), TABLE.bodies().len() - 1);
    for body in TABLE.bodies().iter().filter(|b| b.point.base != aim) {
        assert!(WITHOUT_AIM.owns(body.point.base), "{}", body.point.name);
        // Every other body keeps its sites, `c60c0`'s after its call to
        // `c7840` among them.
        assert!(body.sites.iter().all(|&s| WITHOUT_AIM.resumes(s)), "{}", body.point.name);
    }
    for &site in TABLE.at(aim).unwrap().0.sites {
        assert!(!WITHOUT_AIM.resumes(site), "{site:#x}");
    }
    // The float-free bodies are the same, and the counters are TABLE's.
    for body in WITHOUT_AIM.bodies() {
        let ftz = CpuAbi { mxcsr: 0x9f80, ..Default::default() };
        assert_eq!(WITHOUT_AIM.supports_at(body.point.base, &ftz), TABLE.supports_at(body.point.base, &ftz));
        assert!(TABLE.index(body.point.base).is_some());
    }
}

#[test]
fn bd090_and_the_type_callbacks_need_the_float_environment_at_their_entries_and_sites() {
    let mut good = CpuAbi { mxcsr: 0x1f80, ..Default::default() };
    good.x87.control = 0x027f;
    good.x87.status = 0;
    // Flush to zero: outside the environment the float bodies assume.
    let ftz = CpuAbi { mxcsr: 0x9f80, ..good };
    assert!(TABLE.supports(&good) && !TABLE.supports(&ftz));
    for body in TABLE.bodies() {
        // `bd090`, the type callbacks and the walks' SSE callouts (the
        // spatial visit, the early-mover record, `object_new`) use float
        // state; every other walk body is float free.
        let sse = [&points::OBJECT_SPATIAL_VISIT, &points::OBJECT_EARLY_MOVER_RECORD, &points::CREATURE_PROBE_OBJECT_NEW];
        let free = body.point.base != points::OBJECT_NODE_MATRICES.base
            && !TYPES.iter().any(|t| t.base == body.point.base)
            && !sse.iter().any(|p| p.base == body.point.base);
        for &pc in std::iter::once(&body.point.base).chain(body.sites) {
            assert!(TABLE.supports_at(pc, &good), "{} at {pc:#x}", body.point.name);
            assert_eq!(TABLE.supports_at(pc, &ftz), free, "{} at {pc:#x}", body.point.name);
        }
    }
    // An address no body owns follows the table's environment.
    assert!(!TABLE.supports_at(0x1234, &ftz));
}

#[test]
fn a_segment_and_a_refusal_are_counted_on_their_body() {
    let _l = lock();
    const STACK: u32 = 0x0057_0800;
    const CALLER: u32 = 0x0001_2345;
    let mut g = Guest::new();
    g.map_zeroed(0x0057_0000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    // The root of no object is none, read from no memory but the stack.
    let root = points::OBJECT_ROOT.base;
    let (body, _) = TABLE.at(root).unwrap();
    let before = of(root);
    let mut cpu = CpuAbi { esp: STACK, ecx: u32::MAX, edx: 7, ..Default::default() };
    let t = segment(body, &mut cpu, &mut g, root).unwrap();
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!((cpu.eax, cpu.ecx, cpu.edx, cpu.esp), (u32::MAX, u32::MAX, 7, STACK + 4));
    assert_eq!(of(root).segments, before.segments + 1);
    assert_eq!(of(root).refused, before.refused);

    // A refusal at one of bd090's sites lands on bd090.
    let pose = points::OBJECT_NODE_MATRICES.base;
    let site = TABLE.at(pose).unwrap().0.sites[0];
    let before = of(pose);
    refused(site, Refusal::FloatEnv);
    let after = of(pose);
    assert_eq!(after.refused[0], before.refused[0] + 1);
    assert_eq!(after.segments, before.segments);
    assert_eq!(Refusal::ALL.map(Refusal::name), ["float_env"]);
    // And one at a weapon update's site on fdfb0, not on the dispatcher
    // that called it.
    let weapon = points::WEAPON_UPDATE.base;
    let site = TABLE.at(weapon).unwrap().0.sites[3];
    let (before, dispatcher) = (of(weapon), of(points::OBJECT_TYPE_UPDATE.base));
    refused(site, Refusal::FloatEnv);
    assert_eq!(of(weapon).refused[0], before.refused[0] + 1);
    assert_eq!(of(points::OBJECT_TYPE_UPDATE.base), dispatcher);
    // Outside the table: nothing.
    refused(0x1234, Refusal::FloatEnv);
}
