//! V3 public front matter and the chronological directory. A remote reader can
//! implement Read + Seek over HTTP ranges, or use the shared prefix/footer
//! decoders directly when it needs to issue async range requests.
use crate::{
    Entry, HEAD, Header, INDX, KEYF, MAX_CHUNK, MAX_ENTRIES, Reader, blam, prefix, read_bytes,
};
use anyhow::{Context, Result, ensure};
pub use live_proto::film::range::*;
use std::io::{Read, Seek, SeekFrom};

pub(super) struct Build {
    pub front: Front,
    pub directory: Directory,
    pub has_summary: bool,
}
impl Build {
    pub fn new(header: &Header) -> Self {
        Self {
            front: Front {
                final_head: DATA_OFFSET,
                id: header.meta.id.clone(),
                image_stamp: header.meta.image_stamp.clone(),
                runtime_stamp: header.runtime_stamp.clone(),
                started_ms: header.meta.started_ms,
                map_sha256: String::new(),
                map_origin: String::new(),
                match_start_tick: None,
                ticks_per_second: None,
                roster: Vec::new(),
                summary: Default::default(),
            },
            directory: Directory::default(),
            has_summary: false,
        }
    }
    pub fn set_descriptor(&mut self, bytes: &[u8]) -> Result<()> {
        let desc: blam::MatchHeader = serde_json::from_slice(bytes)?;
        self.front.roster = desc
            .roster
            .into_iter()
            .map(|s| Seat {
                seat: s.seat,
                name: s.name,
                team: s.team,
                kind: match s.kind {
                    blam::SeatKind::Local => "local",
                    blam::SeatKind::Remote => "remote",
                    blam::SeatKind::Phantom => "phantom",
                    blam::SeatKind::Bot => "bot",
                }
                .to_owned(),
            })
            .collect();
        self.front.map_sha256 = desc.map.sha256;
        self.front.map_origin = desc.map.origin;
        self.front.match_start_tick = desc.match_start_tick;
        self.front.ticks_per_second = desc.ticks_per_second;
        Ok(())
    }
    pub fn set_summary(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            !self.has_summary && bytes.len() <= live_proto::film::MAX_SUMMARY_BYTES,
            "invalid or duplicate front summary"
        );
        self.front.summary = serde_json::from_slice(bytes)?;
        live_proto::film::privacy::public_summary(&mut self.front.summary);
        self.has_summary = true;
        Ok(())
    }
}

pub(super) fn key_tick(mut bytes: &[u8]) -> Result<u64> {
    blam::stream::read_varint(&mut bytes).map_err(anyhow::Error::msg)
}

pub(super) fn check_ticks(bytes: &[u8], first: u64, last: u64) -> Result<()> {
    let block = blam::stream::Chunk::parse(bytes).map_err(anyhow::Error::msg)?;
    ensure!(
        block.first_tick == first && block.first_tick + u64::from(block.tick_count - 1) == last,
        "time chunk differs from range directory"
    );
    Ok(())
}

fn front<R: Read + Seek>(input: &mut R) -> Result<Front> {
    input.rewind()?;
    let mut b = [0; PREFIX_BYTES];
    input.read_exact(&mut b)?;
    let p = Prefix::decode(&b).map_err(anyhow::Error::msg)?;
    let mut bytes = vec![0; p.json_len as usize];
    input.read_exact(&mut bytes)?;
    let front: Front = serde_json::from_slice(&bytes)?;
    ensure!(
        front
            .summary
            .players
            .iter()
            .all(|p| usize::from(p.seat) < blam::MAX_SEATS),
        "invalid film score seat"
    );
    let mut occupied = [false; blam::MAX_SEATS];
    for seat in &front.roster {
        let slot = occupied
            .get_mut(usize::from(seat.seat))
            .context("invalid film roster seat")?;
        ensure!(!*slot, "duplicate roster seat");
        *slot = true;
    }
    Ok(front)
}
fn footer<R: Read + Seek>(input: &mut R, file_size: u64) -> Result<Footer> {
    input.seek(SeekFrom::End(-(FOOTER_BYTES as i64)))?;
    let mut b = [0; FOOTER_BYTES];
    input.read_exact(&mut b)?;
    Footer::decode(&b, file_size).map_err(anyhow::Error::msg)
}

fn directory<R: Read + Seek>(input: &mut R, footer: &Footer) -> Result<Directory> {
    let e = prefix(input, footer.cidx_offset)?;
    ensure!(
        e.tag == CIDX && footer.indx_offset - footer.cidx_offset == 8 + u64::from(e.size),
        "invalid chronological index location"
    );
    let bytes = read_bytes(input, &e, footer.cidx_raw_size as usize)?;
    ensure!(
        bytes.len() == footer.cidx_raw_size as usize,
        "incorrect chronological index length"
    );
    let index: Directory = serde_json::from_slice(&bytes)?;
    index.validate(footer).map_err(anyhow::Error::msg)?;
    Ok(index)
}

/// Read only the public header and menu summary. A healthy footer is
/// required for playback but the fixed front can be decoded by itself.
pub fn read_front<R: Read + Seek>(input: &mut R) -> Result<Front> {
    front(input)
}

/// Read the finalised local menu data in constant I/O, regardless of match length.
pub(super) fn listing<R: Read + Seek>(
    input: &mut R,
) -> Result<Option<(crate::Header, blam::Summary)>> {
    let mut magic = [0; 8];
    input.rewind()?;
    input.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Ok(None);
    }
    let len = input.seek(SeekFrom::End(0))?;
    let footer = footer(input, len)?;
    let front = front(input)?;
    ensure!(
        (DATA_OFFSET..footer.cidx_offset).contains(&front.final_head),
        "invalid final film header reference"
    );
    let head = prefix(input, front.final_head)?;
    ensure!(
        head.tag == HEAD
            && head
                .offset
                .checked_add(8 + u64::from(head.size))
                .is_some_and(|n| n <= footer.cidx_offset),
        "invalid final film header bounds"
    );
    let header: Header = crate::decode(input, &head)?;
    validate_header(&header, &front)?;
    Ok(Some((header, front.summary)))
}

fn validate_header(header: &Header, front: &Front) -> Result<()> {
    ensure!(
        header.version == 3
            && header.meta.id == front.id
            && header.meta.started_ms == front.started_ms
            && header.meta.image_stamp == front.image_stamp
            && header.runtime_stamp == front.runtime_stamp,
        "v3 public and private film headers differ"
    );
    Ok(())
}

/// A client interested in a time range never needs the private machine index.
pub struct RangeReader<R> {
    input: R,
    pub front: Front,
    pub directory: Directory,
}
impl<R: Read + Seek> RangeReader<R> {
    pub fn new(mut input: R) -> Result<Self> {
        let len = input.seek(SeekFrom::End(0))?;
        let front = front(&mut input)?;
        let footer = footer(&mut input, len)?;
        let directory = directory(&mut input, &footer)?;
        Ok(Self {
            input,
            front,
            directory,
        })
    }
    /// Read and check one physical range, including its type, framing and clock.
    pub fn chunk(&mut self, n: usize) -> Result<Vec<u8>> {
        let c = self
            .directory
            .chunks
            .get(n)
            .context("unknown film time chunk")?;
        let entry = Entry {
            tag: TICK,
            offset: c.offset,
            size: c.size,
        };
        let raw = read_bytes(&mut self.input, &entry, c.raw_size as usize)?;
        ensure!(
            raw.len() == c.raw_size as usize,
            "incorrect time chunk length"
        );
        check_ticks(&raw, c.first, c.last)?;
        ensure!(
            clock_runs(&raw).map_err(anyhow::Error::msg)? == c.clock,
            "time chunk guest clock differs from the range directory"
        );
        Ok(raw)
    }
}

pub(super) fn open<R: Read + Seek>(mut input: R, len: u64) -> Result<Reader<R>> {
    let footer = footer(&mut input, len)?;
    let front = front(&mut input)?;
    let ranges = directory(&mut input, &footer)?;
    let index = prefix(&mut input, footer.indx_offset)?;
    ensure!(
        index.tag == INDX
            && len - footer.indx_offset - FOOTER_BYTES as u64 == 8 + u64::from(index.size),
        "incorrect v3 machine index bounds"
    );
    let entries = crate::machine::decode(&mut input, &index)?;
    ensure!(
        entries.len() <= MAX_ENTRIES
            && entries.first().is_some_and(|e| e.tag == HEAD)
            && entries
                .last()
                .is_some_and(|e| e.tag == CIDX && e.offset == footer.cidx_offset),
        "incomplete v3 machine index"
    );
    let mut expected = DATA_OFFSET;
    let mut times = ranges.chunks.iter();
    let mut keys = ranges.keyframes.iter();
    for e in &entries {
        ensure!(
            e.tag != INDX && e.offset == expected && e.size > 0 && e.size as usize <= MAX_CHUNK,
            "invalid v3 film chunk"
        );
        expected = expected
            .checked_add(8 + u64::from(e.size))
            .context("film offset overflow")?;
        match e.tag {
            TICK => ensure!(
                times
                    .next()
                    .is_some_and(|c| c.offset == e.offset && c.size == e.size),
                "inconsistent chronological chunk reference"
            ),
            KEYF => ensure!(
                keys.next().is_some_and(|k| k.offset == e.offset),
                "inconsistent keyframe reference"
            ),
            CIDX => ensure!(
                e.offset == footer.cidx_offset,
                "duplicate chronological directory"
            ),
            _ => {}
        }
    }
    ensure!(
        expected == footer.indx_offset && times.next().is_none() && keys.next().is_none(),
        "incomplete v3 film directory"
    );
    let initial: Header = crate::decode(&mut input, &entries[0])?;
    let last = entries.iter().rfind(|e| e.tag == HEAD).unwrap();
    ensure!(
        last.offset == front.final_head,
        "incorrect final film header reference"
    );
    let header: Header = crate::decode(&mut input, last)?;
    ensure!(initial.telemetry_hooks == header.telemetry_hooks, "film telemetry hook plan changed");
    ensure!(initial.render_hooks == header.render_hooks, "film render hook profile changed");
    for h in [&initial, &header] {
        validate_header(h, &front)?;
    }
    Ok(Reader {
        input,
        header,
        front: Some(front),
        ranges: Some(ranges),
        entries,
        finalized: true,
    })
}

#[cfg(test)]
mod tests;
