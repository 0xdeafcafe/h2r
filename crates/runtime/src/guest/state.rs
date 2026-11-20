//! Machine snapshots of [`Guest`]: its layout as plain data, and restoring
//! that layout plus physical RAM in place (docs/runtime/snapshots.md).
//!
//! What a snapshot carries is what the guest can change at run time: the
//! virtual mappings, their permissions and the physical allocator's
//! high-water mark. Reservations, HLE claims and code guards are made by the
//! boot and the HLE crates, not by the guest; they are recorded too, but a
//! restore only checks that the target has the same ones and refuses
//! otherwise ([`RestoreError::Layout`]). A snapshot therefore loads into a
//! machine booted from the same image with the same switches, never into an
//! arbitrary one.
//!
//! Host caches over the layout are rebuilt, never restored: the mapping
//! hints are cleared and the fast-page table (if built) is refilled. Every
//! physical page a write-watch consumer watches is reported dirty, so the
//! D3D caches and anything else keyed by guest memory re-read it.
use super::{Guest, Mapping, Permissions, RAM_SIZE};
use std::{path::Path, sync::atomic::Ordering};

/// A [`super::PhysicalReservation`] as plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRecord {
    pub id: u32,
    pub physical_start: u32,
    pub len: u32,
    pub aliases: Vec<u32>,
    pub owner: String,
}

/// A [`super::VirtualClaim`] as plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRecord {
    pub id: u32,
    pub virtual_start: u32,
    pub len: u32,
    pub owner: String,
}

/// Everything about a [`Guest`] other than its RAM bytes and host caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestLayout {
    /// Sorted by virtual start, as the guest keeps them.
    pub mappings: Vec<Mapping>,
    pub next_physical: u32,
    pub next_id: u32,
    pub reservations: Vec<ReservationRecord>,
    pub claims: Vec<ClaimRecord>,
    /// Page-rounded `guard_code` ranges; ends may equal 2^32.
    pub code: Vec<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RestoreError {
    /// The target was not set up like the machine the snapshot came from.
    #[error("guest layout differs from the snapshot's: {0}")]
    Layout(String),
    /// The snapshot's mappings break an invariant `Guest::map` keeps.
    #[error("invalid mapping in snapshot: {0:x?}")]
    Mapping(Mapping),
    /// The RAM image is not `RAM_SIZE` bytes.
    #[error("snapshot RAM is {0} bytes")]
    RamSize(usize),
    /// A copy-on-write RAM base could not be mapped.
    #[error("guest RAM base: {0}")]
    RamBase(String),
}

impl Permissions {
    /// The read/write/execute bits (1, 2, 4).
    pub const fn bits(self) -> u8 {
        self.0
    }
    /// The inverse of [`Permissions::bits`]; `None` above 7.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits <= 7 { Some(Self(bits)) } else { None }
    }
}

impl Guest {
    /// The layout a snapshot records.
    pub fn layout(&self) -> GuestLayout {
        GuestLayout {
            mappings: self.mappings.clone(),
            next_physical: self.next_physical,
            next_id: self.next_id,
            reservations: self
                .reservations
                .iter()
                .map(|r| ReservationRecord {
                    id: r.id.0,
                    physical_start: r.physical_start,
                    len: r.len,
                    aliases: r.aliases.clone(),
                    owner: r.owner.to_string(),
                })
                .collect(),
            claims: self
                .claims
                .iter()
                .map(|c| ClaimRecord {
                    id: c.id.0,
                    virtual_start: c.virtual_start,
                    len: c.len,
                    owner: c.owner.to_string(),
                })
                .collect(),
            code: self.code.clone(),
        }
    }

    /// Replace the mappings and every byte of physical RAM with a
    /// snapshot's. Nothing changes unless the whole restore is valid: the
    /// reservations, claims and code guards must equal the snapshot's, and
    /// its mappings must be sorted, in RAM, outside the NV2A aperture and
    /// non-overlapping. `ram` is the snapshot's physical RAM.
    ///
    /// Write-watch subscriptions are host state and stay as they are; every
    /// watched page is marked dirty.
    pub fn restore(&mut self, layout: &GuestLayout, ram: &[u8]) -> Result<(), RestoreError> {
        if ram.len() != RAM_SIZE {
            return Err(RestoreError::RamSize(ram.len()));
        }
        self.check_fixed(layout)?;
        Self::check_mappings(&layout.mappings)?;
        self.ram.copy_from_slice(ram);
        self.adopt(layout);
        Ok(())
    }

    /// [`Guest::restore`] with RAM written by `fill` straight into the
    /// guest's own store, so a decoder needs no second 64 MB buffer. The
    /// layout is checked before `fill` runs; if `fill` fails, RAM is left
    /// as `fill` left it and the mappings are not changed, so the caller
    /// must treat the machine as lost.
    pub fn restore_with<E>(
        &mut self,
        layout: &GuestLayout,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<Result<(), E>, RestoreError> {
        self.check_fixed(layout)?;
        Self::check_mappings(&layout.mappings)?;
        if let Err(e) = fill(&mut self.ram) {
            return Ok(Err(e));
        }
        self.adopt(layout);
        Ok(Ok(()))
    }

    /// [`Guest::restore`] over a RAM image `file` of exactly [`RAM_SIZE`]
    /// bytes, mapped private copy-on-write in place of the guest's own
    /// store: pages the match never writes stay the file's shared pages —
    /// one set for every process on the same image — and a write gives
    /// this guest its own page. As `restore`, the layout must match and
    /// every watched page is reported dirty. The fast-page table holds
    /// physical numbers, not pointers, so it needs no rebuild.
    pub fn restore_cow(&mut self, layout: &GuestLayout, file: &Path) -> Result<(), RestoreError> {
        self.check_fixed(layout)?;
        Self::check_mappings(&layout.mappings)?;
        let image = std::fs::File::open(file)
            .map_err(|e| RestoreError::RamBase(format!("{}: {e}", file.display())))?;
        let bytes = image
            .metadata()
            .map_err(|e| RestoreError::RamBase(e.to_string()))?
            .len();
        if bytes != RAM_SIZE as u64 {
            return Err(RestoreError::RamBase(format!(
                "{} is {bytes} bytes, guest RAM is {RAM_SIZE}",
                file.display()
            )));
        }
        self.ram
            .become_private_of(&image)
            .map_err(|e| RestoreError::RamBase(e.to_string()))?;
        self.adopt(layout);
        Ok(())
    }

    /// The guest's RAM pages that differ from the RAM image at `base` (as
    /// [`restore_cow`] adopted it): `(written, total, runs)` with the
    /// runs of written pages as `(first page, pages)`. A match-end census
    /// of how much of a shared base can stay shared.
    pub fn census_against(&self, base: &Path) -> std::io::Result<(u64, u64, Vec<(u32, u32)>)> {
        use std::io::Read;
        let mut image = std::fs::File::open(base)?;
        let bytes = image.metadata()?.len();
        if bytes != RAM_SIZE as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{base:?} is {bytes} bytes, guest RAM is {RAM_SIZE}"),
            ));
        }
        let page = super::PAGE_SIZE as usize;
        let mut chunk = vec![0u8; 1 << 20];
        let mut written = 0u64;
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut at = 0usize;
        let ram: &[u8] = &self.ram;
        while at < RAM_SIZE {
            let n = chunk.len().min(RAM_SIZE - at);
            image.read_exact(&mut chunk[..n])?;
            for (i, theirs) in chunk[..n].chunks(page).enumerate() {
                let start = at + i * page;
                let mine = ram.get(start..start + page).expect("RAM covers the census");
                if mine != theirs {
                    written += 1;
                    match runs.last_mut() {
                        Some((first, pages)) if *first + *pages == (start / page) as u32 => {
                            *pages += 1;
                        }
                        _ => runs.push(((start / page) as u32, 1)),
                    }
                }
            }
            at += n;
        }
        Ok((written, (RAM_SIZE / page) as u64, runs))
    }

    fn adopt(&mut self, layout: &GuestLayout) {
        self.mappings = layout.mappings.clone();
        self.next_physical = layout.next_physical;
        self.next_id = layout.next_id;
        for hint in &self.mapping_hints {
            hint.store(usize::MAX, Ordering::Relaxed);
        }
        self.refresh_fast_range(0, 1u64 << 32);
        if self.watch.active() {
            self.watch.note(0, RAM_SIZE);
        }
    }

    fn check_fixed(&self, layout: &GuestLayout) -> Result<(), RestoreError> {
        let mine = self.layout();
        if mine.reservations != layout.reservations {
            return Err(RestoreError::Layout(format!(
                "reservations {:x?} here, {:x?} in the snapshot",
                mine.reservations, layout.reservations
            )));
        }
        if mine.claims != layout.claims {
            return Err(RestoreError::Layout(format!(
                "claims {:x?} here, {:x?} in the snapshot",
                mine.claims, layout.claims
            )));
        }
        if mine.code != layout.code {
            return Err(RestoreError::Layout(format!(
                "code guards {:x?} here, {:x?} in the snapshot",
                mine.code, layout.code
            )));
        }
        // Reservation ids come from the same counter as the snapshot's
        // later ones; a counter behind them would hand out an id twice.
        if layout.next_id <= mine.reservations.iter().map(|r| r.id).max().unwrap_or(0) {
            return Err(RestoreError::Layout(format!(
                "next id {} is not past the reservations",
                layout.next_id
            )));
        }
        Ok(())
    }

    fn check_mappings(mappings: &[Mapping]) -> Result<(), RestoreError> {
        let mut previous_end = 0u64;
        for m in mappings {
            let start = m.virtual_start as u64;
            let end = start + m.len as u64;
            let bad = m.len == 0
                || start < previous_end
                || m.physical_start as u64 + m.len as u64 > RAM_SIZE as u64
                || (start < super::MMIO_END && end > super::MMIO_START);
            if bad {
                return Err(RestoreError::Mapping(m.clone()));
            }
            previous_end = end;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn booted() -> Guest {
        let mut g = Guest::new();
        g.reserve_physical(0x0100_0000, 0x10000, &[0x8100_0000], "test arena")
            .unwrap();
        g.claim_virtual(0xa000_0000, 0x10000, "test claim").unwrap();
        g.guard_code(0x1_0000, 0x2000).unwrap();
        g
    }

    fn played(g: &mut Guest) {
        g.map_zeroed(0x1_0000, 0x4000, Permissions::RX).unwrap();
        g.map_zeroed(0x2000_0000, 0x3000, Permissions::RW).unwrap();
        g.map_reserved_for_test();
        g.set_u32(0x2000_0010, 0xdead_beef).unwrap();
        g.set_u32(0x8100_0040, 0x1234_5678).unwrap();
    }

    impl Guest {
        fn map_reserved_for_test(&mut self) {
            let id = self.reservations[0].id;
            self.map_reserved(id, 0x8100_0000, 0x10000, Permissions::RW)
                .unwrap();
        }
    }

    #[test]
    fn restore_reproduces_layout_and_bytes() {
        let mut source = booted();
        played(&mut source);
        let layout = source.layout();
        let ram = source.physical_ram().to_vec();

        let mut target = booted();
        target.prepare_fast_pages();
        target.restore(&layout, &ram).unwrap();
        assert_eq!(target.layout(), layout);
        assert_eq!(target.physical_ram(), source.physical_ram());
        assert_eq!(target.u32(0x2000_0010).unwrap(), 0xdead_beef);
        assert_eq!(target.u32(0x8100_0040).unwrap(), 0x1234_5678);
        target.set_u32(0x2000_0014, 7).unwrap();
        assert_eq!(target.u32(0x2000_0014).unwrap(), 7);
        // An allocation after the restore continues from the snapshot's
        // high-water mark, as the source's would.
        target
            .map_zeroed(0x3000_0000, 0x1000, Permissions::RW)
            .unwrap();
        source
            .map_zeroed(0x3000_0000, 0x1000, Permissions::RW)
            .unwrap();
        assert_eq!(target.layout(), source.layout());
    }

    #[test]
    fn restore_drops_mappings_made_after_the_snapshot() {
        let mut g = booted();
        played(&mut g);
        let layout = g.layout();
        let ram = g.physical_ram().to_vec();
        g.map_zeroed(0x3000_0000, 0x1000, Permissions::RW).unwrap();
        g.set_u32(0x2000_0010, 1).unwrap();
        g.restore(&layout, &ram).unwrap();
        assert!(g.u32(0x3000_0000).is_err());
        assert_eq!(g.u32(0x2000_0010).unwrap(), 0xdead_beef);
    }

    #[test]
    fn restore_refuses_a_machine_booted_differently() {
        let mut source = booted();
        played(&mut source);
        let layout = source.layout();
        let ram = source.physical_ram().to_vec();

        let mut other = Guest::new();
        other
            .reserve_physical(0x0100_0000, 0x20000, &[0x8100_0000], "test arena")
            .unwrap();
        other
            .claim_virtual(0xa000_0000, 0x10000, "test claim")
            .unwrap();
        other.guard_code(0x1_0000, 0x2000).unwrap();
        let before = other.layout();
        assert!(matches!(
            other.restore(&layout, &ram),
            Err(RestoreError::Layout(_))
        ));
        assert_eq!(other.layout(), before, "a refused restore changes nothing");
        assert!(other.physical_ram().iter().all(|&b| b == 0));

        let mut no_guard = Guest::new();
        no_guard
            .reserve_physical(0x0100_0000, 0x10000, &[0x8100_0000], "test arena")
            .unwrap();
        no_guard
            .claim_virtual(0xa000_0000, 0x10000, "test claim")
            .unwrap();
        assert!(matches!(
            no_guard.restore(&layout, &ram),
            Err(RestoreError::Layout(_))
        ));
    }

    #[test]
    fn restore_refuses_bad_mappings_and_sizes() {
        let mut g = booted();
        let mut layout = g.layout();
        layout.mappings = vec![
            Mapping {
                virtual_start: 0x2000_0000,
                len: 0x2000,
                physical_start: 0,
                permissions: Permissions::RW,
            },
            Mapping {
                virtual_start: 0x2000_1000,
                len: 0x1000,
                physical_start: 0x2000,
                permissions: Permissions::RW,
            },
        ];
        let ram = vec![0; RAM_SIZE];
        assert!(matches!(
            g.restore(&layout, &ram),
            Err(RestoreError::Mapping(_))
        ));
        layout.mappings.truncate(1);
        assert_eq!(
            g.restore(&layout, &ram[..4096]),
            Err(RestoreError::RamSize(4096))
        );
        g.restore(&layout, &ram).unwrap();
    }

    #[test]
    fn restore_marks_watched_pages_dirty() {
        let mut g = booted();
        played(&mut g);
        let layout = g.layout();
        let ram = g.physical_ram().to_vec();
        let id = g.subscribe_writes("test");
        g.watch_writes(id, 0x8000_0000, RAM_SIZE as u32).unwrap();
        assert!(!g.has_dirty(id));
        g.restore(&layout, &ram).unwrap();
        assert!(g.has_dirty(id));
    }

    /// A RAM image on disk, for the copy-on-write base.
    struct RamImage(std::path::PathBuf);
    impl RamImage {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            let p =
                std::env::temp_dir().join(format!("h2-ram-base-{tag}-{}.bin", std::process::id()));
            std::fs::write(&p, bytes).unwrap();
            Self(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for RamImage {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_cow_base_serves_the_snapshot_and_keeps_writes_private() {
        let mut source = booted();
        played(&mut source);
        let layout = source.layout();
        let image = RamImage::new("cow", source.physical_ram());
        let before = std::fs::read(image.path()).unwrap();

        let mut target = booted();
        target.prepare_fast_pages();
        target.restore_cow(&layout, image.path()).unwrap();
        assert_eq!(target.physical_ram(), source.physical_ram());
        assert_eq!(target.u32(0x2000_0010).unwrap(), 0xdead_beef);
        // A write stays in this guest; the file never changes.
        target.set_u32(0x2000_0010, 7).unwrap();
        assert_eq!(target.u32(0x2000_0010).unwrap(), 7);
        assert_eq!(std::fs::read(image.path()).unwrap(), before);

        // The census counts the written pages and their runs.
        let (written, total, runs) = target.census_against(image.path()).unwrap();
        assert_eq!(total, (RAM_SIZE / super::PAGE_SIZE as usize) as u64);
        assert_eq!(written, 1, "{runs:?}");
        assert_eq!(runs, [(0x2000_0000 / super::PAGE_SIZE, 1)]);

        // A wrong-sized image is a named error, not a mapping.
        let short = RamImage::new("short", &[0u8; 4096]);
        assert!(matches!(
            target.restore_cow(&layout, short.path()),
            Err(RestoreError::RamBase(_))
        ));
    }

    #[test]
    fn restore_with_fills_in_place_and_reports_fill_errors() {
        let mut source = booted();
        played(&mut source);
        let layout = source.layout();
        let mut target = booted();
        let filled = target
            .restore_with(&layout, |ram| {
                ram.copy_from_slice(source.physical_ram());
                Ok::<(), ()>(())
            })
            .unwrap();
        assert_eq!(filled, Ok(()));
        assert_eq!(target.layout(), layout);

        let mut failing = booted();
        let before = failing.layout();
        assert_eq!(
            failing.restore_with(&layout, |_| Err("bad page")).unwrap(),
            Err("bad page")
        );
        assert_eq!(failing.layout(), before);
    }

    #[test]
    fn permissions_bits_round_trip() {
        for p in [
            Permissions::NONE,
            Permissions::READ,
            Permissions::RW,
            Permissions::RX,
            Permissions::RWX,
        ] {
            assert_eq!(Permissions::from_bits(p.bits()), Some(p));
        }
        assert_eq!(Permissions::from_bits(8), None);
    }
}
