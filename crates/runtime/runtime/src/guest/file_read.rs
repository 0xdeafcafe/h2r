//! A private file mapping at the canonical physical RAM address. The VFS
//! supplies the pinned region it would read, including logical EOF.
use super::Guest;

impl Guest {
    /// Install the same bytes as a checked guest write. On refusal, guest
    /// memory is unchanged and NtReadFile uses its normal copy path.
    pub fn map_file_read(
        &mut self, addr: u32, source: &vfs::ImmutableRead<'_>,
    ) -> Result<usize, &'static str> {
        #[cfg(unix)]
        {
            self.map_read_region(addr, source.file(), source.offset(), source.byte_len())
        }
        #[cfg(not(unix))]
        {
            let _ = (addr, source);
            Err("file-backed guest pages require mmap")
        }
    }

    #[cfg(unix)]
    fn map_read_region(
        &mut self, addr: u32, file: &std::fs::File, offset: u64, len: usize,
    ) -> Result<usize, &'static str> {
        use std::os::unix::fs::FileExt;
        // A flat window must keep aliasing RAM, including after any later
        // remap. Traps and recorded HLE outputs see the ordinary write path.
        if self.aliases_ram() || self.hle_writes.is_some() || self.trap.is_some() {
            return Err("aliased or instrumented guest RAM");
        }
        let mut index = self.checked_start(addr, len, super::Access::Write, 1)
            .ok().flatten().ok_or("guest range is not writable")?;
        let end_va = u64::from(addr) + len as u64;
        let mut span = self.span(index, u64::from(addr), end_va);
        while span.len() < len {
            index += 1;
            let next = self.span(index, u64::from(addr) + span.len() as u64, end_va);
            if next.start != span.end {
                return Err("noncontiguous physical guest pages");
            }
            span.end = next.end;
        }
        let page = crate::cow::Ram::host_page_size().ok_or("unknown host page size")?;
        let (head, middle) = map_span(span.start, offset, len, page)?;
        let tail = len - head - middle;
        let end = offset.checked_add(len as u64).ok_or("file offset overflow")?;
        if file.metadata().map_err(|_| "file metadata unavailable")?.len() < end {
            return Err("backing file is shorter than logical read");
        }
        // Collect partial edges before changing any guest bytes. Positioned
        // reads preserve the backend's open file description and seek cursor.
        let mut edges = vec![0; head + tail];
        if head != 0 {
            file.read_exact_at(&mut edges[..head], offset).map_err(|_| "short leading edge")?;
        }
        if tail != 0 {
            file.read_exact_at(&mut edges[head..], end - tail as u64).map_err(|_| "short trailing edge")?;
        }
        self.ram.install_file_pages(span.start + head, file, offset + head as u64, middle, page)?;
        self.ram[span.start..span.start + head].copy_from_slice(&edges[..head]);
        self.ram[span.end - tail..span.end].copy_from_slice(&edges[head..]);
        if self.watch.active() {
            self.watch.note(span.start, len);
        }
        Ok(middle)
    }
}

/// The common whole-page interior. File offsets need to be congruent with
/// the physical RAM offset, which can differ from a guest virtual address.
#[cfg(unix)]
fn map_span(physical: usize, offset: u64, len: usize, page: usize) -> Result<(usize, usize), &'static str> {
    if (physical % page) as u64 != offset % page as u64 {
        return Err("noncongruent file and guest pages");
    }
    let head = physical.wrapping_neg() % page;
    let pages = len.saturating_sub(head) / page;
    if pages < 16 {
        return Err("fewer than sixteen whole host pages");
    }
    Ok((head, pages * page))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn whole_pages_and_partial_edges_follow_the_host_page_size() {
        for page in [4096, 16384] {
            let len = 17 * page + 20;
            assert_eq!(map_span(3, 3 + 4 * page as u64, len, page), Ok((page - 3, 16 * page)));
            assert!(map_span(3, 4, len, page).is_err());
            assert!(map_span(3, 3, 16 * page, page).is_err());
        }
    }
}
