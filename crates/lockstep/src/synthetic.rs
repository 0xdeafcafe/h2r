//! A toy guest for harness tests: no game code, no game data.
//!
//! It has the shape that matters to the harness. Its tick is a generated
//! handler (so [`crate::install_tick_observer`] wraps it like Halo 2's
//! `game_tick`), it keeps a game-time record behind a pointer with the tick
//! count at `+8`, it reads its input from an `XINPUT_STATE`-sized packet in
//! guest memory, and its physics has a threshold quirk (a crouched landing
//! at speed multiplies the bounce), standing in for a super bounce: one
//! changed input on the wrong tick sends the run somewhere else entirely.

#![allow(unsafe_code)]

use std::sync::Arc;

use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, ExecCtx, ExecCtxOpaque, Guest, Runner, Runtime, TransferAbi};

/// Guest data page.
pub const DATA: u32 = 0x0001_0000;
/// The tick function.
pub const TICK: u32 = 0x0040_0000;
/// Where the game-time pointer lives (like Halo 2's `0x00510c54`).
pub const GAME_TIME_PTR: u32 = DATA;
/// The game-time record; `+8` is the tick count.
pub const GAME_TIME: u32 = DATA + 0x100;
/// The input packet the tick reads (an `XINPUT_STATE`).
pub const INPUT: u32 = DATA + 0x200;
/// Player state: x, y, vx, vy (i32, fixed point 1/256), then a crouch latch.
pub const PLAYER: u32 = DATA + 0x300;
const STACK_BOTTOM: u32 = 0x6fff_0000;
const STACK_TOP: u32 = 0x7000_0000;
/// The observer hook for this guest.
pub const HOOK: crate::TickHook = crate::TickHook {
    entry: TICK,
    game_time: Some(GAME_TIME_PTR),
};

/// Logical time per frame: two 60 Hz vblanks.
pub const FRAME_TICKS: u64 = 333_333;

fn i32_at(g: &Guest, a: u32) -> i32 {
    g.u32(a).unwrap_or(0) as i32
}

fn step(g: &mut Guest) -> Result<(), runtime::GuestFault> {
    // XINPUT_STATE: dwPacketNumber, wButtons, bAnalogButtons[8], sThumbLX..
    let buttons = g.u16(INPUT + 4)?;
    let a_pressure = g.u8(INPUT + 6)?;
    let lx = g.u16(INPUT + 14)? as i16 as i32;
    let crouch = buttons & 0x40 != 0; // left thumb click
    let (mut x, mut y) = (i32_at(g, PLAYER), i32_at(g, PLAYER + 4));
    let (mut vx, mut vy) = (i32_at(g, PLAYER + 8), i32_at(g, PLAYER + 12));
    vx = (vx + lx / 512).clamp(-2048, 2048);
    if a_pressure > 128 && y == 0 {
        vy = 900; // jump
    }
    vy -= 60; // gravity
    x = x.wrapping_add(vx);
    y += vy;
    if y <= 0 && vy < 0 {
        // Landing. On a "seam" (1024 units wide, every 4096) while
        // crouched, the push-out overshoots: the toy's super bounce.
        let seam = (2048..3072).contains(&x.rem_euclid(4096));
        vy = if crouch && seam && vy < -600 {
            -vy * 6
        } else {
            0
        };
        y = 0;
    }
    g.set_u32(PLAYER, x as u32)?;
    g.set_u32(PLAYER + 4, y as u32)?;
    g.set_u32(PLAYER + 8, vx as u32)?;
    g.set_u32(PLAYER + 12, vy as u32)?;
    let tick = g.u32(GAME_TIME + 8)?;
    g.set_u32(GAME_TIME + 8, tick.wrapping_add(1))
}

/// The tick, as a generated handler: run one step and return to the caller.
unsafe extern "C" fn tick(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, _pc: u32) -> TransferAbi {
    // SAFETY: the runner passes its exclusive, invocation-scoped pair, and
    // ExecCtxOpaque is ExecCtx behind the FFI boundary.
    let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx.cast::<ExecCtx>()) };
    let mut g = ctx.memory();
    let result = step(&mut g).and_then(|()| g.u32(cpu.esp));
    drop(g);
    match result {
        Ok(ret) => {
            cpu.esp = cpu.esp.wrapping_add(4);
            TransferAbi::ret(ret)
        }
        Err(fault) => ctx.fault(TICK, fault),
    }
}

/// One toy machine.
pub struct Toy {
    pub runtime: Arc<Runtime>,
    ctx: ExecCtx,
    clock: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for Toy {
    fn default() -> Self {
        Self::new()
    }
}

impl Toy {
    pub fn new() -> Self {
        let mut memory = Guest::new();
        memory
            .map_zeroed(DATA, 0x1000, Permissions::RW)
            .expect("data");
        memory
            .map_zeroed(TICK, 0x1000, Permissions::RWX)
            .expect("code");
        memory
            .map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)
            .expect("stack");
        memory
            .set_u32(GAME_TIME_PTR, GAME_TIME)
            .expect("game time pointer");
        memory.set_u32(GAME_TIME + 2, 30).expect("tick rate");
        let mut dispatch = Dispatch::default();
        // SAFETY: `tick` follows the generated-handler ABI above.
        unsafe { dispatch.register_generated(TICK, runtime::cpu::ABI_VERSION, tick) }
            .expect("tick");
        let runtime = Runtime::new(memory, dispatch);
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let c = Arc::clone(&clock);
        runtime.set_guest_clock(Some(Arc::new(move || {
            Some(c.load(std::sync::atomic::Ordering::Relaxed))
        })));
        let ctx = ExecCtx::new(Arc::clone(&runtime), 1);
        Self {
            runtime,
            ctx,
            clock,
        }
    }

    /// Writes the packet the guest will read this frame (as XInputGetState
    /// would).
    pub fn set_input(&self, packet: &[u8; 22]) {
        self.runtime
            .memory()
            .write_bytes(INPUT, packet, 1)
            .expect("input packet");
    }

    /// One frame: the clock moves a frame, then the tick runs once.
    pub fn frame(&mut self) {
        self.clock
            .fetch_add(FRAME_TICKS, std::sync::atomic::Ordering::Relaxed);
        let mut cpu = CpuAbi {
            esp: STACK_TOP - 0x100,
            ..CpuAbi::xbox_thread()
        };
        self.runtime
            .memory()
            .set_u32(cpu.esp, runtime::exec::HARNESS_EXIT)
            .expect("return");
        let t = Runner { max_transfers: 64 }.run(&mut cpu, &mut self.ctx, TICK);
        assert!(
            matches!(
                self.ctx.event(t.event_id),
                Some(runtime::Event::HarnessExit)
            ),
            "the tick did not return: {t:?} {:?}",
            self.ctx.event(t.event_id)
        );
        let _ = self.ctx.take_events();
    }

    pub fn clock(&self) -> u64 {
        self.clock.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The player's height (1/256 units), for "did it bounce" checks.
    pub fn height(&self) -> i32 {
        i32_at(&self.runtime.memory(), PLAYER + 4)
    }
}

/// An `XINPUT_STATE` packet: buttons, the A pressure and the left stick X.
pub fn packet(number: u32, buttons: u16, a: u8, lx: i16) -> [u8; 22] {
    let mut p = [0u8; 22];
    p[0..4].copy_from_slice(&number.to_le_bytes());
    p[4..6].copy_from_slice(&buttons.to_le_bytes());
    p[6] = a;
    p[14..16].copy_from_slice(&lx.to_le_bytes());
    p
}
