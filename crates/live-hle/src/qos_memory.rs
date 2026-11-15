//! Released XNQOS pointers have normal C allocation lifetimes. Reuse their
//! storage so periodic lobby queries cannot exhaust a monotonic task arena.
use crate::state::Storage;
use runtime::Guest;
use std::collections::BTreeMap;

pub(crate) struct QosMemory {
    storage: Storage,
    live: BTreeMap<u32, u32>,
    free: BTreeMap<u32, u32>,
}
impl Default for QosMemory {
    fn default() -> Self {
        Self {
            storage: Storage::with_size(0x10_0000),
            live: BTreeMap::new(),
            free: BTreeMap::new(),
        }
    }
}
impl QosMemory {
    pub fn alloc(&mut self, m: &mut Guest, size: u32) -> Option<u32> {
        let size = size.checked_add(7)? & !7;
        let found = self
            .free
            .iter()
            .find(|(_, n)| **n >= size)
            .map(|(&p, &n)| (p, n));
        let ptr = if let Some((p, n)) = found {
            self.free.remove(&p);
            if n > size {
                self.free.insert(p + size, n - size);
            }
            p
        } else {
            self.storage.alloc(m, size)?
        };
        self.live.insert(ptr, size);
        Some(ptr)
    }
    pub fn release(&mut self, ptr: u32) -> bool {
        let Some(mut size) = self.live.remove(&ptr) else {
            return false;
        };
        let mut start = ptr;
        if let Some((&p, &n)) = self.free.range(..ptr).next_back()
            && p + n == ptr
        {
            self.free.remove(&p);
            start = p;
            size += n;
        }
        if let Some(n) = self.free.remove(&(start + size)) {
            size += n;
        }
        self.free.insert(start, size);
        true
    }
    pub fn clear(&mut self) {
        while let Some((&ptr, _)) = self.live.first_key_value() {
            self.release(ptr);
        }
    }
}
