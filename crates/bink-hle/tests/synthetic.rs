//! Synthetic guest state: a few mapped pages, no XBE, no movie files.
use bink_hle::{
    BinkHle, Clock, HLE_RESERVED, Mode, MovieSource, MovieStream, Mp4Source, StreamInfo,
    SyntheticDecoder, addr::*, register, register_skip, unregister,
};
use runtime::{
    CpuAbi, Dispatch, ExecCtx, Guest, Runner, Runtime, abi, exec::HARNESS_EXIT, guest::Permissions,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const STACK: u32 = 0x0060_0000;
const SCRATCH: u32 = 0x0070_0000; // strings and destination buffers

struct Manual(AtomicU64);
impl Clock for Manual {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// 4x2 frames; pixel bytes are [frame, x, y, 0xff] so copies are checkable.
struct Fake {
    frames: u32,
}
impl MovieStream for Fake {
    fn info(&self) -> StreamInfo {
        StreamInfo {
            width: 4,
            height: 2,
            frames: self.frames,
            fps_num: 30000,
            fps_den: 1001,
        }
    }
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool {
        for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *px = [index as u8, (i % 4) as u8, (i / 4) as u8, 0xff];
        }
        true
    }
}
struct FakeSource {
    name: &'static str,
    frames: u32,
}
impl MovieSource for FakeSource {
    fn open(&self, name: &str) -> Option<Box<dyn MovieStream>> {
        (name == self.name).then(|| {
            Box::new(Fake {
                frames: self.frames,
            }) as Box<dyn MovieStream>
        })
    }
}

struct World {
    ctx: ExecCtx,
    runner: Runner,
    clock: Arc<Manual>,
}

fn world(
    hle: impl FnOnce(Arc<Manual>) -> Arc<BinkHle>,
) -> (World, Arc<BinkHle>, Vec<bink_hle::Registered>, Mode) {
    let mut g = Guest::new();
    // Code pages the overrides sit on (dispatch needs executable memory).
    g.map_zeroed(0x0015_0000, 0x1_0000, Permissions::RWX)
        .unwrap();
    g.map_zeroed(0x003e_0000, 0x1_0000, Permissions::RWX)
        .unwrap();
    // Wrapper state.
    g.map_zeroed(0x004e_9000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    let clock = Arc::new(Manual(AtomicU64::new(1000)));
    let hle = hle(Arc::clone(&clock));
    let mut d = Dispatch::default();
    let (mode, reg) = register(&mut d, &hle);
    let rt = Runtime::new(g, d);
    (
        World {
            ctx: ExecCtx::new(rt, 1),
            runner: Runner::default(),
            clock,
        },
        hle,
        reg,
        mode,
    )
}

impl World {
    fn put_str(&mut self, at: u32, s: &str) {
        let mut m = self.ctx.memory();
        m.write_bytes(at, s.as_bytes(), 1).unwrap();
        m.set_u8(at + s.len() as u32, 0).unwrap();
    }
    /// Call a stdcall function; returns (eax, esp after - esp before call frame).
    fn call(&mut self, f: u32, args: &[u32]) -> (u32, i64) {
        let mut cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        let top = cpu.esp;
        {
            let mut m = self.ctx.memory();
            cpu.esp -= 4 * (args.len() as u32 + 1);
            m.set_u32(cpu.esp, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(cpu.esp + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        let t = self.runner.run(&mut cpu, &mut self.ctx, f);
        assert_eq!(t.kind, abi::STOP, "{t:?} events {:?}", self.ctx.events());
        // esp is back to `top` iff the callee popped the return address and
        // every argument.
        (cpu.eax, cpu.esp as i64 - top as i64)
    }
    fn u32(&mut self, a: u32) -> u32 {
        self.ctx.memory().u32(a).unwrap()
    }
    fn bytes(&mut self, a: u32, n: usize) -> Vec<u8> {
        let mut v = vec![0; n];
        self.ctx.memory().read_bytes(a, &mut v, 1).unwrap();
        v
    }
}

#[test]
fn skip_mode_is_the_default_without_a_source() {
    let (mut w, hle, reg, mode) = world(|_| BinkHle::new(None));
    assert_eq!(mode, Mode::Skip);
    assert_eq!(reg.len(), 1);
    assert_eq!(reg[0].addr, MOVIE_START);
    // The engine had a stale movie open; start must leave "none, finished".
    {
        let mut m = w.ctx.memory();
        m.set_u8(ST_ACTIVE, 1).unwrap();
        m.set_u32(ST_BINK, 0x1234).unwrap();
        m.set_u8(ST_MOVIE_OPEN, 1).unwrap();
        m.set_u8(ST_FINISHED, 0).unwrap();
    }
    w.put_str(SCRATCH, "d:\\bink\\intro_60.bik");
    let (_, esp) = w.call(MOVIE_START, &[SCRATCH, 0x1c6]);
    assert_eq!(esp, 0, "ret 8 must pop return + two args");
    let m = w.ctx.memory();
    assert_eq!(
        m.u32(ST_BINK).unwrap(),
        0,
        "no bink handle: IsPlaying is false"
    );
    assert_eq!(m.u8(ST_ACTIVE).unwrap(), 1, "wrapper stays initialised");
    assert_eq!(m.u8(ST_MOVIE_OPEN).unwrap(), 0);
    assert_eq!(m.u8(ST_FINISHED).unwrap(), 1);
    drop(m);
    let r = hle.requests();
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].path.as_str(), r[0].flags, r[0].played),
        ("d:\\bink\\intro_60.bik", 0x1c6, false)
    );
}

#[test]
fn skip_works_when_wrapper_state_is_unmapped() {
    // A bare harness without the XBE image must not fault.
    let mut g = Guest::new();
    g.map_zeroed(0x0015_0000, 0x1_0000, Permissions::RWX)
        .unwrap();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW)
        .unwrap();
    let hle = BinkHle::new(None);
    let mut d = Dispatch::default();
    register_skip(&mut d, &hle);
    let mut w = World {
        ctx: ExecCtx::new(Runtime::new(g, d), 1),
        runner: Runner::default(),
        clock: Arc::new(Manual(AtomicU64::new(0))),
    };
    let (_, esp) = w.call(MOVIE_START, &[0, 0]); // null path
    assert_eq!(esp, 0);
    assert_eq!(hle.requests()[0].path, "");
}

#[test]
fn source_that_cannot_open_is_a_clean_skip_through_bink_open() {
    let (mut w, hle, _, mode) = world(|_| {
        BinkHle::new(Some(Arc::new(FakeSource {
            name: "attract_60",
            frames: 3,
        })))
    });
    assert_eq!(mode, Mode::Playback);
    w.put_str(SCRATCH, "d:\\bink\\intro_60.bik");
    let (h, esp) = w.call(BINK_OPEN, &[SCRATCH, 0x8002000]);
    assert_eq!((h, esp), (0, 0), "NULL handle, args popped");
    assert!(!hle.requests()[0].played);
    assert_eq!(hle.open_streams(), 0);
}

#[test]
fn playback_cycle_open_decode_copy_next_close() {
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 3,
            })),
            c,
        )
    });
    w.put_str(SCRATCH, "D:\\Bink\\INTRO_60.BIK");
    let (h, esp) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    assert_eq!(esp, 0);
    assert_ne!(h, 0);
    // The fields the engine reads.
    assert_eq!(
        [0x00, 0x04, 0x08, 0x0c, 0x14, 0x18].map(|o| w.u32(h + o)),
        [4, 2, 3, 1, 30000, 1001]
    );
    assert!(hle.requests()[0].played);

    // First frame is due immediately.
    assert_eq!(w.call(BINK_WAIT, &[h]), (0, 0));
    // Copy before any decode writes nothing.
    let dst = SCRATCH + 0x1000;
    assert_eq!(
        w.call(BINK_COPY_TO_BUFFER, &[h, dst, 32, 2, 0, 0, 0x8000_0003]),
        (1, 0)
    );

    assert_eq!(w.call(BINK_DO_FRAME, &[h]), (0, 0));
    // The engine calls NextFrame before CopyToBuffer; copy must still be
    // the frame that was decoded.
    assert_eq!(w.call(BINK_NEXT_FRAME, &[h]), (0, 0));
    assert_eq!(w.u32(h + 0x0c), 2);
    assert_eq!(
        w.call(BINK_COPY_TO_BUFFER, &[h, dst, 32, 2, 1, 0, 0x8000_0003]),
        (0, 0)
    );
    // pitch 32 = 8 pixels; x offset 1 pixel. Row 0: untouched px0, then frame 0.
    let row0 = w.bytes(dst, 20);
    assert_eq!(&row0[..4], &[0, 0, 0, 0]);
    assert_eq!(&row0[4..8], &[0, 0, 0, 0xff]);
    assert_eq!(&row0[8..12], &[0, 1, 0, 0xff]);
    let row1 = w.bytes(dst + 32 + 16, 4);
    assert_eq!(row1, &[0, 3, 1, 0xff]);

    // Next frame is not due until a frame time (33 ms) passes.
    assert_eq!(w.call(BINK_WAIT, &[h]).0, 1);
    w.clock.0.fetch_add(34, Ordering::SeqCst);
    assert_eq!(w.call(BINK_WAIT, &[h]).0, 0);

    // Decode frame 2, then advance: FrameNum reaches Frames.
    w.call(BINK_DO_FRAME, &[h]);
    w.call(BINK_COPY_TO_BUFFER, &[h, dst, 32, 2, 0, 0, 0x8000_0003]);
    assert_eq!(w.bytes(dst, 1)[0], 1, "second frame decoded");
    w.call(BINK_NEXT_FRAME, &[h]);
    assert_eq!(w.u32(h + 0x0c), 3);
    assert_eq!(
        w.u32(h + 0x0c),
        w.u32(h + 0x08),
        "engine's finished test: FrameNum == Frames"
    );

    assert_eq!(w.call(BINK_CLOSE, &[h]), (0, 0));
    assert_eq!(hle.open_streams(), 0);
    assert_eq!(w.u32(h + 0x08), 0);
    // Closed handle: everything is a harmless no-op.
    assert_eq!(w.call(BINK_WAIT, &[h]), (0, 0));
    assert_eq!(w.call(BINK_DO_FRAME, &[h]), (0, 0));
    assert_eq!(w.call(BINK_COPY_TO_BUFFER, &[h, dst, 32, 2, 0, 0, 3]).0, 1);
}

#[test]
fn copy_clips_and_converts() {
    let (mut w, _, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 2,
            })),
            c,
        )
    });
    w.put_str(SCRATCH, "intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    w.call(BINK_NEXT_FRAME, &[h]); // FrameNum 2, so frame index 1 is decoded
    w.call(BINK_DO_FRAME, &[h]);
    let dst = SCRATCH + 0x2000;
    // Destination is 2 pixels wide and 1 row tall: clipped to that. Type 4
    // swaps red and blue: source [1,x,0,ff] becomes [0,x,1,ff].
    assert_eq!(w.call(BINK_COPY_TO_BUFFER, &[h, dst, 8, 1, 0, 0, 4]).0, 0);
    assert_eq!(w.bytes(dst, 8), vec![0, 0, 1, 0xff, 0, 1, 1, 0xff]);
    // Row 1 was clipped away.
    assert_eq!(w.bytes(dst + 8, 8), vec![0; 8]);
    // Unsupported surface type: nothing written, reports 1.
    let dst2 = SCRATCH + 0x3000;
    assert_eq!(w.call(BINK_COPY_TO_BUFFER, &[h, dst2, 16, 2, 0, 0, 9]).0, 1);
    assert_eq!(w.bytes(dst2, 4), vec![0; 4]);
}

#[test]
fn handle_table_is_bounded_and_slots_are_reused() {
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 2,
            })),
            c,
        )
    });
    w.put_str(SCRATCH, "intro_60.bik");
    let hs: Vec<u32> = (0..8).map(|_| w.call(BINK_OPEN, &[SCRATCH, 0]).0).collect();
    assert!(hs.iter().all(|h| *h != 0));
    assert_eq!(
        w.call(BINK_OPEN, &[SCRATCH, 0]).0,
        0,
        "ninth open fails cleanly"
    );
    w.call(BINK_CLOSE, &[hs[3]]);
    assert_eq!(w.call(BINK_OPEN, &[SCRATCH, 0]).0, hs[3]);
    assert_eq!(hle.open_streams(), 8);
}

#[test]
fn sound_and_memory_setup_are_accepted_and_pop_their_args() {
    let (mut w, _, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "x",
                frames: 1,
            })),
            c,
        )
    });
    assert_eq!(
        w.call(BINK_SET_SOUND_SYSTEM, &[BINK_OPEN_DIRECT_SOUND, 0x1000]),
        (1, 0)
    );
    assert_eq!(w.call(BINK_SET_MEMORY, &[1, 2]), (0, 0));
    assert_eq!(w.call(BINK_SET_MIX_BINS, &[0, 0, 0, 2]), (1, 0));
    assert_eq!(w.call(BINK_SET_MIX_BIN_VOLUMES, &[0, 0, 0, 0, 4]), (1, 0));
}

#[test]
fn mp4_source_without_a_manifest_skips() {
    let dir = std::env::temp_dir().join(format!("bink-hle-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let decoder = Arc::new(SyntheticDecoder {
        info: StreamInfo {
            width: 4,
            height: 2,
            frames: 2,
            fps_num: 30,
            fps_den: 1,
        },
    });
    let src = Mp4Source::new(&dir, decoder);
    assert!(src.open("intro_60").is_none(), "not transcoded: no stream");
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(Mp4Source::new(
                &dir,
                Arc::new(SyntheticDecoder {
                    info: StreamInfo {
                        width: 4,
                        height: 2,
                        frames: 2,
                        fps_num: 30,
                        fps_den: 1,
                    },
                }),
            ))),
            c,
        )
    });
    w.put_str(SCRATCH, "d:\\bink\\intro_60.bik");
    assert_eq!(w.call(BINK_OPEN, &[SCRATCH, 0]).0, 0);
    assert!(!hle.requests()[0].played);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unregister_restores_the_previous_override() {
    let hle = BinkHle::new(None);
    let mut d = Dispatch::default();
    let _ = d.register_override(MOVIE_START, "boot's own", |_, _, _| {
        Ok(abi::TransferAbi::stop(7))
    });
    let reg = register_skip(&mut d, &hle);
    assert_eq!(
        d.override_name(MOVIE_START),
        Some("bink_hle::movie_start_skip")
    );
    unregister(&mut d, reg);
    assert_eq!(d.override_name(MOVIE_START), Some("boot's own"));
}

/// Addresses and offsets written out independently of `addr`, so editing a
/// constant there without re-deriving it from the XBE fails here. The
/// `#[ignore]`d test in `tests/xbe_pins.rs` checks the same numbers against
/// the real image.
#[test]
fn addresses_are_pinned() {
    for (got, want) in [
        (BINK_SET_SOUND_SYSTEM, 0x003e_0450),
        (BINK_OPEN, 0x003e_0af0),
        (BINK_WAIT, 0x003e_2330),
        (BINK_SET_MIX_BINS, 0x003e_2630),
        (BINK_SET_MIX_BIN_VOLUMES, 0x003e_2680),
        (BINK_SET_MEMORY, 0x003e_2820),
        (BINK_COPY_TO_BUFFER, 0x003e_2830),
        (BINK_DO_FRAME, 0x003e_2870),
        (BINK_NEXT_FRAME, 0x003e_2e50),
        (BINK_CLOSE, 0x003e_2ff0),
        (MOVIE_START, 0x0015_6090),
        (MOVIE_SERVICE, 0x0015_5f80),
        (MOVIE_IS_PLAYING, 0x0015_5f60),
        (MOVIE_TICK, 0x0015_6040),
        (ST_ACTIVE, 0x004e_9188),
        (ST_MOVIE_OPEN, 0x004e_9189),
        (ST_FINISHED, 0x004e_918c),
        (ST_FLAGS, 0x004e_9190),
        (ST_BINK, 0x004e_9194),
    ] {
        assert_eq!(got, want);
    }
}

#[test]
fn looping_wraps_to_the_first_frame() {
    let (mut w, _, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 3,
            })),
            c,
        )
    });
    w.put_str(SCRATCH, "intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    let dst = SCRATCH + 0x1000;
    let mut shown = Vec::new();
    for _ in 0..4 {
        w.call(BINK_DO_FRAME, &[h]);
        w.call(BINK_NEXT_FRAME, &[h]);
        w.call(BINK_COPY_TO_BUFFER, &[h, dst, 16, 2, 0, 0, 0x8000_0003]);
        shown.push(w.bytes(dst, 1)[0]);
    }
    assert_eq!(shown, [0, 1, 2, 0]);
    assert_eq!(w.u32(h + 0x0c), 2, "FrameNum wrapped and advanced");
}

#[test]
fn arena_stays_in_the_hle_range_and_leaves_guest_reservations_alone() {
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 3,
            })),
            c,
        )
    });
    // The guest has MEM_RESERVEd 0x40000000 (still unmapped) and 0xa0000000
    // is the reserved HLE interval.
    w.put_str(SCRATCH, "intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    assert_ne!(h, 0);
    let arena = hle.arena().unwrap();
    assert!(HLE_RESERVED.contains(&arena) && HLE_RESERVED.contains(&(arena + 0xfff)));
    assert_eq!(arena, 0xa000_0000);
    // Committing the reservation afterwards still works.
    w.ctx
        .memory()
        .map_zeroed(0x4000_0000, 0x1_0000, Permissions::RW)
        .unwrap();
}

#[test]
fn hint_outside_the_hle_range_is_ignored() {
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_arena_hint(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 3,
            })),
            c,
            0x4000_0000,
        )
    });
    w.put_str(SCRATCH, "intro_60.bik");
    assert_ne!(w.call(BINK_OPEN, &[SCRATCH, 0]).0, 0);
    assert_eq!(hle.arena(), Some(HLE_RESERVED.start));
}

#[test]
fn exhausted_hle_range_fails_open_cleanly() {
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 3,
            })),
            c,
        )
    });
    // Someone else occupies the whole interval (aliasing physical 0).
    w.ctx
        .memory()
        .map(
            HLE_RESERVED.start,
            HLE_RESERVED.end - HLE_RESERVED.start,
            0,
            Permissions::RW,
        )
        .unwrap();
    w.put_str(SCRATCH, "intro_60.bik");
    assert_eq!(w.call(BINK_OPEN, &[SCRATCH, 0]), (0, 0));
    assert_eq!(hle.arena(), None);
    assert!(!hle.requests()[0].played);
}

#[test]
fn arena_never_adopts_existing_memory_inside_the_range() {
    // Sentinel in the first HLE page, owned by someone else.
    let (mut w, hle, _, _) = world(|c| {
        BinkHle::with_clock(
            Some(Arc::new(FakeSource {
                name: "intro_60",
                frames: 3,
            })),
            c,
        )
    });
    w.ctx
        .memory()
        .map_zeroed(HLE_RESERVED.start, 0x1000, Permissions::RW)
        .unwrap();
    w.ctx
        .memory()
        .set_u32(HLE_RESERVED.start, 0xdeca_fbad)
        .unwrap();
    w.put_str(SCRATCH, "intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    assert_ne!(h, 0);
    assert_eq!(hle.arena(), Some(HLE_RESERVED.start + 0x1000));
    assert_eq!(w.u32(HLE_RESERVED.start), 0xdeca_fbad);
}
