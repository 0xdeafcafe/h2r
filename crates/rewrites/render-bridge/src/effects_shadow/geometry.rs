//! R2's geometry arm (docs/graphics/retire-d3d-r2.md, "R2's geometry
//! arm"): what the standard transparent handler `0x00041020` makes per
//! entry, made in Rust and compared with the guest's.
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
//! Two things are compared, on sampled frames, through the shared frame
//! sample:
//!
//! - **Passes** (handler entry to return): the pass count the template's
//!   layer-15 word gives, against the builds the handler called, each with
//!   its pass number in order.
//! - **Build** (each build the handler calls, its entry to its return):
//!   the 24-byte sort slot and AL the model makes, against the guest's.
//!
//! The arm has a gate of its own: [`CLEAN`] sampled frames in a row
//! compared clean before it answers, and a difference or refusal stops it.
//! While it answers, on the frames not sampled, the handler's build call
//! does not run: the slot and AL are made in Rust and the build returns
//! (`ret 28`). The draw, the material's textures, samplers and constants
//! stay the guest's. Shaders in the particle material groups (`PRTM`,
//! `prt3`, a virtual call) are refused by name and left to R2's particle
//! lane.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};

use engine::memory::GuestMemory;

use super::{count, view_update, with_gate};
use crate::native_world::scene_build;

/// The handler's return address after its call of the build (`0x00040ff3`).
pub const BUILD_RETURN: u32 = 0x0004_0ff7;
/// The layer the handler draws.
pub const LAYER: u32 = 15;
/// What the build pops after its return address (seven arguments).
pub const POP: u32 = 28;
/// Clean sampled frames in a row before the arm answers.
pub const CLEAN: u32 = 16;
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

/// The arm's own gate: clean sampled frames in a row, and whether it answers.
#[derive(Debug, Default)]
pub struct Proof {
    frame: Option<u32>,
    sampled: bool,
    clean: bool,
    compared: u32,
    streak: u32,
    answering: bool,
}

impl Proof {
    /// A hook runs in `frame`; `sampled` is the shared sample's verdict.
    fn frame(&mut self, frame: u32, sampled: bool) {
        if self.frame == Some(frame) {
            return;
        }
        if self.sampled && self.clean && self.compared > 0 {
            self.streak += 1;
            if self.streak >= CLEAN && !self.answering {
                self.answering = true;
                count("geometry_opened", 1);
            }
        }
        self.frame = Some(frame);
        self.sampled = sampled;
        self.clean = true;
        self.compared = 0;
    }

    fn matched(&mut self, what: &str) {
        self.compared += 1;
        count(&format!("geometry_{what}_matched"), 1);
    }

    fn spoil(&mut self) {
        self.clean = false;
        self.streak = 0;
        if std::mem::take(&mut self.answering) {
            count("geometry_fallback", 1);
        }
    }

    fn differed(&mut self, what: &str, field: &str) {
        self.compared += 1;
        count(&format!("geometry_{what}_mismatched_{field}"), 1);
        self.spoil();
    }

    /// A refusal leaves that entry's build to the guest and counts it by
    /// name; only a difference ends the streak.
    fn refused(&mut self, why: Refusal) {
        count(&format!("geometry_refused_{}", why.name()), 1);
    }

    pub fn answering(&self) -> bool {
        self.answering
    }
}

#[cfg(not(test))]
static PROOF: std::sync::Mutex<Option<Proof>> = std::sync::Mutex::new(None);

#[cfg(not(test))]
fn with_proof<R>(f: impl FnOnce(&mut Proof) -> R) -> R {
    f(PROOF.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Proof::default))
}

#[cfg(test)]
thread_local! {
    static PROOF: RefCell<Proof> = RefCell::new(Proof::default());
}

#[cfg(test)]
fn with_proof<R>(f: impl FnOnce(&mut Proof) -> R) -> R {
    PROOF.with(|p| f(&mut p.borrow_mut()))
}

struct OpenBuild {
    esp: u32,
    slot: u32,
    built: Built,
}

struct OpenHandler {
    esp: u32,
    passes: u32,
    seen: u32,
    wrong_pass: bool,
}

/// The pass of the handler's latest build, for the draw that follows it:
/// the draw's material is `layer 15, pass `selector`, implementation `imp``
/// of the shader's template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    pub shader: u32,
    pub selector: u8,
    pub imp: u8,
    frame: u32,
}

/// Material samples in a row that matched the device.
static MATERIAL_STREAK: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static CURRENT: Cell<Option<Pass>> = const { Cell::new(None) };
    static BUILD: RefCell<Option<OpenBuild>> = const { RefCell::new(None) };
    static HANDLER: RefCell<Option<OpenHandler>> = const { RefCell::new(None) };
}

/// The frame this hook runs in, and whether the shared sample compares it.
fn sampled(m: &dyn GuestMemory) -> Option<bool> {
    let frame = m.u32(view_update::FRAME).ok()?;
    let sampled = with_gate(|g| g.frame(frame));
    with_proof(|p| p.frame(frame, sampled));
    Some(sampled)
}

/// At the handler's entry (`esp` its return address, the entry's `+0x24`
/// the argument).
pub(super) fn begin_handler(m: &dyn GuestMemory, esp: u32) {
    if HANDLER.with(|h| h.borrow_mut().take()).is_some() {
        count("geometry_handler_unbalanced", 1);
    }
    if sampled(m) != Some(true) {
        return;
    }
    let shader = esp.checked_add(4).and_then(|a| m.u32(a).ok()).and_then(|d| m.u32(d).ok());
    let Some(shader) = shader else { return };
    match passes(m, shader) {
        Ok(passes) => HANDLER.with(|h| *h.borrow_mut() = Some(OpenHandler { esp, passes, seen: 0, wrong_pass: false })),
        Err(why) => with_proof(|p| p.refused(why)),
    }
}

/// At the handler's return (`esp` past its argument).
pub(super) fn end_handler(esp: u32) {
    let Some(open) = HANDLER.with(|h| h.borrow_mut().take()) else { return };
    if open.esp.wrapping_add(8) != esp {
        count("geometry_handler_unbalanced", 1);
        return;
    }
    with_proof(|p| match (open.wrong_pass, open.seen == open.passes) {
        (false, true) => p.matched("passes"),
        (true, _) => p.differed("passes", "order"),
        (_, false) => p.differed("passes", "count"),
    });
}

/// At the build's entry from the handler. Opens the arm on a sampled frame
/// or while the arm does not answer; returns `Some(al)` when the slot is
/// made here and the build returns instead (`ret 28`).
pub(crate) fn build(m: &mut dyn GuestMemory, esp: u32) -> Option<bool> {
    let a = args(&*m, esp)?;
    let sampled = sampled(&*m)?;
    HANDLER.with(|h| {
        if let Some(h) = h.borrow_mut().as_mut() {
            h.wrong_pass |= u32::from(a.selector) != h.seen;
            h.seen += 1;
        }
    });
    if BUILD.with(|b| b.borrow_mut().take()).is_some() {
        count("geometry_build_unbalanced", 1);
    }
    let answer = !sampled && with_proof(|p| p.answering());
    CURRENT.set(None);
    let built = match predict(&*m, &a) {
        Ok(built) => built,
        Err(why) => {
            if sampled {
                with_proof(|p| p.refused(why));
            } else {
                crate::native_layers::count_kept_as("", 15, "geometry", why.name(), 1);
            }
            return None;
        }
    };
    if built.al && built.slot[4] != u8::MAX {
        let frame = m.u32(view_update::FRAME).unwrap_or(0);
        CURRENT.set(Some(Pass { shader: a.shader, selector: a.selector, imp: built.slot[4], frame }));
    }
    if answer {
        if m.write(a.slot, &built.slot).is_err() {
            count("answer_fault", 1);
            return None;
        }
        count("answer_geometry", 1);
        return Some(built.al);
    }
    if sampled {
        BUILD.with(|b| *b.borrow_mut() = Some(OpenBuild { esp, slot: a.slot, built }));
    }
    None
}

/// At the build's return (`esp` past its arguments, `eax` its result).
pub(super) fn end_build(m: &dyn GuestMemory, esp: u32, eax: u32) {
    let Some(open) = BUILD.with(|b| b.borrow_mut().take()) else { return };
    if open.esp.wrapping_add(4 + POP) != esp {
        count("geometry_build_unbalanced", 1);
        return;
    }
    let mut slot = [0; SLOT];
    let field = if m.read(open.slot, &mut slot).is_err() {
        Some("fault")
    } else if (eax as u8 != 0) != open.built.al {
        Some("return")
    } else if slot != open.built.slot {
        Some("slot")
    } else {
        None
    };
    with_proof(|p| match field {
        None => p.matched("build"),
        Some(f) => p.differed("build", f),
    });
}

/// At a transparent geometry draw (the indexed call after the build): the
/// pass whose material is compared with the device's, on a sampled frame,
/// once. `None` on any other frame, or when no build of this frame made one.
pub fn material_sample(m: &dyn GuestMemory) -> Option<Pass> {
    if sampled(m)? != true {
        return None;
    }
    let pass = CURRENT.take()?;
    (m.u32(view_update::FRAME).ok()? == pass.frame).then_some(pass)
}

/// A sampled draw whose material the tags did not give, named
/// (`geometry_material_refused_<why>`).
pub fn material_refused(why: &str) {
    count(&format!("geometry_material_refused_{why}"), 1);
}

/// The comparison of a sampled draw's material (`transparent_material::compare`):
/// `fields` where the device differs, `skipped` what was not compared. A
/// difference ends the streak; [`material_proven`] holds after [`CLEAN`]
/// matches in a row. Counted `geometry_material_{matched,mismatched_<field>,skipped_<part>,proven}`.
pub fn material_record(fields: &[String], skipped: &[&str]) {
    for part in skipped {
        count(&format!("geometry_material_skipped_{part}"), 1);
    }
    if fields.is_empty() {
        count("geometry_material_matched", 1);
        if MATERIAL_STREAK.fetch_add(1, Ordering::Relaxed) + 1 == CLEAN {
            count("geometry_material_proven", 1);
        }
        return;
    }
    MATERIAL_STREAK.store(0, Ordering::Relaxed);
    for field in fields {
        count(&format!("geometry_material_mismatched_{field}"), 1);
    }
}

/// Whether the last [`CLEAN`] sampled draws' materials matched the device.
pub fn material_proven() -> bool {
    MATERIAL_STREAK.load(Ordering::Relaxed) >= CLEAN
}

/// One entry the answer draws natively: a BSP cluster's part of a shader
/// whose template has `passes` passes in layer 15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub shader: u32,
    pub cluster: u32,
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

/// Whether the answer is open: the build answer and the material both
/// proved, [`CLEAN`] sampled draws in a row each, and no difference since.
pub fn proven() -> bool {
    with_proof(|p| p.answering()) && material_proven()
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
/// guest's. Only on a frame that is not sampled, with [`proven`], for a BSP
/// cluster's part whose every pass the build model accepts with the
/// generic implementation 0 (the one the layer's compile took).
pub(crate) fn answer_handler(m: &mut dyn GuestMemory, esp: u32) -> bool {
    use native_render::transparent::{Source, read_entry};
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return false };
    if sampled(&*m) != Some(false) || !proven() {
        return false;
    }
    let Some(data) = esp.checked_add(4).and_then(|a| m.u32(a).ok()) else { return false };
    let Some(entry) = read_entry(&Ro(&*m), data) else {
        kept("entry");
        return false;
    };
    let Source::Cluster(cluster) = entry.source else {
        kept("not_cluster");
        return false;
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
    let request = Request { shader: entry.shader, cluster: u32::from(cluster), part: entry.part, passes };
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

/// A snapshot load: an open handler or build never returns.
pub(super) fn forget() {
    CURRENT.set(None);
    HANDLER.with(|h| h.borrow_mut().take());
    BUILD.with(|b| b.borrow_mut().take());
}

#[cfg(test)]
mod tests;
