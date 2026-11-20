//! `HostBackend` write-behind: every guest-visible answer is the one a
//! synchronous write gives. docs/runtime/storage.md "Write-behind".

use super::*;
use crate::write_behind::{MAX_IN_FLIGHT, Queue};
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, AtomicUsize};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "vfs-write-behind-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn backend_in(&self, name: &str, behind: bool) -> HostBackend {
        HostBackend::new(
            self.0.join(name).join("root"),
            self.0.join(name).join("metadata"),
        )
        .unwrap()
        .with_write_behind(behind)
    }
    fn backend(&self) -> HostBackend {
        self.backend_in("v", true)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn queue(backend: &HostBackend) -> Arc<Queue> {
    backend
        .write_behind
        .as_ref()
        .expect("write-behind on")
        .queue()
        .clone()
}
fn read(backend: &mut HostBackend, path: &str, offset: u64, n: usize) -> Vec<u8> {
    let mut v = vec![0; n];
    let got = backend.read_at(path, offset, &mut v).unwrap();
    v.truncate(got);
    v
}
/// Unpause the writer after a moment, from another thread.
fn unpause_soon(q: &Arc<Queue>) -> std::thread::JoinHandle<()> {
    let q = q.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        q.test(|t| t.paused = false);
    })
}

#[test]
fn without_write_behind_writes_land_at_once() {
    let temp = Temp::new();
    let mut sync = temp.backend_in("s", false);
    assert!(sync.write_behind.is_none());
    sync.create("a", false, 0).unwrap();
    sync.write_at("a", 0, b"now").unwrap();
    assert_eq!(fs::read(sync.root.join("a")).unwrap(), b"now");
}

#[test]
fn queued_writes_look_landed_to_stat_listings_and_usage() {
    let temp = Temp::new();
    let mut b = temp.backend();
    b.create("a", false, 0).unwrap();
    let available = b.volume_info().unwrap().available_allocation_units;
    let q = queue(&b);
    q.test(|t| t.paused = true);
    assert_eq!(b.write_at("a", 0, &[1; 100]), Ok(100));
    // Overlapping, then past the first cluster.
    assert_eq!(b.write_at("a", 50, &[2; 100]), Ok(100));
    assert_eq!(b.write_at("a", 20_000, b"tail"), Ok(4));
    let host = b.root.join("a");
    assert_eq!(fs::metadata(&host).unwrap().len(), 0, "still in flight");
    let info = b.stat("a").unwrap();
    assert_eq!(info.end_of_file, 20_004);
    assert_eq!(info.allocation_size, 2 * FATX_CLUSTER_SIZE);
    assert_ne!(info.basic.attributes & ATTR_ARCHIVE, 0);
    let listed = b.read_directory("").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].metadata.end_of_file, 20_004);
    assert_eq!(listed[0].metadata.allocation_size, 2 * FATX_CLUSTER_SIZE);
    assert_eq!(
        b.volume_info().unwrap().available_allocation_units,
        available - 2
    );
    // A write past the queued end grows from the queued end.
    assert_eq!(b.write_at("a", 20_004, b"!"), Ok(1));
    assert_eq!(b.stat("a").unwrap().end_of_file, 20_005);
    // A read waits for the queue, and sees the writes in order.
    let unpause = unpause_soon(&q);
    let mut expected = vec![1; 50];
    expected.extend([2; 100]);
    expected.extend([0; 50]);
    assert_eq!(read(&mut b, "a", 0, 200), expected);
    unpause.join().unwrap();
    assert_eq!(read(&mut b, "a", 20_000, 16), b"tail!");
    assert_eq!(fs::metadata(&host).unwrap().len(), 20_005);
}

#[test]
fn operations_on_a_files_bytes_wait_for_its_queued_writes() {
    let temp = Temp::new();
    let mut b = temp.backend();
    let q = queue(&b);
    // Every job sleeps first: data is really in flight below.
    q.test(|t| t.delay = Duration::from_millis(30));
    let host = |b: &HostBackend, p: &str| b.root.join(p);

    // set_len truncates after the write lands, not before.
    b.create("trunc", false, 0).unwrap();
    b.write_at("trunc", 0, &[7; 10]).unwrap();
    b.set_len("trunc", 5).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(fs::read(host(&b, "trunc")).unwrap(), [7; 5]);

    // flush: the data is on the host (and synced) when it returns.
    b.create("flush", false, 0).unwrap();
    b.write_at("flush", 0, b"durable").unwrap();
    b.flush("flush").unwrap();
    assert_eq!(fs::read(host(&b, "flush")).unwrap(), b"durable");

    // cleanup (close) of a writer flushes the same way.
    b.write_at("flush", 7, b"!").unwrap();
    b.cleanup("flush", true).unwrap();
    assert_eq!(fs::read(host(&b, "flush")).unwrap(), b"durable!");

    // host_file: an outside reader sees the bytes.
    b.create("mapped", false, 0).unwrap();
    b.write_at("mapped", 0, b"seen").unwrap();
    let mapped = b.host_file("mapped").unwrap();
    assert_eq!(mapped.len, 4);
    assert_eq!(fs::read(&mapped.path).unwrap(), b"seen");

    // remove, then create at the path: the old file's queued write never
    // reaches the new one.
    b.create("again", false, 0).unwrap();
    b.write_at("again", 0, &[9; 1000]).unwrap();
    b.remove("again").unwrap();
    b.create("again", false, 0).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(b.stat("again").unwrap().end_of_file, 0);
    assert_eq!(fs::metadata(host(&b, "again")).unwrap().len(), 0);
    b.write_at("again", 0, b"new").unwrap();
    assert_eq!(read(&mut b, "again", 0, 16), b"new");

    // overwrite_allocated truncates after the write lands.
    b.write_at("again", 3, &[1; 100]).unwrap();
    b.overwrite_allocated("again", 1, 1).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(fs::metadata(host(&b, "again")).unwrap().len(), 0);

    // format: nothing queued lands in the emptied volume.
    b.write_at("trunc", 100, &[3; 100]).unwrap();
    b.format().unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(fs::read_dir(&b.root).unwrap().count(), 0);

    // drop lands everything still queued (the 30 ms delay is still on).
    b.create("last", false, 0).unwrap();
    b.write_at("last", 0, b"kept").unwrap();
    assert!(!b.write_behind.as_ref().unwrap().idle(), "still in flight");
    let path = host(&b, "last");
    drop(b);
    assert_eq!(fs::read(path).unwrap(), b"kept");
}

#[test]
fn a_failed_host_write_is_reported_once_by_the_files_next_operation() {
    let temp = Temp::new();
    let mut b = temp.backend();
    b.create("a", false, 0).unwrap();
    b.create("b", false, 0).unwrap();
    let available = b.volume_info().unwrap().available_allocation_units;
    let q = queue(&b);

    // The guest got success; flush reports the failure, once.
    q.test(|t| t.fail_next = Some(ErrorKind::StorageFull));
    assert_eq!(b.write_at("a", 0, &[1; 20_000]), Ok(20_000));
    assert_eq!(b.flush("a"), Err(Error::DiskFull));
    assert_eq!(b.flush("a"), Ok(()));
    // The write never landed: stat and usage now follow the host.
    assert_eq!(b.stat("a").unwrap().end_of_file, 0);
    assert_eq!(
        b.volume_info().unwrap().available_allocation_units,
        available
    );

    // A later write reports it (and writes nothing); the next one works.
    q.test(|t| t.fail_next = Some(ErrorKind::Other));
    assert_eq!(b.write_at("a", 0, b"lost"), Ok(4));
    b.settle_all();
    assert_eq!(b.write_at("a", 0, b"kept"), Err(Error::IoDeviceError));
    assert_eq!(b.write_at("a", 0, b"kept"), Ok(4));
    assert_eq!(read(&mut b, "a", 0, 16), b"kept");

    // Errors belong to their file: "b" never sees "a"'s.
    q.test(|t| t.fail_next = Some(ErrorKind::Other));
    b.write_at("a", 0, b"gone").unwrap();
    b.write_at("b", 0, b"fine").unwrap();
    assert_eq!(read(&mut b, "b", 0, 16), b"fine");
    b.flush("b").unwrap();
    let mut buffer = [0; 4];
    assert_eq!(b.read_at("a", 0, &mut buffer), Err(Error::IoDeviceError));
    assert_eq!(read(&mut b, "a", 0, 16), b"kept");

    // A failed format leaves errors for surviving files reportable.
    q.test(|t| t.fail_next = Some(ErrorKind::StorageFull));
    b.write_at("a", 0, b"x").unwrap();
    b.next_format_error.set(Some(Error::AccessDenied));
    assert_eq!(b.format(), Err(Error::AccessDenied));
    assert_eq!(b.read_at("a", 0, &mut buffer), Err(Error::DiskFull));

    // Format drops latched errors with the files.
    q.test(|t| t.fail_next = Some(ErrorKind::Other));
    b.write_at("a", 0, b"x").unwrap();
    b.format().unwrap();
    b.create("a", false, 0).unwrap();
    assert_eq!(b.flush("a"), Ok(()));
}

#[test]
fn queued_bytes_stay_bounded_and_writers_wait_for_room() {
    let temp = Temp::new();
    let mut b = temp.backend();
    b.create("a", false, 0).unwrap();
    let q = queue(&b);
    q.test(|t| t.paused = true);
    const CHUNK: usize = 256 * 1024;
    let fit = MAX_IN_FLIGHT / CHUNK;
    let n = 3 * fit;
    let done = Arc::new(AtomicUsize::new(0));
    let writer = {
        let done = done.clone();
        std::thread::spawn(move || {
            for i in 0..n {
                assert_eq!(
                    b.write_at("a", (i * CHUNK) as u64, &vec![i as u8; CHUNK]),
                    Ok(CHUNK)
                );
                done.fetch_add(1, Ordering::SeqCst);
            }
            b
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while done.load(Ordering::SeqCst) < fit && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(done.load(Ordering::SeqCst), fit, "the next write waits");
    assert_eq!(q.in_flight(), MAX_IN_FLIGHT);
    q.test(|t| t.paused = false);
    let mut b = writer.join().unwrap();
    assert!(q.test(|t| t.peak_in_flight) <= MAX_IN_FLIGHT);
    for i in 0..n {
        assert_eq!(
            read(&mut b, "a", (i * CHUNK) as u64, CHUNK),
            vec![i as u8; CHUNK]
        );
    }
    // One write larger than the bound is admitted once the queue is empty.
    q.test(|t| t.paused = true);
    let big = vec![5; MAX_IN_FLIGHT + 1];
    assert_eq!(b.write_at("a", 0, &big), Ok(big.len()));
    let unpause = unpause_soon(&q);
    assert_eq!(read(&mut b, "a", 0, big.len()), big);
    unpause.join().unwrap();
}

/// A tiny deterministic generator (xorshift64*).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Everything a guest could observe of a result, timestamps aside (they
/// are wall-clock, two-second granular).
fn shown(info: &Metadata) -> String {
    format!(
        "eof={} alloc={} attr={:#x}",
        info.end_of_file, info.allocation_size, info.basic.attributes
    )
}

/// One random operation on `b`, described.
fn step(b: &mut HostBackend, op: u64, path: &str, a: u64, c: u64, fill: u8) -> String {
    match op {
        0 => format!("{:?}", b.create(path, false, 0)),
        1..=4 => {
            let data = vec![fill; c as usize % (300 * 1024) + 1];
            format!("{:?}", b.write_at(path, a % (96 * 1024), &data))
        }
        5 => {
            let mut buf = vec![0; c as usize % (200 * 1024)];
            let got = b.read_at(path, a % (128 * 1024), &mut buf);
            match got {
                Ok(n) => format!("Ok({n}) {:?}", hash(&buf[..n])),
                Err(e) => format!("{e:?}"),
            }
        }
        6 => format!("{:?}", b.stat(path).as_ref().map(shown)),
        7 => format!("{:?}", b.set_len(path, a % (128 * 1024))),
        8 => format!("{:?}", b.flush(path)),
        9 => format!("{:?}", b.cleanup(path, c.is_multiple_of(2))),
        10 => format!("{:?}", b.remove(path)),
        11 => format!(
            "{:?}",
            b.read_directory("").map(|l| l
                .iter()
                .map(|e| format!("{} {}", e.name, shown(&e.metadata)))
                .collect::<Vec<_>>())
        ),
        12 => format!(
            "{:?}",
            b.volume_info().map(|v| v.available_allocation_units)
        ),
        13 => format!("{:?}", b.overwrite_allocated(path, c % (64 * 1024), a)),
        14 => format!("{:?}", b.release_allocation(path, a)),
        _ => format!("{:?}", b.host_file(path).map(|f| f.len)),
    }
}
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}
fn host_bytes(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn random_operations_give_the_same_results_with_and_without_write_behind() {
    for seed in 1..=4u64 {
        let temp = Temp::new();
        // A small volume, so growth also meets DiskFull.
        let volume = |b: HostBackend| b.with_volume("t", 1, 40 * FATX_CLUSTER_SIZE);
        let mut sync = volume(temp.backend_in("sync", false));
        let mut behind = volume(temp.backend_in("behind", true));
        let q = queue(&behind);
        q.test(|t| t.delay = Duration::from_micros(300));
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        for i in 0..250 {
            let op = rng.below(16);
            let path = ["a", "b", "c"][rng.below(3) as usize];
            let (a, c) = (rng.next(), rng.next());
            let fill = rng.below(256) as u8;
            let want = step(&mut sync, op, path, a, c, fill);
            let got = step(&mut behind, op, path, a, c, fill);
            assert_eq!(got, want, "seed {seed} step {i}: op {op} on {path}");
        }
        let (sync_root, behind_root) = (sync.root.clone(), behind.root.clone());
        drop(sync);
        drop(behind);
        assert_eq!(
            host_bytes(&behind_root),
            host_bytes(&sync_root),
            "seed {seed}"
        );
    }
}

/// `cargo test -p vfs --release bench_write_behind -- --ignored --nocapture`:
/// calling-thread time per 256 KiB write, synchronous against queued.
#[test]
#[ignore]
fn bench_write_behind() {
    const CHUNK: usize = 256 * 1024;
    const N: usize = 256; // 64 MiB per run
    let temp = Temp::new();
    let data = vec![0x5a; CHUNK];
    let run = |name: &str, behind: bool, gap: Duration| {
        let mut b = temp.backend_in(&format!("{name}-{gap:?}"), behind);
        b.create("cache", false, 0).unwrap();
        let mut spent = Vec::with_capacity(N);
        for i in 0..N {
            let t = Instant::now();
            b.write_at("cache", (i * CHUNK) as u64, &data).unwrap();
            spent.push(t.elapsed());
            // The rest of the game's frame, busy (a sleeping thread
            // wakes on a cold, slow core and every cost inflates).
            let t = Instant::now();
            while t.elapsed() < gap {
                std::hint::spin_loop();
            }
        }
        b.flush("cache").unwrap();
        let mean = spent.iter().sum::<Duration>() / N as u32;
        spent.sort();
        let median = spent[N / 2];
        let (p90, max) = (spent[N * 9 / 10], spent[N - 1]);
        eprintln!(
            "{name:>6}, {gap:>5?} between writes: mean {mean:>9.1?}, median {median:>9.1?}, p90 {p90:>9.1?}, max {max:>9.1?}"
        );
        median
    };
    // Back to back, the writer is the bottleneck and backpressure holds the
    // caller to the host's rate; with room between writes (the game's
    // frames, its reads), the caller pays only the bookkeeping and copy.
    for gap in [0, 1, 5] {
        let gap = Duration::from_millis(gap);
        let sync = run("sync", false, gap);
        let behind = run("behind", true, gap);
        if gap >= Duration::from_millis(5) {
            assert!(behind < sync);
        }
    }
}
