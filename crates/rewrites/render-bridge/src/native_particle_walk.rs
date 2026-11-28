//! The particle walk `0x00175610` answered in Rust
//! (docs/graphics/native-particles.md, "Walking a system's particles"):
//! with the callback `0x00224550`, [`particle_births::births`] makes the
//! walk's births and the input block's writes. The host takes the batches
//! ([`Host::accept`]), the writes go to the guest and the walk returns
//! (`ret 0x10`) without running, so the callback never feeds the D3D
//! draw. The frame's taken batches go to the host at the scene's seal
//! ([`Host::publish`]), which publishes them as GPU particle systems
//! (`native_render::particle_set`) for the frame graph's slot 9 stage.
//!
//! A refusal runs the guest's walk and is counted by name:
//! `layer_plan_15_particle_walk_refused_<name>`. A draw whose definition
//! is known not to build ([`known_refusal`]: the mirror's check, or the
//! host's kept refusal) is refused at the entry, before the model walks.

use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::Mutex;

use engine::function_eval::constant::BYTE;
use guest_layout::memory::GuestMemory;
use engine::particle_emission as pe;
use engine::particle_emission::constant::TINT;
use engine::particle_walk::{self as pw, Call};
use guest_layout::points::{Loc, Point, Sig};
use hooks::{Builder, Flow};
use native_render::particle_mirror as nm;

use crate::particle_births::{self, Batch, Births};

/// The walk's entry: `(system, location, callback, draw)`, EDI the input
/// block, `ret 0x10`.
pub const WALK: Point = Point::new(
    "PARTICLE_WALK",
    pw::ENTRY,
    Sig::Fn {
        args: &[("system", Loc::Stack(0)), ("location", Loc::Stack(1)), ("callback", Loc::Stack(2)), ("draw", Loc::Stack(3))],
        ret: None,
        pop: POP as u16,
    },
    "docs/graphics/native-particles.md",
);
/// What the walk pops: its four arguments.
pub const POP: u32 = 0x10;

/// Takes a frame's batches and draws them as GPU particle systems (the
/// game: `particle_set::System`s from the `prt3` tag and its bitmap).
pub trait Host: Send + Sync {
    /// Whether every batch of one walk can be drawn: each key's system
    /// built (definition, sprites, blend, bitmap). `Err` names why not,
    /// and the guest's walk runs.
    fn accept(&self, m: &dyn GuestMemory, batches: &[Batch]) -> Result<(), &'static str>;
    /// The refusal kept this map for the system of `definition` and
    /// `bitmap` (`[draw]`, `[draw + 4]`), if one was refused: one lookup,
    /// nothing built.
    fn refused(&self, definition: u32, bitmap: u32) -> Option<&'static str>;
    /// The frame's taken batches, in the guest's order, at the scene's
    /// seal: published as the frame's set (none when empty).
    fn publish(&self, batches: Vec<Batch>);
}

#[cfg(not(test))]
static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
#[cfg(not(test))]
static FRAME: Mutex<Vec<Batch>> = Mutex::new(Vec::new());

/// Installs the host (boot).
#[cfg(not(test))]
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

#[cfg(not(test))]
fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(not(test))]
fn with_frame<R>(f: impl FnOnce(&mut Vec<Batch>) -> R) -> R {
    f(&mut FRAME.lock().unwrap_or_else(|e| e.into_inner()))
}

/// `layer_plan_15_particle_walk_<outcome>`.
#[cfg(not(test))]
fn count(outcome: &str, n: u64) {
    crate::native_layers::count(15, &format!("particle_walk_{outcome}"), n);
}

// Tests run in parallel: each thread keeps its own host, frame and counts.
#[cfg(test)]
thread_local! {
    static HOST: std::cell::RefCell<Option<Arc<dyn Host>>> = const { std::cell::RefCell::new(None) };
    static FRAME: std::cell::RefCell<Vec<Batch>> = const { std::cell::RefCell::new(Vec::new()) };
    static TESTED: std::cell::RefCell<BTreeMap<String, u64>> = const { std::cell::RefCell::new(BTreeMap::new()) };
}

#[cfg(test)]
pub fn set_host(host: Arc<dyn Host>) {
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

#[cfg(test)]
fn host() -> Option<Arc<dyn Host>> {
    HOST.with(|h| h.borrow().clone())
}

#[cfg(test)]
fn with_frame<R>(f: impl FnOnce(&mut Vec<Batch>) -> R) -> R {
    FRAME.with(|b| f(&mut b.borrow_mut()))
}

#[cfg(test)]
fn count(outcome: &str, n: u64) {
    TESTED.with(|t| *t.borrow_mut().entry(format!("particle_walk_{outcome}")).or_default() += n);
}

#[cfg(test)]
fn tested(outcome: &str) -> u64 {
    TESTED.with(|t| t.borrow().get(&format!("particle_walk_{outcome}")).copied().unwrap_or(0))
}

/// The walk entered with ESP `esp` (its return address) and EDI `edi`,
/// under `mxcsr` and `fcw`: `true` when it is answered here (its batches
/// taken for the frame, its writes made), so the guest's walk must not
/// run. `false` runs it: another callback, or a refusal counted by name.
pub fn answer(mem: &mut dyn GuestMemory, esp: u32, edi: u32, mxcsr: u32, fcw: u16) -> bool {
    let Ok(call) = Call::at(&*mem, esp, edi) else {
        count("refused_fault", 1);
        return false;
    };
    if call.callback != pe::ENTRY {
        count("other_callback", 1);
        return false;
    }
    if let Some(name) = known_refusal(&*mem, call.draw) {
        count(&format!("refused_{name}"), 1);
        return false;
    }
    let taken = particle_births::births(&*mem, call, mxcsr, fcw).and_then(|b| {
        host().ok_or("no_host")?.accept(&*mem, &b.batches)?;
        Ok(b)
    });
    let Births { batches, writes } = match taken {
        Ok(b) => b,
        Err(name) => {
            count(&format!("refused_{name}"), 1);
            return false;
        }
    };
    // The host has taken the batches: the walk is answered whatever the
    // writes do, so the guest never runs it over a part-written block.
    if write(mem, &writes).is_err() {
        count("write_fault", 1);
    }
    count("taken", 1);
    count("particles", batches.iter().map(|b| b.spawns.len() as u64).sum());
    with_frame(|f| f.extend(batches));
    true
}

/// Why the draw `draw`'s system cannot be taken, known before the walk:
/// its definition's flags and class fail the mirror's check
/// (`particle_mirror::check`), or the host kept a refusal for it. `None`
/// when neither, or when the draw does not read (the walk names that).
pub fn known_refusal(m: &dyn GuestMemory, draw: u32) -> Option<&'static str> {
    let r = |at: u32| m.u32(at).ok();
    let (definition, bitmap) = (r(draw)?, r(draw.wrapping_add(4))?);
    let (flags, class) = (r(definition)?, r(definition.wrapping_add(4))? as u16);
    let (byte, tint) = (f32::from_bits(r(BYTE)?), f32::from_bits(r(TINT)?));
    if let Err(e) = nm::check(flags, class, byte, tint) {
        return Some(e.name());
    }
    host()?.refused(definition, bitmap)
}

/// The block's bytes, in runs.
fn write(mem: &mut dyn GuestMemory, writes: &BTreeMap<u32, u8>) -> Result<(), guest_layout::memory::MemFault> {
    let mut run: Option<(u32, Vec<u8>)> = None;
    for (&at, &byte) in writes {
        match &mut run {
            Some((start, bytes)) if start.wrapping_add(bytes.len() as u32) == at => bytes.push(byte),
            _ => {
                if let Some((start, bytes)) = run.take() {
                    mem.write(start, &bytes)?;
                }
                run = Some((at, vec![byte]));
            }
        }
    }
    if let Some((start, bytes)) = run {
        mem.write(start, &bytes)?;
    }
    Ok(())
}

/// The scene's seal (`scene_step::seal`): the frame's batches go to the
/// host, which publishes them. With no host nothing was taken.
pub fn frame_end() {
    let batches = with_frame(std::mem::take);
    if let Some(h) = host() {
        h.publish(batches);
    }
}

/// A new scene with the last frame's batches never sealed (a frame with
/// no game view, a map change): they are dropped, counted `unpublished`.
pub fn frame_begin() {
    let left = with_frame(std::mem::take);
    count("unpublished", left.len() as u64);
}

/// The walk's hook, inside `native_particles`' build (on with the native
/// renderer, `impl.render.native`).
pub(crate) fn build(b: &mut Builder) {
    b.raw()
        .before(&WALK, "native_particle_walk", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let answered = answer(&mut *rt.memory(), c.cpu.esp, c.cpu.edi, c.cpu.mxcsr, c.cpu.x87.control);
            if !answered {
                return Ok(Flow::Continue);
            }
            c.ret(POP).map(Flow::Return)
        })
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::function_eval::constant::ONE;
    use guest_layout::memory::FlatMemory;
    use std::sync::Mutex;

    const ESP: u32 = 0x0050_0000;
    const STATE: u32 = 0x0050_1000;
    const SYSTEM: u32 = 0x0050_2000;
    const LOCATION: u32 = 0x0050_3000;
    const DRAW: u32 = 0x0050_4000;
    const TAG_TABLE: u32 = 0x0050_5000;
    const DEF: u32 = 0x0050_6000;
    const BITMAP: u32 = 0x0050_7000;

    /// A call of the walk with `callback`; `tags` false leaves the tag
    /// table unset, so the definition's read faults.
    fn world(callback: u32, tags: bool) -> FlatMemory {
        let mut m = FlatMemory::new(0x0045_0000, 0x0010_0000);
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        for (i, v) in [SYSTEM, LOCATION, callback, DRAW].into_iter().enumerate() {
            m.set_u32(ESP + 4 + 4 * i as u32, v).unwrap();
        }
        if tags {
            // Tag 0 of no group the resolver reads: definition 0.
            m.set_u32(pw::TAGS, TAG_TABLE).unwrap();
        }
        // No emitter.
        m.set_u32(LOCATION + 4, u32::MAX).unwrap();
        m
    }

    #[derive(Default)]
    struct Recorder {
        refuse: Option<&'static str>,
        /// What `refused` answers for `(DEF, BITMAP)`.
        kept: Option<&'static str>,
        accepted: Mutex<usize>,
        published: Mutex<Vec<Vec<Batch>>>,
    }

    impl Host for Recorder {
        fn accept(&self, _: &dyn GuestMemory, _: &[Batch]) -> Result<(), &'static str> {
            *self.accepted.lock().unwrap() += 1;
            self.refuse.map_or(Ok(()), Err)
        }
        fn refused(&self, definition: u32, bitmap: u32) -> Option<&'static str> {
            self.kept.filter(|_| (definition, bitmap) == (DEF, BITMAP))
        }
        fn publish(&self, batches: Vec<Batch>) {
            self.published.lock().unwrap().push(batches);
        }
    }

    #[test]
    fn a_refused_walk_runs_the_guest_and_is_counted_by_name() {
        let mut m = world(pe::ENTRY, false);
        set_host(Arc::new(Recorder::default()));
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f), "the guest walks");
        assert_eq!(tested("refused_fault"), 1);
        assert_eq!(m.u32(STATE + 0x48).unwrap(), 0, "nothing written");
        // The host's refusal is counted by its name too.
        let mut m = world(pe::ENTRY, true);
        set_host(Arc::new(Recorder { refuse: Some("template"), ..Default::default() }));
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f));
        assert_eq!(tested("refused_template"), 1);
        assert_eq!(m.u32(STATE + 0x48).unwrap(), 0, "a refused walk writes nothing");
        // Another callback is the guest's, not a refusal.
        let mut m = world(0x0022_0000, true);
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f));
        assert_eq!(tested("other_callback"), 1);
        assert_eq!(tested("taken"), 0);
    }

    /// A walk whose tag table faults, so the model's walk would refuse
    /// `fault`, drawn through a readable definition of `class`.
    fn drawn(class: u16) -> FlatMemory {
        let mut m = world(pe::ENTRY, false);
        m.set_u32(DRAW, DEF).unwrap();
        m.set_u32(DRAW + 4, BITMAP).unwrap();
        m.set_u16(DEF + 4, class).unwrap();
        m
    }

    #[test]
    fn a_definition_known_refused_is_refused_at_the_entry_without_the_model() {
        let host = Arc::new(Recorder { kept: Some("input_the_gpu_lacks_12"), ..Default::default() });
        set_host(host.clone());
        let mut m = drawn(0);
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f), "the guest walks");
        assert_eq!(tested("refused_input_the_gpu_lacks_12"), 1, "the kept name");
        assert_eq!(tested("refused_fault"), 0, "the model never walked");
        assert_eq!(*host.accepted.lock().unwrap(), 0, "nor the host built");
        assert_eq!(m.u32(STATE + 0x48).unwrap(), 0, "nothing written");
        // The mirror's check needs no host: a class other than 0.
        let mut m = drawn(2);
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f));
        assert_eq!(tested("refused_orientation"), 1);
        assert_eq!(tested("refused_fault"), 0);
        // Not known: the walk runs, and names its own refusal.
        set_host(Arc::new(Recorder::default()));
        let mut m = drawn(0);
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f));
        assert_eq!(tested("refused_fault"), 1);
    }

    #[test]
    fn no_host_is_a_refusal() {
        let mut m = world(pe::ENTRY, true);
        HOST.with(|h| *h.borrow_mut() = None);
        assert!(!answer(&mut m, ESP, STATE, 0x1f80, 0x027f));
        assert_eq!(tested("refused_no_host"), 1);
    }

    #[test]
    fn a_taken_walk_writes_the_block_and_its_frame_goes_to_the_host_at_the_seal() {
        let host = Arc::new(Recorder::default());
        set_host(host.clone());
        let mut m = world(pe::ENTRY, true);
        assert!(answer(&mut m, ESP, STATE, 0x1f80, 0x027f), "answered");
        // A new system and location take the block's records.
        assert_eq!(m.u32(STATE + 0x48).unwrap(), SYSTEM);
        assert_eq!(m.u32(STATE + 0x4c).unwrap(), LOCATION);
        assert_eq!((tested("taken"), tested("particles")), (1, 0));
        frame_end();
        assert_eq!(host.published.lock().unwrap().as_slice(), &[Vec::<Batch>::new()]);
        frame_begin();
        assert_eq!(tested("unpublished"), 0);
    }

    #[test]
    fn a_frame_never_sealed_is_dropped_and_counted() {
        set_host(Arc::new(Recorder::default()));
        with_frame(|f| {
            f.push(Batch {
                key: native_render::particle_mirror::Key { definition: 1, bitmap: 2, tint: [0; 4], size: 0 },
                spawns: vec![],
            })
        });
        frame_begin();
        assert_eq!(tested("unpublished"), 1);
        assert!(with_frame(|f| f.is_empty()));
    }

    #[test]
    fn writes_go_in_runs() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let w: BTreeMap<u32, u8> = [(0x1000, 1), (0x1001, 2), (0x1010, 3)].into_iter().collect();
        write(&mut m, &w).unwrap();
        assert_eq!(m.bytes(0x1000, 2).unwrap(), [1, 2]);
        assert_eq!(m.u8(0x1010).unwrap(), 3);
        let far: BTreeMap<u32, u8> = [(0x9000, 1)].into_iter().collect();
        assert!(write(&mut m, &far).is_err());
    }
}
