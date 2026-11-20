//! Released XNQOS pointers have normal C allocation lifetimes. Reuse their
//! storage so periodic lobby queries cannot exhaust a monotonic task arena.
use crate::state::Storage;
use runtime::Guest;
use std::collections::BTreeMap;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
    pub(crate) fn validate(&self, claims: &[(u32, u32)]) -> Result<(), String> {
        let (mut next, end) = self
            .storage
            .used_range(claims, 0x10_0000)?
            .unwrap_or_default();
        let mut blocks: Vec<_> = self.live.iter().chain(&self.free).collect();
        blocks.sort_unstable_by_key(|entry| *entry.0);
        for (&start, &len) in blocks {
            if start != next || len == 0 || len % 8 != 0 {
                return Err("invalid Live QoS allocation".into());
            }
            next = next
                .checked_add(len)
                .ok_or("Live QoS allocation overflow")?;
        }
        if next != end {
            return Err("incomplete Live QoS arena".into());
        }
        Ok(())
    }
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
