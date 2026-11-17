//! Opt-in surveys behind the scenario layouts in docs/formats/h1-scripts.md:
//! which scenario block a script value of each type indexes, found from the
//! names the scripts keep. Prints only offsets, strides and counts.
use std::{collections::BTreeMap, fs::File, io::BufReader, path::PathBuf};

fn disc(var: &str, name: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../disc")
            .join(name)
    })
}

fn fixed(b: &[u8], tok: &str) -> bool {
    b.len() > tok.len() && b.starts_with(tok.as_bytes()) && b[tok.len()] == 0
}

#[test]
#[ignore = "reads disc/halo2.iso"]
fn h2_scenario_blocks_by_script_type() {
    let path = disc("H2_ISO", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let want: Vec<String> = std::env::var("H2_MAPS")
        .unwrap_or("05a_deltaapproach,03a_oldmombasa,01b_spacestation".into())
        .split(',')
        .map(String::from)
        .collect();
    for e in iso
        .entries()
        .unwrap()
        .iter()
        .filter(|e| !e.is_dir && want.iter().any(|w| e.path.ends_with(&format!("/{w}.map"))))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let s = hs::Scenario::read(&mut map).unwrap();
        let tag = map.scenario().unwrap().clone();
        let addr = tag.location.unwrap().address;
        let bytes = map.read_tag_bytes(tag.id).unwrap();
        let strings = map.strings().to_vec();
        let sid = |v: u32| strings.get((v & 0xff_ffff) as usize).cloned().flatten();
        let layouts = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tags/src/schema/layouts.txt"),
        )
        .unwrap();
        let strides: BTreeMap<usize, usize> = layouts
            .split("\ngroup ")
            .find(|g| g.starts_with("scnr "))
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("  0x") && !l.starts_with("   "))
            .filter_map(|l| {
                let w: Vec<_> = l.split_whitespace().collect();
                (w.get(1) == Some(&"block")).then(|| {
                    (
                        usize::from_str_radix(&w[0][2..], 16).unwrap(),
                        usize::from_str_radix(&w[2][2..], 16).unwrap(),
                    )
                })
            })
            .collect();
        let mut atoms: BTreeMap<u16, Vec<(u32, String)>> = BTreeMap::new();
        for n in s
            .nodes()
            .iter()
            .filter(|n| n.live() && n.atom() && !n.global() && n.value_type() != 2)
        {
            if let Ok(t) = s.string(n.string_offset()) {
                atoms
                    .entry(n.opcode())
                    .or_default()
                    .push((n.data(), t.to_owned()));
            }
        }
        println!("== {}", e.path);
        if std::env::var("DUMP").is_ok() {
            for (&field, &stride) in &strides {
                let count =
                    u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize;
                let a = u32::from_le_bytes(bytes[field + 4..field + 8].try_into().unwrap());
                if !matches!(field, 0x1e8 | 0xa0) {
                    continue;
                }
                if count == 0 || a < addr {
                    println!("  {field:#x} stride {stride:#x} count {count}");
                    continue;
                }
                let st = (a - addr) as usize;
                println!("  {field:#x} stride {stride:#x} count {count}");
                for i in 0..count.min(6) {
                    println!(
                        "    [{i}] {:02x?}",
                        &bytes[st + i * stride..st + (i + 1) * stride]
                    );
                }
            }
        }
        for (&ty, list) in &atoms {
            if !matches!(ty, 12 | 14 | 15 | 16 | 18 | 27 | 35 | 36 | 37) {
                continue;
            }
            let mut samples: Vec<_> = list
                .iter()
                .take(4)
                .map(|(d, t)| format!("{d:#x}={t}"))
                .collect();
            samples.dedup();
            println!("type {ty}: {} atoms, e.g. {samples:?}", list.len());
            for (&field, &stride) in &strides {
                let count =
                    u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize;
                let a = u32::from_le_bytes(bytes[field + 4..field + 8].try_into().unwrap());
                if count == 0 || count > 20000 || a < addr || (a - addr) as usize >= bytes.len() {
                    continue;
                }
                let start = (a - addr) as usize;
                {
                    if start + count * stride > bytes.len() {
                        continue;
                    }
                    for k in (0..stride).step_by(2) {
                        let ok = list.iter().all(|(d, t)| {
                            let i = (*d & 0xffff) as usize;
                            if i >= count {
                                return false;
                            }
                            let el = &bytes[start + i * stride..start + (i + 1) * stride];
                            fixed(&el[k..], t)
                                || (k + 4 <= stride
                                    && k % 4 == 0
                                    && sid(u32::from_le_bytes(el[k..k + 4].try_into().unwrap()))
                                        .as_deref()
                                        == Some(t.as_str()))
                        });
                        if ok {
                            println!(
                                "  type {ty}: field {field:#x} count {count} stride {stride:#x} name at +{k:#x}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "reads disc/halo1.iso"]
fn h1_scenario_blocks_by_script_type() {
    let path = disc("H1_ISO", "halo1.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let level = std::env::var("H1_LEVEL").unwrap_or("a30".into());
    let e = d.map(&level).unwrap().clone();
    let bytes = iso.read(&e).unwrap();
    let m = h1map::H1Map::from_reader(&bytes[..]).unwrap();
    let t = h1scripts::Tables::read(&m).unwrap();
    let scnr = m.tag_data(m.scenario()).unwrap();
    let space = m.tag_space();
    let mut atoms: BTreeMap<u16, Vec<(u32, String)>> = BTreeMap::new();
    for n in t
        .nodes
        .iter()
        .filter(|n| n.live() && n.atom() && !n.global() && n.value_type() != 2)
    {
        if let Ok(s) = t.string(n.string_offset()) {
            atoms
                .entry(n.opcode())
                .or_default()
                .push((n.data(), s.to_owned()));
        }
    }
    for (&ty, list) in &atoms {
        if !matches!(ty, 12 | 13 | 14 | 16 | 21) {
            continue;
        }
        let mut samples: Vec<_> = list
            .iter()
            .take(4)
            .map(|(d, t)| format!("{d:#x}={t}"))
            .collect();
        samples.dedup();
        println!("type {ty}: {} atoms, e.g. {samples:?}", list.len());
        for field in (0..0x5a0).step_by(4) {
            let Some(b) = h1map::Block::read(scnr, field) else {
                continue;
            };
            if b.count == 0 || b.count > 5000 {
                continue;
            }
            for stride in (4..=0x100).step_by(4) {
                let Ok(els) = space.block(b, stride) else {
                    break;
                };
                for k in (0..stride).step_by(4) {
                    if list.iter().all(|(d, s)| {
                        let i = (*d & 0xffff) as usize;
                        i < b.count as usize && fixed(&els[i * stride + k..(i + 1) * stride], s)
                    }) {
                        println!(
                            "  type {ty}: field {field:#x} count {} stride {stride:#x} name at +{k:#x}; values {:?}",
                            b.count,
                            list.iter().map(|(d, _)| *d).take(3).collect::<Vec<_>>()
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "reads disc/halo2.iso"]
fn h2_vehicle_seat_labels() {
    let path = disc("H2_ISO", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with("/05a_deltaapproach.map"))
        .unwrap();
    let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
    let mut map = map::Map::from_region(
        BufReader::new(File::open(&path).unwrap()),
        base,
        e.size.into(),
    )
    .unwrap();
    let strings = map.strings().to_vec();
    let sid = |v: u32| {
        strings
            .get((v & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .filter(|s| s.len() == (v >> 24) as usize && !s.is_empty())
    };
    let vehis: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"vehi"))
        .cloned()
        .collect();
    for t in vehis {
        let addr = t.location.unwrap().address;
        let bytes = map.read_tag_bytes(t.id).unwrap();
        for (field, stride) in [(0x1c8usize, 0xb0usize), (0x2f8, 0x2a0)] {
            let count = u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize;
            let a = u32::from_le_bytes(bytes[field + 4..field + 8].try_into().unwrap());
            if count == 0 {
                continue;
            }
            let st = (a - addr) as usize;
            for i in 0..count {
                let el = &bytes[st + i * stride..st + (i + 1) * stride];
                let found: Vec<_> = (0..stride)
                    .step_by(4)
                    .filter_map(|k| {
                        sid(u32::from_le_bytes(el[k..k + 4].try_into().unwrap()))
                            .map(|s| format!("+{k:#x}={s}"))
                    })
                    .collect();
                println!("{:?} {:#x} {field:#x}[{i}] {found:?}", t.name, t.id.0);
            }
        }
    }
}
