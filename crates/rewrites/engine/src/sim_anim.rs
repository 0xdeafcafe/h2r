//! The guest side of `sim_anim`: one animation stream evaluated natively at
//! `0x2798a0` (docs/engine/anim-sampling.md).
//!
//! `0x2798a0` is the call the three evaluation wrappers (`0x279d80`,
//! `0x279e40`, `0x27a060`) make once they hold a resolved view of the
//! animation. It sets the frame and weight globals, picks a mode and calls
//! the stream dispatcher `0x279860` for the static and the animated
//! stream; everything under it (evaluators, codecs) is replaced here with
//! one call into `sim_anim::sample`. The wrappers themselves stay guest
//! code: they call the tag resolver and the resource cache, which are not
//! animation code.
//!
//! [`stream`] reads the arguments and the view, repeats the setup's
//! arithmetic, takes the clip from the store (copying the blob the first
//! time), samples, and writes back only the record fields the guest would
//! write. It returns the [`Decline`] before writing anything when the call
//! is outside what the kernel reproduces exactly; the hook then panics
//! with its name. The family's scratch globals (`0x504410..0x5044c4`,
//! `0x55e530..0x55e590`, `0x55ebb8..0x55ebe8`) and the dead stack slots
//! are left unwritten: nothing outside the family reads them.
//!
//! [`skeleton`] is phase 1b: the skeleton walk of `bd090`, the node records
//! turned into node matrices by `sim_anim::build_matrices`
//! (docs/engine/skeleton-walk.md).
//!
//! [`markers`](markers) is phase 1c's marker slice: `b8d30`'s marker lookup
//! with the group scan and transform fill of `1d8f00` and `1d8f50` by
//! `sim_anim::model_markers` (docs/engine/markers.md).
//! [`palette`](palette) is phase 1c's first-person slice: the bone
//! palette of `16760c` by `sim_anim::palette` (docs/engine/palette.md).
//! [`unit_aim`] and [`first_person_filters`] run the complete caller
//! segments at `c7840` and `166d75`. The latter inlines the [`filter`]
//! step `11eed0` for the seat axes and weapon slots.
//!
//! [`graph_walk`] is phase 1b's other loop: the animation graph's node
//! walk `1dd1c0` by `sim_anim::graph_matrices`, checked against the guest
//! at every call by its arm (docs/engine/skeleton-walk.md, "The graph walk
//! `1dd1c0`").
mod segments;
pub use segments::{FIRST_PERSON_FILTER_SITES, UNIT_AIM_SITES, first_person_filters, unit_aim};
pub mod facial;
pub mod filter;
pub mod graph_walk;
pub mod manager;
pub mod markers;
pub mod palette;
pub mod skeleton;

use crate::function_eval::constant::{ONE, SMALL};
use crate::sim_native::Memory;
use ::sim_anim::{
    ClipStore, Effects, Frame, Mode, Ops, Record, Request, RotOp, Secondary, Shape, Sizes, VecOp,
    Which, kernel,
};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_core::det::{self, Fp};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

pub const STREAM: u32 = 0x0027_98a0;
/// The codec table: 40 bytes per codec, the integer-frame functions
/// (rotation, translation, scale) at `+0`, the interpolating ones at `+12`.
pub const CODECS: u32 = 0x0047_fb24;
const NEAR_ONE: u32 = 0x0045_dc78;
const SIGN_MASK: u32 = 0x0047_ffc0;
/// Bytes the setup's stack frame spans below the entry `esp` (three
/// pushes and the dispatcher's frame); records there would alias it.
const STACK_BELOW: u32 = 0x100;
/// Guest regions the original writes during the call besides the records.
const SCRATCH: [(u32, u32); 4] = [
    (0x0050_4400, 0x0050_4500),
    (0x0055_e500, 0x0055_e600),
    (0x0055_eb80, 0x0055_ec00),
    (0x0050_2430, 0x0050_2434),
];

/// The process-wide clip store.
pub static STORE: LazyLock<ClipStore> = LazyLock::new(|| ClipStore::new(&sim_core::epoch::SIM));

/// Why the kernel declined a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// An SSE exception the guest's MXCSR leaves unmasked (`#XM` in the
    /// guest). Every other MXCSR runs: the call computes under it
    /// (`Fp::guest`) and writes back its flags.
    Float,
    /// A guest read faulted.
    Fault,
    /// No frames (`last < 0`) or a frame beyond `cvttss2si`'s range.
    Frames,
    /// A remapped skeleton (kinds 1 and 2).
    Remap,
    /// Mode 0 with an element that has direct-test entries (mode 4).
    DirectTest,
    /// The shape has no layout (negative sizes, too large).
    Layout,
    /// The record buffer is misaligned or aliases the inputs or scratch.
    Records,
    /// The kernel declined, and why ([`::sim_anim::Refusal`]).
    Kernel(::sim_anim::Refusal),
    /// The element's node count exceeds the view's, so the guest's walk
    /// would read flag bits past the view's nodes.
    Nodes,
}

impl Decline {
    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Fault => "fault",
            Decline::Frames => "frames",
            Decline::Remap => "remap",
            Decline::DirectTest => "direct-test",
            Decline::Layout => "layout",
            Decline::Records => "records",
            Decline::Kernel(_) => "kernel",
            Decline::Nodes => "nodes",
        }
    }
}

impl std::fmt::Display for Decline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Decline::Float => write!(f, "float: unmasked SSE exception"),
            Decline::Kernel(why) => write!(f, "kernel: {why}"),
            other => write!(f, "{}", other.name()),
        }
    }
}

/// Call counts since start, for timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub native: u64,
    pub inactive: u64,
}

static NATIVE: AtomicU64 = AtomicU64::new(0);
static INACTIVE: AtomicU64 = AtomicU64::new(0);

/// Guest memory changed under the store (map load, checkpoint revert or
/// restore, snapshot load): move the epoch and drop every derived clip now,
/// not at the next lookup, so a map's clips are not held after it unloads.
pub fn invalidate() {
    STORE.invalidate();
}

pub fn stats() -> Stats {
    Stats {
        native: NATIVE.load(Ordering::Relaxed),
        inactive: INACTIVE.load(Ordering::Relaxed),
    }
}

/// The rotation function behind a codec-table pointer.
pub fn rotation_op(f: u32) -> Option<RotOp> {
    Some(match f {
        0x0028_c470 => RotOp::Static16,
        0x0028_c960 => RotOp::Frame16,
        0x0028_c790 => RotOp::Blend16,
        0x0028_c710 | 0x002c_4d60 => RotOp::FrameRaw,
        0x0028_c530 => RotOp::BlendF32,
        0x0028_cdb0 => RotOp::KeyInt { wide: false },
        0x0028_d560 => RotOp::KeyInt { wide: true },
        0x0028_c9e0 => RotOp::KeyInterp { wide: false },
        0x0028_d180 => RotOp::KeyInterp { wide: true },
        _ => return None,
    })
}

/// The translation function behind a codec-table pointer.
pub fn translation_op(f: u32) -> Option<VecOp> {
    Some(match f {
        0x0028_c4e0 => VecOp::Static,
        0x0028_c750 | 0x002c_4da0 => VecOp::FrameRaw,
        0x0028_c880 | 0x0028_c5d0 => VecOp::FrameLerp,
        0x0028_cf40 => VecOp::KeyInt { wide: false },
        0x0028_d6f0 => VecOp::KeyInt { wide: true },
        0x0028_cb70 => VecOp::KeyInterp { wide: false },
        0x0028_d320 => VecOp::KeyInterp { wide: true },
        _ => return None,
    })
}

/// The scale function behind a codec-table pointer.
pub fn scale_op(f: u32) -> Option<VecOp> {
    Some(match f {
        0x0028_c510 => VecOp::Static,
        0x002c_4de0 => VecOp::FrameRaw,
        0x0028_c6b0 => VecOp::FrameLerp,
        0x0028_d090 => VecOp::KeyInt { wide: false },
        0x0028_d840 => VecOp::KeyInt { wide: true },
        0x0028_ccc0 => VecOp::KeyInterp { wide: false },
        0x0028_d470 => VecOp::KeyInterp { wide: true },
        _ => return None,
    })
}

type Step<T> = Result<T, Decline>;

fn read<T>(r: Result<T, GuestFault>) -> Step<T> {
    r.map_err(|_| Decline::Fault)
}

fn ops(g: &impl Memory, codec: u8, interpolate: bool) -> Step<Ops> {
    let entry = CODECS + 40 * u32::from(codec) + if interpolate { 12 } else { 0 };
    Ok(Ops {
        rotation: rotation_op(read(g.u32(entry))?),
        translation: translation_op(read(g.u32(entry + 4))?),
        scale: scale_op(read(g.u32(entry + 8))?),
    })
}

fn clamp(index: i32, last: i32) -> u32 {
    if index < 0 { 0 } else { index.min(last) as u32 }
}

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// What the call writes, decided before anything is written.
struct Plan {
    out: u32,
    records: Vec<Record>,
    written: Vec<u8>,
    effects: Effects,
}

/// `Ok(Err(mxcsr))`: the call is inactive (weight at or below the cutoff)
/// and writes nothing but the MXCSR its compare leaves.
fn plan(c: &CpuAbi, g: &impl Memory, store: &ClipStore) -> Step<Result<Plan, u32>> {
    // The setup and the evaluators run under the guest's MXCSR, whatever
    // it holds, as retail's SSE code does.
    let mut fp = Fp::guest(c.mxcsr);
    let sp = c.esp;
    let arg = |k: u32| read(g.u32(sp.wrapping_add(4 * k)));
    let xmm = |i: usize| u32::from_le_bytes(c.xmm[i][..4].try_into().unwrap());
    let (frame, weight) = (xmm(1), xmm(4));
    let cutoff = read(g.u32(SMALL))?;
    fp.check(weight);
    fp.check(cutoff);
    if !fp.exact() {
        return Err(Decline::Float);
    }
    if det::compare(weight, cutoff).is_le() {
        return Ok(Err(fp.mxcsr().unwrap_or(c.mxcsr)));
    }

    let (skeleton_cap, remap, mask_at, out) = (arg(3)?, arg(4)?, arg(5)?, arg(6)?);
    let element = arg(2)?;
    let mut interpolate = read(g.u8(sp.wrapping_add(28)))? != 0;
    let has_second = read(g.u8(sp.wrapping_add(32)))? != 0;
    let one = read(g.u32(ONE))?;
    let near_one = read(g.u32(NEAR_ONE))?;
    let sign = read(g.u32(SIGN_MASK))?;
    fp.check(near_one);

    let ty = read(g.u8(element.wrapping_add(0x10)))?;
    let element_nodes = i32::from(read(g.u8(element.wrapping_add(0x13)))?);
    let last = i32::from(read(g.u16(element.wrapping_add(0x14)))? as i16) - 1;
    if last < 0 || !det::truncates(frame) {
        return Err(Decline::Frames);
    }
    // `cvttss2si` of a fraction raises PE.
    let inexact = |x: u32| f32::from_bits(det::from_int(det::truncate(x))) != f32::from_bits(x);
    if inexact(frame) {
        fp.raise(simd::PE);
    }
    let i = clamp(det::truncate(frame), last);
    let mut f = Frame { a: i, b: i, w: 0 };
    let mut secondary = None;
    if has_second {
        let t = arg(9)?;
        if !det::truncates(t) {
            return Err(Decline::Frames);
        }
        if inexact(t) {
            fp.raise(simd::PE);
        }
        let j = det::truncate(t);
        let start = clamp(j, last);
        let frame2 = Frame {
            a: start,
            b: clamp(j.wrapping_add(1), last),
            w: fp.sub(t, det::from_int(start as i32)),
        };
        f.b = clamp(i as i32 + 1, last);
        f.w = fp.sub(frame, det::from_int(i as i32));
        secondary = Some(Secondary {
            frame: frame2,
            blend: arg(10)?,
        });
    } else if interpolate {
        if i as i32 == last {
            interpolate = false;
        } else {
            f.b = clamp(i as i32 + 1, last);
            let w = fp.sub(frame, det::from_int(i as i32));
            fp.check(w);
            if !fp.exact() {
                return Err(Decline::Float);
            }
            let tiny = det::compare(cutoff, w).is_gt();
            if tiny || det::compare(w, near_one).is_gt() {
                if !tiny {
                    f.a = i + 1;
                }
                interpolate = false;
            } else {
                f.w = w;
            }
        }
    }

    if remap != 0 {
        return Err(Decline::Remap);
    }
    let count = if (skeleton_cap as i32) <= element_nodes {
        skeleton_cap as i32
    } else {
        element_nodes
    };

    let full = det::compare(weight, near_one).is_gt();
    let (mode, w) = if ty == 1 {
        (if full { Mode::Add } else { Mode::AddScaled }, weight)
    } else if !full {
        (Mode::Blend, weight)
    } else {
        (Mode::Replace, one)
    };

    let view = c.esi;
    let data = read(g.u32(view))?;
    let sizes_at = read(g.u32(view.wrapping_add(4)))?;
    let view_nodes = read(g.u8(view.wrapping_add(8)))?;
    let sizes = Sizes(read(g.load::<16>(sizes_at))?);
    let has_static = ty == 0 && sizes.static_len() != 0;
    let has_animated = sizes.animated_len() != 0;
    if has_animated && mode == Mode::Replace && read(g.u32(element.wrapping_add(0x64)))? as i32 > 0
    {
        return Err(Decline::DirectTest);
    }
    if count > i32::from(view_nodes) {
        return Err(Decline::Nodes);
    }
    let shape = Shape {
        sizes,
        nodes: view_nodes,
    };
    let clip = store
        .get(sizes_at, shape, |layout| {
            let mut bytes = vec![0; layout.len as usize].into_boxed_slice();
            g.read_bytes(data, &mut bytes, 1).ok()?;
            Some(bytes)
        })
        .ok_or(Decline::Layout)?;
    let blob = (data, data.wrapping_add(clip.layout.len));

    let mut streams = [None, None];
    if has_static {
        let s = clip.stream(Which::Static).ok_or(Decline::Layout)?;
        streams[0] = Some(ops(g, s.codec, false)?);
    }
    if has_animated {
        let s = clip.stream(Which::Animated).ok_or(Decline::Layout)?;
        streams[1] = Some(ops(g, s.codec, interpolate)?);
    }

    let live = usize::try_from(count).unwrap_or(0);
    let words = live.div_ceil(32);
    let mask = if mask_at != 0 {
        let mut m = vec![0u32; words];
        for (k, w) in m.iter_mut().enumerate() {
            *w = read(g.u32(mask_at.wrapping_add(4 * k as u32)))?;
        }
        Some(m)
    } else {
        None
    };

    let mut records = vec![[0u32; 8]; live];
    if live > 0 {
        let span = (out, out.wrapping_add(32 * live as u32));
        let mut inputs = vec![
            blob,
            (sizes_at, sizes_at.wrapping_add(16)),
            (sp.wrapping_sub(STACK_BELOW), sp.wrapping_add(44)),
            (view, view.wrapping_add(12)),
            (element, element.wrapping_add(0x6c)),
        ];
        if mask_at != 0 {
            inputs.push((mask_at, mask_at.wrapping_add(4 * words as u32)));
        }
        if out & 15 != 0
            || span.1 < span.0
            || inputs.iter().chain(&SCRATCH).any(|&r| overlaps(span, r))
        {
            return Err(Decline::Records);
        }
        let mut bytes = vec![0u8; 32 * live];
        read(g.read_bytes(out, &mut bytes, 1))?;
        for (r, chunk) in records.iter_mut().zip(bytes.chunks_exact(32)) {
            *r = std::array::from_fn(|k| {
                u32::from_le_bytes(chunk[4 * k..4 * k + 4].try_into().unwrap())
            });
        }
    }

    if !fp.exact() {
        return Err(Decline::Float);
    }
    let request = Request {
        mode,
        weight: w,
        count,
        mask: mask.as_deref(),
        frame: f,
        secondary,
        one,
        sign,
        data_addr: data,
        streams,
        mxcsr: fp.mxcsr().unwrap_or(c.mxcsr),
    };
    let mut written = vec![0u8; live];
    let effects = ::sim_anim::sample(&request, &clip, &mut records, &mut written)
        .map_err(Decline::Kernel)?;
    Ok(Ok(Plan {
        out,
        records,
        written,
        effects,
    }))
}

fn finish(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi, GuestFault> {
    let target = g.u32(c.esp)?;
    c.esp = c.esp.wrapping_add(4 + 0x28);
    Ok(TransferAbi::ret(target))
}

/// `0x2798a0` natively. `Ok(Err(why))` when the kernel declines; nothing
/// has been written then.
pub fn stream(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    store: &ClipStore,
) -> Result<Result<TransferAbi, Decline>, GuestFault> {
    let plan = match plan(c, g, store) {
        Ok(p) => p,
        Err(why) => return Ok(Err(why)),
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(mxcsr) => {
            INACTIVE.fetch_add(1, Ordering::Relaxed);
            c.mxcsr = mxcsr;
            return finish(c, g).map(Ok);
        }
    };
    for (n, (rec, &bits)) in plan.records.iter().zip(&plan.written).enumerate() {
        if bits == 0 {
            continue;
        }
        let at = plan.out.wrapping_add(32 * n as u32);
        let bytes = |range: std::ops::Range<usize>| -> Vec<u8> {
            rec[range].iter().flat_map(|w| w.to_le_bytes()).collect()
        };
        if bits & kernel::ROTATION != 0 {
            g.write_bytes(at, &bytes(0..4), 1)?;
        }
        if bits & kernel::TRANSLATION != 0 {
            g.write_bytes(at.wrapping_add(0x10), &bytes(4..7), 1)?;
        }
        if bits & kernel::SCALE != 0 {
            g.write_bytes(at.wrapping_add(0x1c), &bytes(7..8), 1)?;
        }
    }
    c.mxcsr = plan.effects.mxcsr;
    if plan.effects.mmx {
        // The int16 codecs end with `emms`: TOP 0, every tag empty.
        c.x87.status &= !0x3800;
        c.x87.tag = 0xffff;
    }
    NATIVE.fetch_add(1, Ordering::Relaxed);
    finish(c, g).map(Ok)
}
