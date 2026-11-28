//! `native-decal-decorators` (docs/graphics/retire-d3d-decal-decorators.md):
//! the BSP's decal decorators (types 1 and 2), drawn by the native
//! renderer at their place in the sorted transparent layer, and the cut
//! of their draw `0x0004ebd0`.
//!
//! The draw is called by the sorted walk `0x00040f60` once per pass of a
//! placement's shader in layer 15 (stdcall `(shader, 0, 0, layer, pass,
//! argument, entry)`, `ret 0x1c`). It makes no D3D entry of its own
//! before it sets `c[53]`, the stream, the shader (`0x0001bbf0`), the
//! stage textures, `ZBias` 1, one `DrawVertices` and `ZBias` 0.
//!
//! - **The stage.** A guest body runs inside the host's draw scope: its
//!   resolved `DrawVertices` is drawn natively at the call, so in the
//!   sorted order ([`Host::begin_body`], [`Host::end_body`]). The draw
//!   the host admitted is the call's template.
//! - **The cut.** Once one whole frame's bodies were all drawn natively
//!   with no difference (the proof), a call returns without its body
//!   (`ret 0x1c`) when the host can draw it from the last body's template
//!   ([`Host::holds`]: the same key, and every device and shader-cache
//!   word that body left still in place, so nothing ran since) with this
//!   placement's own vertices ([`Host::draw`]). The call's own guest write,
//!   the geometry cache's stamp (`0x0012de70`'s resident path), is made.
//! - **The arm.** One eligible call in [`SAMPLE`] runs its body anyway;
//!   the host compares the body's draw with the one the cut would have
//!   made and the words it left with the words before it. A difference
//!   sends every later call to the body for the session.
//!
//! A call that runs its body is counted by cause in `render.scene`
//! (`scene_kept_decal_decorator_<cause>`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use scene_extract::GuestRead;
use scene_extract::layout::decorators::{
    ACTIVE_BSP, BLOCK_GEOMETRY, BLOCK_PLACEMENTS, BSP_DECORATORS, CLASS_SIZE, GEOMETRY_SIZE,
    LOD_FACTORS, PLACEMENT_SIZE, SCENARIO, SCENARIO_PALETTE, SET_CLASSES, cache_stamp, definition,
};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-decal-decorators";

/// The switch (`impl.render.native-decal-decorators=off` runs every body
/// on D3D: no stage, no cut).
pub const SWITCH: &str = "impl.render.native-decal-decorators";

/// Whether the stage and its cut are placed: with the native renderer,
/// unless the switch is off.
pub fn on(switches: &hooks::Switches) -> bool {
    true
        && switches.feature("impl.render.native-decal-decorators", true)
}

/// The draw's stack bytes (`ret 0x1c`).
pub const ARG_BYTES: u32 = 0x1c;
/// One eligible call in this many runs its body for the arm.
pub const SAMPLE: u64 = 64;

/// The lazy constants' flags word: bits 0 and 1 set once `0x0004ebe4` and
/// `0x0004ec18` have stored their three floats each.
pub const INIT_FLAGS: u32 = 0x0055_ee5c;
/// `c[53]`'s `w` (`0x0004ed02`).
pub const C53_W: u32 = 0x0045_dbc0;
/// The geometry record's block (`+0x28`): its vertex stream descriptor at
/// `+0x18` (type byte, stride byte `+1`, offset `+8`, D3D buffer `+0x10`).
pub const RECORD_BLOCK: u32 = 0x28;
pub const BLOCK_STREAM: u32 = 0x18;
/// The class's type byte: 1 draws a quad list, 2 a triangle list.
pub const CLASS_TYPE: u32 = 4;
/// The placement's vertex range (`u16` each): first vertex `+0xc`, count `+0xe`.
pub const PLACEMENT_START: u32 = 0xc;
pub const PLACEMENT_COUNT: u32 = 0xe;
/// The entry the build filled: implementation index byte `+4`, the
/// implementation `+0x10`.
pub const ENTRY_IMPLEMENTATION_INDEX: u32 = 4;
pub const ENTRY_IMPLEMENTATION: u32 = 0x10;
/// Most vertices one placement may draw.
const MAX_VERTICES: u32 = 0xffff;

/// What `DrawVertices` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Type 1, `D3DPT_QUADLIST` (8).
    Quads,
    /// Type 2, `D3DPT_TRIANGLELIST` (5).
    Triangles,
}

/// The placement's vertex stream as `0x0001c620` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stream {
    pub kind: u8,
    pub stride: u8,
    pub offset: u32,
    pub buffer: u32,
    /// The buffer's data: `0x80000000 | ([buffer + 4] & 0x0fffffff)`.
    pub data: u32,
}

/// What a template is kept and matched by: every input of the body
/// besides the vertex range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// Stack words 0 to 4: shader, the two zeros, layer, pass.
    pub args: [u32; 5],
    /// The entry's implementation index and implementation.
    pub implementation: (u8, u32),
    pub block: u32,
    pub kind: Kind,
    pub stream: Stream,
    /// `c[53]` as bits.
    pub c53: [u32; 4],
}

/// One call as its body would make it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The call's whole input; `None` when its body draws nothing
    /// (a block with no buffer) or makes no draw it could stand for (a
    /// class type other than 1 or 2: [`Call::kind`]).
    pub key: Option<Key>,
    /// The block has no buffer: the body writes the stamp and returns.
    pub empty: bool,
    /// The class's type byte.
    pub class_type: u8,
    /// The stamp `0x0012de70` writes: (address, value).
    pub stamp: (u32, u32),
    pub start: u32,
    pub count: u32,
}

impl Call {
    /// The bytes the placement draws: `count` vertices from `start`.
    pub fn vertices(&self, mem: &dyn GuestMemory) -> Result<Vec<u8>, &'static str> {
        let key = self.key.as_ref().ok_or("type")?;
        let stride = u32::from(key.stream.stride);
        let at = key
            .stream
            .data
            .wrapping_add(key.stream.offset)
            .wrapping_add(self.start * stride);
        mem.bytes(at, (self.count * stride) as usize)
            .map_err(|_| "vertices_unread")
    }
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

fn word(mem: &dyn GuestMemory, addr: u32) -> Result<u32, &'static str> {
    mem.u32(addr).map_err(|_| "unread")
}

fn byte(mem: &dyn GuestMemory, addr: u32) -> Result<u8, &'static str> {
    mem.u8(addr).map_err(|_| "unread")
}

fn half(mem: &dyn GuestMemory, addr: u32) -> Result<u16, &'static str> {
    mem.u16(addr).map_err(|_| "unread")
}

/// The call at `esp` (its return address at `[esp]`) as the body reads
/// it (`0x0004ec48`..`0x0004edbc`). `Err` names why the body must run:
/// `init` (the lazy constants not yet stored), `nonresident` (the
/// geometry cache would load the record), `vertices` (a range past the
/// stream's bound), `unread`.
pub fn read_call(mem: &dyn GuestMemory, esp: u32) -> Result<Call, &'static str> {
    if word(mem, INIT_FLAGS)? & 3 != 3 {
        return Err("init");
    }
    let arg = |n: u32| word(mem, esp.wrapping_add(4 + 4 * n));
    let args = [arg(0)?, arg(1)?, arg(2)?, arg(3)?, arg(4)?];
    let argument = arg(5)?;
    let entry = arg(6)?;
    let block = word(mem, word(mem, ACTIVE_BSP)?.wrapping_add(BSP_DECORATORS))?;
    let placement = word(mem, block.wrapping_add(BLOCK_PLACEMENTS))?
        .wrapping_add(((argument >> 8) & 0x3f_ffff).wrapping_mul(PLACEMENT_SIZE));
    let record = word(mem, block.wrapping_add(BLOCK_GEOMETRY))?.wrapping_add(
        (i32::from(half(mem, placement.wrapping_add(6))? as i16) as u32)
            .wrapping_mul(GEOMETRY_SIZE),
    );
    let palette = i32::from(byte(mem, placement)? as i8) as u32;
    let set = word(
        mem,
        word(mem, word(mem, SCENARIO)?.wrapping_add(SCENARIO_PALETTE))?
            .wrapping_add(palette.wrapping_mul(8).wrapping_add(4)),
    )?;
    let set = definition(&Ro(mem), set & 0xffff).ok_or("unread")?;
    let class = word(mem, set.wrapping_add(SET_CLASSES))?
        .wrapping_add(u32::from(byte(mem, placement.wrapping_add(1))?) * CLASS_SIZE);
    let stamp = cache_stamp(&Ro(mem), record)
        .map_err(|_| "unread")?
        .ok_or("nonresident")?;
    let geometry = word(mem, record.wrapping_add(RECORD_BLOCK))?;
    let descriptor = geometry.wrapping_add(BLOCK_STREAM);
    let buffer = word(mem, descriptor.wrapping_add(0x10))?;
    let class_type = byte(mem, class.wrapping_add(CLASS_TYPE))?;
    let start = u32::from(half(mem, placement.wrapping_add(PLACEMENT_START))?);
    let count = u32::from(half(mem, placement.wrapping_add(PLACEMENT_COUNT))?);
    if buffer == 0 {
        // `0x0004ecd1`: the body returns after the stamp.
        return Ok(Call {
            key: None,
            empty: true,
            class_type,
            stamp,
            start,
            count,
        });
    }
    let kind = match class_type {
        1 => Kind::Quads,
        2 => Kind::Triangles,
        _ => {
            return Ok(Call {
                key: None,
                empty: false,
                class_type,
                stamp,
                start,
                count,
            });
        }
    };
    if count > MAX_VERTICES
        || start
            .checked_add(count)
            .is_none_or(|end| end > MAX_VERTICES + 1)
    {
        return Err("vertices");
    }
    let stream = Stream {
        kind: byte(mem, descriptor)?,
        stride: byte(mem, descriptor.wrapping_add(1))?,
        offset: word(mem, descriptor.wrapping_add(8))?,
        buffer,
        data: 0x8000_0000 | (word(mem, buffer.wrapping_add(4))? & 0x0fff_ffff),
    };
    let lod = 4 * (argument >> 30);
    let c53 = [
        word(mem, LOD_FACTORS[0] + lod)?,
        word(mem, LOD_FACTORS[1] + lod)?,
        0,
        word(mem, C53_W)?,
    ];
    let implementation = (
        byte(mem, entry.wrapping_add(ENTRY_IMPLEMENTATION_INDEX))?,
        word(mem, entry.wrapping_add(ENTRY_IMPLEMENTATION))?,
    );
    Ok(Call {
        key: Some(Key {
            args,
            implementation,
            block: geometry,
            kind,
            stream,
            c53,
        }),
        empty: false,
        class_type,
        stamp,
        start,
        count,
    })
}

/// What a guest body's draw came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    /// `Ok`: drawn natively and kept as the template; `Err`: drawn by D3D,
    /// and why.
    pub native: Result<(), &'static str>,
    /// For a sampled body, the fields where the cut would have differed.
    pub differs: Vec<String>,
}

/// What draws the calls: boot's glue over the device.
pub trait Host: Send + Sync {
    /// The present count: a frame's bodies prove the stage.
    fn frame(&self) -> u64;
    /// Before a body of `call`: arms the native admission of its draw;
    /// `sampled` also keeps the words before it and the draw the cut
    /// would make, for the arm.
    fn begin_body(&self, mem: &mut dyn GuestMemory, call: &Call, sampled: bool);
    /// After the body: what its draw came to.
    fn end_body(&self, mem: &mut dyn GuestMemory, call: &Call) -> Body;
    /// Whether the last body's template stands for `call`: the same key,
    /// and the words that body left still in place.
    fn holds(&self, mem: &dyn GuestMemory, call: &Call) -> Result<(), &'static str>;
    /// Draws `call` from the template with its own vertices, or names why
    /// not, with nothing recorded.
    fn draw(&self, mem: &mut dyn GuestMemory, call: &Call) -> Result<(), &'static str>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call runs its body on D3D.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What a call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Drawn (or nothing to draw) and the stamp written: the call returns.
    Return,
    /// The body runs; `sampled` for the arm.
    Body { sampled: bool },
}

/// The proof: a frame whose bodies were all drawn natively.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Proof {
    generation: u64,
    frame: u64,
    bodies: u64,
    refused: u64,
    proven: bool,
}

impl Proof {
    /// At a call in `frame` of map `generation`: a new map forgets the
    /// proof; a new frame closes the last one, proving the stage when it
    /// had bodies and none was refused.
    fn roll(&mut self, generation: u64, frame: u64) {
        if generation != self.generation {
            *self = Proof {
                generation,
                frame,
                ..Proof::default()
            };
        } else if frame != self.frame {
            self.proven |= self.bodies > 0 && self.refused == 0;
            self.frame = frame;
            self.bodies = 0;
            self.refused = 0;
        }
    }
}

#[derive(Default)]
struct Counts {
    calls: u64,
    returned: u64,
    empty: u64,
    eligible: u64,
    checked: u64,
    bodies_native: u64,
    proof: Proof,
    /// Calls that ran the body, by cause.
    kept: BTreeMap<&'static str, u64>,
    /// Bodies whose draw stayed on D3D, by cause.
    guest_d3d: BTreeMap<&'static str, u64>,
    /// Arm differences, by field.
    mismatched: BTreeMap<String, u64>,
}

/// The counts, the proof and the session's fallback.
struct State {
    counts: Mutex<Option<Counts>>,
    fallback: AtomicBool,
}

impl State {
    const fn new() -> Self {
        Self {
            counts: Mutex::new(None),
            fallback: AtomicBool::new(false),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Counts) -> T) -> T {
        f(self
            .counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Counts::default))
    }

    fn keep(&self, cause: &'static str) -> Turn {
        self.with(|c| *c.kept.entry(cause).or_default() += 1);
        Turn::Body { sampled: false }
    }

    fn fallen_back(&self) -> bool {
        self.fallback.load(Ordering::Relaxed)
    }

    fn turn(
        &self,
        mem: &mut dyn GuestMemory,
        esp: u32,
        host: Option<&dyn Host>,
        generation: u64,
    ) -> (Turn, Option<Call>) {
        self.with(|c| c.calls += 1);
        if let Some(h) = host {
            let frame = h.frame();
            self.with(|c| c.proof.roll(generation, frame));
        }
        let call = match read_call(&*mem, esp) {
            Ok(call) => call,
            Err(cause) => return (self.keep(cause), None),
        };
        if call.key.is_none() {
            if call.empty {
                // A block with no buffer: the body writes the stamp and
                // returns, with no D3D entry.
                if mem.set_u32(call.stamp.0, call.stamp.1).is_err() {
                    return (self.keep("stamp"), Some(call));
                }
                self.with(|c| {
                    c.empty += 1;
                    c.returned += 1;
                });
                return (Turn::Return, Some(call));
            }
            return (self.keep("type"), Some(call));
        }
        if self.fallen_back() {
            return (self.keep("fallback"), Some(call));
        }
        let Some(host) = host else {
            return (self.keep("host"), Some(call));
        };
        if !self.with(|c| c.proof.proven) {
            return (self.keep("unproven"), Some(call));
        }
        if let Err(cause) = host.holds(&*mem, &call) {
            return (self.keep(cause), Some(call));
        }
        let eligible = self.with(|c| {
            c.eligible += 1;
            c.eligible
        });
        if eligible % SAMPLE == 0 {
            self.with(|c| *c.kept.entry("sampled").or_default() += 1);
            return (Turn::Body { sampled: true }, Some(call));
        }
        // The stamp first, as the body makes it before its draw; a refused
        // draw leaves it to the body, which writes the same word again.
        if mem.set_u32(call.stamp.0, call.stamp.1).is_err() {
            return (self.keep("stamp"), Some(call));
        }
        if let Err(cause) = host.draw(mem, &call) {
            return (self.keep(cause), Some(call));
        }
        self.with(|c| c.returned += 1);
        (Turn::Return, Some(call))
    }

    fn record(&self, body: &Body, sampled: bool) {
        self.with(|c| {
            c.proof.bodies += 1;
            match body.native {
                Ok(()) => c.bodies_native += 1,
                Err(cause) => {
                    c.proof.refused += 1;
                    *c.guest_d3d.entry(cause).or_default() += 1;
                }
            }
            if sampled || !body.differs.is_empty() {
                c.checked += 1;
            }
            for f in &body.differs {
                *c.mismatched.entry(f.clone()).or_default() += 1;
            }
        });
        if !body.differs.is_empty() && !self.fallback.swap(true, Ordering::Relaxed) {
            eprintln!(
                "native-decal-decorators: a sampled body differs from the cut ({:?}); every later call runs its body",
                body.differs
            );
        }
    }

    fn counters(&self) -> Vec<(String, u64)> {
        let fallback = self.fallen_back();
        self.with(|c| {
            let mut out = vec![
                ("decal_decorator_calls".to_owned(), c.calls),
                ("decal_decorator_returned".to_owned(), c.returned),
                ("decal_decorator_empty".to_owned(), c.empty),
                ("decal_decorator_eligible".to_owned(), c.eligible),
                ("decal_decorator_checked".to_owned(), c.checked),
                ("decal_decorator_bodies_native".to_owned(), c.bodies_native),
                (
                    "decal_decorator_proven".to_owned(),
                    u64::from(c.proof.proven),
                ),
                ("decal_decorator_fallback".to_owned(), u64::from(fallback)),
            ];
            out.extend(
                c.kept
                    .iter()
                    .map(|(k, n)| (format!("scene_kept_decal_decorator_{k}"), *n)),
            );
            out.extend(
                c.guest_d3d
                    .iter()
                    .map(|(k, n)| (format!("decal_decorator_guest_d3d_{k}"), *n)),
            );
            out.extend(
                c.mismatched
                    .iter()
                    .map(|(f, n)| (format!("decal_decorator_mismatch_{f}"), *n)),
            );
            out
        })
    }
}

static STATE: State = State::new();

/// Whether a difference has sent the session back to the bodies.
pub fn fallen_back() -> bool {
    STATE.fallen_back()
}

/// The call's turn at the draw's entry (`esp` its stack). On
/// [`Turn::Return`] it is drawn and its stamp written.
pub fn turn(mem: &mut dyn GuestMemory, esp: u32) -> (Turn, Option<Call>) {
    STATE.turn(
        mem,
        esp,
        host().as_deref(),
        crate::native_map_work::generation(),
    )
}

/// A body's outcome: its draw for the proof, a sample's differences for
/// the arm (any difference sends the session to the bodies).
pub fn record(body: &Body, sampled: bool) {
    STATE.record(body, sampled);
}

/// `render.scene` rows: `decal_decorator_{calls,returned,empty,eligible,checked,bodies_native,proven,fallback}`,
/// the calls that ran the body by cause (`scene_kept_decal_decorator_<cause>`),
/// the bodies drawn by D3D by cause (`decal_decorator_guest_d3d_<cause>`)
/// and the arm's differences (`decal_decorator_mismatch_<field>`).
pub fn counters() -> Vec<(String, u64)> {
    STATE.counters()
}

/// [`counters`] with static names (each distinct name is leaked once).
pub fn rows() -> Vec<(&'static str, u64)> {
    static NAMES: Mutex<BTreeMap<String, &'static str>> = Mutex::new(BTreeMap::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    counters()
        .into_iter()
        .map(|(n, v)| {
            (
                *names
                    .entry(n.clone())
                    .or_insert_with(|| Box::leak(n.into_boxed_str())),
                v,
            )
        })
        .collect()
}

/// The original body at `addr`.
fn original(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
    addr: u32,
) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget {
        pc: cpu.eip,
        target: addr,
        kind: runtime::abi::JUMP,
        route: runtime::abi::ENTRY,
    });
    Ok(runtime::TransferAbi::fault(event))
}

pub struct NativeDecalDecorators;

impl Feature for NativeDecalDecorators {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Decal decorators drawn by the native renderer in the sorted layer, their draw cut",
        )
        .doc("docs/graphics/retire-d3d-decal-decorators.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .implement(
                &points::DECAL_DECORATOR_DRAW,
                "native_decal_decorators",
                |cpu, ctx, addr| {
                    if addr != points::DECAL_DECORATOR_DRAW.base {
                        return original(cpu, ctx, addr);
                    }
                    let rt = Arc::clone(ctx.runtime());
                    let (outcome, call) = {
                        let mut g = rt.memory();
                        let (outcome, call) = turn(&mut *g, cpu.esp);
                        if outcome == Turn::Return {
                            return g.ret(cpu, ARG_BYTES);
                        }
                        (outcome, call)
                    };
                    let sampled = outcome == Turn::Body { sampled: true };
                    let host = host().filter(|_| call.as_ref().is_some_and(|c| c.key.is_some()));
                    let Some((host, call)) = host.zip(call) else {
                        return original(cpu, ctx, addr);
                    };
                    host.begin_body(&mut *rt.memory(), &call, sampled);
                    let result = original(cpu, ctx, addr);
                    let body = host.end_body(&mut *rt.memory(), &call);
                    record(&body, sampled);
                    result
                },
            )
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
