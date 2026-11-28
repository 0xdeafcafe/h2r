//! The frame body of `0x00031c80`, `render_dynamic_lights`
//! (docs/graphics/native-light-gather.md, "The dynamic lights, step by
//! step", slice L2).
//!
//! [`plan`] reads and refuses before any write: the float state, the
//! visible list (step 1, computed whole), and every read the drawn
//! lights' steps make on the state the list leaves. [`run`] then
//! publishes the list and makes steps 2 to 16 for each row in the
//! guest's order, reading each value when the guest reads it. Steps 6,
//! 12 and 13 are the Rust leaves of [`super`], and a leaf that refuses
//! gives its step to its guest function; steps 7 to 9 are inline; steps
//! 5, 10, 11, 15 and 16 are guest calls. Step 14 (slice L3) is the
//! records reset `0x0002cb30` as a guest call (its hook sizes the
//! structure records' detail for the world caller), the cluster and
//! placement walks as [`world_walk::records`] in the light's kind, each
//! given to its guest function when it refuses, and the object records
//! `0x00133720` as a guest call. Step 16 is the passes `0x00031170` as a
//! guest call; [`run_with`] hands each light's [`Gathered`] and its
//! [`Passes`] to the caller first, which puts the light in its scene
//! (slice L5). Once it has written, the body finishes the frame.
//!
//! The guest keeps its locals in the `0x1518` bytes below its entry
//! `ESP` and hands its callees pointers into them; the body uses the
//! same bytes ([`slot`]) and makes every call below them
//! ([`GuestCall::reserve`]), so a callee sees the pointers and the stack
//! the guest would give it.

use std::cell::Cell;

use super::guest::{LIGHTS, MAIN_VIS, NONE, OBJECTS, TAGS};
use super::visible::{self, VisibleLight, VisibleList};
use super::{Refusal, Result, extra_caster, geometry, volume};
use guest_layout::entry::{Cx, EntryError, GuestCall, Regs, Ret};
use crate::light_constants::Fpu;
use guest_layout::memory::{GuestMemory, MemFault};
use crate::render_loop::world_walk::{self, Walk};

/// The render frame counter the shadow arms sample by.
const RENDER_FRAME: u32 = 0x004b_a034;

/// The frame below the entry `ESP`: the probe's `0x1508` and the four
/// saved registers.
pub const FRAME: u32 = 0x1518;

/// Offsets in the frame from its bottom, `S` = entry `ESP` - [`FRAME`].
pub mod slot {
    /// The visible list's argument (`0x00031c91`): the probe leaves `ESP`
    /// at [`SHADOW`], so the push lands just below it. With lights to
    /// draw, the saved `EBX` (`0x00031c9f`) overwrites it.
    pub const LIST_ARGUMENT: u32 = 0xc;
    /// The shadow byte; `0x00031660` is handed its address and
    /// `0x00031170` its dword, whose upper bytes are whatever the stack
    /// held: nothing in the frame writes them.
    pub const SHADOW: u32 = 0x10;
    /// The frusta count `0x000c1d90` writes (i16).
    pub const FRUSTA_COUNT: u32 = 0x18;
    /// The posed volume, `0x7c` bytes.
    pub const VOLUME: u32 = 0x34;
    /// The visible rows, up to 128 of `0x14` bytes.
    pub const ROWS: u32 = 0xb0;
    /// The frusta, up to six of `0x1bc` bytes.
    pub const FRUSTA: u32 = 0xab0;
}

/// The guest functions the body calls.
pub mod call {
    use guest_layout::points;
    /// The marker name of an owner's attachment: ECX the owner, AX the index.
    pub const REPOSE_MARKER: u32 = 0x000b_8c40;
    /// The first-person marker frame: ECX the owner; stack (marker,
    /// &position, &forward, &up), `ret 0x10`.
    pub const FIRST_PERSON_MARKER: u32 = 0x0016_640f;
    /// The re-posed basis: EAX &up, ECX &forward.
    pub const REPOSE_BASIS: u32 = 0x0003_1b40;
    pub const LIGHT_VOLUME: u32 = 0x000c_17f0;
    pub const LIGHT_FRUSTA: u32 = 0x000c_1d90;
    pub const SCENE_VISIBILITY: u32 = 0x0013_20f0;
    pub const GEOMETRY_FLAGS: u32 = points::LIGHT_GEOMETRY_FLAGS.base;
    pub const EXTRA_CASTER: u32 = points::LIGHT_EXTRA_CASTER.base;
    pub const RESET_RECORDS: u32 = points::RESET_SCENE_RECORDS.base;
    pub const CLUSTER_RECORDS: u32 = points::WORLD_CLUSTER_RECORDS.base;
    pub const PLACEMENT_RECORDS: u32 = points::WORLD_PLACEMENT_RECORDS.base;
    pub const OBJECT_RECORDS: u32 = points::OBJECT_RENDER_RECORDS.base;
    pub const PART_VISIBILITY: u32 = 0x0004_4720;
    pub const LIGHT_PASSES: u32 = points::RENDER_LIGHT_PASSES.base;
}

/// The guest's return address after each of its calls, which
/// [`call_site`] reports while the body makes that call.
pub mod site {
    pub const REPOSE_MARKER: u32 = 0x0003_1d67;
    pub const FIRST_PERSON_MARKER: u32 = 0x0003_1d6f;
    pub const REPOSE_BASIS: u32 = 0x0003_1db1;
    pub const LIGHT_VOLUME: u32 = 0x0003_1dcd;
    pub const LIGHT_FRUSTA: u32 = 0x0003_1e81;
    pub const SCENE_VISIBILITY: u32 = 0x0003_1eaf;
    pub const GEOMETRY_FLAGS: u32 = 0x0003_1ed2;
    pub const EXTRA_CASTER: u32 = 0x0003_1eea;
    pub const RESET_RECORDS: u32 = 0x0003_1ef5;
    pub const CLUSTER_RECORDS: u32 = 0x0003_1efa;
    pub const PLACEMENT_RECORDS: u32 = 0x0003_1f00;
    pub const OBJECT_RECORDS: u32 = 0x0003_1f09;
    pub const PART_VISIBILITY: u32 = 0x0003_1f0e;
    pub const LIGHT_PASSES: u32 = 0x0003_1f2b;
}

/// The word a row's weight must be above to be drawn (`0x00031cb6`).
const DRAWN_THRESHOLD: u32 = 0x0045_dc58;
/// The zero word the shadow scalar is compared with (`0x00031e1c`).
const ZERO: u32 = 0x0045_dbd8;
/// Set: no source visibility for the light's gather (`0x00031deb`).
const NO_SOURCE_VIS: u32 = 0x004b_a010;
/// Dynamic shadows on (`0x00031e0a`).
const SHADOWS_ON: u32 = 0x004b_a020;
/// The view's cluster, for an unclustered first-person light (`0x00031e4f`).
const VIEW_CLUSTER: u32 = 0x004b_9ee4;
/// The first-person lights' table: eight slots of `0x408` bytes, the
/// light datum at `+0x400`, and the count of slots taken.
pub const FIRST_PERSON_TABLE: u32 = 0x004c_6700;
pub const FIRST_PERSON_SLOT: u32 = 0x408;
pub const FIRST_PERSON_COUNT: u32 = 0x0050_9440;
const FIRST_PERSON_SLOTS: i32 = 8;
/// The visibility struct `0x001320f0` filled (`0x00031eea`).
const RECORDS_VIS: u32 = 0x004c_0b80;

thread_local! {
    /// Per thread: the body and the guest calls it makes run on the thread
    /// that runs the guest's render.
    static SITE: Cell<u32> = const { Cell::new(0) };
}

/// While the body makes a guest call, the return address the guest's
/// own call pushes ([`site`]); `None` otherwise. A hook that tests its
/// caller's return address reads this when its caller is the body.
pub fn call_site() -> Option<u32> {
    match SITE.get() {
        0 => None,
        s => Some(s),
    }
}

/// How a step with a Rust leaf was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    Rust,
    /// The leaf refused; its guest function ran.
    Guest(Refusal),
}

/// Step 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repose {
    None,
    /// The marker lookup ran; `marker` is its answer.
    Asked {
        marker: u32,
    },
}

/// What steps 3 to 15 did for one drawn light.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gathered {
    /// The light record `L`.
    pub record: u32,
    /// The `ligh` datum.
    pub definition: u32,
    pub definition_flags: u32,
    /// `L+0x4c` when `L+0x54` is not -1, else -1.
    pub owner: u32,
    pub root: u32,
    pub repose: Repose,
    pub volume: By,
    /// The first-person slot it took.
    pub slot: Option<i32>,
    /// The gather's shadow decision.
    pub shadow: bool,
    /// Restricted to the main view's clusters.
    pub source_vis: bool,
    pub cluster: i32,
    pub frusta: i16,
    /// The visibility struct `0x001320f0` answered.
    pub vis: u32,
    pub geometry: By,
    /// Step 13, for a definition with bit 22.
    pub extra_caster: Option<By>,
    /// Step 14: the cluster walk `0x00133390` and the placement walk
    /// `0x00133520` in the light's kind.
    pub clusters: By,
    pub placements: By,
    /// Step 16: what the passes are handed.
    pub passes: Passes,
}

/// Step 16's call of `0x00031170`: its registers and stack words, as
/// the body hands them (`0x00031f0e`..`0x00031f2b`). The scene's light
/// is made from these and the light's item lists
/// (docs/graphics/native-light-gather.md, slice L5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passes {
    /// EAX: the `ligh` datum, the record's `+4`.
    pub definition: u32,
    /// ECX: the posed volume at `S+0x34`.
    pub context: u32,
    /// Stack 0: the light datum, the row's `+4`.
    pub light: u32,
    /// Stack 1: the record's frame block, `L+0x84`.
    pub position: u32,
    /// Stack 2: the dword at `S+0x10`, the shadow byte in its low byte.
    pub shadow: u32,
    /// Stack 3: the row's weight, f32 bits.
    pub weight: u32,
}

impl Passes {
    /// The four stack words, in the order they are pushed from the top.
    pub fn args(&self) -> [u32; 4] {
        [self.light, self.position, self.shadow, self.weight]
    }
}

/// Called by [`run_with`] for each drawn light, after its gather and
/// before its passes' call.
pub type OnPasses<'a> = dyn FnMut(&Gathered) + Send + 'a;

/// One row of the visible list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameLight {
    pub row: VisibleLight,
    /// The row's weight is above the threshold word.
    pub drawn: bool,
    pub gathered: Option<Gathered>,
}

/// What one call of `0x00031c80` did, in draw order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightFrame {
    pub view: u32,
    pub render_frame: u32,
    pub lights: Vec<FrameLight>,
}

/// What [`run`] starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub entry_esp: u32,
    /// The caller's float state, which the record walks compute under.
    pub fpu: Fpu,
    /// Step 1, computed whole; its writes are the body's first.
    pub list: VisibleList,
}

impl Plan {
    fn bottom(&self) -> u32 {
        self.entry_esp.wrapping_sub(FRAME)
    }
}

/// A light's record, definition and owners as steps 3 and 4 read them.
#[derive(Clone, Copy, Debug)]
struct Light {
    record: u32,
    definition: u32,
    data: u32,
    attach: u16,
    owner: u32,
    root: u32,
}

impl Light {
    /// Steps 3 and 4 (`0x00031cc6`..`0x00031d3d`): no salt is checked.
    /// `None` for an owner chain that does not end.
    fn read(m: &dyn GuestMemory, datum: u32) -> Result<Option<Self>, MemFault> {
        let data = m.u32(m.u32(LIGHTS)?.wrapping_add(0x44))?;
        let record = data.wrapping_add((datum & 0xffff).wrapping_mul(0x110));
        let definition = m.u32(record.wrapping_add(4))?;
        let tags = m.u32(TAGS)?;
        let data = m.u32(
            tags.wrapping_add((definition & 0xffff) << 4)
                .wrapping_add(8),
        )?;
        let attach = m.u16(record.wrapping_add(0x54))?;
        let (owner, root) = if attach == 0xffff {
            (NONE, NONE)
        } else {
            let owner = m.u32(record.wrapping_add(0x4c))?;
            match root_of(m, owner)? {
                Some(root) => (owner, root),
                None => return Ok(None),
            }
        };
        Ok(Some(Self {
            record,
            definition,
            data,
            attach,
            owner,
            root,
        }))
    }

    fn flags(&self, m: &dyn GuestMemory) -> Result<u32, MemFault> {
        m.u32(self.data)
    }

    /// Step 5's condition (`0x00031d37`..`0x00031d49`).
    fn repose_due(&self, flags: u32) -> bool {
        self.attach != 0xffff && flags & 0x20 != 0 && flags & 0x4_0000 == 0
    }
}

/// The root of `owner`'s parent chain (`0x00031d20`): the object headers'
/// bodies' `+0x14`, no salt checked.
fn root_of(m: &dyn GuestMemory, owner: u32) -> Result<Option<u32>, MemFault> {
    if owner == NONE {
        return Ok(Some(NONE));
    }
    let data = m.u32(m.u32(OBJECTS)?.wrapping_add(0x44))?;
    let mut handle = owner;
    for _ in 0..0x1_0000 {
        let body = m.u32(
            data.wrapping_add((handle & 0xffff).wrapping_mul(12))
                .wrapping_add(8),
        )?;
        let parent = m.u32(body.wrapping_add(0x14))?;
        if parent == NONE {
            return Ok(Some(handle));
        }
        handle = parent;
    }
    Ok(None)
}

fn drawn(weight: u32, threshold: f32) -> bool {
    // comiss, jbe: a NaN is not drawn.
    f32::from_bits(weight) > threshold
}

/// Reads and refuses before any write. Each drawn light's steps 3 to 16
/// read what is checked here; a re-pose that is due needs an owner.
pub fn plan(m: &dyn GuestMemory, entry_esp: u32, fpu: Fpu) -> Result<Plan> {
    visible::fp_supported(fpu)?;
    let bottom = entry_esp.wrapping_sub(FRAME);
    // The frame must be there to be written.
    m.bytes(bottom, FRAME as usize)?;
    let list = visible::list(m, bottom.wrapping_add(slot::ROWS), fpu)?;
    let threshold = m.f32(DRAWN_THRESHOLD)?;
    for &global in &[
        ZERO,
        NO_SOURCE_VIS,
        SHADOWS_ON,
        VIEW_CLUSTER,
        FIRST_PERSON_COUNT,
        RECORDS_VIS,
    ] {
        m.u32(global)?;
    }
    for row in list.ranked() {
        if !drawn(row.weight, threshold) {
            continue;
        }
        let light = Light::read(m, row.light)?.ok_or(Refusal::Layout)?;
        let flags = light.flags(m)?;
        if light.repose_due(flags) && light.owner == NONE {
            return Err(Refusal::Unsupported);
        }
        let r = light.record;
        for offset in [0x18, 0x24, 0x48, 0x84, 0x90, 0xac, 0xb8, 0xf8] {
            m.bytes(r.wrapping_add(offset), 12)?;
        }
    }
    Ok(Plan {
        entry_esp,
        fpu,
        list,
    })
}

/// Clears [`call_site`] when the call returns, or when the body is
/// dropped in it (a load).
struct Clear;

impl Drop for Clear {
    fn drop(&mut self) {
        SITE.set(0);
    }
}

/// A guest call below the frame, with [`call_site`] set while it runs.
async fn guest(cx: &Cx, site: u32, call: GuestCall) -> Result<Regs, EntryError> {
    SITE.set(site);
    let _clear = Clear;
    cx.call(call.reserve(FRAME)).await
}

/// Publishes the plan's writes and runs the lights. Returns as
/// `0x00031c80` does (`ret`, EAX 0) with what it did.
pub async fn run(cx: &Cx, plan: Plan) -> Result<(Ret, LightFrame), EntryError> {
    run_with(cx, plan, &mut |_: &Gathered| {}).await
}

/// [`run`], handing each drawn light's [`Gathered`] (with its
/// [`Passes`]) to `on_passes` just before its passes' guest call, so the
/// caller can put the light in its scene before the passes draw it.
pub async fn run_with(
    cx: &Cx,
    plan: Plan,
    on_passes: &mut OnPasses<'_>,
) -> Result<(Ret, LightFrame), EntryError> {
    let bottom = plan.bottom();
    // 0x00031c91: the list's argument, pushed below the probed frame.
    cx.write(|m| {
        m.set_u32(
            bottom.wrapping_add(slot::LIST_ARGUMENT),
            bottom.wrapping_add(slot::ROWS),
        )
    })?;
    cx.write(|m| plan.list.writes.apply(m))?;
    let mut frame = LightFrame {
        view: plan.list.view,
        render_frame: plan.list.frame,
        lights: Vec::new(),
    };
    for i in 0..plan.list.count {
        let row_at = bottom
            .wrapping_add(slot::ROWS)
            .wrapping_add(i * visible::VISIBLE_BYTES as u32);
        let (row, threshold) = cx.read(|m| -> Result<_, MemFault> {
            let bytes = m.bytes(row_at, visible::VISIBLE_BYTES)?;
            let row = VisibleLight::decode(bytes.as_slice().try_into().expect("a row's bytes"));
            Ok((row, m.f32(DRAWN_THRESHOLD)?))
        })?;
        if !drawn(row.weight, threshold) {
            frame.lights.push(FrameLight {
                row,
                drawn: false,
                gathered: None,
            });
            continue;
        }
        let gathered = light(cx, bottom, plan.fpu, row_at, row.light, on_passes).await?;
        frame.lights.push(FrameLight {
            row,
            drawn: true,
            gathered: Some(gathered),
        });
    }
    Ok((Ret::new(0, 0), frame))
}

/// Steps 3 to 16 for the drawn row at `row_at`.
async fn light(
    cx: &Cx,
    bottom: u32,
    fpu: Fpu,
    row_at: u32,
    datum: u32,
    on_passes: &mut OnPasses<'_>,
) -> Result<Gathered, EntryError> {
    let light = cx.read(|m| Light::read(m, datum))?;
    // The plan checked the chain; nothing the body has called since
    // writes object parents.
    let Some(l) = light else {
        return Err(EntryError::IsolatedCall(format!(
            "light {datum:#x}: its owner chain does not end"
        )));
    };
    let rec = l.record;
    let volume_at = bottom.wrapping_add(slot::VOLUME);
    let shadow_at = bottom.wrapping_add(slot::SHADOW);
    let frusta_at = bottom.wrapping_add(slot::FRUSTA);
    let count_at = bottom.wrapping_add(slot::FRUSTA_COUNT);
    let (position, forward, up) = (
        rec.wrapping_add(0x84),
        rec.wrapping_add(0xac),
        rec.wrapping_add(0xb8),
    );

    // Step 5: the re-pose.
    let flags = cx.read(|m| l.flags(m))?;
    let mut repose = Repose::None;
    if l.repose_due(flags) {
        let eax = (((l.definition & 0xffff) << 4) & 0xffff_0000) | u32::from(l.attach);
        let request = GuestCall::new(call::REPOSE_MARKER)
            .ecx(l.owner)
            .eax(eax)
            .args([position, forward, up]);
        let marker = guest(cx, site::REPOSE_MARKER, request).await?.eax;
        let request = GuestCall::new(call::FIRST_PERSON_MARKER)
            .ecx(l.owner)
            .args([marker, position, forward, up]);
        guest(cx, site::FIRST_PERSON_MARKER, request).await?;
        // 0x00031d6f: the target is the position plus the forward vector.
        cx.write(|m| -> Result<(), MemFault> {
            for c in 0..3 {
                let v =
                    m.f32(position.wrapping_add(4 * c))? + m.f32(forward.wrapping_add(4 * c))?;
                m.set_f32(rec.wrapping_add(0x90 + 4 * c), v)?;
            }
            Ok(())
        })?;
        let request = GuestCall::new(call::REPOSE_BASIS).eax(up).ecx(forward);
        guest(cx, site::REPOSE_BASIS, request).await?;
        repose = Repose::Asked { marker };
    }

    // Step 6: the volume.
    let volume_by = match cx.read(|m| volume::light_volume(m, volume_at, datum, 1)) {
        Ok(posed) => {
            cx.write(|m| posed.writes.apply(m))?;
            By::Rust
        }
        Err(why) => {
            let request = GuestCall::new(call::LIGHT_VOLUME)
                .eax(volume_at)
                .edx(datum)
                .args([1]);
            guest(cx, site::LIGHT_VOLUME, request).await?;
            By::Guest(why)
        }
    };

    // Step 7: the first-person slot.
    let slot = cx.write(|m| -> Result<Option<i32>, MemFault> {
        let n = m.i32(FIRST_PERSON_COUNT)?;
        if n >= FIRST_PERSON_SLOTS {
            return Ok(None);
        }
        let at = FIRST_PERSON_TABLE
            .wrapping_add(0x400)
            .wrapping_add((n as u32).wrapping_mul(FIRST_PERSON_SLOT));
        m.set_u32(at, datum)?;
        m.set_i32(FIRST_PERSON_COUNT, n.wrapping_add(1))?;
        Ok(Some(n))
    })?;

    // Steps 8 and 9: the shadow byte, the source visibility, the cluster.
    let (flags, shadow, source, cluster) = cx.write(|m| -> Result<_, MemFault> {
        let source = if m.u8(NO_SOURCE_VIS)? != 0 {
            0
        } else {
            MAIN_VIS
        };
        let flags = l.flags(m)?;
        let shadow = flags & 8 == 0
            && m.u8(SHADOWS_ON)? != 0
            && m.f32(rec.wrapping_add(0xf8))? > m.f32(ZERO)?
            && m.u16(volume_at)? != 0;
        m.set_u8(shadow_at, u8::from(shadow))?;
        let own = m.u16(rec.wrapping_add(0x48))? as i16 as i32;
        let cluster = if own == -1 && flags & 0x20 != 0 && flags & 0x4_0000 != 0 {
            m.i32(VIEW_CLUSTER)?
        } else {
            own
        };
        Ok((flags, shadow, source, cluster))
    })?;

    // Step 10: the frusta, Rust's once its shadow arm is proven
    // (`crate::light_frusta`); a sampled call is the guest's and is
    // compared, and a refusal is the guest's for that call.
    let bits = (flags >> 12) & 0x400;
    let frusta_call = crate::light_frusta::Call::from_registers(
        frusta_at,
        volume_at,
        count_at,
        bits,
        [position, volume_at, frusta_at, count_at],
    );
    let frame_now = cx.read(|m| m.u32(RENDER_FRAME)).unwrap_or(0);
    let frusta_plan = crate::light_frusta::arm::plan(frame_now);
    let built = match frusta_plan {
        crate::shadow_arm::Plan::Guest => None,
        _ => match cx.read(|m| crate::light_frusta::build(m, &frusta_call, fpu.mxcsr, fpu.x87_control)) {
            Ok(built) => Some(built),
            Err(why) => {
                crate::light_frusta::arm::refuse(why.name());
                None
            }
        },
    };
    match (frusta_plan, built) {
        (crate::shadow_arm::Plan::Answer, Some(built)) => {
            cx.write(|m| built.proposal.apply(m))?;
            crate::light_frusta::arm::answered();
        }
        (plan, built) => {
            let request = GuestCall::new(call::LIGHT_FRUSTA)
                .eax(frusta_at)
                .ecx(volume_at)
                .edx(count_at)
                .esi(bits)
                .args([position, volume_at, frusta_at, count_at]);
            guest(cx, site::LIGHT_FRUSTA, request).await?;
            if let (crate::shadow_arm::Plan::Compare, Some(built)) = (plan, built) {
                cx.read(|m| crate::light_frusta::arm::compare(&built.proposal, m, frame_now, datum));
            }
        }
    }

    // Step 11: the light's visibility.
    let (frusta, extent) = cx.read(|m| -> Result<_, MemFault> {
        Ok((m.u16(count_at)? as i16, m.u32(rec.wrapping_add(0x24))?))
    })?;
    let bounds = rec.wrapping_add(0x18);
    let request = GuestCall::new(call::SCENE_VISIBILITY)
        .eax(1)
        .ecx(i32::from(frusta) as u32)
        .edx(bounds & !0xff)
        .esi(bits)
        .args([bits, frusta_at, bounds, extent, source, 0, cluster as u32]);
    let visibility = guest(cx, site::SCENE_VISIBILITY, request).await?;
    let vis = visibility.eax;

    // Step 12: the geometry flags. Their point byte is SETE over the EDX
    // the visibility call left.
    let point = (visibility.edx & !0xff) | u32::from(cx.read(|m| m.u16(volume_at))? == 0);
    let args = geometry::Arguments {
        light: datum,
        definition: l.definition,
        vis,
        owner: l.owner,
        root: l.root,
        point,
        shadow: shadow_at,
    };
    let mut flags_eax = 0;
    let geometry_by = match cx.read(|m| geometry::flags(m, args)) {
        Ok(writes) => {
            cx.write(|m| writes.apply(m))?;
            By::Rust
        }
        Err(why) => {
            let request = GuestCall::new(call::GEOMETRY_FLAGS)
                .eax(datum)
                .ecx(l.definition)
                .esi(vis)
                .args([l.owner, l.root, point, shadow_at]);
            flags_eax = guest(cx, site::GEOMETRY_FLAGS, request).await?.eax;
            By::Guest(why)
        }
    };

    // Step 13: the extra caster.
    let extra_caster_by = if cx.read(|m| l.flags(m))? & 0x40_0000 != 0 {
        let al = cx.read(|m| m.u8(shadow_at))?;
        Some(match cx.read(|m| extra_caster::list(m, al, l.owner, vis)) {
            Ok(writes) => {
                cx.write(|m| writes.apply(m))?;
                By::Rust
            }
            Err(why) => {
                let request = GuestCall::new(call::EXTRA_CASTER)
                    .eax((flags_eax & !0xff) | u32::from(al))
                    .ecx(l.owner)
                    .args([vis]);
                guest(cx, site::EXTRA_CASTER, request).await?;
                By::Guest(why)
            }
        })
    } else {
        None
    };

    // Steps 14 and 15: the records and their part visibility. The reset
    // stays the guest's: in the light's kind it is one write
    // (`[0x4c0b78]` = `[0x4c0b7c]`, 0x0002cba7), and the hook on it sizes
    // the walks' detail for the world caller (lod_distance).
    let records = cx.read(|m| m.u32(RECORDS_VIS))?;
    guest(
        cx,
        site::RESET_RECORDS,
        GuestCall::new(call::RESET_RECORDS).ebx(records),
    )
    .await?;
    let clusters = walk(
        cx,
        records,
        Walk::Clusters,
        fpu,
        site::CLUSTER_RECORDS,
        GuestCall::new(call::CLUSTER_RECORDS).ebx(records),
    )
    .await?;
    let placements = walk(
        cx,
        records,
        Walk::Placements,
        fpu,
        site::PLACEMENT_RECORDS,
        GuestCall::new(call::PLACEMENT_RECORDS)
            .ebx(records)
            .args([records]),
    )
    .await?;
    let request = GuestCall::new(call::OBJECT_RECORDS)
        .eax(records)
        .ebx(records)
        .args([1]);
    guest(cx, site::OBJECT_RECORDS, request).await?;
    guest(
        cx,
        site::PART_VISIBILITY,
        GuestCall::new(call::PART_VISIBILITY).ebx(records),
    )
    .await?;

    // Step 16: the passes.
    let (weight, shadow_word, light_datum, definition) = cx.read(|m| -> Result<_, MemFault> {
        Ok((
            m.u32(row_at.wrapping_add(0xc))?,
            m.u32(shadow_at)?,
            m.u32(row_at.wrapping_add(4))?,
            m.u32(rec.wrapping_add(4))?,
        ))
    })?;
    let passes = Passes {
        definition,
        context: volume_at,
        light: light_datum,
        position,
        shadow: shadow_word,
        weight,
    };
    let gathered = Gathered {
        record: rec,
        definition: l.definition,
        definition_flags: flags,
        owner: l.owner,
        root: l.root,
        repose,
        volume: volume_by,
        slot,
        shadow,
        source_vis: source != 0,
        cluster,
        frusta,
        vis,
        geometry: geometry_by,
        extra_caster: extra_caster_by,
        clusters,
        placements,
        passes,
    };
    on_passes(&gathered);
    let request = GuestCall::new(call::LIGHT_PASSES)
        .eax(passes.definition)
        .ecx(passes.context)
        .ebx(records)
        .args(passes.args());
    guest(cx, site::LIGHT_PASSES, request).await?;
    Ok(gathered)
}

/// One record walk of step 14: Rust when [`world_walk::records`] takes
/// it, which writes nothing when it refuses; else `request`, the guest
/// function, whose own hooks retry it row by row.
async fn walk(
    cx: &Cx,
    vis: u32,
    walk: Walk,
    fpu: Fpu,
    site: u32,
    request: GuestCall,
) -> Result<By, EntryError> {
    let made = cx.write(|m| world_walk::records(m, vis, walk, fpu.mxcsr, fpu.x87_control));
    match made {
        Ok(_) => Ok(By::Rust),
        Err(why) => {
            guest(cx, site, request).await?;
            Ok(By::Guest(match why {
                world_walk::Refused::Float => Refusal::Unsupported,
                world_walk::Refused::Cold => Refusal::Residency,
                world_walk::Refused::Layout => Refusal::Layout,
                world_walk::Refused::Write => Refusal::Memory,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_slots_fit_the_frame() {
        assert_eq!(slot::ROWS + 128 * 0x14, slot::FRUSTA);
        assert_eq!(slot::FRUSTA + 6 * 0x1bc, FRAME);
        assert_eq!(slot::VOLUME + volume::BYTES as u32, slot::ROWS);
    }

    #[test]
    fn a_nan_weight_is_not_drawn() {
        assert!(!drawn(f32::NAN.to_bits(), 0.0));
        assert!(!drawn(0.0f32.to_bits(), 0.0));
        assert!(drawn(0.5f32.to_bits(), 0.0));
    }
}
