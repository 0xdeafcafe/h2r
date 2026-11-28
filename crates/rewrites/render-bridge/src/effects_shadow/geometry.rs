//! R2's geometry arm (docs/graphics/retire-d3d-r2.md, "R2's geometry
//! arm"): what the standard transparent handler `0x00041020` makes per
//! entry, made in Rust.
//!
//! The handler (`0x00040f60`, with the opaque path's build `0x0004cbf0`
//! and draw `0x0004d0b0`) reads the entry's shader datum (`+0x24`) and
//! render argument (`+0x28`: record index `<< 8`, the low byte the part),
//! takes the template's layer-15 word (pass count `>> 9`), and per pass
//! `i` calls the build `(shader, 0, 0, 15, i, argument, slot)` and, when
//! its AL is set, the draw. Layer 15 takes the build's generic arm (the
//! jump table `0x0004d054` sends layers 4 to 15 to `0x0004ce0f`), the
//! one [`crate::native_world::scene_build::build`] ports.
//!
//! On every frame the handler's build call does not run: the slot and AL
//! are made in Rust and the build returns (`ret 28`), and a BSP
//! cluster's, a placement's or an object's entry is drawn natively by the
//! answer host ([`answer_handler`]). No frame is the guest's for
//! comparison. Shaders in the particle material groups (`PRTM`, `prt3`, a
//! virtual call) are refused by name (`scene_kept_15_geometry_<name>`)
//! and left to the particle path.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;

use super::{count, view_update, with_gate};
use crate::native_world::scene_build;

/// The handler's return address after its call of the build (`0x00040ff3`).
pub const BUILD_RETURN: u32 = 0x0004_0ff7;
/// The layer the handler draws.
pub const LAYER: u32 = 15;
/// What the build pops after its return address (seven arguments).
pub const POP: u32 = 28;
/// The sort slot's size.
const SLOT: usize = 24;
/// The most passes a template has (a layer span's 7 bits).
const MAX_PASSES: u32 = 128;

const PRTM: u32 = u32::from_be_bytes(*b"PRTM");
const PRT3: u32 = u32::from_be_bytes(*b"prt3");
const SHAD: u32 = u32::from_be_bytes(*b"shad");

/// Why the model makes nothing, named in `geometry_refused_<name>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A guest read faulted.
    Fault,
    /// The shader is a particle material (`PRTM`, `prt3`): the guest calls its virtual function.
    ParticleMaterial,
    /// The shader datum is not a `shad` tag.
    NotShader,
    /// The render argument's record index is out of range.
    Record,
    /// The template's pass count is out of range.
    Passes,
    /// The build's chain of tags does not read as the model needs.
    Layout,
}

impl Refusal {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fault => "fault",
            Self::ParticleMaterial => "particle_material",
            Self::NotShader => "not_shader",
            Self::Record => "record",
            Self::Passes => "passes",
            Self::Layout => "layout",
        }
    }
}

/// The build's arguments at its entry (`esp` its return address).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub shader: u32,
    pub lod: u8,
    pub selector: u8,
    pub argument: u32,
    pub slot: u32,
}

/// The handler's own call of the build for layer 15; `None` for any other.
pub fn args(m: &dyn GuestMemory, esp: u32) -> Option<Args> {
    if m.u32(esp).ok()? != BUILD_RETURN {
        return None;
    }
    let arg = |n: u32| m.u32(esp.checked_add(4 + 4 * n)?).ok();
    (arg(3)? == LAYER).then_some(Args {
        shader: arg(0)?,
        lod: arg(2)? as u8,
        selector: arg(4)? as u8,
        argument: arg(5)?,
        slot: arg(6)?,
    })
}

fn tag_group(m: &dyn GuestMemory, table: u32, datum: u32) -> Result<u32, Refusal> {
    if datum as u16 > i16::MAX as u16 {
        return Err(Refusal::Layout);
    }
    let at = table.checked_add((datum & 0xffff) * 16).ok_or(Refusal::Layout)?;
    m.u32(at).map_err(|_| Refusal::Fault)
}

fn shader_group(m: &dyn GuestMemory, shader: u32) -> Result<(), Refusal> {
    let table = m.u32(engine::tags::TAG_ENTRIES_GLOBAL).map_err(|_| Refusal::Fault)?;
    match tag_group(m, table, shader)? {
        PRTM | PRT3 => Err(Refusal::ParticleMaterial),
        SHAD => Ok(()),
        _ => Err(Refusal::NotShader),
    }
}

/// The passes the handler runs for `shader`: the template's layer-15 word
/// `>> 9` (`0x00040fbb..0x00040fd6`), with the first LOD's layer base.
pub fn passes(m: &dyn GuestMemory, shader: u32) -> Result<u32, Refusal> {
    shader_group(m, shader)?;
    let rd = |at: Option<u32>| at.ok_or(Refusal::Layout).and_then(|a| m.u32(a).map_err(|_| Refusal::Fault));
    let nonzero = |v: u32| if v == 0 { Err(Refusal::Layout) } else { Ok(v) };
    let table = nonzero(rd(Some(engine::tags::TAG_ENTRIES_GLOBAL))?)?;
    let data = |datum: u32| nonzero(rd(table.checked_add((datum & 0xffff) * 16 + 8))?);
    let material = data(shader)?;
    let instance = nonzero(rd(material.checked_add(0x24))?)?;
    let template = data(rd(Some(instance))?)?;
    let header = nonzero(rd(template.checked_add(0x5c))?)?;
    let lods = nonzero(rd(header.checked_add(4))?)?;
    let layers = nonzero(rd(header.checked_add(12))?)?;
    let base = u32::from(m.u16(lods).map_err(|_| Refusal::Fault)? & 511);
    let at = layers.checked_add((base + LAYER) * 2).ok_or(Refusal::Layout)?;
    let count = u32::from(m.u16(at).map_err(|_| Refusal::Fault)?) >> 9;
    if count > MAX_PASSES {
        return Err(Refusal::Passes);
    }
    Ok(count)
}

/// What the build leaves: the slot and AL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Built {
    pub slot: [u8; SLOT],
    pub al: bool,
}

/// Makes the build's slot and AL for `a`, from the slot as it stands.
pub fn predict(m: &dyn GuestMemory, a: &Args) -> Result<Built, Refusal> {
    let mut old = [0; SLOT];
    m.read(a.slot, &mut old).map_err(|_| Refusal::Fault)?;
    predict_from(m, a, old)
}

/// [`predict`] with the slot's bytes as given (the slot itself is not read).
pub fn predict_from(m: &dyn GuestMemory, a: &Args, old: [u8; SLOT]) -> Result<Built, Refusal> {
    shader_group(m, a.shader)?;
    if (a.argument >> 8) as u16 >= 0x352 {
        return Err(Refusal::Record);
    }
    let e = scene_build::build(m, a.shader, a.lod, LAYER, a.selector, a.argument, old, true)
        .ok_or(Refusal::Layout)?;
    Ok(Built { slot: e.bytes, al: e.accepted })
}

/// The frame this hook runs in, counted by the shared gate.
fn frame(m: &dyn GuestMemory) -> Option<()> {
    let frame = m.u32(view_update::FRAME).ok()?;
    with_gate(|g| g.frame(frame));
    Some(())
}

/// At the build's entry from the handler: `Some(al)` when the slot is
/// made here and the build returns instead (`ret 28`); `None` runs the
/// guest's build, kept by name.
pub(crate) fn build(m: &mut dyn GuestMemory, esp: u32) -> Option<bool> {
    let a = args(&*m, esp)?;
    frame(&*m)?;
    let built = match predict(&*m, &a) {
        Ok(built) => built,
        Err(why) => {
            crate::native_layers::count_kept_as("", 15, "geometry", why.name(), 1);
            return None;
        }
    };
    if m.write(a.slot, &built.slot).is_err() {
        count("answer_fault", 1);
        return None;
    }
    count("answer_geometry", 1);
    Some(built.al)
}

/// Which structure geometry an answered entry draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// A BSP cluster, by index.
    Cluster(u32),
    /// An instanced-geometry placement, by index.
    Placement(u32),
    /// An object, by handle: its model's passes, lit as the object is.
    Object(u32),
}

/// One entry the answer draws natively: a BSP cluster's or a placement's
/// part of a shader whose template has `passes` passes in layer 15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub shader: u32,
    pub owner: Owner,
    /// The entry's render record (what the scene's item keys on).
    pub record: u16,
    pub part: u8,
    pub passes: u32,
}

/// The native draw of an entry's passes (the game glue,
/// `native_layers_glue::answer_transparent`): `Err` names why the guest's
/// handler runs instead.
pub trait AnswerHost: Send + Sync {
    fn draw(&self, mem: &mut dyn GuestMemory, request: &Request) -> Result<(), &'static str>;
}

static HOST: Mutex<Option<Arc<dyn AnswerHost>>> = Mutex::new(None);

pub fn set_answer_host(host: Option<Arc<dyn AnswerHost>>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = host;
}

struct Ro<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

fn kept(cause: &str) {
    crate::native_layers::count_kept_as("", 15, "geometry_draw", cause, 1);
}

/// At the handler's entry (`esp` its return address). True when the entry
/// is drawn natively and the handler returns (`ret 4`); false runs the
/// guest's. For a BSP cluster's, a placement's or an object's part whose
/// every pass the build model accepts with the generic implementation 0
/// (the one the layer's compile took).
pub(crate) fn answer_handler(m: &mut dyn GuestMemory, esp: u32) -> bool {
    use native_render::transparent::{Source, read_entry};
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return false };
    if frame(&*m).is_none() {
        kept("frame");
        return false;
    }
    let Some(data) = esp.checked_add(4).and_then(|a| m.u32(a).ok()) else { return false };
    let Some(entry) = read_entry(&Ro(&*m), data) else {
        kept("entry");
        return false;
    };
    let owner = match entry.source {
        Source::Cluster(c) => Owner::Cluster(u32::from(c)),
        Source::Placement(p) => Owner::Placement(u32::from(p)),
        Source::Object(handle) => Owner::Object(handle),
    };
    let Some(argument) = data.checked_add(4).and_then(|a| m.u32(a).ok()) else { return false };
    let passes = match passes(&*m, entry.shader) {
        Ok(n) if n > 0 => n,
        Ok(_) => {
            kept("no_passes");
            return false;
        }
        Err(why) => {
            kept(why.name());
            return false;
        }
    };
    for selector in 0..passes {
        let a = Args { shader: entry.shader, lod: 0, selector: selector as u8, argument, slot: 0 };
        match predict_from(&*m, &a, [0; SLOT]) {
            Ok(b) if b.al && b.slot[4] == 0 => {}
            Ok(_) => {
                kept("pass");
                return false;
            }
            Err(why) => {
                kept(why.name());
                return false;
            }
        }
    }
    let request = Request { shader: entry.shader, owner, record: entry.record, part: entry.part, passes };
    match host.draw(m, &request) {
        Ok(()) => {
            count("answer_geometry_draw", 1);
            true
        }
        Err(cause) => {
            kept(cause);
            false
        }
    }
}

/// A snapshot load: the arm holds nothing across frames.
pub(super) fn forget() {}

#[cfg(test)]
mod tests;
