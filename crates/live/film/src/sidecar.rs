//! Disposable, build-specific machine keyframes. The sealed film remains the
//! source of the input and HLE streams.
use crate::{
    Entry, KEYF, MARK, MAX_CHUNK, Reader, blam, compress_chunk, prefix, read_bytes,
    tape::{Mark, SidecarCursor},
    write_envelope, xbe_digest,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_CAP_BYTES: u64 = 4 << 30;
const MAGIC: &[u8; 8] = b"H2KEYS1!";
const MAX_HEADER: usize = 64 << 10;
const MAX_INDEX: usize = 8 << 20;
const MAX_FRAMES: usize = 32_768;
const TAIL_BYTES: u64 = 8;
const WATCHED: &str = ".watched";
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    film_id: String,
    header_digest: String,
    stamp: String,
    switches: String,
    generation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Key {
    tick: u64,
    slices: u64,
    offset: u64,
    size: u32,
    sha256: String,
    ram_anchor: Option<u64>,
    #[serde(flatten)]
    cursor: SidecarCursor,
}

impl Key {
    fn entry(&self) -> Entry {
        Entry {
            tag: KEYF,
            offset: self.offset,
            size: self.size,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Directory {
    keyframes: Vec<Key>,
}

/// A checked directory and the open file it describes. Payloads are decoded
/// and checked on demand, with at most one full anchor per restored frame.
pub struct SidecarReader {
    path: PathBuf,
    file: File,
    id: Identity,
    len: u64,
    keys: Vec<Key>,
}

impl SidecarReader {
    /// A missing, incompatible or damaged cache is a miss. Successful opens
    /// mark the film as watched, across all builds.
    pub fn open<R: Read + Seek>(
        films_dir: &Path,
        film: &mut Reader<R>,
        stamp: &str,
        switches: &str,
    ) -> Result<Option<Self>> {
        let cache = films_dir.join("cache");
        let Some(path) = build_path(&cache, &film.header.meta.id, stamp, switches) else {
            return Ok(None);
        };
        let Some(_lock) = lock_cache(&cache, false)? else {
            return Ok(None);
        };
        let dir = path.parent().unwrap();
        if !is_directory(dir)? {
            return Ok(None);
        }
        touch(dir)?;
        let Some(file) = open_regular(&path)? else {
            return Ok(None);
        };
        let sidecar = match Self::read_index(path.clone(), file) {
            Ok(sidecar) => sidecar,
            Err(_) => {
                remove_file(&path)?;
                return Ok(None);
            }
        };
        // The short filename hash can collide, so always compare the full key.
        if sidecar.id.stamp != stamp || sidecar.id.switches != switches {
            return Ok(None);
        }
        if sidecar.id.film_id != film.header.meta.id
            || sidecar.id.header_digest != film.header_digest()?
            || validate_marks(&sidecar.keys, film).is_err()
        {
            drop(sidecar);
            remove_file(&path)?;
            return Ok(None);
        }
        Ok(Some(sidecar))
    }

    fn read_index(path: PathBuf, mut file: File) -> Result<Self> {
        let len = file.metadata()?.len();
        let (id, data_start) = read_header(&mut file)?;
        let footer = len
            .checked_sub(TAIL_BYTES)
            .context("missing sidecar footer")?;
        file.seek(SeekFrom::Start(footer))?;
        let index_offset = read_u64(&mut file)?;
        ensure!(
            (data_start..footer).contains(&index_offset),
            "invalid sidecar index offset"
        );
        let entry = prefix(&mut file, index_offset)?;
        ensure!(
            entry.tag == blam::TIDX
                && index_offset.checked_add(8 + u64::from(entry.size)) == Some(footer),
            "invalid sidecar index envelope"
        );
        let dir: Directory = serde_json::from_slice(&read_bytes(&mut file, &entry, MAX_INDEX)?)?;
        let keys = dir.keyframes;
        ensure!(
            !keys.is_empty() && keys.len() <= MAX_FRAMES,
            "invalid sidecar keyframe count"
        );
        ensure!(
            keys.windows(2).all(|p| p[0].tick < p[1].tick),
            "unordered sidecar keyframes"
        );
        for key in &keys {
            if let Some(tick) = key.ram_anchor {
                let base = keys
                    .binary_search_by_key(&tick, |row| row.tick)
                    .ok()
                    .map(|i| &keys[i])
                    .context("missing sidecar RAM anchor")?;
                ensure!(
                    tick < key.tick && base.ram_anchor.is_none(),
                    "invalid sidecar RAM anchor"
                );
            }
        }
        let mut physical: Vec<_> = keys.iter().collect();
        physical.sort_unstable_by_key(|row| row.offset);
        let mut expected = data_start;
        for key in physical {
            ensure!(
                key.offset == expected
                    && key.size > 0
                    && key.size as usize <= MAX_CHUNK
                    && prefix(&mut file, key.offset)? == key.entry(),
                "invalid sidecar keyframe envelope"
            );
            expected = expected
                .checked_add(8 + u64::from(key.size))
                .context("sidecar offset overflow")?;
            ensure!(
                expected <= index_offset,
                "sidecar keyframe overlaps directory"
            );
        }
        ensure!(expected == index_offset, "incomplete sidecar directory");
        Ok(Self {
            path,
            file,
            id,
            len,
            keys,
        })
    }

    pub fn ticks(&self) -> impl DoubleEndedIterator<Item = u64> + '_ {
        self.keys.iter().map(|key| key.tick)
    }

    pub fn bytes(&self) -> u64 {
        self.len
    }

    pub(crate) fn nearest(&self, target: u64) -> Option<usize> {
        self.keys
            .partition_point(|key| key.tick <= target)
            .checked_sub(1)
    }

    pub(crate) fn tick(&self, index: usize) -> u64 {
        self.keys[index].tick
    }

    pub(crate) fn cursor(&self, index: usize) -> SidecarCursor {
        self.keys[index].cursor
    }

    fn read_raw(&mut self, index: usize) -> Result<Vec<u8>> {
        let key = &self.keys[index];
        let raw = read_bytes(&mut self.file, &key.entry(), MAX_CHUNK)?;
        ensure!(
            xbe_digest(&raw) == key.sha256,
            "corrupt sidecar keyframe at tick {}",
            key.tick
        );
        Ok(raw)
    }

    fn read_one(&mut self, index: usize) -> Result<blam::Keyframe> {
        let raw = self.read_raw(index)?;
        let key = &self.keys[index];
        let frame = blam::decode_keyframe(raw, key.tick)?;
        ensure!(
            frame.slices == key.slices,
            "sidecar keyframe disagrees with capture point"
        );
        Ok(frame)
    }

    pub(crate) fn verify_payloads(&mut self) -> Result<()> {
        for index in 0..self.keys.len() {
            self.read_one(index)?;
        }
        Ok(())
    }

    pub(crate) fn disk_delta(&mut self, index: usize) -> Result<Vec<u8>> {
        let raw = self.read_raw(index)?;
        let key = &self.keys[index];
        let ([slices, ..], disk, _) = blam::split_keyframe(&raw, key.tick)?;
        ensure!(
            slices == key.slices,
            "sidecar keyframe disagrees with capture point"
        );
        Ok(disk.to_vec())
    }

    pub(crate) fn keyframe(&mut self, index: usize) -> Result<blam::Keyframe> {
        let mut frame = self.read_one(index)?;
        if let Some(tick) = self.keys[index].ram_anchor {
            let anchor = self
                .keys
                .binary_search_by_key(&tick, |key| key.tick)
                .map_err(|_| anyhow::anyhow!("missing sidecar RAM anchor"))?;
            frame.ram_anchor = Some((tick, self.read_one(anchor)?.snapshot));
        }
        Ok(frame)
    }

    /// Discard only the generation this reader checked. A newer sidecar
    /// published while this film was playing remains available.
    pub fn invalidate(self) -> Result<()> {
        let cache = self.path.parent().unwrap().parent().unwrap();
        let Some(_lock) = lock_cache(cache, false)? else {
            return Ok(());
        };
        let Some(mut current) = open_regular(&self.path)? else {
            return Ok(());
        };
        let remove = read_header(&mut current).map_or(true, |(id, _)| id == self.id);
        drop(current);
        drop(self.file);
        if remove {
            remove_file(&self.path)?;
        }
        Ok(())
    }
}

/// Stage optional machine frames privately, then publish a complete cache
/// against the sealed film. Dropping the writer discards the staging file.
pub struct SidecarWriter {
    path: PathBuf,
    stage: PathBuf,
    file: Option<File>,
    id: Identity,
    cap_bytes: u64,
    failed: bool,
    inherited: Option<SidecarReader>,
    keys: BTreeMap<u64, Key>,
}

impl SidecarWriter {
    pub fn new(
        films_dir: &Path,
        film_id: &str,
        stamp: &str,
        switches: &str,
        cap_bytes: u64,
    ) -> Result<Self> {
        Self::create(films_dir, film_id, stamp, switches, cap_bytes, None)
    }

    /// Extend a valid sidecar without losing existing keyframes on pause or
    /// rewind. If the cache is missing, start a fresh generation.
    pub fn resume<R: Read + Seek>(
        films_dir: &Path,
        film: &mut Reader<R>,
        stamp: &str,
        switches: &str,
        cap_bytes: u64,
    ) -> Result<Self> {
        let inherited = SidecarReader::open(films_dir, film, stamp, switches)?;
        Self::create(
            films_dir,
            &film.header.meta.id,
            stamp,
            switches,
            cap_bytes,
            inherited,
        )
    }

    fn create(
        films_dir: &Path,
        film_id: &str,
        stamp: &str,
        switches: &str,
        cap_bytes: u64,
        inherited: Option<SidecarReader>,
    ) -> Result<Self> {
        let cache = films_dir.join("cache");
        let path =
            build_path(&cache, film_id, stamp, switches).context("unsupported sidecar film ID")?;
        ensure!(cap_bytes > 0, "film sidecar cache is disabled");
        ensure!(
            stamp.len() <= MAX_HEADER && switches.len() <= MAX_HEADER,
            "sidecar build key exceeds limit"
        );
        fs::create_dir_all(films_dir)?;
        let _lock = lock_cache(&cache, true)?.context("missing film sidecar cache")?;
        let dir = path.parent().unwrap();
        ensure_directory(dir)?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let (file, stage, generation) = loop {
            let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let generation = format!("{}-{time:x}-{sequence:x}", std::process::id());
            let stage = dir.join(format!(".stage-{generation}"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&stage) {
                Ok(file) => break (file, stage, generation),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.into()),
            }
        };
        let mut writer = Self {
            path,
            stage,
            file: Some(file),
            id: Identity {
                film_id: film_id.into(),
                header_digest: "0".repeat(64),
                stamp: stamp.into(),
                switches: switches.into(),
                generation,
            },
            cap_bytes,
            failed: false,
            inherited,
            keys: BTreeMap::new(),
        };
        write_header(writer.file.as_mut().unwrap(), &writer.id)?;
        Ok(writer)
    }

    fn at(&self, tick: u64) -> Option<&Key> {
        self.keys.get(&tick).or_else(|| {
            let base = self.inherited.as_ref()?;
            base.keys
                .binary_search_by_key(&tick, |key| key.tick)
                .ok()
                .map(|i| &base.keys[i])
        })
    }

    pub fn contains(&self, tick: u64) -> bool {
        self.at(tick).is_some()
    }

    /// Store the unchanged KEYF payload. The MARK is the capture point in
    /// the film and must separate consumed and remaining call batches.
    pub fn append(
        &mut self,
        frame: &blam::Keyframe,
        anchor_tick: Option<u64>,
        mark_offset: u64,
    ) -> Result<()> {
        self.append_at(frame, anchor_tick, SidecarCursor::new(mark_offset))
    }

    pub fn append_at(
        &mut self,
        frame: &blam::Keyframe,
        anchor_tick: Option<u64>,
        cursor: SidecarCursor,
    ) -> Result<()> {
        ensure!(!self.failed, "sidecar output already failed");
        ensure!(!self.contains(frame.tick), "duplicate sidecar keyframe");
        let inherited_count = self.inherited.as_ref().map_or(0, |base| base.keys.len());
        ensure!(
            inherited_count + self.keys.len() < MAX_FRAMES,
            "too many sidecar keyframes"
        );
        if let Some(tick) = anchor_tick {
            ensure!(
                tick < frame.tick && self.at(tick).is_some_and(|key| key.ram_anchor.is_none()),
                "sidecar keyframe needs an earlier standalone RAM anchor"
            );
        }
        let raw = blam::encode_keyframe(frame)?;
        let compressed = compress_chunk(&raw)?;
        let file = self.file.as_mut().unwrap();
        let offset = file.stream_position()?;
        ensure!(
            offset.saturating_add(8 + compressed.len() as u64 + TAIL_BYTES) <= self.cap_bytes,
            "sidecar exceeds cache cap"
        );
        self.failed = true;
        let entry = write_envelope(file, KEYF, offset, &compressed)?;
        self.keys.insert(
            frame.tick,
            Key {
                tick: frame.tick,
                slices: frame.slices,
                offset,
                size: entry.size,
                sha256: xbe_digest(&raw),
                ram_anchor: anchor_tick,
                cursor,
            },
        );
        self.failed = false;
        Ok(())
    }

    pub fn finish(mut self, film_path: &Path) -> Result<()> {
        ensure!(!self.failed, "sidecar output already failed");
        if self.keys.is_empty() {
            return Ok(());
        }
        // Copy inherited envelopes without decoding their snapshots. A reader
        // still checks each payload and its anchor before using them.
        let file = self.file.as_mut().unwrap();
        if let Some(base) = self.inherited.as_mut() {
            for key in &base.keys {
                let mut key = key.clone();
                let size = 8 + u64::from(key.size);
                let offset = file.stream_position()?;
                ensure!(
                    offset.saturating_add(size + TAIL_BYTES) <= self.cap_bytes,
                    "sidecar exceeds cache cap"
                );
                base.file.seek(SeekFrom::Start(key.offset))?;
                ensure!(
                    io::copy(&mut (&mut base.file).take(size), file)? == size,
                    "truncated sidecar keyframe"
                );
                key.offset = offset;
                self.keys.insert(key.tick, key);
            }
        }
        let keys: Vec<_> = std::mem::take(&mut self.keys).into_values().collect();
        let cache = self.path.parent().unwrap().parent().unwrap();
        let _lock = lock_cache(cache, true)?.context("missing film sidecar cache")?;
        let mut film = Reader::open(film_path)?;
        ensure!(
            self.id.film_id == film.header.meta.id,
            "sidecar film ID changed"
        );
        self.id.header_digest = film.header_digest()?;
        validate_marks(&keys, &mut film)?;
        if let Some(base) = self.inherited.as_ref() {
            ensure!(
                base.id.header_digest == self.id.header_digest,
                "film changed while writing its sidecar"
            );
        }
        let expected = self.inherited.as_ref().map(|base| &base.id);
        let current = open_regular(&self.path)?
            .map(|mut file| read_header(&mut file).map(|(id, _)| id))
            .transpose()?;
        ensure!(
            current.as_ref() == expected,
            "sidecar changed while writing"
        );
        let raw = serde_json::to_vec(&Directory { keyframes: keys })?;
        ensure!(raw.len() <= MAX_INDEX, "sidecar directory exceeds limit");
        let compressed = compress_chunk(&raw)?;
        let offset = file.stream_position()?;
        let len = offset.saturating_add(8 + compressed.len() as u64 + TAIL_BYTES);
        ensure!(len <= self.cap_bytes, "sidecar exceeds cache cap");
        write_envelope(file, blam::TIDX, offset, &compressed)?;
        file.write_all(&offset.to_le_bytes())?;
        file.rewind()?;
        write_header(file, &self.id)?;
        file.sync_all()?;
        drop(self.file.take());

        // Library deletion may have unlinked the staging name while it was
        // being written. In that case there is nothing to publish.
        ensure!(self.stage.is_file(), "sidecar staging file was removed");
        touch(self.path.parent().unwrap())?;
        prune_cache(cache, self.cap_bytes - len, Some(&self.path))?;
        fs::rename(&self.stage, &self.path)?;
        Ok(())
    }
}

impl Drop for SidecarWriter {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = remove_file(&self.stage);
    }
}

fn validate_marks<R: Read + Seek>(keys: &[Key], film: &mut Reader<R>) -> Result<()> {
    let mut previous = None;
    for key in keys {
        let mark_offset = key.cursor.mark_offset;
        ensure!(
            key.tick <= film.header.meta.length_ticks
                && previous
                    .is_none_or(|(slices, offset)| slices < key.slices && offset < mark_offset),
            "invalid sidecar capture order"
        );
        let entry = film
            .entries
            .binary_search_by_key(&mark_offset, |e| e.offset)
            .ok()
            .map(|i| film.entries[i].clone())
            .context("missing sidecar capture point")?;
        ensure!(
            entry.tag == MARK,
            "sidecar capture point is not a film mark"
        );
        let mark: Mark = serde_json::from_slice(&film.read_bytes_limited(&entry, MAX_HEADER)?)?;
        ensure!(
            matches!(mark, Mark::D3dBarrier { slices, tick, saved: true } if slices == key.slices && tick == key.tick),
            "sidecar keyframe does not match the film capture point"
        );
        previous = Some((key.slices, mark_offset));
    }
    Ok(())
}

fn read_header(file: &mut File) -> Result<(Identity, u64)> {
    file.rewind()?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "invalid sidecar magic");
    let mut len = [0; 4];
    file.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    ensure!(len <= MAX_HEADER, "sidecar header exceeds limit");
    let mut raw = vec![0; len];
    file.read_exact(&mut raw)?;
    Ok((serde_json::from_slice(&raw)?, 12 + len as u64))
}

fn write_header(file: &mut File, id: &Identity) -> Result<()> {
    let raw = serde_json::to_vec(id)?;
    ensure!(raw.len() <= MAX_HEADER, "sidecar header exceeds limit");
    file.write_all(MAGIC)?;
    file.write_all(&(raw.len() as u32).to_le_bytes())?;
    file.write_all(&raw)?;
    Ok(())
}

fn read_u64(input: &mut impl Read) -> Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn build_path(cache: &Path, film_id: &str, stamp: &str, switches: &str) -> Option<PathBuf> {
    if !valid_id(film_id) {
        return None;
    }
    // Keep in step with snapshot::BuildKey::id, without linking the machine
    // snapshot implementation into film readers and the Live service.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in stamp.bytes().chain([0]).chain(switches.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    Some(cache.join(film_id).join(format!("{hash:016x}.h2keys")))
}

fn valid_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return false;
    }
    let upper = id.to_ascii_uppercase();
    !matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit())
}

/// Remove all cached builds and any staged writers for a film ID.
pub fn remove_film(films_dir: &Path, film_id: &str) -> Result<()> {
    if valid_id(film_id) {
        let cache = films_dir.join("cache");
        if let Some(_lock) = lock_cache(&cache, false)? {
            remove_film_dir(&cache, film_id)?;
        }
    }
    Ok(())
}

/// Hold publication off from cache cleanup through the library's file removal.
pub fn remove_film_with(
    films_dir: &Path,
    film_id: &str,
    delete: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if !valid_id(film_id) {
        return delete();
    }
    let cache = films_dir.join("cache");
    let _lock = lock_cache(&cache, true)?.context("missing film sidecar cache")?;
    remove_film_dir(&cache, film_id)?;
    delete()
}

fn remove_film_dir(cache: &Path, film_id: &str) -> Result<()> {
    let dir = cache.join(film_id);
    if is_directory(&dir)? {
        fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// Evict the least recently watched sidecars until published bytes fit the cap.
pub fn prune(films_dir: &Path, cap_bytes: u64) -> Result<()> {
    let cache = films_dir.join("cache");
    if let Some(_lock) = lock_cache(&cache, false)? {
        prune_cache(&cache, cap_bytes, None)?;
    }
    Ok(())
}

fn prune_cache(cache: &Path, cap_bytes: u64, replaced: Option<&Path>) -> Result<()> {
    let mut files = Vec::new();
    let mut bytes = 0u64;
    for dir in fs::read_dir(cache)? {
        let dir = dir?;
        if !dir.file_type()?.is_dir() {
            continue;
        }
        let watched = fs::metadata(dir.path().join(WATCHED))
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH);
        for file in fs::read_dir(dir.path())? {
            let file = file?;
            let path = file.path();
            if path.extension().is_none_or(|ext| ext != "h2keys")
                || replaced == Some(path.as_path())
                || !file.file_type()?.is_file()
            {
                continue;
            }
            let meta = file.metadata()?;
            let size = meta.len();
            bytes = bytes
                .checked_add(size)
                .context("sidecar cache size overflow")?;
            files.push((watched, meta.modified().unwrap_or(UNIX_EPOCH), path, size));
        }
    }
    files.sort_unstable();
    for (_, _, path, size) in files {
        if bytes <= cap_bytes {
            break;
        }
        remove_file(&path)?;
        bytes -= size;
    }
    Ok(())
}

fn touch(dir: &Path) -> Result<()> {
    let marker = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(WATCHED))?;
    marker.set_modified(SystemTime::now())?;
    Ok(())
}

fn lock_cache(cache: &Path, create: bool) -> Result<Option<File>> {
    if create {
        ensure_directory(cache)?;
    } else if !is_directory(cache)? {
        return Ok(None);
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join(".lock"))?;
    lock.lock()?;
    Ok(Some(lock))
}

fn ensure_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            ensure!(is_directory(path)?, "invalid sidecar cache path");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

fn is_directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(metadata.is_dir(), "invalid sidecar cache directory");
            Ok(true)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn open_regular(path: &Path) -> Result<Option<File>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(metadata.is_file(), "invalid sidecar cache file");
            Ok(Some(File::open(path)?))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests;
