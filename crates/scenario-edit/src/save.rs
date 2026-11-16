//! Opening a map (a file, or a file inside the user's disc image) and saving
//! the edited scenario as a new `.map`.
//!
//! Saving goes through `map::writer::MapEditor`: the source streams through
//! unchanged except for the scenario patches from [`Scenario::patches`] and
//! the header checksum at `+0x2d0`, which the writer recomputes. With no
//! edits the output is byte-identical to the source.
use crate::scenario::Scenario;
use crate::{Error, Result};
use map::Map;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

/// Where a map's bytes come from: a whole file, or a byte range of one
/// (a map inside an XISO). The source is only ever read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSource {
    pub path: PathBuf,
    /// `(offset, length)` inside `path`, or `None` for the whole file.
    pub region: Option<(u64, u64)>,
}
impl MapSource {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            region: None,
        }
    }
    pub fn open(&self) -> Result<Map<BufReader<File>>> {
        let reader = BufReader::new(File::open(&self.path)?);
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
                    MapSource {
                        path: iso.to_owned(),
                        region: Some((
                            partition + u64::from(e.sector) * xiso::SECTOR,
                            u64::from(e.size),
                        )),
                    },
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
    let mut editor = map.edit();
    for (offset, bytes) in scenario.patches() {
        editor.patch_tag(scenario.tag, offset, &bytes)?;
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
