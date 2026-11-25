//! The Tags tool on other maps of the source, and what a tag's data is.
//!
//! **Browsing.** Beside the open map's document ([`App::tags`]) the Tags
//! tool can show any other map of the source read-only: a shared cache
//! (`shared`, `single_player_shared`), the main menu, another level. Its
//! tags list and read as the open map's do; nothing edits them, as the
//! scenario session that saves tag edits holds the open map only. Picking
//! `shared` or `single_player_shared` in the Maps list browses it (they
//! are not levels, so the Scenario tool has nothing to show of them), as
//! does a map the Scenario tool can't open.
//!
//! **Where a tag lives.** Every retail tag has its metadata in the map
//! that uses it, and its raw data (pixels, geometry, samples) in that map
//! or a shared one (`shared_refs`). A tag with no data in its map (a map
//! built otherwise) is looked up by group and name in the source's shared
//! caches and marked with the one that has it ([`TagEntry::elsewhere`]).
//!
//! **Data.** A bitmap's first level decodes to RGBA for the preview
//! (`assets::decode_image`, from the map as read from its source: field
//! edits are not in it), and a sound's pitch ranges, permutations and
//! chunks list from the map's `ugh!` gestalt (`tags::sound`).
//!
//! [`TagEntry::elsewhere`]: super::tag_doc::TagEntry::elsewhere
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::PathBuf;
use std::sync::Arc;

use editor_core::MapSource;
use editor_core::tag_bundle;
use map::{Map, ResourceFile, TagId};
use serde_json::{Value, json};

use super::map_pack::Picture;
use super::tag_doc::{Access, TagDoc};
use super::{App, ToolKind};

/// Why a browsed map's tags are read-only.
pub const BROWSING: &str = "a map browsed in the Tags tool: its tags are shown, not edited \
     (open it in the Scenario tool to edit its tags)";

/// The shared caches, by the names their maps have on the disc.
pub const SHARED_CACHES: [(ResourceFile, &str); 3] = [
    (ResourceFile::Shared, "shared"),
    (ResourceFile::SinglePlayerShared, "single_player_shared"),
    (ResourceFile::MainMenu, "mainmenu"),
];

/// Whether `name` is a shared cache the Maps list opens in the Tags tool
/// (`shared`, `single_player_shared`): not a level.
pub fn opens_as_tags(name: &str) -> bool {
    ["shared", "single_player_shared"]
        .iter()
        .any(|s| s.eq_ignore_ascii_case(name))
}

/// A map of the source shown read-only in the Tags tool.
pub struct Browse {
    /// Its name in the source.
    pub map: String,
    pub doc: TagDoc,
    /// The bitmap preview: the tag it is of and the picture, or why there
    /// is none.
    pub preview: Option<(TagId, Result<Arc<Picture>, String>)>,
}

/// A map reader of any kind.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}
/// A map opened through [`ReadSeek`].
pub type AnyMap = Map<Box<dyn ReadSeek>>;

fn file_reader(path: &std::path::Path) -> Result<Box<dyn ReadSeek>, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Box::new(BufReader::new(f)))
}

fn open_reader(
    r: Box<dyn ReadSeek>,
    region: Option<(u64, u64)>,
    name: &str,
) -> Result<AnyMap, String> {
    match region {
        None => Map::from_reader(r),
        Some((base, len)) => Map::from_region(r, base, len),
    }
    .map_err(|e| format!("{name}: {e}"))
}

/// A reader with byte runs laid over it, `(offset in the reader, bytes)`:
/// a read returns the runs' bytes where they cover it and the reader's
/// elsewhere.
pub struct Patched<R> {
    inner: R,
    /// Where the next read starts.
    at: u64,
    /// Where `inner` is, when known: a read that starts there does not
    /// seek it (a buffered reader's seek drops its buffer).
    inner_at: Option<u64>,
    runs: Vec<(u64, Vec<u8>)>,
}

impl<R> Patched<R> {
    pub fn new(inner: R, mut runs: Vec<(u64, Vec<u8>)>) -> Self {
        runs.sort_by_key(|r| r.0);
        Self {
            inner,
            at: 0,
            inner_at: None,
            runs,
        }
    }
}

impl<R: Read + Seek> Read for Patched<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.inner_at != Some(self.at) {
            self.inner.seek(std::io::SeekFrom::Start(self.at))?;
        }
        let n = self.inner.read(buf)?;
        let (lo, hi) = (self.at, self.at + n as u64);
        for (start, bytes) in &self.runs {
            let end = start + bytes.len() as u64;
            if *start >= hi {
                break;
            }
            if end <= lo {
                continue;
            }
            let (from, to) = (lo.max(*start), hi.min(end));
            buf[(from - lo) as usize..(to - lo) as usize]
                .copy_from_slice(&bytes[(from - start) as usize..(to - start) as usize]);
        }
        self.at = hi;
        self.inner_at = Some(hi);
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for Patched<R> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            std::io::SeekFrom::Start(n) => Some(n),
            std::io::SeekFrom::Current(d) => self.at.checked_add_signed(d),
            std::io::SeekFrom::End(d) => {
                let end = self.inner.seek(std::io::SeekFrom::End(0))?;
                self.inner_at = Some(end);
                end.checked_add_signed(d)
            }
        };
        self.at = at.ok_or_else(|| std::io::Error::other("seek before the start"))?;
        Ok(self.at)
    }
}

fn region(path: PathBuf, r: Option<(u64, u64)>) -> Option<assets::Region> {
    let (base, length) = match r {
        Some(r) => r,
        None => (0, std::fs::metadata(&path).ok()?.len()),
    };
    Some(assets::Region { path, base, length })
}

impl App {
    /// The document the Tags tool shows: the browsed map's, else the open
    /// map's.
    pub fn shown_tags(&self) -> &TagDoc {
        self.tag_browse.as_ref().map_or(&self.tags, |b| &b.doc)
    }
    /// The map browsed in the Tags tool, if one is.
    pub fn browsing(&self) -> Option<&str> {
        self.tag_browse.as_ref().map(|b| b.map.as_str())
    }
    /// The name of the map the Tags tool shows.
    pub fn shown_map_name(&self) -> Option<&str> {
        self.browsing().or(self.loaded.as_deref())
    }

    /// [`super::Action::BrowseTags`]: show map `name` of the source in the
    /// Tags tool read-only, or (`None`, or the open map's name) the open
    /// map again.
    pub(crate) fn browse_tags(&mut self, name: Option<&str>) {
        let name = name.filter(|n| {
            !(self.session.is_open()
                && self.loaded.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(n)))
        });
        let Some(name) = name else {
            if let Some(b) = self.tag_browse.take() {
                self.info(format!("tags: back to the open map from {}", b.map));
            }
            self.sync_tags();
            return;
        };
        if self.browsing().is_some_and(|b| b.eq_ignore_ascii_case(name)) {
            return;
        }
        match self.open_browse(name) {
            Ok(b) => {
                let n = b.doc.tags().len();
                self.info(format!("tags: {} ({n} tags, read-only)", b.map));
                self.tag_browse = Some(b);
            }
            Err(e) => self.error(format!("tags: {name}: {e}")),
        }
    }

    pub(super) fn browse_shared_tag(&mut self, id: TagId) {
        let Some(t) = self.tags.tag(id) else {
            return;
        };
        let Some(cache) = t.elsewhere.as_deref() else {
            return;
        };
        let result = self.open_browse(cache).and_then(|mut browse| {
            let id = browse.doc.tags().iter()
                .find(|tag| tag.group == t.group && tag.name == t.name && tag.size > 0)
                .map(|tag| tag.id)
                .ok_or_else(|| format!("{} is unavailable in {cache}", t.name))?;
            browse.doc.selected = Some(id);
            Ok(browse)
        });
        match result {
            Ok(browse) => {
                self.tag_browse = Some(browse);
                self.tool = super::ToolKind::Tags;
            }
            Err(e) => self.error(format!("tags: {e}")),
        }
    }

    fn open_browse(&self, name: &str) -> Result<Browse, String> {
        let source = self
            .source
            .as_ref()
            .ok_or("open a disc, map or folder first")?;
        let mut m = source.open_map(name).map_err(|e| format!("{e:#}"))?;
        let mut doc = TagDoc::new(self.tags.registry());
        doc.load(&mut m).map_err(|e| e.to_string())?;
        doc.read_only(BROWSING);
        resolve_elsewhere(source, &mut doc, name);
        Ok(Browse {
            map: name.to_owned(),
            doc,
            preview: None,
        })
    }

    /// Mark the open map's tags that have no data in it with the shared
    /// cache that has them.
    pub(super) fn mark_elsewhere(&mut self) {
        let (Some(source), Some(own)) = (self.source.as_ref(), self.loaded.clone()) else {
            return;
        };
        resolve_elsewhere(source, &mut self.tags, &own);
    }

    /// When `name` opens in the Tags tool rather than the Scenario tool
    /// (a shared cache): browse it and show the Tags tool.
    pub(super) fn select_shared_cache(&mut self, name: &str) -> bool {
        if !opens_as_tags(name) || self.is_mappack_map(name) || self.source.is_none() {
            return false;
        }
        self.browse_tags(Some(name));
        if self.browsing().is_some() {
            self.tool = ToolKind::Tags;
            self.info(format!(
                "{name} is a shared cache, not a level: its tags are in the Tags tool"
            ));
        }
        true
    }

    /// The map the Tags tool shows, opened from its source (field edits
    /// are not in it).
    pub fn open_shown_map(&self) -> Result<AnyMap, String> {
        let (r, region, name) = self.shown_reader()?;
        open_reader(r, region, &name)
    }

    /// The map the Tags tool shows with its document's metadata edits laid
    /// over its bytes ([`TagDoc::patches`], placed by the map's own
    /// address space): what a reader of the map sees once the edits save.
    /// A grown block's new room is not in it (it has no place in the
    /// file until the save appends it). The Shader tab's read and preview
    /// read through it (docs/editor/rich-editors.md, "Shaders").
    pub fn open_shown_map_edited(&self) -> Result<AnyMap, String> {
        let patches = self.shown_tags().patches();
        if patches.is_empty() {
            return self.open_shown_map();
        }
        let plain = self.open_shown_map()?;
        let (r, region, name) = self.shown_reader()?;
        let base = region.map_or(0, |(at, _)| at);
        let mut runs = Vec::with_capacity(patches.len());
        for (address, bytes) in patches {
            let len = u32::try_from(bytes.len()).map_err(|_| "an edit too long to place")?;
            let at = plain
                .file_offset(address, len)
                .map_err(|e| format!("the edit at {address:#010x}: {e}"))?;
            runs.push((base + at, bytes));
        }
        let r: Box<dyn ReadSeek> = Box::new(Patched::new(r, runs));
        open_reader(r, region, &name)
    }

    /// The shown map's reader, its region in that reader (none for the
    /// whole of it) and a name for its errors.
    fn shown_reader(&self) -> Result<(Box<dyn ReadSeek>, Option<(u64, u64)>, String), String> {
        if let Some(b) = &self.tag_browse {
            let src = self
                .source
                .as_ref()
                .ok_or("open a disc, map or folder first")?;
            let (path, r) = src.location(&b.map).map_err(|e| format!("{e:#}"))?;
            return Ok((file_reader(&path)?, r, path.display().to_string()));
        }
        let s = self.session.source().ok_or("no map is open")?;
        if let Some(bytes) = &s.bytes {
            let r: Box<dyn ReadSeek> = Box::new(Cursor::new(bytes.clone()));
            return Ok((r, None, "the open map".to_owned()));
        }
        Ok((file_reader(&s.path)?, s.region, s.path.display().to_string()))
    }

    /// The files the shown map's raw pointers select: the map itself and
    /// the shared caches the source has (a Map Pack map's are the Halo 2
    /// source's).
    pub fn shown_raw_files(&self) -> Result<assets::ResourceFiles, String> {
        let mut files = assets::ResourceFiles::new();
        if let Some(b) = &self.tag_browse {
            let src = self
                .source
                .as_ref()
                .ok_or("open a disc, map or folder first")?;
            let mut names: Vec<(ResourceFile, &str)> = vec![(ResourceFile::Local, b.map.as_str())];
            for (file, name) in SHARED_CACHES {
                names.push((file, name));
            }
            for (file, name) in names {
                if let Ok((path, r)) = src.location(name)
                    && let Some(reg) = region(path, r)
                {
                    let _ = files.set(file, reg);
                }
            }
            return Ok(files);
        }
        let s = self.session.source().ok_or("no map is open")?;
        match &s.bytes {
            Some(b) => files.set_bytes(ResourceFile::Local, b.clone()),
            None => {
                if let Some(reg) = region(s.path.clone(), s.region) {
                    files
                        .set(ResourceFile::Local, reg)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        for (file, shared) in scenario_edit::geometry::shared_sources(s) {
            if let Some(reg) = region(shared.path.clone(), shared.region) {
                let _ = files.set(file, reg);
            }
        }
        Ok(files)
    }

    /// Bitmap tag `id`'s image `image` at its base level, as RGBA.
    pub fn bitmap_picture(&self, id: TagId, image: usize) -> Result<Picture, String> {
        let mut map = self.open_shown_map()?;
        let mut files = self.shown_raw_files()?;
        let decoded = tags::DecodedTag::read(&mut map, id).map_err(|e| e.to_string())?;
        let tags::TagData::Bitmap(b) = decoded.data else {
            return Err("not a bitmap".into());
        };
        let img = b
            .images
            .get(image)
            .ok_or_else(|| format!("the bitmap has {} images", b.images.len()))?;
        let d = assets::decode_image(img, &mut files).map_err(|e| e.to_string())?;
        let l = d
            .levels
            .into_iter()
            .find(|l| l.level == 0 && l.face == 0)
            .ok_or("the image has no base level")?;
        let n = (l.width * l.height * 4) as usize;
        let rgba = l.rgba.get(..n).ok_or("the base level is short")?.to_vec();
        Ok(Picture {
            width: l.width,
            height: l.height,
            rgba,
        })
    }

    /// [`super::Action::TagPreview`]: decode bitmap tag `id` of the shown
    /// map for the Tags tool (kept until another is asked for).
    pub(super) fn preview_tag(&mut self, id: TagId) {
        let r = self.bitmap_picture(id, 0).map(Arc::new);
        if let Err(e) = &r {
            self.error(format!("preview: {e}"));
        }
        match self.tag_browse.as_mut() {
            Some(b) => b.preview = Some((id, r)),
            None => self.tag_preview = Some((id, r)),
        }
    }

    /// The bitmap preview of tag `id` of the shown map, once asked for.
    pub fn tag_picture(&self, id: TagId) -> Option<&Result<Arc<Picture>, String>> {
        let p = match &self.tag_browse {
            Some(b) => b.preview.as_ref(),
            None => self.tag_preview.as_ref(),
        }?;
        (p.0 == id).then_some(&p.1)
    }

    /// What tag `id` of the shown map's resources are: for a bitmap its
    /// images, for a sound its pitch ranges, permutations and chunks
    /// (through the map's `ugh!`); `null` for other groups.
    pub fn tag_media(&self, id: TagId) -> Result<Value, String> {
        let doc = self.shown_tags();
        let t = doc.tag(id).ok_or("no such tag")?;
        match &t.group.0 {
            b"bitm" => {
                let mut map = self.open_shown_map()?;
                let decoded = tags::DecodedTag::read(&mut map, id).map_err(|e| e.to_string())?;
                let tags::TagData::Bitmap(b) = decoded.data else {
                    return Err("not a bitmap".into());
                };
                Ok(json!({
                    "bitmap": b.images.iter().map(|i| json!({
                        "width": i.dimensions[0],
                        "height": i.dimensions[1],
                        "depth": i.dimensions[2],
                        "format": format!("{:?}", i.format),
                        "mips": i.mip_count,
                        "lods": i.lods.iter().flatten().count(),
                    })).collect::<Vec<_>>()
                }))
            }
            b"snd!" => {
                let mut map = self.open_shown_map()?;
                let header =
                    tags::sound::read_header(&mut map, id).map_err(|e| e.to_string())?;
                let gestalt = tags::sound::Gestalt::read(&mut map).map_err(|e| e.to_string())?;
                let sound = gestalt.resolve(&header).map_err(|e| e.to_string())?;
                let ranges: Vec<Value> = sound
                    .ranges
                    .iter()
                    .map(|r| {
                        let perms: Vec<Value> = r
                            .permutations
                            .iter()
                            .map(|p| {
                                let chunks: Vec<Value> = gestalt
                                    .chunks_of(p)
                                    .iter()
                                    .map(|c| {
                                        json!({
                                            "file": tag_bundle::file_name(c.pointer.file),
                                            "offset": c.pointer.offset,
                                            "size": c.size,
                                        })
                                    })
                                    .collect();
                                json!({ "encoded_size": p.encoded_size, "chunks": chunks })
                            })
                            .collect();
                        json!({ "permutations": perms })
                    })
                    .collect();
                Ok(json!({
                    "sound": {
                        "sample_rate": header.sample_rate.hz(),
                        "encoding": format!("{:?}", header.encoding),
                        "compression": format!("{:?}", header.compression),
                        "pitch_ranges": ranges,
                    }
                }))
            }
            _ => Ok(Value::Null),
        }
    }
}

/// Mark `doc`'s tags that have no data in it (map `own`) with the shared
/// cache of `source` that has a tag of the same group and name with data.
/// Nothing is opened when every tag has its data.
pub fn resolve_elsewhere(source: &MapSource, doc: &mut TagDoc, own: &str) {
    let lacking: Vec<(TagId, map::FourCC, String)> = doc
        .tags()
        .iter()
        .filter(|t| matches!(t.access, Access::None(why) if why == super::tag_doc::NO_DATA))
        .map(|t| (t.id, t.group, t.name.clone()))
        .collect();
    if lacking.is_empty() {
        return;
    }
    for (_, cache) in SHARED_CACHES {
        if cache.eq_ignore_ascii_case(own) {
            continue;
        }
        let Ok(m) = source.open_map(cache) else {
            continue;
        };
        let have: std::collections::HashSet<(map::FourCC, &str)> = m
            .tags()
            .iter()
            .filter(|t| t.location.is_some_and(|l| l.size > 0))
            .filter_map(|t| Some((t.group, t.name.as_deref()?)))
            .collect();
        for (id, group, name) in &lacking {
            if have.contains(&(*group, name.as_str()))
                && doc.tag(*id).is_some_and(|t| t.elsewhere.is_none())
            {
                doc.set_elsewhere(*id, cache);
            }
        }
    }
}

#[cfg(test)]
mod patched_tests {
    use super::Patched;
    use std::io::{Cursor, Read, Seek, SeekFrom};

    /// Reads across, inside and beside the runs see the runs' bytes where
    /// they cover and the reader's elsewhere, through seeks of each kind.
    #[test]
    fn runs_lay_over_the_readers_bytes() {
        let base: Vec<u8> = (0..32).collect();
        let runs = vec![(10, vec![0xaa, 0xbb]), (4, vec![0xcc; 3])];
        let mut p = Patched::new(Cursor::new(base), runs);
        let mut all = Vec::new();
        p.read_to_end(&mut all).unwrap();
        let mut want: Vec<u8> = (0..32).collect();
        want[4..7].fill(0xcc);
        want[10..12].copy_from_slice(&[0xaa, 0xbb]);
        assert_eq!(all, want);
        let mut two = [0u8; 2];
        p.seek(SeekFrom::Start(5)).unwrap();
        p.read_exact(&mut two).unwrap();
        assert_eq!(two, [0xcc, 0xcc]);
        p.seek(SeekFrom::Current(4)).unwrap();
        p.read_exact(&mut two).unwrap();
        assert_eq!(two, [0xbb, 12]);
        assert_eq!(p.seek(SeekFrom::End(-1)).unwrap(), 31);
        p.read_exact(&mut two[..1]).unwrap();
        assert_eq!(two[0], 31);
        assert!(p.seek(SeekFrom::Current(-100)).is_err());
    }
}
