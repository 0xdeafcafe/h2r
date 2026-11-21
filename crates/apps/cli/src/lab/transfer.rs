//! Bulk transfers after a JSON request on the same lab connection.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use anyhow::{Context, Result, ensure};

use super::proto;

const CHUNK: usize = 64 << 10;

#[derive(Default)]
pub(super) struct Transfers {
    state: Mutex<TransferState>,
    finished: Condvar,
}

#[derive(Default)]
struct TransferState {
    active: usize,
    draining: bool,
}

impl Transfers {
    pub(super) fn begin(&self) -> Result<Transfer<'_>> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.draining, "the lab host is restarting");
        state.active += 1;
        Ok(Transfer(self))
    }

    /// Stop admitting transfers and wait for every admitted request to finish
    /// its transfer. Called once the replacement is installed and restart follows.
    pub(super) fn drain(&self) {
        let mut state = self.state.lock().unwrap();
        state.draining = true;
        drop(
            self.finished
                .wait_while(state, |state| state.active != 0)
                .unwrap(),
        );
    }
}

pub(super) struct Transfer<'a>(&'a Transfers);

impl Drop for Transfer<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.active -= 1;
        if state.active == 0 {
            self.0.finished.notify_all();
        }
    }
}

pub(super) struct TempFile {
    pub file: File,
    pub path: PathBuf,
}

impl TempFile {
    pub fn new(dir: &Path) -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("h2-lab-{}-{n}.tmp", std::process::id()));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => return Ok(Self { file, path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
            }
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// A tar stream uses bounded frames and a zero-length terminator so the
/// host can report tar's exit status after delivering the data.
pub(super) fn send_archive(mut src: impl Read, mut dst: impl Write) -> Result<()> {
    let mut buf = [0; CHUNK];
    loop {
        let n = src.read(&mut buf)?;
        dst.write_all(&(n as u32).to_be_bytes())?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n])?;
    }
}

pub(super) fn receive_archive(mut src: impl Read, mut dst: impl Write) -> Result<()> {
    let mut buf = [0; CHUNK];
    loop {
        let mut len = [0; 4];
        src.read_exact(&mut len)?;
        let n = u32::from_be_bytes(len) as usize;
        ensure!(n <= CHUNK, "invalid archive frame size");
        if n == 0 {
            return Ok(());
        }
        src.read_exact(&mut buf[..n])?;
        dst.write_all(&buf[..n])?;
    }
}

#[derive(Clone, Copy)]
pub(super) enum Artifacts {
    Run,
    Memory,
}

impl Artifacts {
    pub(super) fn member(self, id: &str) -> PathBuf {
        let run = PathBuf::from(id);
        match self {
            Self::Run => run,
            Self::Memory => run.join("mem"),
        }
    }

    pub(super) fn command(self) -> &'static str {
        match self {
            Self::Run => "fetch",
            Self::Memory => "mem",
        }
    }
}

/// Extract a numeric run into private staging with the platform tar's normal
/// path protections. Publish regular files only, after checking the whole tree.
pub(super) fn extract<T>(
    archive: &Path,
    to: &Path,
    id: &str,
    artifacts: Artifacts,
    inspect: impl FnOnce(&Path) -> T,
) -> Result<(PathBuf, T)> {
    proto::check_job_id(id)?;
    let member = artifacts.member(id);
    let stage = to.join(format!(".fetch-{}", live_proto::sign::random_id()));
    fs::DirBuilder::new().mode(0o700).create(&stage)?;
    let result = (|| {
        let status = Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["--no-same-owner", "--no-same-permissions"])
            .arg("-C")
            .arg(&stage)
            .arg("-xf")
            .arg(archive)
            .arg("--")
            .arg(&member)
            .status()
            .context("extracting lab archive")?;
        ensure!(
            status.success(),
            "extracting runs/{id} failed with {status}"
        );
        let run = PathBuf::from(id);
        let mut pending = vec![run.clone()];
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        while let Some(dir) = pending.pop() {
            let src = stage.join(&dir);
            ensure!(
                fs::symlink_metadata(&src)?.is_dir(),
                "expected a run directory"
            );
            for item in fs::read_dir(src)? {
                let item = item?;
                let relative = dir.join(item.file_name());
                let meta = fs::symlink_metadata(item.path())?;
                if meta.is_dir() {
                    pending.push(relative);
                } else {
                    ensure!(
                        meta.is_file() && meta.nlink() == 1,
                        "expected a regular run artifact"
                    );
                    files.push(relative);
                }
            }
            dirs.push(dir);
        }
        // Analyze this fetch's validated files, even if the destination
        // contains artifacts from an earlier fetch of the same numeric ID.
        let inspected = inspect(&stage.join(&member));
        for dir in dirs {
            let path = to.join(dir);
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    ensure!(
                        fs::symlink_metadata(path)?.is_dir(),
                        "expected a destination directory"
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }
        for file in files {
            // Copy data alone into a private file. Rename replaces any old
            // artifact without following its symlink or preserving archive modes.
            let path = to.join(&file);
            let mut out = TempFile::new(path.parent().expect("under the run directory"))?;
            io::copy(&mut File::open(stage.join(file))?, &mut out.file)?;
            fs::rename(&out.path, path)?;
        }
        Ok((to.join(member), inspected))
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

#[cfg(test)]
fn extract_run(archive: &Path, to: &Path, id: &str) -> Result<PathBuf> {
    extract(archive, to, id, Artifacts::Run, |_| ()).map(|(path, ())| path)
}

pub(super) fn branch_refs(branch: &str) -> Result<(String, String)> {
    proto::check_rev(branch)?;
    let source = format!("refs/heads/{branch}");
    let dest = format!("refs/heads/lab/{branch}");
    let status = Command::new("git")
        .args(["check-ref-format", &source])
        .status()
        .context("checking git branch name")?;
    ensure!(status.success(), "invalid git branch name {branch:?}");
    Ok((source, dest))
}

pub(super) fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    command
}

pub(super) fn resolve_commit(repo: &Path, rev: &str) -> Result<Option<String>> {
    let out = git(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .context("resolving git commit")?;
    if !out.status.success() {
        return Ok(None);
    }
    let commit = String::from_utf8(out.stdout)?.trim().to_owned();
    proto::check_commit(&commit)?;
    Ok(Some(commit))
}

pub(super) fn make_bundle(source: &str, bases: &[String]) -> Result<TempFile> {
    let bundle = TempFile::new(&std::env::temp_dir())?;
    let repo = Path::new(".");
    let mut git = git(repo);
    git.args(["bundle", "create", "-", source]);
    for base in bases {
        proto::check_commit(base)?;
        if resolve_commit(repo, base)?.is_some() {
            git.arg(format!("^{base}"));
        }
    }
    let status = git.stdout(bundle.file.try_clone()?).status()?;
    ensure!(status.success(), "creating git bundle failed");
    Ok(bundle)
}

pub(super) fn ensure_unchecked(repo: &Path, dest: &str) -> Result<()> {
    let out = git(repo)
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()?;
    ensure!(
        out.status.success(),
        "listing host checkout worktrees failed"
    );
    let record = format!("branch {dest}");
    ensure!(
        !out.stdout
            .split(|byte| *byte == 0)
            .any(|line| line == record.as_bytes()),
        "{dest} is checked out on the host"
    );
    Ok(())
}

/// Import objects from the advertised source ref without moving any refs.
pub(super) fn import_bundle(repo: &Path, bundle: &Path, source: &str, commit: &str) -> Result<()> {
    let out = git(repo)
        .args(["bundle", "list-heads"])
        .arg(bundle)
        .arg(source)
        .output()?;
    ensure!(
        out.status.success()
            && String::from_utf8(out.stdout)?.trim() == format!("{commit} {source}"),
        "git bundle does not contain {source} at {commit}"
    );
    let out = git(repo)
        .args(["fetch", "--no-tags", "--no-write-fetch-head"])
        .arg(bundle)
        .arg(source)
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        out.status.success(),
        "importing git bundle: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn fetched_artifacts_stay_in_the_selected_run() {
        let root = std::env::temp_dir().join(live_proto::sign::random_id());
        let source = root.join("source");
        let dest = root.join("dest");
        fs::create_dir_all(source.join("0001")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("0001/meta.json"), "first").unwrap();
        fs::write(source.join("other"), "sibling").unwrap();
        let archive = root.join("artifacts.tar");
        let pack = || {
            assert!(
                Command::new("tar")
                    .arg("-cf")
                    .arg(&archive)
                    .arg("-C")
                    .arg(&source)
                    .args(["0001", "other"])
                    .status()
                    .unwrap()
                    .success()
            )
        };
        pack();
        assert!(extract_run(&archive, &dest, "../outside").is_err());
        let run = extract_run(&archive, &dest, "0001").unwrap();
        assert_eq!(fs::read_to_string(run.join("meta.json")).unwrap(), "first");
        assert!(!dest.join("other").exists());
        fs::write(source.join("0001/meta.json"), "second").unwrap();
        pack();
        extract_run(&archive, &dest, "0001").unwrap();
        assert_eq!(fs::read_to_string(run.join("meta.json")).unwrap(), "second");
        std::os::unix::fs::symlink("meta.json", source.join("0001/link")).unwrap();
        pack();
        assert!(extract_run(&archive, &dest, "0001").is_err());
        assert!(!run.join("link").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memory_artifacts_bring_only_the_mem_directory_and_inspect_this_fetch() {
        let root = std::env::temp_dir().join(live_proto::sign::random_id());
        let source = root.join("source");
        let dest = root.join("dest");
        fs::create_dir_all(source.join("0002/mem")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("0002/run.log"), "log").unwrap();
        fs::write(source.join("0002/mem/mem-7.ndjson"), "{}\n").unwrap();
        let archive = root.join("mem.tar");
        assert!(
            Command::new("tar")
                .arg("-cf")
                .arg(&archive)
                .arg("-C")
                .arg(&source)
                .arg("0002/mem")
                .status()
                .unwrap()
                .success()
        );
        let (path, seen) = extract(&archive, &dest, "0002", Artifacts::Memory, |dir| {
            assert!(dir.ends_with("0002/mem"));
            fs::read_dir(dir).unwrap().count()
        })
        .unwrap();
        assert_eq!(seen, 1);
        assert_eq!(path, dest.join("0002/mem"));
        assert!(path.join("mem-7.ndjson").is_file());
        assert!(!dest.join("0002/run.log").exists());
        assert_eq!(Artifacts::Memory.command(), "mem");
        assert_eq!(Artifacts::Run.member("0002"), PathBuf::from("0002"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_closes_admission_and_waits_for_every_transfer() {
        let transfers = Transfers::default();
        std::thread::scope(|scope| {
            let upload = transfers.begin().unwrap();
            let download = transfers.begin().unwrap();
            let restart = scope.spawn(|| transfers.drain());
            let deadline = Instant::now() + Duration::from_secs(3);
            while !transfers.state.lock().unwrap().draining {
                assert!(Instant::now() < deadline, "restart never closed admission");
                std::thread::yield_now();
            }

            assert!(transfers.begin().is_err());
            assert!(!restart.is_finished());
            drop(upload);
            assert!(!restart.is_finished());
            drop(download);
            restart.join().unwrap();
        });
    }
}
