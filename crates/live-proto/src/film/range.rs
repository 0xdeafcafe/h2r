//! Public byte ranges in a unified film. Byte counts in the directory
//! exclude the four-byte tag and four-byte little-endian packed length.
use super::Summary;
use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 8] = *b"H2BLAM3!";
pub const FOOTER_MAGIC: [u8; 8] = *b"H2RANGE3";
pub const PREFIX_BYTES: usize = 32;
pub const FRONT_CAPACITY: usize = 128 << 10;
pub const DATA_OFFSET: u64 = (PREFIX_BYTES + FRONT_CAPACITY) as u64;
pub const FOOTER_BYTES: usize = 40;
pub const MAX_TICK_BYTES: usize = super::stream::MAX_BYTES;
/// Inclusive scene replay window, at approximately 30 simulation ticks per second.
pub const SCENE_WINDOW: u64 = 2048;
pub const MAX_SCENE_CHUNKS: usize = 16;
pub const MAX_TICK_PACKED: usize = MAX_TICK_BYTES + (64 << 10);
pub const MAX_INDEX_BYTES: usize = 8 << 20;
pub const MAX_INDEX_PACKED: usize = 16 << 20;
pub const MAX_ROWS: usize = 1_000_000;
pub const TICK: [u8; 4] = *b"TICK";
pub const CIDX: [u8; 4] = *b"CIDX";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefix {
    pub json_len: u32,
}
impl Prefix {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let b: &[u8; PREFIX_BYTES] = bytes.try_into().map_err(|_| "wrong film prefix length")?;
        if b[..8] != MAGIC
            || b[8..12] != 3u32.to_le_bytes()
            || b[12..16] != (FRONT_CAPACITY as u32).to_le_bytes()
            || b[20..] != [0; 12]
        {
            return Err("unsupported film front header".into());
        }
        let json_len = u32::from_le_bytes(b[16..20].try_into().unwrap());
        if json_len == 0 || json_len as usize > FRONT_CAPACITY {
            return Err("missing or oversized front summary".into());
        }
        Ok(Self { json_len })
    }

    pub fn encode(&self) -> [u8; PREFIX_BYTES] {
        let mut b = [0; PREFIX_BYTES];
        b[..8].copy_from_slice(&MAGIC);
        b[8..12].copy_from_slice(&3u32.to_le_bytes());
        b[12..16].copy_from_slice(&(FRONT_CAPACITY as u32).to_le_bytes());
        b[16..20].copy_from_slice(&self.json_len.to_le_bytes());
        b
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Seat {
    pub seat: u8,
    pub name: String,
    pub team: i32,
    pub kind: String,
}

/// The menu and web page can read this object without the private boot
/// manifest, HLE state or raw retail variant bytes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Front {
    /// Local library metadata. The addressed private HEAD is never a public range.
    pub final_head: u64,
    pub id: String,
    pub image_stamp: String,
    pub runtime_stamp: String,
    pub started_ms: u64,
    pub map_sha256: String,
    pub map_origin: String,
    pub match_start_tick: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks_per_second: Option<u16>,
    pub roster: Vec<Seat>,
    pub summary: Summary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footer {
    pub cidx_offset: u64,
    pub indx_offset: u64,
    pub file_len: u64,
    pub cidx_raw_size: u32,
}
impl Footer {
    pub fn decode(bytes: &[u8], file_size: u64) -> Result<Self, String> {
        let b: &[u8; FOOTER_BYTES] = bytes.try_into().map_err(|_| "wrong film footer length")?;
        if b[..8] != FOOTER_MAGIC || b[36..40] != [0; 4] {
            return Err("unsupported film range footer".into());
        }
        let f = Self {
            cidx_offset: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            indx_offset: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            file_len: u64::from_le_bytes(b[24..32].try_into().unwrap()),
            cidx_raw_size: u32::from_le_bytes(b[32..36].try_into().unwrap()),
        };
        let span = f.indx_offset.checked_sub(f.cidx_offset);
        let tail = file_size.checked_sub(f.indx_offset);
        if f.file_len != file_size
            || f.cidx_offset < DATA_OFFSET
            || !span.is_some_and(|n| (9..=8 + MAX_INDEX_PACKED as u64).contains(&n))
            || !tail.is_some_and(|n| n > FOOTER_BYTES as u64 + 8)
            || f.cidx_raw_size == 0
            || f.cidx_raw_size as usize > MAX_INDEX_BYTES
        {
            return Err("invalid film range footer bounds".into());
        }
        Ok(f)
    }
    pub fn encode(&self) -> [u8; FOOTER_BYTES] {
        let mut b = [0; FOOTER_BYTES];
        b[..8].copy_from_slice(&FOOTER_MAGIC);
        b[8..16].copy_from_slice(&self.cidx_offset.to_le_bytes());
        b[16..24].copy_from_slice(&self.indx_offset.to_le_bytes());
        b[24..32].copy_from_slice(&self.file_len.to_le_bytes());
        b[32..36].copy_from_slice(&self.cidx_raw_size.to_le_bytes());
        b
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Chunk {
    /// Changes to the guest-clock run inside this boot-time chunk.
    /// Every known run advances by one per film tick until the next anchor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clock: Vec<(u8, Option<u64>)>,
    /// First tick of the nearest chunk containing a full public scene.
    pub scene: u64,
    pub first: u64,
    pub last: u64,
    pub offset: u64,
    pub size: u32,
    pub raw_size: u32,
}

/// Collect the compact clock map from the same records consumed by playback.
pub fn clock_runs(raw: &[u8]) -> Result<Vec<(u8, Option<u64>)>, String> {
    use super::stream;
    let block = stream::Chunk::parse(raw)?;
    let mut anchors = Vec::new();
    let mut previous = None;
    for record in block.records() {
        let record = record?;
        if record.tag != stream::CLOCK {
            continue;
        }
        let offset =
            u8::try_from(record.tick - block.first_tick).map_err(|_| "clock outside chunk")?;
        if previous == Some(offset) {
            return Err("duplicate clock anchor".into());
        }
        previous = Some(offset);
        let guest = stream::decode_clock(record.payload)?;
        // A scene without an observed match clock has an empty map.
        if !anchors.is_empty() || guest.is_some() {
            anchors.push((offset, guest));
        }
    }
    Ok(anchors)
}

impl Chunk {
    pub fn guest_tick(&self, film: u64) -> Option<u64> {
        if !(self.first..=self.last).contains(&film) {
            return None;
        }
        let offset = film - self.first;
        let end = self
            .clock
            .partition_point(|(at, _)| u64::from(*at) <= offset);
        let (at, guest) = self.clock.get(end.checked_sub(1)?)?;
        (*guest)?.checked_add(offset - u64::from(*at))
    }
    fn valid_clock(&self) -> bool {
        for (i, (at, guest)) in self.clock.iter().copied().enumerate() {
            if u64::from(at) > self.last - self.first {
                return false;
            }
            let length = if let Some((next, _)) = self.clock.get(i + 1) {
                if *next <= at {
                    return false;
                }
                u64::from(*next - at)
            } else {
                (self.last - self.first) - u64::from(at) + 1
            };
            if guest.is_some_and(|tick| tick.checked_add(length - 1).is_none()) {
                return false;
            }
        }
        true
    }
    fn locate_guest(&self, tick: u64) -> Option<u64> {
        for (i, (offset, start)) in self.clock.iter().copied().enumerate() {
            let Some(delta) = start.and_then(|start| tick.checked_sub(start)) else {
                continue;
            };
            let end = self
                .clock
                .get(i + 1)
                .map_or(self.last - self.first + 1, |(next, _)| u64::from(*next));
            if delta < end - u64::from(offset) {
                return Some(self.first + u64::from(offset) + delta);
            }
        }
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Keyframe {
    pub tick: u64,
    pub offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Directory {
    pub version: u32,
    pub chunks: Vec<Chunk>,
    pub keyframes: Vec<Keyframe>,
}
impl Default for Directory {
    fn default() -> Self {
        Self {
            version: 1,
            chunks: Vec::new(),
            keyframes: Vec::new(),
        }
    }
}
impl Directory {
    pub fn validate(&self, footer: &Footer) -> Result<(), String> {
        self.validate_at(footer.cidx_offset)
    }

    /// Validate against the observed CIDX offset while the directory is in
    /// memory, even when its footer arrives in a later upload part.
    pub fn validate_at(&self, cidx_offset: u64) -> Result<(), String> {
        if self.version != 1 || self.chunks.len() + self.keyframes.len() > MAX_ROWS {
            return Err("unsupported film range directory".into());
        }
        let mut next_tick = Some(1);
        let mut end = DATA_OFFSET;
        let mut scene = None;
        let mut scene_index = 0;
        for (i, c) in self.chunks.iter().enumerate() {
            if c.scene == c.first {
                scene_index = i;
            }
            if Some(c.first) != next_tick
                || c.first > c.last
                || c.last - c.first >= 128
                || !c.valid_clock()
                || (c.scene != c.first && Some(c.scene) != scene)
                || c.scene > c.first
                || c.last - c.scene >= SCENE_WINDOW
                || i - scene_index >= MAX_SCENE_CHUNKS
                || c.offset < end
                || c.size == 0
                || c.size as usize > MAX_TICK_PACKED
                || c.raw_size == 0
                || c.raw_size as usize > MAX_TICK_BYTES
            {
                return Err("invalid film time chunk".into());
            }
            end = c
                .offset
                .checked_add(8 + u64::from(c.size))
                .ok_or("film range overflow")?;
            if end > cidx_offset {
                return Err("time chunk overlaps film directory".into());
            }
            next_tick = c.last.checked_add(1);
            scene = Some(c.scene);
        }
        let mut previous: Option<&Keyframe> = None;
        for k in &self.keyframes {
            if k.offset < DATA_OFFSET
                || k.offset >= cidx_offset
                || previous.is_some_and(|p| k.tick <= p.tick || k.offset <= p.offset)
            {
                return Err("invalid film keyframe reference".into());
            }
            previous = Some(k);
        }
        Ok(())
    }

    /// Smallest set of complete data chunks intersecting the inclusive time span.
    pub fn covering(&self, first: u64, last: u64) -> &[Chunk] {
        if first > last {
            return &[];
        }
        let a = self.chunks.partition_point(|c| c.last < first);
        let b = self.chunks.partition_point(|c| c.first <= last);
        self.chunks.get(a..b).unwrap_or(&[])
    }
    /// Earliest recorded occurrence of the guest tick, including clocks which
    /// have repeated after a campaign revert or while the simulation was paused.
    pub fn film_tick_for_guest(&self, tick: u64) -> Option<u64> {
        self.chunks
            .iter()
            .find_map(|chunk| chunk.locate_guest(tick))
    }

    pub fn nearest_keyframe(&self, tick: u64) -> Option<&Keyframe> {
        let end = self.keyframes.partition_point(|k| k.tick <= tick);
        end.checked_sub(1).map(|i| &self.keyframes[i])
    }
}
