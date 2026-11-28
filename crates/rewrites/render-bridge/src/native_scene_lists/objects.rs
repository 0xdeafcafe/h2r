use super::*;

pub(super) const MAIN_VIS: u32 = 0x0054_7f88;
pub(super) const LIGHT_VIS: u32 = 0x0054_aa68;
pub(super) const VIEW_KIND: u32 = 0x2a78;
pub(super) const VIS_FLAGS: u32 = 0x2a60;
const SMALL_SPHERE: u32 = 0x0045_dbf4;
const OBJECT_EPOCH: u32 = 0x004d_e2fc;
const OBJECT_STAMPS: u32 = 0x004d_e300;
const CLUSTER_OBJECTS: [(u32, u32); 2] = [(0x004d_e2e0, 0x004d_e2e4), (0x004d_e2d4, 0x004d_e2d8)];
const CLUSTER_LIGHTS: (u32, u32) = (0x004e_0310, 0x004e_0314);
pub(super) const ACTIVE_BSP: u32 = 0x004e_0348;
const ACTIVE_SCENARIO: u32 = 0x004e_0350;
const SKY_OVERRIDE_FLAGS: u32 = 0x004e_64c8;
const VIEW_SKY_DATUM: u32 = 0x004b_9f8c;
const SKY_CLUSTER_ENABLED: u32 = 0x004b_9ee9;
const SKY_CLUSTER_PREDICATE: u32 = 0x0046_86c4;
const SKY_CLUSTER_INDEX: u32 = 0x004b_9eec;
const CAMERA_PLAYER: u32 = 0x004b_9ed4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub handle: u32,
    pub flags: u16,
    extra: u32,
    pub mask: u16,
}

pub(super) struct List {
    header: u32,
    pub capacity: usize,
    pub count: usize,
}

impl Row {
    pub(super) fn plain(handle: u32) -> Self {
        Self {
            handle,
            flags: 0,
            extra: 0,
            mask: u16::MAX,
        }
    }
}

impl List {
    pub(super) fn read(m: &dyn GuestMemory, header: u32) -> Result<Self> {
        let count = m.u16(at(header, 4)?)? as usize;
        let capacity = m.u32(header)? as usize;
        if capacity > 0x8000 || count > capacity || count >= 0x8000 {
            return Err(Refusal::Layout);
        }
        Ok(Self {
            header,
            capacity,
            count,
        })
    }

    pub(super) fn column(&self, m: &dyn GuestMemory, offset: u32) -> Result<u32> {
        ptr(m, at(self.header, offset)?)
    }

    pub(super) fn rows(&self, m: &dyn GuestMemory) -> Result<Vec<Row>> {
        if self.count == 0 {
            return Ok(Vec::new());
        }
        let handles = m.bytes(self.column(m, 0xc)?, self.count * 4)?;
        let flags = m.bytes(self.column(m, 8)?, self.count * 2)?;
        let extras = m.bytes(self.column(m, 0x10)?, self.count * 4)?;
        let masks = m.bytes(self.column(m, 0x14)?, self.count * 2)?;
        Ok((0..self.count)
            .map(|i| Row {
                handle: u32::from_le_bytes(handles[4 * i..4 * i + 4].try_into().unwrap()),
                flags: u16::from_le_bytes(flags[2 * i..2 * i + 2].try_into().unwrap()),
                extra: u32::from_le_bytes(extras[4 * i..4 * i + 4].try_into().unwrap()),
                mask: u16::from_le_bytes(masks[2 * i..2 * i + 2].try_into().unwrap()),
            })
            .collect())
    }

    pub(super) fn has_room(&self, count: usize) -> bool {
        count + 1 < self.capacity
    }

    pub(super) fn publish(
        &self,
        m: &dyn GuestMemory,
        out: &mut Output,
        rows: &[Row],
    ) -> Result<()> {
        if !rows.is_empty() {
            out.array(
                self.column(m, 0xc)?,
                rows.iter().flat_map(|r| r.handle.to_le_bytes()).collect(),
            );
            out.array(
                self.column(m, 8)?,
                rows.iter().flat_map(|r| r.flags.to_le_bytes()).collect(),
            );
            out.array(
                self.column(m, 0x10)?,
                rows.iter().flat_map(|r| r.extra.to_le_bytes()).collect(),
            );
            out.array(
                self.column(m, 0x14)?,
                rows.iter().flat_map(|r| r.mask.to_le_bytes()).collect(),
            );
        }
        out.half(at(self.header, 4)?, rows.len() as u16);
        Ok(())
    }
}

struct Refs {
    heads: u32,
    pool: Pool,
}

impl Refs {
    fn read(m: &dyn GuestMemory, (heads, header): (u32, u32)) -> Result<Self> {
        let header = ptr(m, header)?;
        let stride = m.u32(at(header, 0x24)?)?;
        if stride < 12 {
            return Err(Refusal::Layout);
        }
        Ok(Self {
            heads: ptr(m, heads)?,
            pool: Pool::read(m, header, stride)?,
        })
    }

    fn walk(
        &self,
        m: &dyn GuestMemory,
        cluster: u32,
        mut each: impl FnMut(u32) -> Result<()>,
    ) -> Result<()> {
        let mut reference = m.u32(at(self.heads, cluster * 4)?)?;
        for _ in 0..self.pool.end {
            if reference == NONE {
                return Ok(());
            }
            let node = self.pool.slot(m, reference)?;
            let handle = m.u32(at(node, 4)?)?;
            if handle == NONE {
                return Ok(());
            }
            reference = m.u32(at(node, 8)?)?;
            each(handle)?;
        }
        if reference == NONE {
            Ok(())
        } else {
            Err(Refusal::Layout)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Sphere {
    pub centre: [f32; 3],
    pub radius: f32,
}

impl Sphere {
    pub(super) fn read(m: &dyn GuestMemory, addr: u32) -> Result<Self> {
        Ok(Self {
            centre: m.vec3(addr)?,
            radius: m.f32(at(addr, 12)?)?,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) enum Clip {
    Portals,
    Sphere(Sphere),
    All,
}

impl Clip {
    pub(super) fn read(m: &dyn GuestMemory, vis: u32) -> Result<Self> {
        match m.u32(at(vis, 0x2a7c)?)? {
            0 => Ok(Self::Portals),
            1 => Ok(Self::Sphere(Sphere::read(m, at(vis, 0x2a68)?)?)),
            2 => Ok(Self::All),
            _ => Err(Refusal::Layout),
        }
    }

    // Result 0 rejects, 1 intersects and 2 is fully inside.
    pub(super) fn admits(
        self,
        m: &dyn GuestMemory,
        clusters: u32,
        ordinal: u32,
        object: Sphere,
        force: bool,
    ) -> Result<u8> {
        match self {
            Self::Portals if force => Ok(1),
            Self::Portals => portal_test(m, clusters, ordinal, object),
            Self::Sphere(s) => {
                let [x, y, z] = std::array::from_fn(|i| object.centre[i] - s.centre[i]);
                let limit = object.radius + s.radius;
                if !matches!(
                    (limit * limit).partial_cmp(&((z * z + x * x) + y * y)),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                ) {
                    return Ok(0);
                }
                // The containment branch uses the game's double-precision
                // x87 control word, even though the broad test uses SSE.
                let [x, y, z] =
                    std::array::from_fn(|i| f64::from(s.centre[i]) - f64::from(object.centre[i]));
                let far = ((z * z + y * y) + x * x).sqrt() + f64::from(object.radius);
                Ok(if f64::from(s.radius) >= far { 2 } else { 1 })
            }
            Self::All => Ok(1),
        }
    }
}

pub(super) fn plane_distance(normal: [f32; 3], offset: f32, [x, y, z]: [f32; 3]) -> f32 {
    ((normal[2] * z + normal[1] * y) + normal[0] * x) - offset
}

fn portal_test(m: &dyn GuestMemory, clusters: u32, ordinal: u32, sphere: Sphere) -> Result<u8> {
    let cluster = at(clusters, 0xa6e + ordinal * 0x1a)?;
    let groups = m.u16(clusters)?;
    if groups > 6 {
        return Err(Refusal::Layout);
    }
    let mut result = 0;
    for group in 0..u32::from(groups) {
        let count = m.u16(at(cluster, 2 + group * 2)?)? as u32;
        if count == 0 {
            continue;
        }
        let first = m.u16(at(cluster, 0xe + group * 2)?)? as u32;
        if first + count > 0x8000 {
            return Err(Refusal::Layout);
        }
        for ordinal in first..first + count {
            let frustum = at(clusters, 0x1974 + ordinal * 0x108)?;
            let result_here = classify_frustum(m, frustum, sphere)?;
            result = result.max(result_here);
            if result == 2 {
                return Ok(2);
            }
        }
    }
    Ok(result)
}

fn classify_frustum(m: &dyn GuestMemory, frustum: u32, sphere: Sphere) -> Result<u8> {
    // Bounds and all six planes are contiguous. A single checked read
    // avoids repeating the RAM mapping lookup for every plane component.
    let mut bytes = [0; 0x78];
    m.read(at(frustum, 0x60)?, &mut bytes)?;
    let f32_at = |offset| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    for (i, p) in sphere.centre.into_iter().enumerate() {
        let lower = f32_at(i * 8);
        let upper = f32_at(i * 8 + 4);
        if p - sphere.radius > upper || lower > p + sphere.radius {
            return Ok(0);
        }
    }
    let mut inside = true;
    for i in 0..6 {
        let base = 0x18 + i * 16;
        let n = [f32_at(base), f32_at(base + 4), f32_at(base + 8)];
        let d = plane_distance(n, f32_at(base + 12), sphere.centre);
        // Retail checks the fifth plane for rejection only.
        if d > sphere.radius {
            return Ok(0);
        }
        if i != 4 && (0.0 - sphere.radius).partial_cmp(&d) != Some(std::cmp::Ordering::Greater) {
            inside = false;
        }
    }
    Ok(if inside { 2 } else { 1 })
}

struct Gather<'a> {
    m: &'a dyn GuestMemory,
    vis: u32,
    clusters: u32,
    view_flags: u32,
    main_view: bool,
    clip: Clip,
    object_pool: Pool,
    object_epoch: u32,
    list: List,
    rows: Vec<Row>,
    current: HashMap<u32, usize>,
    masks: ClusterMasks,
    out: Output,
    restrict_to: Option<Vec<Row>>,
}

impl Gather<'_> {
    fn add(&mut self, handle: u32, ordinal: u32, skip: bool, force: bool) -> Result<()> {
        let object_index = handle & 0xffff;
        if object_index >= 2048 {
            return Err(Refusal::Layout);
        }
        let stamp = OBJECT_STAMPS + object_index * 4;
        if let Some(&i) = self.current.get(&object_index) {
            if self.rows[i].handle != handle {
                return Err(Refusal::Layout);
            }
            let mask = self.rows[i].mask;
            if self.rows[i].flags & 0x800 == 0 && mask != u16::MAX {
                self.masks.include(self.m, mask, ordinal)?;
            }
            self.rows[i].flags |= if skip { 0x400 } else { 0 } | if force { 0x200 } else { 0 };
            return Ok(());
        }
        if self.m.u32(stamp)? == self.object_epoch {
            return Err(Refusal::Layout);
        }

        let body = self.object_pool.body(self.m, handle)?;
        let sphere = Sphere::read(self.m, at(body, if self.main_view { 0x30 } else { 0x50 })?)?;
        let visibility = self
            .clip
            .admits(self.m, self.clusters, ordinal, sphere, force)?;
        if visibility == 0 && self.view_flags & 0x80 == 0 {
            return Ok(());
        }
        let inside = wholly_inside(self.m, self.clip, self.main_view, sphere, visibility)?;
        let first_side = visibility == 0
            || self.view_flags & 1 == 0
            || self
                .restrict_to
                .as_ref()
                .is_none_or(|list| list.iter().any(|r| r.handle == handle));
        let second_side = first_side && (self.main_view || self.m.u8(at(body, 6)?)? & 1 == 0);
        let (positive, negative) = self.clip_sides(sphere)?;
        let flags = (u16::from(negative) << 15)
            | (u16::from(positive) << 14)
            | (u16::from(second_side) << 13)
            | (u16::from(first_side) << 12)
            | (u16::from(inside) << 11)
            | (u16::from(skip) << 10)
            | (u16::from(force) << 9);

        let mask = if !inside && self.m.u8(at(body, 7)?)? & 0x80 != 0 {
            self.masks.allocate(self.m, ordinal)?
        } else {
            u16::MAX
        };
        // The guest may allocate a clip mask even after the list fills.
        if self.list.has_room(self.rows.len()) {
            self.current.insert(object_index, self.rows.len());
            self.rows.push(Row {
                handle,
                flags,
                extra: 0,
                mask,
            });
            self.out.word(stamp, self.object_epoch);
        }
        Ok(())
    }

    fn clip_sides(&self, sphere: Sphere) -> Result<(bool, bool)> {
        if self.view_flags & 0x200 == 0 {
            return Ok((true, true));
        }
        let d = plane_distance(
            self.m.vec3(at(self.vis, 0x2a88)?)?,
            self.m.f32(at(self.vis, 0x2a94)?)?,
            sphere.centre,
        );
        Ok(if sphere.radius < d.abs() {
            (d > 0.0, d < 0.0)
        } else {
            (true, true)
        })
    }
}

pub(super) fn wholly_inside(
    m: &dyn GuestMemory,
    clip: Clip,
    main_view: bool,
    sphere: Sphere,
    visibility: u8,
) -> Result<bool> {
    Ok(visibility == 2
        || (matches!(clip, Clip::Portals)
            && visibility != 0
            && main_view
            && m.f32(SMALL_SPHERE)? >= sphere.radius))
}

pub(super) struct ClusterMasks {
    vis: u32,
    used: u16,
    words: std::collections::BTreeMap<u32, u32>,
}

impl ClusterMasks {
    pub(super) fn read(m: &dyn GuestMemory, vis: u32) -> Result<Self> {
        let used = m.u16(at(vis, 0xa5e)?)?;
        if used > 512 {
            return Err(Refusal::Layout);
        }
        Ok(Self {
            vis,
            used,
            words: Default::default(),
        })
    }

    pub(super) fn include(&mut self, m: &dyn GuestMemory, mask: u16, ordinal: u32) -> Result<()> {
        // The special sky pass's -1 ordinal and indices beyond a mask's
        // four words can alias adjacent fields in the retail routine.
        if mask >= 512 || ordinal >= 128 {
            return Err(Refusal::Unsupported);
        }
        let word = at(self.vis, 0xa60 + u32::from(mask) * 16 + (ordinal / 32) * 4)?;
        let value = match self.words.entry(word) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(m.u32(word)?),
        };
        *value |= 1 << (ordinal % 32);
        Ok(())
    }

    pub(super) fn allocate(&mut self, m: &dyn GuestMemory, ordinal: u32) -> Result<u16> {
        if self.used == 512 {
            return Ok(0);
        }
        let mask = self.used;
        self.used += 1;
        for word in 0..4 {
            self.words
                .insert(at(self.vis, 0xa60 + u32::from(mask) * 16 + word * 4)?, 0);
        }
        self.include(m, mask, ordinal)?;
        Ok(mask)
    }

    pub(super) fn publish(self, out: &mut Output) -> Result<()> {
        out.half(at(self.vis, 0xa5e)?, self.used);
        for (address, value) in self.words {
            out.word(address, value);
        }
        Ok(())
    }
}

pub(super) fn cluster_uses_sky(m: &dyn GuestMemory, cluster: u32) -> Result<bool> {
    let bsp = m.u32(ACTIVE_BSP)?;
    let scenario = m.u32(ACTIVE_SCENARIO)?;
    if bsp == 0 || scenario == 0 {
        return Ok(false);
    }
    if m.u32(SKY_OVERRIDE_FLAGS)? & 0x10 != 0 {
        return Ok(true);
    }
    let clusters = ptr(m, at(bsp, 0xa0)?)?;
    let sky = m.u8(at(clusters, cluster * 0xb0 + 0x70)?)?;
    if sky == 0xff {
        return Ok(false);
    }
    let view_sky = m.u32(VIEW_SKY_DATUM)?;
    if view_sky == NONE {
        return Ok(true);
    }
    let index = if sky & 0x80 != 0 {
        let table = ptr(m, at(bsp, 0x68)?)?;
        m.u16(at(table, u32::from(sky & 0x7f) * 24)?)?
    } else {
        u16::from(sky)
    };
    if index == u16::MAX {
        return Ok(false);
    }
    let skies = ptr(m, at(scenario, 0x34c)?)?;
    Ok(m.u32(at(skies, u32::from(index) * 16 + 8)?)? == view_sky)
}

fn sky_cluster(m: &dyn GuestMemory, vis: u32, count: u16, enabled: bool) -> Result<Option<u32>> {
    if !enabled
        || count == 0
        || m.u8(SKY_CLUSTER_ENABLED)? == 0
        || m.u16(SKY_CLUSTER_PREDICATE)? == u16::MAX
    {
        return Ok(None);
    }
    let index = m.u32(SKY_CLUSTER_INDEX)?;
    if index == NONE {
        return Ok(None);
    }
    let bsp = ptr(m, ACTIVE_BSP)?;
    let length = m.u32(at(bsp, 0xac)?)?;
    if index >= length {
        return if length == 0 {
            Ok(None)
        } else {
            Err(Refusal::Layout)
        };
    }
    let table = ptr(m, at(bsp, 0xb0)?)?;
    let cluster = m.u16(at(table, index.checked_mul(2).ok_or(Refusal::Layout)?)?)?;
    if cluster == u16::MAX {
        return Ok(None);
    }
    if cluster >= 512 || vis != MAIN_VIS {
        return Err(Refusal::Layout);
    }
    Ok(Some(u32::from(cluster)))
}

pub(super) fn gather(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    let vis = stack(m, cpu, 4);
    let stage = if vis == Ok(LIGHT_VIS) {
        SceneList::LightObjects
    } else {
        SceneList::ViewObjects
    };
    (stage, vis.and_then(|vis| build_gather(m, cpu, vis)))
}

fn build_gather(m: &dyn GuestMemory, cpu: &CpuAbi, vis: u32) -> Result<Output> {
    fp_supported(cpu)?;
    let kind = m.u32(at(vis, VIEW_KIND)?)?;
    if !matches!((vis, kind), (MAIN_VIS, 0) | (LIGHT_VIS, 1 | 2)) {
        return Err(Refusal::Unsupported);
    }
    let clip = Clip::read(m, vis)?;
    if matches!(clip, Clip::Sphere(_)) && cpu.x87.control != 0x027f {
        return Err(Refusal::Unsupported);
    }
    let clusters = ptr(m, vis)?;
    let count = m.u16(at(clusters, 0xa6c)?)?;
    if count > 512 {
        return Err(Refusal::Layout);
    }
    let list = List::read(m, ptr(m, at(vis, 0x14)?)?)?;
    // This entry follows the visibility reset and has no prior object rows.
    if list.count != 0 {
        return Err(Refusal::Unsupported);
    }
    let view_flags = m.u32(at(vis, VIS_FLAGS)?)?;
    let prior_vis = m.u32(at(vis, 0x2ad0)?)?;
    let restrict_to = if prior_vis != 0 && view_flags & 1 != 0 {
        Some(List::read(m, ptr(m, at(prior_vis, 0x14)?)?)?.rows(m)?)
    } else {
        None
    };
    let mut walk = Gather {
        m,
        vis,
        clusters,
        view_flags,
        main_view: kind == 0,
        clip,
        object_pool: Pool::read(m, ptr(m, OBJECTS)?, 12)?,
        object_epoch: m.u32(OBJECT_EPOCH)?,
        list,
        rows: Vec::new(),
        current: HashMap::new(),
        masks: ClusterMasks::read(m, vis)?,
        out: Output::default(),
        restrict_to,
    };
    let object_refs = if view_flags & 0x40 == 0 && count != 0 {
        Some([
            Refs::read(m, CLUSTER_OBJECTS[0])?,
            Refs::read(m, CLUSTER_OBJECTS[1])?,
        ])
    } else {
        None
    };
    let gather_lights = kind == 0 && view_flags & 0x10 == 0;
    let light_refs = if gather_lights && count != 0 {
        Some(Refs::read(m, CLUSTER_LIGHTS)?)
    } else {
        None
    };
    let mut lights = if gather_lights {
        Some(lights::Candidates::read(m, vis)?)
    } else {
        None
    };
    for ordinal in 0..u32::from(count) {
        let cluster = m.u16(at(clusters, 0xa6e + ordinal * 0x1a)?)? as u32;
        if cluster >= 512 {
            return Err(Refusal::Layout);
        }
        let sky = if object_refs.is_some() {
            cluster_uses_sky(m, cluster)?
        } else {
            false
        };
        if let Some([a, b]) = &object_refs {
            a.walk(m, cluster, |handle| {
                if view_flags & 0x400 == 0
                    || m.u8(at(walk.object_pool.body(m, handle)?, 0xaa)?)? == 7
                {
                    walk.add(handle, ordinal, sky, false)?;
                }
                Ok(())
            })?;
            b.walk(m, cluster, |handle| walk.add(handle, ordinal, sky, false))?;
        }
        if let (Some(refs), Some(lights)) = (&light_refs, &mut lights) {
            refs.walk(m, cluster, |handle| {
                lights.add(handle, |sphere| {
                    Ok(clip.admits(m, clusters, ordinal, sphere, false)? != 0
                        || view_flags & 0x80 != 0)
                })
            })?;
        }
    }
    if let Some(lights) = &mut lights {
        if m.u16(at(vis, 8)?)? != u16::MAX && m.u32(CAMERA_PLAYER)? != NONE {
            lights.add_player_lights()?;
        }
        lights.publish(&mut walk.out)?;
    }
    if let Some([a, b]) = &object_refs
        && let Some(sky) = sky_cluster(m, vis, count, kind == 0)?
    {
        a.walk(m, sky, |handle| walk.add(handle, u32::MAX, false, true))?;
        b.walk(m, sky, |handle| walk.add(handle, u32::MAX, false, true))?;
    }
    walk.list.publish(m, &mut walk.out, &walk.rows)?;
    walk.masks.publish(&mut walk.out)?;
    Ok(walk.out)
}

pub(super) fn order(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::ObjectOrder, order_rows(m, cpu))
}

fn order_rows(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    let vis = stack(m, cpu, 4)?;
    if !matches!(vis, MAIN_VIS | LIGHT_VIS) {
        return Err(Refusal::Unsupported);
    }
    let list = List::read(m, ptr(m, at(vis, 0x14)?)?)?;
    let mut rows = list.rows(m)?;
    let mut out = Output::default();
    if !rows.is_empty() {
        let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
        let tags = ptr(m, TAGS)?;
        promote(&mut rows, |handle| {
            if handle == NONE {
                return Ok(false);
            }
            let body = objects.body(m, handle)?;
            let definition = m.u32(body)?;
            if definition == NONE {
                return Ok(false);
            }
            let model = m.u32(at(tag(m, tags, definition)?, 0x38)?)?;
            if model == NONE {
                return Ok(false);
            }
            let render_model = m.u32(at(tag(m, tags, model)?, 4)?)?;
            if render_model == NONE {
                return Ok(false);
            }
            Ok(m.i32(at(tag(m, tags, render_model)?, 0x74)?)? > 0)
        })?;
    }
    list.publish(m, &mut out, &rows)?;
    Ok(out)
}

// Swap at the cursor: the displaced entry moves to the match's position.
// A stable partition would change the order of the non-promoted entries.
fn promote(rows: &mut [Row], mut priority: impl FnMut(u32) -> Result<bool>) -> Result<()> {
    let mut head = 0;
    for i in 0..rows.len() {
        if priority(rows[i].handle)? {
            rows.swap(head, i);
            head += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    use guest_layout::testing::FakeGuest;

    #[test]
    fn light_object_gather_walks_both_partitions_and_stamps_each_visible_object_once() {
        const VIS: u32 = LIGHT_VIS;
        const CLUSTERS: u32 = 0x10000;
        const LIST: u32 = 0x20000;
        const HANDLES: u32 = 0x21000;
        const FLAGS: u32 = 0x22000;
        const EXTRAS: u32 = 0x23000;
        const MASKS: u32 = 0x24000;
        const OBJ_POOL: u32 = 0x30000;
        const OBJ_DATA: u32 = 0x31000;
        const BODIES: u32 = 0x32000;
        const CHAIN_HEADS: u32 = 0x40000;
        const REF_POOL: u32 = 0x41000;
        const REF_DATA: u32 = 0x42000;
        const STACK: u32 = 0x50000;
        const EPOCH: u32 = 17;

        let mut m = FakeGuest::new();
        m.put_u32(VIS, CLUSTERS)
            .put_u32(VIS + VIEW_KIND, 1)
            .put_u32(VIS + 0x2a7c, 1)
            .put_f32(VIS + 0x2a74, 3.0)
            .put_u32(VIS + 0x14, LIST)
            .put_u16(VIS + 0xa5e, 0)
            .put_u16(CLUSTERS + 0xa6c, 2)
            .put_u16(CLUSTERS + 0xa6e, 0)
            .put_u16(CLUSTERS + 0xa6e + 0x1a, 1)
            .put_u32(LIST, 10)
            .put_u32(LIST + 8, FLAGS)
            .put_u32(LIST + 0xc, HANDLES)
            .put_u32(LIST + 0x10, EXTRAS)
            .put_u32(LIST + 0x14, MASKS)
            .put_u32(OBJECTS, OBJ_POOL)
            .put_u32(OBJ_POOL + 0x24, 12)
            .put_u32(OBJ_POOL + 0x38, 3)
            .put_u32(OBJ_POOL + 0x44, OBJ_DATA)
            .put_u32(OBJECT_EPOCH, EPOCH)
            .put_u32(STACK + 4, VIS);
        for (i, x) in [0.0, 3.4, 8.0].into_iter().enumerate() {
            let i = i as u32;
            let body = BODIES + i * 0x100;
            m.put_u16(OBJ_DATA + i * 12, 1)
                .put_u32(OBJ_DATA + i * 12 + 8, body)
                .put_u8(body + 6, u8::from(i == 1))
                .put_f32(body + 0x50, x)
                .put_f32(body + 0x5c, 0.5);
        }
        let handle = |i: u32| 0x10000 | i;
        for (partition, (heads, header)) in CLUSTER_OBJECTS.into_iter().enumerate() {
            let i = partition as u32;
            let base = CHAIN_HEADS + i * 8;
            let pool = REF_POOL + i * 0x80;
            m.put_u32(heads, base)
                .put_u32(header, pool)
                .put_u32(pool + 0x24, 12)
                .put_u32(pool + 0x38, 3)
                .put_u32(pool + 0x44, REF_DATA);
        }
        m.put_u32(CHAIN_HEADS, handle(0))
            .put_u32(CHAIN_HEADS + 4, handle(1))
            .put_u32(CHAIN_HEADS + 8, handle(1))
            .put_u32(CHAIN_HEADS + 12, NONE);
        for (i, next) in [(0, NONE), (1, handle(2)), (2, NONE)] {
            let node = REF_DATA + i * 12;
            m.put_u16(node, 1)
                .put_u32(node + 4, handle(i))
                .put_u32(node + 8, next);
        }
        let cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::xbox_thread()
        };
        let (stage, result) = gather(&m, &cpu);
        assert!(matches!(stage, SceneList::LightObjects));
        let out = result.unwrap();
        assert_eq!(m.u16(LIST + 4).unwrap(), 0);
        for edit in out.writes {
            m.put(edit.addr, edit.bytes.slice());
        }
        let rows = List::read(&m, LIST).unwrap().rows(&m).unwrap();
        assert_eq!(
            rows,
            [
                Row {
                    flags: 0xf800,
                    ..Row::plain(handle(0))
                },
                Row {
                    flags: 0xd000,
                    ..Row::plain(handle(1))
                },
            ]
        );
        assert_eq!(m.u32(OBJECT_STAMPS).unwrap(), EPOCH);
        assert_eq!(m.u32(OBJECT_STAMPS + 4).unwrap(), EPOCH);
        assert_eq!(m.u32(OBJECT_STAMPS + 8).unwrap(), 0);
    }

    #[test]
    fn object_priority_swaps_all_four_columns() {
        let mut rows = [0, 1, 2, 3, 4].map(|handle| Row {
            handle,
            flags: 0x1000 | handle as u16,
            extra: 100 + handle,
            mask: handle as u16,
        });
        let original = rows;
        promote(&mut rows, |handle| Ok(handle == 2 || handle == 4)).unwrap();
        assert_eq!(
            rows,
            [
                original[2],
                original[4],
                original[0],
                original[3],
                original[1]
            ]
        );
    }

    #[test]
    fn portal_gather_uses_the_wide_views_side_and_bottom_planes() {
        // +X forward, +Z up, 4:3 viewport. Place one sphere in each strip
        // exposed by widening the horizontal lens from 70 to 110 degrees.
        let clusters = 0x1000;
        let frustum = clusters + 0x1974;
        let mut m = FlatMemory::new(clusters, 0x2000);
        m.set_u16(clusters, 1).unwrap();
        m.set_u16(clusters + 0xa70, 1).unwrap();
        for i in 0..3 {
            m.set_f32(frustum + 0x60 + i * 8, -100.).unwrap();
            m.set_f32(frustum + 0x64 + i * 8, 100.).unwrap();
        }
        for (degrees, inside) in [(70f32, false), (110., true)] {
            let h = (degrees.to_radians() * 0.5).tan();
            let v = h * 0.75;
            for (i, normal) in [[-h, 1., 0.], [-h, -1., 0.], [-v, 0., 1.], [-v, 0., -1.]].iter().enumerate() {
                let length = (normal[0] * normal[0] + 1.).sqrt();
                for (axis, component) in normal.iter().enumerate() {
                    m.set_f32(frustum + 0x78 + i as u32 * 16 + axis as u32 * 4, component / length).unwrap();
                }
            }
            for centre in [[10., 11., 0.], [10., -11., 0.], [10., 0., -8.]] {
                let visible = Clip::Portals.admits(&m, clusters, 0, Sphere { centre, radius: 0.1 }, false).unwrap();
                assert_eq!(visible != 0, inside, "{degrees} degree view at {centre:?}");
            }
        }
    }

    #[test]
    fn portals_reject_outside_and_keep_intersections() {
        let mut m = FlatMemory::new(0x1000, 0x200);
        for i in 0..3 {
            m.set_f32(0x1060 + i * 8, -50.0).unwrap();
            m.set_f32(0x1064 + i * 8, 50.0).unwrap();
        }
        for i in 0..6 {
            m.set_f32(0x1084 + i * 16, 10.0).unwrap();
        }
        m.set_f32(0x1078, 1.0).unwrap();
        let pick = |x| {
            classify_frustum(
                &m,
                0x1000,
                Sphere {
                    centre: [x, 0.0, 0.0],
                    radius: 1.0,
                },
            )
            .unwrap()
        };
        assert_eq!(pick(8.0), 2);
        assert_eq!(pick(10.0), 1);
        assert_eq!(pick(12.0), 0);
    }
}
