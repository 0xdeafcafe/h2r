//! Entry I, `0x00301800`: the general MOPP ray's parity arm
//! (docs/engine/collision-queries.md, "Entry I").
//!
//! The guest's query runs and stays the answer. Four observers feed one
//! [`RayCheck`] per query: [`begin`] before the setup, [`enter`] at each
//! call of the float interpreter `0x003009e0`, [`key`] at the instanced
//! collection's child method and [`end`] after the setup returns. The
//! outcome goes to the world's `ray` counters with the first mismatch.
//! Nothing here writes guest memory.
//!
//! A query runs inside its caller's call, and a child shape can run a
//! query of its own inside a terminal, so the open queries are a stack per
//! thread. Each observation belongs to the innermost one, matched by its
//! VM (the interpreter's `ecx`) or its collection (the child method's).
use super::Read;
use super::layout::{self, mopp_ray as l};
use super::preflight::CollisionMemory;
use runtime::CpuAbi;
use sim_collision::mopp::Tree;
use sim_collision::mopp::ray::{
    Constants, Event, Frame, Outcome, Precision, Ray, RayCheck, RayWalk, Segment, Unverified, Vec4,
};
use sim_collision::{Bail, CollisionWorld, GuestRead};
use sim_core::det;
use std::cell::RefCell;
use std::sync::Arc;

/// Queries open at once on one thread. A deeper stack means observers
/// were skipped (a query that never returned through its own); it is
/// dropped and those queries go uncounted.
const MAX_SESSIONS: usize = 16;

/// x87 registers the interpreter's sequences need free.
const X87_FREE: u32 = 3;

/// A decoded program's bytecode, shared with the world's cache.
#[derive(Clone)]
struct Program(Arc<Tree>);

impl AsRef<[u8]> for Program {
    fn as_ref(&self) -> &[u8] {
        &self.0.source().bytecode
    }
}

struct Session {
    vm: u32,
    collection: u32,
    code: u32,
    keys: bool,
    inverse_scale: u32,
    entered: bool,
    check: RayCheck<Program>,
}

enum Slot {
    /// The query could not be followed from its setup.
    Skip(Bail),
    Check(Box<Session>),
}

thread_local! {
    static SESSIONS: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
}

fn vec4(m: &impl GuestRead, at: u32) -> Result<Vec4, Bail> {
    let b = m.bytes(at, 16)?;
    Ok(std::array::from_fn(|k| {
        u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap())
    }))
}

fn words(m: &impl GuestRead, at: u32) -> Result<[u32; 8], Bail> {
    let b = m.bytes(at, 32)?;
    Ok(std::array::from_fn(|k| {
        u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap())
    }))
}

/// The setup's inputs, read at its entry: `ecx` the VM; stack
/// `collection, code, ray_input, parent_body, collector`.
fn open(c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Session, Bail> {
    if !det::fast_env(c.mxcsr) || !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, X87_FREE) {
        return Err(Bail::Env);
    }
    let precision = Precision::from_control(c.x87.control)?;
    let (sp, vm) = (c.esp, c.ecx);
    let (collection, code, input, collector) = {
        let m = Read(&*g, (0, 0));
        let arg = |k: u32| m.u32(sp.wrapping_add(4 * k));
        (arg(1)?, arg(2)?, arg(3)?, arg(5)?)
    };
    let revision = g.mopp_revision(code)?;
    let m = Read(&*g, (0, 0));
    let tree = world.mopp(&m, code, revision)?;
    let source = tree.source();
    let origin = [
        source.origin[0],
        source.origin[1],
        source.origin[2],
        source.scale,
    ];
    let constants = Constants {
        zero: m.u32(layout::ZERO)?,
        one: m.u32(layout::ONE)?,
        unit: vec4(&m, l::UNIT)?,
        root_scale: m.u32(l::ROOT_SCALE)?,
    };
    let ray = Ray {
        origin,
        from: vec4(&m, input)?,
        to: vec4(&m, input.wrapping_add(l::INPUT_TO))?,
        precision,
        constants,
    };
    let filter = m.u32(input.wrapping_add(l::INPUT_FILTER))?;
    let fraction = m.u32(collector.wrapping_add(l::COLLECTOR_FRACTION))?;
    let child = m.u32(m.u32(collection)?.wrapping_add(l::COLLECTION_CHILD))?;
    // Every terminal reaches the collection when there is no filter; the
    // instanced collection's method is where the key can be seen.
    let keys = filter == 0 && child == l::INSTANCE_CHILD;
    let root = ray.root()?;
    let walk = RayWalk::new(Program(tree), ray, &root);
    Ok(Session {
        vm,
        collection,
        code,
        keys,
        inverse_scale: root.inverse_scale,
        entered: false,
        check: RayCheck::new(walk, keys, fraction),
    })
}

/// Before `0x00301800`: open a query.
pub fn begin(c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) {
    let slot = match open(c, g, world) {
        Ok(session) => Slot::Check(Box::new(session)),
        Err(why) => Slot::Skip(why),
    };
    SESSIONS.with_borrow_mut(|open| {
        if open.len() >= MAX_SESSIONS {
            open.clear();
        }
        open.push(slot);
    });
}

/// The innermost query, while it is still being compared.
fn with_open(f: impl FnOnce(&mut Session)) {
    SESSIONS.with_borrow_mut(|open| {
        if let Some(Slot::Check(session)) = open.last_mut()
            && !session.check.decided()
        {
            f(session);
        }
    });
}

/// Before `0x003009e0`: `ecx` the VM; stack `frame, pc, segment`.
pub fn enter(c: &CpuAbi, g: &impl CollisionMemory) {
    with_open(|s| {
        if s.vm != c.ecx {
            return;
        }
        let m = Read(g, (0, 0));
        let read = || -> Result<(Event, u32, u32), Bail> {
            let arg = |k: u32| m.u32(c.esp.wrapping_add(4 * k));
            let frame = Frame::from_words(words(&m, arg(1)?)?);
            let pc = arg(2)?.wrapping_sub(s.code).wrapping_sub(l::BYTECODE);
            let segment = Segment::from_words(words(&m, arg(3)?)?);
            let fraction = m.u32(s.vm.wrapping_add(l::FRACTION))?;
            let inverse = m.u32(s.vm.wrapping_add(l::INVERSE_SCALE))?;
            Ok((Event::Enter { pc, frame, segment }, fraction, inverse))
        };
        match read() {
            Ok((event, fraction, inverse)) => {
                if !s.entered {
                    s.entered = true;
                    s.check.expect("inverse scale", s.inverse_scale, inverse);
                }
                s.check.observe(fraction, event);
            }
            Err(why) => s.check.decline(why),
        }
    });
}

/// Before the instanced collection's child method: `ecx` the collection;
/// stack `key, buffer`.
pub fn key(c: &CpuAbi, g: &impl CollisionMemory) {
    with_open(|s| {
        if !s.keys || s.collection != c.ecx {
            return;
        }
        let m = Read(g, (0, 0));
        let key = m.u32(c.esp.wrapping_add(4));
        let fraction = m.u32(s.vm.wrapping_add(l::FRACTION));
        match (key, fraction) {
            (Ok(key), Ok(fraction)) => s.check.observe(fraction, Event::Key(key)),
            (Err(f), _) | (_, Err(f)) => s.check.decline(f.into()),
        }
    });
}

/// After `0x00301800` returns: close the innermost query and count it.
pub fn end(g: &impl CollisionMemory, world: &CollisionWorld) {
    let Some(slot) = SESSIONS.with_borrow_mut(Vec::pop) else {
        return;
    };
    let outcome = match slot {
        Slot::Skip(why) => Outcome::Unverified(Unverified::Bail(why)),
        Slot::Check(s) => match Read(g, (0, 0)).u32(s.vm.wrapping_add(l::FRACTION)) {
            Ok(fraction) => s.check.finish(fraction),
            Err(f) => Outcome::Unverified(Unverified::Bail(f.into())),
        },
    };
    world.record_ray(&outcome);
}

/// Queries open on this thread.
pub fn open_queries() -> usize {
    SESSIONS.with_borrow(Vec::len)
}
