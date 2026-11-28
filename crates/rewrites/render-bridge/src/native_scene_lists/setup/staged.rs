//! Shared writes during a recursive setup. Later children read the
//! palette cursor, caches and records the earlier entries would publish.

use std::collections::BTreeMap;

use super::*;

pub(super) struct Staged<'a> {
    base: &'a dyn GuestMemory,
    edits: BTreeMap<u32, Vec<u8>>,
}

impl<'a> Staged<'a> {
    pub(super) fn new(base: &'a dyn GuestMemory) -> Self {
        Self {
            base,
            edits: BTreeMap::new(),
        }
    }

    pub(super) fn apply(&mut self, output: Output) -> Result<()> {
        for write in output.writes {
            self.write(write.addr, write.bytes.slice())?;
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Output {
        let mut out = Output::default();
        let mut run = None::<(u32, Vec<u8>)>;
        for (addr, bytes) in self.edits {
            if let Some((start, span)) = &mut run
                && u64::from(*start) + span.len() as u64 == u64::from(addr)
            {
                span.extend(bytes);
                continue;
            }
            if let Some((start, span)) = run.replace((addr, bytes)) {
                out.array(start, span);
            }
        }
        if let Some((addr, bytes)) = run {
            out.array(addr, bytes);
        }
        out
    }
}

impl GuestMemory for Staged<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> std::result::Result<(), MemFault> {
        self.base.read(addr, buf)?;
        let end = u64::from(addr) + buf.len() as u64;
        let previous = self.edits.range(..addr).next_back();
        for (&start, edit) in previous.into_iter().chain(
            self.edits
                .range(addr..)
                .take_while(|(start, _)| u64::from(**start) < end),
        ) {
            let first = addr.max(start) as u64;
            let last = end.min(u64::from(start) + edit.len() as u64);
            if first < last {
                let to = (first - u64::from(addr)) as usize;
                let from = (first - u64::from(start)) as usize;
                let len = (last - first) as usize;
                buf[to..to + len].copy_from_slice(&edit[from..from + len]);
            }
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, buf: &[u8]) -> std::result::Result<(), MemFault> {
        if buf.is_empty() {
            return Ok(());
        }
        let end = u64::from(addr) + buf.len() as u64;
        if end > 1 << 32 {
            return Err(MemFault {
                addr,
                len: buf.len() as u32,
                write: true,
            });
        }
        // Keep final ranges disjoint, even when a child changed a
        // parent-written byte.
        if let Some((&start, prior)) = self.edits.range_mut(..addr).next_back() {
            let left = (addr - start) as usize;
            if left < prior.len() {
                let tail = prior.split_off(left);
                if tail.len() > buf.len() {
                    self.edits.insert(end as u32, tail[buf.len()..].to_vec());
                }
            }
        }
        while let Some((&start, _)) = self.edits.range(addr..).next() {
            if u64::from(start) >= end {
                break;
            }
            let covered = (end - u64::from(start)) as usize;
            let bytes = self.edits.remove(&start).unwrap();
            if covered < bytes.len() {
                self.edits.insert(end as u32, bytes[covered..].to_vec());
            }
        }
        self.edits.insert(addr, buf.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn recursive_setup_observes_only_latest_staged_writes() {
        let m = FlatMemory::new(0x1000, 32);
        let mut plan = Staged::new(&m);
        plan.write(0x1004, &[1, 2, 3, 4, 5, 6]).unwrap();
        plan.write(0x1006, &[7, 8]).unwrap();
        plan.write(0x1002, &[9, 10, 11, 12, 13]).unwrap();
        assert_eq!(plan.bytes(0x1002, 8).unwrap(), [9, 10, 11, 12, 13, 8, 5, 6]);
        assert_eq!(m.u32(0x1004).unwrap(), 0);
        let writes = plan.finish().writes;
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].addr, 0x1002);
        assert_eq!(writes[0].bytes.slice(), [9, 10, 11, 12, 13, 8, 5, 6]);
    }
}
