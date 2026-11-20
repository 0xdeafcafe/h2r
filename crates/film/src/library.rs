use crate::{FilmMeta, Reader};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct LibraryEntry {
    pub meta: FilmMeta,
    /// None for v1 films, which have only the original header score line.
    pub summary: Option<crate::blam::Summary>,
    pub version: u32,
    pub runtime_stamp: String,
    pub xbe_sha256: String,
    pub guest_image: Option<crate::image::GuestImage>,
    pub saved: bool,
    pub lease_id: Option<String>,
    pub game_id: Option<String>,
    pub path: PathBuf,
}
impl std::ops::Deref for LibraryEntry {
    type Target = FilmMeta;
    fn deref(&self) -> &FilmMeta {
        &self.meta
    }
}
pub struct Library {
    dir: PathBuf,
}
impl Library {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(dir.as_ref())?;
        Ok(Self {
            dir: dir.as_ref().to_owned(),
        })
    }
    fn entries(&self) -> Result<Vec<LibraryEntry>> {
        let mut rows = Vec::new();
        for file in fs::read_dir(&self.dir)? {
            let file = file?;
            let path = file.path();
            if !file.file_type()?.is_file() || path.extension().is_none_or(|s| s != "h2film") {
                continue;
            }
            // A recorder may still be writing its first HEAD. Keep the
            // other films available when a recording has no readable header.
            let Ok(mut file) = fs::File::open(&path) else {
                continue;
            };
            let (header, summary) = match crate::range::listing(&mut file) {
                Ok(Some((header, summary))) => (header, Some(summary)),
                Ok(None) => {
                    let Ok(mut reader) = Reader::new(file) else {
                        continue;
                    };
                    let summary = if reader.header.version == 2 {
                        let Ok(summary) = crate::blam::summary(&mut reader) else {
                            continue;
                        };
                        Some(summary)
                    } else {
                        None
                    };
                    (reader.header, summary)
                }
                Err(_) => continue,
            };
            rows.push(LibraryEntry {
                version: header.version,
                runtime_stamp: header.runtime_stamp,
                xbe_sha256: header.xbe_sha256,
                guest_image: header.guest_image,
                summary,
                game_id: header.game_id.or_else(|| header.lease_id.clone()),
                lease_id: header.lease_id,
                saved: path.with_extension("h2film.saved").is_file(),
                path,
                meta: header.meta,
            });
        }
        rows.sort_by(|a, b| {
            b.meta
                .started_ms
                .cmp(&a.meta.started_ms)
                .then(a.path.cmp(&b.path))
        });
        Ok(rows)
    }
    /// Recent (25) plus any older pinned films. Listing never removes a file.
    pub fn list(&self) -> Result<Vec<LibraryEntry>> {
        Ok(self
            .entries()?
            .into_iter()
            .enumerate()
            .filter_map(|(i, r)| (i < 25 || r.saved).then_some(r))
            .collect())
    }
    pub fn get(&self, id: &str) -> Result<LibraryEntry> {
        let mut matches = self.entries()?.into_iter().filter(|r| r.meta.id == id);
        let row = matches.next().context("unknown film ID")?;
        ensure!(matches.next().is_none(), "ambiguous film ID");
        Ok(row)
    }
    pub fn save(&self, id: &str) -> Result<()> {
        let p = self.get(id)?.path.with_extension("h2film.saved");
        match fs::File::create_new(p) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            r => Ok(r.map(|_| ())?),
        }
    }
    pub fn unsave(&self, id: &str) -> Result<()> {
        remove_pin(&self.get(id)?.path)
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.get(id)?.path;
        remove_pin(&path)?;
        Ok(fs::remove_file(path)?)
    }
}
fn remove_pin(path: &Path) -> Result<()> {
    match fs::remove_file(path.with_extension("h2film.saved")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => Ok(r?),
    }
}
