//! Opt-in checks against the owner's discs. Set `H1_ISO` (default
//! `disc/halo1.iso`) and `H2_DISC` (default `disc/halo2.iso`). Nothing read
//! here is written to the repository; summaries go to stdout.
use std::{fs::File, io::BufReader, path::PathBuf};

use h1scripts::{Catalog, Tables, decompile::decompile};

fn disc(var: &str, name: &str) -> PathBuf {
    std::env::var_os(var)
        .or_else(|| {
            if var == "H2_DISC" {
                std::env::var_os("H2_ISO")
            } else {
                None
            }
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../disc")
                .join(name)
        })
}

fn h1() -> (Catalog, Vec<(String, h1map::H1Map)>) {
    let path = disc("H1_ISO", "halo1.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let xbe_entry = iso.find("/default.xbe").unwrap();
    let xbe_bytes = iso.read(&xbe_entry).unwrap();
    let catalog = Catalog::from_xbe(&xbe::Xbe::parse(&xbe_bytes).unwrap()).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let maps = h1map::disc::CAMPAIGN_LEVELS
        .iter()
        .map(|&l| {
            let e = d.map(l).unwrap().clone();
            let bytes = iso.read(&e).unwrap();
            (l.to_owned(), h1map::H1Map::from_reader(&bytes[..]).unwrap())
        })
        .collect();
    (catalog, maps)
}

#[test]
#[ignore = "reads disc/halo1.iso"]
fn every_campaign_script_decompiles() {
    let (catalog, maps) = h1();
    println!(
        "catalog: {} builtins, {} types",
        catalog.functions().len(),
        catalog.types().len()
    );
    let special = (0..catalog.functions().len() as u16)
        .filter(|&i| !catalog.fixed_signature(i))
        .count();
    println!("special forms: {special}");
    for (name, map) in &maps {
        let t = Tables::read(map).unwrap();
        let d = decompile(&t, &catalog).unwrap_or_else(|e| panic!("{name}: {e}"));
        let s = &d.stats;
        println!(
            "{name}: scripts {} globals {} refs {} live {} reached {} emitted {} calls {} (checked {} special {}) script-calls {} globals-refs {} literals {} casts {} stale-heads {} builtins {}",
            s.scripts,
            s.globals,
            s.references,
            s.live_nodes,
            s.reached_nodes,
            s.emitted_nodes,
            s.builtin_calls,
            s.checked_calls,
            s.special_calls,
            s.script_calls,
            s.global_refs,
            s.literals,
            s.casts,
            s.stale_heads,
            s.builtin_uses.len()
        );
        assert_eq!(
            s.reached_nodes, s.live_nodes,
            "{name}: unreached live nodes"
        );
    }
}

fn h2() -> h1scripts::translate::Target {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso.find("default.xbe").unwrap();
    let bytes = iso.read(&e).unwrap();
    let mut c = hs::Catalog::from_xbe(&xbe::Xbe::parse(&bytes).unwrap()).unwrap();
    let mut scenarios = Vec::new();
    for e in iso
        .entries()
        .unwrap()
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let s = hs::Scenario::read(&mut map).unwrap();
        c.observe(&s).unwrap();
        scenarios.push(s);
    }
    let mut t = h1scripts::translate::Target::from_catalog(&c);
    for s in &scenarios {
        t.observe_externals(s);
    }
    t
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn translation_coverage() {
    use h1scripts::translate::{Coverage, Kind, Table};
    let (catalog, maps) = h1();
    let target = h2();
    let table = Table::build(&catalog, &target);
    let unmapped: Vec<_> = catalog
        .types()
        .iter()
        .zip(&table.types)
        .filter(|(_, m)| m.is_none())
        .map(|(n, _)| n.as_str())
        .collect();
    println!(
        "H2 named builtins {}, H1 types without an H2 type: {unmapped:?}",
        target.builtins.len()
    );
    let mut all = Coverage::default();
    for (name, map) in &maps {
        let t = Tables::read(map).unwrap();
        let d = decompile(&t, &catalog).unwrap();
        let c = table.coverage(&d.stats.builtin_uses);
        let pct = |k| 100.0 * c.of(k) as f64 / c.total() as f64;
        println!(
            "{name}: calls {} direct {:.1}% renamed {:.1}% adapter {:.1}% missing {:.1}% ({} builtins missing)",
            c.total(),
            pct(Kind::Direct),
            pct(Kind::Renamed),
            pct(Kind::Adapter),
            pct(Kind::Missing),
            c.missing.len()
        );
        all.add(&c);
    }
    let pct = |k| 100.0 * all.of(k) as f64 / all.total() as f64;
    println!(
        "all: calls {} direct {:.1}% renamed {:.1}% adapter {:.1}% missing {:.1}% ({} builtins missing)",
        all.total(),
        pct(Kind::Direct),
        pct(Kind::Renamed),
        pct(Kind::Adapter),
        pct(Kind::Missing),
        all.missing.len()
    );
    let mut ranked: Vec<_> = all.missing.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (i, n) in ranked {
        let t = table.builtin(*i).unwrap();
        let f = catalog.function(*i).unwrap();
        let params: Vec<_> = f
            .parameters
            .iter()
            .map(|&p| catalog.type_name(p).unwrap())
            .collect();
        println!(
            "MISSING {n:5} {} ({}) -> {} [{}]",
            t.name,
            params.join(" "),
            catalog.type_name(f.return_type).unwrap(),
            t.reason.unwrap_or("")
        );
    }
    for t in table.builtins.iter().filter(|t| t.kind == Kind::Adapter) {
        println!(
            "ADAPTER {} -> {} {:?}",
            t.name,
            t.target.as_ref().unwrap().0,
            t.adapter.unwrap()
        );
    }
}

fn h2_catalog() -> hs::Catalog {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso.find("default.xbe").unwrap();
    let bytes = iso.read(&e).unwrap();
    hs::Catalog::from_xbe(&xbe::Xbe::parse(&bytes).unwrap()).unwrap()
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_campaign_script_compiles_and_round_trips() {
    use h1scripts::compile::{Missing, compile};
    let (catalog, maps) = h1();
    let target = h2();
    let h2c = h2_catalog();
    let table = h1scripts::translate::Table::build(&catalog, &target);
    const BASE: u32 = 0x8010_0000;
    for (name, map) in &maps {
        let t = Tables::read(map).unwrap();
        let refs: Vec<String> = t
            .references
            .iter()
            .map(|r| map.tag(r.id).map(|t| t.name.clone()).unwrap_or_default())
            .collect();
        for policy in [Missing::Stub, Missing::Extension] {
            let c = compile(&t, &catalog, &table, &target, policy, &refs)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let bytes = c.scenario_bytes(BASE);
            let s = hs::Scenario::decode(&bytes, BASE).unwrap_or_else(|e| panic!("{name}: {e}"));
            s.verify_codec_symmetry(&bytes, BASE).unwrap();
            assert_eq!(s.nodes().len(), c.nodes.len());
            assert_eq!(s.scripts().len(), t.scripts.len());
            assert_eq!(s.globals().len(), t.globals.len());
            let reached = s.reachable(s.roots()).unwrap().len();
            assert_eq!(reached, c.nodes.len(), "{name}: unreached nodes");
            if policy == Missing::Stub {
                let v = s.validate(&h2c).unwrap_or_else(|e| panic!("{name}: {e}"));
                let src = s.decompile(&h2c, None).unwrap();
                let counts = hs::source_counts(&src.source).unwrap();
                assert_eq!(counts.nodes, src.emitted_nodes);
                println!(
                    "{name}: h2 nodes {} builtin-calls {} script-calls {} hooks {} missing-calls {} missing-globals {} strings {}B",
                    c.nodes.len(),
                    v.builtin_calls,
                    v.script_calls,
                    c.hooks.len(),
                    c.missing.len(),
                    c.missing_globals.len(),
                    c.strings.len()
                );
            }
        }
    }
}

/// Under `Cutscenes::Skip`, per level: no call of the skip guard is left
/// (each became `false`), the stubbed conversation and command list
/// statuses read as done, and every node is still reached. a10's `(wake tutorial_setup)` wakes `fast_setup`, by its index.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn skipped_cutscenes_on_every_level() {
    use h1scripts::compile::{Cutscenes, Missing, SKIP_GUARD, compile_with};
    let (catalog, maps) = h1();
    let target = h2();
    let table = h1scripts::translate::Table::build(&catalog, &target);
    const BASE: u32 = 0x8010_0000;
    for (name, map) in &maps {
        let t = Tables::read(map).unwrap();
        let src = decompile(&t, &catalog).unwrap().source;
        let guards = src.matches(&format!("({SKIP_GUARD})")).count();
        let mut seen = Vec::new();
        for cut in [Cutscenes::Play, Cutscenes::Skip] {
            let c = compile_with(
                &t,
                &catalog,
                &table,
                &target,
                Missing::Stub,
                cut,
                &|_, _| true,
                &[],
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            let bytes = c.scenario_bytes(BASE);
            let s = hs::Scenario::decode(&bytes, BASE).unwrap();
            s.verify_codec_symmetry(&bytes, BASE).unwrap();
            assert_eq!(s.reachable(s.roots()).unwrap().len(), c.nodes.len());
            let text = |n: &hs::Node| s.string(n.string_offset()).unwrap_or("").to_owned();
            let calls = s
                .nodes()
                .iter()
                .filter(|n| n.script_call())
                .filter(|n| text(s.node(hs::Datum(n.data())).unwrap()) == SKIP_GUARD)
                .count();
            // Script atoms naming a setup script: (index, token); the
            // data word is `0xffff0000 | index`.
            let setups: Vec<(u32, String)> = s
                .nodes()
                .iter()
                .filter(|n| n.atom() && n.opcode() == 10)
                .map(|n| (n.data() & 0xffff, text(n)))
                .filter(|(_, t)| t.ends_with("_setup"))
                .collect();
            seen.push((calls, setups));
        }
        let (played, skipped) = (&seen[0], &seen[1]);
        println!(
            "{name}: {guards} skip guards in the source; calls left played {} skipped {}; setups woken played {:?} skipped {:?}",
            played.0, skipped.0, played.1, skipped.1
        );
        assert_eq!(played.0, guards, "{name}");
        assert_eq!(skipped.0, 0, "{name}");
        let index = |n: &str| t.scripts.iter().position(|s| s.name == n).unwrap() as u32;
        if name == "a10" {
            assert!(
                played
                    .1
                    .contains(&(index("tutorial_setup"), "tutorial_setup".into()))
            );
            assert!(!skipped.1.iter().any(|(_, t)| t == "tutorial_setup"));
            assert!(
                skipped
                    .1
                    .contains(&(index("fast_setup"), "fast_setup".into()))
            );
        } else {
            assert_eq!(played.1, skipped.1, "{name}");
        }
    }
}

/// The stubbed builtins (`Kind::Missing`) the ten levels' waits read: per
/// builtin, the `sleep_until` and `if` conditions holding a call to it,
/// per level, with why it is missing and Halo 1's signature. A wait reads
/// a stub when its condition (not following script calls) calls one. A
/// stub is handled when the compiler gives it a value its waits read as
/// done (`DONE_VALUES`, `ai_status` on `STATUS_LADDER`, or zero where
/// every wait reads zero as done); the rest are listed per level.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn waits_on_stubs() {
    use h1scripts::translate::{Kind, Table};
    use std::collections::BTreeMap;
    let (catalog, maps) = h1();
    let target = h2();
    let table = Table::build(&catalog, &target);
    let missing = |i: u16| table.builtin(i).is_some_and(|t| t.kind == Kind::Missing);
    // Zero is done for every wait on these (`(= 0 x)`, `(sleep x)`).
    const ZERO_DONE: &[&str] = &["recording_time", "ai_going_to_vehicle"];
    let handled = |b: &str| {
        b == "ai_status"
            || ZERO_DONE.contains(&b)
            || h1scripts::compile::DONE_VALUES.iter().any(|d| d.0 == b)
    };
    // builtin -> (level -> (sleep_until count, if count))
    let mut by: BTreeMap<String, BTreeMap<String, [usize; 2]>> = BTreeMap::new();
    for (name, map) in &maps {
        let t = Tables::read(map).unwrap();
        let mut waits = [0usize; 2];
        let mut blocked = [0usize; 2];
        let mut still = [0usize; 2];
        let mut left: BTreeMap<String, usize> = BTreeMap::new();
        for n in t
            .nodes
            .iter()
            .filter(|n| n.live() && !n.atom() && !n.script_call())
        {
            let Ok(f) = catalog.function(n.opcode()) else {
                continue;
            };
            let k = match f.name.as_str() {
                "sleep_until" => 0,
                "if" => 1,
                _ => continue,
            };
            let head = t.node(hs::Datum(n.data())).unwrap();
            let cond = head.next();
            if cond == hs::Datum::NULL {
                continue;
            }
            waits[k] += 1;
            let mut found = std::collections::BTreeSet::new();
            let mut stack = vec![cond];
            while let Some(d) = stack.pop() {
                let x = t.node(d).unwrap();
                if x.atom() || x.script_call() {
                    continue;
                }
                if missing(x.opcode()) {
                    found.insert(catalog.function(x.opcode()).unwrap().name.clone());
                    continue;
                }
                let mut a = t.node(hs::Datum(x.data())).unwrap().next();
                while a != hs::Datum::NULL {
                    stack.push(a);
                    a = t.node(a).unwrap().next();
                }
            }
            if !found.is_empty() {
                blocked[k] += 1;
            }
            let open: Vec<&String> = found.iter().filter(|b| !handled(b)).collect();
            if !open.is_empty() {
                still[k] += 1;
                for b in open {
                    *left.entry(b.clone()).or_default() += 1;
                }
            }
            for b in found {
                by.entry(b).or_default().entry(name.clone()).or_default()[k] += 1;
            }
        }
        println!(
            "{name}: sleep_until {} ({} read a stub, {} an unhandled one), if {} ({} read a stub, {} an unhandled one); unhandled {left:?}",
            waits[0], blocked[0], still[0], waits[1], blocked[1], still[1]
        );
    }
    let mut ranked: Vec<_> = by.iter().collect();
    ranked.sort_by_key(|(_, l)| std::cmp::Reverse(l.values().map(|v| v[0] + v[1]).sum::<usize>()));
    for (b, levels) in ranked {
        let i = catalog.index_of(b).unwrap();
        let f = catalog.function(i).unwrap();
        let params: Vec<_> = f
            .parameters
            .iter()
            .map(|&p| catalog.type_name(p).unwrap())
            .collect();
        let h2 = target.builtins.get(b).map(|x| {
            let p: Vec<_> = x
                .parameters
                .iter()
                .map(|&p| target.types[p as usize].as_str())
                .collect();
            format!(
                "h2 {}({}) -> {}",
                x.index,
                p.join(" "),
                target.types[x.return_type as usize]
            )
        });
        let total: usize = levels.values().map(|v| v[0] + v[1]).sum();
        println!(
            "STUB {total:4} {b}({}) -> {} [{}] {} {:?}",
            params.join(" "),
            catalog.type_name(f.return_type).unwrap(),
            table.builtin(i).unwrap().reason.unwrap_or(""),
            h2.unwrap_or_default(),
            levels
        );
    }
}

/// Halo 1's `sound_impulse_time` becomes Halo 2's
/// `sound_impulse_language_time`: one sound argument, a number back, and
/// the evaluator at `0x2a9fe0` that returns the ticks a playing impulse
/// has left (`0x18a240`). Stubbed, every wait on a line of dialogue ended
/// at once.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn sound_impulse_time_waits_on_the_playing_sound() {
    use h1scripts::translate::{Kind, Table};
    let (catalog, _) = h1();
    let target = h2();
    let table = Table::build(&catalog, &target);
    let t = table
        .builtins
        .iter()
        .find(|t| t.name == "sound_impulse_time")
        .unwrap();
    assert_ne!(t.kind, Kind::Missing, "{:?}", t.reason);
    let (name, b) = t.target.as_ref().unwrap();
    assert_eq!(name, "sound_impulse_language_time");
    let c = h2_catalog();
    let f = c.function(b.index).unwrap();
    let types: Vec<_> = f
        .parameters()
        .iter()
        .map(|&p| c.type_name(p).unwrap())
        .collect();
    println!(
        "{} -> {name}: index {}, evaluator {:#x}, ({}) -> {}, {:?} {:?}",
        t.name,
        b.index,
        f.evaluate(),
        types.join(" "),
        c.type_name(f.return_type()).unwrap(),
        t.kind,
        t.adapter
    );
    assert_eq!(f.evaluate(), 0x002a_9fe0);
    assert_eq!(types, ["sound"]);
}

/// Halo 1's `(sleep -1 [script])` sleeps until woken; Halo 2's `sleep`
/// ignores a negative duration (E `0x002087e0`), so every one is compiled
/// as Halo 2's `sleep_forever` (`0x00208750`). On every level: as many
/// `sleep_forever` calls as Halo 1 has negative sleeps, and no Halo 2
/// `sleep` left with a negative literal. a10's continuous
/// `dialog_flavor_captain` is eight lines with `(sleep -1)` between them;
/// with the sleeps ignored it started every line each tick.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn negative_sleeps_sleep_forever_on_every_level() {
    use h1scripts::compile::{Missing, compile};
    let (catalog, maps) = h1();
    let target = h2();
    let forever = target.builtins["sleep_forever"].clone();
    let sleep = target.builtins["sleep"].clone();
    // E: the evaluators of 19 and 20 are 0x002087e0 and 0x00208750.
    assert_eq!((sleep.index, forever.index), (19, 20));
    let table = h1scripts::translate::Table::build(&catalog, &target);
    let mut total = 0;
    for (name, map) in &maps {
        let t = Tables::read(map).unwrap();
        let source = decompile(&t, &catalog).unwrap().source;
        let negative = source.matches("(sleep -").count();
        let c = compile(&t, &catalog, &table, &target, Missing::Stub, &[]).unwrap();
        assert_eq!(c.forever.len(), negative, "{name}");
        let s = hs::Scenario::decode(&c.scenario_bytes(0x8010_0000), 0x8010_0000).unwrap();
        for n in s
            .nodes()
            .iter()
            .filter(|n| !n.atom() && !n.script_call() && n.opcode() == sleep.index)
        {
            let head = s.node(hs::Datum(n.data())).unwrap();
            let d = *s.node(head.next()).unwrap();
            assert!(
                !(d.atom() && !d.global() && (d.data() as u16 as i16) < 0),
                "{name}: a negative sleep is left"
            );
        }
        let calls = s
            .nodes()
            .iter()
            .filter(|n| !n.atom() && !n.script_call() && n.opcode() == forever.index)
            .count();
        assert_eq!(calls, negative, "{name}");
        let scripts: std::collections::BTreeSet<&str> =
            c.forever.iter().map(|u| u.definition.as_str()).collect();
        println!("{name}: {negative} negative sleeps -> sleep_forever, in {scripts:?}");
        total += negative;
    }
    assert!(total > 0);
}

/// The cinematic calls of a10's opening (`x10` and the scripts it calls:
/// fades, camera moves, letterbox, BSP switches, looping sounds) carry
/// over one for one: each is the Halo 2 builtin of the same name with the
/// same parameter types, so a duration stays a `short` count of ticks on
/// both sides. Retail Halo 2's own scripts pass those durations as ticks
/// too (on the disc the commonest literals are `fade_in` 15, 60, 30,
/// `fade_out` 0, 15, 30 and `camera_set` 0, 500, 60: as seconds a
/// 500-second camera move, as ticks 17 seconds).
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn cinematic_calls_keep_halo1_s_ticks() {
    use h1scripts::translate::{Kind, Table};
    const CALLS: &[&str] = &[
        "fade_in",
        "fade_out",
        "camera_set",
        "camera_control",
        "camera_time",
        "cinematic_start",
        "cinematic_stop",
        "cinematic_show_letterbox",
        "switch_bsp",
        "sound_looping_start",
        "sound_looping_stop",
        "sound_class_set_gain",
        "sound_impulse_start",
        "object_set_scale",
    ];
    let (catalog, _) = h1();
    let target = h2();
    let table = Table::build(&catalog, &target);
    let h2_type = |t: u16| target.types[t as usize].clone();
    for name in CALLS {
        let i = (0..catalog.functions().len() as u16)
            .find(|&i| catalog.function(i).unwrap().name == *name)
            .unwrap();
        let f = catalog.function(i).unwrap();
        let tr = table.builtin(i).unwrap();
        assert_eq!(tr.kind, Kind::Direct, "{name}");
        let (h2_name, b) = tr.target.clone().unwrap();
        assert_eq!(h2_name, *name);
        let h1_params: Vec<_> = f
            .parameters
            .iter()
            .map(|&p| catalog.type_name(p).unwrap().to_owned())
            .collect();
        let h2_params: Vec<_> = b.parameters.iter().map(|&p| h2_type(p)).collect();
        assert_eq!(h1_params, h2_params, "{name}");
        println!("{name}: ({}) both", h1_params.join(" "));
    }
    // Retail Halo 2's literal durations.
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let mut seen: std::collections::BTreeMap<(&str, i16), usize> = Default::default();
    for e in iso
        .entries()
        .unwrap()
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let s = hs::Scenario::read(&mut map).unwrap();
        for n in s
            .nodes()
            .iter()
            .filter(|n| n.live() && !n.atom() && !n.script_call())
        {
            let head = s.node(hs::Datum(n.data())).unwrap();
            let Ok(name) = s.string(head.string_offset()) else {
                continue;
            };
            let Some(&call) = ["fade_in", "fade_out", "camera_set"]
                .iter()
                .find(|c| **c == name)
            else {
                continue;
            };
            let mut a = head.next();
            let mut last = None;
            while a != hs::Datum::NULL {
                let x = *s.node(a).unwrap();
                last = Some(x);
                a = x.next();
            }
            if let Some(x) = last.filter(|x| x.atom() && !x.global()) {
                *seen.entry((call, x.data() as u16 as i16)).or_default() += 1;
            }
        }
    }
    for call in ["fade_in", "fade_out", "camera_set"] {
        let mut v: Vec<_> = seen.iter().filter(|((c, _), _)| *c == call).collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<_> = v
            .iter()
            .take(6)
            .map(|((_, d), n)| format!("{d} x{n}"))
            .collect();
        println!("retail {call} durations: {}", top.join(", "));
        assert!(v.iter().take(3).all(|((_, d), _)| d % 5 == 0), "{call}");
    }
}

#[test]
#[ignore = "reads H1_ISO and H2_DISC"]
fn encounter_controls_emit_live_evaluators_and_native_orders() {
    use h1ai::control::Operation;
    use h1scripts::compile::{Cutscenes, Missing, compile_with_ai};
    use hs::h1_ai_slots::{CLAIMS, CONTROL, VEHICLE_COUNT, Operation as RuntimeOperation};

    let (catalog, maps) = h1();
    let target = h2();
    let descriptors = h2_catalog();
    let translations = h1scripts::translate::Table::build(&catalog, &target);
    for binding in h1scripts::aicontrol::BINDINGS {
        let builtin = descriptors.function(binding.index).unwrap();
        assert_eq!(builtin.evaluate(), binding.evaluator, "{}", binding.name);
        assert_eq!(target.builtins[binding.name].index, binding.index);
    }
    let mut installed_operations = std::collections::BTreeSet::new();
    for claim in CLAIMS {
        assert!(claim.matches(descriptors.function(claim.slot).unwrap()));
    }
    let mut runtime_operations = std::collections::BTreeSet::new();
    let mut counted_spawn = 0;
    let mut vehicle = 0;
    let mut grenades = false;
    let mut explicit_distance = false;
    for (name, map) in &maps {
        let tables = Tables::read(map).unwrap();
        let requests = h1scripts::aicontrol::requests(&tables, &catalog);
        let source = h1ai::h1::H1Ai::read(map).unwrap();
        let opts = h1ai::convert::Options::default();
        let mut plan = h1ai::convert::convert(&source, &opts);
        let controls = h1ai::control::prepare(&source, &mut plan, &opts, &requests);
        if requests
            .iter()
            .any(|r| r.operation == Operation::FollowDistance)
        {
            explicit_distance |= plan.ai.orders.iter().any(|o| o.follow_radius == 5.0);
        }
        installed_operations.extend(
            controls
                .actions
                .iter()
                .filter(|(_, actions)| !actions.is_empty())
                .map(|(request, _)| request.operation),
        );
        let compiled = compile_with_ai(
            &tables,
            &catalog,
            &translations,
            &target,
            Missing::Stub,
            Cutscenes::Play,
            &|_, _| true,
            &[],
            Some(&controls),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        const BASE: u32 = 0x8010_0000;
        let scenario = hs::Scenario::decode(&compiled.scenario_bytes(BASE), BASE).unwrap();
        scenario
            .validate(&descriptors)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        for node in scenario
            .nodes()
            .iter()
            .filter(|n| !n.atom() && !n.script_call())
        {
            let first = || {
                scenario
                    .node(scenario.node(hs::Datum(node.data())).unwrap().next())
                    .unwrap()
            };
            match node.opcode() {
                CONTROL | VEHICLE_COUNT => {
                    let head = scenario.node(hs::Datum(node.data())).unwrap();
                    let operation = RuntimeOperation::from_marker(head.data()).unwrap();
                    runtime_operations.insert(operation as u16);
                }
                280 => grenades = true,
                291 => {
                    counted_spawn += 1;
                    let count = scenario.node(first().next()).unwrap();
                    assert_eq!((count.value_type(), count.data()), (7, 1));
                }
                333 => {
                    let order = scenario.node(first().next()).unwrap().data() as u16;
                    assert!(
                        usize::from(order) < plan.ai.orders.len(),
                        "{name}: order {order}"
                    );
                    assert!((first().data() as usize) < plan.ai.squads.len());
                }
                343 => {
                    vehicle += 1;
                    let seat = scenario
                        .node(scenario.node(first().next()).unwrap().next())
                        .unwrap();
                    assert_eq!(
                        descriptors.type_name(seat.value_type()).unwrap(),
                        "unit_seat_mapping"
                    );
                }
                _ => {}
            }
        }
    }
    for operation in [
        Operation::Attack,
        Operation::Defend,
        Operation::Maneuver,
        Operation::ManeuverEnable,
        Operation::Spawn,
        Operation::FollowPlayers,
        Operation::FollowAi,
        Operation::FollowDisable,
        Operation::Migrate,
    ] {
        assert!(installed_operations.contains(&operation), "{operation:?}");
    }
    // A missing parent call can remove a translated descendant. These
    // assertions check that the converted graphs still reach each family.
    for operation in [
        RuntimeOperation::SeePlayers,
        RuntimeOperation::Free,
        RuntimeOperation::FreeUnits,
        RuntimeOperation::MigrateUnits,
        RuntimeOperation::Teleport,
        RuntimeOperation::TeleportUnsupported,
        RuntimeOperation::Maneuver,
        RuntimeOperation::VehicleEncounter,
        RuntimeOperation::VehicleDistance,
        RuntimeOperation::VehicleActorType,
        RuntimeOperation::VehicleDisable,
        RuntimeOperation::VehicleTick,
        RuntimeOperation::VehicleCount,
    ] {
        assert!(runtime_operations.contains(&(operation as u16)), "{operation:?}");
    }
    assert!(counted_spawn > 0 && vehicle > 0 && grenades);
    assert!(explicit_distance);
}
