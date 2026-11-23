//! Large files open for writing, carried by reference in machine snapshots
//! (docs/runtime/snapshots.md, "vfs and storage"). The game keeps its map
//! cache slots (`cache00N.map`, 180 MB) open for writing all the time it
//! plays, and writes one only while it copies a map in. A snapshot records
//! such a file by its length, a write generation and a content
//! fingerprint ([`FileReference`]) instead of its bytes. From the first
//! capture on, every change the VFS makes to the file saves the bytes it
//! is about to overwrite (copy-before-write), so a restore puts back
//! exactly the captured bytes by undoing the changes made since, newest
//! first. A restore after no change costs a `stat`.
//!
//! The undo log is linear and only grows: a restore's own writes are
//! journaled too, so every captured state stays reachable from any later
//! one. The log is bounded by [`MAX_JOURNAL_BYTES`]; a change that would
//! pass it drops that file's log, and a restore to a state before the drop
//! is accepted only if the file's fingerprint still matches, else refused
//! ([`crate::StateError::Unjournaled`]).
use crate::{Backend, Error};
use std::collections::BTreeMap;

/// The most overwritten bytes the VFS keeps, across every journaled file.
/// A map copy into a cache slot after a capture passes it, and a restore
/// to that capture is refused unless the slot holds the captured map again.
pub const MAX_JOURNAL_BYTES: u64 = 32 << 20;

/// A file open for writing, too large to carry ([`crate::MAX_CARRIED_BYTES`]),
/// as a snapshot records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileReference {
    /// The VFS that captured it ([`crate::Vfs::journal_instance`]); a
    /// restore into another one checks the length and fingerprint.
    pub instance: u64,
    /// The file's write generation in that VFS: a fresh number after every
    /// change, and the captured number again after a restore to it.
    pub generation: u64,
    /// The file's length.
    pub length: u64,
    /// A checksum of the file's bytes (not cryptographic).
    pub fingerprint: u64,
}

/// One change to a journaled file, as the bytes to put back.
#[derive(Debug, Clone)]
struct Undo {
    /// The file's length before the change.
    old_len: u64,
    /// Where `old` sat: the bytes in `[offset, offset + old.len())` before
    /// the change, never past `old_len`.
    offset: u64,
    old: Vec<u8>,
}

/// A change about to be made to a file.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    /// `len` bytes written at `at`.
    Write { at: u64, len: u64 },
    /// The file's length set (or the file truncated, overwritten, removed:
    /// `SetLen(0)`).
    SetLen(u64),
}

#[derive(Debug)]
struct Tracked {
    mount: usize,
    /// The path's lowercase key, as file objects hold it.
    key: String,
    path: String,
    generation: u64,
    /// The fingerprint of the bytes at a generation, taken at its first
    /// capture.
    fingerprint: Option<(u64, u64)>,
    /// Entry number of `entries[0]`: earlier ones were dropped.
    base: u64,
    entries: Vec<Undo>,
    /// Captured generation to the entry number from which undoing every
    /// later entry, newest first, gives the file's bytes at it.
    marks: BTreeMap<u64, u64>,
}

impl Tracked {
    fn end(&self) -> u64 {
        self.base + self.entries.len() as u64
    }
    fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.old.len() as u64).sum()
    }
}

/// What a restore does for one referenced file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Undo the journal back to the captured generation: nothing to do
    /// when the file is at it already.
    Rewind(u64),
    /// The fingerprint matched: the file holds the captured bytes. In the
    /// capturing VFS it takes the captured generation again, if nothing
    /// changed it after the check (the generation it had then; `None`: it
    /// was not journaled).
    Verified {
        generation: u64,
        fingerprint: u64,
        seen: Option<u64>,
    },
    /// Captured by another VFS and verified by fingerprint.
    Foreign,
}

/// Every journaled file of one VFS.
#[derive(Debug)]
pub(crate) struct Journals {
    instance: u64,
    next_generation: u64,
    files: Vec<Tracked>,
    bytes: u64,
}

impl Default for Journals {
    fn default() -> Self {
        Self {
            instance: instance_id(),
            next_generation: 1,
            files: Vec::new(),
            bytes: 0,
        }
    }
}

/// A number no other VFS (in this process or another) is likely to have.
fn instance_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let count = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    mix(nanos ^ (u64::from(std::process::id()) << 40) ^ count.rotate_left(20)) | 1
}

impl Journals {
    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }

    /// The overwritten bytes held, across every file.
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    fn fresh(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    fn find(&self, mount: usize, key: &str) -> Option<usize> {
        self.files
            .iter()
            .position(|t| t.mount == mount && t.key == key)
    }

    /// The reference for the file at `path` (`key` its lowercase form),
    /// `length` bytes long: from here on its changes are journaled. Reads
    /// the file only for the first capture of each generation, to
    /// fingerprint it.
    pub(crate) fn capture(
        &mut self,
        backend: &mut dyn Backend,
        mount: usize,
        key: &str,
        path: &str,
        length: u64,
    ) -> Result<FileReference, Error> {
        let index = match self.find(mount, key) {
            Some(index) => index,
            None => {
                let generation = self.fresh();
                self.files.push(Tracked {
                    mount,
                    key: key.to_owned(),
                    path: path.to_owned(),
                    generation,
                    fingerprint: None,
                    base: 0,
                    entries: Vec::new(),
                    marks: BTreeMap::new(),
                });
                self.files.len() - 1
            }
        };
        let file = &mut self.files[index];
        let fingerprint = match file.fingerprint {
            Some((generation, hash)) if generation == file.generation => hash,
            _ => {
                let hash = fingerprint(backend, path, length)?;
                file.fingerprint = Some((file.generation, hash));
                hash
            }
        };
        let end = file.end();
        file.marks.insert(file.generation, end);
        Ok(FileReference {
            instance: self.instance,
            generation: file.generation,
            length,
            fingerprint,
        })
    }

    /// Called before every change the VFS makes to a file. A journaled
    /// file takes a fresh generation and, while a capture can still rewind
    /// to before the change, saves the bytes the change overwrites.
    pub(crate) fn before(
        &mut self,
        backend: &mut dyn Backend,
        mount: usize,
        key: &str,
        change: Change,
    ) {
        if self.files.is_empty() {
            return;
        }
        let Some(index) = self.find(mount, key) else {
            return;
        };
        let generation = self.fresh();
        self.files[index].generation = generation;
        if self.files[index].marks.is_empty() {
            return;
        }
        let path = self.files[index].path.clone();
        let old_len = match backend.stat(&path) {
            Ok(info) => info.end_of_file,
            // Gone (removed since): there is nothing to overwrite.
            Err(Error::ObjectNameNotFound) => 0,
            Err(_) => {
                self.drop_journal(index);
                return;
            }
        };
        let (from, to) = match change {
            Change::Write { at, len } => (at.min(old_len), at.saturating_add(len).min(old_len)),
            Change::SetLen(len) => (len.min(old_len), old_len),
        };
        let need = to - from;
        if self.bytes + need > MAX_JOURNAL_BYTES {
            self.drop_journal(index);
            return;
        }
        let mut old = vec![0; need as usize];
        if read_exact(backend, &path, from, &mut old).is_err() {
            self.drop_journal(index);
            return;
        }
        self.bytes += need;
        self.files[index].entries.push(Undo {
            old_len,
            offset: from,
            old,
        });
    }

    /// Forget the file's log: no capture before now can rewind through it.
    fn drop_journal(&mut self, index: usize) {
        let file = &mut self.files[index];
        self.bytes -= file.bytes();
        file.base = file.end();
        file.entries = Vec::new();
        file.marks.clear();
    }

    /// Forget every journaled file on a mount (formatted or seeded).
    pub(crate) fn forget_mount(&mut self, mount: usize) {
        self.files.retain(|t| t.mount != mount);
        self.bytes = self.files.iter().map(Tracked::bytes).sum();
    }

    /// Forget one file (written back whole from a snapshot that carried it).
    pub(crate) fn forget(&mut self, mount: usize, key: &str) {
        if let Some(index) = self.find(mount, key) {
            let file = self.files.remove(index);
            self.bytes -= file.bytes();
        }
    }

    /// How a restore puts back the file `reference` captured, which now
    /// has `length` bytes (`None`: it is gone), or `None` when it cannot.
    pub(crate) fn plan(
        &self,
        backend: &mut dyn Backend,
        mount: usize,
        key: &str,
        path: &str,
        reference: &FileReference,
        length: Option<u64>,
    ) -> Option<Plan> {
        let here = reference.instance == self.instance;
        let file = self.find(mount, key).map(|index| &self.files[index]);
        if here && let Some(file) = file {
            // At the captured generation with another length, the file was
            // changed outside the VFS: only the fingerprint can tell.
            let at_capture = file.generation == reference.generation;
            if (at_capture && length == Some(reference.length))
                || (!at_capture && file.marks.contains_key(&reference.generation))
            {
                return Some(Plan::Rewind(reference.generation));
            }
        }
        if length != Some(reference.length)
            || fingerprint(backend, path, reference.length).ok() != Some(reference.fingerprint)
        {
            return None;
        }
        Some(if here {
            Plan::Verified {
                generation: reference.generation,
                fingerprint: reference.fingerprint,
                seen: file.map(|f| f.generation),
            }
        } else {
            Plan::Foreign
        })
    }

    /// Carry out a [`Plan`] for the file at `path`, after the restore closed
    /// the open objects (a close may have changed it since the plan).
    /// Rewinding undoes the journal newest first, journaling the undo's own changes, so states
    /// captured after the target stay reachable. A host write that fails
    /// is logged.
    pub(crate) fn apply(
        &mut self,
        backend: &mut dyn Backend,
        mount: usize,
        key: &str,
        path: &str,
        plan: Plan,
    ) {
        match plan {
            Plan::Foreign => {}
            Plan::Verified {
                generation,
                fingerprint,
                seen,
            } => {
                let current = self.find(mount, key);
                if current.map(|index| self.files[index].generation) != seen {
                    return;
                }
                let index = match current {
                    Some(index) => index,
                    None => {
                        self.files.push(Tracked {
                            mount,
                            key: key.to_owned(),
                            path: path.to_owned(),
                            generation,
                            fingerprint: None,
                            base: 0,
                            entries: Vec::new(),
                            marks: BTreeMap::new(),
                        });
                        self.files.len() - 1
                    }
                };
                let file = &mut self.files[index];
                file.generation = generation;
                file.fingerprint = Some((generation, fingerprint));
                let end = file.end();
                file.marks.insert(generation, end);
            }
            Plan::Rewind(generation) => {
                let Some(index) = self.find(mount, key) else {
                    return;
                };
                let file = &self.files[index];
                if file.generation == generation {
                    return;
                }
                let Some(&from) = file.marks.get(&generation) else {
                    return;
                };
                let start = (from - file.base) as usize;
                let undo: Vec<Undo> = file.entries[start..].iter().rev().cloned().collect();
                let path = file.path.clone();
                for u in undo {
                    if let Err(error) = self.undo(backend, mount, key, &path, &u) {
                        eprintln!("vfs: snapshot load: {path} not rewound: {error}");
                        return;
                    }
                }
                let Some(index) = self.find(mount, key) else {
                    return;
                };
                let file = &mut self.files[index];
                file.generation = generation;
                let end = file.end();
                file.marks.insert(generation, end);
            }
        }
    }

    fn undo(
        &mut self,
        backend: &mut dyn Backend,
        mount: usize,
        key: &str,
        path: &str,
        u: &Undo,
    ) -> Result<(), Error> {
        match backend.stat(path) {
            Ok(_) => {}
            Err(Error::ObjectNameNotFound) => backend.create(path, false, 0)?,
            Err(error) => return Err(error),
        }
        self.before(backend, mount, key, Change::SetLen(u.old_len));
        backend.set_len(path, u.old_len)?;
        if u.old.is_empty() {
            return Ok(());
        }
        self.before(
            backend,
            mount,
            key,
            Change::Write {
                at: u.offset,
                len: u.old.len() as u64,
            },
        );
        let mut at = 0;
        while at < u.old.len() {
            match backend.write_at(path, u.offset + at as u64, &u.old[at..])? {
                0 => return Err(Error::DiskFull),
                n => at += n,
            }
        }
        Ok(())
    }
}

fn read_exact(
    backend: &mut dyn Backend,
    path: &str,
    offset: u64,
    buf: &mut [u8],
) -> Result<(), Error> {
    let mut at = 0;
    while at < buf.len() {
        match backend.read_at(path, offset + at as u64, &mut buf[at..])? {
            0 => return Err(Error::EndOfFile),
            n => at += n,
        }
    }
    Ok(())
}

const CHUNK: u64 = 1 << 20;
const K: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

/// One chunk's hash, seeded with its number.
fn hash_chunk(seed: u64, bytes: &[u8]) -> u64 {
    let mut lanes = [
        mix(seed ^ K),
        mix(seed.wrapping_add(1) ^ K),
        mix(seed.wrapping_add(2) ^ K),
        mix(seed.wrapping_add(3) ^ K),
    ];
    let mut step = |block: &[u8]| {
        for (lane, word) in lanes.iter_mut().zip(block.chunks_exact(8)) {
            let word = u64::from_le_bytes(word.try_into().expect("8 bytes"));
            *lane = (*lane ^ word).wrapping_mul(K).rotate_left(31);
        }
    };
    let mut blocks = bytes.chunks_exact(32);
    for block in &mut blocks {
        step(block);
    }
    let rest = blocks.remainder();
    let mut tail = [0; 32];
    tail[..rest.len()].copy_from_slice(rest);
    step(&tail[..]);
    lanes.iter().fold(mix(bytes.len() as u64), |h, &lane| {
        mix(h ^ lane).wrapping_mul(K)
    })
}

/// A checksum of the first `len` bytes of the file at `path`, to tell
/// whether a file holds the bytes it held at a capture. Not cryptographic:
/// it guards against stale files, not chosen ones.
pub(crate) fn fingerprint(backend: &mut dyn Backend, path: &str, len: u64) -> Result<u64, Error> {
    let mut buf = vec![0; CHUNK.min(len) as usize];
    let mut hash = mix(len ^ K);
    let mut at = 0;
    let mut chunk = 0u64;
    while at < len {
        let n = (len - at).min(CHUNK) as usize;
        read_exact(backend, path, at, &mut buf[..n])?;
        hash = mix(hash ^ hash_chunk(chunk, &buf[..n]));
        at += n as u64;
        chunk += 1;
    }
    Ok(hash)
}
