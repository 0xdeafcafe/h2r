//! Scenario runner for the fastmem write-watch prototype. Each scenario runs
//! in its own process so crashes, task-wide ports and port-name counts stay
//! isolated from the test harness. `tests/process.rs` drives it; it also runs
//! by hand. Linux x86-64 also has `chain-handler` for a previous SIGSEGV
//! action. The Mach port chain and benchmark run on macOS arm64:
//!
//! ```text
//! fastmem-probe smoke | chain-task | teardown | all
//! fastmem-probe null-deref | unregistered-write | region-read-none | unattached-thread-write
//! fastmem-probe bench [pages] [rounds]
//! ```

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("all");
    match mode {
        "smoke" => scenarios::smoke(),
        "chain-task" => scenarios::chain_task(),
        "teardown" => scenarios::teardown(),
        "all" => {
            scenarios::smoke();
            scenarios::chain_task();
            scenarios::teardown();
        }
        "null-deref" => scenarios::null_deref(),
        "unregistered-write" => scenarios::unregistered_write(),
        "region-read-none" => scenarios::region_read_none(),
        "unattached-thread-write" => scenarios::unattached_thread_write(),
        "bench" => {
            let pages = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256);
            let rounds = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(40);
            scenarios::bench(pages, rounds);
        }
        other => {
            eprintln!("unknown scenario {other}");
            std::process::exit(2);
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "fastmem-probe/linux.rs"]
mod linux;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() -> std::io::Result<()> {
    linux::run(&std::env::args().nth(1).unwrap_or_else(|| "all".into()))
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
#[test]
fn linux_rejects_unknown_scenarios() {
    assert!(linux::run("unknown").is_err());
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
fn main() {
    eprintln!("fastmem-probe only runs on macOS arm64 or Linux x86-64");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod scenarios {
    use fastmem_proto::ops::*;
    use fastmem_proto::sys::{PROT_NONE, PROT_READ};
    use fastmem_proto::{Mapping, RegionId, WatchServer, bench as costs};

    macro_rules! check {
        ($cond:expr, $($msg:tt)*) => {
            if !$cond {
                eprintln!("FAIL: {}", format!($($msg)*));
                std::process::exit(1);
            }
        };
    }

    /// Server with one 8-page region, armed, and the calling thread attached.
    fn setup() -> (WatchServer, Mapping, RegionId) {
        let server = WatchServer::start().expect("start");
        let map = Mapping::new(8).expect("map");
        let id = server.register(map.base(), map.len()).expect("register");
        server.arm(id).expect("arm");
        (server, map, id)
    }

    pub fn smoke() {
        let (server, map, id) = setup();
        let _a = server.attach_current_thread().expect("attach");
        // SAFETY: aligned addresses inside the mapping.
        unsafe {
            str64(map.page(1), 0x1111);
            str64(map.page(1).add(8), 0x2222);
            check!(load64(map.page(1)) == 0x1111, "first write lost");
        }
        let s = server.stats();
        check!(s.handled == 1, "expected one fault, got {s:?}");
        check!(server.dirty_pages(id).unwrap() == vec![1], "dirty set");
        let n = server.rearm_dirty(id, |_| ()).unwrap();
        check!(n == 1, "rearm count {n}");
        // SAFETY: as above.
        unsafe { str64(map.page(1), 0x3333) };
        check!(server.stats().handled == 2, "re-armed page did not fault");
        println!("smoke ok: {:?}", server.stats());
    }

    pub fn chain_task() {
        // The "previously installed" handler: a task-wide server watching P.
        let previous = WatchServer::start().expect("start previous");
        let p = Mapping::new(2).expect("map p");
        let pid = previous.register(p.base(), p.len()).expect("register p");
        previous.arm(pid).expect("arm p");
        let _task = previous.attach_task().expect("attach task");

        let (server, r, _) = setup();
        let _thread = server.attach_current_thread().expect("attach thread");
        // SAFETY: aligned addresses inside the mappings.
        unsafe {
            str64(r.page(0), 1);
            str64(p.page(1), 2);
            check!(load64(p.page(1)) == 2, "write to P lost");
        }
        let ours = server.stats();
        let theirs = previous.stats();
        check!(ours.handled == 1 && ours.forwarded == 1, "ours {ours:?}");
        check!(
            ours.forwarded_to_previous == 0,
            "declined, not relayed: {ours:?}"
        );
        check!(
            theirs.handled == 1,
            "task-level handler did not see P: {theirs:?}"
        );
        println!("chain-task ok: ours {ours:?} previous {theirs:?}");
    }

    pub fn teardown() {
        let before = port_name_count();
        for round in 0..5 {
            let (server, map, _) = setup();
            {
                let _a = server.attach_current_thread().expect("attach");
                check!(thread_bad_access_port() == server.port(), "not installed");
                // SAFETY: aligned address inside the mapping.
                unsafe { str64(map.page(round), round as u64) };
                check!(server.stats().handled == 1, "round {round}");
            }
            check!(thread_bad_access_port() == 0, "not restored");
        }
        let after = port_name_count();
        check!(before == after, "port names leaked: {before} -> {after}");
        println!("teardown ok: {before} port names before and after 5 rounds");
    }

    pub fn null_deref() {
        let (server, _map, _) = setup();
        let _a = server.attach_current_thread().expect("attach");
        // SAFETY: deliberately faults; this scenario must crash.
        let v = unsafe { ldr8(std::hint::black_box(core::ptr::null())) };
        println!("survived null deref: {v}");
    }

    pub fn unregistered_write() {
        let (server, _map, _) = setup();
        let other = Mapping::new(1).expect("map");
        other.protect(0, 1, PROT_READ).expect("protect");
        let _a = server.attach_current_thread().expect("attach");
        // SAFETY: deliberately faults outside any region.
        unsafe { str64(other.base(), 1) };
        println!("survived unregistered write");
    }

    pub fn region_read_none() {
        let (server, map, _) = setup();
        map.protect(2, 1, PROT_NONE).expect("protect");
        let _a = server.attach_current_thread().expect("attach");
        // SAFETY: deliberately faults: a read, which is not ours to fix.
        let v = unsafe { ldr8(map.page(2)) };
        println!("survived read of PROT_NONE page: {v}");
    }

    pub fn unattached_thread_write() {
        let (server, map, _) = setup();
        let _a = server.attach_current_thread().expect("attach");
        let addr = map.page(3) as usize;
        std::thread::spawn(move || {
            // SAFETY: deliberately faults on a thread that is not attached.
            unsafe { str64(addr as *mut u8, 1) };
        })
        .join()
        .ok();
        println!("survived unattached write");
    }

    pub fn bench(pages: usize, rounds: usize) {
        let c = costs::measure(pages, rounds).expect("measure");
        println!(
            "pages {} rounds {} page size {}",
            c.pages,
            c.rounds,
            fastmem_proto::sys::page_size()
        );
        let pct = |p: costs::Percentiles| {
            format!(
                "p50 {:>6} ns  p10 {:>6}  p90 {:>6}  mean {:>8.0}",
                p.p50, p.p10, p.p90, p.mean
            )
        };
        println!(
            "re-arm one page + first-write fault: {}",
            pct(c.single_rearm_and_fault_ns)
        );
        println!(
            "first-write fault alone:             {}",
            pct(c.fault_only_ns)
        );
        println!(
            "mprotect RO+RW pair, no fault:       {:>8.0} ns",
            c.mprotect_pair_ns
        );
        println!(
            "bulk-armed fault per page:           {:>8.0} ns",
            c.bulk_fault_ns
        );
        println!(
            "rearm_dirty per page (coalesced):    {:>8.0} ns",
            c.bulk_rearm_dirty_ns
        );
        println!(
            "plain store, page open:              {:>8.1} ns",
            c.plain_store_ns
        );
    }
}
