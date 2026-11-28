use super::*;
use guest_layout::datum::layout as dl;
use guest_layout::testing::FakeGuest;

/// The model is process-wide; tests that set it take this.
static MODEL: Mutex<()> = Mutex::new(());

fn model_lock() -> std::sync::MutexGuard<'static, ()> {
    MODEL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn the_override_names_a_model_or_nothing() {
    assert_eq!(parse_override(None), Ok(None));
    assert_eq!(parse_override(Some(" ")), Ok(None));
    assert_eq!(parse_override(Some("halo1")), Ok(Some(HealthModel::Halo1)));
    assert_eq!(parse_override(Some("Halo2")), Ok(Some(HealthModel::Halo2)));
    assert_eq!(parse_override(Some("h1")), Ok(Some(HealthModel::Halo1)));
    assert!(
        parse_override(Some("ce"))
            .unwrap_err()
            .contains("H2_HEALTH_MODEL")
    );
}

#[test]
fn the_override_beats_the_map_and_halo2_is_the_default() {
    let _l = model_lock();
    force(None);
    MAP_MODE.store(0, Ordering::SeqCst);
    assert_eq!(mode(), HealthModel::Halo2, "nothing set: retail");
    set_mode(HealthModel::Halo1);
    assert_eq!(mode(), HealthModel::Halo1);
    force(Some(HealthModel::Halo2));
    assert_eq!(mode(), HealthModel::Halo2, "the debug override wins");
    assert_eq!(forced(), Some(HealthModel::Halo2));
    force(None);
    set_mode(HealthModel::Halo2);
    assert_eq!(mode(), HealthModel::Halo2, "a later map sets it back");
}

fn sample_at(health: f32, flags_high: u8) -> Sample {
    Sample {
        body: 0x1000,
        health,
        flags_high,
    }
}

#[test]
fn halo1_undoes_a_rise_and_keeps_every_fall() {
    let before = sample_at(0.4, 0x00);
    // The regeneration raised it and set "recharging": both undone, the
    // other bits of the byte as the update left them.
    assert_eq!(
        hold(HealthModel::Halo1, before, 0.45, BODY_RECHARGING | 0x10),
        Some((0.4, 0x10))
    );
    // Recharging was already set before: it stays set.
    assert_eq!(
        hold(
            HealthModel::Halo1,
            sample_at(0.4, BODY_RECHARGING),
            0.5,
            BODY_RECHARGING
        ),
        Some((0.4, BODY_RECHARGING))
    );
    // Damage in the same update (a fall) and no change are left alone.
    assert_eq!(hold(HealthModel::Halo1, before, 0.3, 0), None);
    assert_eq!(hold(HealthModel::Halo1, before, 0.4, 0), None);
    // Halo 2's model: retail's result whatever it is.
    assert_eq!(hold(HealthModel::Halo2, before, 0.9, BODY_RECHARGING), None);
}

// ---- a fake game: player 0 on a biped, an AI biped, a vehicle ----

const PLAYERS_H: u32 = 0x0060_0000;
const OBJECTS_H: u32 = 0x0061_0000;
const PLAYER_DATA: u32 = 0x0062_0000;
const OBJECT_DATA: u32 = 0x0063_0000;
const BODY: u32 = 0x0064_0000;
const AI_BODY: u32 = 0x0064_0400;
const VEHICLE_BODY: u32 = 0x0064_0800;
const UNIT: u32 = 0xe001_0000; // salt 0xe001, slot 0
const AI: u32 = 0xe002_0001;
const VEHICLE: u32 = 0xe003_0002;

fn put_game(
    put32: &mut dyn FnMut(u32, u32),
    put16: &mut dyn FnMut(u32, u16),
    put8: &mut dyn FnMut(u32, u8),
) {
    put32(guest_layout::points::PLAYERS.base, PLAYERS_H);
    put32(guest_layout::points::OBJECTS.base, OBJECTS_H);
    for (h, size, data, n) in [
        (PLAYERS_H, 0x21c, PLAYER_DATA, 1),
        (OBJECTS_H, 0xc, OBJECT_DATA, 3),
    ] {
        put32(h + dl::ELEMENT_SIZE, size);
        put32(h + dl::HIGH_WATER, n);
        put32(h + dl::DATA, data);
    }
    // Player 0: local user 0 on UNIT.
    put16(PLAYER_DATA, 0xe0ff);
    put16(PLAYER_DATA + 0x28, 0);
    put32(PLAYER_DATA + 0x2c, UNIT);
    for (i, (salt, kind, body, player)) in [
        (0xe001u16, objects::BIPED, BODY, 0xe0ff_0000u32),
        (0xe002, objects::BIPED, AI_BODY, u32::MAX),
        (0xe003, objects::VEHICLE, VEHICLE_BODY, 0xe0ff_0000),
    ]
    .into_iter()
    .enumerate()
    {
        let h = OBJECT_DATA + 0xc * i as u32;
        put16(h, salt);
        put8(h + 3, kind);
        put32(h + 8, body);
        put32(body + UNIT_PLAYER, player);
        put32(body + BODY_HEALTH, 0.5f32.to_bits());
        put32(body + BODY_SHIELD, 0.0f32.to_bits());
    }
}

fn game() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x004e_0000, 0x1_0000);
    for a in [PLAYERS_H, OBJECTS_H, PLAYER_DATA, OBJECT_DATA, BODY] {
        g.map(a, 0x1000);
    }
    let g = std::cell::RefCell::new(g);
    put_game(
        &mut |a, v| {
            g.borrow_mut().put_u32(a, v);
        },
        &mut |a, v| {
            g.borrow_mut().put_u16(a, v);
        },
        &mut |a, v| {
            g.borrow_mut().put_u8(a, v);
        },
    );
    g.into_inner()
}

#[test]
fn only_a_players_biped_is_sampled() {
    let g = game();
    assert_eq!(
        sample(&g, UNIT).unwrap(),
        Some(Sample {
            body: BODY,
            health: 0.5,
            flags_high: 0
        })
    );
    assert_eq!(sample(&g, AI).unwrap(), None, "AI keeps its own rules");
    assert_eq!(sample(&g, VEHICLE).unwrap(), None, "vehicles too");
    assert_eq!(sample(&g, u32::MAX).unwrap(), None);
    assert_eq!(sample(&g, 0x1234_0000).unwrap(), None, "a stale salt");
}

#[test]
fn apply_hold_writes_back_only_a_rise() {
    let mut g = game();
    let before = sample(&g, UNIT).unwrap().unwrap();
    g.put_f32(BODY + BODY_HEALTH, 0.6)
        .put_u8(BODY + BODY_FLAGS_HIGH, BODY_RECHARGING);
    assert_eq!(
        apply_hold(&mut g, HealthModel::Halo1, before).unwrap(),
        Some(0.6)
    );
    assert_eq!(g.f32(BODY + BODY_HEALTH).unwrap(), 0.5);
    assert_eq!(g.u8(BODY + BODY_FLAGS_HIGH).unwrap(), 0);
    g.put_f32(BODY + BODY_HEALTH, 0.2);
    assert_eq!(
        apply_hold(&mut g, HealthModel::Halo1, before).unwrap(),
        None
    );
    assert_eq!(g.f32(BODY + BODY_HEALTH).unwrap(), 0.2);
}

#[test]
fn the_lines_say_what_happened() {
    assert_eq!(
        restore_line(UNIT, Some(0.3), true, HealthModel::Halo1),
        "health: health pack: object 0xe0010000, a player's body at 0.300, restored to full (Halo 1 model)"
    );
    assert!(restore_line(UNIT, Some(1.0), false, HealthModel::Halo1).contains("refused"));
    assert_eq!(
        start_line(HealthModel::Halo2, false),
        "health: game start: Halo 2 model"
    );
    assert_eq!(
        start_line(HealthModel::Halo1, true),
        "health: game start: Halo 1 model (debug override H2_HEALTH_MODEL): no body regeneration; the health meter is in a converted Halo 1 map's HUD"
    );
}

/// The hooks on a plan, around stand-ins for the damage update (it
/// regenerates or damages the body) and the body restore, on the runtime
/// fixture module: `0x402000` the update, `0x403000` the restore.
#[test]
fn the_hooks_hold_regeneration_on_the_feature_plan() {
    use hooks::points::{BODY_RESTORE, GAME_ACTIVATE, GAME_TICK, OBJECT_DAMAGE_UPDATE};
    use runtime::guest::Permissions;
    use runtime::{Dispatch, Event, Guest, Runner, Runtime, exec::HARNESS_EXIT};
    const UPDATE: u32 = 0x40_2000;
    const RESTORE: u32 = 0x40_3000;
    const FRAME: u32 = 0x60_0e00;
    let _l = model_lock();

    let mut d = Dispatch::default();
    // SAFETY: the fixture's tables, handlers and probes follow its ABI.
    unsafe {
        d.register_generated_module(
            &runtime_test_probe::h2gen_fixture_module,
            runtime_test_probe::h2gen_fixture_module_abi_version,
        )
        .unwrap();
    }
    let mut g = Guest::new();
    g.map_zeroed(0x40_0000, 0x5000, Permissions::RX).unwrap();
    g.map_zeroed(0x004e_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    for a in [PLAYERS_H, OBJECTS_H, PLAYER_DATA, OBJECT_DATA, BODY] {
        g.map_zeroed(a, 0x1000, Permissions::RW).unwrap();
    }
    let rt = Runtime::new(g, d);
    {
        let mut m = rt.memory();
        let m = std::cell::RefCell::new(&mut *m);
        put_game(
            &mut |a, v| m.borrow_mut().set_u32(a, v).unwrap(),
            &mut |a, v| m.borrow_mut().set_u16(a, v).unwrap(),
            &mut |a, v| m.borrow_mut().set_u8(a, v).unwrap(),
        );
    }
    // The stand-in update: `+delta` to the object's health (stack 0),
    // the recharging bit set on a rise, AL 1, `ret 4`.
    let delta = Arc::new(Mutex::new(0.0f32));
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    {
        let delta = Arc::clone(&delta);
        r.add(&hooks::from_fn("stand-in", move |b| {
            let delta = Arc::clone(&delta);
            b.raw()
                .implement(&OBJECT_DAMAGE_UPDATE, "update", move |cpu, ctx, _| {
                    let rt = Arc::clone(ctx.runtime());
                    let mut mem = rt.memory();
                    let object = mem.u32(cpu.esp + 4)?;
                    let body = mem.u32(OBJECT_DATA + 0xc * (object & 0xffff) + 8)?;
                    let d = *delta.lock().unwrap();
                    let h = f32::from_bits(mem.u32(body + BODY_HEALTH)?);
                    mem.set_u32(body + BODY_HEALTH, (h + d).to_bits())?;
                    if d > 0.0 {
                        mem.set_u8(body + BODY_FLAGS_HIGH, BODY_RECHARGING)?;
                    }
                    cpu.eax = 1;
                    mem.ret(cpu, 4)
                });
            b.raw().implement(&BODY_RESTORE, "restore", |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let body = mem.u32(OBJECT_DATA + 0xc * (cpu.eax & 0xffff) + 8)?;
                mem.set_u32(body + BODY_HEALTH, 1.0f32.to_bits())?;
                cpu.eax = 1;
                mem.ret(cpu, 0)
            });
            Ok(())
        }))
        .unwrap();
    }
    assert!(install(&mut r).unwrap());
    let plan = r
        .build(&hooks::Remap::new(&[
            (&OBJECT_DAMAGE_UPDATE, UPDATE),
            (&BODY_RESTORE, RESTORE),
            (&GAME_TICK, 0x40_3005),
            (&GAME_ACTIVATE, 0x40_4000),
        ]))
        .unwrap();
    let text = plan.text();
    assert!(
        text.contains(
            "OBJECT_DAMAGE_UPDATE: before [h1-health:note a player's health] -> implement stand-in:update -> after [h1-health:hold the body's health]"
        ),
        "{text}"
    );
    let installed = plan.install(&rt).unwrap();
    assert!(
        report(&plan, &installed).starts_with(
            "health: hooks installed via feature plan at 0x402000 (regeneration), 0x403000 (health pack log)"
        ),
        "{}",
        report(&plan, &installed)
    );
    let run = |at: u32, eax: u32, arg: u32| {
        {
            let mut m = rt.memory();
            m.set_u32(FRAME, HARNESS_EXIT).unwrap();
            m.set_u32(FRAME + 4, arg).unwrap();
        }
        let mut cpu = runtime::CpuAbi {
            esp: FRAME,
            eax,
            ..Default::default()
        };
        let mut ctx = runtime::ExecCtx::new(rt.clone(), 1);
        let t = Runner::default().run(&mut cpu, &mut ctx, at);
        assert!(
            matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{:?}",
            ctx.event(t.event_id)
        );
        cpu
    };
    let health = |body: u32| f32::from_bits(rt.memory().u32(body + BODY_HEALTH).unwrap());
    let flags = || rt.memory().u8(BODY + BODY_FLAGS_HIGH).unwrap();

    // Halo 2: retail regenerates.
    force(None);
    set_mode(HealthModel::Halo2);
    *delta.lock().unwrap() = 0.25;
    assert_eq!(
        run(UPDATE, 0, UNIT).eax & 0xff,
        1,
        "the update's result comes back"
    );
    assert_eq!(health(BODY), 0.75);
    rt.memory().set_u8(BODY + BODY_FLAGS_HIGH, 0).unwrap();

    // Halo 1: the player's rise is held; the AI's is not.
    set_mode(HealthModel::Halo1);
    run(UPDATE, 0, UNIT);
    assert_eq!(health(BODY), 0.75, "no regeneration");
    assert_eq!(flags(), 0, "and not marked recharging");
    run(UPDATE, 0, AI);
    assert_eq!(health(AI_BODY), 0.75, "AI is not ours to change");

    // Damage still lands.
    *delta.lock().unwrap() = -0.5;
    run(UPDATE, 0, UNIT);
    assert_eq!(health(BODY), 0.25);

    // A health pack restores it, and the restore stays.
    assert_eq!(run(RESTORE, UNIT, 0).eax & 0xff, 1);
    assert_eq!(health(BODY), 1.0);
    *delta.lock().unwrap() = 0.0;
    run(UPDATE, 0, UNIT);
    assert_eq!(health(BODY), 1.0);

    // The debug override turns it on over a Halo 2 map.
    set_mode(HealthModel::Halo2);
    force(Some(HealthModel::Halo1));
    rt.memory()
        .set_u32(BODY + BODY_HEALTH, 0.5f32.to_bits())
        .unwrap();
    *delta.lock().unwrap() = 0.25;
    run(UPDATE, 0, UNIT);
    assert_eq!(health(BODY), 0.5);
    force(None);
}

#[test]
fn halo1_maps_pick_halo1s_model_by_name_and_sandbox() {
    let m = |n: &str, s: bool| model_for_map(n, s);
    assert_eq!(m("h1_bloodgulch", true), HealthModel::Halo1);
    assert_eq!(m("H1_Bloodgulch", true), HealthModel::Halo1);
    assert_eq!(
        m("h1_bloodgulch", false),
        HealthModel::Halo2,
        "Halo 2 sandbox"
    );
    assert_eq!(
        m("a30", false),
        HealthModel::Halo1,
        "campaigns whatever the sandbox"
    );
    assert_eq!(m("d40", true), HealthModel::Halo1);
    for retail in ["coagulation", "01a_tutorial", "mainmenu", "", "a300"] {
        assert_eq!(m(retail, true), HealthModel::Halo2, "{retail}");
    }
}

#[test]
fn a_draw_can_borrow_the_name_even_at_the_end_of_a_mapping() {
    use guest_layout::memory::FlatMemory;
    let mut mem = FlatMemory::new(CACHE_MAP_NAME, 4);
    mem.write(CACHE_MAP_NAME, b"abc\0").unwrap();
    let mut field = [0; 32];
    assert!(matches!(read_map_name(&mem, &mut field), Ok(std::borrow::Cow::Borrowed("abc"))));
    mem.set_u8(CACHE_MAP_NAME + 3, b'd').unwrap();
    assert!(read_map_name(&mem, &mut field).is_err());
}

#[test]
fn the_loaded_map_name_reads_from_the_cache_header() {
    let mut g = FakeGuest::new();
    g.map(CACHE_MAP_NAME, 0x20);
    g.put(CACHE_MAP_NAME, b"h1_bloodgulch\0junk");
    assert_eq!(loaded_map_name(&g).unwrap(), "h1_bloodgulch");
    g.put(CACHE_MAP_NAME, &[b'x'; 32]);
    assert_eq!(loaded_map_name(&g).unwrap().len(), 32);
    assert!(loaded_map_name(&FakeGuest::new()).is_err());
}

#[test]
fn snapshots_wait_for_a_damage_update_or_restore_between_its_hooks() {
    let s = State::default();
    assert_eq!(s.live(), None);
    s.armed.store(true, Ordering::Relaxed);
    assert!(s.live().unwrap().contains("damage update"));
    s.armed.store(false, Ordering::Relaxed);
    *s.restore.lock().unwrap() = Some((1, None));
    assert!(s.live().unwrap().contains("body restore"));
    *s.restore.lock().unwrap() = None;
    assert_eq!(s.live(), None);
}

#[test]
fn a_loaded_snapshot_picks_the_model_from_the_loaded_map() {
    let _l = model_lock();
    force(None);
    set_mode(HealthModel::Halo2);
    let mut g = FakeGuest::new();
    g.map(CACHE_MAP_NAME, 0x20);
    g.put(CACHE_MAP_NAME, b"a30\0");
    reload_mode(&g).unwrap();
    assert_eq!(mode(), HealthModel::Halo1, "a Halo 1 level was loaded");
    g.put(CACHE_MAP_NAME, b"03a_oldmombasa\0");
    reload_mode(&g).unwrap();
    assert_eq!(mode(), HealthModel::Halo2);
    // No map name to read: the model stays and the load says why.
    assert!(reload_mode(&FakeGuest::new()).is_err());
    assert_eq!(mode(), HealthModel::Halo2);
}
