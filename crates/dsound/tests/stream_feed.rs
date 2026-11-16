//! A guest that keeps up never starves a stream: Halo's two ways of feeding
//! one, run on the real kernel scheduler with logical time, the registered
//! overrides and the logical-pacing output, the way `boot` runs the game
//! (`m.run(1)`, then `Output::advance_to` the kernel clock).
//!
//! - Callback refill: the voice streams (`DirectSoundCreateStream` at
//!   `0x335ed3`) keep two 20 ms packets (8 kHz mono PCM16, 320 bytes) and
//!   resubmit each from its completion callback (`0x335d82` →
//!   `0x335d38`).
//! - Polled refill: the sound core's streams (`0x2ae68d`) keep two packets
//!   whose statuses the once-a-frame sound update (`0x21ee80` →
//!   `0x2ae820` → `0x2ae290`) reads, refilling any that is no longer
//!   `E_PENDING`.
//!
//! The guest's frame is paced at 60 Hz or 30 Hz with jitter: it sleeps
//! (`KeDelayExecutionThread`) until its sound update, refills, and calls
//! `DirectSoundDoWork` as the sound update at `0x21ec37` does. Without the
//! completion DPC (the old code) a callback waited for that call, up to a
//! frame and more after its packet completed, so a callback-fed stream
//! starved; with it there is no starve, which is also no gap and no click at
//! a gap's edges.

#[path = "../../kernel/tests/common/mod.rs"]
mod common;
use common::*;

use audio::Mixer;
use dsound::abi::XMEDIAPACKET_STATUS_PENDING;
use dsound::output::QUANTUM;
use dsound::register::{Events, Hle, register};
use dsound::{DirectSound, Layout, OutputConfig, Pacing};
use runtime::{CpuAbi, Event, ExecCtx, Runner, TransferAbi};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CREATE_STREAM: u32 = 0x0037_d835;
const PROCESS: u32 = 0x0037_ad25;
const DO_WORK: u32 = 0x0037_b844;
const MIX: u64 = 48_000;
/// Logical ticks per second.
const TICKS: u64 = 10_000_000;

struct KernelEvents(Arc<kernel::Kernel>);

impl Events for KernelEvents {
    fn signal(&mut self, handle: u32) -> Result<(), String> {
        self.0
            .signal_event(handle)
            .map(drop)
            .map_err(|e| e.to_string())
    }
}

#[derive(Clone, Copy, Debug)]
enum Refill {
    Callback,
    Poll,
}

#[derive(Clone, Copy, Debug)]
struct Game {
    refill: Refill,
    /// Frame period in ticks.
    frame: u64,
    /// When in its frame the sound update runs, ms: uniform in this range.
    update_ms: (u64, u64),
    /// Stream format: rate, channels, and packet length in frames.
    rate: u32,
    channels: u16,
    packet_frames: u32,
    seconds: u64,
}

impl Game {
    /// Halo's voice stream, at the menu's 60 Hz.
    fn voice(frame: u64, update_ms: (u64, u64)) -> Self {
        Self {
            refill: Refill::Callback,
            frame,
            update_ms,
            rate: 8_000,
            channels: 1,
            packet_frames: 160,
            seconds: 10,
        }
    }
    fn packet_bytes(&self) -> u32 {
        self.packet_frames * 2 * u32::from(self.channels)
    }
}

const HZ60: u64 = TICKS / 60;
const HZ30: u64 = TICKS / 30;

#[derive(Debug)]
struct Outcome {
    starves: u64,
    starve_frames: u64,
    refills: u64,
    /// Most callbacks waiting at once between slices.
    max_waiting: usize,
}

/// A tiny deterministic generator for the jitter.
fn lcg(state: &AtomicU64, n: u64) -> u64 {
    let s = state
        .load(Ordering::Relaxed)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    state.store(s, Ordering::Relaxed);
    (s >> 33) % n.max(1)
}

fn ds_call(h: &Harness, addr: u32, args: &[u32]) -> u32 {
    let mut cpu = CpuAbi {
        esp: STACK_TOP - 0x100,
        ..CpuAbi::xbox_thread()
    };
    {
        let mut m = h.runtime.memory();
        m.set_u32(cpu.esp, runtime::exec::HARNESS_EXIT).unwrap();
        for (i, a) in args.iter().enumerate() {
            m.set_u32(cpu.esp + 4 + i as u32 * 4, *a).unwrap();
        }
    }
    let mut ctx = ExecCtx::new(h.runtime.clone(), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, addr);
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
    cpu.eax
}

/// Guest memory layout.
const FMT: u32 = DATA + 0x100;
const DESC: u32 = DATA + 0x200;
const OUT: u32 = DATA + 0x280;
const PACKETS: u32 = DATA + 0x300;
const STATUS: u32 = DATA + 0x400;
const INTERVAL: u32 = DATA + 0x800;
const BUFFERS: u32 = DATA + 0x1000;
/// Each packet's buffer (the largest packet here is 0x4000 bytes).
const BUF: u32 = 0x4000;
const STREAM_CONTEXT: u32 = 0x5157;

fn packet(i: u32) -> u32 {
    PACKETS + i * 0x20
}

fn run(game: Game, completion_dpc: bool) -> Outcome {
    let h = Harness::with_guest(kernel::Config::default(), |g| {
        g.map_zeroed(0x0037_9000, 0xf000, runtime::guest::Permissions::RX)
            .unwrap();
    });
    let mixer = Arc::new(Mutex::new(Mixer::new(MIX as u32)));
    let hle = Hle::new(
        DirectSound::new(Arc::clone(&mixer), Layout::default()),
        Box::new(KernelEvents(Arc::clone(&h.kernel))),
    );
    h.runtime
        .edit_dispatch(|d| register(d, &hle))
        .expect("register");
    let origin = h.kernel.clock().counter();
    let output = hle
        .spawn_output(
            Arc::clone(&h.runtime),
            || audio::NullSink::new(MIX as u32, MIX as usize),
            OutputConfig {
                period: Duration::from_millis(5),
                pacing: Pacing::Logical {
                    origin,
                    max_queued: Duration::from_millis(250),
                    playout: None,
                },
                halt_when: None,
                side: None,
            },
        )
        .unwrap();
    if completion_dpc {
        // As `boot::dsound_glue` installs it.
        h.kernel
            .add_periodic(kernel::periodic::Source {
                name: "dsound completions",
                rate: kernel::periodic::Rate {
                    num: MIX,
                    den: QUANTUM,
                },
                max_catch_up: 64,
                handler: Box::new(hle.completion_dpc()),
            })
            .unwrap();
    }

    // PCM16 WAVEFORMATEX.
    let block = 2 * game.channels;
    let mut b = Vec::new();
    b.extend(1u16.to_le_bytes());
    b.extend(game.channels.to_le_bytes());
    b.extend(game.rate.to_le_bytes());
    b.extend((game.rate * u32::from(block)).to_le_bytes());
    b.extend(block.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(0u16.to_le_bytes());
    h.bytes(FMT, &b);
    let callback = match game.refill {
        Refill::Callback => CODE + 0x200,
        Refill::Poll => 0,
    };
    for (i, w) in [0, 2, FMT, callback, STREAM_CONTEXT, 0].iter().enumerate() {
        h.set(DESC + i as u32 * 4, *w);
    }
    assert_eq!(ds_call(&h, CREATE_STREAM, &[DESC, OUT, 0]), 0);
    let stream = h.get(OUT);
    // Two packets of a steady non-zero signal, both queued.
    let bytes = game.packet_bytes();
    assert!(bytes <= BUF);
    let pcm: Vec<u8> = (0..bytes / 2)
        .flat_map(|_| 0x2000i16.to_le_bytes())
        .collect();
    for i in 0..2 {
        h.bytes(BUFFERS + i * BUF, &pcm);
        let status = STATUS + i * 8;
        for (k, w) in [BUFFERS + i * BUF, bytes, status + 4, status, i, 0]
            .iter()
            .enumerate()
        {
            h.set(packet(i) + k as u32 * 4, *w);
        }
        assert_eq!(ds_call(&h, PROCESS, &[stream, packet(i), 0]), 0);
    }

    let refills = Arc::new(AtomicU64::new(0));
    let frame_no = Arc::new(AtomicU64::new(0));
    let rng = Arc::new(AtomicU64::new(0x5eed));
    let frames = game.seconds * TICKS / game.frame;
    // The frame: sleep until this frame's sound update.
    {
        let (k, frame_no, rng) = (
            Arc::clone(&h.kernel),
            Arc::clone(&frame_no),
            Arc::clone(&rng),
        );
        h.code(0, move |cpu, ctx, _| {
            let n = frame_no.load(Ordering::Relaxed);
            let (lo, hi) = game.update_ms;
            let at = origin + n * game.frame + (lo + lcg(&rng, hi - lo)) * TICKS / 1000;
            let wait = at.saturating_sub(k.clock().counter()).max(1);
            {
                let mut m = ctx.memory();
                let rel = -(wait as i64);
                m.set_u32(INTERVAL, rel as u32)?;
                m.set_u32(INTERVAL + 4, (rel >> 32) as u32)?;
            }
            guest_call(
                cpu,
                ctx,
                runtime::kernel::function_token(kernel::ord::KE_DELAY_EXECUTION_THREAD),
                &[0, 0, INTERVAL],
                CODE + 0x10,
            )
        });
    }
    // The sound update: poll and refill (polled streams), packet 0 then 1.
    for i in 0..2u32 {
        let refills = Arc::clone(&refills);
        let poll = matches!(game.refill, Refill::Poll);
        h.code(0x10 + i * 8, move |cpu, ctx, _| {
            let next = CODE + 0x10 + (i + 1) * 8;
            if poll && ctx.memory().u32(STATUS + i * 8)? != XMEDIAPACKET_STATUS_PENDING {
                refills.fetch_add(1, Ordering::Relaxed);
                return guest_call(cpu, ctx, PROCESS, &[stream, packet(i), 0], next);
            }
            Ok(TransferAbi::jump(next))
        });
    }
    // Then DirectSoundDoWork, as Halo's sound update calls it.
    h.code(0x20, |cpu, ctx, _| {
        guest_call(cpu, ctx, DO_WORK, &[], CODE + 0x30)
    });
    {
        let frame_no = Arc::clone(&frame_no);
        h.code(0x30, move |cpu, ctx, _| {
            if frame_no.fetch_add(1, Ordering::Relaxed) + 1 >= frames {
                return ctx.memory().ret(cpu, 0);
            }
            Ok(TransferAbi::jump(CODE))
        });
    }
    // The completion callback (stdcall, 3 arguments): resubmit on success.
    {
        let refills = Arc::clone(&refills);
        h.code(0x200, move |cpu, ctx, _| {
            let (context, which, status) = {
                let m = ctx.memory();
                (
                    m.u32(cpu.esp + 4)?,
                    m.u32(cpu.esp + 8)?,
                    m.u32(cpu.esp + 12)?,
                )
            };
            assert_eq!(context, STREAM_CONTEXT);
            if status != 0 {
                return ctx.memory().ret(cpu, 12);
            }
            refills.fetch_add(1, Ordering::Relaxed);
            guest_call(cpu, ctx, PROCESS, &[stream, packet(which), 0], CODE + 0x210)
        });
        h.code(0x210, |cpu, ctx, _| ctx.memory().ret(cpu, 12));
    }

    let cpu = CpuAbi {
        eip: CODE,
        esp: STACK_TOP - 4,
        ..CpuAbi::xbox_thread()
    };
    h.set(cpu.esp, runtime::exec::HARNESS_EXIT);
    let mut m = kernel::Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    let mut max_waiting = 0;
    let outcome = loop {
        let outcome = m.run(1).unwrap();
        if !matches!(outcome, kernel::RunOutcome::SliceLimit) {
            break outcome;
        }
        output.advance_to(h.kernel.clock().counter()).unwrap();
        max_waiting = max_waiting.max(hle.pending_callbacks());
    };
    output.halt();
    assert!(
        matches!(outcome, kernel::RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert!(hle.take_stops().is_empty());
    let health = mixer.lock().unwrap().stream_health();
    Outcome {
        starves: health.starves,
        starve_frames: health.starve_frames,
        refills: refills.load(Ordering::Relaxed),
        max_waiting,
    }
}

/// Packets the stream plays in `game.seconds`, less two for the edges.
fn expected_refills(game: &Game) -> u64 {
    game.seconds * u64::from(game.rate) / u64::from(game.packet_frames) - 2
}

fn check(game: Game) {
    let old = run(game, false);
    eprintln!("{game:?}\n  without the completion DPC: {old:?}");
    assert!(old.starves > 0, "the old delivery should starve: {old:?}");
    let new = run(game, true);
    eprintln!("  with it: {new:?}");
    assert_eq!((new.starves, new.starve_frames), (0, 0), "{new:?}");
    assert!(new.refills >= expected_refills(&game), "{new:?}");
}

#[test]
fn a_callback_fed_stream_never_starves_at_60_hz() {
    check(Game::voice(HZ60, (3, 15)));
}

#[test]
fn a_callback_fed_stream_never_starves_at_30_hz() {
    check(Game::voice(HZ30, (8, 31)));
}

#[test]
fn callbacks_wait_at_most_a_quantum() {
    let new = run(Game::voice(HZ30, (8, 31)), true);
    // Each render leaves at most one per packet that completed in it.
    assert!(new.max_waiting <= 2, "{new:?}");
}

/// Polled refill with Halo's own packets (16 KB of 44.1 kHz stereo PCM16,
/// 93 ms, from `0x47f0e0`): the once-a-frame poll keeps the stream fed at
/// 60 Hz and at 30 Hz with jitter, with or without the completion DPC (the
/// poll already saw completions up to its own time; the DPC changes
/// nothing for it).
#[test]
fn a_polled_stream_with_halos_packets_never_starves() {
    for (frame, update_ms) in [(HZ60, (3, 15)), (HZ30, (8, 31))] {
        let game = Game {
            refill: Refill::Poll,
            frame,
            update_ms,
            rate: 44_100,
            channels: 2,
            packet_frames: 0x4000 / 4,
            seconds: 20,
        };
        for dpc in [false, true] {
            let o = run(game, dpc);
            eprintln!("{game:?} dpc {dpc}: {o:?}");
            assert_eq!((o.starves, o.starve_frames), (0, 0), "{o:?}");
            assert!(o.refills >= expected_refills(&game), "{o:?}");
        }
    }
}

/// The completion DPC calls an entry the override layer owns.
#[test]
fn the_completion_dpc_calls_do_work() {
    assert_eq!(dsound::register::COMPLETION_DPC_TARGET, DO_WORK);
    let e = dsound::overrides::ENTRIES
        .iter()
        .find(|e| e.va == DO_WORK)
        .unwrap();
    assert_eq!((e.name, e.argc), ("DirectSoundDoWork", 0));
}
