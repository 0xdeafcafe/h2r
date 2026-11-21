use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use agent_proto::{Client, RpcError, code, theater as api};
use anyhow::{Context, Result, bail};
use serde_json::json;

use super::controls::LibraryLaunch;

const POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Default)]
pub(super) struct Jobs(Arc<Mutex<HashMap<String, api::PrepareState>>>);

impl Jobs {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, api::PrepareState>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn state(&self, id: &str) -> api::PrepareState {
        self.lock().get(id).cloned().unwrap_or(api::PrepareState {
            id: id.to_owned(),
            tick: 0,
            length_ticks: 0,
            status: api::PrepareStatus::Idle,
        })
    }

    pub(super) fn start(
        &self,
        id: String,
        launch: Arc<LibraryLaunch>,
    ) -> Result<api::PrepareState, RpcError> {
        let mut jobs = self.lock();
        let state = match reserve(&mut jobs, &id) {
            Ok(state) => state,
            Err(active) => return Ok(active),
        };
        let jobs_handle = self.clone();
        let job_id = id.clone();
        if let Err(error) = background::spawn("h2-theater-prepare", move || {
            let result = jobs_handle.replay(&job_id, &launch);
            let mut jobs = jobs_handle.lock();
            let state = jobs.get_mut(&job_id).expect("active Prepare job");
            finish(state, result);
        }) {
            jobs.remove(&id);
            return Err(RpcError::new(code::UNAVAILABLE, error.to_string()));
        }
        Ok(state)
    }

    fn replay(&self, id: &str, launch: &LibraryLaunch) -> Result<()> {
        let entry = launch.entry(id).map_err(|e| anyhow::anyhow!(e.message))?;
        if entry.version < 2 {
            bail!("This film does not have an engine replay track");
        }
        self.lock().get_mut(id).unwrap().length_ticks = entry.meta.length_ticks;

        let stem = std::env::temp_dir().join(format!("h2t-{}", live_proto::sign::random_id()));
        let log_path = stem.with_extension("log");
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let log = options.open(&log_path).context("creating Prepare log")?;
        let output = ReplayFiles {
            socket: stem.with_extension("sock"),
            log: log_path,
        };
        let mut child = launch
            .command("prepare", id, &entry.path, &output.socket)
            .map_err(|e| anyhow::anyhow!(e.message))?
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .context("starting Prepare replay")?;
        let mut client = None;
        let exit = loop {
            if let Some(status) = child.try_wait().context("waiting for Prepare replay")? {
                break status;
            }
            if let Some(state) = progress(&output.socket, &mut client) {
                let mut jobs = self.lock();
                let job = jobs.get_mut(id).expect("active Prepare job");
                if state.id == entry.meta.id {
                    job.tick = job.tick.max(state.tick.min(job.length_ticks));
                    job.status = api::PrepareStatus::Running;
                }
            }
            std::thread::sleep(POLL);
        };
        if !exit.success() {
            if let Some(error) = replay_error(&output.log) {
                bail!("{error}");
            }
            bail!("Prepare replay exited with {exit}");
        }
        // The replay only succeeds after verifying the entire tape and
        // finishing the local sidecar. A short replay can finish between polls.
        Ok(())
    }
}

/// Reserve the job before launching a worker. A duplicate gets the current
/// state, and a request after a terminal state can rebuild the cache.
fn reserve(
    jobs: &mut HashMap<String, api::PrepareState>,
    id: &str,
) -> Result<api::PrepareState, api::PrepareState> {
    if let Some(active) = jobs.get(id).filter(|state| state.status.active()) {
        return Err(active.clone());
    }
    let state = api::PrepareState {
        id: id.to_owned(),
        tick: 0,
        length_ticks: 0,
        status: api::PrepareStatus::Starting,
    };
    jobs.insert(id.to_owned(), state.clone());
    Ok(state)
}

fn finish(state: &mut api::PrepareState, result: Result<()>) {
    state.status = match result {
        Ok(()) => {
            state.tick = state.length_ticks;
            api::PrepareStatus::Complete
        }
        Err(error) => api::PrepareStatus::Failed {
            error: error.to_string(),
        },
    };
}

fn progress(socket: &Path, client: &mut Option<Client>) -> Option<api::State> {
    if client.is_none() {
        let connection = Client::connect(socket).ok()?;
        connection.set_timeout(Some(POLL)).ok()?;
        *client = Some(connection);
    }
    let result = client.as_mut()?.call(api::STATE, json!({}));
    match result {
        Ok(value) => serde_json::from_value(value).ok(),
        Err(_) => {
            *client = None;
            None
        }
    }
}

struct ReplayFiles {
    socket: PathBuf,
    log: PathBuf,
}

impl Drop for ReplayFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.log);
    }
}

fn replay_error(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(8 * 1024)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    String::from_utf8_lossy(&tail)
        .lines()
        .rev()
        .filter_map(|line| line.strip_prefix("Error: "))
        .find(|error| !error.starts_with("film playback exited with"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::PrepareStatus;

    #[test]
    fn duplicate_prepare_reuses_progress_and_a_terminal_job_can_retry() {
        let jobs = Jobs::default();
        let id = "synthetic-film";
        assert_eq!(jobs.state(id).status, PrepareStatus::Idle);
        assert_eq!(
            reserve(&mut jobs.lock(), id).unwrap().status,
            PrepareStatus::Starting
        );
        {
            let mut states = jobs.lock();
            let state = states.get_mut(id).unwrap();
            state.length_ticks = 900;
            state.tick = 630;
            state.status = PrepareStatus::Running;
            assert_eq!(reserve(&mut states, id).unwrap_err().tick, 630);
            finish(
                states.get_mut(id).unwrap(),
                Err(anyhow::anyhow!("diverged at 631")),
            );
        }
        let failed = jobs.state(id);
        assert_eq!(failed.tick, 630);
        assert_eq!(
            failed.status,
            PrepareStatus::Failed {
                error: "diverged at 631".into()
            }
        );

        assert_eq!(reserve(&mut jobs.lock(), id).unwrap().tick, 0);
        {
            let mut states = jobs.lock();
            let state = states.get_mut(id).unwrap();
            state.length_ticks = 900;
            finish(state, Ok(()));
        }
        let complete = jobs.state(id);
        assert_eq!(complete.status, PrepareStatus::Complete);
        assert_eq!(complete.tick, 900);
        assert_eq!(jobs.state("other-film").status, PrepareStatus::Idle);
    }
}
