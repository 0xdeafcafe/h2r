use super::*;

fn probe(value: u64, name: u16, scratch: u32) -> Probe {
    Probe { value, name, scratch }
}

/// A world with a few records, a freed slot in the middle and a stepped RNG.
fn sample() -> World {
    let mut w = World::new(7);
    let a = w.probe.alloc(probe(10, 1, 0)).unwrap();
    let b = w.probe.alloc(probe(20, 2, 0)).unwrap();
    w.probe.alloc(probe(30, 3, 0)).unwrap();
    w.probe.free(b).unwrap();
    w.probe.get_mut(a).unwrap().value = 11;
    w.rng.next_u32();
    w.rng.next_u32();
    w
}

#[test]
fn save_restore_round_trips_bytes() {
    let w = sample();
    let bytes = w.save();
    let back = World::restore(&bytes).unwrap();
    assert_eq!(back.save(), bytes);
    assert_eq!(back.sim_hash(), w.sim_hash());
    assert_eq!(back.rng, w.rng);
    // The freed slot and the scan start survive: the next allocation lands
    // where it would have in the original.
    let (mut a, mut b) = (w, back);
    assert_eq!(a.probe.alloc(probe(1, 1, 0)), b.probe.alloc(probe(1, 1, 0)));
}

#[test]
fn hash_ignores_derived_state() {
    let mut a = sample();
    let mut b = sample();
    assert_eq!(a.sim_hash(), b.sim_hash());
    let h = a.probe.handles()[0];
    a.probe.get_mut(h).unwrap().scratch = 999;
    assert_eq!(a.sim_hash(), b.sim_hash());
    assert_eq!(a.save(), b.save());
    // Simulation state does move the hash, after the cached value was read.
    b.probe.get_mut(h).unwrap().value += 1;
    assert_ne!(a.sim_hash(), b.sim_hash());
    b.rng.next_u32();
    let before = b.sim_hash();
    b.rng.next_u32();
    assert_ne!(before, b.sim_hash());
}

#[test]
fn restored_derived_state_is_rebuilt_not_saved() {
    let mut w = sample();
    let h = w.probe.handles()[0];
    w.probe.get_mut(h).unwrap().scratch = 5;
    let back = World::restore(&w.save()).unwrap();
    assert_eq!(back.probe.get(h).unwrap().scratch, 0);
}

#[test]
fn pool_iterates_in_slot_order() {
    let mut p: Pool<Probe> = Pool::new(8, 0x1234);
    let h: Vec<_> = (0..4).map(|i| p.alloc(probe(i, 0, 0)).unwrap()).collect();
    p.free(h[1]).unwrap();
    let d = p.alloc(probe(9, 0, 0)).unwrap();
    assert_eq!(d.index(), 1, "the lowest free slot is taken first");
    let order: Vec<u64> = p.iter().map(|(_, r)| r.value).collect();
    assert_eq!(order, [0, 9, 2, 3]);
    let slots: Vec<u16> = p.iter().map(|(h, _)| h.index()).collect();
    assert_eq!(slots, [0, 1, 2, 3]);
}

#[test]
fn handles_are_salt_and_index_with_retail_salts() {
    let mut p: Pool<Probe> = Pool::new(4, 0x1234);
    let a = p.alloc(Probe::default()).unwrap();
    let b = p.alloc(Probe::default()).unwrap();
    assert_eq!(a.raw(), 0x9234_0000);
    assert_eq!(b.raw(), 0x9235_0001);
    assert_eq!(Handle::NONE.raw(), 0xffff_ffff);
    // A full pool refuses.
    p.alloc(Probe::default()).unwrap();
    p.alloc(Probe::default()).unwrap();
    assert!(p.alloc(Probe::default()).is_none());
}

#[test]
fn salt_skips_ffff() {
    let mut p: Pool<Probe> = Pool::new(2, 0x7ffe);
    let a = p.alloc(Probe::default()).unwrap();
    p.free(a).unwrap();
    let b = p.alloc(Probe::default()).unwrap();
    assert_eq!(a.salt(), 0xfffe);
    assert_eq!(b.salt(), 0x8000);
}

#[test]
fn stale_handle_is_refused() {
    let mut p: Pool<Probe> = Pool::new(4, 1);
    let old = p.alloc(probe(1, 0, 0)).unwrap();
    p.free(old).unwrap();
    assert!(p.get(old).is_none());
    assert!(p.get_mut(old).is_none());
    assert_eq!(p.free(old), Err(StaleHandle(old)));
    let new = p.alloc(probe(2, 0, 0)).unwrap();
    assert_eq!(new.index(), old.index());
    assert_ne!(new.salt(), old.salt());
    assert!(p.get(old).is_none());
    assert_eq!(p.free(old), Err(StaleHandle(old)));
    assert_eq!(p.get(new).unwrap().value, 2);
    assert!(p.get(Handle::NONE).is_none());
}

#[test]
fn rng_stream_is_golden() {
    // Retail's generator (`imul 0x19660d; add 0x3c6ef35f`, docs/engine/sim-world.md)
    // from seed 7: the first draws, the last, and fnv64 over all 1,024 as
    // little-endian words.
    let mut r = Rng::new(7);
    let draws: Vec<u32> = (0..1024).map(|_| r.next_u32()).collect();
    assert_eq!(
        draws[..6],
        [0x3d20_bdba, 0xe9da_b1d1, 0x9ccc_40fc, 0xed4e_a82b, 0x0ca1_9f8e, 0xdb14_a195]
    );
    assert_eq!(draws[1023], 0x027c_0407);
    let mut h = Fnv64::new();
    for d in &draws {
        h.u32(*d);
    }
    assert_eq!(h.finish(), 0xaa49_c80e_3fa9_50ef);
}

#[test]
fn fnv64_matches_the_reference_vectors() {
    assert_eq!(Fnv64::of(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(Fnv64::of(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(Fnv64::of(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn restore_refuses_other_layout() {
    let mut bytes = sample().save();
    // Header: magic (4), stamp length (4), then the stamp.
    bytes[8 + 11] ^= 1;
    match World::restore(&bytes) {
        Err(RestoreError::OtherLayout { found }) => assert_ne!(found, BUILD_STAMP),
        other => panic!("expected OtherLayout, got {:?}", other.err()),
    }
    let mut bad_magic = sample().save();
    bad_magic[0] = b'X';
    assert_eq!(World::restore(&bad_magic).err(), Some(RestoreError::BadMagic));
}

#[test]
fn restore_failure_leaves_world_unchanged() {
    let mut w = sample();
    let (bytes, hash) = (w.save(), w.sim_hash());

    let mut other_stamp = bytes.clone();
    other_stamp[8 + 11] ^= 1;
    let truncated = &bytes[..bytes.len() - 3];
    let mut ghost = bytes.clone();
    // An unknown section kind appended after the valid ones.
    ghost.extend_from_slice(&[0xff, 0x7f, 1, 0, 0, 0, 0, 0]);

    for bad in [other_stamp.as_slice(), truncated, ghost.as_slice(), &[]] {
        assert!(w.restore_into(bad).is_err());
        assert_eq!(w.save(), bytes);
        assert_eq!(w.sim_hash(), hash);
    }
    w.restore_into(&bytes).unwrap();
    assert_eq!(w.save(), bytes);
}

#[test]
fn restore_refuses_inconsistent_pools() {
    let mut w = World::new(1);
    w.probe.alloc(probe(1, 1, 0)).unwrap();
    let bytes = w.save();
    // The probe section's `active` field, last of the pool header, is one
    // word before the first slot: bump it.
    let at = bytes.len() - (2 + 8 + 2) - 4;
    let mut bad = bytes.clone();
    bad[at] = 2;
    assert!(matches!(
        World::restore(&bad),
        Err(RestoreError::Corrupt { kind: kind::PROBE, .. })
    ));
}

/// A subsystem store for the tests: one `u32`, saved in the sound kind.
struct Counter(u32);

impl Store for Counter {
    fn kind(&self) -> u16 {
        kind::SOUND
    }
    fn version(&self) -> u16 {
        1
    }
    fn encode(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        if version != 1 {
            return Err(RestoreError::SectionVersion { kind: kind::SOUND, version });
        }
        let mut r = Reader::new(body);
        let v = r.u32()?;
        r.end(kind::SOUND)?;
        Ok(Box::new(v))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        self.0 = *state.downcast::<u32>().unwrap();
    }
}

fn held(w: &World) -> u32 {
    u32::from_le_bytes(w.store(kind::SOUND).unwrap().encode().try_into().unwrap())
}

#[test]
fn a_held_store_saves_in_its_kinds_place_and_restores_in_place() {
    let mut w = sample();
    w.attach(Box::new(Counter(41))).unwrap();
    assert!(w.attach(Box::new(Counter(0))).is_err(), "one store a kind");
    let (bytes, hash) = (w.save(), w.sim_hash());
    // RNG, then sound, then the probe.
    let mut r = Reader::new(&bytes);
    save::read_header(&mut r).unwrap();
    let mut kinds = Vec::new();
    while !r.is_empty() {
        kinds.push(r.u16().unwrap());
        r.u16().unwrap();
        let len = r.u32().unwrap() as usize;
        r.take(len).unwrap();
    }
    assert_eq!(kinds, [kind::RNG, kind::SOUND, kind::PROBE]);

    let mut back = World::new(1);
    back.attach(Box::new(Counter(0))).unwrap();
    back.restore_into(&bytes).unwrap();
    assert_eq!((held(&back), back.save(), back.sim_hash()), (41, bytes.clone(), hash));
    // A world without the store refuses the section by kind.
    assert_eq!(World::restore(&bytes).err(), Some(RestoreError::UnknownKind { kind: kind::SOUND }));
    // A save without it is refused by a world that holds it, unchanged.
    let plain = sample().save();
    assert_eq!(back.restore_into(&plain).err(), Some(RestoreError::MissingSection { kind: kind::SOUND }));
    assert_eq!(back.save(), bytes);
}
