//! Each complete chunk stands alone. Offsets always refer to the four-byte tag.
pub mod blam;
pub mod image;
mod library;
pub mod machine;
pub mod manifest;
pub mod presentation;
pub mod range;
pub mod sidecar;
pub mod tape;
use anyhow::{Context, Result, ensure};
pub use library::{Library, LibraryEntry};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub type Tag = [u8; 4];
pub const HEAD: Tag = *b"HEAD";
pub const DATA: Tag = *b"DATA";
pub const NETR: Tag = *b"NETR";
pub const RAND: Tag = *b"RAND";
pub const HASH: Tag = *b"HASH";
pub const MARK: Tag = *b"MARK";
pub const KEYF: Tag = *b"KEYF";
pub const INDX: Tag = *b"INDX";
const FOOTER_MAGIC: &[u8; 8] = b"H2FILM2!";
const MAX_ENTRIES: usize = 1_000_000;
const MAX_CHUNK: usize = 80 << 20;

/// The writer has not started this chunk. A recorder can omit an optional
/// machine keyframe and continue its input log after this particular error.
#[derive(Debug)]
pub struct ChunkTooLarge;
impl std::fmt::Display for ChunkTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("film chunk exceeds 80 MiB")
    }
}
impl std::error::Error for ChunkTooLarge {}
mod upload;
pub use upload::{MAX_UPLOAD, UPLOAD_TIMEOUT, upload};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Host,
    Local,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub name: String,
    pub team: i32,
    pub score: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilmMeta {
    pub id: String,
    pub map: String,
    pub variant: String,
    pub started_ms: u64,
    pub length_ticks: u64,
    pub players: Vec<Player>,
    pub source: Source,
    pub image_stamp: String,
}
impl FilmMeta {
    pub fn playable_here(&self, stamp: &str) -> bool {
        !stamp.is_empty() && self.image_stamp == stamp
    }
}
/// Render feature groups the recording executable could place on the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderHooks {
    Slim,
    Desktop,
    Gpu,
}

impl RenderHooks {
    pub fn supports(self, recording: Self) -> bool {
        match recording {
            Self::Slim => true,
            Self::Desktop => matches!(self, Self::Desktop | Self::Gpu),
            Self::Gpu => self == Self::Gpu,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub meta: FilmMeta,
    pub title_id: u32,
    pub xbe_sha256: String,
    /// Simulator ABI. Early v2/v3 writers stored a source fingerprint here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime_stamp: String,
    /// Effective simulation hooks, computed after feature placement, in the sealed HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulation_hooks: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_image: Option<image::GuestImage>,
    pub epoch_100ns: u64,
    pub lease_id: Option<String>,
    /// Canonical Live game ID. Sim leases play under the sim assignment ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    pub xnet_addr: String,
    pub transfers_per_slice: u64,
    pub lobby: bool,
    pub lobby_machine_start: bool,
    pub host_bots: bool,
    /// Legacy presence bit used by earlier readers.
    pub telemetry: bool,
    /// The exact observer chain chosen before guest execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_hooks: Option<live_proto::film::TelemetryHooks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_hooks: Option<RenderHooks>,
    pub live_service: bool,
    pub features: String,
}
impl Header {
    pub fn telemetry_plan(&self) -> live_proto::film::TelemetryHooks {
        use live_proto::film::TelemetryHooks;
        self.telemetry_hooks
            .unwrap_or(match (self.telemetry, self.version) {
                (false, _) => TelemetryHooks::OFF,
                (true, 1 | 2) => TelemetryHooks::EVENTS,
                (true, _) => TelemetryHooks::UNIFIED,
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub tag: Tag,
    pub offset: u64,
    pub size: u32,
}

pub struct Writer<W: Write + Seek> {
    out: W,
    index: Vec<Entry>,
    failed: bool,
    version: u32,
    ranges: Option<Box<range::Build>>,
}
impl<W: Write + Seek> Writer<W> {
    pub fn new(mut out: W, header: &Header) -> Result<Self> {
        ensure!(
            out.seek(SeekFrom::End(0))? == 0,
            "film output already contains data"
        );
        ensure!(
            matches!(header.version, 1..=3),
            "unsupported film version {}",
            header.version
        );
        let ranges = if header.version == 3 {
            out.write_all(&range::Prefix { json_len: 0 }.encode())?;
            out.seek(SeekFrom::Start(range::DATA_OFFSET))?;
            Some(Box::new(range::Build::new(header)))
        } else {
            None
        };
        let mut w = Self {
            out,
            index: vec![],
            failed: false,
            version: header.version,
            ranges,
        };
        w.chunk(HEAD, header)?;
        Ok(w)
    }
    pub fn chunk(&mut self, tag: Tag, value: &impl Serialize) -> Result<()> {
        ensure!(tag != INDX, "finish writes the index");
        self.chunk_bytes(tag, &serde_json::to_vec(value)?)
    }
    /// Write a binary track using the same independent zstd framing as JSON chunks.
    pub fn chunk_bytes(&mut self, tag: Tag, bytes: &[u8]) -> Result<()> {
        ensure!(tag != INDX, "finish writes the index");
        ensure!(self.index.len() < MAX_ENTRIES, "film exceeds chunk limits");
        if let Some(ranges) = &mut self.ranges {
            ensure!(
                tag != range::TICK && tag != range::CIDX,
                "use the chronological writer for film time chunks"
            );
            if tag == blam::SUMM {
                return ranges.set_summary(bytes);
            }
            if tag == blam::DESC {
                ranges.set_descriptor(bytes)?;
            }
            if tag == KEYF {
                let key = range::key_tick(bytes)?;
                ensure!(
                    ranges
                        .directory
                        .keyframes
                        .last()
                        .is_none_or(|k| k.tick < key),
                    "unordered film keyframe"
                );
                self.write(tag, bytes)?;
                let offset = self.index.last().unwrap().offset;
                self.ranges
                    .as_mut()
                    .unwrap()
                    .directory
                    .keyframes
                    .push(range::Keyframe { tick: key, offset });
                return Ok(());
            }
        }
        self.write(tag, bytes)
    }
    /// Write one independently decodable block of records over an inclusive
    /// range of the film clock. No public index row is published on a write failure.
    pub fn chronological(&mut self, first: u64, last: u64, raw: &[u8]) -> Result<()> {
        self.chronological_scene(first, last, true, raw)
    }
    /// A full scene checkpoint includes the current render state at the block start.
    /// Ordinary blocks refer to the latest checkpoint and carry only changes.
    pub fn chronological_scene(
        &mut self,
        first: u64,
        last: u64,
        has_full_scene: bool,
        raw: &[u8],
    ) -> Result<()> {
        let ranges = self
            .ranges
            .as_ref()
            .context("time chunks require film v3")?;
        let scene = if has_full_scene {
            first
        } else {
            let rows = &ranges.directory.chunks;
            let scene = rows
                .last()
                .context("first time chunk requires a scene baseline")?
                .scene;
            ensure!(
                rows.len() < range::MAX_SCENE_CHUNKS
                    || rows[rows.len() - range::MAX_SCENE_CHUNKS].first < scene,
                "too many chunks since the last scene baseline"
            );
            scene
        };
        ensure!(
            last.checked_sub(scene)
                .is_some_and(|span| span < range::SCENE_WINDOW),
            "film scene baseline exceeds seek window"
        );
        ensure!(
            first <= last
                && last - first < 128
                && ranges
                    .directory
                    .chunks
                    .last()
                    .map_or(Some(1), |c| c.last.checked_add(1))
                    == Some(first),
            "nonconsecutive or invalid film tick range"
        );
        ensure!(
            self.index.len() < MAX_ENTRIES && !raw.is_empty() && raw.len() <= range::MAX_TICK_BYTES,
            "film time chunk exceeds limits"
        );
        range::check_ticks(raw, first, last)?;
        let clock = range::clock_runs(raw).map_err(anyhow::Error::msg)?;
        self.write(range::TICK, raw)?;
        let entry = self.index.last().unwrap();
        self.ranges
            .as_mut()
            .unwrap()
            .directory
            .chunks
            .push(range::Chunk {
                clock,
                scene,
                first,
                last,
                offset: entry.offset,
                size: entry.size,
                raw_size: raw.len() as u32,
            });
        Ok(())
    }
    /// Physical offset of the most recently written chunk, for a tape cursor.
    pub fn last_chunk_offset(&self) -> Option<u64> {
        self.index.last().map(|entry| entry.offset)
    }

    fn write(&mut self, tag: Tag, raw: &[u8]) -> Result<()> {
        ensure!(!self.failed, "film output already failed");
        let bytes = compress_chunk(raw)?;
        let offset = self.out.stream_position()?;
        self.failed = true;
        self.index
            .push(write_envelope(&mut self.out, tag, offset, &bytes)?);
        self.failed = false;
        Ok(())
    }
    pub fn finish(mut self) -> Result<W> {
        let public = if let Some(mut ranges) = self.ranges.take() {
            ranges.front.final_head = self
                .index
                .iter()
                .rfind(|e| e.tag == HEAD)
                .context("missing film header")?
                .offset;
            ensure!(ranges.has_summary, "missing film summary");
            let front = serde_json::to_vec(&ranges.front)?;
            ensure!(
                front.len() <= range::FRONT_CAPACITY,
                "public film header exceeds capacity"
            );
            let raw = serde_json::to_vec(&ranges.directory)?;
            ensure!(
                raw.len() <= range::MAX_INDEX_BYTES,
                "film range directory exceeds limit"
            );
            ensure!(self.index.len() < MAX_ENTRIES, "film exceeds chunk limits");
            let at = self.out.stream_position()?;
            self.write(range::CIDX, &raw)?;
            Some((front, at, raw.len() as u32))
        } else {
            None
        };
        let index = std::mem::take(&mut self.index);
        let offset = self.out.stream_position()?;
        let directory = if self.version == 3 {
            machine::encode(&index)?
        } else {
            serde_json::to_vec(&index)?
        };
        self.write(INDX, &directory)?;
        if let Some((front, cidx_offset, cidx_raw_size)) = public {
            let end = self.out.stream_position()?;
            self.out.seek(SeekFrom::Start(0))?;
            self.out.write_all(
                &range::Prefix {
                    json_len: front.len() as u32,
                }
                .encode(),
            )?;
            self.out.write_all(&front)?;
            self.out.seek(SeekFrom::Start(end))?;
            let footer = range::Footer {
                cidx_offset,
                indx_offset: offset,
                cidx_raw_size,
                file_len: end
                    .checked_add(range::FOOTER_BYTES as u64)
                    .context("film length overflow")?,
            };
            self.out.write_all(&footer.encode())?;
        } else if self.version == 2 {
            self.out.write_all(FOOTER_MAGIC)?;
            self.out.write_all(&offset.to_le_bytes())?;
        }
        self.out.flush()?;
        Ok(self.out)
    }
}

pub struct Reader<R: Read + Seek> {
    input: R,
    pub header: Header,
    pub front: Option<range::Front>,
    pub ranges: Option<range::Directory>,
    pub entries: Vec<Entry>,
    pub finalized: bool,
}
impl Reader<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path.as_ref()).with_context(|| path.as_ref().display().to_string())?)
    }
}
impl<R: Read + Seek> Reader<R> {
    pub fn new(mut input: R) -> Result<Self> {
        let end = input.seek(SeekFrom::End(0))?;
        let mut magic = [0; 8];
        input.rewind()?;
        input.read_exact(&mut magic)?;
        if magic == range::MAGIC {
            return range::open(input, end);
        }
        let first = prefix(&mut input, 0)?;
        ensure!(first.tag == HEAD, "film must begin with HEAD");
        let initial: Header = decode(&mut input, &first)?;
        match initial.version {
            2 => return Self::indexed(input, end, first, initial),
            1 => {}
            v => anyhow::bail!("unsupported film version {v}"),
        }
        let mut offset = 0;
        let mut entries = vec![];
        let mut head: Option<Header> = None;
        let mut finalized = false;
        while end - offset >= 8 {
            input.seek(SeekFrom::Start(offset))?;
            let mut prefix = [0; 8];
            input.read_exact(&mut prefix)?;
            let tag: Tag = prefix[..4].try_into()?;
            let size = u32::from_le_bytes(prefix[4..].try_into()?);
            let entry = Entry { tag, offset, size };
            if u64::from(size) > end - offset - 8 {
                break;
            }
            ensure!(
                size as usize <= MAX_CHUNK && entries.len() < MAX_ENTRIES,
                "film exceeds chunk limits"
            );
            ensure!(offset != 0 || tag == HEAD, "film must begin with HEAD");
            offset += 8 + u64::from(size);
            if tag == INDX {
                let stored: Vec<Entry> = decode(&mut input, &entry)?;
                ensure!(offset == end && stored == entries, "invalid film index");
                finalized = true;
                break;
            }
            if tag == HEAD {
                let h: Header = decode(&mut input, &entry)?;
                ensure!(h.version == 1, "unsupported film version {}", h.version);
                if let Some(old) = &head {
                    ensure!(
                        h.meta.id == old.meta.id && h.meta.image_stamp == old.meta.image_stamp,
                        "inconsistent film header"
                    );
                }
                head = Some(h);
            }
            entries.push(entry);
        }
        Ok(Self {
            input,
            front: None,
            ranges: None,
            entries,
            header: head.context("missing complete film header")?,
            finalized,
        })
    }
    // v2 is published only after the footer is durable. Menu and playback can
    // reach the last HEAD and SUMM without inflating or scanning the match.
    fn indexed(mut input: R, end: u64, first: Entry, initial: Header) -> Result<Self> {
        let footer = end.checked_sub(16).context("missing v2 film footer")?;
        input.seek(SeekFrom::Start(footer))?;
        let mut tail = [0; 16];
        input.read_exact(&mut tail)?;
        ensure!(&tail[..8] == FOOTER_MAGIC, "missing v2 film footer");
        let at = u64::from_le_bytes(tail[8..].try_into()?);
        ensure!(at < footer, "invalid v2 index offset");
        let index = prefix(&mut input, at)?;
        ensure!(
            index.tag == INDX && at.checked_add(8 + u64::from(index.size)) == Some(footer),
            "invalid v2 index bounds"
        );
        let entries: Vec<Entry> = decode(&mut input, &index)?;
        ensure!(
            entries.len() <= MAX_ENTRIES && entries.first() == Some(&first),
            "invalid v2 film index"
        );
        let mut expected = 0u64;
        for e in &entries {
            ensure!(
                e.offset == expected && e.tag != INDX && e.size as usize <= MAX_CHUNK,
                "invalid v2 film directory entry"
            );
            expected = expected
                .checked_add(8 + u64::from(e.size))
                .context("film offset overflow")?;
        }
        ensure!(expected == at, "incomplete v2 film directory");
        let last = entries
            .iter()
            .rfind(|e| e.tag == HEAD)
            .context("missing film header")?;
        let header: Header = decode(&mut input, last)?;
        ensure!(
            header.version == 2
                && header.meta.id == initial.meta.id
                && header.meta.image_stamp == initial.meta.image_stamp,
            "inconsistent film header"
        );
        Ok(Self {
            input,
            front: None,
            ranges: None,
            entries,
            header,
            finalized: true,
        })
    }
    pub fn read_bytes(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        self.read_bytes_limited(entry, MAX_CHUNK)
    }
    pub fn read_bytes_limited(&mut self, entry: &Entry, limit: usize) -> Result<Vec<u8>> {
        read_bytes(&mut self.input, entry, limit.min(MAX_CHUNK))
    }
    pub fn read<T: DeserializeOwned>(&mut self, entry: &Entry) -> Result<T> {
        decode(&mut self.input, entry)
    }

    /// SHA-256 of the final HEAD payload, with the original encoding intact.
    pub fn header_digest(&mut self) -> Result<String> {
        ensure!(self.finalized, "film header is not sealed");
        let head = self
            .entries
            .iter()
            .rfind(|e| e.tag == HEAD)
            .context("missing sealed film header")?
            .clone();
        Ok(xbe_digest(&self.read_bytes(&head)?))
    }
}

fn compress_chunk(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() > MAX_CHUNK {
        return Err(ChunkTooLarge.into());
    }
    let mut frame = zstd::stream::Encoder::new(Vec::new(), 1)?;
    frame.set_pledged_src_size(Some(raw.len() as u64))?;
    frame.include_checksum(true)?;
    frame.write_all(raw)?;
    let bytes = frame.finish()?;
    if bytes.len() > MAX_CHUNK {
        return Err(ChunkTooLarge.into());
    }
    Ok(bytes)
}

fn write_envelope(out: &mut impl Write, tag: Tag, offset: u64, bytes: &[u8]) -> Result<Entry> {
    let size = u32::try_from(bytes.len())?;
    out.write_all(&tag)?;
    out.write_all(&size.to_le_bytes())?;
    out.write_all(bytes)?;
    Ok(Entry { tag, offset, size })
}
fn decode<R: Read + Seek, T: DeserializeOwned>(input: &mut R, e: &Entry) -> Result<T> {
    Ok(serde_json::from_slice(&read_bytes(input, e, MAX_CHUNK)?)?)
}
fn prefix<R: Read + Seek>(input: &mut R, offset: u64) -> Result<Entry> {
    input.seek(SeekFrom::Start(offset))?;
    let mut buf = [0; 8];
    input.read_exact(&mut buf)?;
    Ok(Entry {
        tag: buf[..4].try_into()?,
        offset,
        size: u32::from_le_bytes(buf[4..].try_into()?),
    })
}
fn read_bytes<R: Read + Seek>(input: &mut R, e: &Entry, limit: usize) -> Result<Vec<u8>> {
    ensure!(e.size as usize <= MAX_CHUNK, "film chunk too large");
    ensure!(
        prefix(input, e.offset)? == *e,
        "film index differs from chunk"
    );
    let mut decoder = zstd::stream::read::Decoder::new(input.take(e.size.into()))?.single_frame();
    decoder.window_log_max(27)?;
    let mut raw = Vec::new();
    decoder
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut raw)?;
    ensure!(raw.len() <= limit, "inflated film chunk too large");
    // A successful decode of the first frame must consume exactly the range
    // the directory advertised, including its checksum.
    let rest = decoder.finish();
    ensure!(
        rest.buffer().is_empty() && rest.get_ref().limit() == 0,
        "film chunk has extra frames or an incorrect compressed length"
    );
    Ok(raw)
}

#[cfg(test)]
mod tests;

pub fn xbe_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
