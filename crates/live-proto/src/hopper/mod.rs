//! Builds and reads Halo 2's `multiplayer_hopper.dat`, the playlist file
//! (docs/live-motd.md §5, docs/live-matchmaking.md §6). The download
//! envelope is the MOTD's:
//!
//! ```text
//! +0x00  16 bytes   header: CRC, version 4, size ([`crate::title_file`])
//! +0x10  u32 BE     inflated size, 0x162cc0
//! +0x14  zlib stream inflating to the image below
//! ```
//!
//! The inflated payload is a raw fixed-size image with no magic, version or
//! checksum:
//!
//! ```text
//! +0x000000  16 x 0x614     hopper descriptors
//! +0x006140  16 x 0x15cb8   per-hopper data blocks: a 0x44-byte unused
//!                           header, the entry count at +0x44, 100 entry
//!                           slots of 0x37c
//! ```
//!
//! Every field is decoded from the game's own readers in [`layout`]
//! ([`Descriptor`], [`Entry`], the validator port [`check_descriptor`]);
//! [`variant`] is the game variant an entry carries, and [`defaults`] the
//! playlists a server offers when nothing was published
//! ([`HopperFile::default_set`]).
//!
//! The field API below ([`Hopper`], [`Record`]) is the older, partial view
//! the editors still use. Two of its fields are not game fields: `name`
//! reads the bytes from `+0x51b`, which are the last byte of the ninth
//! description and the skill ceiling table (writing a name breaks the
//! validator), and `Record::string` reads the start of the entry's game
//! variant. `Record::id` is the map id, `Record::dwords` the weight and the
//! variant's setting 14, `Record::flags` the variant's flags. The record
//! count is the data block's (`+0x44`); the descriptor's `+0x08` is the
//! playlist id ([`Descriptor::playlist_id`]).
//!
//! Writes touch only the fields they name; every other byte is kept, so a
//! file read and rebuilt (or edited) keeps whatever the game expects. The
//! image is little-endian throughout (the big-endian dword is the download
//! envelope's alone).
//!
//! Game data never goes in the repo: tests build synthetic images, the
//! default set is built from numbers in [`defaults`], and real images come
//! from the owner's own console cache through `h2 live hopper import`
//! (docs/live-admin.md).

pub mod defaults;
pub mod layout;
pub mod variant;

pub use layout::{
    Descriptor, Entry, HopperType, Problem, Rules, check_descriptor, set_slot_entry, slot_entry,
};

use std::io::{Read, Write};

/// The file's name in title storage, published under
/// [`crate::HALO_2_TITLE_ID`]. Kept here (not `lib.rs`) so the module stands
/// alone; `lib.rs` may re-export it.
pub const HALO_2_HOPPER_FILE: &str = "multiplayer_hopper.dat";

/// The size the game requires the payload to inflate to (0x162cc0).
pub const INFLATED_SIZE: usize = 0x162cc0;
/// The hopper channel's title-storage download buffer (docs/live-motd.md
/// §2.2): a larger file fails the download and the game keeps its
/// compiled-in playlists.
pub const MAX_FILE: usize = 0x20000;
/// The hopper file class's version in the header; the game wants exactly 4.
pub const VERSION: u32 = 4;
/// Hopper descriptor entries, then per-hopper data blocks.
pub const HOPPERS: usize = 16;
/// A data block holds exactly 100 entry slots.
pub const MAX_RECORDS: usize = 100;

/// Longest [`Hopper::name`]: the 0x85 bytes from +0x51b minus a NUL (not a
/// game field; module notes).
pub const MAX_NAME: usize = 0x84;
/// Longest [`Record::string`]: 0x48 bytes from +0x24c minus a NUL (the start
/// of the game variant; module notes).
pub const MAX_STRING: usize = 0x47;

pub const DESCRIPTOR: usize = 0x614;
pub const BLOCK: usize = 0x15cb8;
pub const RECORD: usize = 0x37c;

const PREFIX: usize = 16;
pub(crate) const BLOCKS_AT: usize = HOPPERS * DESCRIPTOR;
// The older view's descriptor fields ([`layout`] has them all).
const D_TYPE: usize = 0x00;
const D_STATUS: usize = 0x04;
const D_NAME: usize = 0x51b;
const D_NAME_LEN: usize = 0x85;
const D_DWORDS: usize = 0x5a0;
/// The base wait at +0x5a0, the 16 start waits, and the minimum rank at
/// +0x5e4.
pub const DWORDS: usize = 18;
const D_RANGES: usize = 0x5ec;
/// Per-language title and description slots (nine languages, UTF-8,
/// NUL-terminated), written by the compiled-in defaults (`0x001936e0`) from
/// the `matchmaking_hopper_descriptions` strings.
pub const LANGUAGES: usize = 9;
const D_TITLE: usize = 0x0c;
const D_TITLE_LEN: usize = 0x10;
const D_DESC: usize = 0x9c;
const D_DESC_LEN: usize = 0x80;
/// Longest title and description in UTF-8 bytes (the slot minus its NUL).
pub const MAX_TITLE: usize = D_TITLE_LEN - 1;
pub const MAX_DESCRIPTION: usize = D_DESC_LEN - 1;
// Data-block fields.
use layout::B_COUNT;
const B_RECORDS: usize = 0x48;
// Record fields.
const R_ID: usize = 0x04;
const R_STRING: usize = 0x24c;
const R_STRING_LEN: usize = 0x48;
const R_FLAGS: usize = 0x294;
const R_DWORDS: [usize; 2] = [0x248, 0x2a4];
/// The record's game variant: the 0x130 bytes from +0x24c to the end of the
/// slot, which session setup (`0x00192eb0`) copies out whole for the record
/// it picks. Inside it: the name (32 UTF-16 units at +0x04), the game-engine
/// index (+0x44) and the engine flags (+0x48, bit 0 team play;
/// docs/live-gamestate.md).
const R_VARIANT: usize = 0x24c;
/// Where a record slot's game variant starts, and its size: the variant is
/// the whole tail of the slot.
pub const VARIANT_AT: usize = R_VARIANT;
pub const VARIANT_SIZE: usize = 0x130;
const _: () = assert!(VARIANT_AT + VARIANT_SIZE == RECORD);
const V_NAME: usize = 0x04;
const V_ENGINE: usize = 0x44;
const V_FLAGS: usize = 0x48;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HopperError {
    #[error("hopper index {0} is out of range (there are 16)")]
    Hopper(usize),
    #[error("record slot {0} is out of range (there are 100)")]
    Slot(usize),
    #[error("a record slot is 0x37c bytes, got {0}")]
    SlotBytes(usize),
    #[error("hopper type must be 1 to 5, got {0}")]
    Type(u32),
    #[error("hopper status must be 0 (not loaded), 1 (ok) or 2 (error), got {0}")]
    Status(u32),
    #[error("a hopper needs 1 to 100 records, got {0}")]
    Count(u32),
    #[error("the dword array has {0} entries, not 18")]
    Dwords(usize),
    #[error("{0:?} is longer than {1} ASCII characters")]
    TooLong(String, usize),
    #[error("{0:?} is not printable ASCII")]
    NotAscii(String),
    #[error("{0:?} contains a NUL")]
    Nul(String),
    #[error("not a multiplayer_hopper.dat: {0}")]
    Format(&'static str),
    #[error("not a multiplayer_hopper.dat: {0}")]
    Header(#[from] crate::title_file::HeaderError),
}

/// The older field view of one hopper (module notes): the type, status,
/// the data block's record count, the `+0x51b` bytes, the first three type
/// parameters and the dwords from `+0x5a0`. [`Descriptor`] has every
/// field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hopper {
    /// 1 to 5; the playlist UI shows hoppers of types {2, 4, 5}.
    pub hopper_type: u32,
    /// 0 = not loaded, 1 = ok, 2 = error.
    pub status: u32,
    /// The data block's entry count (`+0x44`), 1 to 100.
    pub record_count: u32,
    /// The bytes from `+0x51b` as ASCII. Not a game field: the last byte
    /// of the ninth description, then the skill ceiling table, which the
    /// validator holds to `i <= table[i]`; leave it as read.
    pub name: String,
    /// The first three type parameters (`+0x5ec`, [`Rules`]).
    pub ranges: [u32; 3],
    /// `+0x5a0` (the base wait), the 16 start waits at `+0x5a4`, and the
    /// minimum rank at `+0x5e4`.
    pub dwords: Vec<u32>,
}

/// The older field view of one entry slot (module notes); [`Entry`] has
/// every field. Move whole slots with [`HopperFile::set_slot`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Record {
    /// The map id (`+0x04`); one the game cannot find marks the hopper as
    /// an error (status 2).
    pub id: u32,
    /// The bytes at `+0x24c` as ASCII: the start of the game variant (its
    /// header and name), not a string of its own.
    pub string: String,
    /// The game variant's flags (`+0x294`, variant `+0x48`).
    pub flags: u32,
    /// The weight (`+0x248`) and the variant's setting 14 (`+0x2a4`).
    pub dwords: [u32; 2],
}

/// A hopper's title and description as the playlist list shows them: the
/// English slot on read; every language slot on write (as `motd set` does).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HopperText {
    pub title: String,
    pub description: String,
}

/// What a record's game variant says about itself (read-only: the rest of
/// its 0x130 bytes are settings this codec does not model).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Variant {
    pub name: String,
    /// Game-engine index: 1 CTF, 2 Slayer, 3 Oddball, 4 King of the Hill,
    /// 7 Juggernaut, 8 Territories, 9 Assault.
    pub engine: u32,
    pub team: bool,
}

/// A NUL-terminated UTF-8 field.
fn get_utf8(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// A `multiplayer_hopper.dat` image plus its download envelope. The image
/// is the source of truth; reads and writes go through it so unknown bytes
/// survive every edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopperFile {
    image: Vec<u8>,
}

fn le32(image: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(image[at..at + 4].try_into().expect("4 bytes"))
}

fn put_le32(image: &mut [u8], at: usize, v: u32) {
    image[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// An ASCII, NUL-free field: read to the first NUL (or the end), written
/// zero-filled so the same text always gives the same bytes.
fn get_ascii(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn put_ascii(field: &mut [u8], s: &str, max: usize) -> Result<(), HopperError> {
    if s.contains('\0') {
        return Err(HopperError::Nul(s.into()));
    }
    if !s.is_ascii() {
        return Err(HopperError::NotAscii(s.into()));
    }
    if s.len() > max {
        return Err(HopperError::TooLong(s.into(), max));
    }
    field.fill(0);
    field[..s.len()].copy_from_slice(s.as_bytes());
    Ok(())
}

impl HopperFile {
    /// Adopts an inflated image; it must be exactly
    /// [`INFLATED_SIZE`] bytes (the game checks nothing else).
    pub fn from_image(image: &[u8]) -> Result<Self, HopperError> {
        if image.len() != INFLATED_SIZE {
            return Err(HopperError::Format("the image is not 0x162cc0 bytes"));
        }
        Ok(HopperFile {
            image: image.to_vec(),
        })
    }

    /// A blank image: every field zero. Record counts of 0 are invalid to
    /// the game, so fill hoppers in with [`HopperFile::set_hopper`] before
    /// serving this to a console; it is a starting point for tests and for
    /// an operator with no base image.
    pub fn blank() -> Self {
        HopperFile {
            image: vec![0u8; INFLATED_SIZE],
        }
    }

    /// The exact inflated image.
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// The image for direct surgery. Nothing here is validated; prefer the
    /// setters.
    pub fn image_mut(&mut self) -> &mut [u8] {
        &mut self.image
    }

    /// Reads a download envelope the way the game does (check the header,
    /// then the big-endian size, inflate).
    pub fn parse(file: &[u8]) -> Result<Self, HopperError> {
        crate::title_file::check(file, VERSION, MAX_FILE)?;
        Self::parse_body(file)
    }

    /// [`HopperFile::parse`], also accepting the zero header of files built
    /// before the header was known (migration and imports).
    pub fn parse_legacy(file: &[u8]) -> Result<Self, HopperError> {
        crate::title_file::check_or_legacy(file, VERSION, MAX_FILE)?;
        Self::parse_body(file)
    }

    fn parse_body(file: &[u8]) -> Result<Self, HopperError> {
        let size = file
            .get(PREFIX..PREFIX + 4)
            .ok_or(HopperError::Format("too short"))?;
        if u32::from_be_bytes(size.try_into().expect("4 bytes")) as usize != INFLATED_SIZE {
            return Err(HopperError::Format("inflated size is not 0x162cc0"));
        }
        let mut raw = Vec::with_capacity(INFLATED_SIZE);
        flate2::read::ZlibDecoder::new(&file[PREFIX + 4..])
            .take(INFLATED_SIZE as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| HopperError::Format("bad zlib stream"))?;
        if raw.len() != INFLATED_SIZE {
            return Err(HopperError::Format(
                "payload does not inflate to 0x162cc0 bytes",
            ));
        }
        Ok(HopperFile { image: raw })
    }

    /// The file the game downloads: the header (CRC, version 4, size), the
    /// big-endian inflated size, and the zlib-compressed image.
    /// Deterministic for one image. Check the result against [`MAX_FILE`]
    /// before publishing ([`HopperFile::build_checked`]).
    pub fn build(&self) -> Vec<u8> {
        let mut body = (INFLATED_SIZE as u32).to_be_bytes().to_vec();
        let mut z = flate2::write::ZlibEncoder::new(&mut body, flate2::Compression::best());
        z.write_all(&self.image).expect("writing to a Vec");
        z.finish().expect("writing to a Vec");
        crate::title_file::seal(VERSION, &body)
    }

    /// [`HopperFile::build`], refusing a file larger than the game's buffer.
    pub fn build_checked(&self) -> Result<Vec<u8>, HopperError> {
        let file = self.build();
        if file.len() > MAX_FILE {
            return Err(crate::title_file::HeaderError::TooLarge(file.len(), MAX_FILE).into());
        }
        Ok(file)
    }

    fn descriptor_at(&self, i: usize) -> Result<usize, HopperError> {
        (i < HOPPERS)
            .then_some(i * DESCRIPTOR)
            .ok_or(HopperError::Hopper(i))
    }

    fn block_at(&self, i: usize) -> Result<usize, HopperError> {
        (i < HOPPERS)
            .then_some(BLOCKS_AT + i * BLOCK)
            .ok_or(HopperError::Hopper(i))
    }

    /// Hopper `i`'s editable descriptor fields.
    pub fn hopper(&self, i: usize) -> Result<Hopper, HopperError> {
        let at = self.descriptor_at(i)?;
        let block = self.block_at(i)?;
        let image = &self.image;
        Ok(Hopper {
            hopper_type: le32(image, at + D_TYPE),
            status: le32(image, at + D_STATUS),
            record_count: le32(image, block + B_COUNT),
            name: get_ascii(&image[at + D_NAME..][..D_NAME_LEN]),
            ranges: [
                le32(image, at + D_RANGES),
                le32(image, at + D_RANGES + 4),
                le32(image, at + D_RANGES + 8),
            ],
            dwords: (0..DWORDS)
                .map(|n| le32(image, at + D_DWORDS + n * 4))
                .collect(),
        })
    }

    /// Writes hopper `i`'s fields, with the record count going to the data
    /// block. The playlist id (`+0x08`) is kept; if it is not one the
    /// validator takes (a blank image), it becomes `i + 1`.
    pub fn set_hopper(&mut self, i: usize, h: &Hopper) -> Result<(), HopperError> {
        let at = self.descriptor_at(i)?;
        if !(1..=5).contains(&h.hopper_type) {
            return Err(HopperError::Type(h.hopper_type));
        }
        if h.status > 2 {
            return Err(HopperError::Status(h.status));
        }
        if !(1..=MAX_RECORDS as u32).contains(&h.record_count) {
            return Err(HopperError::Count(h.record_count));
        }
        if h.dwords.len() != DWORDS {
            return Err(HopperError::Dwords(h.dwords.len()));
        }
        // Written only when it changes: the bytes after +0x51b are a table
        // the game validates (`0x00193560`), which a zero-filled rewrite of
        // an unchanged name would clobber.
        if h.name != get_ascii(&self.image[at + D_NAME..][..D_NAME_LEN]) {
            put_ascii(
                &mut self.image[at + D_NAME..][..D_NAME_LEN],
                &h.name,
                MAX_NAME,
            )?;
        }
        put_le32(&mut self.image, at + D_TYPE, h.hopper_type);
        put_le32(&mut self.image, at + D_STATUS, h.status);
        if !layout::PLAYLIST_IDS.contains(&le32(&self.image, at + layout::D_PLAYLIST_ID)) {
            put_le32(&mut self.image, at + layout::D_PLAYLIST_ID, i as u32 + 1);
        }
        let block = self.block_at(i)?;
        put_le32(&mut self.image, block + B_COUNT, h.record_count);
        for n in 0..DWORDS {
            put_le32(&mut self.image, at + D_DWORDS + n * 4, h.dwords[n]);
        }
        for n in 0..3 {
            put_le32(&mut self.image, at + D_RANGES + n * 4, h.ranges[n]);
        }
        Ok(())
    }

    /// The record count in hopper `i`'s data block (the same as
    /// [`Hopper::record_count`] and [`HopperFile::entry_count`]).
    pub fn block_record_count(&self, i: usize) -> Result<u32, HopperError> {
        let at = self.block_at(i)?;
        Ok(le32(&self.image, at + B_COUNT))
    }

    /// Sets the record count (data block `+0x44`).
    pub fn set_record_count(&mut self, i: usize, count: u32) -> Result<(), HopperError> {
        if !(1..=MAX_RECORDS as u32).contains(&count) {
            return Err(HopperError::Count(count));
        }
        let block = self.block_at(i)?;
        put_le32(&mut self.image, block + B_COUNT, count);
        Ok(())
    }

    fn slot_at(&self, i: usize, j: usize) -> Result<usize, HopperError> {
        let _ = self.descriptor_at(i)?;
        let block = self.block_at(i)?;
        (j < MAX_RECORDS)
            .then_some(block + B_RECORDS + j * RECORD)
            .ok_or(HopperError::Slot(j))
    }

    /// The whole 0x37c-byte slot `j` of hopper `i`: every byte of the
    /// record, editable fields and unknown blobs alike. Copies, reorders and
    /// duplicate-inserts move records with this, so nothing is lost.
    pub fn slot(&self, i: usize, j: usize) -> Result<&[u8], HopperError> {
        let at = self.slot_at(i, j)?;
        Ok(&self.image[at..at + RECORD])
    }

    /// Replaces slot `j` of hopper `i` wholesale (as read by
    /// [`HopperFile::slot`], for instance).
    pub fn set_slot(&mut self, i: usize, j: usize, bytes: &[u8]) -> Result<(), HopperError> {
        let at = self.slot_at(i, j)?;
        if bytes.len() != RECORD {
            return Err(HopperError::SlotBytes(bytes.len()));
        }
        self.image[at..at + RECORD].copy_from_slice(bytes);
        Ok(())
    }

    /// Slot `j`'s editable fields.
    pub fn record(&self, i: usize, j: usize) -> Result<Record, HopperError> {
        let at = self.slot_at(i, j)?;
        Ok(slot_record(&self.image[at..at + RECORD]))
    }

    /// Writes slot `j`'s editable fields; the rest of the slot is untouched.
    pub fn set_record(&mut self, i: usize, j: usize, r: &Record) -> Result<(), HopperError> {
        let at = self.slot_at(i, j)?;
        set_slot_record(&mut self.image[at..at + RECORD], r)
    }

    /// Hopper `i`'s title and description (the English slot).
    pub fn text(&self, i: usize) -> Result<HopperText, HopperError> {
        let at = self.descriptor_at(i)?;
        Ok(HopperText {
            title: get_utf8(&self.image[at + D_TITLE..][..D_TITLE_LEN]),
            description: get_utf8(&self.image[at + D_DESC..][..D_DESC_LEN]),
        })
    }

    /// Writes hopper `i`'s title and description into all nine language
    /// slots, zero-filled.
    pub fn set_text(&mut self, i: usize, t: &HopperText) -> Result<(), HopperError> {
        let at = self.descriptor_at(i)?;
        for (s, max) in [(&t.title, MAX_TITLE), (&t.description, MAX_DESCRIPTION)] {
            if s.contains('\0') {
                return Err(HopperError::Nul(s.clone()));
            }
            if s.len() > max {
                return Err(HopperError::TooLong(s.clone(), max));
            }
        }
        for l in 0..LANGUAGES {
            for (base, len, s) in [
                (D_TITLE, D_TITLE_LEN, &t.title),
                (D_DESC, D_DESC_LEN, &t.description),
            ] {
                let field = &mut self.image[at + base + l * len..][..len];
                field.fill(0);
                field[..s.len()].copy_from_slice(s.as_bytes());
            }
        }
        Ok(())
    }

    /// What slot `j`'s game variant says about itself.
    pub fn variant(&self, i: usize, j: usize) -> Result<Variant, HopperError> {
        let at = self.slot_at(i, j)?;
        Ok(slot_variant(&self.image[at..at + RECORD]))
    }

    /// The editable fields of hopper `i`'s first `record_count` records.
    pub fn records(&self, i: usize) -> Result<Vec<Record>, HopperError> {
        let count = self.hopper(i)?.record_count as usize;
        (0..count.min(MAX_RECORDS))
            .map(|j| self.record(i, j))
            .collect()
    }
}

/// A record slot's editable fields, read from the slot's 0x37c bytes alone
/// (as [`HopperFile::record`] reads them in place).
///
/// # Panics
/// If `slot` is shorter than [`RECORD`].
pub fn slot_record(slot: &[u8]) -> Record {
    let slot = &slot[..RECORD];
    Record {
        id: le32(slot, R_ID),
        string: get_ascii(&slot[R_STRING..][..R_STRING_LEN]),
        flags: le32(slot, R_FLAGS),
        dwords: [le32(slot, R_DWORDS[0]), le32(slot, R_DWORDS[1])],
    }
}

/// Writes a record's editable fields into a slot's 0x37c bytes, the way
/// [`HopperFile::set_record`] does in place; the rest is untouched.
pub fn set_slot_record(slot: &mut [u8], r: &Record) -> Result<(), HopperError> {
    if slot.len() != RECORD {
        return Err(HopperError::SlotBytes(slot.len()));
    }
    // Written only when it changes: the field lies inside the record's
    // game variant, and rewriting an unchanged string zero-fills the
    // variant's name and engine index.
    if r.string != get_ascii(&slot[R_STRING..][..R_STRING_LEN]) {
        put_ascii(&mut slot[R_STRING..][..R_STRING_LEN], &r.string, MAX_STRING)?;
    }
    put_le32(slot, R_ID, r.id);
    put_le32(slot, R_FLAGS, r.flags);
    put_le32(slot, R_DWORDS[0], r.dwords[0]);
    put_le32(slot, R_DWORDS[1], r.dwords[1]);
    Ok(())
}

/// What a slot's game variant says about itself, from the slot's bytes.
///
/// # Panics
/// If `slot` is shorter than [`RECORD`].
pub fn slot_variant(slot: &[u8]) -> Variant {
    let v = &slot[VARIANT_AT..RECORD];
    let name: Vec<u16> = v[V_NAME..][..0x40]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0)
        .collect();
    Variant {
        name: String::from_utf16_lossy(&name),
        engine: le32(v, V_ENGINE),
        team: le32(v, V_FLAGS) & 1 != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hopper(name: &str) -> Hopper {
        Hopper {
            hopper_type: 4,
            status: 1,
            record_count: 2,
            name: name.into(),
            ranges: [1, 8, 4],
            // A packed (value << 16) | field_id dword, as the game's
            // defaults use (30 s and field 0x7a4).
            dwords: vec![0x7530_07a4; DWORDS],
        }
    }

    fn record(id: u32, string: &str) -> Record {
        Record {
            id,
            string: string.into(),
            flags: 0x21,
            dwords: [7, 9],
        }
    }

    #[test]
    fn unchanged_name_and_string_keep_the_bytes_under_them() {
        let mut file = HopperFile::blank();
        // The validated table after +0x51b, and a variant whose first byte
        // is zero (so its "string" reads empty) but whose name is not.
        for (k, b) in file.image_mut()[0x51c..0x59c].iter_mut().enumerate() {
            *b = (k as u8 + 5).min(0x7f);
        }
        let slot = BLOCKS_AT + B_RECORDS;
        file.image_mut()[slot + R_VARIANT + V_NAME] = b'S';
        put_le32(file.image_mut(), slot + R_VARIANT + V_ENGINE, 2);
        let before = file.image().to_vec();
        let h = HopperFile::from_image(&before).unwrap().hopper(0).unwrap();
        let mut h = h;
        h.hopper_type = 1;
        h.record_count = 1;
        file.set_hopper(0, &h).unwrap();
        let r = file.record(0, 0).unwrap();
        assert_eq!(r.string, "");
        file.set_record(
            0,
            0,
            &Record {
                dwords: [5, r.dwords[1]],
                ..r
            },
        )
        .unwrap();
        assert_eq!(&file.image()[0x51b..0x5a0], &before[0x51b..0x5a0]);
        assert_eq!(file.variant(0, 0).unwrap().engine, 2);
        assert_eq!(file.variant(0, 0).unwrap().name, "S");
        assert_eq!(file.record(0, 0).unwrap().dwords[0], 5);
    }

    #[test]
    fn the_record_count_is_the_blocks_and_the_playlist_id_is_kept() {
        let mut file = HopperFile::blank();
        file.set_hopper(3, &hopper("x")).unwrap();
        // A blank slot gets a playlist id the validator takes.
        assert_eq!(file.descriptor(3).unwrap().playlist_id, 4);
        assert_eq!(file.block_record_count(3).unwrap(), 2);
        assert_eq!(file.entry_count(3).unwrap(), 2);
        // An existing id survives count changes and rewrites.
        let mut d = file.descriptor(3).unwrap();
        d.playlist_id = 77;
        file.set_descriptor(3, &d).unwrap();
        file.set_record_count(3, 9).unwrap();
        file.set_hopper(
            3,
            &Hopper {
                record_count: 5,
                ..hopper("x")
            },
        )
        .unwrap();
        assert_eq!(file.descriptor(3).unwrap().playlist_id, 77);
        assert_eq!(file.hopper(3).unwrap().record_count, 5);
        assert_eq!(file.records(3).unwrap().len(), 5);
    }

    #[test]
    fn text_goes_to_every_language_slot() {
        let mut file = HopperFile::blank();
        file.image_mut()[DESCRIPTOR + D_TITLE] = 0xee; // hopper 1 untouched
        let t = HopperText {
            title: "Team Snipers".into(),
            description: "Long shots only.".into(),
        };
        file.set_text(0, &t).unwrap();
        assert_eq!(file.text(0).unwrap(), t);
        for l in 0..LANGUAGES {
            let at = D_TITLE + l * D_TITLE_LEN;
            assert_eq!(&file.image()[at..at + 12], b"Team Snipers");
            assert_eq!(file.image()[at + 12], 0);
        }
        assert_eq!(
            D_DESC + LANGUAGES * D_DESC_LEN,
            0x51c,
            "slots end at the table"
        );
        assert_eq!(file.image()[DESCRIPTOR + D_TITLE], 0xee);
        let long = HopperText {
            title: "Sixteen chars!!!".into(),
            description: String::new(),
        };
        assert!(matches!(
            file.set_text(0, &long),
            Err(HopperError::TooLong(_, 15))
        ));
        let nul = HopperText {
            title: "a\0b".into(),
            description: String::new(),
        };
        assert!(matches!(file.set_text(0, &nul), Err(HopperError::Nul(_))));
    }

    #[test]
    fn slot_helpers_match_the_in_place_accessors() {
        let mut file = HopperFile::blank();
        for (k, b) in file.image_mut().iter_mut().enumerate() {
            *b = (k.wrapping_mul(7) % 253) as u8;
        }
        let r = Record {
            id: 33,
            string: file.record(1, 4).unwrap().string,
            flags: 0x1234,
            dwords: [3, 4],
        };
        let mut slot = file.slot(1, 4).unwrap().to_vec();
        set_slot_record(&mut slot, &r).unwrap();
        file.set_record(1, 4, &r).unwrap();
        assert_eq!(file.slot(1, 4).unwrap(), &slot[..]);
        assert_eq!(slot_record(&slot), file.record(1, 4).unwrap());
        assert_eq!(slot_variant(&slot), file.variant(1, 4).unwrap());
        assert_eq!(
            set_slot_record(&mut slot[1..], &r),
            Err(HopperError::SlotBytes(RECORD - 1))
        );
        assert_eq!((VARIANT_AT, VARIANT_SIZE), (0x24c, 0x130));
    }

    #[test]
    fn variant_reads_name_engine_and_team() {
        let mut file = HopperFile::blank();
        let at = BLOCKS_AT + 2 * BLOCK + B_RECORDS + 3 * RECORD + R_VARIANT;
        assert_eq!(R_VARIANT + 0x130, RECORD, "the variant ends the slot");
        for (k, u) in "Team Fight".encode_utf16().enumerate() {
            file.image_mut()[at + V_NAME + 2 * k..][..2].copy_from_slice(&u.to_le_bytes());
        }
        put_le32(file.image_mut(), at + V_ENGINE, 7);
        put_le32(file.image_mut(), at + V_FLAGS, 0x81);
        assert_eq!(
            file.variant(2, 3).unwrap(),
            Variant {
                name: "Team Fight".into(),
                engine: 7,
                team: true
            }
        );
    }

    #[test]
    fn layout_matches_the_game() {
        assert_eq!(BLOCKS_AT, 0x6140);
        assert_eq!(BLOCKS_AT + HOPPERS * BLOCK, INFLATED_SIZE);
        assert_eq!(INFLATED_SIZE, 0x162cc0);
        assert_eq!(B_RECORDS + MAX_RECORDS * RECORD, BLOCK);
        assert_eq!(D_NAME + D_NAME_LEN, 0x5a0, "the name field ends at +0x5a0");
        assert_eq!(D_DWORDS + DWORDS * 4, 0x5e8);
        const { assert!(D_RANGES + 3 * 4 <= DESCRIPTOR, "ranges fit the descriptor") };
        assert_eq!(R_STRING + R_STRING_LEN, R_FLAGS, "the string ends at flags");

        let file = HopperFile::blank().build();
        assert_eq!(&file[4..8], &4u32.to_le_bytes(), "version 4");
        assert_eq!(&file[8..12], &(file.len() as u32).to_le_bytes(), "size");
        assert_eq!(
            &file[0..4],
            &crate::title_file::crc_register(&file[4..]).to_le_bytes()
        );
        assert!(file.len() <= MAX_FILE);
        assert_eq!(&file[16..20], &[0x00, 0x16, 0x2c, 0xc0]);
        assert!(HopperFile::parse(&file).is_ok());
        // A MOTD-version header is not a hopper.
        let mut wrong = crate::title_file::seal(1, &file[16..]);
        assert!(matches!(
            HopperFile::parse(&wrong),
            Err(HopperError::Header(
                crate::title_file::HeaderError::Version { got: 1, want: 4 }
            ))
        ));
        // The zero header of early builds reads only through parse_legacy.
        wrong = file.clone();
        wrong[..16].fill(0);
        assert!(HopperFile::parse(&wrong).is_err());
        assert_eq!(
            HopperFile::parse_legacy(&wrong).unwrap().image(),
            HopperFile::blank().image()
        );
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&file[20..])
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw.len(), INFLATED_SIZE);
    }

    #[test]
    fn round_trips_and_preserves_unknown_bytes() {
        let mut file = HopperFile::blank();
        // Unknown bytes: a descriptor tail, a block header, a slot blob, and
        // a slot past the record count.
        file.image_mut()[DESCRIPTOR - 1] = 0xaa;
        file.image_mut()[BLOCKS_AT + 3] = 0x55;
        file.image_mut()[BLOCKS_AT + B_RECORDS + 0x100] = 0x77;
        file.image_mut()[BLOCKS_AT + 3 * BLOCK + B_RECORDS + 5 * RECORD + 0x2ff] = 0x33;
        file.set_hopper(3, &hopper("Team Slayer")).unwrap();
        file.set_record_count(3, 2).unwrap();
        file.set_record(3, 0, &record(11, "slayer on beaver creek"))
            .unwrap();
        file.set_record(3, 1, &record(12, "slayer on lockout"))
            .unwrap();

        let built = file.build();
        assert_eq!(built, file.build(), "building is deterministic");
        let back = HopperFile::parse(&built).unwrap();
        assert_eq!(
            back.image(),
            file.image(),
            "the image survives the envelope"
        );
        assert_eq!(back.hopper(3).unwrap(), hopper("Team Slayer"));
        assert_eq!(back.block_record_count(3).unwrap(), 2);
        assert_eq!(
            back.records(3).unwrap(),
            vec![
                record(11, "slayer on beaver creek"),
                record(12, "slayer on lockout")
            ]
        );
        let image = back.image();
        assert_eq!(image[DESCRIPTOR - 1], 0xaa);
        assert_eq!(image[BLOCKS_AT + 3], 0x55);
        assert_eq!(image[BLOCKS_AT + B_RECORDS + 0x100], 0x77);
        assert_eq!(
            image[BLOCKS_AT + 3 * BLOCK + B_RECORDS + 5 * RECORD + 0x2ff],
            0x33
        );
        // The 0x200-byte copy at +0x48 of slot 0 is untouched by set_record.
        let copy_at = BLOCKS_AT + 3 * BLOCK + B_RECORDS + 0x48;
        assert_eq!(&image[copy_at..][..4], &[0; 4]);
    }

    #[test]
    fn slots_move_whole_records() {
        let mut file = HopperFile::blank();
        file.set_hopper(0, &hopper("Rumble Pit")).unwrap();
        file.set_record_count(0, 2).unwrap();
        file.set_record(0, 0, &record(1, "first")).unwrap();
        file.set_record(0, 1, &record(2, "second")).unwrap();
        // Give slot 1's unknown blob a marker, then swap the slots whole.
        file.image_mut()[BLOCKS_AT + B_RECORDS + RECORD + 0x50] = 0x99;
        let second = file.slot(0, 1).unwrap().to_vec();
        let first = file.slot(0, 0).unwrap().to_vec();
        file.set_slot(0, 0, &second).unwrap();
        file.set_slot(0, 1, &first).unwrap();
        assert_eq!(file.record(0, 0).unwrap(), record(2, "second"));
        assert_eq!(file.record(0, 1).unwrap(), record(1, "first"));
        // The blob moved with its record.
        assert_eq!(file.image()[BLOCKS_AT + B_RECORDS + 0x50], 0x99);
        assert_eq!(
            file.slot(0, 3).unwrap().len(),
            RECORD,
            "slots past the count read fine"
        );
        let short = vec![0u8; RECORD - 1];
        assert_eq!(
            file.set_slot(0, 0, &short),
            Err(HopperError::SlotBytes(RECORD - 1))
        );
    }

    #[test]
    fn rejects_what_the_game_cannot_show() {
        let mut file = HopperFile::blank();
        let mut try_set = |h: Hopper| file.set_hopper(0, &h).unwrap_err();
        assert_eq!(
            try_set(Hopper {
                hopper_type: 0,
                ..hopper("x")
            }),
            HopperError::Type(0)
        );
        assert_eq!(
            try_set(Hopper {
                hopper_type: 6,
                ..hopper("x")
            }),
            HopperError::Type(6)
        );
        assert_eq!(
            try_set(Hopper {
                status: 3,
                ..hopper("x")
            }),
            HopperError::Status(3)
        );
        assert_eq!(
            try_set(Hopper {
                record_count: 0,
                ..hopper("x")
            }),
            HopperError::Count(0)
        );
        assert_eq!(
            try_set(Hopper {
                record_count: 101,
                ..hopper("x")
            }),
            HopperError::Count(101)
        );
        assert_eq!(
            try_set(Hopper {
                dwords: vec![0; 17],
                ..hopper("x")
            }),
            HopperError::Dwords(17)
        );
        let long_name = "x".repeat(MAX_NAME + 1);
        assert_eq!(
            try_set(Hopper {
                name: long_name.clone(),
                ..hopper("x")
            }),
            HopperError::TooLong(long_name, MAX_NAME)
        );
        assert_eq!(
            try_set(Hopper {
                name: "café".into(),
                ..hopper("x")
            }),
            HopperError::NotAscii("café".into())
        );
        assert_eq!(
            try_set(Hopper {
                name: "a\0b".into(),
                ..hopper("x")
            }),
            HopperError::Nul("a\0b".into())
        );
        assert_eq!(file.set_record_count(1, 101), Err(HopperError::Count(101)));
        let long = Record {
            string: "s".repeat(MAX_STRING + 1),
            ..record(1, "")
        };
        assert!(matches!(
            file.set_record(0, 0, &long),
            Err(HopperError::TooLong(_, MAX_STRING))
        ));
        assert!(file.set_hopper(16, &hopper("x")).is_err());
        assert!(file.record(16, 0).is_err());
        assert!(file.record(0, MAX_RECORDS).is_err());

        assert_eq!(
            HopperFile::from_image(&[0; 16]),
            Err(HopperError::Format("the image is not 0x162cc0 bytes"))
        );
        assert!(HopperFile::parse(b"short").is_err());
        let mut built = HopperFile::blank().build();
        built[19] ^= 1;
        assert!(HopperFile::parse(&built).is_err());
        let mut truncated = HopperFile::blank().build();
        truncated.truncate(40);
        assert!(HopperFile::parse(&truncated).is_err());
    }
}
