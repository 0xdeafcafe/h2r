//! Synthetic guest state: a few mapped pages, no XBE, no movie files.
use bink_hle::{
    BinkHle, Clock, HLE_RESERVED, Mode, MovieOpenError, MovieSource, MovieStream, Mp4Source,
    StreamInfo, SyntheticDecoder, addr::*, register, register_skip, unregister,
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
    // The movie draw's rectangle (`bink_hle::display::MOVIE_RECT`).
    g.map_zeroed(0x0048_5000, 0x1000, Permissions::RW).unwrap();
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
    assert_eq!(hle.prepare("intro_60"), Err(MovieOpenError::NoSource));
    assert_eq!(
        hle.last_request("INTRO_60").unwrap().failure,
        Some(MovieOpenError::NoSource)
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
fn bink_open_takes_the_prepared_stream_and_picture_without_opening_again() {
    struct CountingSource {
        opens: AtomicU64,
        decodes: Arc<AtomicU64>,
    }
    struct CountedMovie(Arc<AtomicU64>);
    impl MovieSource for CountingSource {
        fn open(&self, name: &str) -> Option<Box<dyn MovieStream>> {
            if name != "attract_60" {
                return None;
            }
            self.opens.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(CountedMovie(self.decodes.clone())))
        }
    }
    impl MovieStream for CountedMovie {
        fn info(&self) -> StreamInfo {
            Fake { frames: 3 }.info()
        }
        fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            Fake { frames: 3 }.decode(index, bgra)
        }
    }

    let source = Arc::new(CountingSource {
        opens: AtomicU64::new(0),
        decodes: Arc::new(AtomicU64::new(0)),
    });
    let (mut w, hle, _, _) = world(|c| BinkHle::with_clock(Some(source.clone()), c));
    assert_eq!(hle.prepare("attract_60"), Ok(()));
    assert_eq!(hle.prepare("D:\\Bink\\ATTRACT_60.BIK"), Ok(()));
    assert_eq!(hle.open_streams(), 0);
    assert_eq!(hle.arena(), None);
    assert_eq!(hle.last_request("attract_60"), None);

    w.put_str(SCRATCH, "d:\\bink\\attract_60.bik");
    let handle = w.call(BINK_OPEN, &[SCRATCH, 0]).0;
    assert_ne!(handle, 0);
    let request = hle.last_request("ATTRACT_60").unwrap();
    assert_eq!(request.sequence, 1);
    assert!(request.request.played);
    assert_eq!(request.failure, None);
    assert_eq!(source.opens.load(Ordering::SeqCst), 1);

    let dst = SCRATCH + 0x100;
    let copy = [handle, dst, 16, 2, 0, 0, 3];
    assert_eq!(w.call(BINK_COPY_TO_BUFFER, &copy), (1, 0));
    assert_eq!(w.call(BINK_DO_FRAME, &[handle]), (0, 0));
    assert_eq!(w.call(BINK_COPY_TO_BUFFER, &copy), (0, 0));
    assert_eq!(w.bytes(dst, 4), [0, 0, 0, 0xff]);
    assert_eq!(source.decodes.load(Ordering::SeqCst), 1);

    w.call(BINK_CLOSE, &[handle]);
    assert_eq!(hle.prepare("attract_60"), Ok(()));
    hle.discard_prepared("d:/bink/attract_60.bik");
    assert_ne!(w.call(BINK_OPEN, &[SCRATCH, 0]).0, 0);
    assert_eq!(source.opens.load(Ordering::SeqCst), 3);
    assert_eq!(hle.last_request("attract_60").unwrap().sequence, 2);
}

#[test]
fn an_open_that_cannot_decode_its_first_frame_fails_preparation() {
    struct Unreadable;
    impl MovieSource for Unreadable {
        fn open(&self, _name: &str) -> Option<Box<dyn MovieStream>> {
            Some(Box::new(Self))
        }
    }
    impl MovieStream for Unreadable {
        fn info(&self) -> StreamInfo {
            Fake { frames: 3 }.info()
        }
        fn decode(&mut self, _index: u32, _bgra: &mut [u8]) -> bool {
            false
        }
    }
    let hle = BinkHle::new(Some(Arc::new(Unreadable)));
    assert_eq!(hle.prepare("attract_60"), Err(MovieOpenError::FirstFrame));
    assert_eq!(hle.open_streams(), 0);
    assert!(hle.requests().is_empty());
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
        audio: None,
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
                    audio: None,
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

// --- installed movies, sound, the wrapper's view ------------------------

use audio::Mixer;
use bink_hle::{
    AudioInfo, DiscMovie, MixerAudio, MovieAudioOut, MovieDecoder, MovieVoice, Selection,
    TranscodedSource, select, synthetic_audio_frames, synthetic_sample,
};
use std::sync::Mutex;

const RATE: u32 = 44_100;

fn synthetic(width: u32, height: u32, frames: u32, audio: bool) -> Arc<SyntheticDecoder> {
    Arc::new(SyntheticDecoder {
        info: StreamInfo {
            width,
            height,
            frames,
            fps_num: 30000,
            fps_den: 1001,
        },
        audio: audio.then_some(AudioInfo {
            sample_rate: RATE,
            channels: 2,
        }),
    })
}

/// Any name opens the synthetic stream.
struct Any(Arc<SyntheticDecoder>);
impl MovieSource for Any {
    fn open(&self, _: &str) -> Option<Box<dyn MovieStream>> {
        self.0.open(std::path::Path::new("synthetic"))
    }
}

/// `MixerAudio` with what it queued and played visible to the test.
struct Counting {
    inner: MixerAudio,
    played: Arc<AtomicU64>,
    queued: Arc<AtomicU64>,
}
struct CountingVoice {
    inner: Box<dyn MovieVoice>,
    played: Arc<AtomicU64>,
    queued: Arc<AtomicU64>,
}
impl MovieAudioOut for Counting {
    fn open(&self, info: AudioInfo) -> Option<Box<dyn MovieVoice>> {
        assert_eq!(
            info,
            AudioInfo {
                sample_rate: RATE,
                channels: 2
            }
        );
        self.played.store(0, Ordering::SeqCst);
        self.queued.store(0, Ordering::SeqCst);
        Some(Box::new(CountingVoice {
            inner: self.inner.open(info)?,
            played: Arc::clone(&self.played),
            queued: Arc::clone(&self.queued),
        }))
    }
}
impl MovieVoice for CountingVoice {
    fn queue(&mut self, pcm: &[i16]) -> bool {
        self.queued
            .fetch_add(pcm.len() as u64 / 2, Ordering::SeqCst);
        self.inner.queue(pcm)
    }
    fn played_frames(&mut self) -> u64 {
        let n = self.inner.played_frames();
        self.played.store(n, Ordering::SeqCst);
        n
    }
    fn flush(&mut self) {
        self.inner.flush()
    }
}

/// The game's mixer, rendered by hand as the audio thread would.
struct Sound {
    mixer: Arc<Mutex<Mixer>>,
    played: Arc<AtomicU64>,
    queued: Arc<AtomicU64>,
}
impl Sound {
    fn new() -> (Self, Arc<dyn MovieAudioOut>) {
        let mixer = Arc::new(Mutex::new(Mixer::new(48_000)));
        let played = Arc::new(AtomicU64::new(0));
        let queued = Arc::new(AtomicU64::new(0));
        let out = Arc::new(Counting {
            inner: MixerAudio::new(Arc::clone(&mixer)),
            played: Arc::clone(&played),
            queued: Arc::clone(&queued),
        });
        (
            Self {
                mixer,
                played,
                queued,
            },
            out,
        )
    }
    /// Render `ms` of 48 kHz stereo output.
    fn render(&self, ms: u32) -> Vec<f32> {
        let mut out = vec![0.0; 48 * ms as usize * 2];
        self.mixer.lock().unwrap().render(&mut out);
        out
    }
    fn streams(&self) -> usize {
        self.mixer.lock().unwrap().stream_count()
    }
}

/// What the engine's wrapper does around the Bink API (docs/audio/bink-hle.md
/// "The engine's movie wrapper"): MovieStart opens and blits the first
/// frame; each tick asks BinkWait and, when a frame is due, runs the decode
/// and blit (`0x1568d0`: DoFrame, NextFrame, lock, CopyToBuffer); the
/// service (`0x155f80`) finishes on `FrameNum == Frames` or a skip button,
/// and the stop path closes the handle.
struct Wrapper {
    h: u32,
    dst: u32,
    pitch: u32,
    height: u32,
    finished: bool,
    /// The frame index of each blitted picture (byte 2 of every synthetic
    /// pixel).
    shown: Vec<u8>,
}
impl Wrapper {
    fn start(w: &mut World, path: &str, dst: u32, pitch: u32, height: u32) -> Option<Self> {
        w.put_str(SCRATCH, path);
        let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
        if h == 0 {
            return None;
        }
        let mut me = Self {
            h,
            dst,
            pitch,
            height,
            finished: false,
            shown: Vec::new(),
        };
        me.blit(w);
        Some(me)
    }
    fn blit(&mut self, w: &mut World) {
        w.call(BINK_DO_FRAME, &[self.h]);
        w.call(BINK_NEXT_FRAME, &[self.h]);
        let r = w.call(
            BINK_COPY_TO_BUFFER,
            &[self.h, self.dst, self.pitch, self.height, 0, 0, 0x8000_0003],
        );
        assert_eq!(r, (0, 0), "copy wrote pixels and popped its arguments");
        self.shown.push(w.bytes(self.dst + 2, 1)[0]);
    }
    /// One engine tick; `skip` is the service's button poll.
    fn tick(&mut self, w: &mut World, skip: bool) {
        if self.finished {
            return;
        }
        if w.call(BINK_WAIT, &[self.h]).0 == 0 {
            self.blit(w);
        }
        let (frame, frames) = (w.u32(self.h + 0x0c), w.u32(self.h + 0x08));
        if frame == frames || skip {
            self.finished = true;
            assert_eq!(w.call(BINK_CLOSE, &[self.h]), (0, 0));
        }
    }
}

fn write_manifest(dir: &std::path::Path, movies: &[(&str, u64)]) {
    let mut m = movies::Manifest::default();
    for (name, size) in movies {
        let out = format!("{name} synthetic output");
        std::fs::write(dir.join(format!("{name}.mov")), &out).unwrap();
        m.upsert(movies::MovieEntry {
            source_name: format!("/bink/{name}.bik"),
            source_size: *size,
            source_sha256: "ab".repeat(32),
            output_name: format!("{name}.mov"),
            output_size: out.len() as u64,
            duration_secs: 1.0,
            ffmpeg_version: "ffmpeg version test".into(),
            settings: "test".into(),
        });
        let mut sha = movies::Sha256::default();
        sha.update(out.as_bytes());
        m.output_sha256.insert(format!("{name}.mov"), movies::hex(&sha.finish()));
    }
    std::fs::write(dir.join(movies::MANIFEST_NAME), m.to_json()).unwrap();
}

#[test]
fn mode_is_picked_from_what_is_installed() {
    let dir = std::env::temp_dir().join(format!("bink-hle-select-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let disc: Vec<DiscMovie> = [("attract_60", 10), ("credits_60", 20), ("intro_60", 30)]
        .iter()
        .map(|(n, s)| DiscMovie {
            path: format!("/bink/{n}.bik"),
            size: *s,
        })
        .collect();
    let decoder = synthetic(4, 2, 3, false);
    let mode = |sel: &Selection| {
        let (_, _, _, mode) = world(|c| BinkHle::with_clock(sel.source(decoder.clone()), c));
        mode
    };

    // Off: no directory at all.
    let off = select(None, &disc, true);
    assert_eq!(mode(&off), Mode::Skip);
    assert!(off.to_string().starts_with("skip (off"), "{off}");

    // None installed.
    let none = select(Some(&dir), &disc, true);
    assert!(matches!(none, Selection::NoneInstalled(_)));
    assert_eq!(mode(&none), Mode::Skip);
    assert!(
        none.to_string().starts_with("skip (none installed"),
        "{none}"
    );

    // Current: all three.
    write_manifest(
        &dir,
        &[("attract_60", 10), ("credits_60", 20), ("intro_60", 30)],
    );
    let current = select(Some(&dir), &disc, true);
    assert!(current.is_playback());
    assert!(
        current.to_string().starts_with("playback (3 transcoded"),
        "{current}"
    );
    let (mut w, hle, _, playing) =
        world(|c| BinkHle::with_clock(current.source(decoder.clone()), c));
    assert_eq!(playing, Mode::Playback);
    w.put_str(SCRATCH, "d:\\bink\\intro_60.bik");
    assert_ne!(
        w.call(BINK_OPEN, &[SCRATCH, 0]).0,
        0,
        "a current movie opens"
    );
    assert!(hle.requests()[0].played);
    // No decoder on this platform: skip even with movies installed.
    let nodec = select(Some(&dir), &disc, false);
    assert_eq!(nodec, Selection::NoDecoder);
    assert_eq!(mode(&nodec), Mode::Skip);

    // Stale: transcoded from another disc (every size differs).
    write_manifest(
        &dir,
        &[("attract_60", 11), ("credits_60", 21), ("intro_60", 31)],
    );
    let stale = select(Some(&dir), &disc, true);
    assert!(matches!(stale, Selection::Stale(_)));
    assert_eq!(mode(&stale), Mode::Skip);
    assert!(stale.to_string().starts_with("skip (stale"), "{stale}");

    // Partly stale: only the current movie is offered; the stale one is a
    // clean NULL from BinkOpen (the wrapper's failed-open path).
    write_manifest(&dir, &[("attract_60", 11), ("intro_60", 30)]);
    let partial = select(Some(&dir), &disc, true);
    assert_eq!(
        partial,
        Selection::Playback {
            dir: dir.clone(),
            names: vec!["intro_60".into()]
        }
    );
    let (mut w, hle, _, _) = world(|c| BinkHle::with_clock(partial.source(decoder.clone()), c));
    assert_eq!(hle.prepare("attract_60"), Err(MovieOpenError::Unavailable));
    w.put_str(SCRATCH, "d:\\bink\\attract_60.bik");
    assert_eq!(w.call(BINK_OPEN, &[SCRATCH, 0]), (0, 0));
    assert_eq!(
        hle.last_request("attract_60").unwrap().failure,
        Some(MovieOpenError::Unavailable)
    );
    w.put_str(SCRATCH, "d:\\bink\\intro_60.bik");
    assert_ne!(w.call(BINK_OPEN, &[SCRATCH, 0]).0, 0);
    // The source alone honours the same list.
    let only = TranscodedSource::new(&dir, decoder.clone()).only(["INTRO_60"]);
    assert!(only.open("intro_60").is_some());
    assert!(only.open("attract_60").is_none());
    std::fs::remove_file(dir.join("intro_60.mov")).unwrap();
    assert_eq!(
        hle.prepare("intro_60"),
        Err(MovieOpenError::MissingOrChangedOutput)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn frames_reach_the_locked_texture_and_mark_its_pages_written() {
    // The wrapper locks a linear 32-bit texture and hands its address and
    // pitch to CopyToBuffer. D3D learns about the new texels from the
    // runtime's write watch, as for any guest store.
    const PHYS: u32 = 0x03f0_0000;
    const TEX: u32 = 0x8000_0000 | PHYS;
    let (w_, h_, pitch) = (16u32, 8u32, 16 * 4 + 32); // padded rows
    let (mut w, _, _, _) =
        world(|c| BinkHle::with_clock(Some(Arc::new(Any(synthetic(w_, h_, 5, false)))), c));
    let id = {
        let mut m = w.ctx.memory();
        m.map(TEX, 0x2000, PHYS, Permissions::RW).unwrap();
        let id = m.subscribe_writes("test d3d");
        m.watch_writes(id, TEX, 0x2000).unwrap();
        assert!(m.take_dirty(id).unwrap().is_empty());
        id
    };
    let mut wrap = Wrapper::start(&mut w, "d:\\bink\\intro_60.bik", TEX, pitch, h_).unwrap();
    let dirty = w.ctx.memory().take_dirty(id).unwrap();
    assert_eq!(dirty.len(), 1, "{dirty:x?}");
    assert_eq!((dirty[0].addr, dirty[0].len), (TEX, 0x1000));
    // Every row of frame 0, at the texture's pitch; the padding untouched.
    for y in 0..h_ {
        let row = w.bytes(TEX + y * pitch, pitch as usize);
        for x in 0..w_ as usize {
            assert_eq!(
                &row[x * 4..x * 4 + 4],
                &[x as u8, y as u8, 0, 0xff],
                "({x},{y})"
            );
        }
        assert!(
            row[w_ as usize * 4..].iter().all(|b| *b == 0),
            "row {y} padding"
        );
    }
    // The next due frame replaces it and dirties the page again.
    w.clock.0.fetch_add(34, Ordering::SeqCst);
    wrap.tick(&mut w, false);
    assert_eq!(wrap.shown, [0, 1]);
    assert_eq!(w.bytes(TEX, 4), [4, 0, 1, 0xff]);
    assert_eq!(w.ctx.memory().take_dirty(id).unwrap().len(), 1);
}

#[test]
fn the_wrapper_sees_the_end_of_the_movie_and_closes_cleanly() {
    let (sound, out) = Sound::new();
    let frames = 12;
    let (mut w, hle, _, _) = world(|c| {
        let hle = BinkHle::with_clock(Some(Arc::new(Any(synthetic(4, 2, frames, true)))), c);
        hle.set_audio_out(out);
        hle
    });
    let dst = SCRATCH + 0x4000;
    let mut wrap = Wrapper::start(&mut w, "d:\\bink\\credits_60.bik", dst, 16, 2).unwrap();
    assert_eq!(
        sound.streams(),
        1,
        "the movie's voice is in the game's mixer"
    );
    // 30 ms ticks with 30 ms of sound rendered between them, as a running
    // game would; the wall clock moves too.
    let mut ticks = 0;
    while !wrap.finished {
        sound.render(30);
        w.clock.0.fetch_add(30, Ordering::SeqCst);
        wrap.tick(&mut w, false);
        ticks += 1;
        assert!(ticks < 100, "never finished: shown {:?}", wrap.shown);
    }
    // Every frame once, in order, until FrameNum == Frames ended it. The
    // wrapper blits (DoFrame, NextFrame, Copy) before its service checks
    // `FrameNum == Frames` (0x15601c), so it stops after the picture before
    // the last one, as with the original player.
    assert_eq!(wrap.shown, (0..frames as u8 - 1).collect::<Vec<_>>());
    // 11 pictures at 29.97 fps is ~370 ms: about 12 ticks of 30 ms.
    assert!((11..=15).contains(&ticks), "{ticks} ticks");
    assert_eq!(hle.open_streams(), 0);
    assert_eq!(sound.streams(), 0, "closing removed the voice");
    assert_eq!(w.u32(wrap.h + 0x08), 0, "HBINK cleared");
    // The next movie starts from a clean slate.
    let again = Wrapper::start(&mut w, "d:\\bink\\attract_60.bik", dst, 16, 2).unwrap();
    assert_eq!(again.shown, [0]);
    assert_eq!(w.u32(again.h + 0x0c), 2);
    assert_eq!(sound.streams(), 1);
}

#[test]
fn a_skip_mid_movie_closes_cleanly() {
    let (sound, out) = Sound::new();
    let (mut w, hle, _, _) = world(|c| {
        let hle = BinkHle::with_clock(Some(Arc::new(Any(synthetic(4, 2, 300, true)))), c);
        hle.set_audio_out(out);
        hle
    });
    let dst = SCRATCH + 0x4000;
    let mut wrap = Wrapper::start(&mut w, "d:\\bink\\intro_60.bik", dst, 16, 2).unwrap();
    for _ in 0..10 {
        sound.render(33);
        w.clock.0.fetch_add(33, Ordering::SeqCst);
        wrap.tick(&mut w, false);
    }
    assert!(wrap.shown.len() >= 9, "{:?}", wrap.shown);
    assert!(sound.played.load(Ordering::SeqCst) > 0);
    // A button: the service stops the movie at once.
    assert!(w.u32(wrap.h + 0x0c) < 300);
    wrap.tick(&mut w, true);
    assert!(wrap.finished);
    assert_eq!(hle.open_streams(), 0);
    assert_eq!(sound.streams(), 0, "the voice is gone, nothing left queued");
    assert!(
        sound.render(50).iter().all(|s| *s == 0.0),
        "silence after the skip"
    );
    // Calls a late tick could still make on the old handle do nothing.
    let h = wrap.h;
    assert_eq!(w.call(BINK_WAIT, &[h]), (0, 0));
    assert_eq!(w.call(BINK_DO_FRAME, &[h]), (0, 0));
    assert_eq!(w.call(BINK_NEXT_FRAME, &[h]), (0, 0));
    assert_eq!(w.call(BINK_COPY_TO_BUFFER, &[h, dst, 16, 2, 0, 0, 3]).0, 1);
    assert_eq!(w.call(BINK_CLOSE, &[h]), (0, 0));
    // The handle is reused for the next movie, from its first frame and
    // the start of its sound.
    let next = Wrapper::start(&mut w, "d:\\bink\\attract_60.bik", dst, 16, 2).unwrap();
    assert_eq!(next.h, h);
    assert_eq!(next.shown, [0]);
    assert_eq!(sound.played.load(Ordering::SeqCst), 0);
}

#[test]
fn sound_plays_at_the_movie_rate_through_the_mixer() {
    let (sound, out) = Sound::new();
    let movie = synthetic(4, 2, 150, true); // ~5 s
    let (mut w, _, _, _) = world(|c| {
        let hle = BinkHle::with_clock(Some(Arc::new(Any(movie.clone()))), c);
        hle.set_audio_out(out);
        hle
    });
    w.put_str(SCRATCH, "intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    // Opening queues sound ahead, starting at the first sample.
    let primed = sound.queued.load(Ordering::SeqCst);
    assert!(
        (RATE as u64 * 4 / 10..=RATE as u64 * 6 / 10).contains(&primed),
        "{primed} frames queued"
    );
    // One second of output in the audio thread's 5 ms periods, with the
    // game's decode (which feeds the sound) every 10 ms and the wall clock
    // moving with it.
    let mut out = Vec::new();
    for i in 0..200 {
        out.extend(sound.render(5));
        if i % 2 == 1 {
            w.clock.0.fetch_add(10, Ordering::SeqCst);
            w.call(BINK_DO_FRAME, &[h]);
        }
    }
    let played = sound.played.load(Ordering::SeqCst);
    // 1 s at 44.1 kHz, less at most the 10 ms packet still in flight.
    assert!(
        (RATE as u64 - 441..=RATE as u64).contains(&played),
        "{played} frames played in 1 s"
    );
    // It is the movie's track, not silence (resampled to 48 kHz), and it
    // never ran dry.
    let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1, "peak {peak}");
    assert_eq!(sound.mixer.lock().unwrap().stream_health().starves, 0);
    assert_ne!(synthetic_sample(0, 1), synthetic_sample(0, 0));
    // The video is at the same second: frame 30 (967 ms) is due, frame 32
    // (1034 ms) is not.
    let at = |w: &mut World, n: u32| {
        w.ctx.memory().set_u32(h + 0x0c, n).unwrap();
        w.call(BINK_WAIT, &[h]).0
    };
    assert_eq!(at(&mut w, 30), 0);
    assert_eq!(at(&mut w, 32), 1);
    // NextFrame from frame 2 jumps to the frame now due (dropping frames).
    w.ctx.memory().set_u32(h + 0x0c, 2).unwrap();
    w.call(BINK_NEXT_FRAME, &[h]);
    assert_eq!(w.u32(h + 0x0c), 30);

    // The whole track is the video's length.
    let a = AudioInfo {
        sample_rate: RATE,
        channels: 2,
    };
    assert_eq!(synthetic_audio_frames(&movie.info, &a), 220_721);
}

/// The wrapper's service spins on BinkWait until it returns 0
/// (`0x155fc2..0x155fd0`). That spin must end on the wall clock even when
/// the sound does not move: the mixer runs on logical time, which a spin
/// charges only a few ns per call. (Slaving BinkWait to the sound made each
/// frame cost millions of calls: the 0.3 fps of the first playback build.)
#[test]
fn a_spin_on_bink_wait_ends_on_the_wall_clock_with_the_sound_frozen() {
    let (sound, out) = Sound::new();
    let (mut w, _, _, _) = world(|c| {
        let hle = BinkHle::with_clock(Some(Arc::new(Any(synthetic(4, 2, 300, true)))), c);
        hle.set_audio_out(out);
        hle
    });
    let dst = SCRATCH + 0x4000;
    let mut wrap = Wrapper::start(&mut w, "d:\\bink\\intro_60.bik", dst, 16, 2).unwrap();
    for frame in 0..5 {
        // 1 ms of wall time per call, and the mixer never renders.
        let mut calls = 0;
        while w.call(BINK_WAIT, &[wrap.h]).0 != 0 {
            w.clock.0.fetch_add(1, Ordering::SeqCst);
            calls += 1;
            assert!(calls < 40, "frame {frame}: still waiting after {calls} ms");
        }
        wrap.blit(&mut w);
    }
    assert_eq!(wrap.shown, [0, 1, 2, 3, 4, 5]);
    assert_eq!(sound.played.load(Ordering::SeqCst), 0);
}

/// What one BinkWait costs through the runner (release numbers in the
/// docs). The service spin calls it back to back.
#[test]
fn bink_wait_call_cost() {
    let (mut w, _, _, _) =
        world(|c| BinkHle::with_clock(Some(Arc::new(Any(synthetic(4, 2, 300, false)))), c));
    w.put_str(SCRATCH, "intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    w.call(BINK_DO_FRAME, &[h]);
    w.call(BINK_NEXT_FRAME, &[h]);
    let n = 100_000u32;
    let t = std::time::Instant::now();
    for _ in 0..n {
        assert_eq!(w.call(BINK_WAIT, &[h]).0, 1);
    }
    eprintln!("BinkWait: {:?}/call", t.elapsed() / n);
}

/// Per-frame host cost of the decode and blit (DoFrame, NextFrame,
/// CopyToBuffer into a contiguous-window texture with the write watch on),
/// at the movies' size and at 720p. Prints the numbers; the bound is loose
/// enough for a debug build and still catches a per-byte path.
#[test]
fn decode_and_blit_cost_per_frame() {
    for (w_, h_) in [(640u32, 480u32), (1280, 720)] {
        const PHYS: u32 = 0x0300_0000;
        const TEX: u32 = 0x8000_0000 | PHYS;
        let pitch = w_ * 4;
        let frames = 30;
        let (mut w, _, _, _) = world(|c| {
            BinkHle::with_clock(Some(Arc::new(Any(synthetic(w_, h_, frames, false)))), c)
        });
        let len = (pitch * h_).next_multiple_of(0x1000);
        let id = {
            let mut m = w.ctx.memory();
            m.map(TEX, len, PHYS, Permissions::RW).unwrap();
            let id = m.subscribe_writes("test d3d");
            m.watch_writes(id, TEX, len).unwrap();
            id
        };
        w.put_str(SCRATCH, "intro_60.bik");
        let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
        let (mut decode, mut copy) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for _ in 0..frames - 1 {
            let t = std::time::Instant::now();
            w.call(BINK_DO_FRAME, &[h]);
            w.call(BINK_NEXT_FRAME, &[h]);
            decode += t.elapsed();
            let t = std::time::Instant::now();
            let r = w.call(BINK_COPY_TO_BUFFER, &[h, TEX, pitch, h_, 0, 0, 0x8000_0003]);
            copy += t.elapsed();
            assert_eq!(r.0, 0);
            assert_eq!(
                w.ctx
                    .memory()
                    .take_dirty(id)
                    .unwrap()
                    .iter()
                    .map(|r| r.len)
                    .sum::<u32>(),
                len
            );
        }
        let n = frames - 1;
        let (d, c) = (decode / n, copy / n);
        eprintln!("{w_}x{h_}: synthetic decode+next {d:?}/frame, copy {c:?}/frame");
        assert!(
            c < std::time::Duration::from_millis(20),
            "{w_}x{h_} copy {c:?}"
        );
    }
}

/// The native display: the tick's decode and blit never lock or copy into
/// the guest's texture, and its draw hands the host the guest's quad and
/// the decoded picture (docs/graphics/retire-d3d-display.md, "Movies").
#[test]
fn a_native_display_takes_the_picture_and_leaves_the_guest_texture_alone() {
    use bink_hle::{MovieDisplay, MovieDraw, display::MOVIE_RECORD, set_display};
    use std::sync::Mutex;

    struct Host(Mutex<Vec<MovieDraw>>);
    impl MovieDisplay for Host {
        fn show(&self, _mem: &mut Guest, draw: &MovieDraw) -> bool {
            self.0.lock().unwrap().push(draw.clone());
            true
        }
    }
    let (mut w, _hle, reg, mode) = world(|c| {
        BinkHle::with_clock(Some(Arc::new(FakeSource { name: "intro_60", frames: 3 })), c)
    });
    assert_eq!(mode, Mode::Playback);
    let addrs: Vec<u32> = reg.iter().map(|r| r.addr).collect();
    assert!(addrs.contains(&MOVIE_DECODE_AND_BLIT) && addrs.contains(&MOVIE_DRAW));
    let host = Arc::new(Host(Mutex::new(Vec::new())));
    set_display(Some(host.clone()));

    w.put_str(SCRATCH, "d:\\bink\\intro_60.bik");
    let (h, _) = w.call(BINK_OPEN, &[SCRATCH, 0]);
    assert_ne!(h, 0);
    {
        let mut m = w.ctx.memory();
        m.set_u32(ST_BINK, h).unwrap();
        m.set_u8(ST_FLAGS, 0xc6).unwrap();
        m.set_u16(ST_WIDTH, 4).unwrap();
        m.set_u16(ST_HEIGHT, 2).unwrap();
        // A texture pointer the native path must never write through.
        m.set_u32(ST_TEXTURE, SCRATCH + 0x100).unwrap();
        for (i, v) in [0i16, 0, 480, 640].into_iter().enumerate() {
            m.set_u16(0x0048_5a8a + 2 * i as u32, v as u16).unwrap();
        }
    }
    let before = w.bytes(SCRATCH + 0x100, 64);
    assert_eq!(w.call(MOVIE_DECODE_AND_BLIT, &[]), (0, 0), "no arguments; EAX as after a copy");
    assert_eq!(w.u32(h + 0x0c), 2, "NextFrame ran after DoFrame");
    assert_eq!(w.call(MOVIE_DRAW, &[]), (0, 0));
    assert_eq!(w.bytes(SCRATCH + 0x100, 64), before, "the guest's texture is untouched");
    let draws = host.0.lock().unwrap().clone();
    assert_eq!(draws.len(), 1);
    let d = &draws[0];
    assert_eq!(d.record, MOVIE_RECORD);
    assert_eq!((d.picture.movie, d.picture.frame, d.picture.width, d.picture.height), (h, 0, 4, 2));
    assert_eq!(&d.picture.bgra[..8], &[0, 0, 0, 0xff, 0, 1, 0, 0xff], "frame 0's texels as decoded");
    assert_eq!(d.quad.corners[0], [318.0, 239.0, 0.0, 0.0], "centred at its own size");
    assert_eq!(d.quad.corners[2], [322.0, 241.0, 4.0, 2.0]);
    // The next frame's picture is a new one; the one shown stays as it was.
    w.call(MOVIE_DECODE_AND_BLIT, &[]);
    w.call(MOVIE_DRAW, &[]);
    let draws = host.0.lock().unwrap().clone();
    assert_eq!((draws[1].picture.frame, draws[0].picture.bgra[0], draws[1].picture.bgra[0]), (1, 0, 1));
    set_display(None);
}
