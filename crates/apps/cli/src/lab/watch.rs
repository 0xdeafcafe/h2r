//! Read the lab feed from disk without taking the host's job or activity lock.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};

use super::host::{Meta, is_final, load_meta, now};
use super::{cache_dir, service};

pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(500);
const INITIAL_TAIL: u64 = 16 << 10;
const LINE_LIMIT: usize = 4 << 10;

pub(super) fn local(args: &[String]) -> Result<()> {
    let root = match args {
        [] => cache_dir().join("lab"),
        [flag, root] if flag == "--root" => PathBuf::from(root),
        _ => bail!("usage: h2 lab host watch [--root DIR]"),
    };
    let mut watch = Watch::new(&root, None)?;
    let stdout = io::stdout();
    let color = stdout.is_terminal();
    let mut out = stdout.lock();
    loop {
        let result = watch.poll(false, &mut |line| Ok(write_line(&mut out, line, color)?));
        if let Err(error) = result {
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
            {
                return Ok(());
            }
            return Err(error);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

pub(super) fn write_line(out: &mut impl Write, line: &str, color: bool) -> io::Result<()> {
    if color && let Some((prefix, rest)) = line.split_once(']') {
        writeln!(out, "\x1b[36m{prefix}]\x1b[0m{rest}")?;
    } else {
        writeln!(out, "{line}")?;
    }
    out.flush()
}

pub(super) struct Watch<'a> {
    root: &'a Path,
    selection: Selection,
}

enum Selection {
    Job(JobFeed),
    Current {
        jobs: BTreeMap<u64, JobFeed>,
        last_seen: Option<u64>,
        logs: Vec<(&'static str, Tail)>,
    },
}

type Emit<'a> = dyn FnMut(&str) -> Result<()> + 'a;

impl<'a> Watch<'a> {
    pub(super) fn new(root: &'a Path, id: Option<&str>) -> Result<Self> {
        let selection = match id {
            Some(id) => {
                let path = root.join("runs").join(id);
                ensure!(
                    fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir())
                        && load_meta(&path)?.is_some(),
                    "no job {id}"
                );
                Selection::Job(JobFeed::new(id.to_owned(), true))
            }
            None => Selection::Current {
                jobs: BTreeMap::new(),
                last_seen: None,
                logs: service::WATCH_LOGS
                    .iter()
                    .map(|&path| (path, Tail::new(true)))
                    .collect(),
            },
        };
        Ok(Self { root, selection })
    }

    /// Returns true when a watch on one named job is finished.
    pub(super) fn poll(&mut self, snapshot: bool, emit: &mut Emit<'_>) -> Result<bool> {
        let runs = self.root.join("runs");
        match &mut self.selection {
            Selection::Job(job) => {
                let dir = runs.join(&job.id);
                let meta = load_meta(&dir)?.with_context(|| format!("no job {}", job.id))?;
                job.poll(&dir, &meta, snapshot, emit)
            }
            Selection::Current {
                jobs,
                last_seen,
                logs,
            } => {
                let attaching = last_seen.is_none();
                let mut finished = Vec::new();
                for (&number, job) in jobs.iter_mut() {
                    let dir = runs.join(&job.id);
                    match load_meta(&dir)? {
                        Some(meta) if !job.poll(&dir, &meta, snapshot, emit)? => {}
                        _ => finished.push(number),
                    }
                }
                for number in finished {
                    jobs.remove(&number);
                }

                let mut latest_finished = None;
                for (number, id) in job_dirs(&runs, *last_seen)? {
                    let dir = runs.join(&id);
                    let Some(meta) = load_meta(&dir)? else {
                        continue;
                    };
                    // Admission publishes metadata in ID order. A directory
                    // without metadata is not yet a job.
                    *last_seen = Some(number);
                    if attaching && is_final(&meta.state) {
                        latest_finished = Some((id, meta));
                        continue;
                    }
                    let mut job = JobFeed::new(id, attaching);
                    if !job.poll(&dir, &meta, snapshot, emit)? {
                        jobs.insert(number, job);
                    }
                }
                if attaching && jobs.is_empty()
                    && let Some((id, meta)) = latest_finished
                {
                    let dir = runs.join(&id);
                    JobFeed::new(id, true).poll(&dir, &meta, snapshot, emit)?;
                }
                for (path, tail) in logs {
                    let label = Path::new(*path).file_name().unwrap().to_string_lossy();
                    tail.poll(&self.root.join(*path), snapshot, &mut |line| {
                        emit(&format!("[host] {label} | {line}"))
                    })?;
                }
                Ok(false)
            }
        }
    }
}

fn job_dirs(runs: &Path, after: Option<u64>) -> Result<Vec<(u64, String)>> {
    let entries = match fs::read_dir(runs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", runs.display())),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        if let Some(id) = entry.file_name().to_str()
            && id.bytes().all(|byte| byte.is_ascii_digit())
            && let Ok(number) = id.parse()
            && after.is_none_or(|after| number > after)
            && entry.file_type()?.is_dir()
        {
            ids.push((number, id.to_owned()));
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Queued,
    AwaitingConsent,
    AwaitingQuit,
    Building,
    AwaitingRunConsent,
    AwaitingRunQuit,
    Running,
    Final,
}

struct JobFeed {
    id: String,
    phase: Option<Phase>,
    build: Tail,
    run: Tail,
    /// The job's `player` notes already shown.
    notes: usize,
}

impl JobFeed {
    fn new(id: String, attaching: bool) -> Self {
        Self {
            id,
            phase: None,
            build: Tail::new(attaching),
            run: Tail::new(attaching),
            notes: 0,
        }
    }

    fn poll(&mut self, dir: &Path, meta: &Meta, snapshot: bool, emit: &mut Emit<'_>) -> Result<bool> {
        if self.phase == Some(Phase::Final) {
            return Ok(true);
        }
        let finished = is_final(&meta.state);
        let running = meta.state == "running";
        let elapsed = || now().saturating_sub(meta.created) as f64;
        self.advance(Phase::Queued, "queued", meta, 0.0, emit)?;
        // What the player chose, or why the job waits for their game.
        for note in meta.player.iter().skip(self.notes) {
            emit(&format!("[{}] {}", self.id, short(note, 200)))?;
        }
        self.notes = self.notes.max(meta.player.len());
        let building = meta.state == "building";
        let built_or_building = building || meta.build_elapsed_s.is_some();
        if !built_or_building {
            self.show_consent(meta, false, elapsed(), emit)?;
        }
        if built_or_building {
            self.advance(
                Phase::Building,
                "building",
                meta,
                meta.build_elapsed_s.unwrap_or_else(elapsed),
                emit,
            )?;
        }
        if self.phase != Some(Phase::Running)
            && (built_or_building || running || finished)
        {
            self.build.poll(
                &dir.join("build.log"),
                snapshot || !building,
                &mut |line| emit(&format!("[{}] build | {line}", self.id)),
            )?;
        }
        if running || finished {
            self.build.file = None;
        }
        if built_or_building {
            self.show_consent(meta, true, elapsed(), emit)?;
        }
        if running || meta.run_elapsed_s.is_some() {
            self.advance(
                Phase::Running,
                "running",
                meta,
                meta.run_elapsed_s.unwrap_or_else(elapsed),
                emit,
            )?;
        }
        if running || finished {
            self.run.poll(
                &dir.join("run.log"),
                snapshot || finished,
                &mut |line| emit(&format!("[{}] run | {line}", self.id)),
            )?;
        }
        if finished {
            self.run.file = None;
            let state = match meta.state.as_str() {
                "done" => "finished",
                "stopped" => "cancelled",
                "skipped" => "skipped",
                _ => "failed",
            };
            self.advance(
                Phase::Final,
                state,
                meta,
                meta.duration_s.unwrap_or_else(elapsed),
                emit,
            )?;
        }
        Ok(finished)
    }

    fn show_consent(
        &mut self,
        meta: &Meta,
        after_build: bool,
        elapsed: f64,
        emit: &mut Emit<'_>,
    ) -> Result<()> {
        let phase = match (meta.state.as_str(), after_build) {
            ("awaiting_consent", false) => Phase::AwaitingConsent,
            ("awaiting_quit", false) => Phase::AwaitingQuit,
            ("awaiting_consent", true) => Phase::AwaitingRunConsent,
            ("awaiting_quit", true) => Phase::AwaitingRunQuit,
            _ => return Ok(()),
        };
        self.advance(phase, &meta.state, meta, elapsed, emit)
    }

    fn advance(
        &mut self,
        phase: Phase,
        state: &str,
        meta: &Meta,
        elapsed: f64,
        emit: &mut Emit<'_>,
    ) -> Result<()> {
        if self.phase.as_ref().is_some_and(|seen| seen >= &phase) {
            return Ok(());
        }
        let level = short(meta.level.as_deref().unwrap_or("-"), 60);
        let commit: String = clean_line(meta.commit.as_deref().unwrap_or("current"))
            .chars()
            .take(12)
            .collect();
        let note = short(meta.request["note"].as_str().unwrap_or("-"), 100);
        let data = if meta.request["own_data"].as_bool() == Some(true) {
            " data=own"
        } else {
            ""
        };
        let reason = if phase == Phase::Final {
            meta.error.as_deref().or(meta.skip_reason.as_deref())
                .map_or_else(String::new, |reason| format!(" | {}", short(reason, 120)))
        } else {
            String::new()
        };
        emit(&format!(
            "[{}] {state} {} level={level} commit={commit} {elapsed:.1}s note={note}{data}{reason}",
            self.id,
            short(&meta.kind, 12)
        ))?;
        self.phase = Some(phase);
        Ok(())
    }
}

fn short(text: &str, max: usize) -> String {
    let clean = clean_line(text);
    match clean.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &clean[..end]),
        None => clean,
    }
}

/// A tail remembers the open inode and offset. On rotation drain the old
/// file first; a line can straddle the rename, so keep any partial line.
struct Tail {
    file: Option<File>,
    offset: u64,
    pending: Vec<u8>,
    attaching: bool,
}

impl Tail {
    fn new(attaching: bool) -> Self {
        Self {
            file: None,
            offset: 0,
            pending: Vec::new(),
            attaching,
        }
    }

    fn poll(&mut self, path: &Path, flush: bool, emit: &mut Emit<'_>) -> Result<()> {
        let next = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => {
                let meta = file.metadata()?;
                ensure!(meta.is_file(), "{} is not a log file", path.display());
                Some((file, meta))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
        };
        if let Some(file) = &self.file {
            let old = file.metadata()?;
            self.drain(old.len(), emit)?;
            if let Some((file, meta)) = next
                && (old.dev(), old.ino()) != (meta.dev(), meta.ino())
            {
                self.file = Some(file);
                self.offset = 0;
                self.drain(meta.len(), emit)?;
            }
        } else if let Some((file, meta)) = next {
            self.file = Some(file);
            if self.attaching {
                self.offset = meta.len().saturating_sub(INITIAL_TAIL);
            }
            self.drain(meta.len(), emit)?;
        } else {
            self.attaching = false;
        }
        if flush && !self.pending.is_empty() {
            self.emit_pending(emit)?;
        }
        Ok(())
    }

    fn drain(&mut self, len: u64, emit: &mut Emit<'_>) -> Result<()> {
        if len < self.offset {
            self.offset = 0;
            self.pending.clear();
        }
        let file = self.file.as_mut().unwrap();
        file.seek(SeekFrom::Start(self.offset))?;
        let mut input = file.take(len - self.offset);
        let mut buf = [0; 8192];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            self.offset += n as u64;
            for &byte in &buf[..n] {
                if byte == b'\n' || self.pending.len() == LINE_LIMIT {
                    emit(&clean_line(&String::from_utf8_lossy(&self.pending)))?;
                    self.pending.clear();
                }
                if byte != b'\n' {
                    self.pending.push(byte);
                }
            }
        }
        Ok(())
    }

    fn emit_pending(&mut self, emit: &mut Emit<'_>) -> Result<()> {
        emit(&clean_line(&String::from_utf8_lossy(&self.pending)))?;
        self.pending.clear();
        Ok(())
    }
}

fn clean_line(line: &str) -> String {
    let mut clean = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.next() {
                Some('[') => {
                    // CSI, including SGR color sequences.
                    while chars.next().is_some_and(|ch| !('@'..='~').contains(&ch)) {}
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    // Terminal strings end at BEL or ESC backslash.
                    let mut escape = false;
                    for ch in chars.by_ref() {
                        if ch == '\x07' || (escape && ch == '\\') {
                            break;
                        }
                        escape = ch == '\x1b';
                    }
                }
                _ => {}
            },
            '\n' | '\r' | '\t' => clean.push(' '),
            ch if !ch.is_control() => clean.push(ch),
            _ => {}
        }
    }
    clean.truncate(clean.trim_end().len());
    clean
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "h2lab-watch-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("creating {}: {e}", path.display()),
                }
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn save(root: &Path, meta: &Meta) -> PathBuf {
        let dir = root.join("runs").join(&meta.id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), serde_json::to_vec(meta).unwrap()).unwrap();
        dir
    }

    fn meta(id: &str, state: &str) -> Meta {
        Meta {
            id: id.into(),
            kind: "run".into(),
            state: state.into(),
            level: Some("zanzibar".into()),
            commit: Some("abcdef012345abcdef012345abcdef012345abcd".into()),
            request: json!({"note": "GPU check\nwide window"}),
            created: 100,
            ..Meta::default()
        }
    }

    fn poll(watch: &mut Watch<'_>, lines: &mut Vec<String>) -> bool {
        watch
            .poll(false, &mut |line| {
                lines.push(line.into());
                Ok(())
            })
            .unwrap()
    }

    #[test]
    fn job_transitions_keep_their_order_between_polls() {
        let scratch = Scratch::new();
        let mut m = meta("0007", "queued");
        let dir = save(&scratch.0, &m);
        let mut watch = Watch::new(&scratch.0, Some("0007")).unwrap();
        let mut lines = Vec::new();
        assert!(!poll(&mut watch, &mut lines));
        assert!(!poll(&mut watch, &mut lines));

        m.state = "building".into();
        m.build_elapsed_s = Some(2.0);
        save(&scratch.0, &m);
        fs::write(dir.join("build.log"), b"building image\n").unwrap();
        fs::write(dir.join("run.log"), b"playing").unwrap();
        assert!(!poll(&mut watch, &mut lines));
        assert_eq!(lines.len(), 3, "running output waited for metadata");

        // The run starts and ends before the next poll.
        m.state = "done".into();
        m.run_elapsed_s = Some(5.0);
        m.duration_s = Some(6.5);
        save(&scratch.0, &m);
        assert!(poll(&mut watch, &mut lines));
        assert!(poll(&mut watch, &mut lines));
        assert_eq!(
            lines,
            [
                "[0007] queued run level=zanzibar commit=abcdef012345 0.0s note=GPU check wide window",
                "[0007] building run level=zanzibar commit=abcdef012345 2.0s note=GPU check wide window",
                "[0007] build | building image",
                "[0007] running run level=zanzibar commit=abcdef012345 5.0s note=GPU check wide window",
                "[0007] run | playing",
                "[0007] finished run level=zanzibar commit=abcdef012345 6.5s note=GPU check wide window",
            ]
        );
    }

    #[test]
    fn job_transitions_show_failure_and_cancellation() {
        let scratch = Scratch::new();
        for (disk, visible) in [("failed", "failed"), ("stopped", "cancelled")] {
            let mut m = meta("0010", disk);
            m.kind = "build".into();
            m.level = None;
            m.request = json!({});
            m.duration_s = Some(1.0);
            m.error = Some("exit 1".into());
            save(&scratch.0, &m);
            let mut watch = Watch::new(&scratch.0, Some("0010")).unwrap();
            let mut lines = Vec::new();
            assert!(poll(&mut watch, &mut lines));
            assert_eq!(
                lines.last().unwrap(),
                &format!("[0010] {visible} build level=- commit=abcdef012345 1.0s note=- | exit 1")
            );
        }
    }

    #[test]
    fn tail_resumes_after_growth_and_rotation() {
        let scratch = Scratch::new();
        let path = scratch.0.join("log");
        fs::write(&path, b"first\n\x1b[32mpar").unwrap();
        let mut tail = Tail::new(true);
        let mut lines = Vec::new();
        let mut emit = |line: &str| {
            lines.push(line.to_owned());
            Ok(())
        };
        tail.poll(&path, false, &mut emit).unwrap();
        tail.poll(&path, false, &mut emit).unwrap();
        let mut append = OpenOptions::new().append(true).open(&path).unwrap();
        append.write_all(b"tial\x1b[0m\nrota").unwrap();
        tail.poll(&path, false, &mut emit).unwrap();
        fs::rename(&path, path.with_extension("1")).unwrap();
        append.write_all(b"ted").unwrap();
        fs::write(&path, b" line\r\nthird\n").unwrap();
        tail.poll(&path, false, &mut emit).unwrap();
        assert_eq!(lines, ["first", "partial", "rotated line", "third"]);
    }

    #[test]
    fn current_drains_finished_jobs_before_discovering_new_ones() {
        let scratch = Scratch::new();
        let mut active = meta("9999", "running");
        active.run_elapsed_s = Some(2.0);
        let run = save(&scratch.0, &active);
        let host_log = scratch.0.join(service::WATCH_LOGS[0]);
        fs::create_dir_all(host_log.parent().unwrap()).unwrap();
        fs::write(&host_log, b"listening\n").unwrap();
        save(&scratch.0, &meta("0001", "done"));

        let mut watch = Watch::new(&scratch.0, None).unwrap();
        let mut lines = Vec::new();
        assert!(!poll(&mut watch, &mut lines));
        assert!(!lines.iter().any(|line| line.starts_with("[0001]")));
        assert!(lines.last().unwrap().ends_with(" | listening"));

        active.state = "done".into();
        active.duration_s = Some(3.0);
        save(&scratch.0, &active);
        fs::write(run.join("run.log"), b"last frame\n").unwrap();
        let mut next = meta("10000", "building");
        next.build_elapsed_s = Some(1.0);
        let next_dir = save(&scratch.0, &next);
        fs::write(next_dir.join("build.log"), b"new image\n").unwrap();

        lines.clear();
        assert!(!poll(&mut watch, &mut lines));
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "[9999] run | last frame");
        assert!(lines[1].starts_with("[9999] finished"));
        assert!(lines[2].starts_with("[10000] queued"));
        assert!(lines[3].starts_with("[10000] building"));
        assert_eq!(lines[4], "[10000] build | new image");
    }

    #[test]
    fn current_keeps_each_active_job_until_its_own_terminal_state() {
        let scratch = Scratch::new();
        let mut first = meta("0001", "running");
        let first_dir = save(&scratch.0, &first);
        let mut second = meta("0002", "awaiting_consent");
        save(&scratch.0, &second);
        fs::write(first_dir.join("run.log"), b"first frame\n").unwrap();

        let mut watch = Watch::new(&scratch.0, None).unwrap();
        let mut lines = Vec::new();
        assert!(!poll(&mut watch, &mut lines));
        assert!(lines.iter().any(|line| line == "[0001] run | first frame"));
        assert!(lines.iter().any(|line| line.starts_with("[0002] awaiting_consent")));

        // The newer job ends while the first is still running.
        second.state = "skipped".into();
        second.skip_reason = Some("declined by the player".into());
        save(&scratch.0, &second);
        fs::write(first_dir.join("run.log"), b"first frame\nsecond frame\n").unwrap();
        lines.clear();
        assert!(!poll(&mut watch, &mut lines));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0], "[0001] run | second frame");
        assert!(lines[1].starts_with("[0002] skipped"));
        assert!(lines[1].ends_with(" | declined by the player"));

        first.state = "done".into();
        save(&scratch.0, &first);
        save(&scratch.0, &meta("0003", "queued"));
        lines.clear();
        assert!(!poll(&mut watch, &mut lines));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("[0001] finished"));
        assert!(lines[1].starts_with("[0003] queued"));
        lines.clear();
        assert!(!poll(&mut watch, &mut lines));
        assert!(lines.is_empty());
    }

    #[test]
    fn a_job_on_the_players_own_data_says_so() {
        let scratch = Scratch::new();
        let mut own = meta("0001", "queued");
        own.request = serde_json::json!({"own_data": true});
        save(&scratch.0, &own);
        save(&scratch.0, &meta("0002", "queued"));
        let mut watch = Watch::new(&scratch.0, None).unwrap();
        let mut lines = Vec::new();
        poll(&mut watch, &mut lines);
        let line = |id: &str| lines.iter().find(|l| l.starts_with(id)).unwrap().clone();
        assert!(line("[0001]").contains(" data=own"), "{lines:?}");
        assert!(!line("[0002]").contains("data=own"));
    }

    #[test]
    fn a_held_job_says_why_once() {
        let scratch = Scratch::new();
        let mut held = meta("0001", "awaiting_quit");
        save(&scratch.0, &held);
        let mut watch = Watch::new(&scratch.0, None).unwrap();
        let mut lines = Vec::new();
        poll(&mut watch, &mut lines);
        // Back in the queue, with the reason the lab couldn't close the game.
        held.state = "queued".into();
        held.player = vec!["player: allowed the job, but the lab could not close their game".into()];
        save(&scratch.0, &held);
        lines.clear();
        poll(&mut watch, &mut lines);
        assert_eq!(
            lines,
            ["[0001] player: allowed the job, but the lab could not close their game"]
        );
        lines.clear();
        poll(&mut watch, &mut lines);
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn feed_adds_color_only_when_requested() {
        let line = "[0007] run | ready";
        let mut plain = Vec::new();
        let mut color = Vec::new();
        write_line(&mut plain, line, false).unwrap();
        write_line(&mut color, line, true).unwrap();
        assert_eq!(plain, b"[0007] run | ready\n");
        assert_eq!(color, b"\x1b[36m[0007]\x1b[0m run | ready\n");
    }
}
