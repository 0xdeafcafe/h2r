use runtime::guest::{PAGE_SIZE, Permissions};
use runtime::watch::CONTIGUOUS_BASE;
use runtime::{DirtyRange, Dispatch, ExecCtx, ExecCtxOpaque, Guest, Runtime, WatchError, ffi};

const VA: u32 = 0x0010_0000;
const ALIAS: u32 = 0x0020_0000;
const PA: u32 = 0x0004_0000;
const LEN: u32 = 8 * PAGE_SIZE;

fn range(pa: u32, pages: u32) -> DirtyRange {
    DirtyRange {
        addr: CONTIGUOUS_BASE | pa,
        len: pages * PAGE_SIZE,
    }
}

/// `VA` and `ALIAS` both map physical `PA` for `LEN` bytes.
fn aliased() -> Guest {
    let mut g = Guest::new();
    g.map(VA, LEN, PA, Permissions::RW).unwrap();
    g.map(ALIAS, LEN, PA, Permissions::RW).unwrap();
    g
}

#[test]
fn writes_through_any_alias_report_the_folded_physical_page() {
    let mut g = aliased();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE | PA, LEN).unwrap();
    assert!(!g.has_dirty(id));
    g.set_u32(VA + 0x10, 1).unwrap();
    g.set_u8(ALIAS + 3 * PAGE_SIZE + 7, 2).unwrap();
    assert!(g.has_dirty(id));
    assert_eq!(
        g.take_dirty(id).unwrap(),
        vec![range(PA, 1), range(PA + 3 * PAGE_SIZE, 1)]
    );
    assert!(!g.has_dirty(id));
    assert_eq!(g.take_dirty(id).unwrap(), vec![]);
    assert_eq!(g.fold_address(ALIAS + 5), Some(CONTIGUOUS_BASE | (PA + 5)));
    assert_eq!(g.fold_address(0x0030_0000), None);
}

#[test]
fn only_watched_pages_are_reported_and_runs_coalesce() {
    let mut g = aliased();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE | (PA + PAGE_SIZE), 3 * PAGE_SIZE)
        .unwrap();
    // Page 0 is not watched. One store straddles pages 1 and 2, one hits 3.
    g.write_bytes(VA, &[1; 16], 1).unwrap();
    g.write_bytes(VA + 2 * PAGE_SIZE - 2, &[1; 4], 1).unwrap();
    g.set_u32(VA + 3 * PAGE_SIZE, 1).unwrap();
    g.set_u32(VA + 6 * PAGE_SIZE, 1).unwrap();
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(PA + PAGE_SIZE, 3)]);
    // A store spanning a whole run reports every page once.
    g.write_bytes(VA, &vec![0; LEN as usize], 1).unwrap();
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(PA + PAGE_SIZE, 3)]);
}

#[test]
fn faulting_writes_report_nothing() {
    let mut g = Guest::new();
    g.map(VA, PAGE_SIZE, PA, Permissions::RW).unwrap();
    g.map(VA + PAGE_SIZE, PAGE_SIZE, PA + PAGE_SIZE, Permissions::READ)
        .unwrap();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE | PA, 2 * PAGE_SIZE)
        .unwrap();
    assert!(g.write_bytes(VA + PAGE_SIZE - 2, &[1; 4], 1).is_err());
    assert!(!g.has_dirty(id));
}

#[test]
fn atomics_and_the_generated_store_import_report() {
    let mut g = aliased();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE | PA, LEN).unwrap();
    g.atomic_xchg(ALIAS + 4, 4, 9).unwrap();
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(PA, 1)]);

    let runtime = Runtime::new(g, Dispatch::default());
    let mut ctx = ExecCtx::new(runtime.clone(), 1);
    let value = 0x1234_5678u32.to_le_bytes();
    // SAFETY: ctx is exclusively borrowed for the call and value is readable.
    let status = unsafe {
        ffi::h2abi_v1_write(
            (&mut ctx as *mut ExecCtx).cast::<ExecCtxOpaque>(),
            0x1000,
            VA + 5 * PAGE_SIZE,
            4,
            1,
            value.as_ptr(),
        )
    };
    assert_eq!(status, 0);
    assert_eq!(
        runtime.memory().take_dirty(id).unwrap(),
        vec![range(PA + 5 * PAGE_SIZE, 1)]
    );
}

#[test]
fn runtime_zero_fills_report() {
    let mut g = Guest::new();
    let id = g.subscribe_writes("test");
    let pa = 0x0100_0000;
    g.watch_writes(id, CONTIGUOUS_BASE | pa, 2 * PAGE_SIZE)
        .unwrap();
    g.reserve_physical(pa, 2 * PAGE_SIZE, &[0x0300_0000], "test")
        .unwrap();
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(pa, 2)]);
}

#[test]
fn reusing_a_freed_physical_page_reports_its_zeroing() {
    let mut g = Guest::new();
    g.map(VA, PAGE_SIZE, 0, Permissions::RW).unwrap();
    g.set_u32(VA, 0xdead_beef).unwrap();
    g.unmap(VA, PAGE_SIZE).unwrap();
    // Use the last page, so map_zeroed finds no fresh RAM and reuses page 0.
    let top = runtime::guest::RAM_SIZE as u32 - PAGE_SIZE;
    g.map(ALIAS, PAGE_SIZE, top, Permissions::RW).unwrap();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE, PAGE_SIZE).unwrap();
    g.map_zeroed(VA, PAGE_SIZE, Permissions::RW).unwrap();
    assert_eq!(g.fold_address(VA), Some(CONTIGUOUS_BASE));
    assert_eq!(g.u32(VA).unwrap(), 0);
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(0, 1)]);
}

#[test]
fn subscribers_are_independent_and_muting_hides_only_the_muted_one() {
    let mut g = aliased();
    let d3d = g.subscribe_writes("d3d");
    let other = g.subscribe_writes("other");
    g.watch_writes(d3d, CONTIGUOUS_BASE | PA, LEN).unwrap();
    g.watch_writes(other, CONTIGUOUS_BASE | PA, 2 * PAGE_SIZE)
        .unwrap();
    let out = g
        .writes_unreported(d3d, |g| {
            g.set_u32(VA, 1).unwrap();
            7
        })
        .unwrap();
    assert_eq!(out, 7);
    assert!(!g.has_dirty(d3d));
    assert_eq!(g.take_dirty(other).unwrap(), vec![range(PA, 1)]);
    // Muting ends with the closure.
    g.set_u32(VA + PAGE_SIZE, 1).unwrap();
    assert_eq!(g.take_dirty(d3d).unwrap(), vec![range(PA + PAGE_SIZE, 1)]);
    assert_eq!(g.take_dirty(other).unwrap(), vec![range(PA + PAGE_SIZE, 1)]);
    assert_eq!(g.write_watch_owners(), vec![(d3d, "d3d"), (other, "other")]);
}

#[test]
fn muting_survives_a_panic() {
    let mut g = aliased();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE | PA, LEN).unwrap();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = g.writes_unreported(id, |_| panic!("boom"));
    }));
    assert!(caught.is_err());
    g.set_u32(VA, 1).unwrap();
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(PA, 1)]);
}

#[test]
fn unwatch_discards_pending_bits_and_unsubscribe_forgets() {
    let mut g = aliased();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE | PA, LEN).unwrap();
    g.set_u32(VA, 1).unwrap();
    g.set_u32(VA + PAGE_SIZE, 1).unwrap();
    g.unwatch_writes(id, CONTIGUOUS_BASE | PA, 1).unwrap();
    assert_eq!(g.take_dirty(id).unwrap(), vec![range(PA + PAGE_SIZE, 1)]);
    g.set_u32(VA, 1).unwrap();
    assert!(!g.has_dirty(id));
    assert!(g.unsubscribe_writes(id));
    assert!(!g.unsubscribe_writes(id));
    assert_eq!(g.take_dirty(id), Err(WatchError::UnknownWatch));
    assert_eq!(
        g.watch_writes(id, CONTIGUOUS_BASE, 1),
        Err(WatchError::UnknownWatch)
    );
    let fresh = g.subscribe_writes("again");
    assert_ne!(fresh, id);
    assert_eq!(g.watch_writes(fresh, PA, 1), Err(WatchError::InvalidRange));
    assert_eq!(
        g.watch_writes(fresh, 0x83ff_f000, PAGE_SIZE + 1),
        Err(WatchError::InvalidRange)
    );
}

/// `cargo test -p runtime --release --test write_watch -- --ignored --nocapture`
#[test]
#[ignore]
fn write_barrier_overhead() {
    const N: u32 = 20_000_000;
    let run = |label: &str, setup: &dyn Fn(&mut Guest)| {
        let mut g = Guest::new();
        // A realistic map: many mappings, so the lookup is a real search.
        for i in 0..64 {
            g.map_zeroed(0x0010_0000 + i * 0x10_0000, 0x1_0000, Permissions::RW)
                .unwrap();
        }
        setup(&mut g);
        let pass = |g: &mut Guest| {
            let start = std::time::Instant::now();
            for i in 0..N {
                let addr = 0x0010_0000 + (i & 63) * 0x10_0000 + ((i * 4) & 0xfffc);
                g.set_u32(std::hint::black_box(addr), i).unwrap();
            }
            start.elapsed().as_nanos() as f64 / N as f64
        };
        pass(&mut g); // first touch of the host pages
        let best = (0..5).map(|_| pass(&mut g)).fold(f64::INFINITY, f64::min);
        println!("{label:<40} {best:.2} ns/store (best of 5)");
    };
    run("no subscriber", &|_| {});
    run("subscriber, other pages watched", &|g| {
        let id = g.subscribe_writes("bench");
        g.watch_writes(id, CONTIGUOUS_BASE | 0x0300_0000, 0x10_0000)
            .unwrap();
    });
    run("subscriber, every store watched", &|g| {
        let id = g.subscribe_writes("bench");
        g.watch_writes(id, CONTIGUOUS_BASE, 0x0400_0000).unwrap();
    });
}

/// What a D3D entry pays: drain (nothing dirty, then one dirty page) and
/// run its body muted.
#[test]
#[ignore]
fn override_entry_overhead() {
    const N: u32 = 2_000_000;
    let mut g = aliased();
    let id = g.subscribe_writes("bench");
    g.watch_writes(id, CONTIGUOUS_BASE, 0x0400_0000).unwrap();
    for dirty in [false, true] {
        let start = std::time::Instant::now();
        for i in 0..N {
            if dirty {
                g.set_u32(VA + (i & 7) * PAGE_SIZE, i).unwrap();
            }
            let ranges = g.take_dirty(id).unwrap();
            std::hint::black_box(ranges);
            g.writes_unreported(id, |g| g.set_u32(VA, i).unwrap())
                .unwrap();
        }
        let ns = start.elapsed().as_nanos() as f64 / N as f64;
        println!("entry, dirty page pending: {dirty:<5}  {ns:.1} ns/entry");
    }
}

/// Dirty pages come back as the same runs a page-by-page model gives, for
/// random sets of written pages: single pages, runs inside one bitmap word,
/// runs across words, whole words and the last page of RAM.
#[test]
fn take_dirty_runs_match_a_page_by_page_model() {
    let pages = runtime::guest::RAM_SIZE as u32 / PAGE_SIZE;
    let mut g = Guest::new();
    g.map(0x1000_0000, pages * PAGE_SIZE, 0, Permissions::RW)
        .unwrap();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE, pages * PAGE_SIZE)
        .unwrap();
    let mut seed = 0x9E37_79B9u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for round in 0..300 {
        let mut written = std::collections::BTreeSet::new();
        for _ in 0..next() % 40 {
            let first = match next() % 4 {
                0 => pages - 1 - next() % 3,
                1 => (next() % (pages / 64)) * 64 + 60,
                _ => next() % pages,
            };
            let n = 1 + next() % 130;
            for p in first..(first + n).min(pages) {
                g.set_u8(0x1000_0000 + p * PAGE_SIZE + next() % PAGE_SIZE, 1)
                    .unwrap();
                written.insert(p);
            }
        }
        let mut want = Vec::new();
        let mut it = written.iter().copied().peekable();
        while let Some(start) = it.next() {
            let mut end = start + 1;
            while it.peek() == Some(&end) {
                it.next();
                end += 1;
            }
            want.push(range(start * PAGE_SIZE, end - start));
        }
        assert_eq!(g.take_dirty(id).unwrap(), want, "round {round}");
    }
}
