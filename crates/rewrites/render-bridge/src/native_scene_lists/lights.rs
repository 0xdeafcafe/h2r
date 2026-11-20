use super::objects::{List, Row, Sphere};
use super::*;

const LIGHTS: u32 = engine::render::light::ARRAY_POINTER;
const LIGHT_BYTES: u32 = engine::render::light::RECORD_SIZE as u32;
const LIGHT_EPOCH: u32 = 0x004e_0308;
const LIGHT_HISTORY: u32 = 0x0052_34e8;
const HISTORY_ROWS: usize = 32;
const CANDIDATE_ROWS: usize = 128;
const LIGHT_CANDIDATES: u32 = objects::MAIN_VIS + 0x10;
const LIGHT_OVERRIDE: u32 = 0x0051_0c50;
const VIEW_INDEX: u32 = 0x004b_9ed4;
const LOCAL_PLAYER: u32 = 0x004b_9ed8;
const CAMERA_CONTROLLER: u32 = 0x004e_8c44;
const FIRST_PERSON_CAMERA: u32 = 0x0023_c110;
const PLAYER_IDENTITIES: u32 = 0x004e_8c20;
const ACTIVE_SCENARIO: u32 = 0x004e_0350;
pub(super) const PROJECTION_DEPTH: [u32; 4] = [0x004b_9e20, 0x004b_9e2c, 0x004b_9e38, 0x004b_9e44];
pub(super) use engine::points::PIXEL_SCALE;
pub(super) const MIN_DEPTH: u32 = 0x0045_dc68;
pub(super) const SIZE_FACTOR: u32 = 0x0045_dc0c;
const EPSILON: u32 = 0x0045_dbdc;
const DISTANCE_EPSILON: u32 = 0x0045_dbd8;
const COLOUR_THRESHOLD: u32 = 0x0045_dc1c;
const LIM_SLOPE: u32 = 0x0045_e0b0;
const LIGHT_OPTIONS: u32 = 0x0051_07e8;
const VISIBILITY_OPTIONS: u32 = 0x004e_9ae8;
const LIGHT_BSP_OVERRIDES: u32 = 0x0055_e4d0;
const RENDER_MODE: u32 = 0x0048_5602;
const RECT_SLOPES: u32 = 0x0046_8774;
const RECIPROCAL_TWO: u32 = 0x0045_dbbc;
const ONE: u32 = 0x0045_dbc0;

fn pool(m: &dyn GuestMemory) -> Result<Pool> {
    Pool::read(m, ptr(m, LIGHTS)?, LIGHT_BYTES)
}

fn active(m: &dyn GuestMemory, light: u32) -> Result<bool> {
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

pub(super) fn root_owner(m: &dyn GuestMemory, mut owner: u32) -> Result<u32> {
    if owner == NONE {
        return Ok(NONE);
    }
    let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
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
    let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
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

pub(super) struct Candidates<'a> {
    m: &'a dyn GuestMemory,
    list: List,
    lights: Pool,
    rows: Vec<Row>,
    epoch: u32,
    stamped: Vec<bool>,
}

impl<'a> Candidates<'a> {
    pub(super) fn read(m: &'a dyn GuestMemory, vis: u32) -> Result<Self> {
        let list = List::read(m, ptr(m, at(vis, 0x10)?)?)?;
        if list.count != 0 {
            return Err(Refusal::Unsupported);
        }
        let lights = pool(m)?;
        Ok(Self {
            m,
            list,
            lights,
            rows: Vec::new(),
            epoch: m.u32(LIGHT_EPOCH)?,
            stamped: vec![false; lights.end as usize],
        })
    }

    pub(super) fn add(
        &mut self,
        handle: u32,
        inside: impl FnOnce(Sphere) -> Result<bool>,
    ) -> Result<()> {
        let row = self.lights.slot(self.m, handle)?;
        let index = (handle & 0xffff) as usize;
        if !self.stamped[index]
            && active(self.m, row)?
            && self.m.u32(at(row, 0xc)?)? != self.epoch
            && inside(Sphere::read(self.m, at(row, 0x18)?)?)?
        {
            self.append(handle, index);
        }
        Ok(())
    }

    fn append(&mut self, handle: u32, index: usize) {
        if self.rows.len() + 1 < self.list.capacity {
            self.rows.push(Row::plain(handle));
            self.stamped[index] = true;
        }
    }

    // The main view also tries the first four player-only lights from the
    // allocated datum bitmap. Apply the unclustered/active tests afterwards.
    pub(super) fn add_player_lights(&mut self) -> Result<()> {
        let m = self.m;
        let header = ptr(m, LIGHTS)?;
        let bitmap = if self.lights.end == 0 {
            0
        } else {
            ptr(m, at(header, 0x48)?)?
        };
        let tags = ptr(m, TAGS)?;
        let mut found = 0;
        for index in 0..self.lights.end {
            if m.u32(at(bitmap, (index / 32) * 4)?)? & (1 << (index % 32)) == 0 {
                continue;
            }
            let row = at(self.lights.data, index * LIGHT_BYTES)?;
            let datum = m.u32(at(row, 4)?)?;
            if datum == NONE {
                continue;
            }
            let flags = m.u32(tag(m, tags, datum)?)?;
            const PLAYER_LIGHT: u32 = 0x0004_0020;
            if flags & PLAYER_LIGHT != PLAYER_LIGHT
                || !visible_from_owner(m, row, flags, m.u32(at(row, 0x4c)?)?)?
            {
                continue;
            }
            let handle = u32::from(m.u16(row)?) << 16 | index;
            let index = index as usize;
            if m.u16(at(row, 0x48)?)? == u16::MAX
                && active(m, row)?
                && !self.stamped[index]
                && m.u32(at(row, 0xc)?)? != self.epoch
            {
                self.append(handle, index);
            }
            found += 1;
            if found == 4 {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn publish(&self, out: &mut Output) -> Result<()> {
        self.list.publish(self.m, out, &self.rows)?;
        for (index, &changed) in self.stamped.iter().enumerate() {
            if changed {
                out.word(
                    at(self.lights.data, index as u32 * LIGHT_BYTES + 0xc)?,
                    self.epoch,
                );
            }
        }
        Ok(())
    }
}

type Visible = [u8; 0x14];
type History = [u8; 0x24];

fn word(row: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(row[offset..offset + 4].try_into().unwrap())
}

fn put(row: &mut [u8], offset: usize, value: u32) {
    row[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn scalar(row: &[u8], offset: usize) -> f32 {
    f32::from_bits(word(row, offset))
}

fn visible_candidate(m: &dyn GuestMemory, light: u32, definition: u32, flags: u32) -> Result<bool> {
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
        let vis = ptr(m, VISIBILITY_OPTIONS)?;
        let bsp = m.u32(at(vis, 0xc14)?)?;
        if bsp >= 32 {
            return Err(Refusal::Layout);
        }
        if m.u32(at(LIGHT_BSP_OVERRIDES, bsp * 4)?)? != 0 && m.u8(vis)? & 2 != 0 {
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
        let row = at(
            instances,
            scenario_index.checked_mul(0x6c).ok_or(Refusal::Layout)?,
        )?;
        if m.u8(at(row, 0x3e)?)? & 1 != 0 {
            override_at = Some(row);
        }
    }
    let shape = if let Some(row) = override_at {
        m.u16(at(row, 0x3c)?)?
    } else {
        m.u16(at(definition, 4)?)?
    };
    let scale = m.f32(at(light, 0xc4)?)?;
    let eps = m.f32(EPSILON)?;
    if shape == 0 {
        let start = m.f32(at(definition, 0x18)?)?;
        let end = m.f32(at(definition, 0x1c)?)?;
        let (min, max) = if let Some(row) = override_at {
            let radius = m.f32(at(row, 0x68)?)?;
            let zero = m.f32(DISTANCE_EPSILON)?;
            let other = if end == zero {
                radius
            } else {
                (end / start) * radius
            };
            (radius, other)
        } else {
            (start, end)
        };
        let (a, b) = (min * scale, max * scale);
        return Ok((if a > b { a } else { b }) > eps);
    }
    let (near, aspect, angle, far) = if let Some(row) = override_at {
        (
            m.f32(at(row, 0x58)?)?,
            m.f32(at(row, 0x5c)?)?,
            m.f32(at(row, 0x60)?)?,
            m.f32(at(row, 0x68)?)?,
        )
    } else {
        (
            m.f32(at(definition, 0x20)?)?,
            m.f32(at(definition, 0x24)?)?,
            m.f32(at(definition, 0x28)?)?,
            m.f32(at(definition, 0x30)?)?,
        )
    };
    let far = far * scale;
    if !(far > eps) {
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
        engine::scene_list_math::cone_slopes(angle, m.f32(RECIPROCAL_TWO)?, aspect)
            .ok_or(Refusal::Unsupported)?
    };
    let width = dx * far + near;
    let height = dy * far + aspect * near;
    Ok(width * width + height * height > m.f32(LIM_SLOPE)?)
}

fn read_visible(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    fp_supported(cpu)?;
    if cpu.x87.control != 0x027f {
        return Err(Refusal::Unsupported);
    }
    let output_addr = stack(m, cpu, 4)?;
    let list = List::read(m, ptr(m, LIGHT_CANDIDATES)?)?;
    let mut candidates = Vec::new();
    if list.count != 0 {
        let lights = pool(m)?;
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
            if !visible_candidate(m, light, definition, flags)?
                || candidates.len() == CANDIDATE_ROWS
            {
                continue;
            }
            let owner = m.u32(at(light, 0x4c)?)?;
            let root = root_owner(m, owner)?;
            let local = is_local_owner(m, root)?;
            let owner_type = if owner != NONE {
                let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
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
            let mut row = [0; 0x14];
            row[0] = u8::from(local);
            row[1] = ((flags >> 5) & 1) as u8;
            row[2] = u8::from(owner_type == 2);
            row[3] = ((flags >> 20) & 1) as u8;
            put(&mut row, 4, handle);
            put(&mut row, 8, root);
            put(&mut row, 12, weight.to_bits());
            put(&mut row, 16, priority.to_bits());
            candidates.push(row);
        }
    }

    let frame = m.u32(FRAME)?;
    let view = m.i32(VIEW_INDEX)?.clamp(0, 3) as u32;
    let history_addr = LIGHT_HISTORY + view * 0x484;
    let mut count = m.u32(history_addr + 0x480)? as usize;
    if count > HISTORY_ROWS {
        return Err(Refusal::Layout);
    }
    let mut history = [[0; 0x24]; HISTORY_ROWS];
    let active = m.bytes(history_addr, count * 0x24)?;
    for (row, bytes) in history.iter_mut().zip(active.chunks_exact(0x24)) {
        row.copy_from_slice(bytes);
    }
    let written = merge_history(&mut history, &mut count, &candidates, frame);

    let override_addr = m.u32(LIGHT_OVERRIDE)?;
    let freeze = override_addr != 0 && m.u8(at(override_addr, 5)?)? != 0;
    if !freeze {
        if history[..count].iter().any(|row| {
            [0xc, 0x10, 0x14, 0x18]
                .into_iter()
                .any(|off| !scalar(row, off).is_finite())
        }) {
            return Err(Refusal::Unsupported);
        }
        // The call site passes the single-precision step 0.01.
        rank_and_fade(&mut history[..count], frame, 3, 0.01);
    }
    if candidates.len() < count {
        candidates.resize(count, [0; 0x14]);
    }
    for (row, past) in candidates.iter_mut().zip(&history[..count]) {
        row.copy_from_slice(&past[..0x14]);
        put(row, 0xc, word(past, 0x18));
    }
    let mut out = Output::default();
    out.array(
        history_addr,
        history[..written].iter().flatten().copied().collect(),
    );
    out.word(history_addr + 0x480, count as u32);
    out.array(output_addr, candidates.into_iter().flatten().collect());
    out.eax = Some(count as u32);
    Ok(out)
}

// Update previous slots by datum, swap out expired slots, and append
// new ones in candidate order. The guest's x86 mask uses the low five
// bits of the ordinal (even when there are more than 32 candidates).
fn merge_history(
    history: &mut [History; HISTORY_ROWS],
    count: &mut usize,
    candidates: &[Visible],
    frame: u32,
) -> usize {
    let mut seen = 0u32;
    let mut written = *count;
    for row in &mut history[..*count] {
        if let Some(i) = candidates.iter().position(|c| word(c, 4) == word(row, 4)) {
            row[..0x14].copy_from_slice(&candidates[i]);
            put(row, 0x1c, frame);
            seen |= 1 << (i % 32);
        }
    }
    let mut i = 0;
    while i < *count {
        if word(&history[i], 0x1c) == frame {
            i += 1;
        } else {
            *count -= 1;
            history[i] = history[*count];
        }
    }
    for (i, candidate) in candidates.iter().enumerate() {
        let bit = 1 << (i % 32);
        if *count == HISTORY_ROWS {
            break;
        }
        if seen & bit != 0 {
            continue;
        }
        seen |= bit;
        let row = &mut history[*count];
        row[..0x14].copy_from_slice(candidate);
        put(row, 0x14, 0);
        put(row, 0x18, word(candidate, 0xc));
        put(row, 0x1c, frame);
        put(row, 0x20, frame);
        *count += 1;
        written = written.max(*count);
    }
    written
}

// 0x0004b160 walks adjacent pairs left to right. Its first two
// passes start at the same pair, then the start advances one slot per
// pass. Preserve the swaps even for inputs it only partially orders.
fn rank_adjacent(rows: &mut [History], after: impl Fn(&History, &History) -> bool) {
    for pass in 0..rows.len() {
        for right in pass.max(1)..rows.len() {
            let left = right - 1;
            if after(&rows[left], &rows[right]) {
                rows.swap(left, right);
            }
        }
    }
}

fn fade(row: &mut History, delta: f32) {
    let rate = scalar(row, 0x14) + delta;
    let opacity = scalar(row, 0x18) + rate;
    let cap = scalar(row, 0xc);
    put(row, 0x14, rate.to_bits());
    put(
        row,
        0x18,
        if 0.0 > opacity {
            0
        } else if opacity > cap {
            cap.to_bits()
        } else {
            opacity.to_bits()
        },
    );
}

fn rank_and_fade(rows: &mut [History], frame: u32, limit: usize, step: f32) {
    let count = rows.len();
    let mut protected = 0;
    for i in 0..count.saturating_sub(1) {
        if rows[i][0] != 0 && rows[i][1] != 0 {
            rows.swap(i, 0);
            protected += 1;
        }
    }
    if protected == 0
        && let Some(i) = rows.iter().position(|row| row[0] != 0)
    {
        rows.swap(0, i);
        protected = 1;
    }
    let mut end = count;
    if count > limit {
        let mut i = protected;
        while i < end {
            if rows[i][2] != 0 && word(&rows[i], 0x20) == frame {
                put(&mut rows[i], 0x18, 0);
                end -= 1;
                rows.swap(i, end);
            } else {
                i += 1;
            }
        }
    }

    rank_adjacent(&mut rows[protected..end], |a, b| {
        (word(a, 8) as i32) < (word(b, 8) as i32)
    });
    let mut i = protected;
    while i < end {
        let owner = word(&rows[i], 8);
        if i != 0 && owner != NONE && owner == word(&rows[i - 1], 8) && rows[i][3] == 0 {
            put(&mut rows[i], 0x18, 0);
            put(&mut rows[i], 0x14, (0.0 - step).to_bits());
            end -= 1;
            rows.swap(i, end);
        }
        i += 1;
    }

    rank_adjacent(&mut rows[protected..end], |a, b| {
        scalar(b, 16) > scalar(a, 16)
    });
    let visible = limit.min(count);
    for row in rows[visible..].iter_mut().rev() {
        if row[3] == 0 && scalar(row, 0x18) > 0.0 {
            fade(row, -step);
        } else {
            put(row, 0x14, 0);
        }
    }
    for row in &mut rows[..visible] {
        if scalar(row, 0xc) > scalar(row, 0x18) {
            fade(row, step);
        } else {
            put(row, 0x14, 0);
        }
    }
}

pub(super) fn visible(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::VisibleLights, read_visible(m, cpu))
}

pub(super) fn geometry(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::LightGeometry, geometry_flags(m, cpu))
}

pub(super) fn extra_caster(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::LightExtraCaster, extra_caster_list(m, cpu))
}

fn extra_caster_list(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    let mut out = Output::default();
    if cpu.ecx == NONE {
        return Ok(out);
    }
    let vis = stack(m, cpu, 4)?;
    if vis != objects::LIGHT_VIS {
        return Err(Refusal::Unsupported);
    }
    let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
    let mut handle = cpu.ecx;
    for _ in 0..objects.end {
        let body = objects.body(m, handle)?;
        let parent = m.u32(at(body, 0x14)?)?;
        if m.u8(at(body, 0xaa)?)? == 0 || parent == NONE {
            let list = List::read(m, ptr(m, at(vis, 0x14)?)?)?;
            if list.has_room(list.count) {
                let mut rows = list.rows(m)?;
                let mut row = Row::plain(handle);
                row.flags = if cpu.eax as u8 == 0 { 0x1000 } else { 0x3000 };
                rows.push(row);
                list.publish(m, &mut out, &rows)?;
            }
            return Ok(out);
        }
        handle = parent;
    }
    Err(Refusal::Layout)
}

fn geometry_flags(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    let vis = cpu.esi;
    if vis != objects::LIGHT_VIS {
        return Err(Refusal::Unsupported);
    }
    let tags = ptr(m, TAGS)?;
    let light = pool(m)?.slot(m, cpu.eax)?;
    let definition = tag(m, tags, cpu.ecx)?;
    if m.u32(at(light, 4)?)? != cpu.ecx {
        return Err(Refusal::Layout);
    }
    let flags = m.u32(definition)?;
    let owner = stack(m, cpu, 4)?;
    let root = stack(m, cpu, 8)?;
    let mut out = Output::default();

    let clusters = List::read(m, ptr(m, at(vis, 0xc)?)?)?;
    let force_clusters = flags & 4 != 0 && clusters.count > 1;
    if force_clusters {
        if stack(m, cpu, 16)? == 0 && stack(m, cpu, 12)? as u8 == 0 {
            return Err(Refusal::Unsupported);
        }
        out.word(at(light, 0xf8)?, m.u32(ONE)?);
    }
    if flags & 0x10000 != 0 {
        write_flags(
            m,
            &mut out,
            &clusters,
            |_, old| Ok((old & !0x2000) | 0x1000),
        )?;
    }
    if flags & 0x10000 != 0 {
        let placements = List::read(m, ptr(m, at(vis, 0x18)?)?)?;
        write_flags(m, &mut out, &placements, |_, old| {
            Ok((old & !0x2000) | 0x1000)
        })?;
    }
    if flags & 0x23800 != 0 {
        let objects = List::read(m, ptr(m, at(vis, 0x14)?)?)?;
        let handles = if flags & 0x3800 != 0 && objects.count != 0 && owner != NONE && root != NONE
        {
            objects.column(m, 0xc)?
        } else {
            0
        };
        write_flags(m, &mut out, &objects, |i, old| {
            let base = if flags & 0x20000 != 0 {
                (old & !0x2000) | 0x1000
            } else {
                old
            };
            if flags & 0x3800 == 0 {
                return Ok(base);
            }
            let matches = owner == NONE || root == NONE || {
                let handle = m.u32(at(handles, i as u32 * 4)?)?;
                handle == owner || handle == root
            };
            Ok(if matches {
                owner_flags(base, flags)
            } else {
                base
            })
        })?;
    }
    Ok(out)
}

fn owner_flags(old: u16, flags: u32) -> u16 {
    let first = flags & 0x2000 == 0 || flags & 0x800 == 0;
    let second = flags & 0x2000 == 0 || flags & 0x1000 == 0;
    (old & !0x3000) | (u16::from(first) << 12) | (u16::from(second) << 13)
}

fn write_flags(
    m: &dyn GuestMemory,
    out: &mut Output,
    list: &List,
    mut apply: impl FnMut(usize, u16) -> Result<u16>,
) -> Result<()> {
    if list.count == 0 {
        return Ok(());
    }
    let addr = list.column(m, 8)?;
    let mut flags = m.bytes(addr, list.count * 2)?;
    for (i, bits) in flags.chunks_exact_mut(2).enumerate() {
        let value = apply(i, u16::from_le_bytes(bits.try_into().unwrap()))?;
        bits.copy_from_slice(&value.to_le_bytes());
    }
    out.array(addr, flags);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(handle: u32, owner: u32, weight: f32) -> Visible {
        let mut row = [0; 0x14];
        put(&mut row, 4, handle);
        put(&mut row, 8, owner);
        put(&mut row, 12, 1.0f32.to_bits());
        put(&mut row, 16, weight.to_bits());
        row
    }

    #[test]
    fn visible_lights_keep_membership_and_rank_with_guest_tie_order() {
        let candidates = [
            candidate(1, NONE, 2.0),
            candidate(2, NONE, 2.0),
            candidate(3, NONE, 4.0),
        ];
        let mut history = [[0; 0x24]; HISTORY_ROWS];
        let mut count = 0;
        merge_history(&mut history, &mut count, &candidates, 7);
        rank_and_fade(&mut history[..count], 7, 3, 0.01);
        assert_eq!(
            history[..count]
                .iter()
                .map(|row| word(row, 4))
                .collect::<Vec<_>>(),
            [3, 1, 2]
        );

        merge_history(&mut history, &mut count, &candidates[1..], 8);
        assert_eq!(count, 2);
        assert_eq!(
            history[..count]
                .iter()
                .map(|row| word(row, 4))
                .collect::<Vec<_>>(),
            [3, 2]
        );
    }

    #[test]
    fn light_rank_repeats_the_first_pass_then_advances_the_start() {
        let mut rows = [[0; 0x24]; 4];
        for (i, row) in rows.iter_mut().enumerate() {
            put(row, 0x10, (i as f32).to_bits());
        }
        rank_adjacent(&mut rows, |a, b| scalar(b, 0x10) > scalar(a, 0x10));
        assert_eq!(rows.map(|row| scalar(&row, 0x10)), [2.0, 3.0, 1.0, 0.0]);
    }

    #[test]
    fn visible_light_history_fades_existing_rows_in_place() {
        let candidates = [
            candidate(1, NONE, 4.0),
            candidate(2, NONE, 3.0),
            candidate(3, NONE, 2.0),
            candidate(4, NONE, 1.0),
        ];
        let mut history = [[0; 0x24]; HISTORY_ROWS];
        let mut count = 0;
        merge_history(&mut history, &mut count, &candidates, 10);
        rank_and_fade(&mut history[..count], 10, 3, 0.01);
        assert_eq!(scalar(&history[3], 0x18), 0.99);
        merge_history(&mut history, &mut count, &candidates, 11);
        rank_and_fade(&mut history[..count], 11, 3, 0.01);
        assert_eq!(scalar(&history[3], 0x18), 0.99f32 - 0.02);
    }

    #[test]
    fn light_geometry_preserves_unrelated_visibility_bits() {
        assert_eq!(owner_flags(0xc602, 0x2800), 0xe602);
        assert_eq!(owner_flags(0xe602, 0x3800), 0xc602);
        assert_eq!(owner_flags(0xc602, 0x800), 0xf602);
    }
}
