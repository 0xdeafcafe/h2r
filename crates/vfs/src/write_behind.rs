//! Host data writes taken off the calling (game) thread: `HostBackend`'s
//! write-behind queue. docs/runtime/storage.md "Write-behind".
//!
//! The backend does every piece of guest-visible bookkeeping on the calling
//! thread and hands only the host `pwrite` to one writer thread per backend.
//! One FIFO thread keeps writes ordered per file (and globally). The caller
//! waits for a path's queued writes before anything that reads or changes
//! that file's host bytes or identity, and sees a pending end of file in
//! the meantime. A failed host write is latched against its path and
//! returned once by the next operation that waits on that path.

use crate::{Error, Result};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, Weak},
    thread::JoinHandle,
};

/// Bytes queued or being written at once. A write that would exceed it
/// waits for the writer to catch up (a single larger write is admitted
/// once the queue is empty), so memory stays bounded.
pub(crate) const MAX_IN_FLIGHT: usize = 4 * 1024 * 1024;

/// `H2_VFS_WRITE_BEHIND=0` (or `off`/`false`) makes host writes
/// synchronous again.
pub(crate) fn enabled_by_env() -> bool {
    enabled(std::env::var("H2_VFS_WRITE_BEHIND").ok().as_deref())
}
pub(crate) fn enabled(value: Option<&str>) -> bool {
    !value.is_some_and(|v| {
        ["0", "off", "false", "no"]
            .iter()
            .any(|off| v.trim().eq_ignore_ascii_case(off))
    })
}

struct Job {
    path: PathBuf,
    file: Arc<File>,
    offset: u64,
    data: Vec<u8>,
}

/// A path with writes queued or in progress.
struct Pending {
    jobs: usize,
    /// The furthest byte any of them writes to.
    end: u64,
}

#[derive(Default)]
pub(crate) struct State {
    jobs: VecDeque<Job>,
    /// Bytes of queued jobs plus the one being written.
    in_flight: usize,
    pending: BTreeMap<PathBuf, Pending>,
    /// First host error per path, until an operation on it takes it.
    errors: BTreeMap<PathBuf, Error>,
    shutdown: bool,
    /// Buffers of landed jobs, reused: a fresh 256 KiB allocation is a
    /// fresh mapping, and faulting it in cost more than the copy.
    spare: Vec<Vec<u8>>,
    /// Capacity held in `spare`, at most [`MAX_IN_FLIGHT`].
    spare_bytes: usize,
    #[cfg(test)]
    test: TestKnobs,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestKnobs {
    /// Fail the next job without writing it.
    pub fail_next: Option<std::io::ErrorKind>,
    /// The writer takes no job while set.
    pub paused: bool,
    /// Sleep before each job, so data is really in flight.
    pub delay: std::time::Duration,
    /// Largest `in_flight` ever seen.
    pub peak_in_flight: usize,
}

pub(crate) struct Queue {
    state: Mutex<State>,
    /// Signalled on every enqueue, completion and shutdown.
    changed: Condvar,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed.wait(guard).unwrap_or_else(|e| e.into_inner())
    }
    #[cfg(test)]
    pub(crate) fn test<R>(&self, f: impl FnOnce(&mut TestKnobs) -> R) -> R {
        let r = f(&mut self.lock().test);
        self.changed.notify_all();
        r
    }
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.lock().in_flight
    }

    fn drain(&self) {
        let mut state = self.lock();
        while !state.pending.is_empty() {
            state = self.wait(state);
        }
    }

    fn run(&self) {
        let mut state = self.lock();
        loop {
            #[cfg(test)]
            while state.test.paused && !state.shutdown {
                state = self.wait(state);
            }
            let Some(job) = state.jobs.pop_front() else {
                if state.shutdown {
                    return;
                }
                state = self.wait(state);
                continue;
            };
            #[cfg(test)]
            let (injected, delay) = (state.test.fail_next.take(), state.test.delay);
            drop(state);
            #[cfg(test)]
            std::thread::sleep(delay);
            #[cfg(test)]
            let result = match injected {
                Some(kind) => Err(std::io::Error::from(kind)),
                None => write_all_at(&job.file, &job.data, job.offset),
            };
            #[cfg(not(test))]
            let result = write_all_at(&job.file, &job.data, job.offset);
            // The last copy of an evicted handle closes here, off the
            // caller's thread.
            drop(job.file);
            state = self.lock();
            state.in_flight -= job.data.len();
            if state.spare_bytes + job.data.capacity() <= MAX_IN_FLIGHT {
                state.spare_bytes += job.data.capacity();
                state.spare.push(job.data);
            }
            let pending = state.pending.get_mut(&job.path).expect("queued path");
            pending.jobs -= 1;
            if pending.jobs == 0 {
                state.pending.remove(&job.path);
            }
            if let Err(e) = result
                && !state.errors.contains_key(&job.path)
            {
                eprintln!(
                    "vfs: write-behind write to {} at {} failed: {e}; reported on the file's next operation",
                    job.path.display(),
                    job.offset
                );
                state.errors.insert(job.path, e.into());
            }
            self.changed.notify_all();
        }
    }
}

#[cfg(unix)]
fn write_all_at(file: &File, data: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, data, offset)
}
#[cfg(not(unix))]
fn write_all_at(mut file: &File, data: &[u8], offset: u64) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    // The writer is the only user of this handle's position while a job
    // for its path is pending; the backend waits before seeking it itself.
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(data)
}

/// Every live writer's queue, for [`drain_live`].
static LIVE: Mutex<Vec<Weak<Queue>>> = Mutex::new(Vec::new());

/// Wait until every live backend's queued writes have landed, leaked
/// backends included: for exit paths (`vfs::flush_pending_metadata`).
pub(crate) fn drain_live() {
    let live: Vec<Arc<Queue>> = LIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for queue in live {
        queue.drain();
    }
}

/// The writer thread and its queue; owned by one `HostBackend`.
pub(crate) struct WriteBehind {
    queue: Arc<Queue>,
    thread: Option<JoinHandle<()>>,
}

impl WriteBehind {
    /// `None` if the writer thread cannot be spawned (logged): the backend
    /// then writes synchronously.
    pub(crate) fn spawn() -> Option<Self> {
        let queue = Arc::new(Queue {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let worker = queue.clone();
        match std::thread::Builder::new()
            .name("vfs-write-behind".into())
            .spawn(move || worker.run())
        {
            Ok(thread) => {
                let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
                live.retain(|q| q.strong_count() > 0);
                live.push(Arc::downgrade(&queue));
                Some(Self {
                    queue,
                    thread: Some(thread),
                })
            }
            Err(e) => {
                eprintln!("vfs: no write-behind thread, writing synchronously: {e}");
                None
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn queue(&self) -> &Arc<Queue> {
        &self.queue
    }

    /// Queue `data` for `offset` of `file` (the host file at `path`),
    /// waiting first while the queue is full.
    pub(crate) fn enqueue(&self, path: &Path, file: Arc<File>, offset: u64, bytes: &[u8]) {
        let q = &self.queue;
        let mut state = q.lock();
        while state.in_flight != 0 && state.in_flight + bytes.len() > MAX_IN_FLIGHT {
            state = q.wait(state);
        }
        state.in_flight += bytes.len();
        #[cfg(test)]
        {
            state.test.peak_in_flight = state.test.peak_in_flight.max(state.in_flight);
        }
        let mut data = match state.spare.pop() {
            Some(spare) => {
                state.spare_bytes -= spare.capacity();
                spare
            }
            None => Vec::new(),
        };
        // Copied outside the lock; the writer cannot see the job yet.
        drop(state);
        data.clear();
        data.extend_from_slice(bytes);
        let mut state = q.lock();
        let end = offset + data.len() as u64;
        state
            .pending
            .entry(path.to_owned())
            .and_modify(|p| {
                p.jobs += 1;
                p.end = p.end.max(end);
            })
            .or_insert(Pending { jobs: 1, end });
        state.jobs.push_back(Job {
            path: path.to_owned(),
            file,
            offset,
            data,
        });
        drop(state);
        q.changed.notify_all();
    }

    /// The end of file `path` will have once its queued writes land, given
    /// its current host length, if any are queued. Look this up before
    /// reading the host length: a write that lands in between is then
    /// still covered by the returned end.
    pub(crate) fn pending_end(&self, path: &Path) -> Option<u64> {
        self.queue.lock().pending.get(path).map(|p| p.end)
    }

    /// Wait until no write to `path` is queued or in progress, then return
    /// (and clear) the error one of them latched.
    pub(crate) fn settle(&self, path: &Path) -> Result<()> {
        self.wait_for(path).errors.remove(path).map_or(Ok(()), Err)
    }

    /// Wait until no write to `path` is queued or in progress.
    pub(crate) fn wait_for(&self, path: &Path) -> MutexGuard<'_, State> {
        let q = &self.queue;
        let mut state = q.lock();
        while state.pending.contains_key(path) {
            state = q.wait(state);
        }
        state
    }

    /// The error latched for `path`, without waiting.
    pub(crate) fn take_error(&self, path: &Path) -> Result<()> {
        self.queue.lock().errors.remove(path).map_or(Ok(()), Err)
    }

    /// Wait until every queued write has landed. Latched errors stay.
    pub(crate) fn drain(&self) {
        self.queue.drain();
    }

    /// Whether anything is queued or being written.
    pub(crate) fn idle(&self) -> bool {
        self.queue.lock().pending.is_empty()
    }

    /// Drop every latched error (the volume they described is gone).
    pub(crate) fn clear_errors(&self) {
        self.queue.lock().errors.clear();
    }

    /// Keep errors for files that may still exist after a partial format.
    pub(crate) fn retain_errors(&self, mut keep: impl FnMut(&Path) -> bool) {
        self.queue.lock().errors.retain(|path, _| keep(path));
    }
}

impl Drop for WriteBehind {
    /// Lands everything still queued, then stops the thread.
    fn drop(&mut self) {
        {
            let mut state = self.queue.lock();
            state.shutdown = true;
            #[cfg(test)]
            {
                state.test.paused = false;
            }
        }
        self.queue.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
