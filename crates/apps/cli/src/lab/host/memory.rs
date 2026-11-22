//! The daemon supervises the game's PID independently of the revision under test.
//! All reports live inside the job directory so an ordinary fetch brings them back.

use std::collections::VecDeque;
use std::fs;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Job, Meta, Shared, agent_socket_path, write_meta};

const INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_LIMIT: u64 = 12 << 30;
const SAMPLES: usize = 300;

pub(super) enum Event {
    Step,
    Finished,
}

#[derive(Default)]
struct Samples {
    recent: VecDeque<(u64, u64)>,
}

impl Samples {
    fn sample(
        &mut self,
        meta: &mut Meta,
        pid: u32,
        elapsed_ms: u64,
        limit: u64,
        read: impl FnOnce(u32) -> Option<u64>,
    ) -> Option<String> {
        let bytes = read(pid)?;
        meta.last_footprint_bytes = Some(bytes);
        meta.peak_footprint_bytes = Some(bytes.max(meta.peak_footprint_bytes.unwrap_or(0)));
        if self.recent.len() == SAMPLES {
            self.recent.pop_front();
        }
        self.recent.push_back((elapsed_ms, bytes));
        (limit != 0 && bytes > limit).then(|| format!("memory limit: {bytes} over {limit} bytes"))
    }
}

/// Run the monitor through builds, takeover and gameplay. Waking it on spawn
/// records a first game sample without delaying a short job by two seconds.
pub(super) fn supervise<T>(
    sh: &Shared,
    id: &str,
    job: &Job,
    run: impl FnOnce() -> T,
) -> (T, Option<String>) {
    let limit = match job {
        Job::Run(run) => run.mem_limit.unwrap_or(DEFAULT_LIMIT),
        Job::Sim(sim) => sim.mem_limit.unwrap_or(DEFAULT_LIMIT),
        Job::Build(_) => DEFAULT_LIMIT,
    };
    let (tx, rx) = mpsc::channel();
    {
        let mut inner = sh.inner.lock().unwrap();
        if let Some(active) = inner.active.as_mut().filter(|a| a.id == id) {
            active.memory_wake = Some(tx.clone());
        }
    }
    struct Finish(mpsc::Sender<Event>);
    impl Drop for Finish {
        fn drop(&mut self) {
            let _ = self.0.send(Event::Finished);
        }
    }
    std::thread::scope(|scope| {
        let guard = scope.spawn(move || monitor(sh, id, limit, rx));
        let finish = Finish(tx);
        let result = run();
        drop(finish);
        (result, guard.join().expect("lab job memory monitor"))
    })
}

fn monitor(sh: &Shared, id: &str, limit: u64, rx: Receiver<Event>) -> Option<String> {
    let start = Instant::now();
    let mut samples = Samples::default();
    loop {
        let pressure = mem_watch::pressure() == Some(true);
        let mut inner = sh.inner.lock().unwrap();
        let active = inner.active.as_ref().filter(|a| a.id == id && !a.stop)?;
        let pgid = active.pgid;
        let meta = inner.jobs.get_mut(id)?;
        let game = pgid
            .filter(|_| meta.state == "running")
            .map(|pid| pid as u32);
        let elapsed = start.elapsed().as_millis() as u64;
        let cap =
            game.and_then(|pid| samples.sample(meta, pid, elapsed, limit, mem_watch::footprint));
        let reason = cap.or_else(|| pressure.then(|| "machine memory pressure".to_owned()));
        if let Some(reason) = &reason {
            meta.error = Some(reason.clone());
        }
        if game.is_some() || reason.is_some() {
            let _ = write_meta(&sh.cfg, meta);
        }
        if let Some(reason) = reason {
            let observation = json!({
                "job_id": id, "job_name": meta.name, "reason": reason, "pid": pgid, "game_pid": game,
                "mem_limit": limit, "pressure": pressure, "samples": samples.recent,
                "last_footprint_bytes": meta.last_footprint_bytes,
                "peak_footprint_bytes": meta.peak_footprint_bytes,
                "elapsed_ms": elapsed, "commit": meta.commit,
            });
            // Prevent another step from starting while collecting diagnostics.
            inner.active.as_mut().unwrap().stop = true;
            drop(inner);
            eprintln!("lab: stopping job {id}: {reason}");
            stop_and_report(sh, id, pgid, game, observation);
            return Some(reason);
        }
        drop(inner);
        match rx.recv_timeout(INTERVAL) {
            Ok(Event::Step) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Ok(Event::Finished) | Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn stop_and_report(
    sh: &Shared,
    id: &str,
    pgid: Option<i32>,
    game: Option<u32>,
    mut observation: Value,
) {
    let dir = sh.cfg.dir("runs").join(id).join("mem");
    let report = dir.join("lab-host.json");
    if let Some(pid) = game {
        if let Err(e) = request_game_report(sh, id) {
            observation["agent_error"] = format!("{e:#}").into();
            if group_is_active(sh, id, pgid) {
                observation["diagnostics"] = json!(mem_watch::diagnostics(pid));
            }
        }
    }
    // The process group may already have exited (or the game may have
    // hit its own hard limit). Signal only while this job still owns it.
    let inner = sh.inner.lock().unwrap();
    if let Some(a) = inner.active.as_ref().filter(|a| a.id == id)
        && let Some(group) = a.pgid
    {
        // SAFETY: step() creates a separate process group for this job.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    drop(inner);
    if let Err(e) =
        fs::create_dir_all(&dir).and_then(|_| fs::write(&report, observation.to_string()))
    {
        eprintln!("lab: writing {}: {e}", report.display());
    }
}

fn group_is_active(sh: &Shared, id: &str, pgid: Option<i32>) -> bool {
    sh.inner
        .lock()
        .unwrap()
        .active
        .as_ref()
        .is_some_and(|a| a.id == id && a.pgid == pgid)
}

fn request_game_report(sh: &Shared, id: &str) -> Result<()> {
    let socket = agent_socket_path(&sh.cfg, id);
    let reports = sh.cfg.dir("runs").join(id).join("mem");
    // The socket can be unresponsive during the failure. Bound the whole
    // operation as well as reads, including a full Unix socket listen backlog.
    let timeout = Duration::from_secs(5);
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut client = agent_proto::Client::connect(&socket)?;
            client.set_timeout(Some(timeout))?;
            let result =
                client.call(agent_proto::method::HOST_MEMORY, json!({ "report": true }))?;
            let snapshot: agent_proto::host_memory::MemoryResult = serde_json::from_value(result)?;
            let path = snapshot
                .reports
                .last()
                .context("game returned no report path")?;
            anyhow::ensure!(
                path.parent() == Some(reports.as_path()) && path.is_file(),
                "game did not leave its report in {}",
                reports.display()
            );
            Ok(())
        })();
        let _ = tx.send(result);
    });
    rx.recv_timeout(timeout)
        .context("requesting the game's memory report")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_record_keeps_last_and_peak_across_samples_and_failed_reads() {
        let mut record: Meta = serde_json::from_value(json!({"id": "0007"})).unwrap();
        assert_eq!(record.last_footprint_bytes, None);
        let mut samples = Samples::default();
        let mut readings = [Some(11), Some(12), None, Some(9), Some(13)].into_iter();
        let mut read = |pid| {
            assert_eq!(pid, 123);
            readings.next().unwrap()
        };
        for t in [0, 2000, 4000, 6000] {
            assert_eq!(samples.sample(&mut record, 123, t, 12, &mut read), None);
        }
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["peak_footprint_bytes"], 12);
        assert_eq!(json["last_footprint_bytes"], 9);
        assert_eq!(samples.recent.len(), 3);
        assert_eq!(
            samples.sample(&mut record, 123, 8000, 12, read).as_deref(),
            Some("memory limit: 13 over 12 bytes")
        );
        assert_eq!(record.peak_footprint_bytes, Some(13));
    }

    #[test]
    fn disabling_job_cap_still_records_footprint() {
        let mut record = Meta::default();
        assert_eq!(
            Samples::default().sample(&mut record, 1, 0, 0, |_| Some(u64::MAX)),
            None
        );
        assert_eq!(record.last_footprint_bytes, Some(u64::MAX));
    }
}
