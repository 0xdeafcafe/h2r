//! The areas a Studio map carries (docs/game/gametype-markers.md, "Areas
//! saved with a map"): a record Studio appends to the end of the `.map` it
//! saves, which the game reads when the map loads.
//!
//! A flag has no spare bytes for an area, so the areas are a table at the
//! file's tail: one row per netgame flag that has one, keyed by the flag's
//! block, index and identifier, with a fingerprint (its type and the bits
//! of its position) so a row finds its flag again when flags were added or
//! removed since. The distances are the exact `f32` bits the Forge
//! document spells in hex (`forge_doc::Area`), so a record and a document
//! name one area one way.
//!
//! ```text
//! record  = "H2AR" version:u8 count:u16le row*count crc:u32le
//! row     = block:u8 index:u32le type:i16le identifier:i16le
//!           at:[u32le;3] shape:u8 below:u32le above:u32le
//!           (radius:u32le | points:u8 [u32le;2]*points | half:[u32le;2] | radius:u32le)
//! shape   = kind | SHOWN
//! kind    = 0 radius, 1 path, 2 box, 3 sphere (a sphere's below and above are zero),
//!           4 none (the row says only that the flag is shown: nothing follows
//!           the kind byte but the next row)
//! SHOWN   = 0x80, the flag's play form shows in a match
//! trailer = length:u32le "H2AE"          ; the last 8 bytes of the file
//! ```
//!
//! `length` is the record's own, `crc` the FNV-1a of the bytes before it.
//! A file with no trailer has no areas. A later save appends a new record
//! after an older one; the reader takes the one at the very end.
use std::io::{Read, Seek, SeekFrom};

use crate::area::{Area, Shape};

pub const MAGIC: [u8; 4] = *b"H2AR";
pub const END: [u8; 4] = *b"H2AE";
pub const VERSION: u8 = 1;
/// The block of a netgame flag in a row (the only block that has areas).
pub const NETGAME_FLAGS: u8 = 0;
/// The most rows a record holds.
pub const MAX_ROWS: usize = 256;
/// The bit of a row's shape byte that says the flag's play form shows in a
/// match (docs/game/gametype-marker-forms.md, "When a form shows").
pub const SHOWN: u8 = 0x80;
/// The kind of a row that has no area, only [`Row::show`].
pub const NO_AREA: u8 = 4;
/// The trailer's size in bytes.
pub const TRAILER: usize = 8;

/// One netgame flag's area, with what finds the flag again.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The flag's index when the record was written: a hint, tried first.
    pub index: u32,
    /// The flag's type (the hill, territory or site kind).
    pub flag_type: i16,
    /// The flag's identifier (a hill's number).
    pub identifier: i16,
    /// The flag's position when the record was written.
    pub at: [f32; 3],
    /// The flag's own area, or none for a row that only shows the flag.
    pub area: Option<Area>,
    /// Whether the flag's play form shows in a match. A map of the map's
    /// own flags shows none by default, so a row says so for the flags that do.
    pub show: bool,
}

/// A netgame flag as the game or Studio reads it, for [`place`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flag {
    pub flag_type: i16,
    pub identifier: i16,
    pub at: [f32; 3],
}

/// A record that does not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    Truncated,
    Magic,
    Version(u8),
    Checksum,
    Shape(u8),
    TooMany(usize),
    Area(String),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the areas record is cut short"),
            Self::Magic => write!(f, "the areas record has the wrong magic"),
            Self::Version(v) => write!(f, "the areas record is version {v}, not {VERSION}"),
            Self::Checksum => write!(f, "the areas record's checksum does not match"),
            Self::Shape(s) => write!(f, "the areas record has a block or shape {s} it does not know"),
            Self::TooMany(n) => write!(f, "the areas record has {n} rows, over {MAX_ROWS}"),
            Self::Area(why) => write!(f, "the areas record holds an area that does not validate: {why}"),
        }
    }
}

impl std::error::Error for RecordError {}

fn fnv(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0x811c_9dc5u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// The record for `rows` with its trailer, ready to append to a file: at
/// most [`MAX_ROWS`] rows (the rest are not written). An empty `rows` is
/// a record of none, which a later save writes to take the areas back off.
pub fn encode(rows: &[Row]) -> Vec<u8> {
    let rows = &rows[..rows.len().min(MAX_ROWS)];
    let mut w = Vec::new();
    w.extend_from_slice(&MAGIC);
    w.push(VERSION);
    w.extend_from_slice(&(rows.len() as u16).to_le_bytes());
    for r in rows {
        w.push(NETGAME_FLAGS);
        w.extend_from_slice(&r.index.to_le_bytes());
        w.extend_from_slice(&r.flag_type.to_le_bytes());
        w.extend_from_slice(&r.identifier.to_le_bytes());
        for v in r.at {
            w.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        let shown = if r.show { SHOWN } else { 0 };
        let Some(area) = &r.area else {
            w.push(NO_AREA | shown);
            continue;
        };
        match &area.shape {
            Shape::Radius(_) => w.push(shown),
            Shape::Path(_) => w.push(1 | shown),
            Shape::Box { .. } => w.push(2 | shown),
            Shape::Sphere(_) => w.push(3 | shown),
        }
        // A sphere has no height: its two are written as zero.
        let (below, above) = match area.shape {
            Shape::Sphere(_) => (0.0, 0.0),
            _ => (area.below, area.above),
        };
        w.extend_from_slice(&below.to_bits().to_le_bytes());
        w.extend_from_slice(&above.to_bits().to_le_bytes());
        match &area.shape {
            Shape::Radius(radius) | Shape::Sphere(radius) => {
                w.extend_from_slice(&radius.to_bits().to_le_bytes());
            }
            Shape::Box { half } => {
                for v in half {
                    w.extend_from_slice(&v.to_bits().to_le_bytes());
                }
            }
            Shape::Path(points) => {
                w.push(points.len().min(255) as u8);
                for p in points.iter().take(255) {
                    for v in p {
                        w.extend_from_slice(&v.to_bits().to_le_bytes());
                    }
                }
            }
        }
    }
    let crc = fnv(&w);
    w.extend_from_slice(&crc.to_le_bytes());
    let len = w.len() as u32;
    w.extend_from_slice(&len.to_le_bytes());
    w.extend_from_slice(&END);
    w
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], RecordError> {
        let out = self.b.get(self.at..self.at + n).ok_or(RecordError::Truncated)?;
        self.at += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, RecordError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, RecordError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Result<i16, RecordError> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32, RecordError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Result<f32, RecordError> {
        Ok(f32::from_bits(self.u32()?))
    }
}

/// The rows of one record (without its trailer). Every area is validated.
pub fn decode(record: &[u8]) -> Result<Vec<Row>, RecordError> {
    if record.len() < 4 {
        return Err(RecordError::Truncated);
    }
    let (body, crc) = record.split_at(record.len() - 4);
    if fnv(body).to_le_bytes() != crc {
        return Err(RecordError::Checksum);
    }
    let mut r = Reader { b: body, at: 0 };
    if r.take(4)? != MAGIC {
        return Err(RecordError::Magic);
    }
    let version = r.u8()?;
    if version != VERSION {
        return Err(RecordError::Version(version));
    }
    let n = usize::from(r.u16()?);
    if n > MAX_ROWS {
        return Err(RecordError::TooMany(n));
    }
    let mut rows = Vec::with_capacity(n);
    for _ in 0..n {
        let block = r.u8()?;
        if block != NETGAME_FLAGS {
            return Err(RecordError::Shape(block));
        }
        let index = r.u32()?;
        let flag_type = r.i16()?;
        let identifier = r.i16()?;
        let at = [r.f32()?, r.f32()?, r.f32()?];
        let shape = r.u8()?;
        let show = shape & SHOWN != 0;
        let shape = shape & !SHOWN;
        if shape == NO_AREA {
            rows.push(Row {
                index,
                flag_type,
                identifier,
                at,
                area: None,
                show,
            });
            continue;
        }
        let (mut below, mut above) = (r.f32()?, r.f32()?);
        let shape = match shape {
            0 => Shape::Radius(r.f32()?),
            1 => {
                let k = usize::from(r.u8()?);
                let mut points = Vec::with_capacity(k);
                for _ in 0..k {
                    points.push([r.f32()?, r.f32()?]);
                }
                Shape::Path(points)
            }
            2 => Shape::Box {
                half: [r.f32()?, r.f32()?],
            },
            3 => {
                (below, above) = (0.0, 0.0);
                Shape::Sphere(r.f32()?)
            }
            other => return Err(RecordError::Shape(other)),
        };
        let area = Area { shape, below, above };
        area.validate().map_err(|e| RecordError::Area(e.to_string()))?;
        rows.push(Row {
            index,
            flag_type,
            identifier,
            at,
            area: Some(area),
            show,
        });
    }
    if r.at != body.len() {
        return Err(RecordError::Truncated);
    }
    Ok(rows)
}

/// The rows of the record at the end of `file`, or none when the file has
/// no trailer: read from the end, so a map of any size costs two reads.
pub fn read_tail<R: Read + Seek>(file: &mut R) -> Result<Option<Vec<Row>>, RecordError> {
    let io = |_| RecordError::Truncated;
    let len = file.seek(SeekFrom::End(0)).map_err(io)?;
    if len < TRAILER as u64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - TRAILER as u64)).map_err(io)?;
    let mut t = [0u8; TRAILER];
    file.read_exact(&mut t).map_err(io)?;
    if t[4..] != END {
        return Ok(None);
    }
    let n = u64::from(u32::from_le_bytes([t[0], t[1], t[2], t[3]]));
    if n + TRAILER as u64 > len {
        return Err(RecordError::Truncated);
    }
    file.seek(SeekFrom::Start(len - TRAILER as u64 - n)).map_err(io)?;
    let mut record = vec![0u8; n as usize];
    file.read_exact(&mut record).map_err(io)?;
    decode(&record).map(Some)
}

/// Which flag each row belongs to, by flag index: a row finds the flag at
/// its own index when that flag has its type and its position's bits, else
/// the first flag not yet taken with its type, identifier and position
/// bits. A flag takes one row. Rows that find no flag are returned in the
/// second list (the flag moved, changed type or was removed after the
/// save), and the game and Studio leave them out.
pub fn place(rows: &[Row], flags: &[Flag]) -> (Vec<(usize, Area)>, Vec<usize>) {
    let (found, lost) = place_rows(rows, flags);
    (
        found
            .into_iter()
            .filter_map(|(i, row)| Some((i, row.area.clone()?)))
            .collect(),
        lost,
    )
}

/// [`place`] giving each flag its whole row, for what the row says besides
/// the area (its [`Row::show`]).
pub fn place_rows<'a>(rows: &'a [Row], flags: &[Flag]) -> (Vec<(usize, &'a Row)>, Vec<usize>) {
    let bits = |p: [f32; 3]| p.map(f32::to_bits);
    let same = |row: &Row, f: &Flag| row.flag_type == f.flag_type && bits(row.at) == bits(f.at);
    let mut taken = vec![false; flags.len()];
    let mut placed = Vec::new();
    let mut lost = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let hinted = usize::try_from(row.index)
            .ok()
            .filter(|&i| flags.get(i).is_some_and(|f| !taken[i] && same(row, f)));
        let found = hinted.or_else(|| {
            flags
                .iter()
                .enumerate()
                .position(|(i, f)| !taken[i] && same(row, f) && f.identifier == row.identifier)
        });
        match found {
            Some(i) => {
                taken[i] = true;
                placed.push((i, row));
            }
            None => lost.push(n),
        }
    }
    placed.sort_by_key(|(i, _)| *i);
    (placed, lost)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            Row {
                index: 2,
                flag_type: 11,
                identifier: 0,
                at: [1.0, 2.0, 0.5],
                area: Some(Area::radius(2.5)),
                show: true,
            },
            Row {
                index: 5,
                flag_type: 10,
                identifier: 3,
                at: [-4.0, 0.25, 1.0],
                area: Some(Area::path(vec![[-2.0, -2.0], [2.0, -2.0], [0.0, 3.0]])),
                show: false,
            },
        ]
    }

    #[test]
    fn a_record_round_trips_bit_for_bit_at_the_end_of_a_file() {
        let mut file = vec![7u8; 100];
        file.extend(encode(&rows()));
        let back = read_tail(&mut Cursor::new(file)).unwrap().unwrap();
        assert_eq!(back, rows());
        // A file with no trailer has no areas; an empty record is none.
        assert_eq!(read_tail(&mut Cursor::new(vec![7u8; 100])).unwrap(), None);
        assert_eq!(read_tail(&mut Cursor::new(encode(&[]))).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn a_rows_shown_bit_rides_the_shape_byte_and_places_with_its_flag() {
        let r = rows();
        let back = decode(&encode(&r)[..encode(&r).len() - TRAILER]).unwrap();
        assert_eq!(back.iter().map(|row| row.show).collect::<Vec<_>>(), [true, false]);
        // The shape survives the bit: the path stays a path.
        assert_eq!(back[1].area, r[1].area);
        // A row with no area only shows its flag, and places with none.
        let only = [Row {
            index: 0,
            flag_type: 11,
            identifier: 0,
            at: [1.0, 2.0, 0.5],
            area: None,
            show: true,
        }];
        let bytes = encode(&only);
        assert_eq!(decode(&bytes[..bytes.len() - TRAILER]).unwrap(), only);
        let flag = Flag {
            flag_type: 11,
            identifier: 0,
            at: [1.0, 2.0, 0.5],
        };
        assert!(place(&only, &[flag]).0.is_empty());
        assert_eq!(place_rows(&only, &[flag]).0.len(), 1);
        let flags = [Flag {
            flag_type: 11,
            identifier: 0,
            at: [1.0, 2.0, 0.5],
        }];
        let (placed, _) = place_rows(&r, &flags);
        assert_eq!(placed.len(), 1);
        assert!(placed[0].1.show);
    }

    #[test]
    fn the_last_record_wins_and_a_damaged_one_is_refused() {
        let mut file = encode(&rows());
        file.extend(encode(&rows()[..1]));
        assert_eq!(read_tail(&mut Cursor::new(file.clone())).unwrap().unwrap().len(), 1);
        let n = file.len();
        file[n - 20] ^= 1;
        assert_eq!(read_tail(&mut Cursor::new(file)), Err(RecordError::Checksum));
        let mut bad = encode(&rows());
        let n = bad.len();
        bad[n - 8..n - 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_tail(&mut Cursor::new(bad)), Err(RecordError::Truncated));
    }

    #[test]
    fn an_area_that_does_not_validate_is_not_read() {
        let mut r = rows();
        r[0].area = Some(Area::radius(500.0));
        let bytes = encode(&r);
        assert!(matches!(
            decode(&bytes[..bytes.len() - TRAILER]),
            Err(RecordError::Area(_))
        ));
    }

    #[test]
    fn a_row_finds_its_flag_again_when_flags_were_added_or_removed() {
        let r = rows();
        let flag = |t, i, at| Flag {
            flag_type: t,
            identifier: i,
            at,
        };
        // Two flags were added before them: the hints are wrong, the
        // fingerprints right.
        let flags = [
            flag(0, 0, [9.0; 3]),
            flag(1, 0, [8.0; 3]),
            flag(10, 3, [-4.0, 0.25, 1.0]),
            flag(11, 0, [1.0, 2.0, 0.5]),
        ];
        let (placed, lost) = place(&r, &flags);
        assert_eq!(placed.iter().map(|p| p.0).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(Some(&placed[1].1), r[0].area.as_ref());
        assert!(lost.is_empty());
        // A flag that moved loses its row.
        let flags = [flag(11, 0, [1.0, 2.0, 0.6]), flag(10, 3, [-4.0, 0.25, 1.0])];
        let (placed, lost) = place(&r, &flags);
        assert_eq!((placed.len(), lost), (1, vec![0]));
        // A flag takes one row.
        let twins = [flag(11, 0, [1.0, 2.0, 0.5]); 2];
        let (placed, lost) = place(&[r[0].clone(), r[0].clone()], &twins);
        assert_eq!((placed.len(), lost.len()), (2, 0));
        assert_eq!(placed.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1]);
    }
}
