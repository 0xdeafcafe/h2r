//! A Halo 1 multiplayer map converted **fresh** (nothing of its Halo 2
//! base pinned), from the owner's discs (`$H1_ISO`, `$H2_DISC`), checked
//! against the pinned conversion of the same map. Nothing is written to
//! disk; counts go to stdout. `H1_MAPS=a,b` picks maps (default
//! `bloodgulch`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::{Sandbox, known, prepare, structure_mopp};
use h1map::H1Map;
use map::{FourCC, Map, TagId};
use tags::localization::matg_locale_raw_len;
use tags::schema::{Field, Memory, Schema, Struct, visit};

type M = Map<Cursor<Vec<u8>>>;

fn h2_map(iso: &mut xiso::Xiso<BufReader<File>>, name: &str) -> M {
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap_or_else(|| panic!("no {name}.map on the Halo 2 disc"));
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn word(b: &[u8], at: u32) -> u32 {
    u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().unwrap())
}

/// Every datum of `m`, as its tag's group and name.
fn names(m: &M) -> HashMap<u32, String> {
    m.tags()
        .iter()
        .map(|t| {
            (
                t.id.0,
                format!("{} {}", t.group, t.name.as_deref().unwrap_or("?")),
            )
        })
        .collect()
}

/// `m`'s tags by group and name.
fn by_key(m: &M) -> HashMap<(FourCC, String), TagId> {
    m.tags()
        .iter()
        .map(|t| ((t.group, t.name.clone().unwrap_or_default()), t.id))
        .collect()
}

/// One element in address-free and index-free form: block addresses
/// zeroed, every word that is a datum of the map as its tag's name, each
/// local raw pointer as the bytes it names (the locale descriptor length,
/// its paired size, or 16 bytes), external ones as they are.
fn element(
    s: &Struct,
    e: &[u8],
    file: &[u8],
    ids: &HashMap<u32, String>,
    matg_root: bool,
) -> String {
    let mut bytes = e.to_vec();
    let mut raws = Vec::new();
    let mut tags = Vec::new();
    for k in (0..e.len() / 4 * 4).step_by(4) {
        if let Some(n) = ids.get(&word(e, k as u32)) {
            tags.push(format!("{k:#x} {n}"));
            bytes[k..k + 4].fill(0);
        }
    }
    for (&o, f) in &s.fields {
        match f {
            Field::Block(_) => bytes[o as usize + 4..o as usize + 8].fill(0),
            Field::Raw => {
                let v = word(e, o);
                bytes[o as usize..o as usize + 4].fill(0);
                if v == u32::MAX || v == 0 {
                    raws.push("null".to_owned());
                } else if matg_root && let Some(n) = matg_locale_raw_len(e, o) {
                    let at = v as usize;
                    let n = usize::try_from(n).expect("locale resource length");
                    raws.push(format!("{:02x?}", &file[at..at + n]));
                } else if v >> 30 != 0 {
                    raws.push(format!("{v:#x}"));
                } else {
                    let n = s
                        .fields
                        .iter()
                        .find_map(|(&k, f)| {
                            matches!(f, Field::RawSize(r) if *r == o).then(|| word(e, k))
                        })
                        .unwrap_or(16) as usize;
                    let at = v as usize;
                    raws.push(format!("{:02x?}", file.get(at..at + n)));
                }
            }
            _ => {}
        }
    }
    format!("{bytes:02x?} {raws:?} {tags:?}")
}

/// The tag's elements in visit order, address-free.
fn canonical(
    group: FourCC,
    s: &Struct,
    address: u32,
    mem: &Memory,
    file: &[u8],
    ids: &HashMap<u32, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    visit(s, address, mem, &mut |s, e, path| {
        let matg_root = group == FourCC(*b"matg") && path.is_empty();
        out.push(format!("{path:x?} {}", element(s, e, file, ids, matg_root)));
    })
    .expect("visit");
    out
}

/// Every tag reference and datum of every ordinary tag must name a tag of
/// the map, of the referenced group (or a descendant), with data (BSP
/// tags excepted). Returns (references checked, problems).
fn reference_sweep(m: &M, mem: &Memory) -> (usize, Vec<String>) {
    let schema = Schema::builtin();
    let descends = |g: FourCC, of: FourCC| {
        g == of
            || m.groups()
                .iter()
                .any(|x| x.class == g && (x.parent == Some(of) || x.grandparent == Some(of)))
    };
    let in_bsp = |g: FourCC| g == FourCC(*b"sbsp") || g == FourCC(*b"ltmp");
    let (mut n, mut bad) = (0, Vec::new());
    for t in m.tags() {
        let (Some(l), Some(s)) = (t.location, schema.groups.get(&t.group)) else {
            continue;
        };
        if mem.region(l.address).is_none_or(|r| r.0 != l.address) {
            continue;
        }
        let label = format!("{} {}", t.group, t.name.as_deref().unwrap_or("?"));
        visit(s, l.address, mem, &mut |s, e, _| {
            for (&o, f) in &s.fields {
                let (g, d) = match f {
                    // Retail has references with a null group and a real
                    // datum (two `mode`s naming a `shad`): no group check.
                    Field::TagRef => (
                        Some(word(e, o))
                            .filter(|&g| g != u32::MAX)
                            .map(|g| FourCC(g.to_be_bytes())),
                        word(e, o + 4),
                    ),
                    Field::TagId => (None, word(e, o)),
                    _ => continue,
                };
                if d == u32::MAX {
                    continue;
                }
                n += 1;
                match m.tag(TagId(d)) {
                    Err(_) => bad.push(format!("{label}: {d:#x} names no tag")),
                    Ok(r) if g.is_some_and(|g| !descends(r.group, g)) => {
                        bad.push(format!("{label}: a {:?} reference names a {}", g, r.group))
                    }
                    Ok(r) if r.location.is_none() && !in_bsp(r.group) => bad.push(format!(
                        "{label}: {} {} has no data",
                        r.group,
                        r.name.as_deref().unwrap_or("?")
                    )),
                    Ok(_) => {}
                }
            }
        })
        .unwrap_or_else(|| bad.push(format!("{label}: does not walk")));
    }
    (n, bad)
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn a_multiplayer_map_converts_fresh_with_nothing_pinned() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let h2_iso = std::env::var("H2_DISC").expect("set H2_DISC");
    let maps: Vec<String> = std::env::var("H1_MAPS").map_or_else(
        |_| vec!["bloodgulch".to_owned()],
        |s| s.split(',').map(str::to_owned).collect(),
    );
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut h2 = xiso::Xiso::open(BufReader::new(File::open(&h2_iso).unwrap())).unwrap();
    let schema = Schema::builtin();
    for name in maps {
        let started = std::time::Instant::now();
        let Some(e) = disc.map(&name).cloned() else {
            println!("{name}: not on this Halo 1 disc");
            continue;
        };
        let file = iso.read(&e).unwrap();
        let sha = content::Sha256::of(&file);
        let h1 = H1Map::from_reader(Cursor::new(file)).unwrap();
        let base_name = known(&name).unwrap().1;
        let mut base = h2_map(&mut h2, base_name);
        let p = prepare(&h1, sha, &mut base, Sandbox::Halo2).unwrap();
        let mopp = structure_mopp(&p.collision).unwrap();
        let pinned = p.build(&mut base, &mopp).unwrap().bytes;
        let fresh = p.build_fresh(&mut base, &mopp).unwrap();
        let r = &fresh.report;
        println!(
            "{name} onto {base_name}: fresh {} bytes (pinned {}); {} tags unpinned ({} aliases), \
             {} raw resources ({} bytes) copied, {} external kept, {} sound chunks; \
             scenario blocks copied {}, left empty {:x?}",
            fresh.bytes.len(),
            pinned.len(),
            r.unpinned,
            r.aliases,
            r.raw.0,
            r.raw.1,
            r.external,
            r.sound_chunks,
            r.scenario_blocks.len(),
            r.scenario_skipped
        );

        let mut m = Map::from_reader(Cursor::new(pinned.clone())).unwrap();
        let mut f = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        f.validate_checksum().unwrap();
        // The base's tags nothing reaches are dropped (fresh::prune).
        let pruned = &r.pruned;
        println!(
            "  pruned {} base tags {:?}; {} palette entries nulled, {} tag list entries dropped",
            pruned.tags(),
            pruned.groups,
            pruned.palette_entries,
            pruned.resource_entries
        );
        // Most go from the pinned map already (fresh::prune_pinned).
        let early = &r.pinned_pruned;
        println!(
            "  pinned map: {} base tags dropped {:?} ({:#x} bytes freed, {} kept for a pointer into them); {} palette entries nulled, {} tag list entries dropped",
            early.tags(),
            early.groups,
            early.freed,
            early.kept_by_pointer,
            early.palette_entries,
            early.resource_entries
        );
        assert!(
            pruned.tags() + early.tags() > 0,
            "nothing of the base pruned"
        );
        let mm = Memory::of(&mut m).unwrap();
        let fm = Memory::of(&mut f).unwrap();
        let (mids, fids) = (names(&m), names(&f));

        // Built from scratch (fresh::from_scratch): a dense index of its
        // own, every tag one of the pinned map's by group and name, with
        // data where it had; every converted tag kept.
        let s = r.scratch.as_ref().expect("built from scratch");
        println!(
            "  from scratch: {} tags ({} retail, {} converted, {} scenario and BSP; {} datum words \
             rewritten); index {} slots",
            s.tags,
            s.from_base,
            s.from_level,
            s.built_here,
            s.words,
            f.index_slots()
        );
        assert_eq!(f.index_slots() as usize, f.tags().len(), "index holes");
        assert!(f.tags().len() < m.tags().len());
        let (mk, fk) = (by_key(&m), by_key(&f));
        let fid = |t: &map::Tag| {
            fk.get(&(t.group, t.name.clone().unwrap_or_default()))
                .copied()
        };
        let mid = |t: &map::Tag| mk[&(t.group, t.name.clone().unwrap_or_default())];
        for b in f.tags() {
            let a = m.tag(mid(b)).unwrap();
            assert_eq!(a.location.is_some(), b.location.is_some(), "{:?}", a.name);
        }
        let first = base.index_slots() as usize;
        for a in m.tags().iter().filter(|t| t.id.index() >= first) {
            assert!(fid(a).is_some(), "{:?} pruned", a.name);
        }

        // Every reference resolves.
        let (n, bad) = reference_sweep(&f, &fm);
        assert!(
            bad.is_empty(),
            "{} problems: {:#?}",
            bad.len(),
            &bad[..bad.len().min(20)]
        );
        let (pn, _) = reference_sweep(&m, &mm);

        // Every tag the same, address-free, raw data by its bytes; the
        // scenario block by block.
        let (mut same, mut by_group) = (0, BTreeMap::<String, u32>::new());
        // Sounds' indices are the cut gestalt's: compared below.
        let skip: HashSet<[u8; 4]> = [*b"sbsp", *b"ltmp", *b"ugh!", *b"scnr", *b"snd!"].into();
        for t in m.tags().to_vec() {
            let (Some(a), Some(s)) = (t.location, schema.groups.get(&t.group)) else {
                continue;
            };
            if skip.contains(&t.group.0)
                || mm.region(a.address).is_none_or(|r| r.0 != a.address)
                || fid(&t).is_none()
            {
                continue;
            }
            let b = f.tag(fid(&t).unwrap()).unwrap().location.unwrap();
            let x = canonical(t.group, s, a.address, &mm, &pinned, &mids);
            let y = canonical(t.group, s, b.address, &fm, &fresh.bytes, &fids);
            if x != y {
                let i = x.iter().zip(&y).position(|(p, q)| p != q).unwrap_or(0);
                panic!(
                    "{} {:?} differs at element {i}:\n{:?}\n{:?}",
                    t.group,
                    t.name,
                    x.get(i),
                    y.get(i)
                );
            }
            same += 1;
            *by_group.entry(t.group.to_string()).or_default() += 1;
        }
        let scnr = m.scenario().unwrap().clone();
        let root = &schema.groups[&FourCC(*b"scnr")];
        let (sa, sb) = (
            scnr.location.unwrap().address,
            f.scenario().unwrap().location.unwrap().address,
        );
        let (rx, ry) = (
            m.read_tag_bytes(scnr.id).unwrap(),
            f.read_tag_bytes(f.scenario().unwrap().id).unwrap(),
        );
        let pruned_blocks: Vec<u32> = h1convert::fresh::BASE_PALETTES
            .iter()
            .map(|p| p.0)
            .chain([h1convert::fresh::SCENARIO_RESOURCES])
            .collect();
        for &(o, n) in r
            .scenario_blocks
            .iter()
            .filter(|b| !pruned_blocks.contains(&b.0))
        {
            let Some(Field::Block(c)) = root.fields.get(&o) else {
                unreachable!()
            };
            if r.scenario_flat.contains(&o) {
                // Grown by the editor, outside every tag: compare bytes.
                let len = n * c.size;
                let read = |map: &mut M, b: &[u8]| {
                    let off = map.file_offset(word(b, o + 4), len).unwrap();
                    map.read_file_range(off, len.into()).unwrap()
                };
                assert_eq!(
                    read(&mut m, &rx),
                    read(&mut f, &ry),
                    "scenario block {o:#x}"
                );
                continue;
            }
            let only = Struct {
                size: root.size,
                align: root.align,
                fields: [(o, Field::Block(c.clone()))].into(),
            };
            let x = canonical(scnr.group, &only, sa, &mm, &pinned, &mids);
            let y = canonical(scnr.group, &only, sb, &fm, &fresh.bytes, &fids);
            if o == 0x210 {
                // The descriptor's region words name each map's own region.
                assert_eq!(x.len(), y.len());
                continue;
            }
            assert_eq!(x[1..], y[1..], "scenario block {o:#x}");
        }
        // The scenario root's scalars and references.
        let mut differ = Vec::new();
        for k in (0..root.size).step_by(4) {
            let in_block = root
                .fields
                .iter()
                .any(|(&o, f)| matches!(f, Field::Block(_)) && (o..o + 8).contains(&k));
            let (a, b) = (word(&rx, k), word(&ry, k));
            let same = a == b || mids.get(&a).is_some_and(|n| fids.get(&b) == Some(n));
            if !in_block && !same {
                differ.push(k);
            }
        }
        assert!(
            differ.is_empty(),
            "scenario root words differ at {differ:x?}"
        );

        // The sound gestalt, cut to the kept sounds (compact::gestalt): every
        // kept sound names the same entries, names and chunk data as in the
        // pinned map.
        use h1sounds::gestalt::SoundGestalt;
        let gestalt = |map: &mut M| {
            let t = map
                .tags()
                .iter()
                .find(|t| t.group == FourCC(*b"ugh!"))
                .unwrap()
                .clone();
            SoundGestalt::decode(
                &map.read_tag_bytes(t.id).unwrap(),
                t.location.unwrap().address,
            )
            .unwrap()
        };
        let gb = gestalt(&mut f);
        let owner = f.tags().iter().find(|t| t.group == FourCC(*b"ugh!")).unwrap().id;
        for info in &gb.nested[h1sounds::gestalt::blocks::EXTRA_INFOS] {
            let stored = u32::from_le_bytes(info.bytes[0x20..0x24].try_into().unwrap());
            assert_eq!(stored, owner.0, "compacted extra-info resource owner");
        }
        let c = s
            .gestalt
            .as_ref()
            .unwrap_or_else(|| panic!("gestalt kept whole: {:?}", s.gestalt_whole));
        assert!(c.chunks.1 <= c.chunks.0);
        let before = h1convert::compact::map_entries(&mut m, &pinned).unwrap();
        let after = h1convert::compact::map_entries(&mut f, &fresh.bytes).unwrap();
        let mut sounds = 0;
        for (n, y) in &after {
            assert_eq!(before.get(n), Some(y), "{n}");
            sounds += 1;
        }
        assert_eq!(sounds, c.sounds);
        println!(
            "  gestalt cut to {sounds} sounds: {} -> {} bytes, {} -> {} chunks; {} raw ({} bytes) \
             dropped; every sound's entries and chunk data the same",
            c.bytes.0, c.bytes.1, c.chunks.0, c.chunks.1, c.raw_dropped.0, c.raw_dropped.1
        );

        // Bitmaps decode the same: every level of detail and low-detail
        // range holds the same bytes.
        let mut bitmaps = 0;
        // Ranges are file offsets (low-detail ones already past the
        // region's start).
        let range = |r: &tags::ResourceRange, file: &[u8]| {
            if r.pointer.file != map::ResourceFile::Local {
                return Some(format!("{:?}", r.pointer));
            }
            let at = r.pointer.offset as usize;
            file.get(at..at + r.size as usize)
                .map(|b| format!("{b:02x?}"))
        };
        for t in m
            .tags()
            .to_vec()
            .iter()
            .filter(|t| t.group == FourCC(*b"bitm") && fid(t).is_some())
        {
            let x = tags::DecodedTag::read(&mut m, t.id).unwrap();
            let y = tags::DecodedTag::read(&mut f, fid(t).unwrap()).unwrap();
            let (tags::TagData::Bitmap(x), tags::TagData::Bitmap(y)) = (&x.data, &y.data) else {
                unreachable!()
            };
            assert_eq!(x.images.len(), y.images.len());
            for (p, q) in x.images.iter().zip(&y.images) {
                for (a, b) in p.lods.iter().zip(&q.lods) {
                    assert_eq!(
                        a.as_ref().map(|a| range(a, &pinned)),
                        b.as_ref().map(|b| range(b, &fresh.bytes)),
                        "{:?} level of detail",
                        t.name
                    );
                }
                assert_eq!(
                    p.low_detail.as_ref().map(|a| range(a, &pinned)),
                    q.low_detail.as_ref().map(|b| range(b, &fresh.bytes)),
                    "{:?} low detail",
                    t.name
                );
            }
            bitmaps += 1;
        }

        // Every language's string table (matg's locale data, raw) the same.
        let (lm, lf) = (
            tags::Localization::read(&mut m).unwrap(),
            tags::Localization::read(&mut f).unwrap(),
        );
        for lang in tags::Language::ALL {
            assert_eq!(
                format!("{:?}", lm.table(lang)),
                format!("{:?}", lf.table(lang)),
                "{lang:?} strings"
            );
        }

        // The structure: the scenario's descriptor names the fresh map's
        // region, which holds the converted BSP.
        let sbsp = f
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"sbsp"))
            .unwrap();
        let (x, y) = (
            m.read_tag_bytes(mid(sbsp)).unwrap(),
            f.read_tag_bytes(sbsp.id).unwrap(),
        );
        assert_eq!(x.len(), y.len(), "structure size");

        println!(
            "  read back: {} tags, {n} references resolve (pinned map: {pn}); \
             {same} tags identical address-free ({by_group:?}); scenario: {} blocks identical, \
             root identical; sound gestalt cut ({} chunks); {bitmaps} bitmaps decode \
             identically; 9 languages' strings identical; structure {} bytes ({:?})",
            f.tags().len(),
            r.scenario_blocks.len(),
            gb.chunks.len(),
            y.len(),
            started.elapsed()
        );
    }
}

/// Every sound of a fresh multiplayer conversion (`H1_MAPS`, default
/// bloodgulch; the Halo 2 sandbox, as `h2 play` imports it) read back
/// through the gestalt as the engine plays it (`h1sounds::audit`): Halo
/// 1's background sounds hold exactly their Halo 1 samples, and every
/// other sound is byte for byte the base's of the same name.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_multiplayer_sound_plays_what_it_was_made_from() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let h2_iso = std::env::var("H2_DISC").expect("set H2_DISC");
    let maps: Vec<String> = std::env::var("H1_MAPS").map_or_else(
        |_| vec!["bloodgulch".to_owned()],
        |s| s.split(',').map(str::to_owned).collect(),
    );
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut h2 = xiso::Xiso::open(BufReader::new(File::open(&h2_iso).unwrap())).unwrap();
    let mut failed = Vec::new();
    for name in maps {
        let Some(e) = disc.map(&name).cloned() else {
            println!("{name}: not on this Halo 1 disc");
            continue;
        };
        let file = iso.read(&e).unwrap();
        let sha = content::Sha256::of(&file);
        let h1 = H1Map::from_reader(Cursor::new(file)).unwrap();
        let base_name = known(&name).unwrap().1;
        let e = h2
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{base_name}.map")))
            .unwrap();
        let base_bytes = h2.read(&e).unwrap();
        let mut base = Map::from_reader(Cursor::new(base_bytes.clone())).unwrap();
        let p = prepare(&h1, sha, &mut base, Sandbox::Halo2).unwrap();
        let mopp = structure_mopp(&p.collision).unwrap();
        let fresh = p.build_fresh(&mut base, &mopp).unwrap();
        let a =
            h1sounds::audit::audit(&fresh.bytes, Some(&h1), &[(base_name, &base_bytes)]).unwrap();
        println!(
            "{name} on {base_name}: {} sounds, {} from Halo 1 ({:.1} s), {} retail, {} permutations; \
             {} unmatched {:?}",
            a.sounds,
            a.converted,
            a.converted_seconds,
            a.retail,
            a.permutations,
            a.unmatched.len(),
            &a.unmatched[..a.unmatched.len().min(10)]
        );
        for p in &a.problems {
            println!("  {p}");
        }
        if !a.problems.is_empty() || !a.unmatched.is_empty() {
            failed.push(format!(
                "{name}: {} problems, {} unmatched",
                a.problems.len(),
                a.unmatched.len()
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
