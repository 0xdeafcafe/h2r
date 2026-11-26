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
    pub ticks_per_second: Option<u16>,
    pub version: u32,
    pub runtime_stamp: String,
    pub simulation_hooks: Option<String>,
    pub xbe_sha256: String,
    pub guest_image: Option<crate::image::GuestImage>,
    pub saved: bool,
    pub lease_id: Option<String>,
    pub game_id: Option<String>,
    pub path: PathBuf,
}
impl LibraryEntry {
    /// A local menu-only recording has no game to select.
    pub fn is_game(&self) -> bool {
        self.meta.source == crate::Source::Host
            || !self.meta.map.is_empty()
            || !self.meta.players.is_empty()
            || self.summary.as_ref().is_some_and(|s| !s.level.is_empty() || !s.players.is_empty())
    }

    /// A recorded film: `<id>.h2film`, the film file, with `<id>.h2stats`
    /// beside it (docs/game/film-format.md). The row is the film file's
    /// header and seed, read with its game start's block; no tick chunk is
    /// inflated. A sealed stats container of the same film adds its final
    /// header (the map, players and scores) and its scoreboard. An unsealed
    /// film a recorder still holds ([`crate::file::hold`]) is a game in
    /// progress and is not listed; one no process holds (its recording was
    /// killed) lists with the ticks up to its last checkpoint.
    fn film(path: &Path, file: fs::File) -> Result<Self> {
        let mut reader = crate::file::FilmReader::open(file)?;
        ensure!(
            reader.sealed || !crate::file::being_written(path),
            "the film's game is still being recorded"
        );
        let start = reader.game_start()?;
        let seed = reader.seed.header.clone();
        let listed = fs::File::open(crate::file::stats_of(path))
            .ok()
            .and_then(|mut f| crate::range::listing(&mut f).ok().flatten())
            .filter(|(header, _)| header.meta.id == seed.meta.id);
        let (header, summary) = match listed {
            Some((header, front)) => (header, Some(front.summary)),
            None => (seed, None),
        };
        let mut meta = header.meta;
        meta.length_ticks = reader.length_ticks;
        if let Some(start) = &start
            && meta.map.is_empty()
        {
            meta.map.clone_from(&start.game.descriptor.map.name);
        }
        Ok(Self {
            meta,
            summary,
            ticks_per_second: Some(reader.ticks_per_second.into()),
            version: header.version,
            runtime_stamp: header.runtime_stamp,
            // The hooks the game ran under are its game start's stamp.
            simulation_hooks: start
                .and_then(|s| s.stamp.hooks_sha256)
                .or(header.simulation_hooks),
            xbe_sha256: header.xbe_sha256,
            guest_image: header.guest_image,
            saved: path.with_extension("h2film.saved").is_file(),
            game_id: header.game_id.or_else(|| header.lease_id.clone()),
            lease_id: header.lease_id,
            path: path.into(),
        })
    }

    fn split(path: &Path, file: &mut fs::File) -> Result<Self> {
        let (header, layout, seed) = crate::share::read_seed(file)?;
        let stats = match fs::File::open(path.with_extension("h2stats")) {
            Ok(mut f) => {
                let front = crate::range::read_front(&mut f)?;
                ensure!(
                    Some(front.id.as_str()) == seed["game_id"].as_str(),
                    "stats belong to another game"
                );
                Some(front)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let compat: live_proto::film::sharing::Compat =
            serde_json::from_value(seed["compat"].clone())?;
        let started_ms = stats.as_ref().map(|s| s.started_ms).unwrap_or_else(|| {
            file.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64)
        });
        Ok(Self {
            meta: FilmMeta {
                id: live_proto::sign::hex(&header[16..32]),
                map: seed["map"].as_str().context("missing film map")?.into(),
                variant: stats
                    .as_ref()
                    .map(|s| s.summary.variant_name.clone())
                    .unwrap_or_default(),
                started_ms,
                length_ticks: u64::from(layout.ticks),
                players: stats
                    .as_ref()
                    .map(|s| {
                        s.summary
                            .players
                            .iter()
                            .map(|p| crate::Player {
                                name: p.name.clone(),
                                team: p.team,
                                score: p.score,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                source: crate::Source::Local,
                image_stamp: String::new(),
            },
            version: 4,
            runtime_stamp: compat.abi,
            simulation_hooks: Some(compat.hooks).filter(|h| !h.is_empty()),
            xbe_sha256: compat.xbe,
            guest_image: None,
            ticks_per_second: Some(header[12].into()),
            summary: stats.map(|s| s.summary),
            saved: path.with_extension("h2film.saved").is_file(),
            lease_id: None,
            // A local game's simulation ID is independent of its upload reservation.
            game_id: None,
            path: path.into(),
        })
    }

    /// Read the selector row for one file, including a downloaded archive
    /// outside the local library. V3 needs only the front, footer and final HEAD.
    pub fn read(path: &Path) -> Result<Self> {
        let mut file = fs::File::open(path)?;
        use std::io::Read;
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic == b"H2FILM4!" {
            // A recorded film's seed is the film's header; a film shared
            // through Live carries the sharing seed.
            return match Self::film(path, file.try_clone()?) {
                Ok(entry) => Ok(entry),
                Err(_) => Self::split(path, &mut file),
            };
        }
        let (header, summary, ticks_per_second) = match crate::range::listing(&mut file)? {
            Some((header, front)) => (header, Some(front.summary), front.ticks_per_second),
            None => {
                let mut reader = Reader::new(file)?;
                let (summary, rate) = if reader.header.version == 2 {
                    let summary = crate::blam::summary(&mut reader)?;
                    let rate = if let Some(entry) = reader
                        .entries
                        .iter()
                        .find(|e| e.tag == crate::blam::DESC)
                        .cloned()
                    {
                        let desc: crate::blam::MatchHeader = reader.read(&entry)?;
                        desc.ticks_per_second
                    } else {
                        None
                    };
                    (Some(summary), rate)
                } else {
                    (None, None)
                };
                (reader.header, summary, rate)
            }
        };
        Ok(Self {
            version: header.version,
            runtime_stamp: header.runtime_stamp,
            simulation_hooks: header.simulation_hooks,
            xbe_sha256: header.xbe_sha256,
            guest_image: header.guest_image,
            summary,
            ticks_per_second,
            game_id: header.game_id.or_else(|| header.lease_id.clone()),
            lease_id: header.lease_id,
            saved: path.with_extension("h2film.saved").is_file(),
            path: path.into(),
            meta: header.meta,
        })
    }

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
            // The first HEAD or the final index may still be in flight.
            if let Ok(entry) = LibraryEntry::read(&path) {
                rows.push(entry);
            }
        }
        rows.sort_by(|a, b| {
            b.meta
                .started_ms
                .cmp(&a.meta.started_ms)
                .then(a.path.cmp(&b.path))
        });
        Ok(rows)
    }
    /// The newest 25 unsaved games and every saved game, in date order.
    pub fn list(&self) -> Result<Vec<LibraryEntry>> {
        let mut recent = 0;
        Ok(self.entries()?.into_iter().filter(LibraryEntry::is_game).filter(|entry| {
            if entry.saved {
                return true;
            }
            recent += 1;
            recent <= 25
        }).collect())
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
    /// Delete a film: its film file, its stats container and its pin.
    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.get(id)?.path;
        remove_pin(&path)?;
        match fs::remove_file(crate::file::stats_of(&path)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            r => r?,
        }
        Ok(fs::remove_file(path)?)
    }
}
fn remove_pin(path: &Path) -> Result<()> {
    match fs::remove_file(path.with_extension("h2film.saved")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => Ok(r?),
    }
}
