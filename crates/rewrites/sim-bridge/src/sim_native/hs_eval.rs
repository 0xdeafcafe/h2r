//! Expression-entry parity for the HaloScript VM. The guest runs the call.
//! At its return compare an atom's destination slot or the frame push that
//! hands a script/builtin call to the thread dispatcher.
use super::hs_parity::{self, EVALUATOR};
use engine::{datum::layout, hs_pool, hs_syntax, points};
use hooks::Builder;
use hs::vm::{Cast, Cell, HsType, PureCast, STACK_BYTES, apply_pure_cast, classify};
use hs::{Datum, Global, Node};
use runtime::{CpuAbi, Guest};

const FRAME_HEADER: u32 = 0x10;
const STACK_OFFSET: u32 = 0x18;
const THREAD_SIZE: u32 = STACK_OFFSET + STACK_BYTES as u32;

pub(super) fn install(b: &mut Builder) {
    hs_parity::observe(b, &points::HS_EVALUATE, &EVALUATOR, 4, plan, compare);
}

#[derive(Debug)]
struct Evaluation {
    expr: Datum,
    expected: Outcome,
}

#[derive(Debug)]
enum Outcome {
    Atom {
        dest: u32,
        value: Cell,
    },
    Call {
        thread: u32,
        parent: u32,
        child: u32,
        dest: u32,
        flags: u8,
    },
}

/// Each pool can be resized at map attach. Its data pointer and capacity
/// belong to the current map, including for converted scenarios.
fn slot(g: &Guest, pool: u32, index: u32, stride: u32) -> Option<u32> {
    let header = g.u32(pool).ok()?;
    if header == 0 || index >= g.u32(header.checked_add(layout::MAXIMUM_COUNT)?).ok()? {
        return None;
    }
    let data = g.u32(header.checked_add(layout::DATA)?).ok()?;
    if data == 0 {
        return None;
    }
    data.checked_add(index.checked_mul(stride)?)
}

fn plan(cpu: &CpuAbi, g: &Guest) -> Option<Evaluation> {
    let expr = Datum(g.u32(cpu.esp.checked_add(4)?).ok()?);
    let at = slot(g, hs_syntax::NODES, expr.slot() as u32, Node::SIZE as u32)?;
    let n = Node::decode(&g.read::<{ Node::SIZE }>(at).ok()?).ok()?;
    if !n.live() || n.salt() != expr.salt() {
        return None;
    }
    let dest = cpu.ebx;
    let expected = if n.atom() {
        let (from, input) = if n.global() {
            scenario_global(g, n.data() as u16)?
        } else {
            // The map attach has already fixed up string literals to pointers.
            (HsType(n.opcode()), Cell(n.data()))
        };
        let value = cast(g, cpu, from, HsType(n.value_type()), input)?;
        Outcome::Atom { dest, value }
    } else {
        let thread = slot(g, hs_pool::THREADS, cpu.edx & 0xffff, THREAD_SIZE)?;
        let end = thread.checked_add(THREAD_SIZE)?;
        let parent = g.u32(thread + 0x10).ok()?;
        if !(thread + STACK_OFFSET..=end - FRAME_HEADER).contains(&parent) {
            return None;
        }
        let cursor = g.u16(parent + 0xc).ok()? as i16;
        let child = (parent + FRAME_HEADER).checked_add(u32::try_from(cursor).ok()?)?;
        if child > end - FRAME_HEADER {
            return None;
        }
        Outcome::Call {
            thread,
            parent,
            child,
            dest,
            flags: g.u8(thread + 3).ok()? | 1,
        }
    };
    Some(Evaluation { expr, expected })
}

fn scenario_global(g: &Guest, index: u16) -> Option<(HsType, Cell)> {
    // External cells are refreshed by the guest helper and its own arm.
    if index & 0x8000 != 0 {
        return None;
    }
    let i = u32::from(index);
    let scenario = g.u32(points::SCENARIO.base).ok()?;
    if scenario == 0 || i >= g.u32(scenario.checked_add(0x1c0)?).ok()? {
        return None;
    }
    let defs = g.u32(scenario.checked_add(0x1c4)?).ok()?;
    let ty = g
        .u16(defs.checked_add(i * Global::SIZE as u32 + 0x20)?)
        .ok()?;
    let cell = slot(g, hs_pool::GLOBALS, hs_pool::EXTERNALS + i, 8)?;
    Some((HsType(ty), Cell(g.u32(cell.checked_add(4)?).ok()?)))
}

fn cast(g: &Guest, cpu: &CpuAbi, from: HsType, to: HsType, value: Cell) -> Option<Cell> {
    if from.0 >= HsType::COUNT || to.0 >= HsType::COUNT {
        return None;
    }
    let pure = match classify(from, to) {
        Cast::Identity => return Some(value),
        Cast::Pure(p) => p,
        // Resolving an object name or allocating an object list needs the engine.
        _ => return None,
    };
    if pure == PureCast::RealFromLong && cpu.mxcsr & 0x6000 != 0 {
        return None; // The VM uses round-to-nearest for i32 -> f32.
    }
    let mut len = 0;
    if pure == PureCast::BooleanFromStringIsEmpty {
        while g.u8(value.0.checked_add(len as u32)?).ok()? != 0 {
            len += 1;
            if len > hs::MAX_TOKEN_BYTES {
                return None;
            }
        }
    }
    Some(apply_pure_cast(pure, value, || len))
}

fn compare(eval: &Evaluation, _: &CpuAbi, g: &Guest) -> Option<String> {
    let mismatch = |field: &str, want: u32, have: Option<u32>| {
        (Some(want) != have).then(|| {
            format!(
                "expression {:#010x} {field}: expected {want:#010x}, read {have:x?}",
                eval.expr.0
            )
        })
    };
    match eval.expected {
        Outcome::Atom { dest, value } => mismatch("value", value.0, g.u32(dest).ok()),
        Outcome::Call {
            thread,
            parent,
            child,
            dest,
            flags,
        } => mismatch("parent destination", dest, g.u32(parent + 8).ok())
            .or_else(|| mismatch("top frame", child, g.u32(thread + 0x10).ok()))
            .or_else(|| mismatch("child parent", parent, g.u32(child).ok()))
            .or_else(|| mismatch("child expression", eval.expr.0, g.u32(child + 4).ok()))
            .or_else(|| mismatch("child cursor", 0, g.u16(child + 0xc).ok().map(u32::from)))
            .or_else(|| {
                mismatch(
                    "thread flags",
                    u32::from(flags),
                    g.u8(thread + 3).ok().map(u32::from),
                )
            }),
    }
}

#[cfg(test)]
mod tests;
