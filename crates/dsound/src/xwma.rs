//! The native WMA decoder registered at the game's codec boundary.
//!
//! Halo 2 decodes its WMA music and dialogue through the XDK's `XWMAXMO`
//! container, which calls a statically linked WMAv2 codec through six
//! stdcall entries, starting with the creator at `0x38a0c5`. [`register`]
//! replaces those six with [`wma::boundary::Session`] (the clean-room
//! decoder in `crates/wma`), so the container, its file reads and its XMO
//! packets stay the game's own while the codec runs natively. See
//! `docs/audio/wma.md` "Runtime boundary" for the ABI and the status contract.
//!
//! The codec tables are read from the retail XBE when the [`WmaHle`] is
//! built ([`WmaHle::from_xbe`]); none are compiled in.
//!
//! Input: begin packet pulls bytes by calling the game's input callback
//! (`0x388777`, `(data**, size*, context, staging)`, `ret 10h`) as a guest
//! call, with its four arguments and a 136-byte scratch block (the two out
//! words and a 128-byte staging buffer) placed below the caller's stack
//! pointer, until a whole codec packet has arrived or the callback returns
//! no bytes. No lock is held across a guest call.
use crate::register::{Conflict, RegisterError};
use runtime::{CpuAbi, Dispatch, ExecCtx, GuestFault, TransferAbi};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use wma::boundary::{self, CreateParams, INVALID_ARG, OK, Session};

/// `0x38a0c5` create: 11 stack dwords.
pub const CREATE: u32 = 0x0038_a0c5;
/// `0x38a13f` destroy `(&handle)`.
pub const DESTROY: u32 = 0x0038_a13f;
/// `0x38a16b` reset `(handle)`.
pub const RESET: u32 = 0x0038_a16b;
/// `0x38a189` begin packet `(handle)`.
pub const BEGIN: u32 = 0x0038_a189;
/// `0x38a1a7` decode `(handle)`.
pub const DECODE: u32 = 0x0038_a1a7;
/// `0x38a1cc` PCM `(handle, &frames, buffer, bytes)`.
pub const GET_PCM: u32 = 0x0038_a1cc;

/// The six entries: address, name, stack dwords (`ret 4 * argc`).
pub const ENTRIES: [(u32, &str, u32); 6] = [
    (CREATE, "WmaCreateDecoder", 11),
    (DESTROY, "WmaDestroyDecoder", 1),
    (RESET, "WmaResetDecoder", 1),
    (BEGIN, "WmaBeginPacket", 1),
    (DECODE, "WmaDecode", 1),
    (GET_PCM, "WmaGetPcm", 4),
];

/// First handle value handed to the game. The container only stores and
/// passes handles back (`[esi+158h]`); they are never dereferenced, so they
/// sit in kernel space, where a stray dereference faults loudly.
const FIRST_HANDLE: u32 = 0xfee0_0010;
/// Upper bound on callback calls for one begin (a callback that returns a
/// byte at a time still completes a 5945-byte packet).
const MAX_PULLS: u32 = 1 << 16;
/// Bytes one callback may return: its staging buffer is 128 (`0x388a21`).
const STAGING: u32 = 128;

struct Stream {
    session: Session,
    callback: u32,
    context: u32,
}

/// A create the boundary refused (format outside the retail profiles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub params: CreateParams,
}

/// The shared state of the six overrides.
pub struct WmaHle {
    tables: Arc<wma::Tables>,
    streams: Mutex<HashMap<u32, Stream>>,
    next: Mutex<u32>,
    refusals: Mutex<Vec<Refusal>>,
    totals: Mutex<boundary::Stats>,
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl WmaHle {
    pub fn new(tables: Arc<wma::Tables>) -> Arc<Self> {
        Arc::new(Self {
            tables,
            streams: Mutex::new(HashMap::new()),
            next: Mutex::new(FIRST_HANDLE),
            refusals: Mutex::new(Vec::new()),
            totals: Mutex::new(boundary::Stats::default()),
        })
    }

    /// Reads the codec tables out of the retail `default.xbe` bytes (the
    /// SHA-256 is checked first, see `docs/audio/wma.md`).
    pub fn from_xbe(xbe: &[u8]) -> Result<Arc<Self>, wma::Error> {
        let image = wma::VerifiedImage::load(xbe)?;
        Ok(Self::new(Arc::new(wma::Tables::load(&image)?)))
    }

    /// Live codec instances.
    pub fn streams(&self) -> usize {
        lock(&self.streams).len()
    }

    /// Counters summed over every instance, live or destroyed.
    pub fn stats(&self) -> boundary::Stats {
        let mut t = *lock(&self.totals);
        for s in lock(&self.streams).values() {
            add(&mut t, &s.session.stats);
        }
        t
    }

    pub fn take_refusals(&self) -> Vec<Refusal> {
        std::mem::take(&mut lock(&self.refusals))
    }

    fn with<T>(&self, handle: u32, f: impl FnOnce(&mut Stream) -> T) -> Option<T> {
        lock(&self.streams).get_mut(&handle).map(f)
    }
}

/// The codec instances' snapshot section.
pub const SECTION: snapshot::Tag = snapshot::Tag::new("wma");
const SECTION_VERSION: u32 = 1;

impl WmaHle {
    /// Adds the `wma` section (docs/runtime/snapshots.md, phase 4): every
    /// live instance under its handle, with the game's input callback and
    /// context and the whole session (decoder overlap, held packet, partial
    /// input, queued PCM), and the next handle. A begin packet that is
    /// pulling input keeps its state in a guest call continuation, which
    /// `kernel::Machine::quiescent` refuses; between calls an instance is
    /// plain data.
    pub fn save_snapshot(&self, out: &mut snapshot::SnapshotWriter) -> Result<(), snapshot::Error> {
        let WmaHle {
            // Keep: the loading process's tables.
            tables: _,
            streams,
            next,
            // Diagnostics.
            refusals: _,
            totals: _,
        } = self;
        let mut w = snapshot::codec::Writer::new();
        w.u32(*lock(next));
        let streams = lock(streams);
        let mut handles: Vec<u32> = streams.keys().copied().collect();
        handles.sort_unstable();
        w.u32(handles.len() as u32);
        for h in handles {
            let Stream {
                session,
                callback,
                context,
            } = &streams[&h];
            w.u32(h).u32(*callback).u32(*context);
            session.save_state(&mut w);
        }
        out.section(
            SECTION,
            SECTION_VERSION,
            snapshot::Codec::Zstd,
            w.into_bytes(),
        )?;
        Ok(())
    }

    /// Replaces every instance and the handle counter with the `wma`
    /// section's. Decoded first: on an error nothing changes.
    pub fn load_snapshot(&self, snapshot: &snapshot::Snapshot<'_>) -> Result<(), snapshot::Error> {
        let bytes = snapshot.require(SECTION, SECTION_VERSION..=SECTION_VERSION)?;
        let mut r = snapshot::codec::Reader::new(&bytes, "wma");
        let next = r.u32()?;
        let loaded = r.list(12, |r| {
            let handle = r.u32()?;
            let callback = r.u32()?;
            let context = r.u32()?;
            let session = Session::load_state(Arc::clone(&self.tables), r)?;
            Ok((
                handle,
                Stream {
                    session,
                    callback,
                    context,
                },
            ))
        })?;
        r.finish()?;
        *lock(&self.streams) = loaded.into_iter().collect();
        *lock(&self.next) = next;
        Ok(())
    }
}

fn add(t: &mut boundary::Stats, s: &boundary::Stats) {
    t.packets += s.packets;
    t.pcm_frames += s.pcm_frames;
    t.restarts += s.restarts;
    t.errors += s.errors;
    t.dropped_partials += s.dropped_partials;
}

/// What [`register`] installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub addresses: Vec<u32>,
}

fn override_name(name: &str) -> String {
    format!("wma::{name}")
}

/// Installs the six codec entries, or none: an address that already has an
/// override fails with [`RegisterError::Occupied`] and nothing is changed.
pub fn register(dispatch: &mut Dispatch, hle: &Arc<WmaHle>) -> Result<Registration, RegisterError> {
    let conflicts: Vec<_> = ENTRIES
        .iter()
        .filter_map(|&(va, name, _)| {
            dispatch.override_name(va).map(|existing| Conflict {
                addr: va,
                existing: existing.to_owned(),
                ours: override_name(name),
            })
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(RegisterError::Occupied(conflicts));
    }
    for (va, name, argc) in ENTRIES {
        let hle = Arc::clone(hle);
        dispatch.register_override(va, override_name(name), move |cpu, ctx, _| {
            call(&hle, va, argc, cpu, ctx)
        });
    }
    Ok(Registration {
        addresses: ENTRIES.iter().map(|e| e.0).collect(),
    })
}

/// Disables what [`register`] installed (`Dispatch` cannot remove).
pub fn unregister(dispatch: &mut Dispatch, registration: &Registration) {
    for &va in &registration.addresses {
        dispatch.set_enabled(va, false);
    }
}

fn call(
    hle: &Arc<WmaHle>,
    va: u32,
    argc: u32,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
) -> Result<TransferAbi, GuestFault> {
    let (ret_pc, a) = {
        let m = ctx.memory();
        let ret_pc = m.u32(cpu.esp)?;
        let a = (0..argc)
            .map(|i| m.u32(cpu.esp.wrapping_add(4 + 4 * i)))
            .collect::<Result<Vec<_>, _>>()?;
        (ret_pc, a)
    };
    // stdcall: the frame is popped whatever the outcome.
    cpu.esp = cpu.esp.wrapping_add(4 + 4 * argc);
    let done = |cpu: &mut CpuAbi, eax: u32| {
        cpu.eax = eax;
        Ok(TransferAbi::ret(ret_pc))
    };
    match va {
        CREATE => {
            let eax = create(hle, ctx, &a)?;
            done(cpu, eax)
        }
        DESTROY => {
            // (&handle): frees the instance and clears the caller's word.
            if a[0] != 0 {
                let mut m = ctx.memory();
                let handle = m.u32(a[0])?;
                if handle != 0 {
                    if let Some(s) = lock(&hle.streams).remove(&handle) {
                        add(&mut lock(&hle.totals), &s.session.stats);
                    }
                    m.set_u32(a[0], 0)?;
                }
            }
            done(cpu, OK)
        }
        RESET => {
            let r = hle
                .with(a[0], |s| s.session.reset())
                .map_or(INVALID_ARG, |()| OK);
            done(cpu, r)
        }
        DECODE => {
            let r = hle
                .with(a[0], |s| s.session.decode())
                .unwrap_or(INVALID_ARG);
            done(cpu, r)
        }
        GET_PCM => {
            let eax = get_pcm(hle, ctx, &a)?;
            done(cpu, eax)
        }
        BEGIN => match hle.with(a[0], |s| (s.callback, s.context)) {
            None => done(cpu, INVALID_ARG),
            Some((callback, context)) => pull(
                Arc::clone(hle),
                a[0],
                callback,
                context,
                cpu,
                ctx,
                ret_pc,
                0,
            ),
        },
        _ => unreachable!("only ENTRIES are registered"),
    }
}

fn create(hle: &WmaHle, ctx: &mut ExecCtx, a: &[u32]) -> Result<u32, GuestFault> {
    let (out, context, callback) = (a[0], a[1], a[2]);
    let params = CreateParams {
        version: a[3],
        samples_per_block: a[4],
        sample_rate: a[5],
        channels: a[6],
        bytes_per_second: a[7],
        block_align: a[8],
        flags: a[9],
    };
    if out == 0 || callback == 0 {
        return Ok(INVALID_ARG);
    }
    let session = match Session::new(Arc::clone(&hle.tables), &params) {
        Ok(s) => s,
        Err(code) => {
            lock(&hle.refusals).push(Refusal { params });
            return Ok(code);
        }
    };
    let handle = {
        let mut next = lock(&hle.next);
        let h = *next;
        *next = next.wrapping_add(0x10).max(FIRST_HANDLE);
        h
    };
    ctx.memory().set_u32(out, handle)?;
    lock(&hle.streams).insert(
        handle,
        Stream {
            session,
            callback,
            context,
        },
    );
    Ok(OK)
}

/// `(handle, &frames, buffer, bytes)`: up to `frames` (a u16) stereo i16
/// frames, bounded by `bytes`; writes the count back.
fn get_pcm(hle: &WmaHle, ctx: &mut ExecCtx, a: &[u32]) -> Result<u32, GuestFault> {
    let (handle, count_ptr, buffer, bytes) = (a[0], a[1], a[2], a[3]);
    if count_ptr == 0 || buffer == 0 {
        return Ok(INVALID_ARG);
    }
    let mut m = ctx.memory();
    let want = usize::from(m.u16(count_ptr)?).min(bytes as usize / 4);
    let Some(pcm) = hle.with(handle, |s| s.session.take_pcm(want)) else {
        return Ok(INVALID_ARG);
    };
    let out: Vec<u8> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
    m.write_bytes(buffer, &out, 1)?;
    m.set_u16(count_ptr, (pcm.len() / 2) as u16)?;
    Ok(OK)
}

/// Begin packet: call the input callback until the session has a whole
/// packet or the callback runs dry, then begin and return to `ret_pc`.
/// `cpu.esp` is the caller's, with this entry's frame already popped.
#[allow(clippy::too_many_arguments)]
fn pull(
    hle: Arc<WmaHle>,
    handle: u32,
    callback: u32,
    context: u32,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    ret_pc: u32,
    pulls: u32,
) -> Result<TransferAbi, GuestFault> {
    let wants = hle.with(handle, |s| s.session.wants()).unwrap_or(0);
    if wants == 0 || pulls >= MAX_PULLS {
        // A whole packet, or a callback that never runs dry: not the end.
        let status = hle
            .with(handle, |s| s.session.begin(false))
            .unwrap_or(INVALID_ARG);
        cpu.eax = status;
        return Ok(TransferAbi::ret(ret_pc));
    }
    let base = cpu.esp;
    // [data ptr][size][128-byte staging], 16-byte aligned, below the caller.
    let scratch = base.wrapping_sub(0x100) & !0xf;
    let args = scratch.wrapping_sub(16);
    let runtime = Arc::clone(ctx.runtime());
    let mut m = runtime.memory();
    m.set_u32(scratch, 0)?;
    m.set_u32(scratch.wrapping_add(4), 0)?;
    for (i, v) in [
        scratch,
        scratch.wrapping_add(4),
        context,
        scratch.wrapping_add(8),
    ]
    .into_iter()
    .enumerate()
    {
        m.set_u32(args.wrapping_add(4 * i as u32), v)?;
    }
    cpu.esp = args;
    ctx.call_guest(cpu, &mut m, callback, move |cpu, ctx| {
        let result = cpu.eax;
        let bytes = {
            let m = ctx.memory();
            let data = m.u32(scratch)?;
            let size = m.u32(scratch.wrapping_add(4))?.min(STAGING);
            let mut b = vec![0; size as usize];
            if size != 0 {
                m.read_bytes(data, &mut b, 1)?;
            }
            b
        };
        cpu.esp = base;
        if bytes.is_empty() {
            // Dry: the reader's next read has not landed, or it is past
            // the last data packet. The container decides which with this
            // same test after NO_DATA (0x388696); its reader rewinds
            // [+4] on a pending read (0x387fe7).
            let end_of_input = end_of_input(&ctx.memory(), context)?;
            let status = hle
                .with(handle, |s| s.session.begin(end_of_input))
                .unwrap_or(INVALID_ARG);
            cpu.eax = status;
            return Ok(TransferAbi::ret(ret_pc));
        }
        hle.with(handle, |s| {
            s.session.feed(&bytes, result == boundary::PACKET_START)
        });
        pull(hle, handle, callback, context, cpu, ctx, ret_pc, pulls + 1)
    })
}

/// The container's end-of-file test (`0x388696`, and its reader's at
/// `0x387f63`): the next read offset `[container+4]` is past the last data
/// packet's offset `[container+24h]`. `context` is the container, as create
/// receives it from `0x388ac1`.
fn end_of_input(m: &runtime::Guest, context: u32) -> Result<bool, GuestFault> {
    Ok(m.u32(context.wrapping_add(READ_OFFSET))? > m.u32(context.wrapping_add(LAST_PACKET))?)
}

/// Container field: offset of the reader's next ASF data packet.
pub const READ_OFFSET: u32 = 0x4;
/// Container field: offset of the last ASF data packet.
pub const LAST_PACKET: u32 = 0x24;
