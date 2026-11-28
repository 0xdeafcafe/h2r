//! Reading and putting back the state a start point holds: the game-state
//! arena and the `SimExt` records (docs/game/input-film.md, "Start
//! points"). The envelope itself is [`super::start_point`].
//!
//! The arena is read and written as the bytes behind `SimRegion::HALO2`'s
//! pointer word, at a tick boundary, and the extension records go through
//! the checkpoint section ([`ExtRecords`]). Nothing here follows an
//! address the start point holds. A load writes the whole arena and then
//! hashes it the way [`lockstep::hash_sim`] does; whether that is all the
//! engine needs restored is what the load's `matches` answers.
use super::start_point::StartPoint;
use anyhow::{Context, Result, ensure};
use guest_layout::GuestMemory;
use lockstep::SimRegion;

/// The extension records, as a start point holds them. The process's are
/// [`Global`]; tests use a fake.
pub trait ExtRecords {
    /// The checkpoint section: records and slots.
    fn save(&self) -> Vec<u8>;
    /// Replace the records; a refusal changes nothing.
    fn load(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// The live records' digest, `None` with none.
    fn digest(&self) -> Option<u64>;
}

/// The process's `SimExt` state (`sim_ext::global`).
pub struct Global;

impl ExtRecords for Global {
    fn save(&self) -> Vec<u8> {
        sim_ext::global().save_section()
    }
    fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        sim_ext::global()
            .load_section(bytes)
            .map_err(|e| e.to_string())
    }
    fn digest(&self) -> Option<u64> {
        sim_ext::digest()
    }
}

/// Where the arena is and how long it is.
fn span(mem: &dyn GuestMemory) -> Result<(u32, usize)> {
    let SimRegion::Indirect { pointer, len } = SimRegion::HALO2 else {
        unreachable!("the Halo 2 arena is behind a pointer word");
    };
    let base = mem
        .u32(pointer)
        .map_err(|e| anyhow::anyhow!("the arena pointer is unreadable at {:#x}", e.addr))?;
    ensure!(base != 0, "the game has no arena yet");
    Ok((base, len as usize))
}

/// The arena's size in this build, which a reader checks a start point's
/// own against.
pub fn arena_len() -> usize {
    match SimRegion::HALO2 {
        SimRegion::Indirect { len, .. } | SimRegion::Fixed { len, .. } => len as usize,
    }
}

fn read_arena(mem: &dyn GuestMemory) -> Result<Vec<u8>> {
    let (base, len) = span(mem)?;
    let mut bytes = vec![0u8; len];
    mem.read(base, &mut bytes)
        .map_err(|e| anyhow::anyhow!("the arena is unreadable at {:#x}", e.addr))?;
    Ok(bytes)
}

/// The arena hash with the records folded in, as `lockstep::hash_sim` has
/// it.
pub fn current_hash(mem: &dyn GuestMemory, ext: &dyn ExtRecords) -> Result<u64> {
    Ok(lockstep::hash_arena(&read_arena(mem)?, ext.digest()))
}

/// The state at the end of `tick`, as a start point. The caller reads at a
/// tick boundary, so the arena is whole.
pub fn capture(
    mem: &dyn GuestMemory,
    ext: &dyn ExtRecords,
    tick: u32,
    ids: Vec<u8>,
) -> Result<StartPoint> {
    let arena = read_arena(mem)?;
    Ok(StartPoint {
        tick,
        arena_hash: lockstep::hash_arena(&arena, ext.digest()),
        ids,
        arena,
        ext: ext.save(),
    })
}

/// Put a start point's state into the game and return the hash of what is
/// there after. The records go first, since a refusal changes nothing; the
/// arena follows in one write. The caller compares with
/// [`StartPoint::check_loaded`].
pub fn load(
    mem: &mut dyn GuestMemory,
    ext: &mut dyn ExtRecords,
    point: &StartPoint,
) -> Result<u64> {
    let (base, len) = span(mem)?;
    ensure!(
        point.arena.len() == len,
        "start point arena is {} bytes, this build's is {len}",
        point.arena.len()
    );
    ext.load(&point.ext)
        .map_err(anyhow::Error::msg)
        .context("start point extension records are refused")?;
    mem.write(base, &point.arena)
        .map_err(|e| anyhow::anyhow!("the arena is unwritable at {:#x}", e.addr))?;
    current_hash(mem, ext)
}

#[cfg(test)]
pub(super) mod fakes {
    use super::*;
    use guest_layout::FlatMemory;

    pub const POINTER: u32 = 0x004e_6080;
    pub const ARENA: u32 = 0x004e_7000;

    /// Memory holding the arena pointer and an arena of the build's size,
    /// filled from `seed`.
    pub fn guest(seed: u8) -> FlatMemory {
        let mut m = FlatMemory::new(POINTER, (ARENA - POINTER) as usize + arena_len());
        m.write(POINTER, &ARENA.to_le_bytes()).unwrap();
        let bytes: Vec<u8> = (0..arena_len()).map(|i| (i / 13) as u8 ^ seed).collect();
        m.write(ARENA, &bytes).unwrap();
        m
    }

    #[derive(Default)]
    pub struct Ext {
        pub bytes: Vec<u8>,
        pub refuse: bool,
    }

    impl ExtRecords for Ext {
        fn save(&self) -> Vec<u8> {
            self.bytes.clone()
        }
        fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
            if self.refuse {
                return Err("refused".into());
            }
            self.bytes = bytes.to_vec();
            Ok(())
        }
        fn digest(&self) -> Option<u64> {
            (!self.bytes.is_empty()).then(|| self.bytes.iter().map(|&b| u64::from(b)).sum())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;

    #[test]
    fn a_capture_loads_back_into_another_state_with_the_same_hash() {
        let mut ext = Ext {
            bytes: vec![1, 2, 3],
            refuse: false,
        };
        let source = guest(1);
        let point = capture(&source, &ext, 77, vec![9; 4]).unwrap();
        assert_eq!(point.tick, 77);
        assert_eq!(point.arena.len(), arena_len());
        assert_eq!(point.arena_hash, current_hash(&source, &ext).unwrap());

        // The same arena and records, restored over a different state.
        let mut target = guest(2);
        ext.bytes = vec![8];
        assert_ne!(current_hash(&target, &ext).unwrap(), point.arena_hash);
        let after = load(&mut target, &mut ext, &point).unwrap();
        point.check_loaded(after).unwrap();
        assert_eq!(ext.bytes, [1, 2, 3]);
    }

    #[test]
    fn a_start_point_that_hashes_wrong_after_loading_is_reported() {
        let ext = Ext::default();
        let mut point = capture(&guest(1), &ext, 5, vec![]).unwrap();
        point.arena_hash ^= 1;
        let mut target = guest(1);
        let after = load(&mut target, &mut Ext::default(), &point).unwrap();
        assert!(point.check_loaded(after).is_err());
    }

    #[test]
    fn nothing_is_written_when_the_records_or_the_size_are_refused() {
        let ext = Ext {
            bytes: vec![4],
            refuse: false,
        };
        let point = capture(&guest(1), &ext, 5, vec![]).unwrap();

        let mut target = guest(2);
        let before = current_hash(&target, &Ext::default()).unwrap();
        let mut refusing = Ext {
            bytes: vec![],
            refuse: true,
        };
        assert!(load(&mut target, &mut refusing, &point).is_err());
        assert_eq!(current_hash(&target, &Ext::default()).unwrap(), before);

        let mut short = point.clone();
        short.arena.pop();
        assert!(load(&mut target, &mut Ext::default(), &short).is_err());
        assert_eq!(current_hash(&target, &Ext::default()).unwrap(), before);
    }

    #[test]
    fn no_arena_pointer_is_an_error_not_a_zero_state() {
        let mut m = guest(1);
        m.write(POINTER, &0u32.to_le_bytes()).unwrap();
        assert!(capture(&m, &Ext::default(), 1, vec![]).is_err());
        assert!(current_hash(&m, &Ext::default()).is_err());
    }
}
