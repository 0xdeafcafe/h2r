//! The controller-speaker sound index (docs/audio/playstation-audio.md): whenever
//! the game opens a map, fingerprint the start of every `snd!` permutation
//! in it, from the player's own map file, and give DirectSound a new
//! `dsound::padroute::PadRouter`. The shared maps are indexed once at start.
//!
//! The VFS tells us about each file it opens ([`vfs::Vfs::set_open_observer`]);
//! a `.map` file whose bytes are one region of a host file (the disc image,
//! or a file under the data root) is queued for a thread of its own, which
//! opens that region read-only and reads only the tag index, the sound
//! gestalt and the first [`PREFIX_BYTES`] of each permutation. Nothing read
//! is kept but the fingerprints and tag names, and nothing is written.
//!
//! A router only changes which host output hears a voice, never what the
//! guest sees, so installing it from another thread at any time is safe.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use dsound::padroute::{PREFIX_BYTES, PadRouter, PadSpeakerConfig, RouteTable, SoundIndex};
use map::{FourCC, Map, RawPointer, ResourceFile};
use tags::sound::{Gestalt, Permutation, ResourceReader};
use vfs::HostFile;

/// The shared maps every level's raw resources may point into, and where the
/// game finds them.
pub const SHARED_MAPS: [(ResourceFile, &str); 3] = [
    (
        ResourceFile::MainMenu,
        "\\Device\\CdRom0\\maps\\mainmenu.map",
    ),
    (ResourceFile::Shared, "\\Device\\CdRom0\\maps\\shared.map"),
    (
        ResourceFile::SinglePlayerShared,
        "\\Device\\CdRom0\\maps\\single_player_shared.map",
    ),
];

fn slot(f: ResourceFile) -> usize {
    match f {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}

/// Raw-resource reads for one map: its own file and the shared maps, each a
/// bounded region of a host file.
struct Resources {
    files: [Option<(File, HostFile)>; 4],
}

impl Resources {
    fn new(local: &HostFile, shared: &[Option<HostFile>; 3]) -> Self {
        let open = |h: &HostFile| File::open(&h.path).ok().map(|f| (f, h.clone()));
        Resources {
            files: [
                open(local),
                shared[0].as_ref().and_then(open),
                shared[1].as_ref().and_then(open),
                shared[2].as_ref().and_then(open),
            ],
        }
    }
}

impl ResourceReader for Resources {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        let (_, h) = self.files[slot(p.file)]
            .as_ref()
            .ok_or(tags::Error::MissingResource(p.file))?;
        if u64::from(p.offset) + u64::from(size) > h.len {
            return Err(tags::Error::Invalid("sound chunk outside its map"));
        }
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        let (f, h) = self.files[slot(p.file)].as_mut().expect("checked");
        let io = |e: std::io::Error| tags::Error::Map(map::Error::from(e));
        f.seek(SeekFrom::Start(h.offset + u64::from(p.offset)))
            .map_err(io)?;
        let mut out = vec![0; size as usize];
        f.read_exact(&mut out).map_err(io)?;
        Ok(out)
    }
}

/// The identifying start of a permutation: its first [`PREFIX_BYTES`], or
/// all of a shorter one, read chunk by chunk.
fn prefix(g: &Gestalt, p: &Permutation, r: &mut dyn ResourceReader) -> tags::Result<Vec<u8>> {
    let want = (p.encoded_size as usize).min(PREFIX_BYTES);
    let mut out = Vec::with_capacity(want);
    for c in g.chunks_of(p) {
        if out.len() >= want {
            break;
        }
        let take = (want - out.len()).min(c.size as usize) as u32;
        out.extend(r.read_raw(c.pointer, take)?);
    }
    if out.len() != want {
        return Err(tags::Error::Invalid("permutation shorter than its chunks"));
    }
    Ok(out)
}

/// What [`index_map`] added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Indexed {
    /// `snd!` tags with at least one permutation indexed.
    pub tags: usize,
    pub permutations: usize,
    /// Tags or permutations that could not be read (bad indices, chunks
    /// outside their file, a missing shared map) and were left out.
    pub skipped: usize,
}

/// Adds every permutation of every named `snd!` tag in `map` to `index`.
/// Fails only if the map has no readable sound gestalt; a bad tag or chunk
/// is skipped and counted.
pub fn index_map<R: Read + Seek>(
    index: &mut SoundIndex,
    map: &mut Map<R>,
    r: &mut dyn ResourceReader,
) -> tags::Result<Indexed> {
    let g = Gestalt::read(map)?;
    let sounds: Vec<(map::TagId, String)> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"snd!"))
        .filter_map(|t| Some((t.id, t.name.clone()?)))
        .collect();
    let mut done = Indexed::default();
    for (id, name) in sounds {
        let Ok(sound) = tags::sound::read_header(map, id).and_then(|h| g.resolve(&h)) else {
            done.skipped += 1;
            continue;
        };
        let mut any = false;
        for p in sound.ranges.iter().flat_map(|r| &r.permutations) {
            match prefix(&g, p, r) {
                Ok(bytes) if !bytes.is_empty() => {
                    index.insert(&name, &bytes);
                    done.permutations += 1;
                    any = true;
                }
                Ok(_) => {}
                Err(_) => done.skipped += 1,
            }
        }
        done.tags += usize::from(any);
    }
    Ok(done)
}

/// The index of every map seen so far and the routes it is matched against.
pub struct PadIndexer {
    index: SoundIndex,
    table: RouteTable,
    shared: [Option<HostFile>; 3],
    done: HashSet<HostFile>,
}

impl PadIndexer {
    /// `shared` locates `mainmenu.map`, `shared.map` and
    /// `single_player_shared.map`, in that order ([`SHARED_MAPS`]).
    pub fn new(config: &PadSpeakerConfig, shared: [Option<HostFile>; 3]) -> Self {
        PadIndexer {
            index: SoundIndex::new(),
            table: RouteTable::new(config),
            shared,
            done: HashSet::new(),
        }
    }

    /// Indexes the map at `file` once (a file seen before adds nothing) and
    /// returns the router over everything indexed so far, or `None` if
    /// nothing was added.
    pub fn add(&mut self, file: &HostFile) -> Result<Option<PadRouter>, String> {
        self.add_map(file, false)
    }

    fn add_map(&mut self, file: &HostFile, cache_slot: bool) -> Result<Option<PadRouter>, String> {
        if file.len == 0 || !self.done.insert(file.clone()) {
            return Ok(None);
        }
        let error = |e: std::io::Error| format!("{}: {e}", file.path.display());
        let mut reader = BufReader::new(File::open(&file.path).map_err(error)?);
        if cache_slot {
            // The game preallocates slots and invalidates old maps by zeroing
            // the header. Try again on the next open, even at the same length.
            const HEADER_LEN: u64 = 0x800;
            reader.seek(SeekFrom::Start(file.offset)).map_err(error)?;
            let mut header = Vec::new();
            (&mut reader)
                .take(file.len.min(HEADER_LEN))
                .read_to_end(&mut header)
                .map_err(error)?;
            if header.iter().all(|&b| b == 0) {
                self.done.remove(file);
                return Ok(None);
            }
        }
        let mut map = Map::from_region(reader, file.offset, file.len)
            .map_err(|e| format!("{}: {e}", file.path.display()))?;
        let mut res = Resources::new(file, &self.shared);
        let n = index_map(&mut self.index, &mut map, &mut res)
            .map_err(|e| format!("{}: {e}", file.path.display()))?;
        Ok((n.permutations > 0).then(|| self.router()))
    }

    /// Indexes the shared maps themselves (those that are there).
    pub fn add_shared(&mut self) -> Option<PadRouter> {
        let mut router = None;
        for f in self.shared.clone().into_iter().flatten() {
            if let Ok(Some(r)) = self.add(&f) {
                router = Some(r);
            }
        }
        router
    }

    pub fn router(&self) -> PadRouter {
        PadRouter::new(&self.index, &self.table)
    }

    pub fn tag_count(&self) -> usize {
        self.index.tag_count()
    }
}

/// Whether an opened file is a map worth indexing.
fn is_map(path: &str) -> bool {
    path.len() > 4 && path.as_bytes()[path.len() - 4..].eq_ignore_ascii_case(b".map")
}

fn is_cache_slot(device: &str, path: &str) -> bool {
    let name = path.trim_start_matches('\\').as_bytes();
    name.len() == 12
        && name[..5].eq_ignore_ascii_case(b"cache")
        && name[5..8].iter().all(u8::is_ascii_digit)
        && name[8..].eq_ignore_ascii_case(b".map")
        && [
            r"\Device\Harddisk0\Partition3",
            r"\Device\Harddisk0\Partition4",
            r"\Device\Harddisk0\Partition5",
        ]
        .iter()
        .any(|partition| device.eq_ignore_ascii_case(partition))
}

/// The indexer thread. Dropping it removes the VFS observer and joins.
pub struct PadIndex {
    vfs: Arc<Mutex<vfs::Vfs>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PadIndex {
    fn drop(&mut self) {
        // Removing the observer drops the last sender, which ends the thread.
        self.vfs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_open_observer(None);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Watches `vfs` for map opens and calls `install` with a new router after
/// each map is indexed (first the shared maps at `shared`, object names as in
/// [`SHARED_MAPS`]). Errors go to stderr once per map.
pub fn spawn(
    vfs: Arc<Mutex<vfs::Vfs>>,
    config: &PadSpeakerConfig,
    shared: [&str; 3],
    mut install: impl FnMut(PadRouter) + Send + 'static,
) -> PadIndex {
    let (tx, rx) = mpsc::channel::<(HostFile, bool)>();
    let shared = {
        let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
        v.set_open_observer(Some(Box::new(move |f: &vfs::OpenedFile| {
            if let (true, Some(h)) = (is_map(&f.path), &f.host) {
                let _ = tx.send((h.clone(), is_cache_slot(&f.device, &f.path)));
            }
        })));
        shared.map(|name| v.host_file(name))
    };
    let mut indexer = PadIndexer::new(config, shared);
    let join = std::thread::Builder::new()
        .name("pad-speaker-index".into())
        .spawn(move || {
            if let Some(r) = indexer.add_shared() {
                install(r);
            }
            for (file, cache_slot) in rx {
                match indexer.add_map(&file, cache_slot) {
                    Ok(Some(r)) => install(r),
                    Ok(None) => {}
                    Err(e) => eprintln!("pad speaker: not indexed: {e}"),
                }
            }
        })
        .expect("spawning the pad speaker indexer");
    PadIndex {
        vfs,
        join: Some(join),
    }
}

/// [`spawn`] on the kernel's VFS, offering each router to `setting`, which
/// installs it in DirectSound while the controller speaker is on. `None`
/// when the kernel has no VFS.
pub fn install(
    k: &kernel::Kernel,
    config: &PadSpeakerConfig,
    setting: Arc<crate::speaker_setting::SpeakerSetting>,
) -> Option<PadIndex> {
    let vfs = k.vfs()?;
    Some(spawn(
        vfs,
        config,
        SHARED_MAPS.map(|(_, name)| name),
        move |router| setting.offer_router(router),
    ))
}

#[cfg(test)]
#[path = "pad_speaker_tests.rs"]
mod tests;
