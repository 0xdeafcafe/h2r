//! In-process checks. Every test attaches only its own thread (thread-level
//! ports), so tests can run in parallel inside the harness. Anything that
//! must crash, or that changes task-wide ports, lives in `process.rs`.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use fastmem_proto::ops::*;
use fastmem_proto::sys::page_size;
use fastmem_proto::{Error, Mapping, WatchServer, bench};

fn armed(pages: usize) -> (WatchServer, Mapping, fastmem_proto::RegionId) {
    let server = WatchServer::start().unwrap();
    let map = Mapping::new(pages).unwrap();
    let id = server.register(map.base(), map.len()).unwrap();
    server.arm(id).unwrap();
    (server, map, id)
}

#[test]
fn host_pages_are_16k() {
    assert_eq!(page_size(), 16 * 1024);
}

#[test]
fn first_write_marks_dirty_and_retries_each_store_kind() {
    let (server, map, id) = armed(10);
    let _a = server.attach_current_thread().unwrap();
    // One store kind per page; each must fault exactly once, then land.
    unsafe {
        str64(map.page(0).add(64), 0xdead_beef_0000_0001);
        str8(map.page(1).add(3), 0x5a);
        stp(map.page(2).add(32), 7, 9);
        str_q(map.page(3).add(16), 0xa5);
        map.page(4).add(8).cast::<u64>().write(41);
        assert_eq!(server.stats().handled, 5);
        // Atomics fault too (LSE ldadd, and the exclusive pair).
        let old = ldadd(map.page(5).add(8), 3);
        assert_eq!(old, 0);
        exclusive_inc(map.page(6).add(16));
        // DC ZVA is a store for permission purposes.
        map.page(7).add(64).write_bytes(0xff, 64);
        server.rearm_dirty(id, |_| ()).unwrap();
        dc_zva(map.page(7).add(64));
        // A whole-page memset (libc may use DC ZVA or STP Q runs).
        map.page(8).write_bytes(0, page_size());

        assert_eq!(load64(map.page(0).add(64)), 0xdead_beef_0000_0001);
        assert_eq!(*map.page(1).add(3), 0x5a);
        assert_eq!(load64(map.page(2).add(32)), 7);
        assert_eq!(load64(map.page(2).add(40)), 9);
        assert_eq!(*map.page(3).add(31), 0xa5);
        assert_eq!(load64(map.page(4).add(8)), 41);
        assert_eq!(load64(map.page(5).add(8)), 3);
        assert_eq!(load64(map.page(6).add(16)), 1);
        assert_eq!(load64(map.page(7).add(64)), 0);
    }
    // Page 7 faulted twice (write_bytes, then dc zva after the re-arm).
    let s = server.stats();
    assert_eq!(s.handled, 10, "{s:?}");
    assert_eq!(s.forwarded, 0, "{s:?}");
    // Pages 0..=6 were drained by the re-arm; 7 and 8 are dirty now.
    assert_eq!(server.dirty_pages(id).unwrap(), vec![7, 8]);
}

#[test]
fn later_writes_do_not_fault_until_rearmed() {
    let (server, map, id) = armed(4);
    let _a = server.attach_current_thread().unwrap();
    for i in 0..100u64 {
        unsafe { str64(map.page(2).add(8 * (i as usize % 64)), i) };
    }
    assert_eq!(server.stats().handled, 1);
    assert!(server.is_dirty(id, 2).unwrap());
    assert!(!server.is_dirty(id, 1).unwrap());

    let mut seen = Vec::new();
    assert_eq!(server.rearm_dirty(id, |p| seen.push(p)).unwrap(), 1);
    assert_eq!(seen, vec![2]);
    assert!(server.dirty_pages(id).unwrap().is_empty());

    unsafe { str64(map.page(2), 1) };
    assert_eq!(server.stats().handled, 2, "re-armed page must fault again");
    unsafe { str64(map.page(3), 1) };
    assert_eq!(server.stats().handled, 3, "never-written page stays armed");
    assert_eq!(server.dirty_pages(id).unwrap(), vec![2, 3]);
}

#[test]
fn rearm_cycles_repeat() {
    let (server, map, id) = armed(3);
    let _a = server.attach_current_thread().unwrap();
    for round in 0..50u64 {
        unsafe {
            str64(map.page(0), round);
            str64(map.page(2), round);
        }
        assert_eq!(server.rearm_dirty(id, |_| ()).unwrap(), 2);
    }
    assert_eq!(server.stats().handled, 100);
    unsafe { assert_eq!(load64(map.page(2)), 49) };
}

/// 4 KiB guest pages on 16 KiB host pages: one fault opens all four guest
/// pages of a host page, so a later write to a sibling does not fault. The
/// subpage drain must still report that sibling (no false negatives).
#[test]
fn guest_4k_subpages_are_reported_conservatively() {
    const GUEST: usize = 4096;
    let (server, map, id) = armed(2);
    let _a = server.attach_current_thread().unwrap();
    unsafe {
        str64(map.at(GUEST), 1); // guest page 1: faults, opens host page 0
        str64(map.at(3 * GUEST), 3); // guest page 3: same host page, no fault
    }
    assert_eq!(server.stats().handled, 1);
    let mut seen = Vec::new();
    server
        .rearm_dirty_subpages(id, GUEST, |g| seen.push(g))
        .unwrap();
    // Exact would be [1, 3]; host-page granularity adds 0 and 2.
    assert_eq!(seen, vec![0, 1, 2, 3]);
    // Re-armed: a write to guest page 2 now faults again.
    unsafe { str64(map.at(2 * GUEST), 2) };
    assert_eq!(server.stats().handled, 2);
    assert_eq!(
        server.rearm_dirty_subpages(id, 3 * GUEST, |_| ()).err(),
        Some(Error::BadRange)
    );
}

#[test]
fn store_straddling_two_armed_pages_marks_both() {
    let (server, map, id) = armed(2);
    let _a = server.attach_current_thread().unwrap();
    let p = map.page(1).wrapping_sub(3);
    unsafe { str64_unaligned(p, 0x0807_0605_0403_0201) };
    for i in 0..8 {
        assert_eq!(unsafe { *p.add(i) }, i as u8 + 1, "byte {i}");
    }
    assert_eq!(server.dirty_pages(id).unwrap(), vec![0, 1]);
    // One fault per page: the retried store completes once both are open.
    assert_eq!(server.stats().handled, 2);
}

#[test]
fn foreign_write_is_relayed_to_the_previous_thread_handler() {
    // A is installed first; B is stacked on top and does not watch A's region.
    let (a, map_a, id_a) = armed(2);
    let (b, map_b, id_b) = armed(2);
    {
        let _att_a = a.attach_current_thread().unwrap();
        {
            let _att_b = b.attach_current_thread().unwrap();
            assert_eq!(thread_bad_access_port(), b.port());
            unsafe {
                str64(map_a.page(1), 11);
                str64(map_b.page(0), 22);
                assert_eq!(load64(map_a.page(1)), 11);
            }
            let (sa, sb) = (a.stats(), b.stats());
            assert_eq!(sb.handled, 1, "{sb:?}");
            assert_eq!(sb.forwarded, 1, "{sb:?}");
            assert_eq!(sb.forwarded_to_previous, 1, "{sb:?}");
            assert_eq!(sa.handled, 1, "{sa:?}");
            assert_eq!(a.dirty_pages(id_a).unwrap(), vec![1]);
            assert_eq!(b.dirty_pages(id_b).unwrap(), vec![0]);
        }
        assert_eq!(thread_bad_access_port(), a.port(), "B restored A");
    }
    assert_eq!(thread_bad_access_port(), 0, "A restored none");
}

#[test]
fn only_attached_threads_are_watched_in_parallel() {
    let (server, map, id) = armed(16);
    std::thread::scope(|s| {
        for t in 0..4usize {
            let (server, map) = (&server, &map);
            s.spawn(move || {
                let _a = server.attach_current_thread().unwrap();
                for i in 0..4 {
                    unsafe { str64(map.page(t * 4 + i), (t * 10 + i) as u64) };
                }
            });
        }
    });
    assert_eq!(server.stats().handled, 16);
    assert_eq!(server.dirty_pages(id).unwrap(), (0..16).collect::<Vec<_>>());
}

#[test]
fn teardown_and_reinstall_on_one_thread() {
    for round in 0..5usize {
        let (server, map, id) = armed(2);
        {
            let _a = server.attach_current_thread().unwrap();
            assert_eq!(
                server.attach_current_thread().err(),
                Some(Error::AlreadyAttached)
            );
            unsafe { str64(map.page(round % 2), round as u64) };
            assert_eq!(server.stats().handled, 1);
        }
        assert_eq!(thread_bad_access_port(), 0);
        server.unregister(id).unwrap();
        // Unregistered regions are left writable, so this does not fault.
        unsafe { str64(map.page(1), 5) };
    }
}

#[test]
fn registration_is_validated() {
    let server = WatchServer::start().unwrap();
    let map = Mapping::new(4).unwrap();
    assert_eq!(
        server
            .register(map.base().wrapping_add(8), page_size())
            .err(),
        Some(Error::BadRange)
    );
    assert_eq!(
        server.register(map.base(), 100).err(),
        Some(Error::BadRange)
    );
    let id = server.register(map.base(), 2 * page_size()).unwrap();
    assert_eq!(
        server.register(map.page(1), 2 * page_size()).err(),
        Some(Error::BadRange)
    );
    server.register(map.page(2), 2 * page_size()).unwrap();
    server.unregister(id).unwrap();
    assert_eq!(server.unregister(id).err(), Some(Error::BadRegion));
}

#[test]
fn fault_cost_is_measurable() {
    let c = bench::measure(32, 3).unwrap();
    eprintln!("{c:#?}");
    assert!(c.fault_only_ns.p50 > 0);
    // Generous ceiling: fails only if the fault path is pathologically slow.
    assert!(c.fault_only_ns.p50 < 5_000_000, "{c:?}");
}
