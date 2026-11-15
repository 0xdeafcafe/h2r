//! Monotonic guest handle identities; closing a handle never aliases an old
//! value to a new object. Objects may outlive all handles (e.g. live threads).
use crate::status::{self, NtStatus};
use std::collections::BTreeMap;
pub type ObjectId = u32;
#[derive(Debug, Clone, Default)]
pub struct HandleTable {
    next: u32,
    entries: BTreeMap<u32, ObjectId>,
}
impl HandleTable {
    /// Fails with STATUS_INSUFFICIENT_RESOURCES once `limits::MAX_HANDLES`
    /// handles are live or the monotonic value space is exhausted.
    pub fn insert(&mut self, object: ObjectId) -> Result<u32, NtStatus> {
        let handle = self.next_value()?;
        self.next = handle;
        self.entries.insert(handle, object);
        Ok(handle)
    }
    /// Whether one more `insert` would succeed. Callers with side effects
    /// (opening a file, mapping a stack) check this first so a full table
    /// fails before anything is changed; the kernel lock keeps it valid until
    /// the following `insert`.
    pub fn check_capacity(&self) -> Result<(), NtStatus> {
        self.next_value().map(|_| ())
    }
    fn next_value(&self) -> Result<u32, NtStatus> {
        if self.entries.len() >= crate::limits::MAX_HANDLES {
            return Err(status::INSUFFICIENT_RESOURCES);
        }
        self.next
            .checked_add(4)
            .filter(|h| *h < 0xffff_fff0)
            .ok_or(status::INSUFFICIENT_RESOURCES)
    }
    pub fn get(&self, handle: u32) -> Result<ObjectId, NtStatus> {
        self.entries
            .get(&handle)
            .copied()
            .ok_or(status::INVALID_HANDLE)
    }
    pub fn close(&mut self, handle: u32) -> Result<ObjectId, NtStatus> {
        self.entries.remove(&handle).ok_or(status::INVALID_HANDLE)
    }
    pub fn duplicate(&mut self, handle: u32) -> Result<u32, NtStatus> {
        let id = self.get(handle)?;
        self.insert(id)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn references(&self, object: ObjectId) -> usize {
        self.entries.values().filter(|id| **id == object).count()
    }
}
