//! Write metadata survives exits that skip `HostBackend`'s drop
//! (docs/storage.md "Write metadata"): a child process writes, then calls
//! `process::exit` with the backend still alive, and the parent reads the
//! record back.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use vfs::{Backend, BasicInfo, HostBackend, Timestamp};

const CHILD: &str = "VFS_EXIT_TEST_CHILD";
/// 2001-01-01 in FATX time: older than anything a write stamps.
const OLD: Timestamp = Timestamp(126_227_808_000_000_000);

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vfs-exit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn backend(dir: &Path) -> HostBackend {
    HostBackend::new(dir.join("root"), dir.join("metadata")).unwrap()
}

/// Child: an old-stamped file, one write, then `how`, then exit(0) with the
/// backend alive.
fn child(dir: &Path, how: &str) -> ! {
    let mut b = backend(dir);
    b.create("data", false, 0).unwrap();
    b.flush("data").unwrap();
    let mut basic: BasicInfo = b.stat("data").unwrap().basic;
    basic.last_write_time = OLD;
    basic.change_time = OLD;
    b.set_basic_info("data", basic).unwrap();
    b.write_at("data", 0, b"payload").unwrap();
    match how {
        "wait" => {
            // Exit only once the flusher thread has stored the record, so a
            // slow host cannot make the child race it. Generous cap: the
            // parent's assertion reports the failure.
            std::thread::sleep(vfs::METADATA_STALE);
            let give_up = Instant::now() + Duration::from_secs(60);
            while stored_write_time(dir) <= OLD && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        "flush-all" => vfs::flush_pending_metadata(),
        other => panic!("{other}"),
    }
    std::process::exit(0);
}

fn run_child(test: &str, how: &str) -> PathBuf {
    let dir = scratch(how);
    let status = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, format!("{how}:{}", dir.display()))
        .status()
        .unwrap();
    if !status.success() {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("child: {status}");
    }
    dir
}

fn as_child() -> bool {
    let Ok(v) = std::env::var(CHILD) else {
        return false;
    };
    let (how, dir) = v.split_once(':').unwrap();
    child(Path::new(dir), how)
}

fn stored_write_time(dir: &Path) -> Timestamp {
    // A fresh backend reads the record (nothing pending in it).
    let mut b = backend(dir);
    let t = b.stat("data").unwrap().basic.last_write_time;
    drop(b);
    t
}

#[test]
fn an_exit_after_the_stale_window_keeps_the_write_metadata() {
    if as_child() {
        return;
    }
    let started = Instant::now();
    let dir = run_child(
        "an_exit_after_the_stale_window_keeps_the_write_metadata",
        "wait",
    );
    assert!(started.elapsed() >= vfs::METADATA_STALE);
    assert!(
        stored_write_time(&dir) > OLD,
        "the flusher stored the write's times"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flush_pending_metadata_before_an_immediate_exit_keeps_it() {
    if as_child() {
        return;
    }
    let dir = run_child(
        "flush_pending_metadata_before_an_immediate_exit_keeps_it",
        "flush-all",
    );
    assert!(stored_write_time(&dir) > OLD);
    let _ = std::fs::remove_dir_all(dir);
}
