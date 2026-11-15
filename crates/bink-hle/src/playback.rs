//! Override bodies. Everything is `__stdcall`: arguments sit at `esp+4...`
//! and the callee pops them, so each body ends in `ret(cpu, 4 * nargs)`.
use crate::Request;
use crate::addr::*;
use crate::source::{Clock, MovieSource, MovieStream, StreamInfo, SystemClock};
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

struct Slot {
    stream: Box<dyn MovieStream>,
    info: StreamInfo,
    frame: Vec<u8>,
    decoded: bool,
    due_ms: u64,
    frame_ms: u64,
}

struct Inner {
    slots: Vec<Option<Slot>>,
    requests: Vec<Request>,
}

pub struct BinkHle {
    source: Option<Arc<dyn MovieSource>>,
    clock: Arc<dyn Clock>,
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
            clock,
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
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }
    /// Every movie the engine asked for, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.lock().requests.clone()
    }
    pub fn open_streams(&self) -> usize {
        self.lock().slots.iter().flatten().count()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- skip mode -------------------------------------------------------

    /// Replacement for the wrapper's `MovieStart(path, flags) ret 8`.
    pub(crate) fn skip_start(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mut mem = ctx.memory();
        let path_ptr = mem.u32(cpu.esp.wrapping_add(4))?;
        let flags = mem.u32(cpu.esp.wrapping_add(8))?;
        let path = read_cstr(&mem, path_ptr);
        self.lock().requests.push(Request {
            path,
            flags,
            played: false,
        });
        // No movie is open and the last one is "finished". Only touch state
        // when the wrapper's page is mapped (it always is in the real image).
        if mem.u32(ST_BINK).is_ok() {
            mem.set_u32(ST_BINK, 0)?;
            mem.set_u8(ST_MOVIE_OPEN, 0)?;
            mem.set_u8(ST_FINISHED, 1)?;
        }
        mem.ret(cpu, 8)
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
        let stream = self.source.as_ref().and_then(|s| s.open(&name));
        let mut handle = 0;
        let mut played = false;
        if let Some(stream) = stream {
            let info = stream.info();
            let ok = (1..=MAX_DIM).contains(&info.width)
                && (1..=MAX_DIM).contains(&info.height)
                && info.frames > 0
                && info.fps_num > 0
                && info.fps_den > 0;
            if ok {
                let mut inner = self.lock();
                if let Some(i) = inner.slots.iter().position(Option::is_none)
                    && let Some(arena) = self.ensure_arena(&mut mem)
                {
                    {
                        let h = arena + i as u32 * SLOT_SIZE;
                        let frame_ms = (1000 * info.fps_den as u64 / info.fps_num as u64).max(1);
                        mem.set_u32(h + F_WIDTH, info.width)?;
                        mem.set_u32(h + F_HEIGHT, info.height)?;
                        mem.set_u32(h + F_FRAMES, info.frames)?;
                        mem.set_u32(h + F_FRAME_NUM, 1)?;
                        mem.set_u32(h + F_LAST_FRAME, 0)?;
                        mem.set_u32(h + F_RATE, info.fps_num)?;
                        mem.set_u32(h + F_RATE_DIV, info.fps_den)?;
                        inner.slots[i] = Some(Slot {
                            stream,
                            info,
                            frame: vec![0; info.width as usize * info.height as usize * 4],
                            decoded: false,
                            due_ms: self.clock.now_ms(),
                            frame_ms,
                        });
                        handle = h;
                        played = true;
                    }
                }
            }
        }
        self.lock().requests.push(Request {
            path,
            flags,
            played,
        });
        cpu.eax = handle;
        mem.ret(cpu, 8)
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

    /// `BinkWait(bnk)`: nonzero while the next frame is not yet due.
    pub(crate) fn wait(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        let now = self.clock.now_ms();
        cpu.eax = match self
            .slot_index(h)
            .and_then(|i| self.lock().slots[i].as_ref().map(|s| s.due_ms))
        {
            Some(due) if now < due => 1,
            _ => 0,
        };
        mem.ret(cpu, 4)
    }

    /// `BinkDoFrame(bnk)`: decode the current frame into the host buffer.
    pub(crate) fn do_frame(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        if let Some(i) = self.slot_index(h) {
            let n = mem.u32(h + F_FRAME_NUM)?;
            let mut inner = self.lock();
            if let Some(s) = inner.slots[i].as_mut() {
                let idx = n.saturating_sub(1).min(s.info.frames - 1);
                s.decoded = s.stream.decode(idx, &mut s.frame) || s.decoded;
            }
        }
        cpu.eax = 0;
        mem.ret(cpu, 4)
    }

    /// `BinkNextFrame(bnk)`: advance, never past the last frame.
    pub(crate) fn next_frame(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> R {
        let mut mem = ctx.memory();
        let h = mem.u32(cpu.esp.wrapping_add(4))?;
        if let Some(i) = self.slot_index(h) {
            let n = mem.u32(h + F_FRAME_NUM)?;
            let mut inner = self.lock();
            if let Some(s) = inner.slots[i].as_mut() {
                mem.set_u32(h + F_LAST_FRAME, n)?;
                // Like the original (0x3e2fb9..0x3e2fc8): past the last frame
                // NextFrame wraps to frame 1, which is what makes loops loop.
                // Non-looping callers stop on `FrameNum == Frames` first.
                mem.set_u32(h + F_FRAME_NUM, if n >= s.info.frames { 1 } else { n + 1 })?;
                let now = self.clock.now_ms();
                let next = s.due_ms + s.frame_ms;
                // Fell more than a frame behind: resync rather than burst.
                s.due_ms = if now > next + s.frame_ms { now } else { next };
            }
        }
        cpu.eax = 0;
        mem.ret(cpu, 4)
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
            let inner = self.lock();
            if let Some(s) = inner.slots[i].as_ref().filter(|s| s.decoded) {
                let (w, hh) = (s.info.width as usize, s.info.height as usize);
                let rows = hh.min((height as usize).saturating_sub(y as usize));
                let cols = w.min((pitch as usize / 4).saturating_sub(x as usize));
                let mut line = vec![0u8; cols * 4];
                let mut ok = true;
                for r in 0..rows {
                    line.copy_from_slice(&s.frame[r * w * 4..r * w * 4 + cols * 4]);
                    if swap {
                        for px in line.as_chunks_mut::<4>().0 {
                            px.swap(0, 2);
                        }
                    }
                    let at = dst
                        .wrapping_add((y + r as u32).wrapping_mul(pitch))
                        .wrapping_add(x * 4);
                    if mem.write_bytes(at, &line, 1).is_err() {
                        ok = false;
                        break;
                    }
                }
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

fn read_cstr(mem: &runtime::Guest, mut p: u32) -> String {
    let mut out = Vec::new();
    while out.len() < 260 {
        match mem.u8(p) {
            Ok(0) | Err(_) => break,
            Ok(b) => out.push(b),
        }
        p = p.wrapping_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
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
