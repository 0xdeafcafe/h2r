//! The reproducible arms of the geometry resource lookup (0x0012de70).
//! Queue submissions, priority raises and relocation invoke guest services.

use super::staged::Staged;
use super::*;

const RESOURCE_BASE: u32 = 0x004e_6494;
const CACHE: u32 = 0x004e_649c;
const REQUESTS: u32 = 0x004e_648c;
const NONBLOCKING: u32 = 0x0046_8c4c;
const LOAD_PHASE: u32 = 0x004e_64a0;
const LOAD_FILTER: u32 = 0x004e_64a4;
const STRUCTURE_MISSES: u32 = 0x004e_64a8;

pub(super) fn touch(m: &mut Staged<'_>, resource: u32, request: bool) -> Result<bool> {
    if m.u8(at(resource, 0x1e)?)? != 0 {
        let handle = m.u16(at(resource, 0x20)?)?;
        stamp(m, u32::from(handle))?;
        return Ok(true);
    }
    if m.i32(at(resource, 8)?)? <= 0 || m.i32(at(resource, 0xc)?)? < 0 {
        return Ok(false);
    }
    let tag_index = m.u16(at(resource, 0x18)?)?;
    if tag_index > i16::MAX as u16 {
        return Err(Refusal::Layout);
    }
    let group = m.u32(at(ptr(m, TAGS)?, u32::from(tag_index) * 16)?)?;
    let structure = group == u32::from_be_bytes(*b"ltmp") || group == u32::from_be_bytes(*b"sbsp");
    let handle = m.u32(at(resource, 0x20)?)?;
    if request && handle == NONE {
        // Submitting storage I/O is an external side effect. The complete
        // original call still owns the request when there is no queue entry.
        return Err(Refusal::Residency);
    }
    if handle != NONE {
        let (priority, wait) = if request {
            request_policy(m, structure)?
        } else {
            (2, false)
        };
        let index = handle & 0xffff;
        let requests = Pool::read(m, ptr(m, REQUESTS)?, 12)?;
        let entry = requests.slot(m, handle)?;
        if m.u8(at(entry, 2)?)? != 0 || m.u8(at(entry, 3)?)? < priority || wait {
            return Err(Refusal::Residency);
        }
        stamp(m, index)?;
    }
    if structure {
        let count = m.u32(STRUCTURE_MISSES)?;
        m.set_u32(STRUCTURE_MISSES, count.wrapping_add(1))?;
    }
    Ok(false)
}

fn request_policy(m: &dyn GuestMemory, structure: bool) -> Result<(u8, bool)> {
    if m.u8(NONBLOCKING)? == 0 {
        return Ok((7, true));
    }
    let phase = m.i32(LOAD_PHASE)?;
    if phase != 1 {
        return Ok((2, false));
    }
    let filter = m.u32(LOAD_FILTER)?;
    Ok((2, filter == 0 || filter == 1 && structure))
}

fn stamp(m: &mut Staged<'_>, index: u32) -> Result<()> {
    let cache = ptr(m, CACHE)?;
    let pool = ptr(m, at(cache, 0x64)?)?;
    let end = m.u32(at(pool, 0x38)?)?;
    if index >= end || m.u32(at(pool, 0x24)?)? != 0x18 {
        return Err(Refusal::Layout);
    }
    let slots = ptr(m, at(pool, 0x44)?)?;
    let epoch = m.u32(at(cache, 0x38)?)?;
    m.set_u32(at(slots, index * 0x18 + 0x14)?, epoch)?;
    Ok(())
}

pub(in crate::native_scene_lists) fn resource_cache_in_use(m: &dyn GuestMemory) -> Result<bool> {
    Ok(m.u32(RESOURCE_BASE)? != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn resident_resource_writes_the_current_cache_epoch() {
        let resource = 0x10000;
        let cache = 0x20000;
        let pool = 0x21000;
        let slots = 0x22000;
        let mut mem = FakeGuest::new();
        mem.put_u8(resource + 0x1e, 1)
            .put_u16(resource + 0x20, 1)
            .put_u32(CACHE, cache)
            .put_u32(cache + 0x64, pool)
            .put_u32(cache + 0x38, 324)
            .put_u32(pool + 0x24, 0x18)
            .put_u32(pool + 0x38, 2)
            .put_u32(pool + 0x44, slots)
            .put_u32(slots + 0x18 + 0x14, 13);
        let mut plan = Staged::new(&mem);
        assert_eq!(touch(&mut plan, resource, true), Ok(true));
        assert_eq!(plan.u32(slots + 0x18 + 0x14).unwrap(), 324);
        assert_eq!(mem.u32(slots + 0x18 + 0x14).unwrap(), 13);
    }
    #[test]
    fn already_queued_nonblocking_request_counts_the_miss_and_stamps_the_slot() {
        let resource = 0x10000;
        let cache = 0x20000;
        let pool = 0x21000;
        let slots = 0x22000;
        let requests = 0x23000;
        let queue = 0x24000;
        let tags = 0x25000;
        let handle = 0x8900_0000;
        let mut mem = FakeGuest::new();
        mem.put_u32(resource + 8, 12)
            .put_u32(resource + 0x20, handle)
            .put_u32(CACHE, cache)
            .put_u32(cache + 0x64, pool)
            .put_u32(cache + 0x38, 15)
            .put_u32(pool + 0x24, 0x18)
            .put_u32(pool + 0x38, 1)
            .put_u32(pool + 0x44, slots)
            .put_u32(slots + 0x14, 1)
            .put_u32(REQUESTS, requests)
            .put_u32(requests + 0x24, 12)
            .put_u32(requests + 0x38, 1)
            .put_u32(requests + 0x44, queue)
            .put_u16(queue, (handle >> 16) as u16)
            .put_u8(queue + 3, 2)
            .put_u32(TAGS, tags)
            .put_u32(tags, u32::from_be_bytes(*b"sbsp"))
            .put_u32(STRUCTURE_MISSES, 3)
            .put_u8(NONBLOCKING, 1);
        let mut plan = Staged::new(&mem);
        assert_eq!(touch(&mut plan, resource, true), Ok(false));
        assert_eq!(plan.u32(slots + 0x14).unwrap(), 15);
        assert_eq!(plan.u32(STRUCTURE_MISSES).unwrap(), 4);
        assert_eq!(plan.u32(resource + 0x20).unwrap(), handle);
        assert_eq!(mem.u32(slots + 0x14).unwrap(), 1);
    }
}
