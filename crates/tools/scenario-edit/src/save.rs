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
/// package resolved over this location, docs/game/maps.md); `path` and
/// `region` then name its base, the disc's map, which the shared caches
/// are found beside and which a package's patch starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSource {
    pub path: PathBuf,
    /// `(offset, length)` inside `path`, or `None` for the whole file.
    pub region: Option<(u64, u64)>,
    pub bytes: Option<Arc<[u8]>>,
    /// Where the shared caches (`shared`, `mainmenu`, `single_player_shared`)
    /// are found when not beside the map itself: a disc image (their
    /// `/maps/`) or a directory of `.map` files. A Map Pack map leans on the
    /// Halo 2 disc's caches, not its own.
    pub shared: Option<PathBuf>,
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
            shared: None,
        }
    }
    /// `length` bytes at `offset` in `path` (a map inside a disc image).
    pub fn region(path: impl Into<PathBuf>, region: Option<(u64, u64)>) -> Self {
        Self {
            path: path.into(),
            region,
            bytes: None,
            shared: None,
        }
    }
    /// This source with its shared caches taken from `shared` (a disc image
    /// or a directory of maps).
    pub fn with_shared(mut self, shared: Option<PathBuf>) -> Self {
        self.shared = shared;
        self
    }
    /// This source without its in-memory bytes: the base map on disk.
    pub fn base(&self) -> Self {
        Self::region(self.path.clone(), self.region).with_shared(self.shared.clone())
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
    // A geometry edit that cannot be written refuses the whole save,
    // before anything is written (docs/editor/modeling.md, "Rules and
    // limits").
    if let Some(reason) = &changes.geometry_refusal {
        return Err(Error::Declined(format!("geometry: {reason}")));
    }
    if let Some(reason) = &changes.bitmap_refusal {
        return Err(Error::Declined(format!("bitmap: {reason}")));
    }
    let first = map.strings().len();
    let mut editor = map.edit();
    // New string IDs first: the patches already hold their values.
    for (k, s) in changes.strings.iter().enumerate() {
        let id = editor.add_string(s)?;
        if id & 0x00ff_ffff != (first + k) as u32 {
            return Err(Error::Save(
                "a new string ID did not get the index it was given",
            ));
        }
    }
    // A tag written whole to a new place leaves its old bytes dead: a
    // patch there would be lost, so it refuses the save by name.
    for m in &changes.tag_moves {
        let loc = editor
            .source()
            .tag(m.tag)?
            .location
            .ok_or(Error::Save("a moved tag has no payload"))?;
        let (lo, hi) = (
            u64::from(loc.address),
            u64::from(loc.address) + u64::from(loc.size),
        );
        if changes.patches.iter().any(|(a, b)| {
            let (a, e) = (u64::from(*a), u64::from(*a) + b.len() as u64);
            a < hi && lo < e
        }) {
            return Err(Error::Declined(format!(
                "{:#010x} is written whole to a new place, and another edit patches its old bytes: save that edit first",
                m.tag.0
            )));
        }
    }
    for (address, bytes) in &changes.patches {
        editor.patch_metadata(*address, bytes)?;
    }
    for patch in &changes.bsp_patches {
        editor.patch_bsp(patch.space, patch.address, &patch.bytes)?;
    }
    for patch in &changes.raw_patches {
        editor.patch_raw(patch.block, patch.block_size, patch.offset, &patch.bytes)?;
    }
    // A grown section's block goes before the tag index (the index and
    // metadata follow it), and its owner's pointer word names it; the old
    // block stays as dead data.
    for a in &changes.raw_appends {
        let word = editor.append_raw(&a.bytes, a.align)?.encode().to_le_bytes();
        match a.pointer_space {
            None => editor.patch_metadata(a.pointer_address, &word)?,
            Some(space) => editor.patch_bsp(space, a.pointer_address, &word)?,
        }
    }
    // A block that outgrew its source goes to the end of the metadata, and
    // its root pair points there; the old elements stay as dead data.
    for g in &changes.grown {
        let at = editor.append_metadata(&g.bytes, 16)?;
        let mut pair = g.count.to_le_bytes().to_vec();
        pair.extend_from_slice(&at.to_le_bytes());
        editor.patch_metadata(g.root, &pair)?;
    }
    for g in &changes.tag_grown {
        let at = editor.append_metadata(&g.bytes, 16)?;
        let mut pair = g.count.to_le_bytes().to_vec();
        pair.extend_from_slice(&at.to_le_bytes());
        editor.patch_metadata(g.root, &pair)?;
    }
    // The Modeling tool's grown blocks (a level's collision with a solid
    // added): the copy in the ordinary metadata, the pair wherever its tag
    // is, a structure BSP's region included.
    for g in &changes.geometry_grown {
        let at = editor.append_metadata(&g.bytes, g.align)?;
        let mut pair = g.count.to_le_bytes().to_vec();
        pair.extend_from_slice(&at.to_le_bytes());
        match g.pair_space {
            None => editor.patch_metadata(g.pair, &pair)?,
            Some(space) => editor.patch_bsp(space, g.pair, &pair)?,
        }
    }
    // A tag that grew (a bitmap with images or sequences added): its raw
    // blocks first, their pointers written into the copy, then the copy
    // at the end of the metadata with its block pointers rebased to where
    // it lands, and the tag's index entry pointed at it.
    for m in &changes.tag_moves {
        let mut bytes = m.bytes.clone();
        for r in &m.raw {
            let word = editor.append_raw(&r.bytes, r.align)?.encode().to_le_bytes();
            let at = r.pointer as usize;
            bytes
                .get_mut(at..at + 4)
                .ok_or(Error::Save("a moved tag's raw pointer is past its bytes"))?
                .copy_from_slice(&word);
        }
        let address = editor.next_metadata_address(16)?;
        for &o in &m.rebase {
            let at = o as usize;
            let w = bytes
                .get_mut(at..at + 4)
                .ok_or(Error::Save("a moved tag's block pointer is past its bytes"))?;
            let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            w.copy_from_slice(&v.wrapping_add(address).to_le_bytes());
        }
        let at = editor.append_metadata(&bytes, 16)?;
        if at != address {
            return Err(Error::Save(
                "a moved tag did not land where it was laid out",
            ));
        }
        let size =
            u32::try_from(bytes.len()).map_err(|_| Error::Save("a moved tag is too large"))?;
        editor.move_tag(m.tag, at, size)?;
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
