//! Read archive entries to numbered private scratch files. Archive paths
//! identify members for level matching, but never choose an output path.
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::{Candidate, whole_build};

const MIB: u64 = 1 << 20;
pub(super) const MAX_MAP: u64 = 512 * MIB;
pub(super) const MAX_DISC: u64 = 16 * 1024 * MIB;
pub(super) const MAX_FILES: usize = 512;

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn iso(path: &Path) -> bool {
    matches!(extension(path).as_str(), "iso" | "xiso")
}

fn include(path: &Path) -> bool {
    if path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|s| s.starts_with('.') && s != "." || s == "__MACOSX")
    }) {
        return false;
    }
    matches!(
        extension(path).as_str(),
        "map"
            | "ppf"
            | "ips"
            | "bps"
            | "xdelta"
            | "xdelta3"
            | "vcdiff"
            | "sppf"
            | "serenity"
            | "iso"
            | "xiso"
    )
}

fn folder(root: &Path, dir: &Path, out: &mut Vec<Candidate>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let path = entry.path();
        let logical = path.strip_prefix(root)?.to_owned();
        if ty.is_dir() {
            if logical.components().count() > 48 {
                bail!("too many nested directories at {}", path.display());
            }
            if !entry.file_name().to_string_lossy().starts_with('.') {
                folder(root, &path, out)?;
            }
        } else if ty.is_file() && include(&logical) {
            if out.len() >= MAX_FILES {
                bail!("more than {MAX_FILES} map or patch files");
            }
            out.push(Candidate {
                file: path,
                logical,
            });
        }
    }
    Ok(())
}

// Read the cache type before applying the member cap. A whole-game release
// may carry much larger shared resources; stop the reader immediately there.
fn copy_entry(
    mut source: impl Read,
    out: &mut impl Write,
    limit: u64,
    map: bool,
) -> Result<Option<u64>> {
    let mut count = 0;
    if map {
        let mut head = [0; whole_build::HEADER_BYTES];
        source.read_exact(&mut head)?;
        if !whole_build::is_multiplayer(&head)? {
            return Ok(None);
        }
        out.write_all(&head)?;
        count = head.len() as u64;
    }
    if count <= limit {
        count += io::copy(&mut source.take(limit - count + 1), out)?;
    }
    if count > limit {
        bail!("archive entry exceeds its {} MiB limit", limit / MIB);
    }
    Ok(Some(count))
}

fn stream(mut cmd: Command, out: &mut impl Write, limit: u64, map: bool) -> Result<Option<u64>> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("opening archive with {program}; install it or supply the extracted folder")
        })?;
    let stdout = child.stdout.take().context("opening archive stream")?;
    let written = copy_entry(stdout, out, limit, map);
    if !matches!(written, Ok(Some(_))) {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let size = written?;
    if size.is_some() && !status.success() {
        bail!("archive reader exited with {status}");
    }
    Ok(size)
}

fn reader() -> Command {
    // Both macOS and Windows ship libarchive as tar.
    Command::new(if cfg!(any(windows, target_os = "macos")) {
        "tar"
    } else {
        "bsdtar"
    })
}

#[derive(Clone, Copy)]
enum ArchiveReader {
    Libarchive,
    Unrar,
}

impl ArchiveReader {
    fn list(self, path: &Path) -> Command {
        let mut cmd = match self {
            Self::Libarchive => {
                let mut cmd = reader();
                cmd.arg("-tf");
                cmd
            }
            Self::Unrar => {
                let mut cmd = Command::new("unrar");
                cmd.args(["lb", "-p-", "-c-", "--"]);
                cmd
            }
        };
        cmd.arg(path);
        cmd
    }

    fn extract(self, path: &Path, name: &str) -> Command {
        match self {
            Self::Libarchive => {
                let mut cmd = reader();
                cmd.arg("-xqOf").arg(path).arg("--").arg(name);
                cmd
            }
            Self::Unrar => {
                let mut cmd = Command::new("unrar");
                cmd.args(["p", "-inul", "-p-", "-c-", "--"])
                    .arg(path)
                    .arg(name);
                cmd
            }
        }
    }
}

fn archive(path: &Path, scratch: &Path) -> Result<Vec<Candidate>> {
    if extension(path) == "rar" {
        // libarchive cannot decode every legacy RAR filter. Prefer the
        // format's own reader when installed. Never accept a partial decode.
        let has_unrar = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                dir.join(if cfg!(windows) { "unrar.exe" } else { "unrar" })
                    .is_file()
            })
        });
        let backend = if has_unrar {
            ArchiveReader::Unrar
        } else {
            ArchiveReader::Libarchive
        };
        return read_archive(backend, path, scratch).context(
            "reading RAR: for unsupported RAR compression, install unrar or extract the release with WinRAR and import its folder"
        );
    }
    read_archive(ArchiveReader::Libarchive, path, scratch)
}

fn read_archive(backend: ArchiveReader, path: &Path, scratch: &Path) -> Result<Vec<Candidate>> {
    let list = backend.list(path);
    let mut names = Vec::new();
    stream(list, &mut names, 8 * MIB, false)?;
    let listing = std::str::from_utf8(&names).context("archive member names must be UTF-8")?;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut budget = MAX_DISC;
    for name in listing.lines().filter(|name| !name.ends_with('/')) {
        let normalized = match backend {
            ArchiveReader::Unrar => name.replace('\\', "/"),
            ArchiveReader::Libarchive => name.to_owned(),
        };
        let logical = Path::new(&normalized);
        if !include(logical) {
            continue;
        }
        // libarchive matches operands as patterns. Only literal, relative
        // operands are allowed, including spaces and Windows archive folders
        // represented by ZIP's standard '/' separators.
        if logical
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            || normalized.starts_with('@')
            || normalized
                .chars()
                .any(|c| matches!(c, '*' | '?' | '[' | ']' | '\\' | ':') || c.is_control())
            || !seen.insert(logical.to_owned())
        {
            bail!("ambiguous or unsafe archive member {name:?}");
        }
        if seen.len() > MAX_FILES {
            bail!("archive has more than {MAX_FILES} map or patch files");
        }
        let dest = scratch.join(format!("entry-{}.{}", out.len(), extension(logical)));
        let limit = budget.min(if iso(logical) { MAX_DISC } else { MAX_MAP });
        let extract = backend.extract(path, name);
        let mut output = File::create(&dest)?;
        let size = stream(extract, &mut output, limit, extension(logical) == "map")
            .with_context(|| format!("reading {name:?} from {}", path.display()))?;
        let Some(size) = size else { continue };
        budget -= size;
        out.push(Candidate {
            file: dest,
            logical: logical.to_owned(),
        });
    }
    if out.is_empty()
        && listing
            .lines()
            .any(|name| matches!(extension(Path::new(name)).as_str(), "rar" | "zip" | "7z"))
    {
        bail!(
            "the release contains nested archives; extract the intended map pack and import its folder or archive"
        );
    }
    Ok(out)
}

pub(super) fn discover(path: &Path, scratch: &Path) -> Result<Vec<Candidate>> {
    let path = path
        .canonicalize()
        .with_context(|| path.display().to_string())?;
    let mut sources = Vec::new();
    if path.is_dir() {
        folder(&path, &path, &mut sources)?;
    } else if iso(&path) {
        return whole_build::discover_iso(&path, &scratch.join("disc-0"));
    } else {
        sources = archive(&path, scratch)?;
    }
    // A release may name its only level in the archive or folder name.
    let release = if path.is_dir() {
        path.file_name()
    } else {
        path.file_stem()
    };
    let mut out = Vec::new();
    for (i, mut entry) in sources.into_iter().enumerate() {
        if let Some(release) = release {
            entry.logical = Path::new(release).join(entry.logical);
        }
        if iso(&entry.logical) {
            out.extend(whole_build::discover_iso(
                &entry.file,
                &scratch.join(format!("disc-{i}")),
            )?);
        } else {
            if entry.file.metadata()?.len() > MAX_MAP {
                if extension(&entry.logical) == "map" {
                    let mut head = [0; whole_build::HEADER_BYTES];
                    File::open(&entry.file)?.read_exact(&mut head)?;
                    if !whole_build::is_multiplayer(&head)? {
                        continue;
                    }
                }
                bail!(
                    "{} exceeds the {} MiB member limit",
                    entry.logical.display(),
                    MAX_MAP / MIB
                );
            }
            out.push(entry);
        }
    }
    out.sort_by(|a, b| a.logical.cmp(&b.logical));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_pack_import::Scratch;
    use std::path::PathBuf;

    #[test]
    fn skips_non_level_caches_before_reading_the_payload_and_caps_accepted_data() {
        let mut head = vec![0; whole_build::HEADER_BYTES];
        head[..8].copy_from_slice(b"daeh\x08\x00\x00\x00");
        head[0x7fc..].copy_from_slice(b"toof");
        head[0x140] = 3;
        let mut out = Vec::new();
        assert_eq!(
            copy_entry(head.as_slice(), &mut out, 4, true).unwrap(),
            None
        );
        assert!(out.is_empty());
        head[0x140] = 1;
        assert!(copy_entry(head.as_slice(), &mut out, 4, true).is_err());
        assert!(copy_entry(&b"oversize"[..], &mut Vec::new(), 3, false).is_err());
    }

    #[test]
    fn archive_reads_nested_members_without_using_their_paths_as_destinations() {
        let dir = Scratch::new().unwrap();
        let release = dir.0.join("release");
        let maps = release.join("files/Lockout");
        fs::create_dir_all(&maps).unwrap();
        fs::write(maps.join("changes.PPF"), b"invented").unwrap();
        fs::write(release.join("unrelated.txt"), b"ignore this").unwrap();
        let zip = dir.0.join("release.zip");
        let status = reader()
            .args(["--format", "zip", "-cf"])
            .arg(&zip)
            .arg("-C")
            .arg(&release)
            .arg("files")
            .arg("unrelated.txt")
            .status()
            .unwrap();
        assert!(status.success());
        let files = archive(&zip, &dir.0).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].logical, PathBuf::from("files/Lockout/changes.PPF"));
        assert_eq!(fs::read(&files[0].file).unwrap(), b"invented");
        assert_eq!(files[0].file.parent(), Some(dir.0.as_path()));
    }
}
