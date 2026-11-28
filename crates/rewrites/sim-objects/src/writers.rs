//! Writers on the object table, ported from the lifted functions they
//! replace (docs/engine/sim-rewrite-objects.md, section 4): each takes the
//! table (the truth) and the guest memory the object memory pool lives
//! in. A caller imports the arena first and scatters after
//! ([`ObjectTable::import`], [`ObjectTable::scatter`]), so the arena is
//! authoritative between calls.
//!
//! | Function | Writer |
//! |---|---|
//! | `b9890` | [`detach_from_parent`] |
//! | `bc300` | [`free_object`] |
//!
//! The object memory pool's header is at `[4de2ec]` ([`HEAP`]): free bytes
//! at `+30`, the tail block at `+34` and the head block at `+38`. A block
//! has a `0x10` byte header before its body: the size at `+0`, the
//! previous block at `+8` and the next at `+c`.

use guest_layout::{Fault, Mem, MemMut, Off};
use sim_world::Handle;

use crate::table::ObjectTable;

/// The global holding the object memory pool's header address.
pub const HEAP: u32 = 0x004d_e2ec;
/// Bytes of a pool block's header, before the body.
pub const BLOCK_HEADER: u32 = 0x10;
/// The "none" handle.
const NONE: u32 = u32::MAX;

/// Body offsets `b9890` and `be650` use.
mod body {
    /// The next sibling's handle.
    pub const SIBLING: u32 = 0x0c;
    /// The first child's handle.
    pub const CHILD: u32 = 0x10;
    /// The parent's handle.
    pub const PARENT: u32 = 0x14;
    /// The attachment byte cleared with the parent.
    pub const PARENT_BYTE: u32 = 0x18;
}

/// Why a writer could not run.
#[derive(Debug, PartialEq, Eq)]
pub enum WriterError {
    Fault(Fault),
    /// A handle the writer was given, or follows, names no live object.
    Stale(Handle),
}

impl From<Fault> for WriterError {
    fn from(f: Fault) -> Self {
        WriterError::Fault(f)
    }
}

fn u32_at(body: &[u8], off: u32) -> u32 {
    let at = off as usize;
    u32::from_le_bytes(body[at..at + 4].try_into().expect("four bytes"))
}

fn put_u32(body: &mut [u8], off: u32, v: u32) {
    body[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
}

/// The live object in `h`'s slot: retail indexes by the low word and never
/// checks the salt.
fn at(table: &ObjectTable, h: u32) -> Result<Handle, WriterError> {
    let raw = Handle::from_raw(h);
    table.handle_at(raw.index()).ok_or(WriterError::Stale(raw))
}

/// `b9890 (eax = object)`: detach `object` from its parent. The parent's
/// child list (`+10`, linked through `+c`) is walked from its head and
/// `object` is cut out (`be650`: its `+c` is set to none); the header
/// flags lose bit 7; `object`'s parent (`+14`) is none and byte `+18`
/// is `0xff`.
pub fn detach_from_parent(table: &mut ObjectTable, object: Handle) -> Result<(), WriterError> {
    let me = at(table, object.raw())?;
    let parent = u32_at(&table.get(me).expect("live").body, body::PARENT);
    let parent = at(table, parent)?;
    // be650: `edx` addresses the link to test, starting at the parent's
    // child head; a none link ends the walk.
    let mut link: (Handle, u32) = (parent, body::CHILD);
    loop {
        let next = u32_at(&table.get(link.0).expect("live").body, link.1);
        if next == NONE {
            break;
        }
        let cur = at(table, next)?;
        if next == object.raw() {
            let after = u32_at(&table.get(cur).expect("live").body, body::SIBLING);
            put_u32(&mut table.get_mut(link.0).expect("live").body, link.1, after);
            put_u32(&mut table.get_mut(cur).expect("live").body, body::SIBLING, NONE);
            break;
        }
        link = (cur, body::SIBLING);
    }
    let r = table.get_mut(me).expect("live");
    r.flags &= 0x7f;
    put_u32(&mut r.body, body::PARENT, NONE);
    r.body[body::PARENT_BYTE as usize] = 0xff;
    Ok(())
}

/// `bc300 (object: stack 0)`: free `object`'s body from the object memory
/// pool and delete its datum. The header's flags byte is cleared first; a
/// body (non-zero `+8`) is cut out of the pool's block list and its size
/// returned to the pool's free count; then `datum_delete`
/// (`ObjectTable::delete`).
pub fn free_object<M: MemMut + ?Sized>(
    m: &mut M,
    table: &mut ObjectTable,
    object: Handle,
) -> Result<(), WriterError> {
    let me = at(table, object.raw())?;
    let record = table.get_mut(me).expect("live");
    record.flags = 0;
    let body_addr = std::mem::take(&mut record.body_addr);
    if body_addr != 0 {
        let heap = Off::<u32>::new(0).read(&*m, HEAP)?;
        let word = |m: &M, at: u32| Off::<u32>::new(0).read(m, at);
        let size = word(m, body_addr.wrapping_sub(0x10))?;
        let free = word(m, heap.wrapping_add(0x30))?.wrapping_add(size);
        m.write_bytes(heap.wrapping_add(0x30), &free.to_le_bytes())?;
        let next = word(m, body_addr.wrapping_sub(4))?;
        let prev = word(m, body_addr.wrapping_sub(8))?;
        let (next_prev_at, tail_at) = (next.wrapping_add(8), heap.wrapping_add(0x34));
        m.write_bytes(if next != 0 { next_prev_at } else { tail_at }, &prev.to_le_bytes())?;
        let (prev_next_at, head_at) = (prev.wrapping_add(0x0c), heap.wrapping_add(0x38));
        m.write_bytes(if prev != 0 { prev_next_at } else { head_at }, &next.to_le_bytes())?;
    }
    table.delete(me).map_err(|e| WriterError::Stale(e.0))?;
    Ok(())
}

/// A whole `b9890` over the guest's arena: import, detach, scatter.
pub fn detach_in_guest<M: MemMut + ?Sized>(m: &mut M, object: Handle) -> Result<(), WriterError> {
    let mut table = import_live(&*m)?;
    detach_from_parent(&mut table, object)?;
    table.scatter(m)?;
    Ok(())
}

/// A whole `bc300` over the guest's arena: import, free, scatter.
pub fn free_in_guest<M: MemMut + ?Sized>(m: &mut M, object: Handle) -> Result<(), WriterError> {
    let mut table = import_live(&*m)?;
    free_object(m, &mut table, object)?;
    table.scatter(m)?;
    Ok(())
}

fn import_live<M: Mem + ?Sized>(m: &M) -> Result<ObjectTable, WriterError> {
    match ObjectTable::import(m) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => panic!("objects: a writer ran with no objects array"),
        Err(crate::table::ImportError::Fault(f)) => Err(WriterError::Fault(f)),
        Err(e) => panic!("objects: the guest's objects array is corrupt: {e:?}"),
    }
}

#[cfg(test)]
mod tests;
