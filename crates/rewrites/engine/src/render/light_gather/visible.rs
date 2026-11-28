//! The visible-light list, `0x00031910`: the main view's light
//! candidates that pass the tests of `0x00031590`, as rows of
//! [`VisibleLight`], merged into the view's [`History`] and ranked and
//! faded there; the ranked rows go back over the candidates in the
//! caller's buffer and their count is the answer.

use super::guest::{NONE, Pool, Sphere, TAGS, at, lights, objects, ptr, tag};
use super::history::{self, History};
use super::{Refusal, Result, Writes};
use crate::light_constants::Fpu;
use guest_layout::memory::GuestMemory;

/// One row's bytes.
pub const VISIBLE_BYTES: usize = 0x14;
/// Rows the caller's buffer holds.
pub const CANDIDATE_ROWS: usize = 128;

/// The main view's light list header pointer (`MAIN_VIS + 0x10`).
const CANDIDATES: u32 = super::guest::MAIN_VIS + 0x10;
const FRAME: u32 = 0x004b_a034;
const OVERRIDE: u32 = 0x0051_0c50;
const VIEW_INDEX: u32 = 0x004b_9ed4;
const LOCAL_PLAYER: u32 = 0x004b_9ed8;
const CAMERA_CONTROLLER: u32 = 0x004e_8c44;
const FIRST_PERSON_CAMERA: u32 = 0x0023_c110;
const PLAYER_IDENTITIES: u32 = 0x004e_8c20;
const ACTIVE_SCENARIO: u32 = 0x004e_0350;
/// The view's depth plane (`0x00031a1a`..`0x00031a50`).
pub const PROJECTION_DEPTH: [u32; 4] = [0x004b_9e20, 0x004b_9e2c, 0x004b_9e38, 0x004b_9e44];
pub use guest_layout::points::PIXEL_SCALE;
pub const MIN_DEPTH: u32 = 0x0045_dc68;
pub const SIZE_FACTOR: u32 = 0x0045_dc0c;
const EPSILON: u32 = 0x0045_dbdc;
const DISTANCE_EPSILON: u32 = 0x0045_dbd8;
const COLOUR_THRESHOLD: u32 = 0x0045_dc1c;
const LIM_SLOPE: u32 = 0x0045_e0b0;
const LIGHT_OPTIONS: u32 = 0x0051_07e8;
/// The multiplayer engine globals (`points::GAME_ENGINE`); `+0xc14` the
/// engine index, `+0` a flags byte.
const GAME_ENGINE: u32 = 0x004e_9ae8;
/// The engine table (`points::ENGINE_TABLE`): one engine object per index.
const ENGINE_TABLE: u32 = 0x0055_e4d0;
const RENDER_MODE: u32 = 0x0048_5602;
const RECT_SLOPES: u32 = 0x0046_8774;
const RECIPROCAL_TWO: u32 = 0x0045_dbbc;
/// The ranking's limit and step at the call site (`0x00031b1c`,
/// `0x00031b21`): three lights, the single-precision 0.01.
const LIMIT: usize = 3;
const STEP: f32 = 0.01;

/// One row of the visible list (`0x14` bytes). The four bytes are kept
/// as the guest wrote them: the ranking tests each against zero, and an
/// object-shadow row's byte 3 is whatever its buffer held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VisibleLight {
    /// `+0`: the owner is the local player's first-person object.
    pub local_owner: u8,
    /// `+1`: definition bit 5.
    pub first_person: u8,
    /// `+2`: the owner's object type byte is 2.
    pub owner_kind_2: u8,
    /// `+3`: definition bit 20.
    pub keep: u8,
    /// `+4`: the light datum.
    pub light: u32,
    /// `+8`: the owner's root object.
    pub root_owner: u32,
    /// `+0xc`, f32 bits: the record's weight on the way in, the faded one
    /// on the way out.
    pub weight: u32,
    /// `+0x10`, f32 bits: the projected size.
    pub priority: u32,
}

impl VisibleLight {
    pub fn decode(b: &[u8; VISIBLE_BYTES]) -> Self {
        let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Self {
            local_owner: b[0],
            first_person: b[1],
            owner_kind_2: b[2],
            keep: b[3],
            light: word(4),
            root_owner: word(8),
            weight: word(0xc),
            priority: word(0x10),
        }
    }

    pub fn encode(&self) -> [u8; VISIBLE_BYTES] {
        let mut b = [0; VISIBLE_BYTES];
        b[..4].copy_from_slice(&[self.local_owner, self.first_person, self.owner_kind_2, self.keep]);
        for (o, v) in [(4, self.light), (8, self.root_owner), (0xc, self.weight), (0x10, self.priority)] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// Whether the float state is the one the ports compute under: every
/// SSE exception masked, round to nearest, no flush; the x87 at double
/// precision, round to nearest.
pub fn fp_supported(fpu: Fpu) -> Result<()> {
    if fpu.mxcsr & !0x3f != 0x1f80 || fpu.x87_control != 0x027f {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}

/// A light record that is live, lit and bright enough.
pub fn active(m: &dyn GuestMemory, light: u32) -> Result<bool> {
    if m.u8(at(light, 2)?)? & 2 == 0 {
        return Ok(false);
    }
    let eps = m.f32(EPSILON)?;
    if !(m.f32(at(light, 0x24)?)? > eps && m.f32(at(light, 0x100)?)? > eps) {
        return Ok(false);
    }
    let norm = |off| -> Result<f32> {
        let [x, y, z] = m.vec3(at(light, off)?)?;
        Ok((x * x + y * y) + z * z)
    };
    let threshold = m.f32(COLOUR_THRESHOLD)?;
    Ok(norm(0xd4)? > threshold || norm(0xe0)? > threshold)
}

/// The root of an object's parent chain (`+0x14` of each body).
pub fn root_owner(m: &dyn GuestMemory, mut owner: u32) -> Result<u32> {
    if owner == NONE {
        return Ok(NONE);
    }
    let objects = objects(m)?;
    for _ in 0..objects.end {
        let next = m.u32(at(objects.body(m, owner)?, 0x14)?)?;
        if next == NONE {
            return Ok(owner);
        }
        owner = next;
    }
    Err(Refusal::Layout)
}

fn is_local_owner(m: &dyn GuestMemory, owner: u32) -> Result<bool> {
    let player = m.u32(LOCAL_PLAYER)?;
    if owner == NONE || player == NONE {
        return Ok(false);
    }
    if player >= 4 {
        return Err(Refusal::Unsupported);
    }
    let camera = CAMERA_CONTROLLER + player * 320;
    let interpolation = m.f32(camera + 4)?;
    if m.u32(camera + 0xc)? != FIRST_PERSON_CAMERA || interpolation != m.f32(DISTANCE_EPSILON)? {
        return Ok(false);
    }
    let objects = objects(m)?;
    let root = objects.slot(m, root_owner(m, owner)?)?;
    if m.u8(at(root, 3)?)? != 0 {
        return Ok(false);
    }
    let body = ptr(m, at(root, 8)?)?;
    let table = ptr(m, PLAYER_IDENTITIES)?;
    Ok(m.u32(at(table, player * 4 + 0xc)?)? == m.u32(at(body, 0x13c)?)?)
}

fn visible_from_owner(m: &dyn GuestMemory, light: u32, flags: u32, owner: u32) -> Result<bool> {
    if flags & 0x60 == 0 {
        return Ok(true);
    }
    let local = m.u16(at(light, 0x54)?)? != u16::MAX && is_local_owner(m, owner)?;
    Ok((flags & 0x20 == 0 || local) && (flags & 0x40 == 0 || !local))
}

/// The main view's first four player-only lights pass this owner test
/// before the gather's own checks.
pub fn player_light_visible(m: &dyn GuestMemory, light: u32, flags: u32, owner: u32) -> Result<bool> {
    visible_from_owner(m, light, flags, owner)
}

/// `active` and the candidate tests of `0x00031590`.
pub fn candidate_eligible(m: &dyn GuestMemory, light: u32, definition: u32, flags: u32) -> Result<bool> {
    Ok(active(m, light)? && visible_candidate(m, light, definition, flags)?)
}

/// The candidate tests of `0x00031590` after `active`.
pub fn visible_candidate(m: &dyn GuestMemory, light: u32, definition: u32, flags: u32) -> Result<bool> {
    let owner = m.u32(at(light, 0x4c)?)?;
    if !visible_from_owner(m, light, flags, owner)? {
        return Ok(false);
    }
    if flags & 0x400 != 0 && m.u16(RENDER_MODE)? != 2 {
        return Ok(false);
    }
    if m.u16(at(light, 0x48)?)? == u16::MAX && flags & 0x40020 != 0x40020 {
        return Ok(false);
    }
    if m.u8(at(ptr(m, LIGHT_OPTIONS)?, 8)?)? == 0 {
        return Ok(false);
    }
    if flags & 0x100 == 0 {
        let engine = ptr(m, GAME_ENGINE)?;
        let index = m.u32(at(engine, 0xc14)?)?;
        if index >= 32 {
            return Err(Refusal::Layout);
        }
        if m.u32(at(ENGINE_TABLE, index * 4)?)? != 0 && m.u8(engine)? & 2 != 0 {
            return Ok(false);
        }
    }
    shape_visible(m, light, definition)
}

// 0x000c19f0 and 0x000c1a80 build a scratch light shape. The visible-list
// caller uses only radius for a point or the far distance and two widths
// for a volume. The shape used later by the render passes is built there.
fn shape_visible(m: &dyn GuestMemory, light: u32, definition: u32) -> Result<bool> {
    let tags = ptr(m, TAGS)?;
    let scenario_index = m.u32(at(light, 8)?)?;
    let mut override_at = None;
    if scenario_index != NONE {
        let scenario = ptr(m, ACTIVE_SCENARIO)?;
        let instances = ptr(m, at(scenario, 0xec)?)?;
        let row = at(instances, scenario_index.checked_mul(0x6c).ok_or(Refusal::Layout)?)?;
        if m.u8(at(row, 0x3e)?)? & 1 != 0 {
            override_at = Some(row);
        }
    }
    let shape = if let Some(row) = override_at { m.u16(at(row, 0x3c)?)? } else { m.u16(at(definition, 4)?)? };
    let scale = m.f32(at(light, 0xc4)?)?;
    let eps = m.f32(EPSILON)?;
    if shape == 0 {
        let start = m.f32(at(definition, 0x18)?)?;
        let end = m.f32(at(definition, 0x1c)?)?;
        let (min, max) = if let Some(row) = override_at {
            let radius = m.f32(at(row, 0x68)?)?;
            let zero = m.f32(DISTANCE_EPSILON)?;
            let other = if end == zero { radius } else { (end / start) * radius };
            (radius, other)
        } else {
            (start, end)
        };
        let (a, b) = (min * scale, max * scale);
        return Ok((if a > b { a } else { b }) > eps);
    }
    let (near, aspect, angle, far) = if let Some(row) = override_at {
        (m.f32(at(row, 0x58)?)?, m.f32(at(row, 0x5c)?)?, m.f32(at(row, 0x60)?)?, m.f32(at(row, 0x68)?)?)
    } else {
        (
            m.f32(at(definition, 0x20)?)?,
            m.f32(at(definition, 0x24)?)?,
            m.f32(at(definition, 0x28)?)?,
            m.f32(at(definition, 0x30)?)?,
        )
    };
    let far = far * scale;
    if far.partial_cmp(&eps) != Some(std::cmp::Ordering::Greater) {
        return Ok(false);
    }
    let texture = m.u32(at(definition, 0x7c)?)?;
    let aspect = if texture == NONE {
        aspect
    } else {
        let data = ptr(m, at(tag(m, tags, texture)?, 0x48)?)?;
        (f32::from(m.u16(at(data, 6)?)? as i16) / f32::from(m.u16(at(data, 4)?)? as i16)) * aspect
    };
    let (dx, dy) = if shape == 1 {
        let slopes = ptr(m, RECT_SLOPES)?;
        (m.f32(slopes)?, m.f32(at(slopes, 4)?)?)
    } else {
        let angle = if shape == 3 { 0.0 } else { angle };
        crate::scene_list_math::cone_slopes(angle, m.f32(RECIPROCAL_TWO)?, aspect).ok_or(Refusal::Unsupported)?
    };
    let width = dx * far + near;
    let height = dy * far + aspect * near;
    Ok(width * width + height * height > m.f32(LIM_SLOPE)?)
}

/// What `0x00031910` leaves: the view's history after the merge and the
/// ranking, the rows in the caller's buffer (the ranked rows over the
/// candidates), the count it answers, and the writes in order.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleList {
    pub view: u32,
    pub frame: u32,
    pub history: History,
    /// Every row the buffer holds after the call: the first `count` are
    /// the ranked rows, the rest the candidates past them.
    pub rows: Vec<VisibleLight>,
    pub count: u32,
    pub writes: Writes,
}

impl VisibleList {
    /// The ranked rows, in draw order.
    pub fn ranked(&self) -> &[VisibleLight] {
        &self.rows[..self.count as usize]
    }
}

fn candidates(m: &dyn GuestMemory) -> Result<Vec<VisibleLight>> {
    let list = super::guest::List::read(m, ptr(m, CANDIDATES)?)?;
    let mut out = Vec::new();
    if list.count == 0 {
        return Ok(out);
    }
    let lights = lights(m)?;
    let tags = ptr(m, TAGS)?;
    let handles = list.column(m, 0xc)?;
    let [x, y, z, w] = PROJECTION_DEPTH;
    let [x, y, z, w] = [m.f32(x)?, m.f32(y)?, m.f32(z)?, m.f32(w)?];
    let min = m.f32(MIN_DEPTH)?;
    let scale = m.f32(PIXEL_SCALE)?;
    let factor = m.f32(SIZE_FACTOR)?;
    for index in 0..list.count as u32 {
        let handle = m.u32(at(handles, index * 4)?)?;
        if handle == NONE {
            continue;
        }
        let light = lights.slot(m, handle)?;
        if !active(m, light)? {
            continue;
        }
        let definition = tag(m, tags, m.u32(at(light, 4)?)?)?;
        let flags = m.u32(definition)?;
        if !visible_candidate(m, light, definition, flags)? || out.len() == CANDIDATE_ROWS {
            continue;
        }
        let owner = m.u32(at(light, 0x4c)?)?;
        let root = root_owner(m, owner)?;
        let local = is_local_owner(m, root)?;
        let owner_type = if owner != NONE {
            let objects: Pool = objects(m)?;
            m.u8(at(objects.slot(m, owner)?, 3)?)?
        } else {
            u8::MAX
        };
        let sphere = Sphere::read(m, at(light, 0x18)?)?;
        let [px, py, pz] = sphere.centre;
        let depth = (((z * pz + x * px) + y * py) + w).abs();
        let depth = if depth > min { depth } else { min };
        let priority = ((scale / depth) * sphere.radius) * factor;
        let weight = m.f32(at(light, 0x100)?)?;
        if !priority.is_finite() || !weight.is_finite() {
            return Err(Refusal::Unsupported);
        }
        out.push(VisibleLight {
            local_owner: u8::from(local),
            first_person: ((flags >> 5) & 1) as u8,
            owner_kind_2: u8::from(owner_type == 2),
            keep: ((flags >> 20) & 1) as u8,
            light: handle,
            root_owner: root,
            weight: weight.to_bits(),
            priority: priority.to_bits(),
        });
    }
    Ok(out)
}

/// `0x00031910` with `output` its stack argument (the caller's buffer of
/// [`CANDIDATE_ROWS`] rows).
pub fn list(m: &dyn GuestMemory, output: u32, fpu: Fpu) -> Result<VisibleList> {
    fp_supported(fpu)?;
    let mut rows = candidates(m)?;

    let frame = m.u32(FRAME)?;
    let view = m.i32(VIEW_INDEX)?.clamp(0, 3) as u32;
    let history_addr = history::LIGHTS + view * history::STRIDE;
    let mut history = History::decode(m, history_addr)?;
    history::merge(&mut history, &rows, frame);

    let override_addr = m.u32(OVERRIDE)?;
    let freeze = override_addr != 0 && m.u8(at(override_addr, 5)?)? != 0;
    if !freeze {
        if !history.finite() {
            return Err(Refusal::Unsupported);
        }
        history::rank_and_fade(history.rows_mut(), frame, LIMIT, STEP);
    }
    let count = history.count;
    if rows.len() < count {
        rows.resize(count, VisibleLight::default());
    }
    for (row, past) in rows.iter_mut().zip(history.rows()) {
        *row = VisibleLight { weight: past.weight, ..past.visible };
    }
    let mut writes = Writes::new();
    history.encode(history_addr, &mut writes);
    writes.array(output, rows.iter().flat_map(|r| r.encode()).collect());
    Ok(VisibleList { view, frame, history, rows, count: count as u32, writes })
}
