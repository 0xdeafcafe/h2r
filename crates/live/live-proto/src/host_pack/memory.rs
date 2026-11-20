//! Linux smaps accounting shared by the game and the pack supervisor.

/// Resident bytes from one complete smaps rollup or the sum of a process group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidentMemory {
    pub pss: u64,
    pub private: u64,
    pub shared: u64,
    pub private_dirty: u64,
    pub shared_clean: u64,
}

impl ResidentMemory {
    /// Linux prints these counters in KiB. Missing or malformed counters
    /// mean the split is unavailable (a process may exit mid-sample).
    pub fn from_rollup(text: &str) -> Option<Self> {
        let mut pss = None;
        let mut private_clean = None;
        let mut private_dirty = None;
        let mut shared_clean = None;
        let mut shared_dirty = None;
        for line in text.lines() {
            let Some((name, value)) = line.split_once(':') else { continue };
            let field = match name {
                "Pss" => &mut pss,
                "Private_Clean" => &mut private_clean,
                "Private_Dirty" => &mut private_dirty,
                "Shared_Clean" => &mut shared_clean,
                "Shared_Dirty" => &mut shared_dirty,
                _ => continue,
            };
            let kib: u64 = value.trim().strip_suffix("kB")?.trim().parse().ok()?;
            *field = Some(kib.checked_mul(1024)?);
        }
        Some(Self {
            pss: pss?,
            private: private_clean?.checked_add(private_dirty?)?,
            shared: shared_clean?.checked_add(shared_dirty?)?,
            private_dirty: private_dirty?,
            shared_clean: shared_clean?,
        })
    }

    pub fn add_process(&mut self, process: Self) {
        self.pss += process.pss;
        self.private += process.private;
        self.shared += process.shared;
        self.private_dirty += process.private_dirty;
        self.shared_clean += process.shared_clean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exact_counters_in_bytes_and_adds_processes() {
        let text = "Pss_Anon: 99 kB\nPss: 7 kB\nPrivate_Clean: 1 kB\nPrivate_Dirty: 4 kB\nShared_Clean: 3 kB\nShared_Dirty: 2 kB\n";
        let m = ResidentMemory::from_rollup(text).unwrap();
        assert_eq!((m.pss, m.private, m.shared), (7168, 5120, 5120));
        assert_eq!((m.private_dirty, m.shared_clean), (4096, 3072));
        let mut group = m;
        group.add_process(m);
        assert_eq!(group.pss, 14336);
        assert_eq!(ResidentMemory::from_rollup("Pss: 0 kB"), None);
        assert_eq!(ResidentMemory::from_rollup(&text.replace("7 kB", "7 B")), None);
    }
}
