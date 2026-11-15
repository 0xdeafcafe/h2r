//! The second cache volume, N: (`0x002d5d0a`), through the adapter.
mod common;

use common::*;
use input::{InputManager, host::ConvertConfig};
use xapi::{
    CachePartition, CacheSelection, Config, Error, ThreadState, Xapi,
    adapter::{Function, Outcome},
    cache::{self, CacheRow},
};

/// The kernel's seed: one valid row, this title on Partition5 (index 2).
fn seeded() -> (Xapi<Utility>, Memory) {
    let (mut xapi, mut memory, _) = setup();
    memory.put(UTILITY_ROW, &0u32.to_le_bytes());
    memory.put(COUNT_IMPORT, &COUNT.to_le_bytes());
    memory.put(COUNT, &3u32.to_le_bytes());
    memory.put(CERTIFICATE_POINTER, &CERTIFICATE.to_le_bytes());
    memory.put(CERTIFICATE + 8, &TITLE_ID.to_le_bytes());
    xapi.storage.database = Some(database(&[(TITLE_ID, 2, 1)]));
    xapi.storage.mounted = Some(5);
    (xapi, memory)
}

fn mount(xapi: &mut Xapi<Utility>, memory: &mut Memory, thread: &mut ThreadState) -> u32 {
    let (eax, outcome) = call(xapi, memory, thread, Function::XMountCacheDrive, &[]).unwrap();
    assert_eq!(outcome, Outcome::Return { pc: 0x1234_5678 });
    eax
}

#[test]
fn abi_is_stdcall_with_no_arguments_and_a_bool() {
    assert_eq!(Function::XMountCacheDrive.stack_words(), 0);
    assert!(Function::XMountCacheDrive.is_replacement());
    // `call` checks the popped PC and that ESP rises by exactly 4.
    let (mut xapi, mut memory) = seeded();
    let mut thread = ThreadState { last_error: 77 };
    assert_eq!(mount(&mut xapi, &mut memory, &mut thread), 1);
    assert_eq!(thread.last_error, 77, "success leaves the last error");
}

#[test]
fn seeded_database_puts_n_on_partition4_in_the_last_row() {
    let (mut xapi, mut memory) = seeded();
    let mut thread = ThreadState::default();
    let before = memory.bytes.clone();
    assert_eq!(mount(&mut xapi, &mut memory, &mut thread), 1);
    assert_eq!(memory.word(CACHE_ROW), 2, "N: row = count - 1");
    assert_eq!(memory.word(UTILITY_ROW), 0, "Z: row untouched");
    assert_eq!(xapi.storage.calls, ["cache 4"]);
    let db = xapi.storage.database.unwrap();
    assert_eq!(xapi.storage.database_writes, 1);
    assert_eq!(
        cache::row(&db, 0),
        CacheRow {
            title_id: TITLE_ID,
            index: 2,
            valid: 1
        },
        "Z:'s row untouched"
    );
    assert_eq!(
        cache::row(&db, 2),
        CacheRow {
            title_id: TITLE_ID,
            index: 1,
            valid: 0
        }
    );
    // Only the slot, the frame and nothing else changed in guest memory.
    for (i, (a, b)) in before.iter().zip(&memory.bytes).enumerate() {
        let i = i as u32;
        if a != b {
            assert!(
                (CACHE_ROW..CACHE_ROW + 4).contains(&i) || (0x1000..0x1004).contains(&i),
                "{i:#x} changed"
            );
        }
    }
}

#[test]
fn a_failed_database_write_is_ignored_like_retail() {
    let (mut xapi, mut memory) = seeded();
    xapi.storage.write_failure = Some(Error::Win32(19));
    let mut thread = ThreadState { last_error: 77 };
    assert_eq!(mount(&mut xapi, &mut memory, &mut thread), 1);
    assert_eq!(thread.last_error, 77);
    assert_eq!(memory.word(CACHE_ROW), 2);
    assert_eq!(xapi.storage.database_writes, 1, "the write was attempted");
    assert_eq!(xapi.storage.calls, ["cache 4"]);
}

#[test]
fn unmounted_z_fails_before_touching_the_n_slot() {
    let (mut xapi, mut memory) = seeded();
    memory.put(UTILITY_ROW, &u32::MAX.to_le_bytes());
    memory.put(CACHE_ROW, &0x1111_1111u32.to_le_bytes());
    let mut thread = ThreadState::default();
    assert_eq!(mount(&mut xapi, &mut memory, &mut thread), 0);
    assert_eq!(thread.last_error, cache::ERROR_GEN_FAILURE);
    assert_eq!(memory.word(CACHE_ROW), 0x1111_1111);
    assert!(xapi.storage.calls.is_empty());
    assert_eq!(xapi.storage.database_writes, 0);
}

#[test]
fn failures_after_the_row_check_publish_minus_one() {
    // (setup, expected last error, database written?)
    type Case = (fn(&mut Xapi<Utility>, &mut Memory), u32, bool);
    let cases: [Case; 6] = [
        (|x, _| x.storage.database = None, 21, false),
        (
            |x, _| {
                let mut db = database(&[(TITLE_ID, 2, 1)]);
                db[4] = 3; // version
                x.storage.database = Some(db);
            },
            cache::ERROR_INTERNAL_DB_CORRUPTION,
            false,
        ),
        (
            |x, _| {
                let mut db = database(&[(TITLE_ID, 2, 1)]);
                db[0x1fb] = 0;
                x.storage.database = Some(db);
            },
            cache::ERROR_INTERNAL_DB_CORRUPTION,
            false,
        ),
        // The format/link failed: the database was already rewritten, as
        // retail writes it before formatting.
        (
            |x, _| x.storage.cache_failure = Some(Error::Win32(5)),
            5,
            true,
        ),
        // Z: row outside the table.
        (
            |_, m| m.put(UTILITY_ROW, &3u32.to_le_bytes()),
            cache::ERROR_GEN_FAILURE,
            false,
        ),
        // A single cache partition leaves no row for N:.
        (
            |_, m| m.put(COUNT, &1u32.to_le_bytes()),
            cache::ERROR_GEN_FAILURE,
            false,
        ),
    ];
    for (i, (prepare, error, written)) in cases.into_iter().enumerate() {
        let (mut xapi, mut memory) = seeded();
        prepare(&mut xapi, &mut memory);
        memory.put(CACHE_ROW, &7u32.to_le_bytes());
        let mut thread = ThreadState::default();
        assert_eq!(mount(&mut xapi, &mut memory, &mut thread), 0, "case {i}");
        assert_eq!(thread.last_error, error, "case {i}");
        assert_eq!(memory.word(CACHE_ROW), u32::MAX, "case {i}");
        assert_eq!(
            xapi.storage.database_writes,
            usize::from(written),
            "case {i}"
        );
        assert!(xapi.storage.calls.is_empty(), "case {i}");
    }
}

#[test]
fn guest_faults_are_faults_and_leave_the_slot() {
    let (mut xapi, mut memory) = seeded();
    memory.put(CACHE_ROW, &7u32.to_le_bytes());
    memory.put(CERTIFICATE_POINTER, &0x7fffu32.to_le_bytes());
    let mut thread = ThreadState::default();
    assert!(matches!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XMountCacheDrive,
            &[]
        ),
        Err(Error::Memory(_))
    ));
    assert_eq!(memory.word(CACHE_ROW), 7);
    assert!(xapi.storage.calls.is_empty());
    // An unwritable slot faults before any storage access.
    let (mut xapi, mut memory) = seeded();
    memory.readonly = Some((CACHE_ROW, 4));
    assert!(matches!(
        xapi.mount_cache_drive(&mut memory),
        Err(Error::Memory(m)) if m.address == CACHE_ROW && m.write
    ));
    assert_eq!(xapi.storage.database_writes, 0);
}

fn pick(
    rows: &[(u32, u32, u32)],
    count: u32,
    z: u32,
    policy: CachePartition,
) -> xapi::Result<CacheSelection> {
    cache::select(&mut database(rows), count, z, TITLE_ID, policy, 5)
}

#[test]
fn selection_follows_the_retail_rule() {
    use CachePartition::*;
    let sel = |row, partition| Ok(CacheSelection { row, partition });
    // Highest free partition that is not Z:'s.
    assert_eq!(pick(&[(TITLE_ID, 2, 1)], 3, 0, Select), sel(2, 4));
    // Z: in the last row: N: takes the row before it.
    assert_eq!(
        pick(&[(1, 0, 1), (2, 1, 0), (TITLE_ID, 2, 1)], 3, 2, Select),
        sel(1, 4)
    );
    // Valid rows hold partitions; an invalid row does not.
    assert_eq!(
        pick(&[(TITLE_ID, 2, 1), (9, 1, 1), (8, 0, 0)], 3, 0, Select),
        sel(2, 3)
    );
    // Nothing free: reuse the partition N:'s row names.
    assert_eq!(
        pick(&[(TITLE_ID, 2, 1), (9, 1, 1), (8, 0, 1)], 3, 0, Select),
        sel(2, 3)
    );
    // An invalid row naming the highest free partition does not hold it.
    assert_eq!(
        pick(&[(TITLE_ID, 2, 1), (9, 1, 0)], 3, 0, Select),
        sel(2, 4)
    );
    // Z:'s partition is excluded even when its row is not valid.
    assert_eq!(
        pick(&[(TITLE_ID, 2, 0), (9, 0, 1)], 3, 0, Select),
        sel(2, 4)
    );
    // A configured partition, and the conflicts it is checked for.
    assert_eq!(pick(&[(TITLE_ID, 2, 1)], 3, 0, Fixed(3)), sel(2, 3));
    assert!(pick(&[(TITLE_ID, 1, 1)], 3, 0, Fixed(4)).is_err(), "Z:'s");
    assert!(
        pick(&[(TITLE_ID, 0, 1)], 3, 0, Fixed(5)).is_err(),
        "Config's Z:"
    );
    // Table geometry retail would index past.
    for (count, z) in [(0, 0), (1, 0), (2, 2), (42, 0), (u32::MAX, 0)] {
        assert!(
            pick(&[(TITLE_ID, 0, 1)], count, z, Select).is_err(),
            "{count} {z}"
        );
    }
}

#[test]
fn configured_partition_must_not_be_the_utility_partition() {
    for (partition, ok) in [(3, true), (4, true), (5, false), (2, false), (6, false)] {
        let mut config = Config::for_gamepad(0x100);
        config.cache_drive.partition = CachePartition::Fixed(partition);
        let input = InputManager::new(ConvertConfig::default()).unwrap();
        assert_eq!(
            Xapi::new(config, input, Utility::default()).is_ok(),
            ok,
            "{partition}"
        );
    }
}

#[test]
fn storage_without_cache_support_fails_cleanly() {
    struct ZOnly;
    impl xapi::Storage for ZOnly {
        fn mount_utility(&mut self, _: u8, _: bool) -> xapi::Result<()> {
            Ok(())
        }
        fn format_utility(&mut self) -> xapi::Result<()> {
            Ok(())
        }
    }
    let mut config = Config::for_gamepad(0x100);
    config.cache_drive.globals = test_cache_globals();
    let input = InputManager::new(ConvertConfig::default()).unwrap();
    let mut x = Xapi::new(config, input, ZOnly).unwrap();
    let (_, mut memory) = seeded();
    assert_eq!(x.mount_cache_drive(&mut memory), Err(Error::NOT_SUPPORTED));
    assert_eq!(memory.word(CACHE_ROW), u32::MAX);
}
