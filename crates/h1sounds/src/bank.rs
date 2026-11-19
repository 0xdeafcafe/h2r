//! Sound sample chunks shared through an augmented
//! `single_player_shared.map` ("the bank").
//!
//! Halo 2's engine streams a sound's chunks from whichever cache file its
//! pointer names: the top two bits of a chunk's raw pointer select the
//! file (`map::format::ResourceFile`), and every campaign level's map is
//! loaded with `shared`, `single_player_shared` and `mainmenu` already
//! copied into cache slots 0-2 (docs/runtime/cache-slots.md). Retail
//! campaign levels keep their longest sounds in `single_player_shared.map`
//! for exactly this reason, and the audit reads such chunks from the other
//! file (`audit`'s `Data::Elsewhere`).
//!
//! A converted Halo 1 level's looping sounds (its music and ambience) are
//! hours of Xbox ADPCM that never fit the level map's own file-size bound
//! (`0x11800000`, docs/halo1/campaign.md "Music, ambience and the map
//! file"), so they used to be dropped. Instead, a conversion with a
//! [`SoundBank`] stores every looping sound's chunks there:
//!
//! - the bank appends each chunk's bytes past the end of a retail
//!   `single_player_shared.map` and hands back a stored pointer with the
//!   `single_player_shared` file bits set, which the level's sound gestalt
//!   keeps as stored (`gestalt::ChunkPointer::Stored`);
//! - [`SoundBank::finish`] writes the whole file: the retail bytes as
//!   they are (so its own map and every retail level that reads it is
//!   unchanged), then the bank's region, with the copy-time checksum at
//!   `+0x2d0` recomputed over the whole file — the copy machine compares
//!   that field with the XOR of everything it copies, physical EOF
//!   included (docs/formats/map-format.md "Checksum evidence");
//! - the header's declared file length (`+0x08`) is left at retail's, so
//!   the loader's per-type size check (`0x122870`, limit `0x20800000`)
//!   still passes; the appended bytes are raw sample data that nothing
//!   but a chunk pointer names;
//! - a chunk's offset stays under 2^30, the addressing limit of the
//!   pointer's low 30 bits, which bounds the file: retail
//!   `single_player_shared.map` is `0x1995_a000` (409 MiB) and every
//!   looping sound of the ten campaign levels is about 455 MiB more
//!   (docs/halo1/sounds.md "The sound bank"), together under 2^30.
//!
//! Chunks are addressed by content (SHA-256), so two levels that play the
//! same loop share one copy, and a conversion that runs again asks for
//! the same bytes and gets the same pointer. The order chunks were first
//! asked for decides their offsets; an index file written beside the map
//! ([`Index`]) records each stored chunk, so a later single-level
//! conversion can grow an existing bank without moving anything
//! ([`SoundBank::load`]).
//!
//! The import builds the bank from nothing each run (it converts every
//! level anyway) and writes `<data>/campaigns/halo1/single_player_shared.map`
//! plus its index; the boot serves that file as `d:\maps\single_player_shared.map`
//! in place of the disc's (docs/halo1/sounds.md "The sound bank").

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// The stored-pointer bits that name `single_player_shared.map`
/// (`ResourceFile::SinglePlayerShared`, selector 3).
pub const FILE_BITS: u32 = 0xC000_0000;
/// The bank's file on the data partition, served over the disc's own.
pub const FILE_NAME: &str = "single_player_shared.map";
/// The index file written beside the bank's map.
pub const INDEX_NAME: &str = "single_player_shared.soundbank.index";
/// Every chunk aligns to this many bytes inside the region (Design).
const ALIGN: u32 = 4;
/// A chunk's offset must stay below the pointer's low 30 bits.
const MAX_OFFSET: u32 = 0x4000_0000;

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256.digest(bytes).into()
}

/// One chunk stored in a previous bank file: its hash, offset and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub hash: [u8; 32],
    pub offset: u32,
    pub len: u32,
}

/// The chunks a bank file holds, as written by [`SoundBank::finish`]:
/// a header line, then one `chunk` line each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// The bank region's first file offset.
    pub base: u32,
    pub chunks: Vec<Stored>,
}

impl Index {
    /// Parses the index text [`Self::to_text`] writes; a first line that
    /// isn't this bank's says so.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || Error::Invalid("not a sound bank index".into());
        let mut lines = text.lines();
        let first = lines.next().ok_or_else(bad)?;
        if first != "h2recomp sound bank v1" {
            return Err(bad());
        }
        let mut base = None;
        let mut chunks = Vec::new();
        for line in lines {
            let mut fields = line.split(' ');
            match fields.next() {
                Some("base") => {
                    let v = fields.next().ok_or_else(bad)?;
                    base = Some(u32::from_str_radix(v, 16).map_err(|_| bad())?);
                }
                Some("chunk") => {
                    let h = fields.next().ok_or_else(bad)?;
                    let offset = fields.next().ok_or_else(bad)?;
                    let len = fields.next().ok_or_else(bad)?;
                    let mut stored = Stored {
                        hash: [0; 32],
                        offset: u32::from_str_radix(offset, 16).map_err(|_| bad())?,
                        len: u32::from_str_radix(len, 16).map_err(|_| bad())?,
                    };
                    let bytes = h.as_bytes();
                    if bytes.len() != 64 {
                        return Err(bad());
                    }
                    for (k, slot) in stored.hash.iter_mut().enumerate() {
                        *slot = u8::from_str_radix(
                            std::str::from_utf8(&bytes[k * 2..k * 2 + 2]).map_err(|_| bad())?,
                            16,
                        )
                        .map_err(|_| bad())?;
                    }
                    chunks.push(stored);
                }
                _ => return Err(bad()),
            }
        }
        Ok(Index {
            base: base.ok_or_else(bad)?,
            chunks,
        })
    }

    /// The index as text, one line per chunk.
    pub fn to_text(&self) -> String {
        let mut out = String::from("h2recomp sound bank v1\n");
        out.push_str(&format!("base {:x}\n", self.base));
        for c in &self.chunks {
            out.push_str(&format!(
                "chunk {} {:x} {:x}\n",
                c.hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                c.offset,
                c.len
            ));
        }
        out
    }
}

#[derive(Debug)]
struct BankState {
    /// The next free file offset in the region.
    next: u32,
    /// Chunk hash -> stored pointer (file bits included).
    pointers: HashMap<[u8; 32], u32>,
    /// Chunks added to this bank this run, in order.
    added: Vec<(u32, Vec<u8>)>,
    /// Chunks a loaded bank file already held: offset and bytes.
    loaded: Vec<(u32, Vec<u8>)>,
}

/// Sample chunks addressed by content and stored past a retail
/// `single_player_shared.map`. Shared by every conversion that must agree
/// on one file: clone the `Arc` the caller owns, not this.
#[derive(Debug)]
pub struct SoundBank {
    /// The retail file's length: the region starts past it, aligned.
    retail_len: u32,
    state: Mutex<BankState>,
}

impl SoundBank {
    /// An empty bank whose region starts at `retail_len` rounded up.
    pub fn new(retail_len: u32) -> Self {
        let base = retail_len.next_multiple_of(0x200);
        SoundBank {
            retail_len,
            state: Mutex::new(BankState {
                next: base,
                pointers: HashMap::new(),
                added: Vec::new(),
                loaded: Vec::new(),
            }),
        }
    }

    /// The bank a previous conversion wrote, when `file` still begins with
    /// exactly this retail file's bytes: grown in place, so the maps
    /// already written keep their pointers. Anything else (a shorter or
    /// different prefix) is [`Error::Invalid`]: the caller starts a new
    /// bank and says so.
    pub fn load_matching(retail: &[u8], file: &[u8], index: &Index) -> Result<Self> {
        if file.get(..retail.len()) != Some(retail) {
            return Err(Error::Invalid(
                "the bank's file does not begin with this retail single_player_shared.map".into(),
            ));
        }
        Self::load(retail.len() as u32, file, index)
    }

    /// The bank a previous conversion wrote: `file` is the whole augmented
    /// map and `index` what [`SoundBank::finish`] recorded beside it. Every
    /// chunk's bytes are hashed against its entry; the retail prefix must
    /// be `retail_len` long (the caller checks it is the same retail file).
    pub fn load(retail_len: u32, file: &[u8], index: &Index) -> Result<Self> {
        if file.len() < retail_len as usize {
            return Err(Error::Invalid(format!(
                "the bank's file is {} bytes, shorter than the retail {}",
                file.len(),
                retail_len
            )));
        }
        let bank = Self::new(retail_len);
        {
            let mut s = bank.state.lock().unwrap_or_else(|e| e.into_inner());
            for c in &index.chunks {
                let span = file
                    .get(c.offset as usize..(c.offset + c.len) as usize)
                    .ok_or_else(|| {
                        Error::Invalid(format!(
                            "the bank's chunk at {:#x}+{:#x} is outside the file",
                            c.offset, c.len
                        ))
                    })?;
                if hash(span) != c.hash {
                    return Err(Error::Invalid(format!(
                        "the bank's chunk at {:#x}+{:#x} is not the bytes its index names",
                        c.offset, c.len
                    )));
                }
                if c.offset + c.len > s.next {
                    s.next = c.offset + c.len;
                }
                s.pointers.insert(c.hash, FILE_BITS | c.offset);
                s.loaded.push((c.offset, span.to_vec()));
            }
        }
        Ok(bank)
    }

    /// The stored pointer for `bytes`, adding them to the bank when no
    /// earlier chunk has the same content. `Error::Unsupported` when the
    /// region would reach 2^30 bytes, the pointer's addressing limit: the
    /// caller fails loudly rather than dropping the sound.
    pub fn pointer(&self, bytes: &[u8]) -> Result<u32> {
        let h = hash(bytes);
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&p) = s.pointers.get(&h) {
            return Ok(p);
        }
        let offset = s.next.next_multiple_of(ALIGN);
        let end = offset + bytes.len() as u32;
        if end > MAX_OFFSET {
            return Err(Error::Unsupported(format!(
                "the sound bank would reach {end:#x} bytes, over the 2^30 a chunk pointer addresses"
            )));
        }
        s.next = end;
        s.pointers.insert(h, FILE_BITS | offset);
        s.added.push((offset, bytes.to_vec()));
        Ok(FILE_BITS | offset)
    }

    /// How many chunks the bank holds.
    pub fn chunks(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pointers
            .len()
    }

    /// The region's size in bytes: what this bank adds past the retail
    /// file.
    pub fn region_bytes(&self) -> u64 {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        u64::from(s.next - self.retail_len.next_multiple_of(0x200))
    }

    /// The bank as `(map file bytes, index)`: the retail bytes as they
    /// are, the region after them, the checksum at `+0x2d0` recomputed
    /// over the whole file, and every stored chunk in the index so a
    /// later conversion can grow this bank.
    pub fn finish(&self, retail: &[u8]) -> Result<(Vec<u8>, Index)> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if retail.len() != self.retail_len as usize {
            return Err(Error::Invalid(format!(
                "the retail file is {} bytes, the bank was started from {}",
                retail.len(),
                self.retail_len
            )));
        }
        let base = self.retail_len.next_multiple_of(0x200);
        let mut out = retail.to_vec();
        out.resize(s.next as usize, 0);
        let mut index = Index {
            base,
            chunks: Vec::with_capacity(s.pointers.len()),
        };
        for (offset, bytes) in s.loaded.iter().chain(s.added.iter()) {
            let at = *offset as usize;
            out[at..at + bytes.len()].copy_from_slice(bytes);
            index.chunks.push(Stored {
                hash: hash(bytes),
                offset: *offset,
                len: bytes.len() as u32,
            });
        }
        // The copy machine checks the checksum at +0x2d0 against the XOR of
        // everything it copies, physical EOF included: recompute it over
        // the whole file, header excluded.
        let checksum = map::checksum::engine_xor(&out[0x800.min(out.len())..]);
        out[0x2d0..0x2d4].copy_from_slice(&checksum.to_le_bytes());
        index.chunks.sort_by_key(|c| c.offset);
        Ok((out, index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A retail file with its own checksum field already set.
    fn retail(len: usize) -> Vec<u8> {
        let mut b = vec![0x5a; len];
        let checksum = map::checksum::engine_xor(&b[0x800..]);
        b[0x2d0..0x2d4].copy_from_slice(&checksum.to_le_bytes());
        b
    }

    #[test]
    fn the_same_bytes_get_the_same_pointer_and_are_stored_once() {
        let bank = SoundBank::new(0x1000);
        let a = bank.pointer(b"one").unwrap();
        assert_eq!(a & FILE_BITS, FILE_BITS);
        assert_eq!(bank.pointer(b"one").unwrap(), a, "content addressed");
        let b = bank.pointer(b"two").unwrap();
        assert_ne!(a, b);
        assert_eq!(bank.chunks(), 2);
    }

    #[test]
    fn the_first_chunk_starts_past_the_retail_file_aligned() {
        let bank = SoundBank::new(0x1101);
        let p = bank.pointer(&[7; 0x20]).unwrap();
        assert_eq!(p, FILE_BITS | 0x1200);
    }

    #[test]
    fn finish_keeps_the_retail_bytes_and_sets_the_checksum() {
        let r = retail(0x1000);
        let bank = SoundBank::new(r.len() as u32);
        let p = bank.pointer(&[1, 2, 3]).unwrap();
        let (file, index) = bank.finish(&r).unwrap();
        // The retail bytes stand, the chunk sits at its pointer's offset.
        assert_eq!(&file[..0x1000], &r[..]);
        let at = (p & !FILE_BITS) as usize;
        assert_eq!(&file[at..at + 3], &[1, 2, 3]);
        // The copy-time checksum covers the whole physical file.
        assert_eq!(
            map::checksum::engine_xor(&file[0x800..]),
            u32::from_le_bytes(file[0x2d0..0x2d4].try_into().unwrap())
        );
        assert_eq!(index.chunks.len(), 1);
        assert_eq!(index.chunks[0].offset, p & !FILE_BITS);
        assert_eq!(index.chunks[0].len, 3);
    }

    #[test]
    fn an_index_round_trips() {
        let i = Index {
            base: 0x1200,
            chunks: vec![Stored {
                hash: [0xab; 32],
                offset: 0x1200,
                len: 0x7ff8,
            }],
        };
        assert_eq!(Index::parse(&i.to_text()).unwrap(), i);
        assert!(Index::parse("something else\n").is_err());
        assert!(Index::parse("h2recomp sound bank v1\n").is_err(), "no base");
    }

    #[test]
    fn a_loaded_bank_keeps_its_chunks_and_grows_past_them() {
        let r = retail(0x1000);
        let first = SoundBank::new(r.len() as u32);
        let p = first.pointer(&[9; 0x100]).unwrap();
        let (file, index) = first.finish(&r).unwrap();
        // Load it back: the same bytes keep their pointer, new ones append.
        let bank = SoundBank::load(r.len() as u32, &file, &index).unwrap();
        assert_eq!(bank.pointer(&[9; 0x100]).unwrap(), p);
        let q = bank.pointer(&[3; 4]).unwrap();
        assert_eq!(q & !FILE_BITS, (p & !FILE_BITS) + 0x100);
        let (again, _) = bank.finish(&r).unwrap();
        assert_eq!(again[..file.len()], file[..], "nothing moved");
    }

    #[test]
    fn a_loaded_bank_refuses_bytes_its_index_misnames() {
        let r = retail(0x1000);
        let bank = SoundBank::new(r.len() as u32);
        let (_, mut index) = bank.finish(&r).unwrap();
        // An index naming bytes the file doesn't hold.
        index.chunks.push(Stored {
            hash: [0; 32],
            offset: 0x1100,
            len: 0x10,
        });
        let (file, _) = bank.finish(&r).unwrap();
        assert!(SoundBank::load(r.len() as u32, &file, &index).is_err());
    }

    #[test]
    fn a_bank_never_points_past_2_pow_30() {
        let bank = SoundBank::new(0x3fff_f000);
        assert!(bank.pointer(&[0; 0x100]).is_ok());
        assert!(bank.pointer(&[0; 0x1000]).is_err());
    }

    #[test]
    fn finish_refuses_a_different_retail_length() {
        let bank = SoundBank::new(0x1000);
        assert!(bank.finish(&retail(0x2000)).is_err());
    }
}
