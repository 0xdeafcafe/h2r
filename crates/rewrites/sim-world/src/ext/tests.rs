use super::*;

/// A blob written field by field, independently of `SimExt::save`.
fn blob(format: u16, records: &[(&str, u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SEXT");
    out.extend_from_slice(&format.to_le_bytes());
    out.extend_from_slice(&(records.len() as u16).to_le_bytes());
    for (name, version, data) in records {
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

fn filled() -> SimExt {
    let mut ext = SimExt::new();
    ext.set(&RULES, vec![1, 2, 3]).unwrap();
    ext.set(&FORGE, vec![9; 5]).unwrap();
    ext
}

#[test]
fn checksums_match_their_reference_values() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    let mut h = Fnv64::new();
    assert_eq!(h.finish(), 0xcbf2_9ce4_8422_2325);
    h.write(b"a");
    assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn an_empty_container_is_zero_bytes_and_has_no_digest() {
    let ext = SimExt::new();
    assert!(ext.save().is_empty());
    assert_eq!(ext.encoded_len(), 0);
    assert_eq!(ext.digest(), None);
    let mut full = filled();
    full.restore(&[]).unwrap();
    assert!(full.is_empty());
}

#[test]
fn the_byte_form_is_the_documented_one() {
    let ext = filled();
    let expected = blob(
        FORMAT,
        &[("forge", 1, vec![9; 5]), ("rules", 1, vec![1, 2, 3])],
    );
    assert_eq!(ext.save(), expected);
    assert_eq!(ext.encoded_len(), expected.len());
}

#[test]
fn records_round_trip_through_their_bytes() {
    let ext = filled();
    let bytes = ext.save();
    let mut back = SimExt::new();
    back.restore(&bytes).unwrap();
    assert_eq!(back, ext);
    assert_eq!(back.save(), bytes, "the form is canonical");
    assert_eq!(back.get(&RULES), Some(&[1, 2, 3][..]));
    assert_eq!(back.names().collect::<Vec<_>>(), ["forge", "rules"]);
}

#[test]
fn another_container_format_is_refused_and_changes_nothing() {
    let mut ext = filled();
    let before = ext.clone();
    let newer = blob(FORMAT + 1, &[("rules", 1, vec![7])]);
    assert_eq!(
        ext.restore(&newer),
        Err(Error::Format { found: FORMAT + 1 })
    );
    assert_eq!(ext, before);
}

#[test]
fn a_record_of_another_version_is_refused_by_name() {
    let mut writer = SimExt::bare();
    writer.register(Kind::new("rules", 2, 16 * 1024)).unwrap();
    writer
        .set(&Kind::new("rules", 2, 16 * 1024), vec![4, 5])
        .unwrap();
    let mut ext = filled();
    let before = ext.clone();
    assert_eq!(
        ext.restore(&writer.save()),
        Err(Error::RecordVersion {
            name: "rules".into(),
            found: 2,
            expected: 1
        })
    );
    assert_eq!(ext, before);
}

#[test]
fn an_unregistered_kind_is_refused_until_registered() {
    const TEST: Kind = Kind::new("test", 1, 64);
    let bytes = blob(FORMAT, &[("test", 1, b"hello".to_vec())]);
    let mut ext = SimExt::new();
    assert_eq!(ext.restore(&bytes), Err(Error::Unregistered("test".into())));
    assert_eq!(
        ext.set(&TEST, vec![1]),
        Err(Error::Unregistered("test".into()))
    );
    ext.register(TEST).unwrap();
    ext.register(TEST).unwrap();
    assert_eq!(
        ext.register(Kind::new("test", 2, 64)),
        Err(Error::Conflict("test".into()))
    );
    ext.restore(&bytes).unwrap();
    assert_eq!(ext.get(&TEST), Some(&b"hello"[..]));
}

#[test]
fn damaged_or_non_canonical_bytes_are_refused() {
    let good = filled().save();
    let mut ext = SimExt::new();

    let mut flipped = good.clone();
    flipped[12] ^= 1;
    assert!(matches!(ext.restore(&flipped), Err(Error::Crc { .. })));

    assert!(ext.restore(&good[..good.len() - 1]).is_err());
    assert_eq!(ext.restore(&good[..6]), Err(Error::Truncated));

    let mut magic = good.clone();
    magic[0] = b'X';
    assert_eq!(ext.restore(&magic), Err(Error::Magic));

    let unordered = blob(FORMAT, &[("rules", 1, vec![1]), ("forge", 1, vec![2])]);
    assert!(matches!(
        ext.restore(&unordered),
        Err(Error::NotCanonical(_))
    ));
    let repeated = blob(FORMAT, &[("rules", 1, vec![1]), ("rules", 1, vec![2])]);
    assert!(matches!(
        ext.restore(&repeated),
        Err(Error::NotCanonical(_))
    ));
    let none = blob(FORMAT, &[]);
    assert!(matches!(ext.restore(&none), Err(Error::NotCanonical(_))));

    let bad_name = blob(FORMAT, &[("Rules", 1, vec![1])]);
    assert_eq!(ext.restore(&bad_name), Err(Error::Name("Rules".into())));

    let mut trailing = blob(FORMAT, &[("rules", 1, vec![1])]);
    trailing.truncate(trailing.len() - 4);
    trailing.push(0);
    let crc = crc32(&trailing);
    trailing.extend_from_slice(&crc.to_le_bytes());
    assert_eq!(ext.restore(&trailing), Err(Error::Trailing));

    assert!(ext.is_empty(), "no refusal changed anything");
}

#[test]
fn caps_refuse_a_record_and_the_container() {
    let mut ext = SimExt::new();
    assert_eq!(
        ext.set(&RULES, vec![0; RULES.max_len + 1]),
        Err(Error::RecordTooLarge {
            name: "rules".into(),
            len: RULES.max_len + 1,
            max: RULES.max_len
        })
    );
    const BIG: Kind = Kind::new("big", 1, MAX_BYTES);
    ext.register(BIG).unwrap();
    ext.set(&RULES, vec![1]).unwrap();
    assert!(matches!(
        ext.set(&BIG, vec![0; MAX_BYTES - 20]),
        Err(Error::TooLarge { .. })
    ));
    assert_eq!(ext.get(&BIG), None);
    assert_eq!(ext.get(&RULES), Some(&[1][..]));

    let oversized = blob(FORMAT, &[("rules", 1, vec![0; RULES.max_len + 1])]);
    assert!(matches!(
        SimExt::new().restore(&oversized),
        Err(Error::RecordTooLarge { .. })
    ));
}

#[test]
fn records_are_isolated_from_each_other() {
    let mut ext = filled();
    let forge = ext.get(&FORGE).unwrap().to_vec();
    for v in 0..4u8 {
        ext.set(&RULES, vec![v; usize::from(v) + 1]).unwrap();
        assert_eq!(ext.get(&FORGE), Some(&forge[..]));
    }
    assert_eq!(ext.remove(&RULES), Some(vec![3; 4]));
    assert_eq!(ext.get(&FORGE), Some(&forge[..]));
    // A kind of the same name and another version reads and writes nothing.
    let other = Kind::new("forge", 2, 64);
    assert_eq!(ext.get(&other), None);
    assert_eq!(
        ext.set(&other, vec![1]),
        Err(Error::Conflict("forge".into()))
    );
    assert_eq!(ext.remove(&other), None);
    assert_eq!(ext.get(&FORGE), Some(&forge[..]));
}

#[test]
fn the_digest_follows_the_records() {
    let a = filled();
    let b = filled();
    assert_eq!(a.digest(), b.digest());
    assert!(a.digest().is_some());

    let mut changed = filled();
    changed.set(&RULES, vec![1, 2, 4]).unwrap();
    assert_ne!(changed.digest(), a.digest());

    let mut fewer = filled();
    fewer.remove(&FORGE);
    assert_ne!(fewer.digest(), a.digest());

    let mut h = Fnv64::new();
    a.hash_into(&mut h);
    assert_eq!(Some(h.finish()), a.digest());
}

#[test]
fn a_record_survives_checkpoint_saves_and_reverts() {
    let mut s = State::new();
    s.ext.set(&RULES, b"first".to_vec()).unwrap();
    s.checkpoint_saved(0).unwrap();
    s.ext.set(&RULES, b"second".to_vec()).unwrap();
    s.checkpoint_saved(1).unwrap();
    s.ext.set(&RULES, b"live".to_vec()).unwrap();

    s.checkpoint_reverted(0).unwrap();
    assert_eq!(s.ext.get(&RULES), Some(&b"first"[..]));
    s.checkpoint_reverted(1).unwrap();
    assert_eq!(s.ext.get(&RULES), Some(&b"second"[..]));
    // A revert does not use up its slot.
    s.ext.set(&RULES, b"again".to_vec()).unwrap();
    s.checkpoint_reverted(1).unwrap();
    assert_eq!(s.ext.get(&RULES), Some(&b"second"[..]));

    assert_eq!(s.checkpoint_saved(2), Err(Error::Slot(2)));
    assert_eq!(s.checkpoint_reverted(u16::MAX), Err(Error::Slot(u16::MAX)));
    assert_eq!(
        s.ext.get(&RULES),
        Some(&b"second"[..]),
        "out of range changes nothing"
    );

    assert_eq!(s.full_restore(), 1);
    assert!(s.ext.is_empty());
}

#[test]
fn a_revert_to_an_unseen_slot_clears_and_says_so() {
    let mut s = State::new();
    s.ext.set(&FORGE, vec![1]).unwrap();
    s.checkpoint_saved(0).unwrap();
    assert_eq!(s.checkpoint_reverted(1), Err(Error::NoSlot(1)));
    assert!(s.ext.is_empty());
    s.checkpoint_reverted(0).unwrap();
    assert_eq!(s.ext.get(&FORGE), Some(&[1][..]));

    // With no records there is nothing to lose: no complaint.
    let mut unused = State::new();
    assert_eq!(unused.checkpoint_reverted(1), Ok(()));
}

#[test]
fn the_snapshot_section_carries_records_and_slots() {
    let mut s = State::new();
    assert!(s.save_section().is_empty(), "unused: no bytes");

    s.ext.set(&RULES, b"saved".to_vec()).unwrap();
    s.checkpoint_saved(1).unwrap();
    s.ext.set(&RULES, b"live".to_vec()).unwrap();
    let section = s.save_section();

    let mut loaded = State::new();
    loaded.load_section(&section).unwrap();
    assert_eq!(loaded, s);
    assert_eq!(loaded.ext.get(&RULES), Some(&b"live"[..]));
    assert_eq!(loaded.slot(0), None);
    // The checkpoint taken before the snapshot still reverts after it.
    loaded.checkpoint_reverted(1).unwrap();
    assert_eq!(loaded.ext.get(&RULES), Some(&b"saved"[..]));

    loaded.load_section(&[]).unwrap();
    assert_eq!(loaded, State::new());
}

#[test]
fn damaged_section_bytes_change_nothing() {
    let mut s = State::new();
    s.ext.set(&FORGE, vec![3; 3]).unwrap();
    s.checkpoint_saved(0).unwrap();
    let section = s.save_section();

    let mut target = State::new();
    target.ext.set(&RULES, vec![8]).unwrap();
    let before = target.clone();
    // Zero bytes is the valid empty section; every other prefix is short.
    for len in 1..section.len() {
        assert!(
            target.load_section(&section[..len]).is_err(),
            "prefix {len}"
        );
        assert_eq!(target, before);
    }
    let mut version = section.clone();
    version[0] = 2;
    assert!(matches!(
        target.load_section(&version),
        Err(Error::Section(_))
    ));
    let mut extra = section.clone();
    extra.push(0);
    assert_eq!(target.load_section(&extra), Err(Error::Trailing));
    let mut slot = section.clone();
    let last = slot.len() - 6; // inside slot 0's blob, before slot 1's flag
    slot[last] ^= 0x40;
    assert!(target.load_section(&slot).is_err());
    assert_eq!(target, before);
}

mod world_section {
    use crate::ext::checkpoint::global;
    use crate::ext::store::ExtStore;
    use crate::ext::{RULES, FORGE};
    use crate::{World, kind};

    /// The extension state is a world section: a save carries the live
    /// records and the slot copies, a restore brings them back, a refused
    /// restore changes nothing, and the sim hash moves with a record.
    /// One test, because the state is the process's.
    #[test]
    fn the_world_saves_hashes_and_restores_the_extension_records() {
        *global() = super::State::new();
        let mut w = World::new(1);
        w.attach(Box::new(ExtStore)).unwrap();
        assert_eq!(w.store(kind::EXT).unwrap().encode(), Vec::<u8>::new());
        let empty = w.sim_hash();

        {
            let mut g = global();
            g.ext.set(&RULES, vec![1, 2, 3]).unwrap();
            g.checkpoint_saved(0).unwrap();
            g.ext.set(&FORGE, vec![7]).unwrap();
        }
        assert_ne!(w.sim_hash(), empty);
        let saved = w.save();
        let at_save = w.sim_hash();

        *global() = super::State::new();
        assert_eq!(w.sim_hash(), empty);
        w.restore_into(&saved).unwrap();
        assert_eq!(w.sim_hash(), at_save);
        assert_eq!(w.save(), saved);
        assert_eq!(global().ext.get(&FORGE), Some(&[7][..]));
        global().checkpoint_reverted(0).unwrap();
        assert_eq!(global().ext.get(&FORGE), None);
        assert_eq!(global().ext.get(&RULES), Some(&[1, 2, 3][..]));

        // A section the registered kinds refuse is not decoded.
        use crate::Store;
        assert!(ExtStore.decode(1, b"\x01junk").is_err());
        assert!(ExtStore.decode(2, &[]).is_err());

        *global() = super::State::new();
    }
}
