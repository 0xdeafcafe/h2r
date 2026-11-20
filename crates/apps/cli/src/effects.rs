//! `h2 effect`: print an effect tree and validate the effects family.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use map::{FourCC, Map, TagId, TagQuery};
use tags::effects::{
    self, EffectTag, TagRef, contrail::Contrail, damage::DamageEffect, decal::Decal,
    effect::Effect, lens::LensFlare, light::Light, movement::ParticleMovement, particle::Particle,
};

const USAGE: &str = "usage: h2 effect <map-name-or-file> <tag name> [group] [--disc disc.iso]\n       h2 effect --all <map-name-or-file|all> [--disc disc.iso]\n\nPrints a tag of the effects family (effe prt3 pmov jpt! cont deca ligh lens)\nas a tree; effe tags show their parts, particle systems and emitters with\nreferenced tag names. With --all, decodes every tag of those groups in the\nmap (or in all disc maps), follows every outgoing tag reference, prints counts\nand exits non-zero if anything fails to decode or a reference does not resolve\nto a tag of the stated class (or one derived from it).";

type M = Map<BufReader<File>>;

/// One registered group: how to decode a tag and list its references.
struct Entry {
    group: FourCC,
    /// Decode and return the references with their field paths and the
    /// number of function blobs that failed to parse (with the first error).
    check: fn(&mut M, TagId) -> tags::Result<Checked>,
    /// Debug-format the decoded tag.
    dump: fn(&mut M, TagId) -> tags::Result<String>,
}

/// What decoding one tag yields for validation.
struct Checked {
    refs: Vec<(String, TagRef)>,
    functions: u64,
    bad_functions: Vec<String>,
}

fn entry<T: EffectTag>() -> Entry {
    Entry {
        group: FourCC(T::GROUP),
        check: |map, id| {
            let tag = effects::read::<T, _>(map, id)?;
            let mut bad_functions = Vec::new();
            let blobs = tag.functions();
            for (path, bytes) in &blobs {
                // An empty blob is an absent function: the engine's evaluator
                // returns 0 for a data reference of size 0 (0x0013b3aa).
                if bytes.is_empty() {
                    continue;
                }
                // Parse and exercise the evaluator: output must be finite.
                match effects::function::Function::parse(bytes) {
                    Ok(f) => {
                        let ok = [0.0f32, 0.25, 0.5, 1.0]
                            .iter()
                            .all(|&x| f.evaluate(x, 0.5).is_finite());
                        if !ok {
                            bad_functions.push(format!("{path}: non-finite output"));
                        }
                    }
                    Err(e) => bad_functions.push(format!("{path}: {e}")),
                }
            }
            Ok(Checked {
                refs: tag.references(),
                functions: blobs.iter().filter(|(_, b)| !b.is_empty()).count() as u64,
                bad_functions,
            })
        },
        dump: |map, id| Ok(format!("{:#?}", effects::read::<T, _>(map, id)?)),
    }
}

fn registry() -> Vec<Entry> {
    vec![
        entry::<Effect>(),
        entry::<Particle>(),
        entry::<ParticleMovement>(),
        entry::<DamageEffect>(),
        entry::<Contrail>(),
        entry::<Decal>(),
        entry::<Light>(),
        entry::<LensFlare>(),
    ]
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut disc, mut all) = (Vec::new(), None::<PathBuf>, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    match (all, positional.as_slice()) {
        (true, [map]) => validate(map, disc),
        (false, [map, tag]) => show(map, tag, None, disc),
        (false, [map, tag, group]) => show(map, tag, Some(group.parse()?), disc),
        _ => bail!("{USAGE}"),
    }
}

fn name_of(map: &M, r: TagRef) -> String {
    if r.is_null() {
        return format!("none ({})", r.group);
    }
    match map.tag(r.id) {
        Ok(t) => format!("{} {}", t.group, t.name.as_deref().unwrap_or("<unnamed>")),
        Err(_) => format!("<unresolved {} {}>", r.group, r.id),
    }
}

fn show(map_name: &str, tag: &str, group: Option<FourCC>, disc: Option<PathBuf>) -> Result<()> {
    let mut map = super::maps::load_map(map_name, disc)?;
    let query = match group {
        Some(g) => TagQuery::NameInGroup(tag, g),
        None => TagQuery::Name(tag),
    };
    let t = map.tag(query)?.clone();
    let reg = registry();
    let e = reg
        .iter()
        .find(|e| e.group == t.group)
        .with_context(|| format!("{} is a {} tag, not in the effects family", tag, t.group))?;
    println!("{} {} {}", t.id, t.group, tag);
    if t.group == FourCC(*b"effe") {
        let fx = effects::read::<Effect, _>(&mut map, t.id)?;
        print_effect(&map, &fx);
    } else {
        println!("{}", (e.dump)(&mut map, t.id)?);
    }
    Ok(())
}

fn print_effect(map: &M, fx: &Effect) {
    println!(
        "flags {:#x}, {} location(s), {} event(s), looping sound: {}",
        fx.flags,
        fx.locations.len(),
        fx.events.len(),
        name_of(map, fx.looping_sound)
    );
    for (i, l) in fx.locations.iter().enumerate() {
        let s = l.resolve(map.strings()).ok().flatten().unwrap_or("?");
        println!("  location[{i}] {s}");
    }
    for (i, ev) in fx.events.iter().enumerate() {
        println!(
            "  event[{i}] skip {} delay {:?} duration {:?}",
            ev.skip_fraction, ev.delay_bounds, ev.duration_bounds
        );
        for (j, p) in ev.parts.iter().enumerate() {
            println!(
                "    part[{j}] {} at location {}, flags {:#x}: {}",
                p.type_group,
                p.location,
                p.flags,
                name_of(map, p.reference)
            );
        }
        for (j, b) in ev.beams.iter().enumerate() {
            println!(
                "    beam[{j}] shader {}, {} function(s)",
                name_of(map, b.shader),
                b.functions.len()
            );
        }
        for (j, a) in ev.accelerations.iter().enumerate() {
            println!(
                "    acceleration[{j}] {} at location {}, cones {}..{}",
                a.acceleration, a.location, a.inner_cone_angle, a.outer_cone_angle
            );
        }
        for (j, ps) in ev.particle_systems.iter().enumerate() {
            println!(
                "    particle_system[{j}] {} at location {}, lod {:?}",
                name_of(map, ps.particle),
                ps.location,
                ps.lod
            );
            for (k, em) in ps.emitters.iter().enumerate() {
                let bytes: usize = em.properties.iter().map(|p| p.data.len()).sum();
                println!(
                    "      emitter[{k}] movement {}, {} properties ({} function bytes)",
                    name_of(map, em.movement),
                    em.properties.len(),
                    bytes
                );
            }
        }
    }
}

#[derive(Default)]
struct Counts {
    tags: u64,
    decoded: u64,
    failed: u64,
    refs: u64,
    null_refs: u64,
    unresolved: u64,
    wrong_class: u64,
    functions: u64,
    bad_functions: u64,
}

/// Is `tag_group` the stated class or derived from it in the map's hierarchy?
fn class_ok(map: &M, tag_group: FourCC, stated: FourCC) -> bool {
    tag_group == stated
        || map.groups().iter().any(|g| {
            g.class == tag_group && (g.parent == Some(stated) || g.grandparent == Some(stated))
        })
}

fn validate(map_name: &str, disc: Option<PathBuf>) -> Result<()> {
    let names: Vec<String> = if map_name == "all" {
        let disc_path = disc.clone().map(Ok).unwrap_or_else(super::default_disc)?;
        let mut iso = super::open_iso(&disc_path)?;
        iso.entries()?
            .iter()
            .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
            .map(|e| {
                e.path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&e.path)
                    .trim_end_matches(".map")
                    .to_owned()
            })
            .collect()
    } else {
        vec![map_name.to_owned()]
    };
    let reg = registry();
    let mut total: BTreeMap<FourCC, Counts> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    for name in &names {
        let mut map = super::maps::load_map(name, disc.clone())?;
        let ids: Vec<_> = map
            .tags()
            .iter()
            .filter(|t| reg.iter().any(|e| e.group == t.group))
            .map(|t| {
                (
                    t.id,
                    t.group,
                    t.name.clone().unwrap_or_default(),
                    t.location.is_some(),
                )
            })
            .collect();
        for (id, group, tag_name, has_data) in ids {
            let c = total.entry(group).or_default();
            c.tags += 1;
            if !has_data {
                c.failed += 1;
                failures.push(format!("{name}: {group} {tag_name}: no payload"));
                continue;
            }
            let e = reg.iter().find(|e| e.group == group).expect("filtered");
            match (e.check)(&mut map, id) {
                Ok(Checked {
                    refs,
                    functions,
                    bad_functions,
                }) => {
                    c.decoded += 1;
                    c.functions += functions;
                    c.bad_functions += bad_functions.len() as u64;
                    for f in bad_functions {
                        failures.push(format!("{name}: {group} {tag_name}: function {f}"));
                    }
                    for (path, r) in refs {
                        c.refs += 1;
                        if r.is_null() {
                            c.null_refs += 1;
                            continue;
                        }
                        match map.tag(r.id) {
                            Err(_) => {
                                c.unresolved += 1;
                                failures.push(format!(
                                    "{name}: {group} {tag_name}: {path} -> {} unresolved",
                                    r.id
                                ));
                            }
                            Ok(t) if !class_ok(&map, t.group, r.group) => {
                                c.wrong_class += 1;
                                failures.push(format!(
                                    "{name}: {group} {tag_name}: {path} says {} but tag is {}",
                                    r.group, t.group
                                ));
                            }
                            Ok(_) => {}
                        }
                    }
                }
                Err(err) => {
                    c.failed += 1;
                    failures.push(format!("{name}: {group} {tag_name}: {err}"));
                }
            }
        }
    }
    println!(
        "{:<6} {:>8} {:>8} {:>7} {:>9} {:>7} {:>10} {:>11} {:>9} {:>9}",
        "group",
        "tags",
        "decoded",
        "failed",
        "refs",
        "null",
        "unresolved",
        "wrong_class",
        "functions",
        "bad_fns"
    );
    let mut bad = 0;
    for (g, c) in &total {
        println!(
            "{:<6} {:>8} {:>8} {:>7} {:>9} {:>7} {:>10} {:>11} {:>9} {:>9}",
            g.to_string(),
            c.tags,
            c.decoded,
            c.failed,
            c.refs,
            c.null_refs,
            c.unresolved,
            c.wrong_class,
            c.functions,
            c.bad_functions
        );
        bad += c.failed + c.unresolved + c.wrong_class + c.bad_functions;
    }
    println!("{} map(s) checked", names.len());
    for f in failures.iter().take(40) {
        println!("FAIL {f}");
    }
    if failures.len() > 40 {
        println!("... and {} more", failures.len() - 40);
    }
    if bad > 0 {
        bail!("{bad} effects-family failure(s)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// A tiny invented v8 map holding one `effe` tag whose payload is a
    /// well-formed root, with `corrupt` optionally pointing its event block
    /// outside the metadata space. No game data.
    fn synthetic_map(corrupt: bool) -> Vec<u8> {
        fn put(b: &mut [u8], o: usize, v: u32) {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        let cc = |s: &[u8; 4]| u32::from_be_bytes(*s);
        let mut b = vec![0u8; 0x2000];
        b[..4].copy_from_slice(b"daeh");
        b[0x7fc..0x800].copy_from_slice(b"toof");
        b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
        for (o, v) in [
            (4, 8),
            (8, 0x2000),
            (16, 0x1000),
            (20, 0x200),
            (24, 0x600),
            (28, 0x600),
        ] {
            put(&mut b, o, v);
        }
        for (o, v) in [
            (0, 0x8000_0020),
            (4, 1),
            (8, 0x8000_002c),
            (12, 0xffff_ffff),
            (16, 0xffff_ffff),
            (24, 1),
            (28, cc(b"tags")),
        ] {
            put(&mut b, 0x1000 + o, v);
        }
        for (o, v) in [(0, cc(b"effe")), (4, 0xffff_ffff), (8, 0xffff_ffff)] {
            put(&mut b, 0x1020 + o, v);
        }
        for (o, v) in [
            (0, cc(b"effe")),
            (4, 0xa000_0000),
            (8, 0x9000_0000),
            (12, 0x30),
        ] {
            put(&mut b, 0x102c + o, v);
        }
        put(&mut b, 0x121c, cc(b"lsnd"));
        put(&mut b, 0x1220, 0xffff_ffff);
        if corrupt {
            put(&mut b, 0x1214, 1);
            put(&mut b, 0x1218, 0x9100_0000);
        }
        b
    }

    fn write_map(name: &str, corrupt: bool) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2-effects-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, synthetic_map(corrupt)).unwrap();
        path
    }

    /// `main` turns an `Err` from `run` into a non-zero exit, so `--all` must
    /// return `Err` for a map with a failing effect, and `Ok` for a clean one.
    #[test]
    fn all_fails_on_a_malformed_map_and_passes_a_clean_one() {
        let good = write_map("good.map", false);
        let bad = write_map("bad.map", true);
        let run = |p: &PathBuf| super::run(vec!["--all".into(), p.to_string_lossy().into_owned()]);
        let ok = run(&good);
        let err = run(&bad);
        let _ = std::fs::remove_dir_all(good.parent().unwrap());
        ok.expect("clean map validates");
        let msg = err.expect_err("malformed map must fail").to_string();
        assert!(msg.contains("effects-family failure"), "{msg}");
    }

    #[test]
    fn help_is_ok_and_bad_arguments_fail() {
        assert!(super::run(vec!["--help".into()]).is_ok());
        assert!(super::run(vec![]).is_err());
        assert!(super::run(vec!["--bogus".into()]).is_err());
        assert!(
            super::run(vec!["--all".into()]).is_err(),
            "--all needs a map"
        );
        assert!(super::run(vec!["a".into(), "b".into(), "c".into(), "d".into()]).is_err());
    }

    #[test]
    fn registry_covers_the_eight_groups_once() {
        let mut groups: Vec<_> = super::registry()
            .iter()
            .map(|e| e.group.to_string())
            .collect();
        groups.sort();
        assert_eq!(
            groups,
            [
                "cont", "deca", "effe", "jpt!", "lens", "ligh", "pmov", "prt3"
            ]
        );
    }
}
