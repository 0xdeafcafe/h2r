//! Opening a map (a file, or a file inside the user's disc image) and saving
//! the edited scenario as a new `.map`.
//!
//! Saving goes through `map::writer::MapEditor`: the source streams through
//! unchanged except for the scenario patches from [`Scenario::changes`],
//! blocks that outgrew their source appended to the metadata (with the
//! header sizes that grow with it), and the header checksum at `+0x2d0`,
//! which the writer recomputes. With no edits the output is byte-identical
//! to the source.
use crate::scenario::Scenario;
use crate::{Error, Result};
use map::Map;
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a map's bytes come from: a whole file, or a byte range of one
/// (a map inside an XISO). The source is only ever read.
///
/// `bytes`, when set, are the map's current bytes held in memory (a map
/// package resolved over this location, docs/maps.md); `path` and
/// `region` then name its base, the disc's map, which the shared caches
/// are found beside and which a package's patch starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSource {
    pub path: PathBuf,
    /// `(offset, length)` inside `path`, or `None` for the whole file.
    pub region: Option<(u64, u64)>,
    pub bytes: Option<Arc<[u8]>>,
}

/// A map's reader: a host file, or bytes in memory.
pub enum MapReader {
    File(BufReader<File>),
    Memory(Cursor<Arc<[u8]>>),
}
impl Read for MapReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(f) => f.read(buf),
            Self::Memory(m) => m.read(buf),
        }
    }
}
impl Seek for MapReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(f) => f.seek(pos),
            Self::Memory(m) => m.seek(pos),
        }
    }
}

impl MapSource {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            region: None,
            bytes: None,
        }
    }
    /// `length` bytes at `offset` in `path` (a map inside a disc image).
    pub fn region(path: impl Into<PathBuf>, region: Option<(u64, u64)>) -> Self {
        Self {
            path: path.into(),
            region,
            bytes: None,
        }
    }
    /// This source without its in-memory bytes: the base map on disk.
    pub fn base(&self) -> Self {
        Self::region(self.path.clone(), self.region)
    }
    /// The base map's bytes (the file or region, never `bytes`).
    pub fn read_base(&self) -> Result<Vec<u8>> {
        let mut f = File::open(&self.path)?;
        let (at, len) = match self.region {
            Some(r) => r,
            None => (0, f.metadata()?.len()),
        };
        let len = usize::try_from(len).map_err(|_| Error::Save("map too large"))?;
        f.seek(SeekFrom::Start(at))?;
        let mut out = vec![0; len];
        f.read_exact(&mut out)?;
        Ok(out)
    }
    pub fn open(&self) -> Result<Map<MapReader>> {
        if let Some(b) = &self.bytes {
            return Ok(Map::from_reader(MapReader::Memory(Cursor::new(b.clone())))?);
        }
        let reader = MapReader::File(BufReader::new(File::open(&self.path)?));
        Ok(match self.region {
            None => Map::from_reader(reader)?,
            Some((base, len)) => Map::from_region(reader, base, len)?,
        })
    }
    /// The maps under `/maps/` in an XISO, by name (no `.map`).
    pub fn disc_maps(iso: &Path) -> Result<Vec<(String, MapSource)>> {
        let mut x = xiso_open(iso)?;
        let partition = x.partition_offset();
        let entries = x
            .entries()
            .map_err(|_| Error::Save("cannot list the disc"))?;
        Ok(entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| {
                let lower = e.path.to_ascii_lowercase();
                let rest = lower
                    .strip_prefix("/maps/")
                    .or_else(|| lower.strip_prefix("maps/"))?;
                let name = rest.strip_suffix(".map")?;
                if name.contains('/') {
                    return None;
                }
                let start = e.path.len() - 4 - name.len();
                Some((
                    e.path[start..e.path.len() - 4].to_owned(),
                    MapSource::region(
                        iso,
                        Some((
                            partition + u64::from(e.sector) * xiso::SECTOR,
                            u64::from(e.size),
                        )),
                    ),
                ))
            })
            .collect())
    }
}

fn xiso_open(iso: &Path) -> Result<xiso::Xiso<BufReader<File>>> {
    xiso::Xiso::open(BufReader::new(File::open(iso)?)).map_err(|_| Error::Save("not an XISO image"))
}

/// Stream `map` with the scenario's patches applied into `out` (at its
/// current position), with the checksum repaired. Returns the bytes written.
pub fn write_patched<R: Read + Seek>(
    map: &mut Map<R>,
    scenario: &Scenario,
    out: &mut (impl Write + Seek),
) -> Result<u64> {
    let current = map.scenario().ok_or(Error::Save("map has no scenario"))?;
    let loc = current
        .location
        .ok_or(Error::Save("scenario has no payload"))?;
    if current.id != scenario.tag || loc.address != scenario.address || loc.size != scenario.size {
        return Err(Error::Save("the scenario was read from a different map"));
    }
    let changes = scenario.changes();
    let mut editor = map.edit();
    for (address, bytes) in &changes.patches {
        editor.patch_metadata(*address, bytes)?;
    }
    // A block that outgrew its source goes to the end of the metadata, and
    // its root pair points there; the old elements stay as dead data.
    for g in &changes.grown {
        let at = editor.append_metadata(&g.bytes, 16)?;
        let mut pair = g.count.to_le_bytes().to_vec();
        pair.extend_from_slice(&at.to_le_bytes());
        editor.patch_metadata(g.root, &pair)?;
    }
    Ok(editor.write_to(out)?)
}

/// Save to `out`, never over the source file. Writes a sibling temporary
/// file and renames it into place, so a failed save leaves `out` as it was.
pub fn save_as(source: &MapSource, scenario: &Scenario, out: &Path) -> Result<u64> {
    if let (Ok(a), Ok(b)) = (source.path.canonicalize(), out.canonicalize())
        && a == b
    {
        return Err(Error::Save("refusing to overwrite the source map"));
    }
    let mut map = source.open()?;
    let mut name = out
        .file_name()
        .ok_or(Error::Save("output has no file name"))?
        .to_owned();
    name.push(".partial");
    let tmp = out.with_file_name(name);
    let result = (|| {
        let mut w = BufWriter::new(File::create(&tmp)?);
        let n = write_patched(&mut map, scenario, &mut w)?;
        w.flush()?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&tmp, out)?;
        Ok(n)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
