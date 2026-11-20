// Full path and barrel number in failures distinguish shared projectiles and vehicle guns.
use super::*;

fn rows<'a>(r: &'a Record, field: &str) -> &'a [Record] {
    match r.at(field) {
        Some(Val::Block(rows)) => rows,
        value => panic!("{field}: {value:?}"),
    }
}

fn bytes(d: &StructDef, r: &Record) -> Vec<u8> {
    defs::to_built(d, r, &mut |_| None).unwrap().bytes
}

fn words(label: &str, dst: &[u8], src: &[u8], positions: &[(usize, usize)]) {
    for &(to, from) in positions {
        assert_eq!(
            u32_at(dst, to),
            u32_at(src, from),
            "{label}: H2 +{to:x}, H1 +{from:x}"
        );
    }
}

fn same_target(
    map: &H1Map,
    tree: &Tree,
    label: &str,
    r: &Record,
    field: &str,
    id: Option<h1map::TagId>,
) {
    let expected = id.map(|id| map.tag(id).unwrap().name.as_str());
    let actual = reference(r, field).map(|id| {
        assert!(local(tree, id).is_some(), "{label}: {field} is not local");
        tree.tags[(id.0 & 0xffff) as usize].0.as_str()
    });
    assert_eq!(actual, expected, "{label}: {field}");
}

// Explosives can name both a vitality effect and a separate shock wave.
// Check the selected damage against its own source, including its bounds.
fn damage_from(
    map: &H1Map,
    tree: &Tree,
    label: &str,
    r: &Record,
    field: &str,
    candidates: &[h1map::TagId],
) {
    let Some(id) = reference(r, field) else {
        assert!(candidates.is_empty(), "{label}: missing {field}");
        return;
    };
    let target = local(tree, id).unwrap();
    let name = &tree.tags[(id.0 & 0xffff) as usize].0;
    let source = candidates
        .iter()
        .map(|id| map.tag(*id).unwrap())
        .find(|tag| tag.name == *name)
        .unwrap_or_else(|| panic!("{label}: unexpected {field} {name}"));
    words(
        label,
        &target.root.bytes,
        map.tag_data(source).unwrap(),
        &[(0x1c, 0x1d0), (0x20, 0x1d4), (0x24, 0x1d8)],
    );
}

fn event_damage(map: &H1Map, source: Option<h1map::TagId>) -> Vec<h1map::TagId> {
    let Some(id) = source else { return Vec::new() };
    h1::Effect::read(map, map.tag(id).unwrap())
        .unwrap()
        .events
        .into_iter()
        .flat_map(|e| e.parts)
        .filter(|p| p.group == h1map::Group::new(b"jpt!"))
        .filter_map(|p| p.reference)
        .collect()
}

pub(super) fn compare(map: &H1Map, tag: &h1map::Tag, tree: &Tree, rebuilt: &Record) {
    let src = map.tag_data(tag).unwrap();
    let name = format!("{} {}", map.header().name, tag.name);
    let d = def(b"weap");
    let dst = bytes(d, rebuilt);
    words(
        &name,
        &dst,
        src,
        &[
            (0x13c, 0x338),
            (0x150, 0x34c),
            (0x154, 0x350),
            (0x158, 0x354),
            (0x15c, 0x358),
            (0x160, 0x35c),
            (0x164, 0x360),
            (0x208, 0x3e4),
            (0x20c, 0x3e8),
            (0x210, 0x3ec),
            (0x214, 0x3f0),
            (0x218, 0x3f4),
            (0x254, 0x440),
            (0x258, 0x444),
            (0x25c, 0x448),
            (0x260, 0x44c),
        ],
    );
    let zoom = i16_at(src, 0x3da).max(0);
    assert_eq!(i16_at(&dst, 0x1fe), zoom, "{name}: zoom levels");
    if zoom > 0 {
        words(&name, &dst, src, &[(0x200, 0x3dc), (0x204, 0x3e0)]);
    }
    let kind = i16_at(src, 0x4e2);
    assert_eq!(i16_at(&dst, 0x292), kind, "{name}: weapon type");
    let magazines = map
        .block(h1map::Block::read(src, 0x4f0).unwrap(), 0x70)
        .unwrap();
    assert_eq!(
        rows(rebuilt, "magazines").len(),
        magazines.len() / 0x70,
        "{name}"
    );
    for (r, m) in rows(rebuilt, "magazines")
        .iter()
        .zip(magazines.chunks_exact(0x70))
    {
        let h = bytes(d.child("magazines").unwrap(), r);
        assert_eq!(&h[..12], &m[..12], "{name}: magazine flags and capacities");
        words(&name, &h, m, &[(0x10, 0x14), (0x18, 0x1c)]);
        assert_eq!(i16_at(&h, 0x14), i16_at(m, 0x18), "{name}: reload count");
    }
    let raw = map
        .block(h1map::Block::read(src, 0x4fc).unwrap(), 0x114)
        .unwrap();
    let barrels = rows(rebuilt, "barrels");
    let triggers = rows(rebuilt, "block_2c8");
    let charging = !raw.is_empty() && f32_at(raw, 0x48) > 0.0;
    assert_eq!(barrels.len(), raw.len() / 0x114, "{name}: barrels");
    assert_eq!(
        triggers.len(),
        if charging { 1 } else { barrels.len() },
        "{name}: inputs"
    );
    for (i, (r, t)) in barrels.iter().zip(raw.chunks_exact(0x114)).enumerate() {
        let label = format!("{name} barrel {i}");
        let flags = u32_at(t, 0);
        let semi = (kind == 1 && i == 0) || flags & 8 != 0;
        let h = bytes(d.child("barrels").unwrap(), r);
        let bits = [(2, 2), (5, 3), (10, 5), (11, 6), (12, 7)];
        let expected_flags = bits
            .iter()
            .fold(0u32, |v, &(a, b)| v | (((flags >> a) & 1) << b));
        assert_eq!(u32_at(&h, 0), expected_flags, "{label}: flags");
        words(
            &label,
            &h,
            t,
            &[
                (4, 4),
                (8, 8),
                (12, 12),
                (16, 16),
                (0x18, 0x14),
                (0x28, 0x20),
                (0x2c, 0x24),
                (0x38, 0x38),
                (0x3c, 0x3c),
                (0x48, 0x38),
                (0x4c, 0x3c),
                (0x58, 0x78),
                (0x5c, 0x7c),
                (0x60, 0x80),
                (0x68, 0x6c),
                (0x6c, 0x70),
                (0x70, 0x78),
                (0x74, 0x7c),
                (0x78, 0x80),
                (0x7c, 0x84),
                (0x80, 0x88),
                (0x84, 0x8c),
                (0x9c, 0xa4),
                (0xa0, 0xa8),
                (0xa4, 0xb8),
                (0xa8, 0xbc),
            ],
        );
        for (to, from) in [
            (0x50, 0x38),
            (0x54, 0x3c),
            (0xcc, 0xa8),
            (0xd0, 0xa4),
            (0xd4, 0xc),
            (0xd8, 0x10),
            (0xdc, 0x38),
            (0xe0, 0x3c),
        ] {
            let time = f32_at(t, from);
            assert_eq!(
                f32_at(&h, to),
                if time > 0.0 { 1.0 / time } else { 1000.0 },
                "{label}: inverse +{to:x}"
            );
        }
        let shots = i16::from(semi || charging);
        assert_eq!(
            (i16_at(&h, 0x1c), i16_at(&h, 0x1e)),
            (shots, shots),
            "{label}: shots per fire"
        );
        let rate = f32_at(t, 4);
        assert_eq!(
            f32_at(&h, 0x20),
            if shots == 1 && rate > 0.0 {
                1.0 / rate
            } else {
                0.0
            },
            "{label}: recovery"
        );
        assert_eq!(
            i16_at(&h, 0x34),
            if charging {
                0
            } else if semi {
                2
            } else {
                1
            },
            "{label}: prediction"
        );
        assert_eq!(u32_at(&h, 0x98), u32::MAX, "{label}: direct damage");
        if !charging || i == 0 {
            let tr = bytes(d.child("block_2c8").unwrap(), &triggers[i]);
            assert_eq!(
                (i16_at(&tr, 4), i16_at(&tr, 6)),
                (i as i16, if charging { 2 } else { i16::from(semi) }),
                "{label}: trigger"
            );
            assert_eq!(
                (i16_at(&tr, 8), i16_at(&tr, 10)),
                (
                    i as i16,
                    if charging {
                        (barrels.len() - 1) as i16
                    } else {
                        -1
                    }
                ),
                "{label}: barrels"
            );
            if charging {
                words(
                    &label,
                    &tr,
                    t,
                    &[(0x1c, 0x48), (0x20, 0x4c), (0x28, 0x54), (0x2c, 0x58)],
                );
                assert_eq!(
                    i16_at(&tr, 0x24),
                    i16_at(t, 0x50),
                    "{label}: overcharge action"
                );
                assert_eq!(i16_at(&tr, 0x18), 1, "{label}: charging action");
            }
        }
        same_target(
            map,
            tree,
            &label,
            r,
            "projectile",
            h1::reference(map, t, 0x94),
        );
        let fx = map
            .block(h1map::Block::read(t, 0x108).unwrap(), 0x84)
            .unwrap();
        let effects = rows(r, "block_e4");
        assert_eq!(effects.len(), fx.len() / 0x84, "{label}: firing feedback");
        for (r, src) in effects.iter().zip(fx.chunks_exact(0x84)) {
            let rebuilt = bytes(d.child("barrels.block_e4").unwrap(), r);
            words(&label, &rebuilt, src, &[(0, 0)]);
            for (field, offset) in arsenal::FIRING_DAMAGE_REFS.iter().zip([0x54, 0x64, 0x74]) {
                let id = h1::reference(map, src, offset);
                same_target(map, tree, &label, r, field, id);
                if let Some(id) = id {
                    let h1 = map.tag_data(map.tag(id).unwrap()).unwrap();
                    let h2 = &local(tree, reference(r, field).unwrap())
                        .unwrap()
                        .root
                        .bytes;
                    words(
                        &label,
                        h2,
                        h1,
                        &[
                            (0x74, 0x98),
                            (0x78, 0x9c),
                            (0x7c, 0xa0),
                            (0x80, 0xa4),
                            (0x84, 0xb8),
                            (0x88, 0xbc),
                            (0x8c, 0xcc),
                            (0x90, 0xd0),
                            (0x94, 0xd4),
                            (0x98, 0xd8),
                            (0x9c, 0xe8),
                            (0xa4, 0xf0),
                        ],
                    );
                    assert_eq!(
                        f32_at(h2, 0xa0),
                        f32_at(h1, 0xec) / 30.0,
                        "{label}: shake period"
                    );
                }
            }
        }
        if let Some(id) = h1::reference(map, t, 0x94) {
            let src = map.tag_data(map.tag(id).unwrap()).unwrap();
            let proj = &local(tree, reference(r, "projectile").unwrap())
                .unwrap()
                .root;
            let h = &proj.bytes;
            words(
                &label,
                h,
                src,
                &[
                    (0xe0, 0x1c8),
                    (0x168, 0x1d0),
                    (0x16c, 0x1d4),
                    (0x174, 0x1dc),
                    (0x178, 0x1e0),
                ],
            );
            for (to, from) in [
                (0xdc, 0x1c4),
                (0x17c, 0x1e4),
                (0x180, 0x1e8),
                (0x184, 0x1ec),
                (0x188, 0x1ec),
            ] {
                assert_eq!(
                    f32_at(h, to),
                    30.0 * f32_at(src, from),
                    "{label}: projectile +{to:x}"
                );
            }
            if f32_at(src, 0x1e4) != f32_at(src, 0x1e8) {
                let span = f32_at(src, 0x1d4) - f32_at(src, 0x1d0);
                if span > 0.0 {
                    words(&label, h, src, &[(0x18c, 0x1d0), (0x190, 0x1d4)]);
                    assert_eq!(
                        f32_at(h, 0x194),
                        span.recip(),
                        "{label}: distance reciprocal"
                    );
                }
                assert_eq!(
                    u32_at(h, 0x198),
                    0x8000_0000,
                    "{label}: per-medium velocity curve"
                );
            } else {
                assert_eq!(u32_at(h, 0x198), 0, "{label}: constant velocity");
            }
            let pr = defs::from_built(def(b"proj"), proj).unwrap();
            let impact = h1::reference(map, src, 0x224);
            damage_from(map, tree, &label, &pr, "impact_damage", impact.as_slice());
            let normal = event_damage(map, h1::reference(map, src, 0x1ac));
            let attached = h1::reference(map, src, 0x214);
            // The attached slot is independent of the free detonation.
            // Older conversions also used it for a missing free detonation.
            if !normal.is_empty() || attached.is_none() {
                damage_from(map, tree, &label, &pr, "detonation_damage", &normal);
            }
            damage_from(map, tree, &label, &pr, "ref_108", attached.as_slice());
            let super_damage = event_damage(map, h1::reference(map, src, 0x18c));
            damage_from(
                map,
                tree,
                &label,
                &pr,
                "super_detonation_damage",
                &super_damage,
            );
        }
    }
}

#[test]
#[ignore = "reads Blood Gulch and Coagulation from H1_ISO and H2_DISC"]
fn blood_gulch_shotgun_and_ar_firing_profiles() {
    let h1 = h1_level("bloodgulch").unwrap();
    let base = Base::read(&mut h2_map("coagulation").unwrap()).unwrap();
    let mut c = Converter::new(&h1, &base);
    let d = def(b"weap");
    for (weapon, native, semi, pellets, capacity, reload, rate) in [
        ("shotgun", "shotgun", 1, 15, 12, 0.4, 1.0),
        ("assault rifle", "smg", 0, 1, 60, 3.4, 15.0),
    ] {
        let name = format!(r"weapons\{weapon}\{weapon}");
        let t = h1.find(h1map::Group::new(b"weap"), &name).unwrap();
        let id = c.object(t).unwrap();
        let r = defs::from_built(d, &local(&c.tree, id).unwrap().root).unwrap();
        compare(&h1, t, &c.tree, &r);
        let b = bytes(d.child("barrels").unwrap(), &rows(&r, "barrels")[0]);
        let m = &rows(&r, "magazines")[0];
        let tr = bytes(d.child("block_2c8").unwrap(), &rows(&r, "block_2c8")[0]);
        assert_eq!(
            (i16_at(&tr, 6), i16_at(&b, 0x6a), i16_at(&b, 0x2a)),
            (semi, pellets, 1),
            "{name}"
        );
        assert_eq!((f32_at(&b, 4), f32_at(&b, 8)), (rate, rate), "{name}");
        assert_eq!(
            (int(m, "rounds_loaded_maximum"), float(m, "reload_time")),
            (capacity, reload),
            "{name}"
        );
        let h2 = base
            .find(
                b"weap",
                &format!(r"objects\weapons\rifle\{native}\{native}"),
            )
            .unwrap();
        let native_root = base.head(h2, d.size.into()).unwrap();
        let native_barrel = base.element(&native_root, 0x2d0, 0xec, 0).unwrap().unwrap();
        let native_trigger = base.element(&native_root, 0x2c8, 0x40, 0).unwrap().unwrap();
        assert_eq!(i16_at(&native_trigger, 6), semi, "native {native}");
        assert_eq!(
            i16_at(&native_barrel, 0x34),
            i16_at(&b, 0x34),
            "native {native} prediction"
        );
        if semi == 1 {
            assert_eq!(i16_at(&native_barrel, 0x6a), 10, "native shotgun");
            assert_eq!(int(m, "rounds_reloaded"), 1);
            assert!((f32_at(&b, 0x74).to_degrees() - 10.0).abs() < 0.001);
            assert_eq!(f32_at(&b, 0x74), f32_at(&b, 0x78));
        } else {
            assert_eq!((f32_at(&b, 0xc), f32_at(&b, 0x10)), (1.0, 0.1));
            assert_eq!((f32_at(&b, 0x38), f32_at(&b, 0x3c)), (0.6, 1.0));
            assert!((f32_at(&b, 0x74).to_degrees() - 2.0).abs() < 0.001);
            assert!((f32_at(&b, 0x78).to_degrees() - 6.5).abs() < 0.001);
        }
    }
}
