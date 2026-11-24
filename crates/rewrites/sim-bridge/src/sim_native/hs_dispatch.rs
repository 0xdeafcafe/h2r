//! Fixed-signature builtin dispatch. The evaluator reserves its argument block
//! on every entry and starts at most one expression. Keep the parent's frame
//! address across the guest call: evaluating a call argument pushes a child.

use super::hs_parity::{DISPATCH, observe};
use engine::{
    DatumHandle, GuestMemory,
    datum::{DatumArray, layout},
    hs_pool::THREADS,
    hs_syntax::NODES,
    points,
};
use hs::{
    Datum, Node,
    vm::{ArgumentStep, argument_step},
};
use runtime::CpuAbi;

const THREAD_BYTES: u32 = 0x418;
const FIRST_FRAME: u32 = 0x18;
const FRAME_LOCALS: u32 = 0xe;

fn record(mem: &dyn GuestMemory, header: u32, bytes: u32, handle: Datum) -> Option<u32> {
    let array = DatumArray::at(mem.u32(header).ok()?);
    if array.header_field(mem, layout::ELEMENT_SIZE).ok()? != bytes as i32 {
        return None;
    }
    array.try_get(mem, DatumHandle(handle.0)).ok()?
}

fn node(mem: &dyn GuestMemory, datum: Datum) -> Option<Node> {
    let address = record(mem, NODES, Node::SIZE as u32, datum)?;
    let mut bytes = [0; Node::SIZE];
    mem.read(address, &mut bytes).ok()?;
    Node::decode(&bytes).ok()
}

#[derive(Debug)]
struct Dispatch {
    thread: u32,
    parent: u32,
    cursor: u16,
    index_address: u32,
    index: u16,
    next: Datum,
    result: u32,
    child: Option<Child>,
}

#[derive(Debug)]
struct Child {
    frame: u32,
    expression: Datum,
    destination: u32,
    flags: u8,
}

fn plan(cpu: &CpuAbi, mem: &dyn GuestMemory) -> Option<Dispatch> {
    let arg = |offset| mem.u32(cpu.esp.checked_add(offset)?).ok();
    let thread = record(mem, THREADS, THREAD_BYTES, Datum(arg(4)?))?;
    let count = arg(8)?;
    let parameters = arg(12)?;
    let first = arg(16)? as u8 != 0;
    let end = thread.checked_add(THREAD_BYTES)?;
    let parent = mem.u32(thread + 0x10).ok()?;
    if !(thread + FIRST_FRAME..=end.checked_sub(FRAME_LOCALS)?).contains(&parent) {
        return None;
    }

    // Evaluators can reserve other locals before entering this helper.
    let used = u32::from(mem.u16(parent + 0xc).ok()?);
    let args = parent.checked_add(FRAME_LOCALS + used)?;
    let index_address = args.checked_add(count.checked_mul(4)?)?;
    if index_address.checked_add(6)? > end {
        return None;
    }
    let cursor = u16::try_from(used + count * 4 + 6).ok()?;
    let (index, expression) = if first {
        let call = node(mem, Datum(mem.u32(parent + 4).ok()?))?;
        let head = node(mem, Datum(call.data()))?;
        (0, head.next())
    } else {
        (
            mem.u16(index_address).ok()?,
            Datum(mem.u32(index_address + 2).ok()?),
        )
    };
    if u32::from(index) > count {
        return None;
    }
    let signature: Vec<_> = (0..count)
        .map(|i| mem.u16(parameters.checked_add(i * 2)?).ok())
        .collect::<Option<_>>()?;
    let mut result = Dispatch {
        thread,
        parent,
        cursor,
        index_address,
        index,
        next: expression,
        result: args,
        child: None,
    };

    match argument_step(&signature, index.into(), expression, |d| {
        node(mem, d).ok_or(())
    })
    .ok()?
    {
        ArgumentStep::Ready => {}
        // The offline VM faults on a missing or mistyped argument. These
        // malformed calls are outside the parity arm's domain.
        ArgumentStep::Invalid => return None,
        ArgumentStep::Evaluate { node } => {
            result.index += 1;
            result.next = node.next();
            result.result = 0;
            if !node.atom() {
                let frame = index_address.checked_add(8)?;
                if frame.checked_add(FRAME_LOCALS)? > end {
                    return None;
                }
                result.child = Some(Child {
                    frame,
                    expression,
                    destination: args + u32::from(index) * 4,
                    flags: mem.u8(thread + 3).ok()? | 1,
                });
            }
        }
    }
    Some(result)
}

fn compare(expected: &Dispatch, cpu: &CpuAbi, mem: &dyn GuestMemory) -> Option<String> {
    let current = expected.child.as_ref().map_or(expected.parent, |c| c.frame);
    let value = |name, actual, expected| {
        (actual != Some(expected))
            .then(|| format!("{name}: expected {expected:#x}, got {actual:?}"))
    };
    let u16_at = |at| mem.u16(at).ok().map(u32::from);
    let u32_at = |at| mem.u32(at).ok();
    for (name, actual, want) in [
        ("argument block", Some(cpu.eax), expected.result),
        (
            "frame cursor",
            u16_at(expected.parent + 0xc),
            expected.cursor.into(),
        ),
        (
            "argument index",
            u16_at(expected.index_address),
            expected.index.into(),
        ),
        (
            "next expression",
            u32_at(expected.index_address + 2),
            expected.next.0,
        ),
        ("current frame", u32_at(expected.thread + 0x10), current),
    ] {
        if let Some(difference) = value(name, actual, want) {
            return Some(difference);
        }
    }
    if let Some(child) = &expected.child {
        for (name, actual, want) in [
            (
                "destination",
                u32_at(expected.parent + 8),
                child.destination,
            ),
            ("child parent", u32_at(child.frame), expected.parent),
            (
                "child expression",
                u32_at(child.frame + 4),
                child.expression.0,
            ),
            ("child cursor", u16_at(child.frame + 0xc), 0),
            (
                "thread flags",
                mem.u8(expected.thread + 3).ok().map(u32::from),
                child.flags.into(),
            ),
        ] {
            if let Some(difference) = value(name, actual, want) {
                return Some(difference);
            }
        }
    }
    None
}

pub(super) fn install(b: &mut hooks::Builder) {
    observe(
        b,
        &points::HS_EVALUATE_ARGUMENTS,
        &DISPATCH,
        16,
        |cpu, guest| plan(cpu, guest),
        |p, cpu, guest| compare(p, cpu, guest),
    );
}

#[cfg(test)]
mod tests;
