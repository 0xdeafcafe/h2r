//! Builds and reads Halo 2's `multiplayer_hopper.dat`, the playlist file
//! (docs/live-motd.md §5). The download envelope is the MOTD's:
//!
//! ```text
//! +0x00  16 bytes   header (zeros; the game skips it)
//! +0x10  u32 BE     inflated size, 0x162cc0
//! +0x14  zlib stream inflating to the image below
//! ```
//!
//! The inflated payload is a raw fixed-size image with no magic, version or
//! checksum:
//!
//! ```text
//! +0x000000  16 x 0x614     hopper descriptor entries
//! +0x006140  16 x 0x15cb8   per-hopper data blocks
//! ```
//!
//! A descriptor entry (0x614 bytes): `+0x00` type (1..5), `+0x04` status
//! (0 = not loaded, 1 = ok, 2 = error), `+0x08` record count (1..100,
//! validator `0x00193560`), `+0x51b` the ASCII name, `+0x5a0` a dword array,
//! `+0x5e4` a dword, `+0x5ec/+0x5f0/+0x5f4` range fields (the party size
//! limits the session setup reads). A data block (0x15cb8 bytes): a 0x44-byte
//! header, a dword record count at `+0x44`, then 100 record slots of 0x37c.
//! A record: `+0x04` id (must resolve to a `matchmaking_hopper_descriptions`
//! tag record), `+0x24c` an ASCII string, `+0x294` flags, and single dwords
//! at `+0x248` and `+0x2a4`.
//!
//! Only those fields are modelled; everything else in the image is unknown
//! and kept byte for byte, so reading a file and rebuilding it (or editing
//! the known fields) never disturbs what the game expects and we cannot
//! read. The image is little-endian throughout (the big-endian dword is the
//! download envelope's alone).
//!
//! Game data never goes in the repo: tests build synthetic images, and real
//! base images come from the owner's own console cache through
//! `h2 live hopper import` (docs/live-admin.md).

use std::io::{Read, Write};

/// The file's name in title storage, published under
/// [`crate::HALO_2_TITLE_ID`]. Kept here (not `lib.rs`) so the module stands
/// alone; `lib.rs` may re-export it.
pub const HALO_2_HOPPER_FILE: &str = "multiplayer_hopper.dat";

/// The size the game requires the payload to inflate to (0x162cc0).
pub const INFLATED_SIZE: usize = 0x162cc0;
/// Hopper descriptor entries, then per-hopper data blocks.
pub const HOPPERS: usize = 16;
/// The game's record-count validator allows 1..=100, and the block holds
/// exactly 100 slots.
pub const MAX_RECORDS: usize = 100;

/// Longest hopper name: the 0x85-byte field at +0x51b minus its NUL.
pub const MAX_NAME: usize = 0x84;
/// Longest record string: the 0x48-byte field at +0x24c minus its NUL.
pub const MAX_STRING: usize = 0x47;

pub const DESCRIPTOR: usize = 0x614;
pub const BLOCK: usize = 0x15cb8;
pub const RECORD: usize = 0x37c;

const PREFIX: usize = 16;
const BLOCKS_AT: usize = HOPPERS * DESCRIPTOR;
// Descriptor fields (docs/live-motd.md §5.2).
const D_TYPE: usize = 0x00;
const D_STATUS: usize = 0x04;
const D_COUNT: usize = 0x08;
const D_NAME: usize = 0x51b;
const D_NAME_LEN: usize = 0x85;
const D_DWORDS: usize = 0x5a0;
/// 17 dwords at +0x5a0 plus the one at +0x5e4.
pub const DWORDS: usize = 18;
const D_RANGES: usize = 0x5ec;
// Data-block fields.
const B_COUNT: usize = 0x44;
const B_RECORDS: usize = 0x48;
// Record fields.
const R_ID: usize = 0x04;
const R_STRING: usize = 0x24c;
const R_STRING_LEN: usize = 0x48;
const R_FLAGS: usize = 0x294;
const R_DWORDS: [usize; 2] = [0x248, 0x2a4];

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
}

/// The editable fields of one hopper descriptor. Everything else in the
/// 0x614 bytes (the per-player data and strings, the type-name region) is
/// unknown and preserved in the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hopper {
    /// 1 to 5; the playlist UI shows hoppers of types {2, 4, 5}.
    pub hopper_type: u32,
    /// 0 = not loaded, 1 = ok, 2 = error.
    pub status: u32,
    /// 1 to 100; the game's validator (`0x00193560`) enforces this.
    pub record_count: u32,
    /// The playlist's name, ASCII.
    pub name: String,
    /// The range fields at +0x5ec/+0x5f0/+0x5f4 (party size limits).
    pub ranges: [u32; 3],
    /// The dword array at +0x5a0 (17 dwords) plus the dword at +0x5e4.
    pub dwords: Vec<u32>,
}

/// The editable fields of one record slot. The 0x40-byte template blob at
/// +0x08, the 0x200-byte copy at +0x48 and the remaining bytes are unknown
/// and preserved; move them with [`HopperFile::set_slot`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Record {
    /// Must resolve to a `matchmaking_hopper_descriptions` tag record, or
    /// the game marks the slot as an error (status 2).
    pub id: u32,
    /// The ASCII string at +0x24c.
    pub string: String,
    /// The flags dword at +0x294.
    pub flags: u32,
    /// The dwords at +0x248 and +0x2a4.
    pub dwords: [u32; 2],
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

    /// Reads a download envelope the way the game does (skip 16 bytes, check
    /// the big-endian size, inflate).
    pub fn parse(file: &[u8]) -> Result<Self, HopperError> {
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

    /// The file the game downloads: 16 zero bytes, the big-endian inflated
    /// size, and the zlib-compressed image. Deterministic for one image.
    pub fn build(&self) -> Vec<u8> {
        let mut out = vec![0u8; PREFIX];
        out.extend_from_slice(&(INFLATED_SIZE as u32).to_be_bytes());
        let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::best());
        z.write_all(&self.image).expect("writing to a Vec");
        z.finish().expect("writing to a Vec")
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
        let image = &self.image;
        Ok(Hopper {
            hopper_type: le32(image, at + D_TYPE),
            status: le32(image, at + D_STATUS),
            record_count: le32(image, at + D_COUNT),
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

    /// Writes hopper `i`'s editable descriptor fields, validating what the
    /// game validates. The record count is written to the descriptor only;
    /// [`HopperFile::set_record_count`] keeps the data block's copy in step.
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
        put_ascii(
            &mut self.image[at + D_NAME..][..D_NAME_LEN],
            &h.name,
            MAX_NAME,
        )?;
        put_le32(&mut self.image, at + D_TYPE, h.hopper_type);
        put_le32(&mut self.image, at + D_STATUS, h.status);
        put_le32(&mut self.image, at + D_COUNT, h.record_count);
        for n in 0..DWORDS {
            put_le32(&mut self.image, at + D_DWORDS + n * 4, h.dwords[n]);
        }
        for n in 0..3 {
            put_le32(&mut self.image, at + D_RANGES + n * 4, h.ranges[n]);
        }
        Ok(())
    }

    /// The record count in hopper `i`'s data block (kept equal to the
    /// descriptor's by [`HopperFile::set_record_count`]).
    pub fn block_record_count(&self, i: usize) -> Result<u32, HopperError> {
        let at = self.block_at(i)?;
        Ok(le32(&self.image, at + B_COUNT))
    }

    /// Sets the record count in both places (descriptor `+0x08` and block
    /// `+0x44`), as the game's own writer does.
    pub fn set_record_count(&mut self, i: usize, count: u32) -> Result<(), HopperError> {
        if !(1..=MAX_RECORDS as u32).contains(&count) {
            return Err(HopperError::Count(count));
        }
        let at = self.descriptor_at(i)?;
        let block = self.block_at(i)?;
        put_le32(&mut self.image, at + D_COUNT, count);
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
        let image = &self.image;
        Ok(Record {
            id: le32(image, at + R_ID),
            string: get_ascii(&image[at + R_STRING..][..R_STRING_LEN]),
            flags: le32(image, at + R_FLAGS),
            dwords: [le32(image, at + R_DWORDS[0]), le32(image, at + R_DWORDS[1])],
        })
    }

    /// Writes slot `j`'s editable fields; the rest of the slot is untouched.
    pub fn set_record(&mut self, i: usize, j: usize, r: &Record) -> Result<(), HopperError> {
        let at = self.slot_at(i, j)?;
        put_ascii(
            &mut self.image[at + R_STRING..][..R_STRING_LEN],
            &r.string,
            MAX_STRING,
        )?;
        put_le32(&mut self.image, at + R_ID, r.id);
        put_le32(&mut self.image, at + R_FLAGS, r.flags);
        put_le32(&mut self.image, at + R_DWORDS[0], r.dwords[0]);
        put_le32(&mut self.image, at + R_DWORDS[1], r.dwords[1]);
        Ok(())
    }

    /// The editable fields of hopper `i`'s first `record_count` records.
    pub fn records(&self, i: usize) -> Result<Vec<Record>, HopperError> {
        let count = self.hopper(i)?.record_count as usize;
        (0..count.min(MAX_RECORDS))
            .map(|j| self.record(i, j))
            .collect()
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
        assert_eq!(&file[..16], &[0u8; 16]);
        assert_eq!(&file[16..20], &[0x00, 0x16, 0x2c, 0xc0]);
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
