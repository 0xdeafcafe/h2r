//! Framing shared by the engine, stats importer and web player. A time
//! chunk has its own tick origin. A consumer can skip any record without
//! knowing the layout of its payload or retaining state from another chunk.
pub const TAG: [u8; 4] = *b"TICK";
pub const INPUT: u8 = 1;
pub const EVENT: u8 = 2;
pub const POSE: u8 = 3;
pub const HASH: u8 = 4;
pub const MARKER: u8 = 5;
/// An anchor for the legacy telemetry match clock at this film tick.
/// The empty payload is outside a match. Between anchors, a present clock
/// advances once per film tick and an absent clock stays absent.
pub const CLOCK: u8 = 6;
pub const LIFECYCLE_EVENT: u8 = 0;
pub const STATS_EVENT: u8 = 1;
/// Lifecycle observations between the last completed tick and the end of recording.
pub const CLOSING_EVENT: u8 = 2;
pub const MAX_TICKS: usize = 128;
pub const MAX_BYTES: usize = 8 << 20;

#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    pub tick: u64,
    pub tag: u8,
    pub payload: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
pub struct Chunk<'a> {
    pub first_tick: u64,
    pub tick_count: u16,
    last_tick: u64,
    body: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// The envelope is validated here. Walk records to check each length
    /// and timestamp, including lengths of unknown record types.
    pub fn parse(mut bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_BYTES {
            return Err("film time chunk exceeds 8 MiB");
        }
        let first_tick = read_varint(&mut bytes)?;
        let count = read_varint(&mut bytes)?;
        let last_tick = last_tick(first_tick, count)?;
        Ok(Self {
            first_tick,
            tick_count: count as u16,
            last_tick,
            body: bytes,
        })
    }

    pub fn records(&self) -> Records<'a> {
        Records {
            prior: self.first_tick,
            last: self.last_tick,
            remaining: self.body,
        }
    }
}

fn last_tick(first: u64, count: u64) -> Result<u64, &'static str> {
    if first == 0 || count == 0 || count > MAX_TICKS as u64 {
        return Err("invalid film time chunk bounds");
    }
    first
        .checked_add(count - 1)
        .ok_or("film time chunk tick overflow")
}

pub struct Records<'a> {
    prior: u64,
    last: u64,
    remaining: &'a [u8],
}

impl<'a> Iterator for Records<'a> {
    type Item = Result<Record<'a>, &'static str>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let item = (|| {
            let delta = read_varint(&mut self.remaining)?;
            let tick = self
                .prior
                .checked_add(delta)
                .filter(|&t| t <= self.last)
                .ok_or("record outside time chunk")?;
            let tag = byte(&mut self.remaining)?;
            let length = usize::try_from(read_varint(&mut self.remaining)?)
                .map_err(|_| "film record length overflow")?;
            let (payload, rest) = self
                .remaining
                .split_at_checked(length)
                .ok_or("truncated film record")?;
            self.remaining = rest;
            self.prior = tick;
            Ok(Record { tick, tag, payload })
        })();
        if item.is_err() {
            self.remaining = &[];
        }
        Some(item)
    }
}

fn byte(bytes: &mut &[u8]) -> Result<u8, &'static str> {
    let (&byte, rest) = bytes.split_first().ok_or("truncated film record")?;
    *bytes = rest;
    Ok(byte)
}

pub fn read_varint(input: &mut &[u8]) -> Result<u64, &'static str> {
    let mut value = 0;
    for shift in (0..=63).step_by(7) {
        let b = byte(input)?;
        if shift == 63 && b > 1 {
            return Err("film varint overflow");
        }
        value |= u64::from(b & 127) << shift;
        if b & 128 == 0 {
            if shift != 0 && b == 0 {
                return Err("noncanonical film varint");
            }
            return Ok(value);
        }
    }
    Err("unterminated film varint")
}

pub fn decode_clock(mut payload: &[u8]) -> Result<Option<u64>, &'static str> {
    if payload.is_empty() {
        return Ok(None);
    }
    let tick = read_varint(&mut payload)?;
    if !payload.is_empty() {
        return Err("trailing film clock bytes");
    }
    Ok(Some(tick))
}

pub fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push(value as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

pub struct ChunkWriter {
    prior: u64,
    last: u64,
    raw: Vec<u8>,
}

impl ChunkWriter {
    pub fn new(first_tick: u64, count: u16) -> Result<Self, &'static str> {
        let last = last_tick(first_tick, u64::from(count))?;
        let mut raw = Vec::new();
        write_varint(first_tick, &mut raw);
        write_varint(u64::from(count), &mut raw);
        Ok(Self {
            prior: first_tick,
            last,
            raw,
        })
    }

    pub fn record(&mut self, tick: u64, tag: u8, payload: &[u8]) -> Result<(), &'static str> {
        if !(self.prior..=self.last).contains(&tick) {
            return Err("out of order film record");
        }
        let start = self.raw.len();
        write_varint(tick - self.prior, &mut self.raw);
        self.raw.push(tag);
        write_varint(payload.len() as u64, &mut self.raw);
        if self.raw.len().saturating_add(payload.len()) > MAX_BYTES {
            self.raw.truncate(start);
            return Err("film time chunk exceeds 8 MiB");
        }
        self.raw.extend_from_slice(payload);
        self.prior = tick;
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        self.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_skip_foreign_and_unknown_payloads_at_the_same_tick() {
        let mut b = ChunkWriter::new(123_000, 30).unwrap();
        b.record(123_000, 0xfe, &[0xff, 0xff]).unwrap();
        b.record(123_002, INPUT, &[4]).unwrap();
        b.record(123_002, EVENT, &[STATS_EVENT, 8]).unwrap();
        b.record(123_004, POSE, &[9]).unwrap();
        let raw = b.finish();
        for (tag, expected_tick) in [(INPUT, 123_002), (EVENT, 123_002), (POSE, 123_004)] {
            let records: Vec<_> = Chunk::parse(&raw)
                .unwrap()
                .records()
                .map(Result::unwrap)
                .filter(|r| r.tag == tag)
                .map(|r| r.tick)
                .collect();
            assert_eq!(records, [expected_tick]);
        }
    }

    #[test]
    fn bad_foreign_length_or_stamp_stops_the_walker() {
        for raw in [
            &[1, 1, 0, 0xff, 3, 1][..],
            &[1, 1, 1, 0xff, 0],
            &[1, 1, 0, 0xff, 0x80, 0],
        ] {
            let mut records = Chunk::parse(raw).unwrap().records();
            assert!(records.next().unwrap().is_err());
            assert!(records.next().is_none());
        }
    }
}
