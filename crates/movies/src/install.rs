//! Optional installation using an external, user-owned ffmpeg executable.
use crate::{Freshness, MANIFEST_NAME, Manifest, MovieEntry, Sha256, hex, locate_entry};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use xiso::{Entry, Xiso};

/// The installer accepts an app data directory, never a checkout directory.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub data_dir: PathBuf,
    /// Absolute path or executable name searched on PATH. Never a shell command.
    pub ffmpeg: PathBuf,
}
impl InstallOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ffmpeg: "ffmpeg".into(),
        }
    }
}

/// Default data destination for the native macOS application.
pub fn app_movies_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/h2recomp/movies"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    Complete,
    /// Installation may continue without movies; existing conversions survive.
    FfmpegAbsent,
    /// Installation may continue; show this reason and offer a later retry.
    FfmpegUnusable(String),
}
#[derive(Debug)]
pub struct InstallReport {
    pub status: InstallStatus,
    pub converted: Vec<String>,
    pub reused: Vec<String>,
}

/// Discover every non-directory `.bik`, at any depth, case-insensitively.
pub fn discover<R: Read + Seek>(disc: &mut Xiso<R>) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<_> = disc
        .entries()
        .map_err(io::Error::other)?
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".bik"))
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Open the user's ISO read-only. No source media is ever written back.
pub fn install_iso(iso: &Path, options: &InstallOptions) -> io::Result<InstallReport> {
    let mut disc = Xiso::open(File::open(iso)?).map_err(io::Error::other)?;
    install(&mut disc, options)
}

/// Install or resume. Each successful movie is committed separately. Source
/// hashes are checked even on a resume; outputs need both size and SHA-256.
/// A failed conversion returns an error, preserving earlier manifest entries.
pub fn install<R: Read + Seek>(
    disc: &mut Xiso<R>,
    options: &InstallOptions,
) -> io::Result<InstallReport> {
    let entries = discover(disc)?;
    let ffmpeg = match Ffmpeg::probe(&options.ffmpeg)? {
        Ok(f) => f,
        Err(status) => {
            return Ok(InstallReport {
                status,
                converted: vec![],
                reused: vec![],
            });
        }
    };
    let dir = prepare_dir(&options.data_dir)?;
    // Advisory OS lock survives as a file, but is released on crash/exit. Do not
    // unlink it: that would let concurrent installers lock different inodes.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".install.lock"))?;
    lock.try_lock().map_err(io::Error::other)?;
    let mut manifest = Manifest::load(&dir)?.unwrap_or_default();
    let mut report = InstallReport {
        status: InstallStatus::Complete,
        converted: vec![],
        reused: vec![],
    };
    for entry in entries {
        let mut digest = DigestWriter::default();
        copy_exact(disc, &entry, &mut digest)?;
        let source_hash = hex(&digest.sha.finish());
        if let Some(old) = manifest.find(&entry.path)
            && old.freshness(entry.size as u64, &source_hash) == Freshness::Fresh
            && old.settings == ffmpeg.settings()
            && manifest.output_sha256.contains_key(&old.output_name)
            && locate_entry(&dir, old).is_some()
            && manifest.output_matches(&dir, old)?
        {
            report.reused.push(entry.path);
            continue;
        }
        // Seekable input matters: Bink has indexed frames. Spooling also lets
        // the exact same path exercise this pipeline with generated test media.
        let scratch = Scratch::new(&std::env::temp_dir())?;
        let input = scratch.0.join("source");
        let mut source = File::create(&input)?;
        let mut copied = DigestWriter::default();
        copy_exact(disc, &entry, &mut Tee(&mut source, &mut copied))?;
        drop(source);
        if hex(&copied.sha.finish()) != source_hash {
            return Err(io::Error::other("disc source changed while copying"));
        }
        let output = scratch.0.join("output.mov");
        let duration_secs = ffmpeg.encode(&input, &output)?;
        let output_hash = hash_file(&output)?;
        let output_size = fs::metadata(&output)?.len();
        if output_size == 0 {
            return Err(io::Error::other("ffmpeg produced an empty movie"));
        }
        // A fresh output name avoids replacing a file referenced by the old
        // manifest before the new manifest commits (including after a crash).
        let output_name = format!("movie-{output_hash}.mov");
        // Publication staging is on the destination filesystem, so rename
        // remains atomic even if TMPDIR is on a different volume.
        let publication = Scratch::new(&dir)?;
        let staged_output = publication.0.join("output.mov");
        fs::copy(&output, &staged_output)?;
        File::open(&staged_output)?.sync_all()?;
        fs::rename(&staged_output, dir.join(&output_name))?;
        manifest.upsert(MovieEntry {
            source_name: entry.path.clone(),
            source_size: entry.size as u64,
            source_sha256: source_hash,
            output_name: output_name.clone(),
            output_size,
            duration_secs,
            ffmpeg_version: ffmpeg.version.clone(),
            settings: ffmpeg.settings(),
        });
        manifest.output_sha256.insert(output_name, output_hash);
        let manifest_tmp = publication.0.join(MANIFEST_NAME);
        let mut file = File::create(&manifest_tmp)?;
        file.write_all(manifest.to_json().as_bytes())?;
        file.sync_all()?;
        fs::rename(manifest_tmp, dir.join(MANIFEST_NAME))?;
        File::open(&dir)?.sync_all()?;
        report.converted.push(entry.path);
    }
    Ok(report)
}

fn copy_exact<R: Read + Seek>(
    disc: &mut Xiso<R>,
    entry: &Entry,
    out: &mut impl Write,
) -> io::Result<()> {
    let n = disc.copy_to(entry, out).map_err(io::Error::other)?;
    if n != entry.size as u64 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated ISO movie",
        ));
    }
    Ok(())
}

/// Reject repository destinations, including worktrees and symlink aliases.
/// Callers may supply their platform's app data directory for sandboxed apps.
fn prepare_dir(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Resolve each existing component; reject before creating anything in a repo.
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        resolved.push(component);
        if resolved.exists() {
            resolved = resolved.canonicalize()?;
        }
        if resolved.ancestors().any(|p| p.join(".git").exists()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "movies belong in the app data directory, outside a repository",
            ));
        }
    }
    fs::create_dir_all(&resolved)?;
    resolved.canonicalize()
}

struct Ffmpeg {
    path: PathBuf,
    version: String,
    encoder: &'static str,
}
impl Ffmpeg {
    fn probe(path: &Path) -> io::Result<Result<Self, InstallStatus>> {
        let version = match Command::new(path)
            .arg("-version")
            .stdin(Stdio::null())
            .output()
        {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned(),
            Ok(_) => {
                return Ok(Err(InstallStatus::FfmpegUnusable(
                    "ffmpeg -version failed".into(),
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Err(InstallStatus::FfmpegAbsent));
            }
            Err(e) => return Ok(Err(InstallStatus::FfmpegUnusable(e.to_string()))),
        };
        if !version.starts_with("ffmpeg version") {
            return Ok(Err(InstallStatus::FfmpegUnusable(
                "executable is not ffmpeg".into(),
            )));
        }
        let listing = |flag| -> io::Result<String> {
            let out = Command::new(path)
                .args(["-hide_banner", flag])
                .stdin(Stdio::null())
                .output()?;
            if !out.status.success() {
                return Err(io::Error::other(format!("ffmpeg {flag} failed")));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let capabilities = || -> io::Result<&'static str> {
            let dec = listing("-decoders")?;
            if !lists(&dec, "binkvideo")
                || !lists(&dec, "binkaudio_dct")
                || !lists(&dec, "binkaudio_rdft")
            {
                return Err(io::Error::other(
                    "ffmpeg needs Bink video and audio decoders",
                ));
            }
            let enc = listing("-encoders")?;
            if !lists(&enc, "aac") {
                return Err(io::Error::other("ffmpeg needs an AAC encoder"));
            }
            ["libx264", "h264_videotoolbox", "hevc_videotoolbox"]
                .into_iter()
                .find(|e| lists(&enc, e))
                .ok_or_else(|| io::Error::other("ffmpeg needs an H.264 or HEVC encoder"))
        };
        Ok(match capabilities() {
            Ok(encoder) => Ok(Self {
                path: path.into(),
                version,
                encoder,
            }),
            Err(e) => Err(InstallStatus::FfmpegUnusable(e.to_string())),
        })
    }
    fn settings(&self) -> String {
        format!(
            "{}; {}; yuv420p; aac 192k; mov faststart; v1",
            self.encoder,
            if self.encoder == "libx264" {
                "crf18 slow high"
            } else {
                "4M"
            }
        )
    }
    fn encode(&self, input: &Path, output: &Path) -> io::Result<f64> {
        let mut command = Command::new(&self.path);
        command
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(input)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-sn",
                "-dn",
                "-c:v",
                self.encoder,
            ]);
        if self.encoder == "libx264" {
            command.args(["-preset", "slow", "-crf", "18", "-profile:v", "high"]);
        } else {
            command.args(["-b:v", "4M"]);
        }
        if self.encoder.starts_with("hevc") {
            command.args(["-tag:v", "hvc1"]);
        }
        command
            .args([
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-f",
                "mov",
                "-n",
            ])
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = ChildGuard(command.spawn()?);
        // Drain both pipes concurrently and bound stored diagnostics. A noisy
        // failing process must never deadlock installation or exhaust memory.
        let stderr = child.0.stderr.take().unwrap();
        let log = std::thread::spawn(move || drain(stderr));
        let stdout = child.0.stdout.take().unwrap();
        let progress = drain(stdout);
        let status = child.0.wait()?;
        let log = log
            .join()
            .map_err(|_| io::Error::other("ffmpeg log reader panicked"))??;
        if !status.success() {
            return Err(io::Error::other(format!(
                "ffmpeg failed ({status}): {}",
                String::from_utf8_lossy(&log)
            )));
        }
        let progress = progress?;
        let text = String::from_utf8_lossy(&progress);
        let micros = text
            .lines()
            .filter_map(|l| l.strip_prefix("out_time_us=")?.parse::<u64>().ok())
            .next_back()
            .unwrap_or(0);
        Ok(micros as f64 / 1_000_000.0)
    }
}
fn lists(text: &str, name: &str) -> bool {
    text.lines()
        .any(|l| l.split_whitespace().nth(1) == Some(name))
}
fn drain(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() > 65536 {
            bytes.drain(..bytes.len() - 65536);
        }
    }
    Ok(bytes)
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let p = parent.join(format!(
                ".install-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&p) {
                Ok(()) => return Ok(Self(p)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct DigestWriter {
    sha: Sha256,
}
impl Write for DigestWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sha.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Tee<'a>(&'a mut File, &'a mut DigestWriter);
impl Write for Tee<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.0.write(buf)?;
        self.1.write_all(&buf[..n])?;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub(crate) fn hash_file(path: &Path) -> io::Result<String> {
    let mut hash = DigestWriter::default();
    io::copy(&mut File::open(path)?, &mut hash)?;
    Ok(hex(&hash.sha.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_checkout_and_parent_traversal() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(prepare_dir(&root.join("movie-output")).is_err());
        assert!(prepare_dir(&root.join("../movies/movie-output")).is_err());
        assert!(!root.join("movie-output").exists());
    }
    #[test]
    fn missing_ffmpeg_is_optional() {
        assert!(matches!(
            Ffmpeg::probe(Path::new("/nonexistent-h2-ffmpeg")).unwrap(),
            Err(InstallStatus::FfmpegAbsent)
        ));
    }
}
