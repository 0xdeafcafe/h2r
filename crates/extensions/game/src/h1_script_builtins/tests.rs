use super::*;
use guest_layout::{entry::{EntryFuture, Regs, testing}, testing::FakeGuest};

const STACK: u32 = 0x0001_0000;
const CLOCK: u32 = 0x0002_0000;
const CONTROL: u32 = 0x0003_0000;
const ARGUMENTS: u32 = 0x0004_0000;
const DESCRIPTOR: u32 = 0x0005_0000;
const THREAD: u32 = 0xabcd_0001;

fn slot(builtin: RuntimeBuiltin) -> &'static RuntimeSlot {
    RUNTIME_SLOTS.iter().find(|s| s.builtin == builtin).unwrap()
}

fn image(slot: &'static RuntimeSlot) -> FakeGuest {
    let entry = FUNCTION_TABLE + 4 * u32::from(slot.index);
    let mut g = FakeGuest::new();
    g.map(STACK, 16)
        .map(CLOCK, 32)
        .map(ARGUMENTS, 12)
        .map(entry, 4)
        .map(points::GAME_TIME.base, 4)
        .put_u32(STACK + 4, u32::from(slot.index))
        .put_u32(STACK + 8, THREAD)
        .put_u32(STACK + 12, 1)
        .put_u32(points::GAME_TIME.base, CLOCK)
        .put_u32(entry, DESCRIPTOR);
    g
}

fn regs() -> Regs {
    Regs { esp: STACK, ..Regs::default() }
}

#[test]
fn game_time_returns_the_clock_tick_through_haloscript() {
    let slot = slot(RuntimeBuiltin::GameTime);
    let mut g = image(slot);
    g.put_u32(CLOCK + 8, 4521);
    let f = move |cx| -> EntryFuture { Box::pin(evaluate(cx, slot, Arc::default())) };
    let run = testing::run(&f, g, regs(), |_, _| Regs::default());
    assert_eq!(run.result, Ok(Ret::new(0, 12)));
    assert_eq!(
        run.calls,
        [GuestCall::new(RETURN_FROM_EXPRESSION).ecx(THREAD).eax(4521)]
    );
}

#[test]
fn speed_uses_the_native_argument_frame_and_waits_for_child_expressions() {
    let slot = slot(RuntimeBuiltin::GameSpeed);
    let f = move |cx| -> EntryFuture { Box::pin(evaluate(cx, slot, Arc::default())) };
    let mut g = image(slot);
    let speed = 5.0f32.to_bits();
    g.put_u32(ARGUMENTS, speed);
    let argument_call = GuestCall::new(EVALUATE_ARGUMENTS).args([THREAD, 1, DESCRIPTOR + 14, 1]);
    let pending = testing::run(&f, g.clone(), regs(), |_, _| Regs::default());
    assert_eq!(pending.result, Ok(Ret::new(0, 12)));
    assert_eq!(pending.calls, std::slice::from_ref(&argument_call));
    assert_eq!(pending.guest.u32(CLOCK + crate::cheats::TIME_SPEED), Ok(0));

    let done = testing::run(&f, g, regs(), |call, _| Regs {
        eax: if call.target == EVALUATE_ARGUMENTS { ARGUMENTS } else { 0 },
        ..Regs::default()
    });
    assert_eq!(done.result, Ok(Ret::new(0, 12)));
    assert_eq!(
        done.calls,
        [argument_call, GuestCall::new(RETURN_FROM_EXPRESSION).ecx(THREAD).eax(0)]
    );
    assert_eq!(done.guest.u32(CLOCK + crate::cheats::TIME_SPEED), Ok(speed));
}

#[test]
fn look_and_move_queries_each_require_all_four_directions() {
    for (kind, required, unrelated) in [
        (RuntimeBuiltin::LookAllDirections, LOOK_DIRECTIONS, MOVE_DIRECTIONS),
        (RuntimeBuiltin::MoveAllDirections, MOVE_DIRECTIONS, LOOK_DIRECTIONS),
    ] {
        let slot = slot(kind);
        let f = move |cx| -> EntryFuture { Box::pin(evaluate(cx, slot, Arc::default())) };
        for (flags, result) in [(required & (required - 1), 0), (required | 1, 1), (unrelated, 0)] {
            let mut g = image(slot);
            g.map(crate::touch_glue::PLAYER_CONTROL, 4)
                .map(CONTROL, 8)
                .put_u32(crate::touch_glue::PLAYER_CONTROL, CONTROL)
                .put_u32(CONTROL + ACTION_TEST_FLAGS, flags);
            let run = testing::run(&f, g, regs(), |_, _| Regs::default());
            assert_eq!(
                run.calls,
                [GuestCall::new(RETURN_FROM_EXPRESSION).ecx(THREAD).eax(result)]
            );
        }
    }
}

#[test]
fn the_legacy_thumbstick_layouts_use_the_other_look_stick() {
    let slot = slot(RuntimeBuiltin::JoystickNormal);
    let f = move |cx| -> EntryFuture { Box::pin(evaluate(cx, slot, Arc::default())) };
    for (layout, expected) in [(0, 1), (1, 1), (2, 0), (3, 0)] {
        let mut g = image(slot);
        g.map(PLAYER0_THUMBSTICK_LAYOUT, 2).put_u16(PLAYER0_THUMBSTICK_LAYOUT, layout);
        let run = testing::run(&f, g, regs(), |_, _| Regs::default());
        assert_eq!(run.calls, [GuestCall::new(RETURN_FROM_EXPRESSION).ecx(THREAD).eax(expected)]);
    }
}

#[test]
fn extra_ticks_run_before_frame_updates_and_use_the_completed_time() {
    let mut g = image(slot(RuntimeBuiltin::GameSkipTicks));
    g.put_u32(STACK + 4, 0.25f32.to_bits())
        .put_u32(CLOCK + crate::cheats::TIME_TICK_SECONDS, 0.125f32.to_bits());
    let f = |cx| -> EntryFuture { Box::pin(skip_in_frame(cx, 5)) };
    let run = testing::run(&f, g, regs(), |call, m| {
        if call.target == points::GAME_UPDATE_TICKS.base {
            assert_eq!(m.f32(STACK + 4), Ok(0.625));
            // The retail loop can abort before finishing the whole batch.
            m.set_f32(STACK + 4, 0.375).unwrap();
        }
        Regs::default()
    });
    assert_eq!(run.result, Ok(Ret::new(0, 4)));
    assert_eq!(
        run.calls,
        [
            GuestCall::new(points::GAME_UPDATE_TICKS.base).eax(5).ebx(STACK + 4),
            GuestCall::new(points::GAME_UPDATE_FRAME.base).args([0.625f32.to_bits()]),
        ]
    );
}

#[test]
fn numeric_countdown_reads_the_duration_and_start_flag_after_the_extra_argument() {
    let slot = slot(RuntimeBuiltin::CountdownSet);
    let mut g = image(slot);
    g.put_u32(ARGUMENTS, 17)
        .put_u32(ARGUMENTS + 4, 3_723_123)
        .put_u32(ARGUMENTS + 8, 1);
    let state = Arc::new(State::default());
    let shared = Arc::clone(&state);
    let f = move |cx| -> EntryFuture { Box::pin(evaluate(cx, slot, Arc::clone(&shared))) };
    let result = testing::run(&f, g, regs(), |call, _| Regs {
        eax: if call.target == EVALUATE_ARGUMENTS { ARGUMENTS } else { 0 },
        ..Regs::default()
    });
    assert_eq!(result.result, Ok(Ret::new(0, 12)));
    assert_eq!(
        result.calls[0],
        GuestCall::new(EVALUATE_ARGUMENTS).args([THREAD, 3, DESCRIPTOR + 14, 1]),
    );
    let mut timer = state.countdown.lock().unwrap();
    assert!(timer.running);
    assert_eq!(timer.remaining, 3_723_123);
    // 1:02:03.123. Retail 0x56aa0 takes place 4 as ms / 10000 mod 6, so the
    // seconds tens of 372 ten-second units is 0, not 372 mod 10.
    assert_eq!((timer.digit(3), timer.digit(4), timer.digit(5), timer.digit(6)), (3, 0, 2, 0));
    timer.tick(1);
    assert_eq!(timer.remaining, 3_723_090);
    timer.tick(3);
    assert_eq!(timer.remaining, 3_723_023);
    let mut restored = Countdown::default();
    restored.load(&timer.save()).unwrap();
    restored.tick(6);
    assert_eq!(restored.remaining, 3_722_923);
    restored.tick(30 * 3600);
    assert_eq!(restored.remaining, 123_123);
    restored.tick(30 * 3724);
    assert_eq!(restored.remaining, 0);
}

#[test]
fn only_the_campaign_authority_dispatches_a_cinematic_tick_request() {
    let mut g = FakeGuest::new();
    const SIM: u32 = 0x12000;
    const CINEMA: u32 = 0x32000;
    g.put_u32(points::SIM_GLOBALS.base,SIM).put_u32(CINEMATIC_GLOBALS,CINEMA);
    for (mode,author) in [(1,true),(2,true),(3,false),(4,false),(5,false)] {
        g.put_u32(SIM+SIM_MODE,mode).put_u8(CINEMA+CINEMATIC_ACTIVE,1);
        assert_eq!(authoritative_cinematic(&g),Ok(author));
        g.put_u8(CINEMA+CINEMATIC_ACTIVE,0);
        assert_eq!(authoritative_cinematic(&g),Ok(false));
    }
}
