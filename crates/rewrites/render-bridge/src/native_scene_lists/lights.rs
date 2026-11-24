//! The seams of the light leaves: each reads the guest function's
//! registers and stack, calls the leaf in `engine::render::light_gather`
//! (docs/graphics/native-light-gather.md, slice L1) and publishes its
//! writes. The main view's light candidates (`Candidates`) are gathered
//! here with the object gather.

use super::objects::{List, Row, Sphere};
use super::*;
use engine::light_constants::Fpu;
use engine::render::light_gather::{self as gather, extra_caster, geometry, visible};

const LIGHTS: u32 = engine::render::light::ARRAY_POINTER;
const LIGHT_BYTES: u32 = engine::render::light::RECORD_SIZE as u32;
const LIGHT_EPOCH: u32 = 0x004e_0308;
pub(super) use visible::{MIN_DEPTH, PIXEL_SCALE, PROJECTION_DEPTH, SIZE_FACTOR};

impl From<gather::Refusal> for Refusal {
    fn from(r: gather::Refusal) -> Self {
        match r {
            gather::Refusal::Memory => Self::Memory,
            gather::Refusal::Layout => Self::Layout,
            gather::Refusal::Unsupported => Self::Unsupported,
        }
    }
}

impl Output {
    fn writes(&mut self, writes: gather::Writes) {
        for w in writes {
            self.array(w.addr, w.bytes);
        }
    }
}

fn pool(m: &dyn GuestMemory) -> Result<Pool> {
    Pool::read(m, ptr(m, LIGHTS)?, LIGHT_BYTES)
}

fn active(m: &dyn GuestMemory, light: u32) -> Result<bool> {
    Ok(visible::active(m, light)?)
}

pub(super) fn root_owner(m: &dyn GuestMemory, owner: u32) -> Result<u32> {
    Ok(visible::root_owner(m, owner)?)
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
                || !visible::player_light_visible(m, row, flags, m.u32(at(row, 0x4c)?)?)?
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

pub(super) fn candidate_eligible(m: &dyn GuestMemory, light: u32, definition: u32, flags: u32) -> Result<bool> {
    Ok(visible::candidate_eligible(m, light, definition, flags)?)
}

fn read_visible(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    let output_addr = stack(m, cpu, 4)?;
    let list = visible::list(m, output_addr, Fpu { mxcsr: cpu.mxcsr, x87_control: cpu.x87.control })?;
    let mut out = Output::default();
    out.writes(list.writes);
    out.eax = Some(list.count);
    Ok(out)
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
    let vis = if cpu.ecx == NONE { 0 } else { stack(m, cpu, 4)? };
    let mut out = Output::default();
    out.writes(extra_caster::list(m, cpu.eax as u8, cpu.ecx, vis)?);
    Ok(out)
}

fn geometry_flags(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    let args = geometry::Arguments {
        light: cpu.eax,
        definition: cpu.ecx,
        vis: cpu.esi,
        owner: stack(m, cpu, 4)?,
        root: stack(m, cpu, 8)?,
        point: stack(m, cpu, 12)?,
        shadow: stack(m, cpu, 16)?,
    };
    let mut out = Output::default();
    out.writes(geometry::flags(m, args)?);
    Ok(out)
}
