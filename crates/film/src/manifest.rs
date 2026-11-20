//! Initial guest storage and the installed content used to rebuild its mount table.
//! Restore into a new, private directory for every replay, including seeks.
mod keyframe;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const LIMIT: usize = 16 << 20;
#[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Manifest {
    entries: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    assets: Vec<Item>,
}
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Item {
    path: PathBuf,
    data: Option<Vec<u8>>,
    sha256: String,
}

impl Manifest {
    pub fn capture(root: &Path) -> Result<Self> {
        let mut manifest = Self::default();
        let mut left = LIMIT;
        for dir in ["disk", "metadata"] {
            if root.join(dir).try_exists()? {
                manifest.visit(root, Path::new(dir), &mut left)?;
            }
        }
        Ok(manifest)
    }
    /// Rebuild the same package offers and converted campaign level list from
    /// local content. Only digests of installed assets travel with the film.
    pub fn capture_v2(root: &Path) -> Result<Self> {
        let mut guest = Self::capture(root)?;
        let mut assets = Self::default();
        for dir in ["campaigns", "maps"] {
            if root.join(dir).try_exists()? {
                assets.visit(root, Path::new(dir), &mut 0)?;
            }
        }
        guest.assets = assets.entries;
        Ok(guest)
    }

    fn visit(&mut self, root: &Path, path: &Path, left: &mut usize) -> Result<()> {
        ensure!(
            self.entries.len() < 65536 && path.components().count() < 64,
            "film start manifest too large"
        );
        let full = root.join(path);
        let ty = fs::symlink_metadata(&full)?.file_type();
        if ty.is_dir() {
            self.entries.push(Item {
                path: path.to_owned(),
                data: None,
                sha256: String::new(),
            });
            let mut children = fs::read_dir(full)?.collect::<std::io::Result<Vec<_>>>()?;
            children.sort_by_key(|e| e.file_name());
            for e in children {
                // Materialised patches are regenerated from their verified
                // packages. They can be several times larger than the match.
                if path == Path::new("maps") && e.file_name() == "cache" {
                    continue;
                }
                self.visit(root, &path.join(e.file_name()), left)?;
            }
        } else {
            ensure!(
                ty.is_file(),
                "unsupported film start file {}",
                path.display()
            );
            let mut file = fs::File::open(full)?;
            let data = if file.metadata()?.len() <= (*left).min(1 << 20) as u64 {
                let mut bytes = vec![];
                (&mut file).take(*left as u64 + 1).read_to_end(&mut bytes)?;
                *left = left
                    .checked_sub(bytes.len())
                    .context("start file changed during capture")?;
                Some(bytes)
            } else {
                None
            };
            let sha256 = match &data {
                Some(bytes) => format!("{:x}", Sha256::digest(bytes)),
                None => file_digest(&mut file)?,
            };
            self.entries.push(Item {
                path: path.to_owned(),
                sha256,
                data,
            });
        }
        Ok(())
    }
    pub fn restore(&self, root: &Path) -> Result<()> {
        self.restore_with(root, None)
    }
    /// Large initial files are identified by digest. A coordinator can supply
    /// an unchanged copy of the recording's data root for these files.
    pub fn restore_with(&self, root: &Path, initial: Option<&Path>) -> Result<()> {
        self.restore_limited(root, initial, LIMIT)
    }

    fn restore_limited(&self, root: &Path, initial: Option<&Path>, mut left: usize) -> Result<()> {
        fs::create_dir(root).context("film needs a fresh replay directory")?;
        ensure!(self.entries.len() <= 65536 && self.assets.len() <= 65536,
            "film start manifest too large");
        for (e, assets) in self.entries.iter().map(|e| (e, false))
            .chain(self.assets.iter().map(|e| (e, true))) {
            let allowed = if assets {
                e.path.starts_with("maps") || e.path.starts_with("campaigns")
            } else {
                e.path.starts_with("disk") || e.path.starts_with("metadata")
            };
            ensure!(
                allowed && e.path.components().count() < 65
                    && e.path
                        .components()
                        .all(|c| matches!(c, Component::Normal(_))),
                "invalid film start path"
            );
            let p = root.join(&e.path);
            if let Some(bytes) = &e.data {
                ensure!(!assets || bytes.is_empty(), "external film assets require a local source");
                left = left
                    .checked_sub(bytes.len())
                    .context("film start files exceed 16 MiB")?;
                ensure!(
                    format!("{:x}", Sha256::digest(bytes)) == e.sha256,
                    "film start file checksum mismatch"
                );
                fs::File::create_new(p)?.write_all(bytes)?;
            } else if e.sha256.is_empty() {
                fs::create_dir(p)?;
            } else {
                let base = initial
                    .with_context(|| {
                        format!(
                            "{} needs the original start file (--data DIR)",
                            e.path.display()
                        )
                    })?
                    .canonicalize()?;
                let mut source = base.clone();
                for part in e.path.components() {
                    source.push(part.as_os_str());
                    ensure!(
                        !fs::symlink_metadata(&source)?.is_symlink(),
                        "linked initial file"
                    );
                }
                let source = source.canonicalize()?;
                ensure!(
                    source.starts_with(&base) && source.is_file(),
                    "invalid initial file"
                );
                let mut input = fs::File::open(source)?;
                let mut out = fs::File::options()
                    .create_new(true)
                    .write(true)
                    .read(true)
                    .open(p)?;
                std::io::copy(&mut input, &mut out)?;
                std::io::Seek::rewind(&mut out)?;
                ensure!(
                    file_digest(&mut out)? == e.sha256,
                    "wrong start file {}",
                    e.path.display()
                );
            }
        }
        Ok(())
    }
}

pub fn file_digest(input: &mut fs::File) -> Result<String> {
    let mut sha = Sha256::new();
    std::io::copy(input, &mut sha)?;
    Ok(format!("{:x}", sha.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_rebuilds_installed_content_from_local_files_with_matching_digests() {
        let dir = std::env::temp_dir().join(format!("film-assets-{}", live_proto::sign::random_id()));
        let source = dir.join("initial");
        fs::create_dir_all(source.join("campaigns/halo1")).unwrap();
        fs::create_dir_all(source.join("maps/cache")).unwrap();
        fs::write(source.join("maps/cache/unused.map"), b"cached").unwrap();
        let level = Path::new("campaigns/halo1/a30.map");
        fs::write(source.join(level), b"converted map fixture").unwrap();
        let manifest = Manifest::capture_v2(&source).unwrap();
        assert!(manifest.assets.iter().all(|item| item.data.is_none()));
        assert!(manifest.restore(&dir.join("without-source")).is_err());
        let replay = dir.join("replay");
        manifest.restore_with(&replay, Some(&source)).unwrap();
        assert_eq!(fs::read(replay.join(level)).unwrap(), b"converted map fixture");
        assert!(!replay.join("maps/cache").exists());
        fs::write(source.join(level), b"changed map fixture").unwrap();
        assert!(manifest.restore_with(&dir.join("wrong-map"), Some(&source)).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
