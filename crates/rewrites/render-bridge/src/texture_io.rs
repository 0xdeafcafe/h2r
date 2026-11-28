//! The texture cache's streaming reads in Rust: the guest's async I/O
//! pool kept in its own memory (`0x004e0368`, 150 nodes of 64 bytes, a
//! free list `[0x004e28e8]` and a queue `[0x004e28ec]`), without the
//! kernel mutex and semaphore that guard it from the guest's worker
//! thread (`0x00120c30`). The kernel's file model completes every
//! transfer before the call returns (crates/runtime/kernel/README.md,
//! "VFS boundaries"), so a read is queued here and [`pump`] completes it
//! through a [`FileSource`] (`kernel::Kernel::read_at`); until then the
//! entry reads as not loaded and the texture cache answers "not resident
//! yet" (docs/graphics/retire-d3d-fog-fans.md, "Interfaces").
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x001208b0`: a node off the free list | [`queue`] |
//! | `0x001a0f10` / `0x00213760`: the node filled (priority, file, dest, size, file offset, chunk callback `0x001a0e70`, flag byte) and `[0x004e28f0]` raised | [`queue`] |
//! | `0x00120900`: the node put in the queue, highest priority first | [`queue`], [`raise`] |
//! | `0x001209c0`: the node's id (`serial << 8 \| index`), serial raised | [`queue`] |
//! | `0x00120ce0(esi id, priority)`: the read's priority raised | [`raise`] |
//! | `0x00120c30`: the worker's head read (`0x001a0e70`), the loaded flag set, the node back on the free list (`0x00120a30`) | [`pump`] |
//!
//! Equal priorities keep their order of arrival here; the guest's insert
//! walks them by seek distance (`0x001a1720`), which orders reads and
//! changes no result.

use guest_layout::memory::GuestMemory;

/// Why the pool cannot go on.
pub type Cause = &'static str;

const UNREAD: Cause = "texture_io: guest memory does not read";
const UNWRITABLE: Cause = "texture_io: guest memory does not write";
const BROKEN_QUEUE: Cause = "texture_io: the queue's links do not lead to the node";

/// The pool's nodes, their count and size.
pub const NODES: u32 = 0x004e_0368;
pub const NODE_COUNT: u32 = 150;
pub const NODE_SIZE: u32 = 0x40;
/// The id serial: ids are `serial << 8 | node index`.
pub const SERIAL: u32 = 0x004e_0364;
/// The free list's head and the queue's head.
pub const FREE: u32 = 0x004e_28e8;
pub const QUEUE: u32 = 0x004e_28ec;
/// The count of reads issued (the texture cache's I/O load).
pub const LOADS: u32 = 0x004e_28f0;
/// The chunk read `0x001a0e70`: a node's callback at `+0x34`.
pub const CHUNK_READ: u32 = 0x001a_0e70;

// A node: `+0` priority, `+4` id (-1 free), `+8` the read (`+8` file
// handle, `+0xc` dest, `+0x10` size, `+0x14` file offset, `+0x18` done
// pointer, `+0x1c` bytes read, `+0x20` flags), `+0x30` 3, `+0x34` the
// callback, `+0x38` the loaded flag's address, `+0x3c` the next node.
const PRIORITY: u32 = 0;
const ID: u32 = 4;
const HANDLE: u32 = 8;
const DEST: u32 = 0xc;
const SIZE: u32 = 0x10;
const OFFSET: u32 = 0x14;
const DONE_AT: u32 = 0x18;
const READ: u32 = 0x1c;
const FLAGS: u32 = 0x20;
const KIND: u32 = 0x30;
const CALLBACK: u32 = 0x34;
const LOADED_AT: u32 = 0x38;
const NEXT: u32 = 0x3c;

/// One read of the texture cache: `0x0012d5a0` passes these to
/// `0x00213760` (file by the offset word's top two bits, the offset its
/// low 30 bits, the size rounded to 512, the cache slot as the
/// destination, the entry's `+3` as the loaded flag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// The guest's file handle (`[0x00558c98]`, `[0x00557c90]`, ...).
    pub handle: u32,
    pub offset: u32,
    pub size: u32,
    pub dest: u32,
    /// The byte set to 1 when the read ends (the entry's `+3`).
    pub loaded_at: u32,
    pub priority: u32,
}

/// Positioned reads on the guest's file handles. The kernel's is
/// `kernel::Kernel::read_at`; any closure over it serves.
pub trait FileSource {
    /// Fill `out` from `offset` of the file `handle` names; the count read
    /// (fewer than `out.len()` at the end of the file).
    fn read(&mut self, handle: u32, offset: u32, out: &mut [u8]) -> Result<usize, Cause>;
}

impl<F: FnMut(u32, u32, &mut [u8]) -> Result<usize, Cause>> FileSource for F {
    fn read(&mut self, handle: u32, offset: u32, out: &mut [u8]) -> Result<usize, Cause> {
        self(handle, offset, out)
    }
}

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(|_| UNREAD)
}

fn set(mem: &mut dyn GuestMemory, at: u32, v: u32) -> Result<(), Cause> {
    mem.set_u32(at, v).map_err(|_| UNWRITABLE)
}

/// `0x00120900(node)`: the node into the queue before the first node of
/// lower priority (at the head when it beats the head).
fn insert(mem: &mut dyn GuestMemory, node: u32) -> Result<(), Cause> {
    let priority = word(&*mem, node.wrapping_add(PRIORITY))? as i32;
    let mut before = 0;
    let mut at = word(&*mem, QUEUE)?;
    while at != 0 && priority <= word(&*mem, at.wrapping_add(PRIORITY))? as i32 {
        before = at;
        at = word(&*mem, at.wrapping_add(NEXT))?;
    }
    set(mem, node.wrapping_add(NEXT), at)?;
    set(mem, if before == 0 { QUEUE } else { before.wrapping_add(NEXT) }, node)
}

/// Take `node` out of the queue; false when no link leads to it.
fn unlink(mem: &mut dyn GuestMemory, node: u32) -> Result<bool, Cause> {
    let head = word(&*mem, QUEUE)?;
    if head == node {
        let next = word(&*mem, node.wrapping_add(NEXT))?;
        set(mem, QUEUE, next)?;
        return Ok(true);
    }
    let mut at = head;
    let mut hops = 0;
    while at != 0 && hops <= NODE_COUNT {
        let next = word(&*mem, at.wrapping_add(NEXT))?;
        if next == node {
            let after = word(&*mem, node.wrapping_add(NEXT))?;
            set(mem, at.wrapping_add(NEXT), after)?;
            return Ok(true);
        }
        at = next;
        hops += 1;
    }
    Ok(false)
}

/// The node back on the free list, its id -1 (`0x00120a30`, then the
/// worker's push).
fn release(mem: &mut dyn GuestMemory, node: u32) -> Result<(), Cause> {
    set(mem, node.wrapping_add(ID), u32::MAX)?;
    let free = word(&*mem, FREE)?;
    set(mem, node.wrapping_add(NEXT), free)?;
    set(mem, FREE, node)
}

/// The node an id names, when the id is still its own.
fn node_of(mem: &dyn GuestMemory, id: u32) -> Result<Option<u32>, Cause> {
    let node = NODES + (id & 0xff) * NODE_SIZE;
    Ok((id & 0xff < NODE_COUNT && word(mem, node.wrapping_add(ID))? == id).then_some(node))
}

/// Queue a read (`0x00213760`): its id, or `None` when the pool has no
/// free node (nothing is changed; ask again later). The loaded flag is
/// cleared.
pub fn queue(mem: &mut dyn GuestMemory, request: Request) -> Result<Option<u32>, Cause> {
    let node = word(&*mem, FREE)?;
    if node == 0 {
        return Ok(None);
    }
    let next = word(&*mem, node.wrapping_add(NEXT))?;
    set(mem, FREE, next)?;
    set(mem, node.wrapping_add(NEXT), 0)?;
    let loads = word(&*mem, LOADS)?.wrapping_add(1);
    set(mem, LOADS, loads)?;
    mem.set_u8(request.loaded_at, 0).map_err(|_| UNWRITABLE)?;
    for (at, v) in [
        (PRIORITY, request.priority),
        (HANDLE, request.handle),
        (DEST, request.dest),
        (SIZE, request.size),
        (OFFSET, request.offset),
        (DONE_AT, 0),
        (READ, 0),
        (FLAGS, 0),
        (0x24, 0),
        (0x28, 0),
        (0x2c, 0),
        (KIND, 3),
        (CALLBACK, CHUNK_READ),
        (LOADED_AT, request.loaded_at),
    ] {
        set(mem, node.wrapping_add(at), v)?;
    }
    insert(mem, node)?;
    let serial = word(&*mem, SERIAL)?;
    let id = (serial << 8) | ((node - NODES) / NODE_SIZE);
    set(mem, node.wrapping_add(ID), id)?;
    set(mem, SERIAL, serial.wrapping_add(1))?;
    Ok(Some(id))
}

/// `0x00120ce0(id, priority)`: the read's priority set and the node put
/// back in order. False when the id no longer names a queued read (it
/// finished).
pub fn raise(mem: &mut dyn GuestMemory, id: u32, priority: u32) -> Result<bool, Cause> {
    let Some(node) = node_of(&*mem, id)? else {
        return Ok(false);
    };
    if !unlink(mem, node)? {
        return Err(BROKEN_QUEUE);
    }
    set(mem, node.wrapping_add(PRIORITY), priority)?;
    insert(mem, node)?;
    Ok(true)
}

/// Drop a read that has not finished (its cache slot was taken for
/// something else): the node leaves the queue, the flag stays 0. False
/// when it had finished.
pub fn cancel(mem: &mut dyn GuestMemory, id: u32) -> Result<bool, Cause> {
    let Some(node) = node_of(&*mem, id)? else {
        return Ok(false);
    };
    if !unlink(mem, node)? {
        return Err(BROKEN_QUEUE);
    }
    release(mem, node)?;
    Ok(true)
}

/// Whether the read `id` names is still queued.
pub fn pending(mem: &dyn GuestMemory, id: u32) -> Result<bool, Cause> {
    node_of(mem, id).map(|n| n.is_some())
}

/// The worker's loop (`0x00120c30`) run to the queue's end: each head
/// read from its file at its offset into guest memory (`0x001a0e70`), the
/// done pointer written, the loaded flag set (also on a short read, as the
/// worker does), the node freed. The count finished; a failed read leaves
/// its node at the head and stops.
pub fn pump(mem: &mut dyn GuestMemory, source: &mut dyn FileSource) -> Result<usize, Cause> {
    let mut finished = 0;
    loop {
        let node = word(&*mem, QUEUE)?;
        if node == 0 {
            return Ok(finished);
        }
        let at = |field: u32| node.wrapping_add(field);
        let (size, read) = (word(&*mem, at(SIZE))?, word(&*mem, at(READ))?);
        let left = size.saturating_sub(read);
        let mut buf = vec![0; left as usize];
        let got = if left == 0 {
            0
        } else {
            source.read(word(&*mem, at(HANDLE))?, word(&*mem, at(OFFSET))?, &mut buf)?.min(buf.len())
        };
        mem.write(word(&*mem, at(DEST))?.wrapping_add(read), &buf[..got]).map_err(|_| UNWRITABLE)?;
        let done_at = word(&*mem, at(DONE_AT))?;
        if done_at != 0 {
            set(mem, done_at, read + got as u32)?;
        }
        set(mem, at(READ), read + got as u32)?;
        let loaded_at = word(&*mem, at(LOADED_AT))?;
        if loaded_at != 0 {
            mem.set_u8(loaded_at, 1).map_err(|_| UNWRITABLE)?;
        }
        let next = word(&*mem, at(NEXT))?;
        set(mem, QUEUE, next)?;
        release(mem, node)?;
        finished += 1;
    }
}

/// The pool as `0x00120a90` leaves it: every node free and linked.
#[cfg(test)]
pub(crate) fn init(m: &mut dyn GuestMemory) {
    for i in 0..NODE_COUNT {
        let node = NODES + i * NODE_SIZE;
        m.set_u32(node + ID, u32::MAX).unwrap();
        let next = if i + 1 < NODE_COUNT { node + NODE_SIZE } else { 0 };
        m.set_u32(node + NEXT, next).unwrap();
    }
    m.set_u32(FREE, NODES).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const FLAG: u32 = 0x0053_0003;
    const DEST_AT: u32 = 0x0054_0000;

    fn pool() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
        init(&mut m);
        m
    }

    fn request(offset: u32, size: u32, priority: u32, at: u32) -> Request {
        Request { handle: 0x77, offset, size, dest: DEST_AT + at, loaded_at: FLAG + at, priority }
    }

    /// A file of bytes counting up, one read log.
    struct Fake {
        reads: Vec<(u32, u32, usize)>,
        len: u32,
    }

    impl FileSource for Fake {
        fn read(&mut self, handle: u32, offset: u32, out: &mut [u8]) -> Result<usize, Cause> {
            self.reads.push((handle, offset, out.len()));
            let n = (self.len.saturating_sub(offset) as usize).min(out.len());
            for (i, b) in out[..n].iter_mut().enumerate() {
                *b = (offset as usize + i) as u8;
            }
            Ok(n)
        }
    }

    #[test]
    fn a_read_is_queued_not_loaded_then_completed() {
        let mut m = pool();
        let id = queue(&mut m, request(0x1000, 0x200, 2, 0)).unwrap().unwrap();
        assert_eq!(id, 0);
        assert_eq!(m.u8(FLAG).unwrap(), 0);
        assert!(pending(&m, id).unwrap());
        assert_eq!(m.u32(LOADS).unwrap(), 1);
        assert_eq!(m.u32(SERIAL).unwrap(), 1);
        let mut file = Fake { reads: vec![], len: 0x10_0000 };
        assert_eq!(pump(&mut m, &mut file), Ok(1));
        assert_eq!(file.reads, vec![(0x77, 0x1000, 0x200)]);
        assert_eq!(m.u8(FLAG).unwrap(), 1);
        assert_eq!(m.u8(DEST_AT + 5).unwrap(), 5);
        assert!(!pending(&m, id).unwrap());
        // The node is free again, and the next id is a new one.
        assert_eq!(m.u32(QUEUE).unwrap(), 0);
        assert_eq!(queue(&mut m, request(0, 0x200, 2, 0x1000)).unwrap(), Some(0x100));
    }

    #[test]
    fn a_short_read_ends_the_read_loaded() {
        let mut m = pool();
        queue(&mut m, request(0x1f0, 0x200, 2, 0)).unwrap();
        let mut file = Fake { reads: vec![], len: 0x200 };
        assert_eq!(pump(&mut m, &mut file), Ok(1));
        assert_eq!(m.u8(FLAG).unwrap(), 1);
        assert_eq!(m.u8(DEST_AT + 0x10).unwrap(), 0);
    }

    #[test]
    fn the_queue_runs_highest_priority_first_and_raise_reorders() {
        let mut m = pool();
        let low = queue(&mut m, request(0x000, 4, 1, 0)).unwrap().unwrap();
        let mid = queue(&mut m, request(0x100, 4, 2, 0x10)).unwrap().unwrap();
        let mid2 = queue(&mut m, request(0x200, 4, 2, 0x20)).unwrap().unwrap();
        let mut file = Fake { reads: vec![], len: 0x1000 };
        assert_eq!(raise(&mut m, low, 7), Ok(true));
        assert_eq!(pump(&mut m, &mut file), Ok(3));
        let offsets: Vec<u32> = file.reads.iter().map(|r| r.1).collect();
        assert_eq!(offsets, vec![0x000, 0x100, 0x200]);
        // Finished reads cannot be raised or cancelled.
        assert_eq!(raise(&mut m, mid, 9), Ok(false));
        assert_eq!(cancel(&mut m, mid2), Ok(false));
    }

    #[test]
    fn the_queue_orders_by_priority() {
        let mut m = pool();
        queue(&mut m, request(0x000, 4, 1, 0)).unwrap();
        queue(&mut m, request(0x100, 4, 5, 0x10)).unwrap();
        queue(&mut m, request(0x200, 4, 2, 0x20)).unwrap();
        let mut file = Fake { reads: vec![], len: 0x1000 };
        pump(&mut m, &mut file).unwrap();
        let offsets: Vec<u32> = file.reads.iter().map(|r| r.1).collect();
        assert_eq!(offsets, vec![0x100, 0x200, 0x000]);
    }

    #[test]
    fn a_cancelled_read_never_loads() {
        let mut m = pool();
        let gone = queue(&mut m, request(0x000, 4, 2, 0)).unwrap().unwrap();
        queue(&mut m, request(0x100, 4, 1, 0x10)).unwrap();
        assert_eq!(cancel(&mut m, gone), Ok(true));
        let mut file = Fake { reads: vec![], len: 0x1000 };
        assert_eq!(pump(&mut m, &mut file), Ok(1));
        assert_eq!(m.u8(FLAG).unwrap(), 0);
        assert_eq!(m.u8(FLAG + 0x10).unwrap(), 1);
    }

    #[test]
    fn an_exhausted_pool_queues_nothing() {
        let mut m = pool();
        for _ in 0..NODE_COUNT {
            assert!(queue(&mut m, request(0, 4, 2, 0)).unwrap().is_some());
        }
        assert_eq!(queue(&mut m, request(0, 4, 2, 0)), Ok(None));
        assert_eq!(m.u32(LOADS).unwrap(), NODE_COUNT);
    }

    #[test]
    fn a_failed_read_stays_queued() {
        let mut m = pool();
        queue(&mut m, request(0, 4, 2, 0)).unwrap();
        let mut broken = |_: u32, _: u32, _: &mut [u8]| -> Result<usize, Cause> { Err("no file") };
        assert_eq!(pump(&mut m, &mut broken), Err("no file"));
        assert_ne!(m.u32(QUEUE).unwrap(), 0);
        assert_eq!(m.u8(FLAG).unwrap(), 0);
    }
}
