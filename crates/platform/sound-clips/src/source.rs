//! [`ClipSource`]: the keyed clip cache and chunk streaming.
//! docs/engine/sound-instances.md, "Clip source".
//!
//! A [`Library`] says where a permutation's chunks are; the source decodes
//! them. [`ClipSource::get`] never blocks the caller: the first request for
//! a key queues it on one worker thread and answers [`Got::Pending`], later
//! ones answer [`Got::Ready`] with the shared clip (or the error, kept).
//! The cache is bounded by decoded bytes. It drops the least recently used
//! clip nobody else holds, and never one a voice still holds an `Arc` to.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use map::TagId;
use tags::sound::{Compression, Encoding, Gestalt, ResourceReader, SoundHeader};

use crate::{
    Clip, ClipError, NEEDS_EXECUTABLE, adpcm_format, decode, decode_adpcm, decode_wma,
    needs_executable,
};

/// Names one permutation: a sound tag, a pitch range, a permutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClipKey {
    pub tag: TagId,
    pub range: u16,
    pub permutation: u16,
}

/// Where the encoded bytes of sounds are. Implemented over a map's gestalt
/// and resource files by [`GestaltLibrary`].
pub trait Library: Send + Sync + 'static {
    /// The `snd!` header of `tag`.
    fn header(&self, tag: TagId) -> Result<SoundHeader, ClipError>;
    /// How many chunks the permutation `key` names has.
    fn chunk_count(&self, key: &ClipKey) -> Result<usize, ClipError>;
    /// The encoded bytes of chunk `index` of that permutation.
    fn read_chunk(&self, key: &ClipKey, index: usize) -> Result<Vec<u8>, ClipError>;
}

/// A [`Library`] over one gestalt, the headers of the sounds it serves and
/// a resource reader.
pub struct GestaltLibrary {
    gestalt: Gestalt,
    headers: HashMap<TagId, SoundHeader>,
    reader: Mutex<Box<dyn ResourceReader + Send>>,
}

impl GestaltLibrary {
    pub fn new(gestalt: Gestalt, reader: Box<dyn ResourceReader + Send>) -> Self {
        Self {
            gestalt,
            headers: HashMap::new(),
            reader: Mutex::new(reader),
        }
    }

    /// Serve the sound `tag` with `header`.
    pub fn add(&mut self, tag: TagId, header: SoundHeader) {
        self.headers.insert(tag, header);
    }

    /// Every `snd!` tag of `map` over its gestalt. A tag whose header does
    /// not read is left out, so asking for it fails by name.
    pub fn from_map<R: std::io::Read + std::io::Seek>(
        file: &mut map::Map<R>,
        reader: Box<dyn ResourceReader + Send>,
    ) -> Result<Self, ClipError> {
        let gestalt = Gestalt::read(file).map_err(|e| ClipError::Failed(e.to_string()))?;
        let sounds: Vec<TagId> = file
            .tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"snd!"))
            .map(|t| t.id)
            .collect();
        let mut library = Self::new(gestalt, reader);
        for id in sounds {
            if let Ok(header) = tags::sound::read_header(file, id) {
                library.add(id, header);
            }
        }
        Ok(library)
    }

    fn chunk(&self, key: &ClipKey, index: usize) -> Result<tags::sound::Chunk, ClipError> {
        let header = self.header(key.tag)?;
        let sound = self
            .gestalt
            .resolve(&header)
            .map_err(|e| ClipError::Failed(e.to_string()))?;
        let p = sound
            .ranges
            .get(usize::from(key.range))
            .and_then(|r| r.permutations.get(usize::from(key.permutation)))
            .ok_or_else(|| ClipError::Failed("the sound has no such permutation".into()))?;
        self.gestalt
            .chunks_of(p)
            .get(index)
            .copied()
            .ok_or_else(|| ClipError::Failed("the permutation has no such chunk".into()))
    }
}

impl Library for GestaltLibrary {
    fn header(&self, tag: TagId) -> Result<SoundHeader, ClipError> {
        self.headers
            .get(&tag)
            .cloned()
            .ok_or_else(|| ClipError::Failed(format!("sound {tag:?} is not in the library")))
    }

    fn chunk_count(&self, key: &ClipKey) -> Result<usize, ClipError> {
        let header = self.header(key.tag)?;
        let sound = self
            .gestalt
            .resolve(&header)
            .map_err(|e| ClipError::Failed(e.to_string()))?;
        let p = sound
            .ranges
            .get(usize::from(key.range))
            .and_then(|r| r.permutations.get(usize::from(key.permutation)))
            .ok_or_else(|| ClipError::Failed("the sound has no such permutation".into()))?;
        Ok(self.gestalt.chunks_of(p).len())
    }

    fn read_chunk(&self, key: &ClipKey, index: usize) -> Result<Vec<u8>, ClipError> {
        let c = self.chunk(key, index)?;
        let mut reader = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        reader
            .check(c.pointer, c.size)
            .and_then(|()| reader.read_raw(c.pointer, c.size))
            .map_err(|e| ClipError::Failed(e.to_string()))
    }
}

/// What [`ClipSource::get`] has for a key.
#[derive(Clone, Debug)]
pub enum Got {
    Ready(Arc<Clip>),
    /// Queued or decoding; ask again.
    Pending,
}

/// What the source counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Permutations decoded (not cache answers).
    pub decoded: u64,
    /// Codec streams asked for with no executable tables.
    pub refused_without_tables: u64,
    /// Clips dropped to stay inside the budget.
    pub evicted: u64,
    /// Clips and decoded bytes in the cache now.
    pub cached_clips: usize,
    pub cached_bytes: usize,
}

enum Entry {
    Pending,
    Ready { clip: Arc<Clip>, used: u64 },
    Failed(ClipError),
}

#[derive(Default)]
struct State {
    entries: HashMap<ClipKey, Entry>,
    bytes: usize,
    clock: u64,
}

struct Inner {
    library: Arc<dyn Library>,
    tables: Option<Arc<wma::Tables>>,
    budget: usize,
    state: Mutex<State>,
    done: Condvar,
    decoded: AtomicU64,
    refused: AtomicU64,
    evicted: AtomicU64,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn encoded(&self, key: &ClipKey) -> Result<Vec<u8>, ClipError> {
        let mut data = Vec::new();
        for i in 0..self.library.chunk_count(key)? {
            data.extend(self.library.read_chunk(key, i)?);
        }
        Ok(data)
    }

    fn decode_key(&self, key: &ClipKey) -> Result<Clip, ClipError> {
        let header = self.library.header(key.tag)?;
        let data = self.encoded(key)?;
        decode(&header, &data, self.tables.as_deref())
    }

    fn finish(&self, key: ClipKey, result: Result<Clip, ClipError>) {
        let mut s = self.lock();
        s.clock += 1;
        let used = s.clock;
        let entry = match result {
            Ok(clip) => {
                self.decoded.fetch_add(1, Ordering::Relaxed);
                s.bytes += clip.pcm.len() * 2;
                Entry::Ready {
                    clip: Arc::new(clip),
                    used,
                }
            }
            Err(e) => Entry::Failed(e),
        };
        s.entries.insert(key, entry);
        self.evict(&mut s, key);
        drop(s);
        self.done.notify_all();
    }

    /// Drop least recently used clips only the cache holds until the decoded
    /// bytes fit the budget. `keep` is never dropped.
    fn evict(&self, s: &mut State, keep: ClipKey) {
        while s.bytes > self.budget {
            let victim = s
                .entries
                .iter()
                .filter_map(|(k, e)| match e {
                    Entry::Ready { clip, used } if *k != keep && Arc::strong_count(clip) == 1 => {
                        Some((*used, *k))
                    }
                    _ => None,
                })
                .min_by_key(|(used, _)| *used);
            let Some((_, k)) = victim else { return };
            if let Some(Entry::Ready { clip, .. }) = s.entries.remove(&k) {
                s.bytes -= clip.pcm.len() * 2;
                self.evicted.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The keyed clip cache: one decode per key, on a worker thread.
pub struct ClipSource {
    inner: Arc<Inner>,
    queue: Sender<ClipKey>,
}

impl ClipSource {
    /// Decoded bytes the cache keeps before it drops unheld clips.
    pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;

    /// A source over `library`. `tables` are the executable's codec tables,
    /// shared with boot's XWMA shim; without them a codec stream is refused.
    pub fn new(library: impl Library, tables: Option<Arc<wma::Tables>>, budget: usize) -> Self {
        let inner = Arc::new(Inner {
            library: Arc::new(library),
            tables,
            budget,
            state: Mutex::new(State::default()),
            done: Condvar::new(),
            decoded: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
        });
        let (queue, jobs) = mpsc::channel::<ClipKey>();
        let worker = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("sound-clips".into())
            .spawn(move || {
                while let Ok(key) = jobs.recv() {
                    let result = worker.decode_key(&key);
                    worker.finish(key, result);
                }
            })
            .expect("the sound-clips worker thread starts");
        Self { inner, queue }
    }

    /// The clip for `key`: ready, still decoding, or why it cannot be.
    pub fn get(&self, key: ClipKey) -> Result<Got, ClipError> {
        let mut s = self.inner.lock();
        s.clock += 1;
        let now = s.clock;
        match s.entries.get_mut(&key) {
            Some(Entry::Ready { clip, used }) => {
                *used = now;
                return Ok(Got::Ready(Arc::clone(clip)));
            }
            Some(Entry::Pending) => return Ok(Got::Pending),
            Some(Entry::Failed(e)) => return Err(e.clone()),
            None => {}
        }
        // A refusal needs no worker: the header says it.
        let refused = self.inner.library.header(key.tag).and_then(|h| {
            if needs_executable(&h) && self.inner.tables.is_none() {
                self.inner.refused.fetch_add(1, Ordering::Relaxed);
                Err(ClipError::Refused(NEEDS_EXECUTABLE.into()))
            } else {
                adpcm_or_codec(&h)
            }
        });
        if let Err(e) = refused {
            s.entries.insert(key, Entry::Failed(e.clone()));
            return Err(e);
        }
        s.entries.insert(key, Entry::Pending);
        drop(s);
        let _ = self.queue.send(key);
        Ok(Got::Pending)
    }

    /// [`Self::get`], waiting up to `timeout` for the decode.
    pub fn wait(&self, key: ClipKey, timeout: Duration) -> Result<Arc<Clip>, ClipError> {
        let end = Instant::now() + timeout;
        loop {
            if let Got::Ready(c) = self.get(key)? {
                return Ok(c);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ClipError::Failed("the clip did not decode in time".into()));
            }
            let s = self.inner.lock();
            if matches!(s.entries.get(&key), Some(Entry::Pending)) {
                let _ = self.inner.done.wait_timeout(s, left);
            }
        }
    }

    /// A long permutation chunk by chunk, for `audio::Stream`.
    pub fn stream(&self, key: ClipKey) -> Result<ChunkReader, ClipError> {
        let header = self.inner.library.header(key.tag)?;
        if needs_executable(&header) && self.inner.tables.is_none() {
            self.inner.refused.fetch_add(1, Ordering::Relaxed);
            return Err(ClipError::Refused(NEEDS_EXECUTABLE.into()));
        }
        let kind = match adpcm_or_codec(&header)? {
            Kind::Adpcm { channels, rate } => ReaderKind::Adpcm { channels, rate },
            Kind::Codec => ReaderKind::Codec { decoded: None },
        };
        let count = self.inner.library.chunk_count(&key)?;
        let (channels, rate) = match &kind {
            ReaderKind::Adpcm { channels, rate } => (*channels as u16, *rate),
            ReaderKind::Codec { .. } => (2, 44100),
        };
        Ok(ChunkReader {
            inner: Arc::clone(&self.inner),
            key,
            kind,
            next: 0,
            count,
            carry: Vec::new(),
            channels,
            rate,
        })
    }

    pub fn stats(&self) -> Stats {
        let s = self.inner.lock();
        Stats {
            decoded: self.inner.decoded.load(Ordering::Relaxed),
            refused_without_tables: self.inner.refused.load(Ordering::Relaxed),
            evicted: self.inner.evicted.load(Ordering::Relaxed),
            cached_clips: s
                .entries
                .values()
                .filter(|e| matches!(e, Entry::Ready { .. }))
                .count(),
            cached_bytes: s.bytes,
        }
    }
}

enum Kind {
    Adpcm { channels: usize, rate: u32 },
    Codec,
}

/// Which decoder a header takes, or why none does.
fn adpcm_or_codec(h: &SoundHeader) -> Result<Kind, ClipError> {
    match (h.compression, h.encoding) {
        (Compression::Wma | Compression::XboxAdpcm, Encoding::Codec) => Ok(Kind::Codec),
        _ => {
            let (channels, rate) = adpcm_format(h)?;
            Ok(Kind::Adpcm { channels, rate })
        }
    }
}

/// One run of decoded interleaved samples out of a [`ChunkReader`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmChunk {
    pub pcm: Vec<i16>,
}

enum ReaderKind {
    Adpcm {
        channels: usize,
        rate: u32,
    },
    /// A codec stream cannot be cut at a chunk: it decodes whole on the
    /// first read, then hands out [`ChunkReader::CODEC_FRAMES`] at a time.
    Codec {
        decoded: Option<(Arc<[i16]>, usize)>,
    },
}

/// A permutation decoded one chunk at a time. Xbox ADPCM blocks are
/// independent, so each read decodes the whole blocks the chunks so far
/// hold and carries the rest into the next.
pub struct ChunkReader {
    inner: Arc<Inner>,
    key: ClipKey,
    kind: ReaderKind,
    next: usize,
    count: usize,
    carry: Vec<u8>,
    pub channels: u16,
    pub rate: u32,
}

impl ChunkReader {
    /// Frames a codec stream hands out per read.
    pub const CODEC_FRAMES: usize = 16 * 1024;

    /// The next run of samples, or `None` at the end.
    pub fn next_chunk(&mut self) -> Result<Option<PcmChunk>, ClipError> {
        match &mut self.kind {
            ReaderKind::Adpcm { channels, rate } => {
                let (channels, rate) = (*channels, *rate);
                let unit = 36 * channels;
                while self.next < self.count {
                    let bytes = self.inner.library.read_chunk(&self.key, self.next)?;
                    self.next += 1;
                    self.carry.extend(bytes);
                    let whole = self.carry.len() / unit * unit;
                    if whole == 0 {
                        continue;
                    }
                    let rest = self.carry.split_off(whole);
                    let data = std::mem::replace(&mut self.carry, rest);
                    let clip = decode_adpcm(&data, channels, rate)?;
                    return Ok(Some(PcmChunk {
                        pcm: clip.pcm.to_vec(),
                    }));
                }
                if self.carry.is_empty() {
                    Ok(None)
                } else {
                    let n = self.carry.len();
                    self.carry.clear();
                    Err(ClipError::Failed(format!(
                        "Xbox ADPCM: {n} bytes left over that are not a whole block"
                    )))
                }
            }
            ReaderKind::Codec { decoded } => {
                if decoded.is_none() {
                    let tables = self
                        .inner
                        .tables
                        .as_deref()
                        .ok_or_else(|| ClipError::Refused(NEEDS_EXECUTABLE.into()))?;
                    let data = self.inner.encoded(&self.key)?;
                    *decoded = Some((decode_wma(tables, &data)?.pcm, 0));
                }
                let (pcm, at) = decoded.as_mut().expect("decoded above");
                if *at >= pcm.len() {
                    return Ok(None);
                }
                let end = (*at + Self::CODEC_FRAMES * 2).min(pcm.len());
                let chunk = PcmChunk {
                    pcm: pcm[*at..end].to_vec(),
                };
                *at = end;
                Ok(Some(chunk))
            }
        }
    }
}
