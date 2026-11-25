//! `render.picture`: the game's presented frames, live, for a client to
//! show (docs/runtime/agent-api.md, "Picture feed"; docs/editor/game-view.md).
//!
//! Two transports share one set of params and one per-frame stamp
//! ([`PictureEvent`]):
//!
//! - **ring**: a memory-mapped file of [`SLOTS`] RGBA slots under the cache
//!   directory ([`Ring`]). The game writes each frame into the slot after
//!   the newest, guarded by a per-slot sequence number, and the reader
//!   copies the newest slot whose sequence is even and unchanged across
//!   the copy. The reader writes back the newest frame it took
//!   ([`Ring::read_newest`]); a ring left unread for a second stops the
//!   game's readbacks. The connection that opened it gets a `picture`
//!   event per frame.
//! - **stream**: for a game reached over `tcp://`. The opening connection
//!   switches to length-prefixed binary frames after the reply line, as
//!   `live.subscribe` does: each is a [`PictureEvent`] and a JPEG
//!   ([`read_stream_frame`]).
//!
//! The frame source is a hub ([`hub`]) the window shell attaches its
//! readback to, as [`crate::host_shot`] does, so the shell links this
//! module without the agent.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

pub const OPEN: &str = "render.picture.open";
pub const CLOSE: &str = "render.picture.close";
pub const METHODS: &[&str] = &[OPEN, CLOSE];

/// Ring defaults: the asked rate and width when none is given.
pub const RING_FPS: u32 = 60;
pub const RING_WIDTH: u32 = 1280;
/// Stream defaults (docs/editor/game-view.md, "Frames").
pub const STREAM_FPS: u32 = 15;
pub const STREAM_WIDTH: u32 = 640;
/// Accepted `width` and `fps`.
pub const MIN_WIDTH: u32 = 16;
pub const MAX_WIDTH: u32 = 2560;
pub const MAX_FPS: u32 = 60;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A shared-memory ring: same machine only.
    #[default]
    Ring,
    /// JPEG frames on this connection, after the reply.
    Stream,
}

/// `render.picture.open`'s params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// The widest frame wanted, in pixels; a frame keeps its aspect and is
    /// never scaled up. The ring's slots hold `width` x `width`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Frames a second at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
}

impl OpenParams {
    /// The width, rate and transport asked for, defaults filled in, or why
    /// they are refused.
    pub fn resolve(&self) -> Result<(u32, u32, Transport), String> {
        let transport = self.transport.unwrap_or_default();
        let (width, fps) = match transport {
            Transport::Ring => (RING_WIDTH, RING_FPS),
            Transport::Stream => (STREAM_WIDTH, STREAM_FPS),
        };
        let width = self.width.unwrap_or(width);
        let fps = self.fps.unwrap_or(fps);
        if !(MIN_WIDTH..=MAX_WIDTH).contains(&width) {
            return Err(format!("width must be {MIN_WIDTH} to {MAX_WIDTH}"));
        }
        if !(1..=MAX_FPS).contains(&fps) {
            return Err(format!("fps must be 1 to {MAX_FPS}"));
        }
        Ok((width, fps, transport))
    }
}

/// `render.picture.open`'s result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    /// The picture's id, for `render.picture.close` and its events.
    pub id: u64,
    pub transport: Transport,
    /// The ring's file ([`Ring::open`]); none for a stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The largest frame: a ring slot's capacity, `[width, width]`. Each
    /// frame's own size is in its slot and its event.
    pub size: [u32; 2],
    /// `rgba8` (ring) or `jpeg` (stream).
    pub format: String,
    pub fps: u32,
}

/// `render.picture.close`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseParams {
    pub id: u64,
}

/// One published frame: the `picture` event of a ring, the header of a
/// stream frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureEvent {
    /// The picture ([`Opened::id`]).
    pub id: u64,
    /// This picture's frame number, from 1.
    pub frame: u64,
    /// The game's tick when the frame was taken (none before a game state
    /// is readable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// Frames the game had presented (`at.present`).
    pub present: u64,
    /// Milliseconds since the picture opened.
    pub time_ms: f64,
    pub width: u32,
    pub height: u32,
}

// ---- the ring ----

/// Frames the ring holds.
pub const SLOTS: usize = 3;
const MAGIC: [u8; 8] = *b"H2PICT\0\x01";
const VERSION: u32 = 1;
/// `rgba8`: 4 bytes a pixel, rows top first, tightly packed.
pub const FORMAT_RGBA8: u32 = 1;

// The header, all little-endian and 8-byte aligned.
const AT_MAGIC: usize = 0;
const AT_VERSION: usize = 8;
const AT_SLOTS: usize = 12;
const AT_CAP_W: usize = 16;
const AT_CAP_H: usize = 20;
const AT_FORMAT: usize = 24;
const AT_SLOT_BYTES: usize = 32;
/// The newest frame number published (0: none yet). Slot `n % SLOTS`.
const AT_NEWEST: usize = 40;
/// The newest frame number the reader took: the reader writes it.
const AT_READ: usize = 48;
/// Non-zero once the game closed the picture.
const AT_CLOSED: usize = 56;
const AT_META: usize = 64;
const META_BYTES: usize = 48;
// Within a slot's meta.
const M_SEQ: usize = 0;
const M_FRAME: usize = 8;
const M_TICK: usize = 16;
const M_PRESENT: usize = 24;
const M_TIME_US: usize = 32;
/// Width in the low half, height in the high half.
const M_SIZE: usize = 40;
/// Where slot 0's pixels start.
pub const DATA_AT: usize = 256;
const NO_TICK: u64 = u64::MAX;

const _: () = assert!(AT_META + SLOTS * META_BYTES <= DATA_AT);

/// What a slot holds besides its pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotMeta {
    pub frame: u64,
    pub tick: Option<u64>,
    pub present: u64,
    pub time_us: u64,
    pub width: u32,
    pub height: u32,
}

/// A picture's ring, mapped: the game creates it ([`Ring::create`]), a
/// client maps it ([`Ring::open`]). Both may be in one process (the
/// tests); each has its own mapping.
pub struct Ring {
    _map: memmap2::MmapMut,
    ptr: *mut u8,
    len: usize,
    cap: (u32, u32),
    slot_bytes: usize,
}

// SAFETY: the mapping lives as long as the Ring; every shared field is
// accessed through atomics, and pixel copies are guarded by the slot's
// sequence number.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Creates a new ring file at `path` (refusing an existing one), mode
    /// 0600, holding [`SLOTS`] frames of up to `cap` pixels.
    pub fn create(path: &Path, cap: (u32, u32)) -> io::Result<Ring> {
        let slot_bytes = cap.0 as usize * cap.1 as usize * 4;
        let len = DATA_AT + SLOTS * slot_bytes;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        file.set_len(len as u64)?;
        // SAFETY: a file we just created; nobody else has written it.
        let mut map = unsafe { memmap2::MmapMut::map_mut(&file) }?;
        map[AT_MAGIC..AT_MAGIC + 8].copy_from_slice(&MAGIC);
        map[AT_VERSION..AT_VERSION + 4].copy_from_slice(&VERSION.to_le_bytes());
        map[AT_SLOTS..AT_SLOTS + 4].copy_from_slice(&(SLOTS as u32).to_le_bytes());
        map[AT_CAP_W..AT_CAP_W + 4].copy_from_slice(&cap.0.to_le_bytes());
        map[AT_CAP_H..AT_CAP_H + 4].copy_from_slice(&cap.1.to_le_bytes());
        map[AT_FORMAT..AT_FORMAT + 4].copy_from_slice(&FORMAT_RGBA8.to_le_bytes());
        map[AT_SLOT_BYTES..AT_SLOT_BYTES + 8].copy_from_slice(&(slot_bytes as u64).to_le_bytes());
        let ptr = map.as_mut_ptr();
        Ok(Ring {
            _map: map,
            ptr,
            len,
            cap,
            slot_bytes,
        })
    }

    /// Maps an existing ring to read it.
    pub fn open(path: &Path) -> io::Result<Ring> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        // SAFETY: the game resizes the file only before it answers with
        // its path; shared fields are atomics.
        let mut map = unsafe { memmap2::MmapMut::map_mut(&file) }?;
        let bad = |why: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("not a picture ring: {why}"),
            )
        };
        if map.len() < DATA_AT || map[AT_MAGIC..AT_MAGIC + 8] != MAGIC {
            return Err(bad("no header"));
        }
        let u32_at = |at: usize| u32::from_le_bytes(map[at..at + 4].try_into().expect("4 bytes"));
        if u32_at(AT_VERSION) != VERSION
            || u32_at(AT_SLOTS) as usize != SLOTS
            || u32_at(AT_FORMAT) != FORMAT_RGBA8
        {
            return Err(bad("version, slots or format"));
        }
        let cap = (u32_at(AT_CAP_W), u32_at(AT_CAP_H));
        let slot_bytes = u64::from_le_bytes(
            map[AT_SLOT_BYTES..AT_SLOT_BYTES + 8]
                .try_into()
                .expect("8 bytes"),
        ) as usize;
        if slot_bytes != cap.0 as usize * cap.1 as usize * 4
            || map.len() < DATA_AT + SLOTS * slot_bytes
        {
            return Err(bad("size"));
        }
        let len = map.len();
        let ptr = map.as_mut_ptr();
        Ok(Ring {
            _map: map,
            ptr,
            len,
            cap,
            slot_bytes,
        })
    }

    /// The largest frame a slot holds.
    pub fn capacity(&self) -> (u32, u32) {
        self.cap
    }

    fn word(&self, at: usize) -> &AtomicU64 {
        assert!(at.is_multiple_of(8) && at + 8 <= DATA_AT);
        // SAFETY: in bounds, 8-aligned (the mapping is page aligned), and
        // only ever accessed atomically.
        unsafe { &*(self.ptr.add(at) as *const AtomicU64) }
    }

    fn meta(&self, slot: usize, field: usize) -> &AtomicU64 {
        self.word(AT_META + slot * META_BYTES + field)
    }

    fn data(&self, slot: usize) -> *mut u8 {
        let at = DATA_AT + slot * self.slot_bytes;
        assert!(at + self.slot_bytes <= self.len);
        // SAFETY: in bounds (checked above).
        unsafe { self.ptr.add(at) }
    }

    // ---- the game's side ----

    /// Writes frame `meta.frame` (one more than the newest) into its slot
    /// and publishes it. `rgba` is `meta.width` x `meta.height`, within the
    /// capacity.
    pub fn publish(&self, meta: SlotMeta, rgba: &[u8]) {
        assert!(meta.width <= self.cap.0 && meta.height <= self.cap.1);
        assert_eq!(rgba.len(), meta.width as usize * meta.height as usize * 4);
        let slot = (meta.frame % SLOTS as u64) as usize;
        let seq = self.meta(slot, M_SEQ);
        let s0 = seq.load(Ordering::Relaxed);
        // Odd while the slot is being written.
        seq.store(s0.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        // SAFETY: within the slot; a reader racing this copy sees the odd
        // or a changed sequence and throws its copy away.
        unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), self.data(slot), rgba.len()) };
        self.meta(slot, M_FRAME)
            .store(meta.frame, Ordering::Relaxed);
        self.meta(slot, M_TICK)
            .store(meta.tick.unwrap_or(NO_TICK), Ordering::Relaxed);
        self.meta(slot, M_PRESENT)
            .store(meta.present, Ordering::Relaxed);
        self.meta(slot, M_TIME_US)
            .store(meta.time_us, Ordering::Relaxed);
        self.meta(slot, M_SIZE).store(
            u64::from(meta.width) | u64::from(meta.height) << 32,
            Ordering::Relaxed,
        );
        seq.store(s0.wrapping_add(2), Ordering::Release);
        self.word(AT_NEWEST).store(meta.frame, Ordering::Release);
    }

    /// The newest frame number the reader says it took.
    pub fn read_ack(&self) -> u64 {
        self.word(AT_READ).load(Ordering::Acquire)
    }

    /// The game closed the picture: a reader stops.
    pub fn mark_closed(&self) {
        self.word(AT_CLOSED).store(1, Ordering::Release);
    }

    // ---- the reader's side ----

    /// The newest frame number published (0: none yet).
    pub fn newest(&self) -> u64 {
        self.word(AT_NEWEST).load(Ordering::Acquire)
    }

    /// Whether the game closed this picture.
    pub fn closed(&self) -> bool {
        self.word(AT_CLOSED).load(Ordering::Acquire) != 0
    }

    /// Copies the newest whole frame into `out` (resized to fit) and tells
    /// the game it was taken. `None` before the first frame, or when the
    /// writer kept overwriting the slot for every try.
    pub fn read_newest(&self, out: &mut Vec<u8>) -> Option<SlotMeta> {
        for _ in 0..16 {
            let newest = self.newest();
            if newest == 0 {
                return None;
            }
            let slot = (newest % SLOTS as u64) as usize;
            let seq = self.meta(slot, M_SEQ);
            let s1 = seq.load(Ordering::Acquire);
            if !s1.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let size = self.meta(slot, M_SIZE).load(Ordering::Relaxed);
            let (width, height) = (size as u32, (size >> 32) as u32);
            let tick = self.meta(slot, M_TICK).load(Ordering::Relaxed);
            let meta = SlotMeta {
                frame: self.meta(slot, M_FRAME).load(Ordering::Relaxed),
                tick: (tick != NO_TICK).then_some(tick),
                present: self.meta(slot, M_PRESENT).load(Ordering::Relaxed),
                time_us: self.meta(slot, M_TIME_US).load(Ordering::Relaxed),
                width,
                height,
            };
            let bytes = width as usize * height as usize * 4;
            if width > self.cap.0 || height > self.cap.1 {
                continue;
            }
            out.resize(bytes, 0);
            // SAFETY: within the slot. The copy may race the writer; the
            // sequence check below throws a torn copy away.
            unsafe { std::ptr::copy_nonoverlapping(self.data(slot), out.as_mut_ptr(), bytes) };
            fence(Ordering::Acquire);
            if seq.load(Ordering::Relaxed) != s1 || meta.frame != newest {
                continue;
            }
            self.word(AT_READ).fetch_max(meta.frame, Ordering::AcqRel);
            return Some(meta);
        }
        None
    }
}

// ---- the stream ----

/// A stream frame's payload is capped at this.
pub const STREAM_MAX: usize = 16 << 20;

/// One frame off the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamFrame {
    pub event: PictureEvent,
    pub jpeg: Vec<u8>,
}

/// A stream frame's bytes: a little-endian u32 payload length, then the
/// payload: a u32 header length, the [`PictureEvent`] as JSON, the JPEG.
pub fn encode_stream_frame(event: &PictureEvent, jpeg: &[u8]) -> Vec<u8> {
    let header = serde_json::to_vec(event).expect("picture events serialise");
    let payload = 4 + header.len() + jpeg.len();
    let mut out = Vec::with_capacity(4 + payload);
    out.extend_from_slice(&(payload as u32).to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(jpeg);
    out
}

/// Reads one stream frame (blocking).
pub fn read_stream_frame(r: &mut impl Read) -> io::Result<StreamFrame> {
    let bad = |why: String| io::Error::new(io::ErrorKind::InvalidData, why);
    let mut word = [0u8; 4];
    r.read_exact(&mut word)?;
    let payload = u32::from_le_bytes(word) as usize;
    if !(4..=STREAM_MAX).contains(&payload) {
        return Err(bad(format!("picture frame of {payload} bytes")));
    }
    let mut bytes = vec![0u8; payload];
    r.read_exact(&mut bytes)?;
    let header = u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
    if header > payload - 4 {
        return Err(bad(format!(
            "picture header of {header} bytes in {payload}"
        )));
    }
    let event = serde_json::from_slice(&bytes[4..4 + header]).map_err(|e| bad(e.to_string()))?;
    let jpeg = bytes.split_off(4 + header);
    Ok(StreamFrame { event, jpeg })
}

/// Writes one stream frame.
pub fn write_stream_frame(w: &mut impl Write, event: &PictureEvent, jpeg: &[u8]) -> io::Result<()> {
    w.write_all(&encode_stream_frame(event, jpeg))
}

// ---- the frame source ----

/// A presented frame read back: tightly packed RGBA8, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grabbed {
    /// The frame's number in its source: a grab asked with this number
    /// as `after` answers `None` until a newer frame is presented.
    pub seq: u64,
    pub size: (u32, u32),
    pub rgba: Vec<u8>,
}

/// The shell's readback: the newest presented frame unless it is `after`.
pub type Grab = Box<dyn Fn(Option<u64>) -> Result<Option<Grabbed>, String> + Send + Sync>;

/// Where the window shell (or a headless host) and `render.picture` meet.
/// [`hub`] is the process's; tests make their own.
#[derive(Default)]
pub struct Hub {
    grab: Mutex<Option<Arc<Grab>>>,
}

static HUB: Hub = Hub::new();

/// The process's hub.
pub fn hub() -> &'static Hub {
    &HUB
}

/// Without a frame source there is nothing to show.
pub const NO_SOURCE: &str =
    "this game publishes no frames (no window and no headless GPU view): no picture to open";

impl Hub {
    pub const fn new() -> Hub {
        Hub {
            grab: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Arc<Grab>>> {
        self.grab.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A frame source is here.
    pub fn attach(&self, grab: Grab) {
        *self.lock() = Some(Arc::new(grab));
    }

    /// The frame source is gone.
    pub fn detach(&self) {
        *self.lock() = None;
    }

    /// Whether a source is attached.
    pub fn attached(&self) -> bool {
        self.lock().is_some()
    }

    /// The newest frame unless it is `after`; the readback runs on the
    /// calling thread, outside the hub's lock.
    pub fn grab(&self, after: Option<u64>) -> Result<Option<Grabbed>, String> {
        let grab = self.lock().clone().ok_or(NO_SOURCE)?;
        grab(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("h2-picture-proto-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_and_limits() {
        assert_eq!(
            OpenParams::default().resolve().unwrap(),
            (RING_WIDTH, RING_FPS, Transport::Ring)
        );
        let stream = OpenParams {
            transport: Some(Transport::Stream),
            ..Default::default()
        };
        assert_eq!(
            stream.resolve().unwrap(),
            (STREAM_WIDTH, STREAM_FPS, Transport::Stream)
        );
        for bad in [
            OpenParams {
                width: Some(8),
                ..Default::default()
            },
            OpenParams {
                width: Some(MAX_WIDTH + 1),
                ..Default::default()
            },
            OpenParams {
                fps: Some(0),
                ..Default::default()
            },
            OpenParams {
                fps: Some(61),
                ..Default::default()
            },
        ] {
            assert!(bad.resolve().is_err(), "{bad:?}");
        }
        let p: OpenParams = serde_json::from_str(r#"{"width":320,"transport":"stream"}"#).unwrap();
        assert_eq!(p.resolve().unwrap(), (320, STREAM_FPS, Transport::Stream));
        assert!(serde_json::from_str::<OpenParams>(r#"{"size":1}"#).is_err());
    }

    #[test]
    fn a_ring_round_trips_a_frame_and_takes_the_ack() {
        let dir = scratch("roundtrip");
        let path = dir.join("r.rgba");
        let ring = Ring::create(&path, (4, 4)).unwrap();
        let reader = Ring::open(&path).unwrap();
        let mut out = Vec::new();
        assert_eq!(reader.read_newest(&mut out), None);
        let meta = SlotMeta {
            frame: 1,
            tick: Some(7),
            present: 9,
            time_us: 10,
            width: 4,
            height: 2,
        };
        ring.publish(meta, &[5u8; 32]);
        assert_eq!(reader.read_newest(&mut out), Some(meta));
        assert_eq!(out, vec![5u8; 32]);
        assert_eq!(ring.read_ack(), 1);
        ring.mark_closed();
        assert!(reader.closed());
        assert!(
            Ring::create(&path, (4, 4)).is_err(),
            "an existing file is refused"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop((ring, reader));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stream_frames_round_trip() {
        let event = PictureEvent {
            id: 3,
            frame: 2,
            tick: None,
            present: 40,
            time_ms: 66.5,
            width: 2,
            height: 1,
        };
        let mut bytes = Vec::new();
        write_stream_frame(&mut bytes, &event, b"jpeg").unwrap();
        write_stream_frame(&mut bytes, &event, b"").unwrap();
        let mut r = bytes.as_slice();
        assert_eq!(
            read_stream_frame(&mut r).unwrap(),
            StreamFrame {
                event: event.clone(),
                jpeg: b"jpeg".to_vec()
            }
        );
        assert_eq!(read_stream_frame(&mut r).unwrap().jpeg, b"");
        assert!(read_stream_frame(&mut r).is_err());
        let huge = ((STREAM_MAX + 1) as u32).to_le_bytes();
        assert!(read_stream_frame(&mut huge.as_slice()).is_err());
    }

    #[test]
    fn the_hub_needs_a_source() {
        let hub = Hub::new();
        assert_eq!(hub.grab(None).unwrap_err(), NO_SOURCE);
        hub.attach(Box::new(|after| {
            Ok((after != Some(1)).then(|| Grabbed {
                seq: 1,
                size: (1, 1),
                rgba: vec![0; 4],
            }))
        }));
        assert!(hub.attached());
        assert_eq!(hub.grab(None).unwrap().unwrap().seq, 1);
        assert_eq!(hub.grab(Some(1)).unwrap(), None);
        hub.detach();
        assert!(!hub.attached());
    }
}
