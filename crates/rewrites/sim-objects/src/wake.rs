//! The wake leaves (docs/engine/object-update.md, "Slice 1: walks and
//! dispatchers"): `b7360` wake, `baf80` root ancestor, `bba20` wake for
//! the post walk and `bc7b0` wake children. None calls guest code, so each
//! is a plain function over guest memory.
//!
//! Each makes the stores retail makes, in retail's order and width (a byte
//! store stays a byte store), and returns the registers retail leaves in
//! `eax`, `ecx` and `edx` and how its last flag-setting instruction ended
//! ([`Exit`]): the caller may be guest code, which can read any of them
//! (the callout contract, docs/engine/sim-rewrite-objects.md section 5).
//! Every leaf here saves and restores `ebx`, `ebp`, `esi` and `edi`, or
//! never writes them.
//!
//! Each also makes the stack stores retail makes below its entry `esp`
//! (its pushes, and the return addresses and arguments of the calls
//! between the leaves), at retail's addresses and in retail's order
//! ([`Stack`]). They are dead once the leaf returns, but the caller's next
//! deeper frame overlays them: a slot that frame never writes holds what
//! the leaf left there, as it does in retail.
//!
//! The header index is `handle & 0xffff` times `0xc`, not times the
//! pool's element size: retail hard-codes the stride here.

use guest_layout::object::header;
use guest_layout::{DatumArray, Fault, Mem, MemMut, Object, ObjectHeader, Off};

const NONE: u32 = u32::MAX;

/// How deep [`wake_children`] may nest before it reports a fault: deeper
/// than the pool has slots means the child links hold a cycle, where
/// retail recurses until the guest stack runs out.
pub const MAX_DEPTH: usize = 1 << 16;

/// Where a leaf's frame is and what it pushes: `esp` at entry (the
/// address of its return address) and the caller's `ebx`, `esi` and
/// `edi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stack {
    pub esp: u32,
    pub ebx: u32,
    pub esi: u32,
    pub edi: u32,
}

/// The return addresses of the calls between the leaves.
const BBA20_AFTER_ROOT: u32 = 0x000b_ba49;
const BBA20_AFTER_WAKE: u32 = 0x000b_ba63;
const BBA20_AFTER_CHILDREN: u32 = 0x000b_ba6d;
const BC7B0_AFTER_WAKE: u32 = 0x000b_c7e6;
const BC7B0_AFTER_CHILD: u32 = 0x000b_c80c;

/// `push v`.
fn push<M: MemMut + ?Sized>(m: &mut M, esp: &mut u32, v: u32) -> Result<(), Fault> {
    *esp = esp.wrapping_sub(4);
    Off::<u32>::new(0).write(m, *esp, v)
}

/// `call` from `esp`, returning to `ret`: the callee's entry `esp`.
fn call<M: MemMut + ?Sized>(m: &mut M, esp: u32, ret: u32) -> Result<u32, Fault> {
    let mut esp = esp;
    push(m, &mut esp, ret)?;
    Ok(esp)
}

/// What a leaf leaves in the caller-visible registers it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub flags: Flags,
}

/// The last flag-setting instruction before `ret`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flags {
    /// `cmp r, -1` with `r` none: ZF and PF set, CF, SF, OF and AF clear.
    NoneCompared,
    /// `test byte ptr [header+2], 2` with AWAKE set: every status flag
    /// clear (the result `2` has odd parity); AF undefined.
    AwakeTested,
}

fn u32_at<M: Mem + ?Sized>(m: &M, addr: u32) -> Result<u32, Fault> {
    Off::<u32>::new(0).read(m, addr)
}

/// `[4e0300]`: the objects datum array's header.
fn array<M: Mem + ?Sized>(m: &M) -> Result<u32, Fault> {
    u32_at(m, guest_layout::global::OBJECTS.base)
}

/// The array's element storage (`+44`), read through `array`.
fn data<M: Mem + ?Sized>(m: &M, array: u32) -> Result<u32, Fault> {
    DatumArray::DATA.read(m, array)
}

/// `(handle & 0xffff) * 3`: what retail keeps in a register before
/// scaling by 4.
fn index3(handle: u32) -> u32 {
    (handle & 0xffff) * 3
}

/// `b7360 (object: eax)`: set AWAKE on the object and every ancestor up
/// the parent chain. Nothing when the object is already AWAKE or the
/// handle is none (the header at index `ffff` is still read first, as
/// retail reads it). The loop does not test an ancestor before storing,
/// so an ancestor that is already AWAKE is stored again. Pushes `ebx`
/// and `esi`.
pub fn wake<M: MemMut + ?Sized>(m: &mut M, object: u32, s: Stack) -> Result<Exit, Fault> {
    let mut esp = s.esp;
    push(m, &mut esp, s.ebx)?;
    push(m, &mut esp, s.esi)?;
    let array = array(m)?;
    let mut ecx = data(m, array)?;
    let edx = index3(object);
    let flags = ObjectHeader::FLAGS.read(m, ecx.wrapping_add(edx * 4))?;
    if flags & header::AWAKE != 0 {
        return Ok(Exit { eax: object, ecx, edx, flags: Flags::AwakeTested });
    }
    if object == NONE {
        return Ok(Exit { eax: object, ecx, edx, flags: Flags::NoneCompared });
    }
    let mut eax = object;
    loop {
        let off = (eax & 0xffff) * ObjectHeader::SIZE;
        let at = off.wrapping_add(ecx);
        let f = ObjectHeader::FLAGS.read(m, at)?;
        ObjectHeader::FLAGS.write(m, at, f | header::AWAKE)?;
        ecx = data(m, array)?;
        let body = ObjectHeader::BODY.read(m, off.wrapping_add(ecx))?;
        eax = Object::PARENT.read(m, body)?.raw();
        if eax == NONE {
            return Ok(Exit { eax, ecx, edx, flags: Flags::NoneCompared });
        }
    }
}

/// `baf80 (object: ecx) -> eax`: the root ancestor, following parent
/// links; none in, none out. `edx` is the caller's when the handle is
/// none (retail does not touch it), else the header storage.
pub fn root<M: Mem + ?Sized>(m: &M, object: u32, edx: u32) -> Result<Exit, Fault> {
    if object == NONE {
        return Ok(Exit { eax: NONE, ecx: NONE, edx, flags: Flags::NoneCompared });
    }
    let edx = data(m, array(m)?)?;
    let mut ecx = object;
    loop {
        let eax = ecx;
        let body = ObjectHeader::BODY.read(m, edx.wrapping_add(index3(ecx) * 4))?;
        ecx = Object::PARENT.read(m, body)?.raw();
        if ecx == NONE {
            return Ok(Exit { eax, ecx, edx, flags: Flags::NoneCompared });
        }
    }
}

/// `bba20 (object: eax)`: wake an object for the post-physics walk. A
/// CHILD is replaced by its root ancestor first. When ACTIVE: [`wake`],
/// then POST is or-ed into its header (read again after the wake). Then
/// [`wake_children`] on it, whose registers the leaf returns with.
/// Pushes `ebx`, `esi` and `edi`.
pub fn wake_for_post<M: MemMut + ?Sized>(m: &mut M, object: u32, s: Stack) -> Result<Exit, Fault> {
    let mut esp = s.esp;
    push(m, &mut esp, s.ebx)?;
    push(m, &mut esp, s.esi)?;
    let ebx = data(m, array(m)?)?;
    let mut esi = object;
    let mut edi = ebx.wrapping_add(index3(object) * 4);
    let flags = ObjectHeader::FLAGS.read(m, edi)?;
    push(m, &mut esp, s.edi)?;
    if flags & header::CHILD != 0 {
        // `baf80` with `ecx` = the object; its `edx` dies here.
        call(m, esp, BBA20_AFTER_ROOT)?;
        esi = root(m, esi, 0)?.eax;
        edi = ebx.wrapping_add(index3(esi) * 4);
    }
    if ObjectHeader::FLAGS.read(m, edi)? & header::ACTIVE != 0 {
        let at = call(m, esp, BBA20_AFTER_WAKE)?;
        wake(m, esi, Stack { esp: at, ebx, esi, edi })?;
        let f = ObjectHeader::FLAGS.read(m, edi)?;
        ObjectHeader::FLAGS.write(m, edi, f | header::POST)?;
    }
    push(m, &mut esp, esi)?;
    let at = call(m, esp, BBA20_AFTER_CHILDREN)?;
    wake_children(m, esi, Stack { esp: at, ebx, esi, edi })
}

/// The type mask `bc7b0` wakes: bipd, vehi and crea.
const WAKES_WITH_PARENT: u32 = 0x1003;

/// `bc7b0 (object: stack 0)`: for a biped, vehicle or creature, [`wake`]
/// it; then the same for every child, depth first, reading each child's
/// next sibling (from its body, through its header, again) after its
/// subtree. Retail recurses; this keeps an explicit stack of the open
/// frames, each holding the header of the child it is visiting, and
/// makes the stack stores of the recursion: frame `d` is entered at
/// `s.esp - 0xc * d` and pushes `esi` (the caller's, then the header of
/// the child being visited), the child's handle and the return address.
pub fn wake_children<M: MemMut + ?Sized>(m: &mut M, object: u32, s: Stack) -> Result<Exit, Fault> {
    // The header (retail's `esi` in the caller's frame) of the child each
    // open frame is visiting.
    let mut frames: Vec<u32> = Vec::new();
    // Frame `depth`'s entry `esp`.
    let entry = |depth: usize| s.esp.wrapping_sub(12u32.wrapping_mul(depth as u32));
    // `push ecx; lea esi, ...; call bc7b0` from the frame entered at `at`.
    let descend = |m: &mut M, at: u32, child: u32| -> Result<(), Fault> {
        let mut esp = at.wrapping_sub(4);
        push(m, &mut esp, child)?;
        call(m, esp, BC7B0_AFTER_CHILD).map(|_| ())
    };
    let mut visit = object;
    'visit: loop {
        let mut esp = entry(frames.len());
        let hdr = data(m, array(m)?)?.wrapping_add(index3(visit) * 4);
        push(m, &mut esp, frames.last().copied().unwrap_or(s.esi))?;
        let body = ObjectHeader::BODY.read(m, hdr)?;
        let kind = ObjectHeader::KIND.read(m, hdr)?;
        // `mov edx, 1; shl edx, cl`: the count is masked to five bits.
        let mut out = Exit { eax: visit, ecx: 0, edx: 1u32 << (kind & 31), flags: Flags::NoneCompared };
        if out.edx & WAKES_WITH_PARENT != 0 {
            let at = call(m, esp, BC7B0_AFTER_WAKE)?;
            out = wake(m, visit, Stack { esp: at, ebx: s.ebx, esi: body, edi: s.edi })?;
        }
        let first = Object::FIRST_CHILD.read(m, body)?.raw();
        if first != NONE {
            if frames.len() >= MAX_DEPTH {
                return Err(Fault { addr: hdr, len: 4, write: false });
            }
            let child = data(m, array(m)?)?.wrapping_add(index3(first) * 4);
            descend(&mut *m, entry(frames.len()), first)?;
            frames.push(child);
            visit = first;
            continue 'visit;
        }
        // This frame returns: `ecx` none from the first-child test.
        let mut out = Exit { ecx: NONE, flags: Flags::NoneCompared, ..out };
        while let Some(&child) = frames.last() {
            // Back in the parent frame after `call bc7b0`.
            let eax = ObjectHeader::BODY.read(m, child)?;
            let ecx = Object::NEXT_SIBLING.read(m, eax)?.raw();
            if ecx != NONE {
                let child = data(m, array(m)?)?.wrapping_add(index3(ecx) * 4);
                descend(&mut *m, entry(frames.len() - 1), ecx)?;
                *frames.last_mut().expect("an open frame") = child;
                visit = ecx;
                continue 'visit;
            }
            frames.pop();
            // The parent returns with the last child's body in `eax` and
            // that child's `edx`.
            out = Exit { eax, ecx, edx: out.edx, flags: Flags::NoneCompared };
        }
        return Ok(out);
    }
}

#[cfg(test)]
mod tests;
