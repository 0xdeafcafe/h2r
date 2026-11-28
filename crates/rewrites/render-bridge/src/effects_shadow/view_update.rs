//! The particle systems' per-view update `0x00174420` in Rust: the
//! stamps it writes and the sorted entries it queues
//! (docs/graphics/retire-d3d.md, "R2's shadow arm").
//!
//! `0x00174420(handle, first person, flag, third)` (`ret 0x10`, **Code**,
//! disassembled from the retail executable):
//!
//! 1. The system at `handle` (`[0x00510c74]`, `0x54` each). Its
//!    definition (`0x001751d0`) names a particle tag at `+4`; a tag that is
//!    not a particle (`prt3`) or particle model (`PRTM`) skips to 4.
//! 2. The tag's shader flags (`[[[shader]+0x5c]+4]+2`); definition
//!    `+0x16` bit 5 skips to 4; `+0x14` scales the queue offset.
//! 3. Each particle of the system (`[0x0051ec8c]`, `0x34` each, from
//!    `+0x30`, linked by `+0xc`): one whose `+2` is not the first-person
//!    byte gets the frame stamp `+0x30` and nothing else. With `flag`, the
//!    position is put in the view's axes, stamped and queued. Otherwise a
//!    cull decides: the view's own cluster or system `+0xc` bit `0x400`
//!    forces it (system `+0xd` bit 2 set); the cluster bits, the region
//!    frusta (`0x00165010`) or the portal frustum (`0x00181800`) reject it
//!    (`+0xd` bit 2 cleared); a kept one is stamped and, outside a forced
//!    system, faded by distance (`0x00249bb0`). A drawn particle with
//!    shader flag `0x8000` queues a sorted entry (`0x0001e4e0`, handler
//!    `0x00173c90`, payload system and particle); any other flag calls
//!    `0x00040e30`, which the model refuses.
//! 4. Each child (`+0x40`, linked by `+0x38`) takes the parent's
//!    `+0xc` bit `0x400`, then is updated with the same three arguments.
//!
//! The definition's particle-model reads go through two singletons
//! (`0x00479868` for `prt3`, `0x00479874` for `PRTM`): `0x00137bd0` sets a
//! singleton's tag and tag data before every use, and the model makes the
//! same writes in the same order. No path under `0x00174420` reads the
//! shared random seeds (`[0x004e7408]`): the model predicts that they do
//! not move.

use std::collections::BTreeMap;

use guest_layout::memory::{GuestMemory, MemFault};

use super::overlay::Overlay;

pub const ENTRY: u32 = 0x0017_4420;

pub const SYSTEMS: u32 = 0x0051_0c74;
pub const SYSTEM: u32 = 0x54;
pub const PARTICLES: u32 = 0x0051_ec8c;
pub const PARTICLE: u32 = 0x34;
const TAGS: u32 = 0x004e_3b44;
pub const FRAME: u32 = 0x004b_a034;
/// The view's position and its forward and up axes (`0x004b9da0..c4`).
const CAMERA: u32 = 0x004b_9da0;
const VIEW_FORWARD: u32 = 0x004b_9dac;
const VIEW_UP: u32 = 0x004b_9db8;
const DISTANCE_SCALE: u32 = 0x004b_9dcc;
const OFFSET_SCALE: u32 = 0x0045_dc1c;
const ZERO: u32 = 0x0045_dbd8;
const ONE: u32 = 0x0045_dbc0;
const CLUSTER_BITS: u32 = 0x0054_81a4;
/// The view's frusta: `+0` the window count, `+0x94` the portal frustum,
/// `+0xa6e` the region windows, `+0x1974` the window frusta.
const FRUSTA: u32 = 0x0054_7f88;
const CLUSTER_REGION: u32 = 0x0054_7fa4;
const FADE_OVERRIDE: u32 = 0x0051_0c50;

const PRT3: u32 = 0x7072_7433;
const PRTM: u32 = 0x5052_544d;
const EFFE: u32 = 0x6566_6665;
const BSDT: u32 = 0x6273_6474;

/// The particle-model singletons: group, object, retail vtable.
pub const SINGLETONS: [(u32, u32, u32); 2] = [
    (PRT3, 0x0047_9868, 0x0045_35f0),
    (PRTM, 0x0047_9874, 0x0045_3598),
];
/// Bytes of a singleton the model compares: vtable, tag, tag data.
pub const SINGLETON_BYTES: u32 = 12;

/// The sorted transparent list (docs/graphics/native-transparents.md).
pub const LIST_END: u32 = 0x004b_6280;
pub const LIST_FIRST: u32 = 0x004b_6284;
pub const LIST_ENTRIES: u32 = 0x004b_6288;
pub const LIST_ORDER: u32 = 0x004b_628c;
pub const LIST_OVERFLOW: u32 = 0x004b_6291;
pub const LIST_CAPACITY: u32 = 0x400;
pub const LIST_STRIDE: u32 = 0x70;
const ENTRY_CAMERA: u32 = 0x0048_5618;
const ENTRY_FORWARD: u32 = 0x0048_5624;
/// The particle entry's handler (`0x00174f80` for one particle, layer 15).
pub const PARTICLE_HANDLER: u32 = 0x0017_3c90;

/// Children and particles followed at most before the model gives up.
const LINKS: u32 = 0x1_0000;
/// System nesting and definition parents followed at most.
const DEPTH: u32 = 64;

/// Why the model makes no prediction. Each is a named counter
/// (`stamps_refused_<name>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A read or write the guest would make faults.
    Fault,
    /// A data array whose element size is not the retail one.
    Layout,
    /// `0x001751d0` answers 0 for the system's tag group: the guest then
    /// reads near 0.
    NoDefinition,
    /// A definition parent's tag is not a particle model: `0x00137bd0`
    /// answers 0 and `0x001751d0` calls through it.
    NoModel,
    /// A singleton whose vtable is not the retail one.
    SingletonVtable,
    /// A drawn particle with a shader flag besides `0x8000`: the call of
    /// `0x00040e30` is not modelled.
    ObjectPath,
    /// The distance fade's x87 sum at 64-bit precision or another
    /// rounding mode.
    X87,
    /// The distance fade's sum is not finite in single precision.
    DistanceRange,
    /// More than [`DEPTH`] nested systems or parents.
    Depth,
    /// More than [`LINKS`] children or particles: a cycle.
    Cycle,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Refusal::Fault => "fault",
            Refusal::Layout => "layout",
            Refusal::NoDefinition => "no_definition",
            Refusal::NoModel => "no_model",
            Refusal::SingletonVtable => "singleton_vtable",
            Refusal::ObjectPath => "object_path",
            Refusal::X87 => "x87",
            Refusal::DistanceRange => "distance_range",
            Refusal::Depth => "depth",
            Refusal::Cycle => "cycle",
        }
    }
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Refusal::Fault
    }
}

pub type Result<T> = std::result::Result<T, Refusal>;

/// What one outermost call writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prediction {
    /// Every byte the call writes, with its final value.
    pub writes: BTreeMap<u32, u8>,
    /// The sorted entries it queues, by address, in order.
    pub entries: Vec<u32>,
}

/// The data of the array whose header is at `[header]`, with elements of
/// `stride` bytes.
pub fn array_data(m: &dyn GuestMemory, header: u32, stride: u32) -> Result<u32> {
    let array = m.u32(header)?;
    if m.u32(array.wrapping_add(0x24))? != stride {
        return Err(Refusal::Layout);
    }
    Ok(m.u32(array.wrapping_add(0x44))?)
}

/// The array's data and its live extent in bytes (`+0x38`, the end).
pub fn array_span(m: &dyn GuestMemory, header: u32, stride: u32) -> Result<(u32, u32)> {
    let data = array_data(m, header, stride)?;
    let end = m.i32(m.u32(header)?.wrapping_add(0x38))?;
    if !(0..=0x1_0000).contains(&end) {
        return Err(Refusal::Layout);
    }
    Ok((data, end as u32 * stride))
}

fn tag_entry(m: &dyn GuestMemory, tag: u32) -> Result<u32> {
    Ok(m.u32(TAGS)?.wrapping_add((tag & 0xffff) << 4))
}

/// The tag's group, indexed as the guest does: by the handle's low word,
/// sign-extended (`movsx`).
fn tag_group(m: &dyn GuestMemory, tag: u32) -> Result<u32> {
    let index = (tag as u16 as i16 as i32) << 4;
    Ok(m.u32(m.u32(TAGS)?.wrapping_add(index as u32))?)
}

fn tag_data(m: &dyn GuestMemory, tag: u32) -> Result<u32> {
    Ok(m.u32(tag_entry(m, tag)?.wrapping_add(8))?)
}

/// `0x00137bd0` (EDX `tag`): the particle-model singleton of the tag's
/// group, set to the tag (`[vtable+0x50]`, `0x002243a0`), or `None`.
pub fn particle_model(m: &mut dyn GuestMemory, tag: u32) -> Result<Option<u32>> {
    let group = tag_group(m, tag)?;
    let Some(&(_, object, vtable)) = SINGLETONS.iter().find(|(g, ..)| *g == group) else {
        return Ok(None);
    };
    if m.u32(object)? != vtable {
        return Err(Refusal::SingletonVtable);
    }
    let data = tag_data(m, tag)?;
    m.set_u32(object + 4, tag)?;
    m.set_u32(object + 8, data)?;
    Ok(Some(object))
}

fn is_prtm(object: u32) -> bool {
    object == SINGLETONS[1].1
}

/// `[vtable+0x28]`: the particle's shader reference (`0x002242a0`,
/// `0x00224d70`).
fn shader_reference(m: &dyn GuestMemory, object: u32) -> Result<u32> {
    let data = m.u32(object + 8)?;
    if is_prtm(object) {
        let tag = m.u32(data.wrapping_add(0x1c))?;
        return Ok(m.u32(tag_data(m, tag)?.wrapping_add(0x24))?);
    }
    Ok(m.u32(data.wrapping_add(0x90))?)
}

/// `[vtable+0x1c]`: element `index` of the tag's definition block
/// (`0x00224260`, `0x00224d40`).
fn definition_block(m: &dyn GuestMemory, object: u32, index: u32) -> Result<u32> {
    let data = m.u32(object + 8)?;
    let block = if is_prtm(object) { 0x7c } else { 0x88 };
    Ok(m.u32(data.wrapping_add(block))?
        .wrapping_add(index.wrapping_mul(0x38)))
}

/// `0x001751d0` (ECX `system`): the system's definition.
pub fn definition(m: &mut dyn GuestMemory, system: u32) -> Result<u32> {
    definition_at(m, system, 0)
}

fn definition_at(m: &mut dyn GuestMemory, system: u32, depth: u32) -> Result<u32> {
    if depth > DEPTH {
        return Err(Refusal::Depth);
    }
    let tag = m.u32(system.wrapping_add(0x10))?;
    let event = u32::from(m.u16(system.wrapping_add(0xe))?).wrapping_mul(0x38);
    match tag_group(m, tag)? {
        EFFE => {
            let events = m.u32(tag_data(m, tag)?.wrapping_add(0x18))?;
            let part = m.u32(system.wrapping_add(0x14))?.wrapping_mul(0x38);
            Ok(m.u32(events.wrapping_add(part).wrapping_add(0x34))?
                .wrapping_add(event))
        }
        BSDT => Ok(m
            .u32(tag_data(m, tag)?.wrapping_add(0x18))?
            .wrapping_add(event)),
        PRTM | PRT3 => {
            let parent = m.u32(system.wrapping_add(0x48))?;
            let parent = definition_at(m, parent, depth + 1)?;
            let tag = m.u32(parent.wrapping_add(4))?;
            let object = particle_model(m, tag)?.ok_or(Refusal::NoModel)?;
            let index = u32::from(m.u16(system.wrapping_add(0xe))?);
            definition_block(m, object, index)
        }
        _ => Err(Refusal::NoDefinition),
    }
}

/// `0x0003eb20` (EDX `cluster`): whether the view's own cluster is
/// `cluster`.
fn view_cluster(m: &dyn GuestMemory, cluster: i32) -> Result<bool> {
    if cluster == -1 || m.u8(0x004b_9ee9)? == 0 {
        return Ok(false);
    }
    let index = m.u32(0x004b_9eec)?;
    if index == u32::MAX || m.u16(0x0046_86c4)? == 0xffff {
        return Ok(false);
    }
    let structure = m.u32(0x004e_0348)?;
    if m.i32(structure.wrapping_add(0xac))? <= 0 {
        return Ok(false);
    }
    let table = m.u32(structure.wrapping_add(0xb0))?;
    Ok(m.u16(table.wrapping_add(index.wrapping_mul(2)))? as i16 as i32 == cluster)
}

fn cluster_bit(m: &dyn GuestMemory, cluster: i32) -> Result<bool> {
    let word = CLUSTER_BITS.wrapping_add(((cluster >> 5) as u32).wrapping_mul(4));
    Ok(m.u32(word)? & (1 << (cluster & 31)) != 0)
}

/// `0x00181800` (EAX `frustum`, ECX the centre, XMM1 `radius`): 0 when
/// the sphere is outside, 2 when inside, 1 when it crosses.
pub fn sphere_in_frustum(
    m: &dyn GuestMemory,
    frustum: u32,
    centre: [f32; 3],
    radius: f32,
) -> Result<u32> {
    let f = |o: u32| m.f32(frustum.wrapping_add(o));
    let [x, y, z] = centre;
    if x - radius > f(0x64)? || y - radius > f(0x6c)? || z - radius > f(0x74)? {
        return Ok(0);
    }
    if f(0x60)? > x + radius || f(0x68)? > radius + y || f(0x70)? > radius + z {
        return Ok(0);
    }
    let mut distance = [0.0f32; 6];
    for (k, d) in distance.iter_mut().enumerate() {
        let p = 0x78 + 0x10 * k as u32;
        *d = ((f(p + 8)? * z + f(p + 4)? * y) + f(p)? * x) - f(p + 0xc)?;
        if *d > radius {
            return Ok(0);
        }
    }
    // The fifth plane's distance is not kept (its register is reused), so
    // the inside test reads planes 0 to 3 and 5.
    let inside = 0.0 - radius;
    let all = [0, 1, 2, 3, 5].iter().all(|&k| inside > distance[k]);
    Ok(if all { 2 } else { 1 })
}

/// `0x00165010` (EAX `region`, EDI `frusta`, XMM7 `radius`): whether the
/// sphere meets a window frustum of the region.
fn sphere_in_region(
    m: &dyn GuestMemory,
    region: i32,
    frusta: u32,
    centre: [f32; 3],
    radius: f32,
) -> Result<bool> {
    let base = (region.wrapping_mul(0x1a) as u32)
        .wrapping_add(frusta)
        .wrapping_add(0xa6e);
    let windows = i32::from(m.u16(frusta)? as i16);
    let mut seen = false;
    let mut inside = false;
    for i in 0..windows.max(0) as u32 {
        let counts = base.wrapping_add(2 + 2 * i);
        let mut j = 0i32;
        if i32::from(m.u16(counts)? as i16) <= 0 {
            continue;
        }
        loop {
            if inside {
                break;
            }
            let index = i32::from(m.u16(counts.wrapping_add(0xc))? as i16).wrapping_add(j);
            let frustum = (index.wrapping_mul(0x108) as u32)
                .wrapping_add(frusta)
                .wrapping_add(0x1974);
            let k = sphere_in_frustum(m, frustum, centre, radius)?;
            if k != 0 {
                inside = k == 2;
                seen = true;
            }
            j += 1;
            if j >= i32::from(m.u16(counts)? as i16) {
                break;
            }
        }
    }
    Ok(seen)
}

/// `0x00249bb0` (EAX `definition`, XMM1 `distance`): the distance fade.
pub fn fade(m: &dyn GuestMemory, definition: u32, distance: f32) -> Result<f32> {
    let one = m.f32(ONE)?;
    let o = m.u32(FADE_OVERRIDE)?;
    if o != 0 && m.u8(o.wrapping_add(5))? != 0 {
        return Ok(one);
    }
    let d = |o: u32| m.f32(definition.wrapping_add(o));
    let (near, far) = (d(0x18)?, d(0x24)?);
    if !(distance > near) || !(far > distance) {
        return Ok(0.0);
    }
    if d(0x1c)? + near > distance {
        return Ok((distance - near) * d(0x20)?);
    }
    if distance > far - d(0x28)? {
        return Ok((far - distance) * d(0x2c)?);
    }
    Ok(one)
}

/// The x87 distance `0x001747eb..0x00174813`: `sqrt((z² + y²) + x²)`
/// times the scale, stored as `f32`, at the control word `cw`'s precision.
pub fn distance(d: [f32; 3], scale: f32, cw: u16) -> Result<f32> {
    if (cw >> 10) & 3 != 0 {
        return Err(Refusal::X87);
    }
    let [x, y, z] = d;
    let v = match (cw >> 8) & 3 {
        // 24-bit: every step rounds to single precision.
        0 => {
            let sum = (z * z + y * y) + x * x;
            sum.sqrt() * scale
        }
        // 53-bit: double precision, rounded once at the store.
        2 => {
            let (x, y, z) = (f64::from(x), f64::from(y), f64::from(z));
            (((z * z + y * y) + x * x).sqrt() * f64::from(scale)) as f32
        }
        _ => return Err(Refusal::X87),
    };
    if !v.is_finite() {
        return Err(Refusal::DistanceRange);
    }
    Ok(v)
}

/// The view's axes as `0x001744ff..0x001745a3` keeps them: forward, up,
/// and their cross product.
#[derive(Clone, Copy)]
struct Axes {
    forward: [f32; 3],
    up: [f32; 3],
    side: [f32; 3],
}

impl Axes {
    fn read(m: &dyn GuestMemory) -> Result<Self> {
        let forward = m.vec3(VIEW_FORWARD)?;
        let up = m.vec3(VIEW_UP)?;
        let ([fx, fy, fz], [ux, uy, uz]) = (forward, up);
        let side = [fz * uy - uz * fy, uz * fx - fz * ux, fy * ux - uy * fx];
        Ok(Self { forward, up, side })
    }

    /// `0x00174621..0x001746be`: `p` in the view's axes, from the camera.
    fn place(&self, p: [f32; 3], camera: [f32; 3]) -> [f32; 3] {
        let ([fx, fy, fz], [ux, uy, uz], [s0, s1, s2]) = (self.forward, self.up, self.side);
        let [px, py, pz] = p;
        let x = (py * s0 + pz * ux) + px * fx;
        let y = (px * fy + py * s1) + pz * uy;
        let z = (px * fz + py * s2) + pz * uz;
        [camera[0] + x, camera[1] + y, camera[2] + z]
    }
}

/// The model of one outermost call.
struct Model<'a> {
    mem: Overlay<'a>,
    cw: u16,
    entries: Vec<u32>,
}

/// What `0x00174420(args)` writes, from `mem` at its entry, with the x87
/// control word `cw`.
pub fn predict(mem: &dyn GuestMemory, args: [u32; 4], cw: u16) -> Result<Prediction> {
    let mut model = Model {
        mem: Overlay::new(mem),
        cw,
        entries: Vec::new(),
    };
    model.update(args, 0)?;
    Ok(Prediction {
        writes: model.mem.writes().clone(),
        entries: model.entries,
    })
}

impl Model<'_> {
    fn update(&mut self, args: [u32; 4], depth: u32) -> Result<()> {
        if depth > DEPTH {
            return Err(Refusal::Depth);
        }
        let system = self.system(args[0])?;
        let def = definition(&mut self.mem, system)?;
        let tag = self.mem.u32(def.wrapping_add(4))?;
        if particle_model(&mut self.mem, tag)?.is_some() {
            self.particles(system, args)?;
        }
        self.children(system, args, depth)
    }

    fn system(&self, handle: u32) -> Result<u32> {
        Ok(array_data(&self.mem, SYSTEMS, SYSTEM)?.wrapping_add((handle & 0xffff) * SYSTEM))
    }

    fn particles(&mut self, system: u32, args: [u32; 4]) -> Result<()> {
        let [_, first_person, flag, third] = args.map(|a| a as u8);
        let m = &mut self.mem;
        let def = definition(m, system)?;
        let tag = m.u32(def.wrapping_add(4))?;
        let reference = match tag_group(m, tag)? {
            PRTM | PRT3 => {
                let object = particle_model(m, tag)?.ok_or(Refusal::NoModel)?;
                shader_reference(m, object)?
            }
            _ => m.u32(tag_data(m, tag)?.wrapping_add(0x24))?,
        };
        let shader = tag_data(m, m.u32(reference)?)?;
        let flags = m.u32(
            m.u32(m.u32(shader.wrapping_add(0x5c))?.wrapping_add(4))?
                .wrapping_add(2),
        )?;
        let def = definition(m, system)?;
        if (m.u8(def.wrapping_add(0x16))? >> 5) & 1 != 0 {
            return Ok(());
        }
        let scale = f32::from(m.u16(def.wrapping_add(0x14))? as i16) * m.f32(OFFSET_SCALE)?;
        let first = m.u32(system.wrapping_add(0x30))?;
        let axes = if flag != 0 {
            Some(Axes::read(m)?)
        } else {
            None
        };
        let mut next = first;
        let mut links = 0;
        while next != u32::MAX {
            links += 1;
            if links > LINKS {
                return Err(Refusal::Cycle);
            }
            let particle = array_data(&self.mem, PARTICLES, PARTICLE)?
                .wrapping_add((next & 0xffff) * PARTICLE);
            let shown = u8::from(self.mem.u8(particle.wrapping_add(2))? != 0);
            if first_person != shown {
                self.stamp(particle)?;
            } else if let Some(axes) = axes {
                let at = axes.place(
                    self.mem.vec3(particle.wrapping_add(0x10))?,
                    self.mem.vec3(CAMERA)?,
                );
                self.stamp(particle)?;
                self.draw(system, particle, at, scale, flags)?;
            } else if self.cull(system, def, particle, third)? {
                let at = self.mem.vec3(particle.wrapping_add(0x10))?;
                self.draw(system, particle, at, scale, flags)?;
            }
            next = self.mem.u32(particle.wrapping_add(0xc))?;
        }
        Ok(())
    }

    fn stamp(&mut self, particle: u32) -> Result<()> {
        let frame = self.mem.u32(FRAME)?;
        Ok(self.mem.set_u32(particle.wrapping_add(0x30), frame)?)
    }

    /// System `+0xd` bit 2: the system was forced visible.
    fn forced(&mut self, system: u32, set: bool) -> Result<()> {
        let at = system.wrapping_add(0xd);
        let b = self.mem.u8(at)?;
        Ok(self.mem.set_u8(at, if set { b | 4 } else { b & !4 })?)
    }

    /// `0x001746ca..0x0017483a`: whether a particle outside the view's
    /// axes path is drawn, with its stamps.
    fn cull(&mut self, system: u32, def: u32, particle: u32, third: u8) -> Result<bool> {
        let m = &self.mem;
        let cluster = i32::from(m.u16(system.wrapping_add(0x20))? as i16);
        let forced = view_cluster(m, cluster)? || (m.u16(system.wrapping_add(0xc))? >> 10) & 1 != 0;
        let portal = (m.u16(def.wrapping_add(0x16))? >> 9) & 1 != 0;
        let fade_out = if third == 0 && !portal && !cluster_bit(m, cluster)? {
            if !forced {
                // Drawn without a stamp (`0x00174729`).
                return Ok(true);
            }
            self.forced(system, true)?;
            false
        } else if forced {
            self.forced(system, true)?;
            false
        } else if third != 0 {
            self.forced(system, false)?;
            true
        } else {
            let centre = m.vec3(particle.wrapping_add(0x20))?;
            let radius = m.f32(particle.wrapping_add(0x2c))?;
            let frusta = m.u32(FRUSTA)?;
            let seen = if portal {
                sphere_in_frustum(m, frusta.wrapping_add(0x94), centre, radius)? != 0
            } else {
                let region = i32::from(m.u8(CLUSTER_REGION.wrapping_add(cluster as u32))? as i8);
                sphere_in_region(m, region, frusta, centre, radius)?
            };
            if !seen {
                return Ok(false);
            }
            self.forced(system, false)?;
            true
        };
        let m = &self.mem;
        let drawn = if fade_out {
            let p = m.vec3(particle.wrapping_add(0x10))?;
            let c = m.vec3(CAMERA)?;
            let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            let at = distance(d, m.f32(DISTANCE_SCALE)?, self.cw)?;
            fade(m, def, at)? > m.f32(ZERO)?
        } else {
            true
        };
        self.stamp(particle)?;
        Ok(drawn)
    }

    /// `0x0017486b..0x00174907`: the queue offset and the sorted entry.
    fn draw(
        &mut self,
        system: u32,
        particle: u32,
        at: [f32; 3],
        scale: f32,
        flags: u32,
    ) -> Result<()> {
        let f = self.mem.vec3(ENTRY_FORWARD)?;
        let at = [
            f[0] * scale + at[0],
            f[1] * scale + at[1],
            f[2] * scale + at[2],
        ];
        if flags & 0x8000 != 0 {
            self.queue(system, particle, at)?;
        }
        if flags & 0xffff_7fff != 0 {
            return Err(Refusal::ObjectPath);
        }
        Ok(())
    }

    /// `0x0001e4e0` with handler `0x00173c90` and the 8-byte payload
    /// (system, particle).
    fn queue(&mut self, system: u32, particle: u32, at: [f32; 3]) -> Result<()> {
        let m = &mut self.mem;
        let count = m.u32(LIST_END)?;
        if count as i32 >= LIST_CAPACITY as i32 {
            if m.u8(LIST_OVERFLOW)? != 0 {
                m.set_u8(LIST_OVERFLOW, 0)?;
            }
            return Ok(());
        }
        let entry = m
            .u32(LIST_ENTRIES)?
            .wrapping_add(count.wrapping_mul(LIST_STRIDE));
        let c = m.vec3(ENTRY_CAMERA)?;
        let d = [at[0] - c[0], at[1] - c[1], at[2] - c[2]];
        let f = m.vec3(ENTRY_FORWARD)?;
        let depth = 0.0 - ((f[2] * d[2] + f[1] * d[1]) + f[0] * d[0]);
        m.set_u32(LIST_END, count + 1)?;
        m.set_u32(entry, 3)?;
        m.set_f32(entry + 4, depth)?;
        m.set_u32(entry + 8, 0)?;
        m.set_u8(entry + 0xc, 0)?;
        m.set_u32(entry + 0x20, PARTICLE_HANDLER)?;
        m.set_u32(entry + 0x24, system)?;
        m.set_u32(entry + 0x28, particle)?;
        for (i, v) in at.iter().enumerate() {
            m.set_f32(entry + 0x64 + 4 * i as u32, *v)?;
        }
        self.entries.push(entry);
        Ok(())
    }

    fn children(&mut self, system: u32, args: [u32; 4], depth: u32) -> Result<()> {
        let mut next = self.mem.u32(system.wrapping_add(0x40))?;
        let mut links = 0;
        while next != u32::MAX {
            links += 1;
            if links > LINKS {
                return Err(Refusal::Cycle);
            }
            let child = self.system(next)?;
            if self.mem.u16(system.wrapping_add(0xc))? & 0x400 != 0 {
                let v = self.mem.u16(child.wrapping_add(0xc))? | 0x400;
                self.mem.set_u16(child.wrapping_add(0xc), v)?;
            } else {
                let v = self.mem.u8(child.wrapping_add(0xd))? & !4;
                self.mem.set_u8(child.wrapping_add(0xd), v)?;
            }
            self.update([next, args[1], args[2], args[3]], depth + 1)?;
            next = self.mem.u32(child.wrapping_add(0x38))?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    pub const SYS_ARRAY: u32 = 0x0060_0000;
    pub const SYS_DATA: u32 = 0x0060_1000;
    pub const PART_ARRAY: u32 = 0x0061_0000;
    pub const PART_DATA: u32 = 0x0061_1000;
    pub const TAG_TABLE: u32 = 0x0062_0000;
    const DEF: u32 = 0x0063_0000;
    const PRT3_DATA: u32 = 0x0063_1000;
    const SHADER_REF: u32 = 0x0063_2000;
    const SHADER_DATA: u32 = 0x0063_3000;
    const SHADER_A: u32 = 0x0063_4000;
    const SHADER_B: u32 = 0x0063_5000;
    const EFFE_DATA: u32 = 0x0063_6000;
    const EVENTS: u32 = 0x0063_7000;
    pub const ROWS: u32 = 0x0064_0000;
    const FRUSTA_AT: u32 = 0x0065_0000;
    pub const FRAME_NOW: u32 = 77;

    /// Tags: 1 the effect, 2 the particle, 3 the shader.
    pub fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        for (array, data, stride) in [
            (SYS_ARRAY, SYS_DATA, SYSTEM),
            (PART_ARRAY, PART_DATA, PARTICLE),
        ] {
            m.set_u32(array + 0x24, stride).unwrap();
            m.set_u32(array + 0x38, 8).unwrap();
            m.set_u32(array + 0x44, data).unwrap();
        }
        m.set_u32(SYSTEMS, SYS_ARRAY).unwrap();
        m.set_u32(PARTICLES, PART_ARRAY).unwrap();
        m.set_u32(TAGS, TAG_TABLE).unwrap();
        for (i, group, data) in [
            (1, EFFE, EFFE_DATA),
            (2, PRT3, PRT3_DATA),
            (3, 0x7368_6164, SHADER_DATA),
        ] {
            m.set_u32(TAG_TABLE + i * 16, group).unwrap();
            m.set_u32(TAG_TABLE + i * 16 + 4, i).unwrap();
            m.set_u32(TAG_TABLE + i * 16 + 8, data).unwrap();
        }
        // The effect's event 0, part 0: the definition at DEF.
        m.set_u32(EFFE_DATA + 0x18, EVENTS).unwrap();
        m.set_u32(EVENTS + 0x34, DEF).unwrap();
        m.set_u32(DEF + 4, 2).unwrap();
        m.set_u16(DEF + 0x14, 2).unwrap();
        // The particle's shader reference and its flags 0x8000.
        m.set_u32(PRT3_DATA + 0x90, SHADER_REF).unwrap();
        m.set_u32(SHADER_REF, 3).unwrap();
        m.set_u32(SHADER_DATA + 0x5c, SHADER_A).unwrap();
        m.set_u32(SHADER_A + 4, SHADER_B).unwrap();
        m.set_u32(SHADER_B + 2, 0x8000).unwrap();
        m.set_u32(SINGLETONS[0].1, SINGLETONS[0].2).unwrap();
        m.set_u32(SINGLETONS[1].1, SINGLETONS[1].2).unwrap();
        m.set_u32(FRAME, FRAME_NOW).unwrap();
        m.set_f32(OFFSET_SCALE, 0.5).unwrap();
        m.set_f32(ONE, 1.0).unwrap();
        m.set_f32(DISTANCE_SCALE, 1.0).unwrap();
        m.set_f32(ENTRY_FORWARD, 1.0).unwrap();
        m.set_u32(LIST_ENTRIES, ROWS).unwrap();
        m.set_u32(FRUSTA, FRUSTA_AT).unwrap();
        m
    }

    /// System `i`: the effect's, its first particle `first`, no children.
    pub fn system(m: &mut FlatMemory, i: u32, first: u32) -> u32 {
        let s = SYS_DATA + i * SYSTEM;
        m.set_u32(s + 0x10, 1).unwrap();
        m.set_u32(s + 0x30, first).unwrap();
        m.set_u32(s + 0x40, u32::MAX).unwrap();
        m.set_u16(s + 0x20, 5).unwrap();
        s
    }

    /// Particle `i`, shown in third person, at `x`, linked to `next`.
    pub fn particle(m: &mut FlatMemory, i: u32, x: f32, next: u32) -> u32 {
        let p = PART_DATA + i * PARTICLE;
        m.set_u32(p + 0xc, next).unwrap();
        m.set_f32(p + 0x10, x).unwrap();
        p
    }

    #[test]
    fn a_forced_system_stamps_and_queues_each_particle() {
        let mut m = world();
        let s = system(&mut m, 0, 0);
        particle(&mut m, 0, 4.0, 1);
        particle(&mut m, 1, 6.0, u32::MAX);
        m.set_u16(s + 0xc, 0x400).unwrap();
        let before = m.bytes.clone();
        let p = predict(&m, [0, 0, 0, 0], 0x027f).unwrap();
        assert_eq!(m.bytes, before, "the model never writes the guest");
        let mut after = m.clone();
        for (&a, &b) in &p.writes {
            after.set_u8(a, b).unwrap();
        }
        assert_eq!(after.u8(s + 0xd).unwrap(), 0x04);
        for i in 0..2 {
            assert_eq!(
                after.u32(PART_DATA + i * PARTICLE + 0x30).unwrap(),
                FRAME_NOW
            );
        }
        assert_eq!(p.entries, [ROWS, ROWS + LIST_STRIDE]);
        assert_eq!(after.u32(LIST_END).unwrap(), 2);
        // Offset 2 * 0.5 along the entry forward x; depth from the entry camera.
        assert_eq!(after.f32(ROWS + 0x64).unwrap(), 5.0);
        assert_eq!(after.f32(ROWS + 4).unwrap(), -5.0);
        assert_eq!(after.u32(ROWS + 0x20).unwrap(), PARTICLE_HANDLER);
        assert_eq!(after.u32(ROWS + 0x24).unwrap(), s);
        assert_eq!(after.u32(ROWS + 0x28).unwrap(), PART_DATA);
        // The particle singleton holds the particle tag.
        assert_eq!(after.u32(SINGLETONS[0].1 + 4).unwrap(), 2);
        assert_eq!(after.u32(SINGLETONS[0].1 + 8).unwrap(), PRT3_DATA);
    }

    #[test]
    fn the_other_view_stamps_only_and_a_rejected_particle_is_not_stamped() {
        let mut m = world();
        system(&mut m, 0, 0);
        let p0 = particle(&mut m, 0, 4.0, 1);
        let p1 = particle(&mut m, 1, 6.0, u32::MAX);
        // Particle 0 is first person's; the call is third person's.
        m.set_u8(p0 + 2, 1).unwrap();
        // Cluster 5's bit is set, so particle 1 meets the region frusta:
        // no windows, so it is rejected.
        m.set_u32(CLUSTER_BITS, 1 << 5).unwrap();
        let p = predict(&m, [0, 0, 0, 0], 0x027f).unwrap();
        let mut after = m.clone();
        for (&a, &b) in &p.writes {
            after.set_u8(a, b).unwrap();
        }
        assert_eq!(after.u32(p0 + 0x30).unwrap(), FRAME_NOW);
        assert_eq!(after.u32(p1 + 0x30).unwrap(), 0);
        assert!(p.entries.is_empty());
    }

    #[test]
    fn children_take_the_parents_forced_bit_and_are_updated() {
        let mut m = world();
        let parent = system(&mut m, 0, u32::MAX);
        let child = system(&mut m, 1, u32::MAX);
        m.set_u32(parent + 0x40, 1).unwrap();
        m.set_u32(child + 0x38, u32::MAX).unwrap();
        m.set_u16(parent + 0xc, 0x400).unwrap();
        let p = predict(&m, [0, 0, 0, 0], 0x027f).unwrap();
        assert_eq!(p.writes.get(&(child + 0xd)), Some(&0x04));
        // Without the bit, the child's is cleared.
        m.set_u16(parent + 0xc, 0).unwrap();
        m.set_u16(child + 0xc, 0x400).unwrap();
        let p = predict(&m, [0, 0, 0, 0], 0x027f).unwrap();
        assert_eq!(p.writes.get(&(child + 0xd)), Some(&0x00));
    }

    #[test]
    fn what_the_model_cannot_make_is_a_named_refusal() {
        let mut m = world();
        system(&mut m, 0, 0);
        particle(&mut m, 0, 4.0, u32::MAX);
        m.set_u16(SYS_DATA + 0xc, 0x400).unwrap();
        // A shader flag besides 0x8000 calls 0x00040e30.
        m.set_u32(SHADER_B + 2, 0x8001).unwrap();
        assert_eq!(predict(&m, [0, 0, 0, 0], 0x027f), Err(Refusal::ObjectPath));
        m.set_u32(SHADER_B + 2, 0x8000).unwrap();
        // A singleton that is not the retail one.
        m.set_u32(SINGLETONS[0].1, 0).unwrap();
        assert_eq!(
            predict(&m, [0, 0, 0, 0], 0x027f),
            Err(Refusal::SingletonVtable)
        );
        m.set_u32(SINGLETONS[0].1, SINGLETONS[0].2).unwrap();
        // A tag group with no definition.
        m.set_u32(TAG_TABLE + 16, 0x1234_5678).unwrap();
        assert_eq!(
            predict(&m, [0, 0, 0, 0], 0x027f),
            Err(Refusal::NoDefinition)
        );
        assert_eq!(Refusal::ObjectPath.name(), "object_path");
    }

    #[test]
    fn the_distance_keeps_the_x87_precision_or_refuses() {
        assert_eq!(distance([3.0, 4.0, 0.0], 2.0, 0x027f), Ok(10.0));
        assert_eq!(distance([3.0, 4.0, 0.0], 2.0, 0x007f), Ok(10.0));
        assert_eq!(distance([3.0, 4.0, 0.0], 2.0, 0x037f), Err(Refusal::X87));
        assert_eq!(distance([3.0, 4.0, 0.0], 2.0, 0x0e7f), Err(Refusal::X87));
        assert_eq!(
            distance([f32::MAX, 0.0, 0.0], 1.0, 0x007f),
            Err(Refusal::DistanceRange)
        );
    }

    #[test]
    fn the_fade_and_the_frustum_follow_the_guests_compares() {
        let mut m = world();
        let d = 0x0066_0000;
        for (o, v) in [
            (0x18, 1.0f32),
            (0x1c, 1.0),
            (0x20, 0.5),
            (0x24, 10.0),
            (0x28, 2.0),
            (0x2c, 0.25),
        ] {
            m.set_f32(d + o, v).unwrap();
        }
        assert_eq!(fade(&m, d, 0.5).unwrap(), 0.0);
        assert_eq!(fade(&m, d, 1.5).unwrap(), 0.25);
        assert_eq!(fade(&m, d, 5.0).unwrap(), 1.0);
        assert_eq!(fade(&m, d, 9.0).unwrap(), 0.25);
        assert_eq!(fade(&m, d, f32::NAN).unwrap(), 0.0);
        // A box of +-10 and six planes that all keep the origin inside.
        let f = 0x0067_0000;
        for (o, v) in [
            (0x60, -10.0f32),
            (0x64, 10.0),
            (0x68, -10.0),
            (0x6c, 10.0),
            (0x70, -10.0),
            (0x74, 10.0),
        ] {
            m.set_f32(f + o, v).unwrap();
        }
        for k in 0..6 {
            m.set_f32(f + 0x78 + 0x10 * k + 0xc, 5.0).unwrap();
            m.set_f32(f + 0x78 + 0x10 * k, 1.0).unwrap();
        }
        assert_eq!(sphere_in_frustum(&m, f, [0.0; 3], 1.0).unwrap(), 2);
        assert_eq!(sphere_in_frustum(&m, f, [5.0, 0.0, 0.0], 1.0).unwrap(), 1);
        assert_eq!(sphere_in_frustum(&m, f, [7.0, 0.0, 0.0], 1.0).unwrap(), 0);
        assert_eq!(sphere_in_frustum(&m, f, [0.0, 12.0, 0.0], 1.0).unwrap(), 0);
    }
}
