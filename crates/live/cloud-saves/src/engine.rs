//! Sync at sign-in, pushes while the game runs.
//!
//! - [`Engine::sync`], before the game starts: the server's list, this
//!   machine's items and the base go through [`merge::plan`], and each step
//!   is carried out. Conflict copies are written before anything is
//!   replaced, and a step whose copy cannot be made is not carried out.
//! - [`Engine::push`], while the game runs: only uploads and deletions of
//!   the containers that changed, each against the version both sides last
//!   agreed. Nothing is written into U: under a running game; a container
//!   that changed on another machine meanwhile is left for the next sync.
//! - Offline, pushes queue (the queue is kept on disk), and the
//!   [`Worker`] tries again later with a growing pause.
//! - A damaged meta file (an empty or torn `SaveMeta.xbx` or
//!   `TitleMeta.xbx`, [`crate::meta_damaged`]) is never uploaded, and one
//!   downloaded is never written; each is a line in the report.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::local::Local;
use crate::merge::{self, Action, Step, Why};
use crate::remote::{Remote, RemoteError};
use crate::state::{Places, State};
use crate::watch::{self, Debouncer};
use crate::{KIND_SAVE, Key, LocalItem, Sha256, StorageItem, meta_damaged, now_ms};

/// What a sync or push did. `lines` are for stderr, one per change.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
    pub pulled: usize,
    pub pushed: usize,
    /// Deleted here because the server's copy was deleted.
    pub removed: usize,
    /// Deleted on the server because this machine deleted it.
    pub deleted: usize,
    /// Conflict copies kept.
    pub conflicts: usize,
    /// Left for the next sync: the server's copy changed meanwhile.
    pub deferred: usize,
    pub failed: usize,
    /// Damaged meta files not uploaded, or not written here.
    pub damaged: usize,
    pub offline: bool,
    pub unsupported: bool,
    /// Containers waiting for the server.
    pub queued: usize,
}

impl Report {
    /// One line for the end of a sync.
    pub fn summary(&self) -> String {
        if self.unsupported {
            return "this server keeps no cloud saves; saves stay on this machine".into();
        }
        let mut parts = Vec::new();
        for (n, what) in [
            (self.pulled, "downloaded"),
            (self.pushed, "uploaded"),
            (self.removed, "deleted here"),
            (self.deleted, "deleted in the cloud"),
            (self.conflicts, "conflict copies kept"),
            (self.deferred, "left for next time"),
            (self.failed, "failed"),
            (self.damaged, "damaged meta files skipped"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        let done = if parts.is_empty() {
            "up to date".to_string()
        } else {
            parts.join(", ")
        };
        if self.offline {
            format!(
                "{done}; the server cannot be reached, {} waiting to upload",
                self.queued
            )
        } else {
            done
        }
    }
}

pub struct Engine<R: Remote> {
    pub local: Local,
    remote: R,
    places: Places,
    state: State,
    /// Each profile's settings as last seen, to notice changes in play.
    seen_settings: BTreeMap<Key, Sha256>,
}

/// Seconds since 1970 as `YYYYMMDDTHHMMSSZ`, for conflict copies' names.
fn stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

impl<R: Remote> Engine<R> {
    pub fn new(local: Local, remote: R, places: Places) -> io::Result<Self> {
        let state = State::load(&places)?;
        let mut engine = Engine {
            local,
            remote,
            places,
            state,
            seen_settings: BTreeMap::new(),
        };
        engine.seen_settings = engine.settings_hashes();
        Ok(engine)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn places(&self) -> &Places {
        &self.places
    }

    fn save_state(&self, report: &mut Report) {
        if let Err(e) = self.state.save(&self.places) {
            report.failed += 1;
            report.lines.push(format!(
                "cannot keep {}: {e}",
                self.places.state_file().display()
            ));
        }
    }

    /// Copies `bytes` to `conflicts/<stamp>-<side>/...`; its path.
    fn keep_copy(&self, stamp: &str, side: &str, key: &Key, bytes: &[u8]) -> io::Result<PathBuf> {
        let path = self.places.conflict_copy(stamp, side, key);
        std::fs::create_dir_all(path.parent().expect("a copy has a parent"))?;
        std::fs::write(&path, bytes)?;
        Ok(path)
    }

    /// Every container this machine changed since the base, for the queue.
    fn local_changes(&self) -> BTreeSet<Key> {
        let Ok(local) = self.local.scan() else {
            return BTreeSet::new();
        };
        let mut out = BTreeSet::new();
        for k in local.keys().chain(self.state.base.keys()) {
            // Damaged: unchanged, as in the merge (never uploaded).
            if local.get(k).is_some_and(|l| l.damaged) {
                continue;
            }
            let now = local.get(k).map(|l| l.sha256);
            let then = self.state.base.get(k).and_then(|b| b.sha256);
            if now != then {
                out.insert(group_key(k));
            }
        }
        out
    }

    /// The merge at sign-in, before the game reads its saves.
    pub fn sync(&mut self) -> Report {
        let mut report = Report::default();
        let remote = match self.remote.list() {
            Ok(items) => items,
            Err(RemoteError::Unsupported) => {
                report.unsupported = true;
                return report;
            }
            Err(e) => {
                report.offline = true;
                let changed = self.local_changes();
                self.state.queue.extend(changed);
                report.queued = self.state.queue.len();
                report.lines.push(format!("not synced: {e}"));
                self.save_state(&mut report);
                return report;
            }
        };
        let remote: BTreeMap<Key, StorageItem> = remote
            .into_iter()
            .map(|i| (Key::new(&i.kind, &i.name), i))
            .collect();
        let local = match self.local.scan() {
            Ok(l) => l,
            Err(e) => {
                report.failed += 1;
                report
                    .lines
                    .push(format!("not synced: cannot read {}: {e}", self.local.udata.display()));
                return report;
            }
        };
        let steps = merge::plan(&self.state.base, &local, &remote);
        let stamp = stamp(now_ms());
        let mut queue = BTreeSet::new();
        for step in &steps {
            self.apply(step, &local, &remote, &stamp, &mut queue, &mut report);
        }
        self.state.queue = queue;
        report.queued = self.state.queue.len();
        self.seen_settings = self.settings_hashes();
        self.save_state(&mut report);
        report
    }

    fn apply(
        &mut self,
        step: &Step,
        local: &BTreeMap<Key, LocalItem>,
        remote: &BTreeMap<Key, StorageItem>,
        stamp: &str,
        queue: &mut BTreeSet<Key>,
        report: &mut Report,
    ) {
        let key = &step.key;
        let r = remote.get(key);
        let r_version = r.map_or(0, |r| r.version);
        let r_hash = r.filter(|r| !r.deleted).and_then(|r| r.sha256);
        let needs_server = !matches!(step.action, Action::Agree | Action::Leave);
        if needs_server && report.offline {
            if matches!(step.action, Action::Push { .. } | Action::DeleteRemote { .. }) {
                queue.insert(group_key(key));
            }
            return;
        }
        let why = match step.why {
            Why::Same => "",
            Why::RemoteChanged => "changed on another machine",
            Why::LocalChanged => "changed here",
            Why::ChangeBeatDeletion => "a change beats a deletion",
            Why::RemoteNewer => "CONFLICT: changed on both, the other machine's is newer",
            Why::LocalNewer => "CONFLICT: changed on both, this machine's is newer",
            Why::ContainerFollows => "CONFLICT: its save was decided as a whole",
            Why::LocalDamaged => "damaged here, replaced by the cloud's copy",
        };
        match step.action {
            Action::Agree => self.state.agree(key, r_version, r_hash),
            Action::Leave => {
                report.damaged += 1;
                report.lines.push(format!(
                    "{key}: damaged here (empty or torn), and the cloud has no copy; not uploaded"
                ));
            }
            Action::Pull { keep_local } => {
                let r = r.expect("a pull has a server item");
                if keep_local {
                    let kept = self
                        .local
                        .read(key)
                        .and_then(|b| self.keep_copy(stamp, "this-machine", key, &b.unwrap_or_default()));
                    match kept {
                        Ok(path) => {
                            report.conflicts += 1;
                            report.lines.push(format!(
                                "{key}: {why}; this machine's copy kept in {}",
                                path.display()
                            ));
                        }
                        Err(e) => {
                            report.failed += 1;
                            report
                                .lines
                                .push(format!("{key}: not downloaded, its copy could not be kept: {e}"));
                            return;
                        }
                    }
                }
                let bytes = match self.remote.get(key) {
                    Ok(b) => b,
                    Err(e) => return self.network_failed(key, &e, false, queue, report),
                };
                if Some(Sha256::of(&bytes)) != r_hash {
                    report.failed += 1;
                    report
                        .lines
                        .push(format!("{key}: not downloaded: the bytes do not match their hash"));
                    return;
                }
                if meta_damaged(&key.name, &bytes) {
                    report.damaged += 1;
                    report.lines.push(format!(
                        "{key}: not downloaded: the cloud's copy is damaged (empty or torn)"
                    ));
                    // This machine's good copy mends the cloud's.
                    if let Some(l) = local.get(key).filter(|l| !l.damaged) {
                        self.upload(key, l, r_version, "the cloud's copy was damaged", queue, report);
                    }
                    return;
                }
                match self.local.write(key, &bytes, r.modified) {
                    Ok(()) => {
                        self.state.agree(key, r_version, r_hash);
                        report.pulled += 1;
                        if !keep_local {
                            report.lines.push(format!("{key}: downloaded ({why})"));
                        }
                    }
                    Err(e) => {
                        report.failed += 1;
                        report.lines.push(format!("{key}: not written: {e}"));
                    }
                }
            }
            Action::RemoveLocal => {
                let kept = self
                    .local
                    .read(key)
                    .and_then(|b| self.keep_copy(stamp, "deleted-elsewhere", key, &b.unwrap_or_default()));
                let result = kept.and_then(|path| self.local.remove(key).map(|()| path));
                match result {
                    Ok(path) => {
                        self.state.agree(key, r_version, None);
                        report.removed += 1;
                        report.lines.push(format!(
                            "{key}: deleted here, as on another machine (a copy is in {})",
                            path.display()
                        ));
                    }
                    Err(e) => {
                        report.failed += 1;
                        report.lines.push(format!("{key}: not deleted here: {e}"));
                    }
                }
            }
            Action::Push {
                if_match,
                keep_remote,
            } => {
                if keep_remote {
                    let kept = self
                        .remote
                        .get(key)
                        .map_err(|e| e.to_string())
                        .and_then(|b| {
                            self.keep_copy(stamp, "other-machine", key, &b)
                                .map_err(|e| e.to_string())
                        });
                    match kept {
                        Ok(path) => {
                            report.conflicts += 1;
                            report.lines.push(format!(
                                "{key}: {why}; the other machine's copy kept in {}",
                                path.display()
                            ));
                        }
                        Err(e) => {
                            report.failed += 1;
                            report
                                .lines
                                .push(format!("{key}: not uploaded, the other copy could not be kept: {e}"));
                            return;
                        }
                    }
                }
                let Some(l) = local.get(key) else { return };
                self.upload(key, l, if_match, &why_line(why, keep_remote), queue, report);
            }
            Action::DeleteRemote { if_match } => self.delete(key, if_match, queue, report),
        }
    }

    fn upload(
        &mut self,
        key: &Key,
        l: &LocalItem,
        if_match: u64,
        why: &str,
        queue: &mut BTreeSet<Key>,
        report: &mut Report,
    ) {
        let bytes = match self.local.read(key) {
            Ok(Some(b)) => b,
            Ok(None) => return,
            Err(e) => {
                report.failed += 1;
                report.lines.push(format!("{key}: not uploaded: {e}"));
                return;
            }
        };
        if meta_damaged(&key.name, &bytes) {
            report.damaged += 1;
            report
                .lines
                .push(format!("{key}: not uploaded: damaged here (empty or torn)"));
            return;
        }
        match self.remote.put(key, &bytes, if_match, l.modified) {
            Ok(item) => {
                self.state.agree(key, item.version, item.sha256);
                report.pushed += 1;
                if !why.is_empty() {
                    report.lines.push(format!("{key}: uploaded ({why})"));
                }
            }
            Err(e) => self.network_failed(key, &e, true, queue, report),
        }
    }

    fn delete(&mut self, key: &Key, if_match: u64, queue: &mut BTreeSet<Key>, report: &mut Report) {
        match self.remote.delete(key, if_match) {
            Ok(tomb) => {
                self.state.agree(key, tomb.version, None);
                report.deleted += 1;
                report.lines.push(format!("{key}: deleted in the cloud (deleted here)"));
            }
            Err(e) => self.network_failed(key, &e, true, queue, report),
        }
    }

    fn network_failed(
        &mut self,
        key: &Key,
        e: &RemoteError,
        push: bool,
        queue: &mut BTreeSet<Key>,
        report: &mut Report,
    ) {
        match e {
            RemoteError::Offline(_) => {
                report.offline = true;
                if push {
                    queue.insert(group_key(key));
                }
            }
            RemoteError::Conflict(_) => {
                report.deferred += 1;
                report.lines.push(format!(
                    "{key}: changed on another machine meanwhile; sorted out at the next sign-in"
                ));
            }
            e => {
                report.failed += 1;
                report.lines.push(format!("{key}: {e}"));
            }
        }
    }

    /// While the game runs: upload what changed in `groups` (containers
    /// from [`watch::container`], settings items) and in the queue. Never
    /// writes into U:.
    pub fn push(&mut self, groups: &[Key]) -> Report {
        let mut report = Report::default();
        let todo: BTreeSet<Key> = groups
            .iter()
            .cloned()
            .chain(std::mem::take(&mut self.state.queue))
            .collect();
        let mut queue = BTreeSet::new();
        for group in todo {
            if report.offline {
                queue.insert(group);
                continue;
            }
            let top = match group.kind.as_str() {
                KIND_SAVE => self.local.actual_top(&group.name),
                _ => group.name.clone(),
            };
            let local = match self.local.scan_under(&group.kind, &top) {
                Ok(l) => l,
                Err(e) => {
                    report.failed += 1;
                    report.lines.push(format!("{group}: cannot read it: {e}"));
                    continue;
                }
            };
            let mut keys: BTreeSet<Key> = local.keys().cloned().collect();
            keys.extend(
                self.state
                    .base
                    .keys()
                    .filter(|k| same_group(k, &group))
                    .cloned(),
            );
            for key in keys {
                if report.offline {
                    queue.insert(group.clone());
                    break;
                }
                let l = local.get(&key);
                let base = self.state.base.get(&key).cloned();
                let base_version = base.as_ref().map_or(0, |b| b.version);
                if l.map(|l| l.sha256) == base.as_ref().and_then(|b| b.sha256) {
                    continue;
                }
                match l {
                    Some(l) => self.upload(&key, l, base_version, "changed here", &mut queue, &mut report),
                    None => self.delete(&key, base_version, &mut queue, &mut report),
                }
            }
        }
        self.state.queue = queue;
        report.queued = self.state.queue.len();
        self.save_state(&mut report);
        report
    }

    fn settings_hashes(&self) -> BTreeMap<Key, Sha256> {
        self.local
            .scan_settings()
            .into_iter()
            .map(|(k, l)| (k, l.sha256))
            .collect()
    }

    /// Settings items that changed since last asked (or since the sync).
    pub fn settings_changed(&mut self) -> Vec<Key> {
        let now = self.settings_hashes();
        let changed = now
            .keys()
            .chain(self.seen_settings.keys())
            .filter(|k| now.get(*k) != self.seen_settings.get(*k))
            .cloned()
            .collect::<BTreeSet<_>>();
        self.seen_settings = now;
        changed.into_iter().collect()
    }
}

fn why_line(why: &str, conflict: bool) -> String {
    if conflict { String::new() } else { why.to_string() }
}

/// The group a key is pushed in: its container, or itself.
fn group_key(k: &Key) -> Key {
    let (kind, name) = k.group();
    Key::new(kind, name)
}

/// Whether `k` belongs to `group` (save containers compare without case,
/// as the VFS does).
fn same_group(k: &Key, group: &Key) -> bool {
    let (kind, name) = k.group();
    kind == group.kind
        && if kind == KIND_SAVE {
            name.eq_ignore_ascii_case(&group.name)
        } else {
            name == group.name
        }
}

/// Timings for [`Worker`].
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// How often it looks at the debouncer and the settings.
    pub tick: Duration,
    pub quiet: Duration,
    pub max_delay: Duration,
    /// The first pause after the server could not be reached; it doubles
    /// up to `retry_max`.
    pub retry: Duration,
    pub retry_max: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            tick: Duration::from_millis(500),
            quiet: watch::QUIET,
            max_delay: watch::MAX_DELAY,
            retry: Duration::from_secs(30),
            retry_max: Duration::from_secs(300),
        }
    }
}

enum Msg {
    Changed(Key),
    Stop,
}

/// The engine on a thread of its own while the game runs: it hears about
/// U:'s changes through [`Worker::observer`], waits for each container
/// to be quiet, pushes, and retries the queue. Stopping (or dropping) it
/// pushes whatever is still waiting.
pub struct Worker<R: Remote + 'static> {
    tx: mpsc::Sender<Msg>,
    join: Option<std::thread::JoinHandle<Engine<R>>>,
}

pub type Log = Box<dyn Fn(&str) + Send>;

impl<R: Remote + 'static> Worker<R> {
    pub fn spawn(mut engine: Engine<R>, config: WorkerConfig, log: Log) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        // Background work: nothing waits on a push but the last one at stop.
        let join = background::spawn("cloud-saves", move || {
            let mut debounce = Debouncer::new(config.quiet, config.max_delay);
            let mut retry = config.retry;
            let mut retry_at: Option<Instant> = None;
            let run = |engine: &mut Engine<R>, groups: &[Key], retry_at: &mut Option<Instant>, retry: &mut Duration| {
                let report = engine.push(groups);
                for l in &report.lines {
                    log(l);
                }
                if report.offline {
                    log(&format!(
                        "the server cannot be reached; {} waiting, trying again in {}s",
                        report.queued,
                        retry.as_secs()
                    ));
                    *retry_at = Some(Instant::now() + *retry);
                    *retry = (*retry * 2).min(config.retry_max);
                } else {
                    *retry_at = None;
                    *retry = config.retry;
                }
            };
            loop {
                match rx.recv_timeout(config.tick) {
                    Ok(Msg::Changed(k)) => {
                        debounce.touch(k, Instant::now());
                        continue;
                    }
                    Ok(Msg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let mut last = debounce.drain();
                        last.extend(engine.settings_changed());
                        if !last.is_empty() || !engine.state.queue.is_empty() {
                            run(&mut engine, &last, &mut retry_at, &mut retry);
                        }
                        return engine;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let now = Instant::now();
                for k in engine.settings_changed() {
                    debounce.touch(k, now);
                }
                let due = debounce.due(now);
                let waiting = !engine.state.queue.is_empty();
                let retry_due = retry_at.is_none_or(|t| now >= t);
                if (!due.is_empty() && retry_at.is_none()) || (waiting && retry_due) {
                    run(&mut engine, &due, &mut retry_at, &mut retry);
                } else if !due.is_empty() {
                    // Offline and not yet time to try: queue them.
                    engine.state.queue.extend(due);
                }
            }
        })?;
        Ok(Worker {
            tx,
            join: Some(join),
        })
    }

    /// The VFS change observer: U:'s changes, to this worker.
    pub fn observer(&self) -> vfs::ChangeObserver {
        let tx = self.tx.clone();
        Box::new(move |c: &vfs::Changed| {
            if let Some(k) = watch::container(c) {
                let _ = tx.send(Msg::Changed(k));
            }
        })
    }

    /// Pushes what is waiting and hands the engine back.
    pub fn stop(mut self) -> Option<Engine<R>> {
        let _ = self.tx.send(Msg::Stop);
        self.join.take()?.join().ok()
    }
}

impl<R: Remote + 'static> Drop for Worker<R> {
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            let _ = self.tx.send(Msg::Stop);
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests;
