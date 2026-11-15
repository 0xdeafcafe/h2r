//! The guest thread's lifecycle on real threads: pacing sleeps, pauses,
//! stop requests, timeouts and every way a guest can end. Needs a GPU
//! adapter for `GuestHost::headless`; skips without one.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use app::worker::{GuestFn, GuestWorker};
use app::{AppConfig, Control, GuestExit, GuestHost};

fn host() -> Option<GuestHost> {
    let config = AppConfig {
        gamepads: false,
        ..AppConfig::default()
    };
    match GuestHost::headless(&config) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn wait(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < limit {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    f()
}

/// A guest whose next frame is an hour ahead: it sits in the pacing sleep.
fn sleeper(entered: Arc<AtomicBool>) -> GuestFn {
    Box::new(move |mut host: GuestHost| {
        assert_eq!(host.tick(0), Control::Continue);
        entered.store(true, Ordering::SeqCst);
        loop {
            if host.tick(36_000_000_000) == Control::Stop {
                return Ok(());
            }
        }
    })
}

#[test]
fn close_during_a_pacing_sleep() {
    let Some(h) = host() else { return };
    let entered = Arc::new(AtomicBool::new(false));
    let mut w = GuestWorker::spawn(h, sleeper(Arc::clone(&entered)), || {}).unwrap();
    assert!(wait(Duration::from_secs(2), || entered.load(Ordering::SeqCst)));
    std::thread::sleep(Duration::from_millis(20));
    let t = Instant::now();
    assert_eq!(w.stop(Duration::from_secs(5)), GuestExit::Returned);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

#[test]
fn close_during_a_pause() {
    let Some(h) = host() else { return };
    let life = h.lifecycle();
    life.set_paused(true);
    let ticks = Arc::new(AtomicU32::new(0));
    let t2 = Arc::clone(&ticks);
    let guest: GuestFn = Box::new(move |mut host: GuestHost| {
        while host.tick(0) == Control::Continue {
            t2.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    });
    let mut w = GuestWorker::spawn(h, guest, || {}).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        0,
        "a paused guest does not run"
    );
    assert!(w.try_finish().is_none());
    let t = Instant::now();
    assert_eq!(w.stop(Duration::from_secs(5)), GuestExit::Returned);
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_pause_during_a_pacing_sleep_holds_the_guest() {
    let Some(h) = host() else { return };
    let life = h.lifecycle();
    let ran = Arc::new(AtomicBool::new(false));
    let r2 = Arc::clone(&ran);
    let guest: GuestFn = Box::new(move |mut host: GuestHost| {
        host.tick(0);
        // One frame (200 ms) ahead: the pacer sleeps.
        let c = host.tick(2_000_000);
        r2.store(true, Ordering::SeqCst);
        assert_eq!(c, Control::Continue);
        Ok(())
    });
    let mut w = GuestWorker::spawn(h, guest, || {}).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    life.set_paused(true);
    // Well past the 200 ms pacing deadline: still paused, so not returned.
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !ran.load(Ordering::SeqCst),
        "tick returned Continue while paused"
    );
    let t = Instant::now();
    life.set_paused(false);
    assert!(wait(Duration::from_secs(2), || ran.load(Ordering::SeqCst)));
    // The pause reset the origin: no extra sleep owed for the pause.
    assert!(
        t.elapsed() < Duration::from_millis(150),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(w.stop(Duration::from_secs(2)), GuestExit::Returned);
}

#[test]
fn a_guest_that_ignores_stop_times_out() {
    let Some(h) = host() else { return };
    let release = Arc::new(AtomicBool::new(false));
    let r2 = Arc::clone(&release);
    let guest: GuestFn = Box::new(move |_host: GuestHost| {
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    });
    let mut w = GuestWorker::spawn(h, guest, || {}).unwrap();
    let t = Instant::now();
    assert_eq!(w.stop(Duration::from_millis(100)), GuestExit::StillRunning);
    assert!(t.elapsed() >= Duration::from_millis(100));
    assert!(w.lifecycle().should_stop(), "the stop was requested");
    release.store(true, Ordering::SeqCst);
    assert_eq!(w.stop(Duration::from_secs(2)), GuestExit::Returned);
}

fn finished(guest: GuestFn) -> Option<GuestExit> {
    let h = host()?;
    let exited = Arc::new(AtomicBool::new(false));
    let e2 = Arc::clone(&exited);
    let mut w = GuestWorker::spawn(h, guest, move || e2.store(true, Ordering::SeqCst)).unwrap();
    assert!(
        wait(Duration::from_secs(5), || exited.load(Ordering::SeqCst)),
        "on_exit runs"
    );
    assert!(wait(Duration::from_secs(2), || w.try_finish().is_some()));
    let e = w.try_finish().cloned();
    assert!(!w.lifecycle().should_stop(), "nobody asked it to stop");
    e
}

#[test]
fn normal_return() {
    assert_eq!(
        finished(Box::new(|_| Ok(()))).unwrap_or(GuestExit::Returned),
        GuestExit::Returned
    );
}

#[test]
fn error_return() {
    let Some(e) = finished(Box::new(|_| {
        Err(anyhow::anyhow!("boot failed").context("running the XBE"))
    })) else {
        return;
    };
    assert_eq!(e, GuestExit::Failed("running the XBE: boot failed".into()));
}

#[test]
fn panic_is_caught() {
    let Some(e) = finished(Box::new(|_| panic!("guest fault at {:#x}", 0x1234))) else {
        return;
    };
    assert_eq!(e, GuestExit::Panicked("guest fault at 0x1234".into()));
}
