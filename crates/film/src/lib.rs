//! Each complete chunk stands alone. Offsets always refer to the four-byte tag.
mod library;
pub mod manifest;
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
const MAX_CHUNK: usize = 80 << 20;
pub const MAX_UPLOAD: u64 = 64 << 20;

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub meta: FilmMeta,
    pub title_id: u32,
    pub xbe_sha256: String,
    pub epoch_100ns: u64,
    pub lease_id: Option<String>,
    pub xnet_addr: String,
    pub transfers_per_slice: u64,
    pub lobby: bool,
    pub lobby_machine_start: bool,
    pub host_bots: bool,
    pub telemetry: bool,
    pub live_service: bool,
    pub features: String,
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
}
impl<W: Write + Seek> Writer<W> {
    pub fn new(mut out: W, header: &Header) -> Result<Self> {
        ensure!(
            out.seek(SeekFrom::End(0))? == 0,
            "film output already contains data"
        );
        let mut w = Self {
            out,
            index: vec![],
            failed: false,
        };
        w.chunk(HEAD, header)?;
        Ok(w)
    }
    pub fn chunk(&mut self, tag: Tag, value: &impl Serialize) -> Result<()> {
        ensure!(tag != INDX, "finish writes the index");
        self.write(tag, value)
    }
    fn write(&mut self, tag: Tag, value: &impl Serialize) -> Result<()> {
        ensure!(!self.failed, "film output already failed");
        let raw = serde_json::to_vec(value)?;
        ensure!(raw.len() <= MAX_CHUNK, "film chunk too large");
        let bytes = zstd::stream::encode_all(raw.as_slice(), 1)?;
        ensure!(bytes.len() <= MAX_CHUNK, "compressed film chunk too large");
        let offset = self.out.stream_position()?;
        self.failed = true;
        let size = bytes.len() as u32;
        self.out.write_all(&tag)?;
        self.out.write_all(&size.to_le_bytes())?;
        self.out.write_all(&bytes)?;
        self.index.push(Entry { tag, offset, size });
        self.failed = false;
        Ok(())
    }
    pub fn finish(mut self) -> Result<W> {
        let index = std::mem::take(&mut self.index);
        self.write(INDX, &index)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

pub struct Reader<R: Read + Seek> {
    input: R,
    pub header: Header,
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
                size as usize <= MAX_CHUNK && entries.len() < 1_000_000,
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
            entries,
            header: head.context("missing complete film header")?,
            finalized,
        })
    }
    pub fn read<T: DeserializeOwned>(&mut self, entry: &Entry) -> Result<T> {
        decode(&mut self.input, entry)
    }
}
fn decode<R: Read + Seek, T: DeserializeOwned>(input: &mut R, e: &Entry) -> Result<T> {
    ensure!(e.size as usize <= MAX_CHUNK, "film chunk too large");
    input.seek(SeekFrom::Start(
        e.offset.checked_add(8).context("invalid offset")?,
    ))?;
    let mut decoder = zstd::stream::read::Decoder::new(input.take(e.size.into()))?;
    decoder.window_log_max(27)?;
    let mut raw = Vec::new();
    decoder.take(MAX_CHUNK as u64 + 1).read_to_end(&mut raw)?;
    ensure!(raw.len() <= MAX_CHUNK, "inflated film chunk too large");
    Ok(serde_json::from_slice(&raw)?)
}

/// A completed host film, sent after the match using its telemetry.ingest token.
pub fn upload(live_url: &str, token: &str, game_id: &str, path: impl AsRef<Path>) -> Result<()> {
    let mut url = url::Url::parse(live_url)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && live_proto::bearer_allowed(url.as_str()),
        "film upload requires HTTPS or a loopback Live URL"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "invalid Live base URL"
    );
    ensure!(
        !game_id.is_empty() && game_id != "." && game_id != "..",
        "missing or invalid game ID"
    );
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid Live URL"))?
        .pop_if_empty()
        .extend(["v1", "games", game_id, "film"]);
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(size <= MAX_UPLOAD, "film exceeds 64 MiB");
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .redirects(0)
        .build()
        .post(url.as_str())
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/octet-stream")
        .set("Content-Length", &size.to_string())
        .send(file)?;
    Ok(())
}

#[cfg(test)]
mod tests;

pub fn xbe_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
