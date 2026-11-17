//! A synthetic machine saved to the store and loaded into a second machine
//! booted the same way: the post-load determinism check is lockstep's
//! whole-RAM hash, as `state.load` will use it (docs/runtime/snapshots.md).
use runtime::guest::{Permissions, RAM_SIZE};
use runtime::{Dispatch, Guest, Runtime};
use snapshot::{Base, BuildKey, Header, Snapshot, SnapshotWriter, Store, guest};

fn build() -> BuildKey {
    BuildKey {
        stamp: "xbe 0000 generator test".into(),
        switches: "agent".into(),
    }
}

/// What a boot sets up before the guest runs: fixed by image and switches.
fn boot() -> Guest {
    let mut g = Guest::new();
    g.reserve_physical(0x0200_0000, 0x40_0000, &[0x8200_0000], "synthetic arena")
        .unwrap();
    g.claim_virtual(0xa0f0_0000, 0x8_0000, "synthetic hle")
        .unwrap();
    g.map_zeroed(0x1_0000, 0x10_0000, Permissions::RX).unwrap();
    g.guard_code(0x1_0000, 0x10_0000).unwrap();
    g
}

/// What playing does: allocations, writes, a mapping freed again.
fn play(g: &mut Guest, seed: u32) {
    g.map_zeroed(0x3000_0000, 0x20_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(0x3100_0000, 0x1000, Permissions::RW).unwrap();
    g.unmap(0x3100_0000, 0x1000).unwrap();
    let r = g.physical_reservations()[0].id;
    g.map_reserved(r, 0x8200_0000, 0x40_0000, Permissions::RW)
        .unwrap();
    scribble(g, seed);
}

/// Writes only, over what `play` mapped.
fn scribble(g: &mut Guest, seed: u32) {
    let mut x = seed | 1;
    for i in 0..0x8000u32 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        g.set_u32(0x3000_0000 + i * 16, x).unwrap();
        g.set_u32(0x8200_0000 + (x & 0x3f_fffc), i).unwrap();
    }
}

fn header(name: &str) -> Header {
    Header {
        build: build(),
        name: name.into(),
        tick: 100,
        presents: 200,
        notes: vec![("map".into(), "synthetic".into())],
    }
}

fn save(rt: &Runtime, name: &str, base: Option<&Base<'_>>) -> Vec<u8> {
    let mut w = SnapshotWriter::new(header(name));
    guest::save_guest(&mut w, &rt.memory(), base).unwrap();
    guest::save_tsc(&mut w, rt).unwrap();
    w.finish().unwrap()
}

fn scratch(test: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("h2-snapshot-machine-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_loaded_machine_hashes_like_the_saved_one() {
    let source = Runtime::new(boot(), Dispatch::default());
    play(&mut source.memory(), 7);
    for _ in 0..3 {
        source.read_tsc();
    }
    let saved_hash = lockstep::hash_guest(&source.memory());

    let root = scratch("load");
    let store = Store::new(&root, &build());
    let bytes = save(&source, "synthetic.fight", None);
    // Mostly-zero 64 MB of RAM: the file is a small fraction of it.
    assert!(bytes.len() < RAM_SIZE / 16, "{} bytes", bytes.len());
    store.save("synthetic.fight", &bytes).unwrap();

    let target = Runtime::new(boot(), Dispatch::default());
    let file = store.load("synthetic.fight").unwrap();
    let snapshot = Snapshot::open(&file, &build()).unwrap();
    assert_eq!(snapshot.header.tick, 100);
    guest::load_guest(&snapshot, &mut target.memory(), None).unwrap();
    guest::load_tsc(&snapshot, &target).unwrap();

    let loaded_hash = lockstep::hash_guest(&target.memory());
    assert_eq!(loaded_hash, saved_hash);
    assert_eq!(target.memory().layout(), source.memory().layout());
    assert_eq!(target.read_tsc(), source.read_tsc());

    // Both machines go on identically from here.
    for rt in [&source, &target] {
        let mut g = rt.memory();
        g.map_zeroed(0x3200_0000, 0x1000, Permissions::RW).unwrap();
        g.set_u32(0x3200_0000, 0x5eed).unwrap();
    }
    assert_eq!(
        lockstep::hash_guest(&target.memory()),
        lockstep::hash_guest(&source.memory())
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn loading_twice_gives_the_same_machine_after_play_in_between() {
    let rt = Runtime::new(boot(), Dispatch::default());
    play(&mut rt.memory(), 3);
    let bytes = save(&rt, "again", None);
    let saved = lockstep::hash_guest(&rt.memory());
    for round in 0..2 {
        // Play on, diverging further each round, then load back.
        scribble(&mut rt.memory(), 100 + round);
        rt.memory()
            .map_zeroed(0x3300_0000 + round * 0x1000, 0x1000, Permissions::RW)
            .unwrap();
        assert_ne!(lockstep::hash_guest(&rt.memory()), saved);
        let snapshot = Snapshot::open(&bytes, &build()).unwrap();
        guest::load_guest(&snapshot, &mut rt.memory(), None).unwrap();
        assert_eq!(lockstep::hash_guest(&rt.memory()), saved, "round {round}");
    }
}

#[test]
fn a_delta_loads_over_its_base() {
    let rt = Runtime::new(boot(), Dispatch::default());
    play(&mut rt.memory(), 5);
    let base_bytes = save(&rt, "base", None);
    let base_ram = rt.memory().physical_ram().to_vec();
    rt.memory().set_u32(0x3000_0040, 0xfeed).unwrap();
    let delta = save(
        &rt,
        "delta",
        Some(&Base {
            name: "base",
            ram: &base_ram,
            dirty: None,
        }),
    );
    assert!(
        delta.len() * 4 < base_bytes.len(),
        "{} vs {}",
        delta.len(),
        base_bytes.len()
    );
    let want = lockstep::hash_guest(&rt.memory());

    let target = Runtime::new(boot(), Dispatch::default());
    let snapshot = Snapshot::open(&delta, &build()).unwrap();
    let ram = snapshot.require(guest::RAM, 1..=1).unwrap();
    assert_eq!(
        snapshot::pages::base_name(&ram).unwrap().as_deref(),
        Some("base")
    );
    // The base's RAM comes from decoding the base file.
    let base_snapshot = Snapshot::open(&base_bytes, &build()).unwrap();
    let mut base_decoded = vec![0u8; RAM_SIZE];
    snapshot::pages::decode(
        &base_snapshot.require(guest::RAM, 1..=1).unwrap(),
        &mut base_decoded,
        None,
    )
    .unwrap();
    guest::load_guest(&snapshot, &mut target.memory(), Some(&base_decoded)).unwrap();
    assert_eq!(lockstep::hash_guest(&target.memory()), want);
}

#[test]
fn a_machine_booted_differently_is_refused_and_untouched() {
    let rt = Runtime::new(boot(), Dispatch::default());
    play(&mut rt.memory(), 9);
    let bytes = save(&rt, "x", None);
    let snapshot = Snapshot::open(&bytes, &build()).unwrap();

    let mut other = Guest::new();
    other
        .reserve_physical(0x0200_0000, 0x40_0000, &[0x8200_0000], "another owner")
        .unwrap();
    let other = Runtime::new(other, Dispatch::default());
    let before = lockstep::hash_guest(&other.memory());
    assert!(matches!(
        guest::load_guest(&snapshot, &mut other.memory(), None),
        Err(snapshot::Error::Restore(_))
    ));
    assert_eq!(lockstep::hash_guest(&other.memory()), before);
}
