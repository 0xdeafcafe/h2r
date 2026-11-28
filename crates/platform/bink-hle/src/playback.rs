//! Override bodies. Everything is `__stdcall`: arguments sit at `esp+4...`
//! and the callee pops them, so each body ends in `ret(cpu, 4 * nargs)`.
use crate::{MovieOpenError, Request, RequestOutcome, read_cstr};
use crate::audio_out::{MovieAudioOut, MovieVoice};
use crate::source::{AudioInfo, Clock, MovieSource, MovieStream, StreamInfo, SystemClock};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi, guest::Permissions};
use std::sync::{Arc, Mutex};

type R = Result<TransferAbi, GuestFault>;

const SLOTS: u32 = 8;
const SLOT_SIZE: u32 = 0x40;
const MAX_DIM: u32 = 4096;
/// Guest addresses reserved for HLE-owned storage. Nothing in this interval is
/// ever handed to the guest (not by MEM_RESERVE/commit, not by the loader), so
/// HLE crates can map pages here without taking an address the guest has
/// reserved but not yet committed. Shared policy: other HLE crates should take
/// their pages from here too, and must not collide with each other.
pub const HLE_RESERVED: std::ops::Range<u32> = 0xa000_0000..0xa100_0000;

/// Where in [`HLE_RESERVED`] the HBINK page search starts by default.
pub const DEFAULT_ARENA_HINT: u32 = HLE_RESERVED.start;

// BINK struct fields the engine reads (Bink 1.x public header).
const F_WIDTH: u32 = 0x00;
const F_HEIGHT: u32 = 0x04;
const F_FRAMES: u32 = 0x08;
const F_FRAME_NUM: u32 = 0x0c; // 1-based; equals Frames on the last frame
const F_LAST_FRAME: u32 = 0x10;
const F_RATE: u32 = 0x14;
const F_RATE_DIV: u32 = 0x18;

// BinkCopyToBuffer surface types (low byte of flags).
const SURFACE_32: u32 = 3;
const SURFACE_32R: u32 = 4;
const SURFACE_32A: u32 = 5;
const SURFACE_32RA: u32 = 6;

/// Sound queued ahead of the play position.
const AUDIO_LEAD_MS: u64 = 500;
/// Sound is handed over in packets this long.
const AUDIO_PACKET_MS: u64 = 10;

/// A movie's sound: the voice it plays on and how much has been fed.
struct Sound {
    voice: Box<dyn MovieVoice>,
    info: AudioInfo,
    /// Frames queued on the voice since open or the last loop.
    queued: u64,
    /// The stream has no more sound.
    ended: bool,
    pcm: Vec<i16>,
}

struct Slot {
    stream: Box<dyn MovieStream>,
    info: StreamInfo,
    frame: Vec<u8>,
    /// The 0-based frame `frame` holds, once one is decoded.
    decoded: Option<u32>,
    /// Preflight may decode a picture before the guest asks to show it.
    frame_visible: bool,
    /// Wall time (the [`Clock`]) at which the movie position was 0.
    anchor_ms: u64,
    sound: Option<Sound>,
    /// One destination row, reused by CopyToBuffer when it must convert.
    row: Vec<u8>,
    /// The decoded picture as the native display last took it: its frame
    /// and texels, shared until the next frame is decoded.
    shown: Option<(u32, Arc<[u8]>)>,
}

impl Slot {
    /// When frame `n` (1-based) is due, in ms of movie time.
    fn due_ms(&self, n: u32) -> u64 {
        n.saturating_sub(1) as u64 * 1000 * self.info.fps_den as u64 / self.info.fps_num as u64
    }
    /// The 1-based frame whose time has come at movie time `pos`.
    fn frame_at(&self, pos: u64) -> u64 {
        pos * self.info.fps_num as u64 / (1000 * self.info.fps_den as u64) + 1
    }
    /// Movie time now, on the wall clock from the open (or the last loop).
    ///
    /// Not the sound's play position: the mixer renders on the kernel's
    /// logical clock, which moves by executed guest work, and the wrapper's
    /// service spins on BinkWait (`0x155fc2..0x155fd0`) until a frame is
    /// due. A spin charges a few ns of logical time per call, so a clock
    /// that waits for the sound needs millions of calls per frame (the
    /// 0.3 fps of the first playback build). The sound plays at its own
    /// rate from the same start; in a paced run (`h2 play`) logical time
    /// keeps to the wall, and the two stay together.
    fn position_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.anchor_ms)
    }
    /// Keep [`AUDIO_LEAD_MS`] of sound queued ahead of what has played.
    fn feed(&mut self) {
        let Some(s) = self.sound.as_mut() else {
            return;
        };
        let rate = s.info.sample_rate as u64;
        let ch = s.info.channels as usize;
        let lead = rate * AUDIO_LEAD_MS / 1000;
        if s.ended || s.queued.saturating_sub(s.voice.played_frames()) >= lead {
            return;
        }
        let packet = (rate * AUDIO_PACKET_MS / 1000).max(1) as usize;
        s.pcm.resize(packet * ch, 0);
        let played = s.voice.played_frames();
        while !s.ended && s.queued.saturating_sub(played) < lead {
            let n = self.stream.read_audio(&mut s.pcm).min(packet);
            if n == 0 || !s.voice.queue(&s.pcm[..n * ch]) {
                s.ended = true;
                break;
            }
            s.queued += n as u64;
        }
    }
    /// A loop: movie time and the sound start over.
    fn restart(&mut self, now: u64) {
        self.anchor_ms = now;
        if let Some(s) = self.sound.as_mut() {
            s.voice.flush();
            s.ended = !self.stream.rewind_audio();
            s.queued = 0;
        }
        self.feed();
    }
}

struct Inner {
    slots: Vec<Option<Slot>>,
    requests: Vec<RequestOutcome>,
}

struct Movie {
    stream: Box<dyn MovieStream>,
    info: StreamInfo,
    first_frame: Option<Vec<u8>>,
}

pub struct BinkHle {
    source: Option<Arc<dyn MovieSource>>,
    /// One stream primed for the upcoming attract start. It has no guest
    /// handle or audio voice until BinkOpen consumes it.
    prepared: Mutex<Option<(String, Movie)>>,
    clock: Arc<dyn Clock>,
    /// Where movie sound plays; `None` plays movies silent.
    audio: Mutex<Option<Arc<dyn MovieAudioOut>>>,
    /// Where to start the search for a free page.
    arena_hint: u32,
    /// The page we mapped, once we have.
    arena: Mutex<Option<u32>>,
    inner: Mutex<Inner>,
}

impl BinkHle {
    /// `None` gives skip mode.
    pub fn new(source: Option<Arc<dyn MovieSource>>) -> Arc<Self> {
        Self::with_clock(source, Arc::new(SystemClock::default()))
    }
    pub fn with_clock(source: Option<Arc<dyn MovieSource>>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Self::build(source, clock, DEFAULT_ARENA_HINT)
    }
    /// Choose where the search for the HBINK page starts (page aligned). The
    /// search is confined to [`HLE_RESERVED`]: a hint outside it starts at the
    /// range start. The page used is the first fully unmapped one at or above
    /// the hint; mapped memory is never adopted.
    pub fn with_arena_hint(
        source: Option<Arc<dyn MovieSource>>,
        clock: Arc<dyn Clock>,
        hint: u32,
    ) -> Arc<Self> {
        Self::build(source, clock, hint)
    }
    /// The HBINK page, once the first movie has opened.
    pub fn arena(&self) -> Option<u32> {
        *self.arena.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn build(source: Option<Arc<dyn MovieSource>>, clock: Arc<dyn Clock>, hint: u32) -> Arc<Self> {
        Arc::new(Self {
            source,
            prepared: Mutex::new(None),
            clock,
            audio: Mutex::new(None),
            arena_hint: if HLE_RESERVED.contains(&hint) {
                hint & !0xfff
            } else {
                HLE_RESERVED.start
            },
            arena: Mutex::new(None),
            inner: Mutex::new(Inner {
                slots: (0..SLOTS).map(|_| None).collect(),
                requests: Vec::new(),
            }),
        })
    }
    /// Play movie sound here (the game's mixer, [`crate::MixerAudio`]).
    /// Movies opened before this call stay silent.
    pub fn set_audio_out(&self, out: Arc<dyn MovieAudioOut>) {
        *self.audio.lock().unwrap_or_else(|e| e.into_inner()) = Some(out);
    }
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }
    /// Every movie the engine asked for, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.lock()
            .requests
            .iter()
            .map(|r| r.request.clone())
            .collect()
    }
    /// The latest Bink result for a path or bare movie name.
    pub fn last_request(&self, path: &str) -> Option<RequestOutcome> {
        let name = movie_name(path);
        self.lock()
            .requests
            .iter()
            .rev()
            .find(|r| movie_name(&r.request.path) == name)
            .cloned()
    }
    /// Validate and decode the first frame for an upcoming start. No guest
    /// state or audio changes until BinkOpen consumes the prepared movie.
    /// The name can be a guest path or a bare stem, matched case-insensitively.
    pub fn prepare(&self, path: &str) -> Result<(), MovieOpenError> {
        let name = movie_name(path);
        let mut prepared = self.prepared.lock().unwrap_or_else(|e| e.into_inner());
        if prepared.as_ref().is_some_and(|(key, _)| key == &name) {
            return Ok(());
        }
        let mut movie = self.load_movie(&name)?;
        let mut pixels = vec![0; frame_size(movie.info)];
        if !movie.stream.decode(0, &mut pixels) {
            return Err(MovieOpenError::FirstFrame);
        }
        movie.first_frame = Some(pixels);
        *prepared = Some((name, movie));
        Ok(())
    }
    /// Release a prepared movie when the wrapper's pending start is canceled.
    pub fn discard_prepared(&self, path: &str) {
        let _ = self.take_prepared(&movie_name(path));
    }
    fn take_prepared(&self, name: &str) -> Option<Movie> {
        let mut prepared = self.prepared.lock().unwrap_or_else(|e| e.into_inner());
        if prepared.as_ref().is_some_and(|(key, _)| key.as_str() == name) {
            return prepared.take().map(|(_, movie)| movie);
        }
        None
    }
    pub fn open_streams(&self) -> usize {
        self.lock().slots.iter().flatten().count()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn record_skip(&self, mem: &runtime::Guest, path_ptr: u32, flags: u32) {
        self.record_request(
            read_cstr(mem, path_ptr),
            flags,
            Some(MovieOpenError::NoSource),
        );
    }

    fn record_request(&self, path: String, flags: u32, failure: Option<MovieOpenError>) {
        let mut inner = self.lock();
        let sequence = inner.requests.len() + 1;
        inner.requests.push(RequestOutcome {
            sequence,
            request: Request {
                path,
                flags,
                played: failure.is_none(),
            },
            failure,
        });
    }

    fn load_movie(&self, name: &str) -> Result<Movie, MovieOpenError> {
        let stream = self
            .source
            .as_ref()
            .ok_or(MovieOpenError::NoSource)?
            .try_open(name)?;
        let info = stream.info();
        if !(1..=MAX_DIM).contains(&info.width)
            || !(1..=MAX_DIM).contains(&info.height)
            || info.frames == 0
            || info.fps_num == 0
            || info.fps_den == 0
        {
            return Err(MovieOpenError::InvalidMetadata);
        }
        Ok(Movie {
            stream,
            info,
            first_frame: None,
        })
    }

    // --- Bink API ---------------------------------------------------------

    /// `BinkSetSoundSystem(open_fn, ds)`: accept, return 1. Audio is routed by
    /// the host, not by Bink's DirectSound back end.
    pub(crate) fn set_sound_system(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        cpu.eax = 1;
        ctx.memory().ret(cpu, 8)
    }
    pub(crate) fn set_memory(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        // We never allocate through the game's callbacks.
        cpu.eax = 0;
        ctx.memory().ret(cpu, 8)
    }
    pub(crate) fn set_mix_bins(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        cpu.eax = 1;
        ctx.memory().ret(cpu, 16)
    }
    pub(crate) fn set_mix_bin_volumes(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        cpu.eax = 1;
        ctx.memory().ret(cpu, 20)
    }

    /// `BinkOpen(path, flags) -> HBINK or 0`.
    pub(crate) fn open(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mut mem = ctx.memory();
        let path_ptr = mem.u32(cpu.esp.wrapping_add(4))?;
        let flags = mem.u32(cpu.esp.wrapping_add(8))?;
        let path = read_cstr(&mem, path_ptr);
        let name = movie_name(&path);
        let movie = self
            .take_prepared(&name)
            .map_or_else(|| self.load_movie(&name), Ok);
        let result = match movie {
            Ok(movie) => self.open_handle(&mut mem, movie)?,
            Err(why) => Err(why),
        };
        let (handle, failure) = match result {
            Ok(handle) => (handle, None),
            Err(why) => (0, Some(why)),
        };
        self.record_request(path, flags, failure);
        cpu.eax = handle;
        mem.ret(cpu, 8)
    }

    fn open_handle(
        &self,
        mem: &mut runtime::Guest,
        movie: Movie,
    ) -> Result<Result<u32, MovieOpenError>, GuestFault> {
        let mut inner = self.lock();
        let Some(i) = inner.slots.iter().position(Option::is_none) else {
            return Ok(Err(MovieOpenError::NoHandle));
        };
        let Some(arena) = self.ensure_arena(mem) else {
            return Ok(Err(MovieOpenError::NoArena));
        };
        let Movie {
            stream,
            info,
            first_frame,
        } = movie;
        let h = arena + i as u32 * SLOT_SIZE;
        mem.set_u32(h + F_WIDTH, info.width)?;
        mem.set_u32(h + F_HEIGHT, info.height)?;
        mem.set_u32(h + F_FRAMES, info.frames)?;
        mem.set_u32(h + F_FRAME_NUM, 1)?;
        mem.set_u32(h + F_LAST_FRAME, 0)?;
        mem.set_u32(h + F_RATE, info.fps_num)?;
        mem.set_u32(h + F_RATE_DIV, info.fps_den)?;
        let now = self.clock.now_ms();
        let sound = self.open_sound(&*stream);
        let mut slot = Slot {
            stream,
            info,
            decoded: first_frame.as_ref().map(|_| 0),
            frame: first_frame.unwrap_or_else(|| vec![0; frame_size(info)]),
            frame_visible: false,
            shown: None,
            anchor_ms: now,
            sound,
            row: Vec::new(),
        };
        slot.feed();
        inner.slots[i] = Some(slot);
        Ok(Ok(h))
    }

    fn open_sound(&self, stream: &dyn MovieStream) -> Option<Sound> {
        let info = stream
            .audio_info()
            .filter(|a| a.sample_rate > 0 && (1..=8).contains(&a.channels))?;
        let out = self
            .audio
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        Some(Sound {
            voice: out.open(info)?,
            info,
            queued: 0,
            ended: false,
            pcm: Vec::new(),
        })
    }

    pub(crate) fn close(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mut mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        if let Some(i) = self.slot_index(h) {
            self.lock().slots[i] = None;
            for off in (0..SLOT_SIZE).step_by(4) {
                mem.set_u32(h + off, 0)?;
            }
        }
        cpu.eax = 0;
        mem.ret(cpu, 4)
    }

    /// `BinkWait(bnk)`: nonzero while the current frame (`FrameNum`) is not
    /// yet due on the movie's clock. The wrapper's service calls this in a
    /// tight loop until it returns 0, so it only reads the clock: no sound
    /// feeding, no mixer lock (DoFrame and NextFrame feed, once a frame).
    pub(crate) fn wait(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        let now = self.clock.now_ms();
        cpu.eax = 0;
        if let Some(i) = self.slot_index(h) {
            let n = mem.u32(h + F_FRAME_NUM)?;
            let inner = self.lock();
            if let Some(s) = inner.slots[i].as_ref()
                && s.position_ms(now) < s.due_ms(n)
            {
                cpu.eax = 1;
            }
        }
        mem.ret(cpu, 4)
    }

    /// `BinkDoFrame(bnk)`: decode the current frame into the host buffer.
    pub(crate) fn do_frame(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        self.do_frame_at(&mem, h)?;
        cpu.eax = 0;
        mem.ret(cpu, 4)
    }

    /// `BinkDoFrame`'s body for handle `h`.
    pub(crate) fn do_frame_at(&self, mem: &runtime::Guest, h: u32) -> Result<(), GuestFault> {
        if let Some(i) = self.slot_index(h) {
            let n = mem.u32(h + F_FRAME_NUM)?;
            let mut inner = self.lock();
            if let Some(s) = inner.slots[i].as_mut() {
                let idx = n.saturating_sub(1).min(s.info.frames - 1);
                // The picture already in the buffer is not decoded again: a
                // platform decoder may only go forward cheaply (AVFoundation
                // restarts its reader for anything earlier).
                if s.decoded == Some(idx) || s.stream.decode(idx, &mut s.frame) {
                    s.decoded = Some(idx);
                    s.frame_visible = true;
                }
                s.feed();
            }
        }
        Ok(())
    }

    /// `BinkNextFrame(bnk)`: advance one frame, or, when the clock has run
    /// more than a frame ahead, to the frame now due (frames are dropped to
    /// keep time, as Bink does), never past the last frame.
    pub(crate) fn next_frame(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mut mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        self.next_frame_at(&mut mem, h)?;
        cpu.eax = 0;
        mem.ret(cpu, 4)
    }

    /// `BinkNextFrame`'s body for handle `h`.
    pub(crate) fn next_frame_at(&self, mem: &mut runtime::Guest, h: u32) -> Result<(), GuestFault> {
        if let Some(i) = self.slot_index(h) {
            let n = mem.u32(h + F_FRAME_NUM)?;
            let mut inner = self.lock();
            if let Some(s) = inner.slots[i].as_mut() {
                mem.set_u32(h + F_LAST_FRAME, n)?;
                let now = self.clock.now_ms();
                if n >= s.info.frames {
                    // Like the original (0x3e2fb9..0x3e2fc8): past the last
                    // frame NextFrame wraps to frame 1, which is what makes
                    // loops loop. Non-looping callers stop on
                    // `FrameNum == Frames` first.
                    mem.set_u32(h + F_FRAME_NUM, 1)?;
                    s.restart(now);
                } else {
                    let pos = s.position_ms(now);
                    let due = s.frame_at(pos);
                    let next = (n as u64 + 1).max(due).min(s.info.frames as u64);
                    mem.set_u32(h + F_FRAME_NUM, next as u32)?;
                    s.feed();
                }
            }
        }
        Ok(())
    }

    /// The picture movie `h` shows: what `BinkCopyToBuffer` would copy,
    /// once a frame is decoded for showing. Shared until the next frame.
    pub fn picture(&self, h: u32) -> Option<crate::display::MoviePicture> {
        let i = self.slot_index(h)?;
        let mut inner = self.lock();
        let s = inner.slots[i].as_mut().filter(|s| s.frame_visible)?;
        let frame = s.decoded?;
        let (w, hh) = (s.info.width, s.info.height);
        let bytes = (w * hh * 4) as usize;
        if s.frame.len() < bytes {
            return None;
        }
        let bgra = match &s.shown {
            Some((f, b)) if *f == frame => Arc::clone(b),
            _ => {
                let b: Arc<[u8]> = s.frame[..bytes].into();
                s.shown = Some((frame, Arc::clone(&b)));
                b
            }
        };
        Some(crate::display::MoviePicture { movie: h, frame, width: w, height: hh, bgra })
    }

    /// `BinkCopyToBuffer(bnk, dst, pitch, height, x, y, flags)`. Returns 0
    /// when pixels were written, 1 when not.
    pub(crate) fn copy_to_buffer(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mut mem = ctx.memory();
        let a = |mem: &runtime::Guest, n: u32| mem.u32(cpu.esp.wrapping_add(4 + 4 * n));
        let (h, dst, pitch, height) = (a(&mem, 0)?, a(&mem, 1)?, a(&mem, 2)?, a(&mem, 3)?);
        let (x, y, flags) = (a(&mem, 4)?, a(&mem, 5)?, a(&mem, 6)?);
        let mut result = 1;
        let surface = flags & 0xff;
        let order = match surface {
            SURFACE_32 | SURFACE_32A => Some(false),
            SURFACE_32R | SURFACE_32RA => Some(true),
            _ => None,
        };
        if let (Some(swap), Some(i)) = (order, self.slot_index(h)) {
            let mut inner = self.lock();
            if let Some(s) = inner.slots[i].as_mut().filter(|s| s.frame_visible) {
                let (w, hh) = (s.info.width as usize, s.info.height as usize);
                let rows = hh.min((height as usize).saturating_sub(y as usize));
                let cols = w.min((pitch as usize / 4).saturating_sub(x as usize));
                let row_at = |r: usize| {
                    dst.wrapping_add((y + r as u32).wrapping_mul(pitch))
                        .wrapping_add(x * 4)
                };
                // Bulk writes through Guest::write_bytes (one memcpy and one
                // write-watch note each): the whole picture at once when the
                // destination rows are the source rows, else one per row,
                // converted in a reused row buffer when red and blue swap.
                let ok = if !swap && cols == w && pitch as usize == w * 4 {
                    mem.write_bytes(row_at(0), &s.frame[..rows * w * 4], 1)
                        .is_ok()
                } else {
                    let Slot { frame, row, .. } = s;
                    row.resize(cols * 4, 0);
                    (0..rows).all(|r| {
                        let src = &frame[r * w * 4..r * w * 4 + cols * 4];
                        let line = if swap {
                            row.copy_from_slice(src);
                            for px in row.as_chunks_mut::<4>().0 {
                                px.swap(0, 2);
                            }
                            &row[..]
                        } else {
                            src
                        };
                        mem.write_bytes(row_at(r), line, 1).is_ok()
                    })
                };
                if ok {
                    result = 0;
                }
            }
        }
        cpu.eax = result;
        mem.ret(cpu, 28)
    }

    fn slot_index(&self, h: u32) -> Option<usize> {
        let off = h.checked_sub(self.arena()?)?;
        (off % SLOT_SIZE == 0 && off / SLOT_SIZE < SLOTS).then_some((off / SLOT_SIZE) as usize)
    }

    /// The page for HBINK structs: ours or nothing. The first call scans
    /// [`HLE_RESERVED`] for an unmapped page at or above the hint and maps it.
    fn ensure_arena(&self, mem: &mut runtime::Guest) -> Option<u32> {
        let mut slot = self.arena.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return *slot;
        }
        let mut va = self.arena_hint;
        loop {
            let end = va.checked_add(0x1000)?;
            if end > HLE_RESERVED.end {
                return None;
            }
            // Skip past any mapping that overlaps this page.
            if let Some(m) = mem.mappings().iter().find(|m| {
                (va as u64) < m.virtual_start as u64 + m.len as u64
                    && (end as u64) > m.virtual_start as u64
            }) {
                va = m.virtual_start.checked_add(m.len)?.checked_add(0xfff)? & !0xfff;
                continue;
            }
            // map_zeroed also refuses reserved device ranges; step past those.
            match mem.map_zeroed(va, 0x1000, Permissions::RW) {
                Ok(()) => {
                    *slot = Some(va);
                    return *slot;
                }
                Err(runtime::guest::MapError::InvalidRange) => va = end,
                Err(_) => return None,
            }
        }
    }
}

fn frame_size(info: StreamInfo) -> usize {
    info.width as usize * info.height as usize * 4
}

/// `d:\bink\intro_60.bik` -> `intro_60`.
pub(crate) fn movie_name(path: &str) -> String {
    let file = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    file.strip_suffix(".bik").map(str::to_owned).unwrap_or(file)
}
