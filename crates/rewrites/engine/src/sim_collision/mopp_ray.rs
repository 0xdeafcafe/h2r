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
mod collector;
pub use collector::CollectorCast;
use collector::Setup;

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
use std::sync::Arc;
use std::{cell::RefCell, collections::HashMap};

/// Queries retained per guest thread. Deeper calls count as depth declines.
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
    setup: Setup,
    entered: bool,
    check: RayCheck<Program>,
}

enum Slot {
    /// The query could not be followed from its setup.
    Skip(Bail),
    Check(Box<Session>),
}

#[derive(Default)]
struct Stack {
    slots: Vec<Slot>,
    overflow: usize,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<u32, Stack>> = RefCell::default();
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
fn prepare(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<(Setup, RayWalk<Program>), Bail> {
    if g.trapped() {
        return Err(Bail::Trap);
    }
    if !det::fast_env(c.mxcsr) || !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, X87_FREE) {
        return Err(Bail::Env);
    }
    let precision = Precision::from_control(c.x87.control)?;
    let (sp, vm) = (c.esp, c.ecx);
    let (collection, code, input, parent, collector) = {
        let m = Read(&*g, (0, 0));
        let arg = |k: u32| m.u32(sp.wrapping_add(4 * k));
        (arg(1)?, arg(2)?, arg(3)?, arg(4)?, arg(5)?)
    };
    // The setup copies the code origin and both input vectors with MOVAPS.
    if (code | input | vm) & 15 != 0 {
        return Err(Bail::Fault);
    }
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
    let root = ray.root()?;
    // The setup reads arguments and ray data between VM and stack writes.
    // Decline if forwarding through an overlap would change their values.
    let aligned = sp.checked_sub(4).ok_or(Bail::Fault)? & !15;
    let call_sp = aligned.checked_sub(0x60).ok_or(Bail::Fault)?;
    let outputs = super::preflight::Outputs::new(g, sp, sp - call_sp, [(vm, 0x68)])?;
    for (at, len) in [
        tree.span(),
        (input, 0x28),
        (collector.wrapping_add(l::COLLECTOR_FRACTION), 4),
        (sp.wrapping_add(4), 20),
        (layout::ONE, 4),
        (l::ROOT_SCALE, 4),
    ] {
        outputs.separate(g, at, len)?;
    }
    let setup = Setup::read(
        &m,
        vm,
        code,
        input,
        parent,
        collection,
        collector,
        root.inverse_scale,
        revision,
        constants,
    )?;
    Ok((setup, RayWalk::new(Program(tree), ray, &root)))
}

fn open(c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Session, Bail> {
    let (setup, walk) = prepare(c, g, world)?;
    let m = Read(&*g, (0, 0));
    let vm = c.ecx;
    let code = setup.code;
    let collection = setup.collection;
    let fraction = setup.fraction;
    let child = m.u32(m.u32(collection)?.wrapping_add(l::COLLECTION_CHILD))?;
    let keys = setup.filter() == 0 && child == l::INSTANCE_CHILD;
    Ok(Session {
        vm,
        collection,
        code,
        keys,
        setup,
        entered: false,
        check: RayCheck::new(walk, keys, fraction),
    })
}

/// Before `0x00301800`: open a query.
pub fn begin(thread: u32, c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) {
    SESSIONS.with_borrow_mut(|sessions| {
        let stack = sessions.entry(thread).or_default();
        if stack.slots.len() == MAX_SESSIONS {
            stack.overflow += 1;
        } else {
            stack.slots.push(match open(c, g, world) {
                Ok(session) => Slot::Check(Box::new(session)),
                Err(why) => Slot::Skip(why),
            });
        }
    });
}

/// The innermost query, while it is still being compared.
fn with_open(thread: u32, f: impl FnOnce(&mut Session)) {
    SESSIONS.with_borrow_mut(|open| {
        if let Some(stack) = open.get_mut(&thread)
            && stack.overflow == 0
            && let Some(Slot::Check(session)) = stack.slots.last_mut()
            && !session.check.decided()
        {
            f(session);
        }
    });
}

/// Before `0x003009e0`: `ecx` the VM; stack `frame, pc, segment`.
pub fn enter(thread: u32, c: &CpuAbi, g: &impl CollisionMemory) {
    with_open(thread, |s| {
        if g.trapped() {
            s.check.decline(Bail::Trap);
            return;
        }
        if s.vm != c.ecx {
            return;
        }
        let m = Read(g, (0, 0));
        let read = || -> Result<(Event, u32), Bail> {
            let arg = |k: u32| m.u32(c.esp.wrapping_add(4 * k));
            let frame = Frame::from_words(words(&m, arg(1)?)?);
            let pc = arg(2)?.wrapping_sub(s.code).wrapping_sub(l::BYTECODE);
            let segment = Segment::from_words(words(&m, arg(3)?)?);
            let fraction = m.u32(s.vm.wrapping_add(l::FRACTION))?;
            Ok((Event::Enter { pc, frame, segment }, fraction))
        };
        match read() {
            Ok((event, fraction)) => {
                if !s.entered {
                    s.entered = true;
                    match s.setup.first_difference(&m) {
                        Ok(Some((offset, expected, actual))) => {
                            s.check.expect_byte(offset, expected, actual);
                        }
                        Err(why) => s.check.decline(why),
                        Ok(None) => {}
                    }
                }
                s.check.observe(fraction, event);
            }
            Err(why) => s.check.decline(why),
        }
    });
}

/// Before the instanced collection's child method: `ecx` the collection;
/// stack `key, buffer`.
pub fn key(thread: u32, c: &CpuAbi, g: &impl CollisionMemory) {
    with_open(thread, |s| {
        if g.trapped() {
            s.check.decline(Bail::Trap);
            return;
        }
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
pub fn end(thread: u32, g: &impl CollisionMemory, world: &CollisionWorld) {
    let Some(slot) = SESSIONS.with_borrow_mut(|sessions| {
        let stack = sessions.get_mut(&thread)?;
        if stack.overflow != 0 {
            stack.overflow -= 1;
            return Some(Slot::Skip(Bail::Depth));
        }
        let slot = stack.slots.pop();
        if stack.slots.is_empty() {
            sessions.remove(&thread);
        }
        slot
    }) else {
        return;
    };
    let outcome = match slot {
        Slot::Skip(why) => Outcome::Unverified(Unverified::Bail(why)),
        Slot::Check(_) if g.trapped() => Outcome::Unverified(Unverified::Bail(Bail::Trap)),
        Slot::Check(s) => match Read(g, (0, 0)).u32(s.vm.wrapping_add(l::FRACTION)) {
            Ok(fraction) => s.check.finish(fraction),
            Err(f) => Outcome::Unverified(Unverified::Bail(f.into())),
        },
    };
    world.record_ray(&outcome);
}

/// Queries open on this thread.
pub fn open_queries() -> usize {
    SESSIONS.with_borrow(|sessions| sessions.values().map(|s| s.slots.len() + s.overflow).sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::Guest;
    use sim_core::Epoch;

    #[test]
    fn guest_threads_finish_in_their_own_order() {
        static EPOCH: Epoch = Epoch::new();
        let (a, b) = (CollisionWorld::new(&EPOCH), CollisionWorld::new(&EPOCH));
        let mut g = Guest::new();
        let cpu = CpuAbi {
            mxcsr: 0x1fa0,
            ..Default::default()
        };
        let mut bad_env = cpu;
        bad_env.mxcsr = 0;
        begin(10, &bad_env, &mut g, &a);
        let mut bad_stack = cpu;
        bad_stack.x87.control = 0x027f;
        bad_stack.x87.tag = 0xffff;
        begin(20, &bad_stack, &mut g, &b);
        end(10, &g, &a);
        end(20, &g, &b);
        assert_eq!(a.stats().ray.unverified[Bail::Env as usize], 1);
        assert_eq!(b.stats().ray.unverified[Bail::Fault as usize], 1);
        assert_eq!(open_queries(), 0);
    }
}
