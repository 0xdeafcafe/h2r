//! Bounded menu measurement, including process user+system CPU for all threads.
//! Run serially through the bench gate; see docs/runtime/BOOT.md.
#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    use std::{path::PathBuf, time::Instant};
    fn cpu_seconds() -> std::io::Result<f64> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage writes the entire structure on success.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let usage = unsafe { usage.assume_init() };
        let seconds = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
        Ok(seconds(usage.ru_utime) + seconds(usage.ru_stime))
    }
    // CPU time of the calling thread: the runner thread, which runs every
    // guest thread and, without a render thread, all of the backend's work.
    fn thread_cpu_seconds() -> std::io::Result<f64> {
        let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: clock_gettime writes the entire timespec on success.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ts = unsafe { ts.assume_init() };
        Ok(ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9)
    }
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    anyhow::ensure!(args.len() == 3, "usage: menu_cpu DISC OUT FRESH_DATA_DIR");
    anyhow::ensure!(
        !args[2].exists(),
        "data directory must be new for a reproducible run"
    );
    let opts = boot::RunOpts {
        disc: args[0].clone(),
        data_root: args[2].clone(),
        max_slices: 100_000,
        xapi: true,
        transfers_per_slice: 50_000_000,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        movies: boot::movies_glue::Movies::Off,
    };
    let mut start = None;
    let host = boot::Host {
        trace_limit: Some(4096),
        between_slices: Some(Box::new(move |_| {
            let n = boot::presents::presents();
            if n >= 480 && start.is_none() {
                assert_eq!(n, 480, "measurement must start at present 480");
                start = Some((
                    Instant::now(),
                    cpu_seconds().expect("process CPU clock"),
                    thread_cpu_seconds().expect("thread CPU clock"),
                ));
            }
            if n >= 560 {
                assert_eq!(n, 560, "measurement must stop at present 560");
                let (wall, cpu, thread) = start.expect("window start");
                let seconds = wall.elapsed().as_secs_f64();
                let used = cpu_seconds().expect("process CPU clock") - cpu;
                let guest = thread_cpu_seconds().expect("thread CPU clock") - thread;
                println!(
                    "MENU_CPU presents=480-560 frames=80 wall_s={seconds:.6} cpu_s={used:.6} fps={:.6} cpu_ms_per_frame={:.6} guest_thread_cpu_ms_per_frame={:.6}",
                    80.0 / seconds,
                    used * 1000.0 / 80.0,
                    guest * 1000.0 / 80.0
                );
                return false;
            }
            true
        })),
        ..Default::default()
    };
    let report = boot::run_hosted(
        &boot::read_xbe(&args[0])?,
        &args[1].join(boot::DYLIB),
        &opts,
        host,
    )?;
    let measured = report.outcome == "HostStop" && boot::presents::presents() == 560;
    anyhow::ensure!(
        measured,
        "incomplete menu run: {} ({})",
        report.outcome,
        report.stop
    );
    Ok(())
}
#[cfg(not(unix))]
fn main() {
    panic!("menu_cpu currently requires Unix getrusage");
}
