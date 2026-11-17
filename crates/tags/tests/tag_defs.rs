//! Typed tag definitions against the owner's disc. Opt-in: `H2_DISC` names
//! the ISO. Nothing is written anywhere.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo test --release -p tags --test tag_defs -- --ignored --nocapture
//! ```
//!
//! - `H2_DEFS_GROUPS=hlmt,mode` picks groups (default: every defined group);
//! - `H2_DEFS_MAPS=lockout,shared` picks maps (default: every map);
//! - `H2_DEFS_SKELETON=1`: for the picked groups, ignore the definitions
//!   and print a starting definition from the schema with every other word
//!   an unknown span, summarised from retail (`seen=`);
//! - `H2_DEFS_ANNOTATE=1`: print the picked groups' definitions with
//!   `seen=` refreshed from retail;
//! - `H2_DEFS_EXTRA=a.txt,b.txt`: definition files parsed on top of the
//!   built-in ones (replacing their groups and structs); with no
//!   `H2_DEFS_GROUPS`, only their groups are checked. For drafting a
//!   definition outside the source tree.

use map::{FourCC, Map};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use tags::defs::{self, Coverage, Defs, Str, StructDef, Target, Val, skeleton};
use tags::repo::extract::SharedFiles;
use tags::repo::{Body, LooseTag, MapSource};
use tags::schema::Schema;

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type DiscMap = Map<BufReader<File>>;

fn disc_maps() -> Res<Vec<(String, u64, u64)>> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC")?;
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
    let mut out = Vec::new();
    for e in iso.entries()? {
        let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
        if let Some(stem) = file.strip_suffix(".map")
            && !e.is_dir
        {
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            out.push((stem.to_owned(), base, u64::from(e.size)));
        }
    }
    // Shared caches first: their tags are the ones levels repeat.
    out.sort_by_key(|(s, ..)| {
        (
            !matches!(s.as_str(), "mainmenu" | "shared" | "single_player_shared"),
            s.clone(),
        )
    });
    Ok(out)
}

fn open(base: u64, size: u64) -> Res<DiscMap> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC")?;
    Ok(Map::from_region(
        BufReader::new(File::open(disc)?),
        base,
        size,
    )?)
}

fn shared(schema: &Schema, maps: &[(String, u64, u64)]) -> Res<SharedFiles<BufReader<File>>> {
    let mut s = SharedFiles::default();
    for (stem, base, size) in maps {
        let file = match stem.as_str() {
            "mainmenu" => map::ResourceFile::MainMenu,
            "shared" => map::ResourceFile::Shared,
            "single_player_shared" => map::ResourceFile::SinglePlayerShared,
            _ => continue,
        };
        s.set(schema, file, open(*base, *size)?)?;
    }
    Ok(s)
}

/// Every distinct retail tag of `groups` on the disc (by content), handed
/// to `each` once.
fn each_tag(groups: &HashSet<FourCC>, mut each: impl FnMut(&str, &LooseTag)) -> Res<Totals> {
    let schema = Schema::builtin();
    let maps = disc_maps()?;
    let want: Option<HashSet<String>> = std::env::var("H2_DEFS_MAPS")
        .ok()
        .map(|m| m.split(',').map(str::to_owned).collect());
    let mut sh = shared(schema, &maps)?;
    let mut seen = HashSet::new();
    let mut t = Totals::default();
    for (stem, base, size) in &maps {
        if want.as_ref().is_some_and(|w| !w.contains(stem)) {
            continue;
        }
        let mut src = MapSource::new(schema, open(*base, *size)?, &mut sh)?;
        let ids: Vec<_> = src
            .tags()
            .iter()
            .filter(|x| groups.contains(&x.group))
            .map(|x| x.id)
            .collect();
        for id in ids {
            t.instances += 1;
            match src.extract(id)? {
                Err(r) => {
                    t.refused.push(format!("{stem}: {} ({})", r.key, r.why));
                }
                Ok(tag) => {
                    if matches!(tag.body, Body::Alias { .. }) {
                        t.aliases += 1;
                        continue;
                    }
                    if !seen.insert(sha(&defs::canonical(&tag))) {
                        continue;
                    }
                    each(stem, &tag);
                }
            }
        }
    }
    t.distinct = seen.len();
    Ok(t)
}

#[derive(Default)]
struct Totals {
    instances: usize,
    distinct: usize,
    aliases: usize,
    refused: Vec<String>,
}

fn sha(b: &[u8]) -> [u8; 32] {
    tags::repo::sha256(b)
}

fn picked(defs: &Defs, skeleton: bool) -> HashSet<FourCC> {
    match std::env::var("H2_DEFS_GROUPS") {
        Ok(g) => g
            .split(',')
            .filter_map(defs::parse_fourcc)
            .filter(|g| skeleton || defs.groups.contains_key(g))
            .collect(),
        Err(_) => defs.groups.keys().copied().collect(),
    }
}

/// Values a loose tag must not hold where the definition sees a pointer
/// kind with no fixup: a datum, string index or address that only means
/// something in a pinned map.
fn pinned_leftovers(def: &StructDef, rec: &defs::Record, path: &str, out: &mut Vec<String>) {
    for (f, (_, v)) in def.fields.iter().zip(&rec.fields) {
        let p = format!("{path}.{}", f.name);
        match (&f.kind, v) {
            (
                _,
                Val::Ref {
                    target: Target::Id(_),
                    ..
                }
                | Val::Datum(Target::Id(_)),
            ) => out.push(p),
            (_, Val::String(Str::Index(_))) => out.push(p),
            (_, Val::Unresolved { address, .. }) if *address != 0 => out.push(p),
            (defs::Kind::Block(s), Val::Block(e)) => {
                for (i, r) in e.iter().enumerate() {
                    pinned_leftovers(s, r, &format!("{p}[{i}]"), out);
                }
            }
            (defs::Kind::Struct(s), Val::Struct(r)) => pinned_leftovers(s, r, &p, out),
            _ => {}
        }
    }
}

/// Node alignment is layout, not bytes: the schema keeps the weakest
/// alignment seen per group, the definitions one per element type.
fn without_align(t: &LooseTag) -> LooseTag {
    let mut t = t.clone();
    if let Body::Tree(tr) = &mut t.body {
        for n in &mut tr.nodes {
            n.align = 0;
        }
    }
    t
}

#[derive(Default)]
struct GroupReport {
    tags: usize,
    decoded: usize,
    exact: usize,
    canonical: usize,
    coverage: Coverage,
    failures: Vec<String>,
    pad_nonzero: BTreeMap<String, usize>,
}

/// Every retail tag of every defined group decodes, re-encodes
/// byte-identical, and holds no pinned leftovers; prints byte coverage.
#[test]
#[ignore = "needs the owner's disc"]
fn every_retail_tag_round_trips() -> Res<()> {
    let skeleton_mode = std::env::var_os("H2_DEFS_SKELETON").is_some();
    let annotate = std::env::var_os("H2_DEFS_ANNOTATE").is_some();
    let mut defs = Defs::builtin().clone();
    let mut extra_groups = HashSet::new();
    if let Ok(files) = std::env::var("H2_DEFS_EXTRA") {
        for f in files.split(',').filter(|f| !f.is_empty()) {
            let mut one = Defs {
                structs: defs.structs.clone(),
                groups: Default::default(),
            };
            one.parse_into(f, &std::fs::read_to_string(f)?)?;
            extra_groups.extend(one.groups.keys().copied());
            defs.structs = one.structs;
            defs.groups.extend(one.groups);
        }
    }
    let defs = &defs;
    let schema = Schema::builtin();
    let groups = if !extra_groups.is_empty() && std::env::var_os("H2_DEFS_GROUPS").is_none() {
        extra_groups
    } else {
        picked(defs, skeleton_mode)
    };
    let mut use_defs: BTreeMap<FourCC, StructDef> = BTreeMap::new();
    for g in &groups {
        let d = if skeleton_mode {
            skeleton::from_schema(&defs::fourcc_text(*g), &schema.groups[g])
        } else {
            defs.groups[g].clone()
        };
        use_defs.insert(*g, d);
    }
    let mut reports: BTreeMap<FourCC, GroupReport> = BTreeMap::new();
    let mut stats = skeleton::Stats::new();
    let started = std::time::Instant::now();
    let totals = each_tag(&groups, |stem, tag| {
        let g = tag.key.group;
        let def = &use_defs[&g];
        let r = reports.entry(g).or_default();
        r.tags += 1;
        let rec = match defs::decode(def, tag) {
            Ok(rec) => rec,
            Err(e) => {
                r.failures.push(format!("{stem}: {}: decode: {e}", tag.key));
                return;
            }
        };
        r.decoded += 1;
        let gt = defs::fourcc_text(g);
        skeleton::collect(def, &rec, &gt, &mut stats);
        let mut left = Vec::new();
        pinned_leftovers(def, &rec, &gt, &mut left);
        if !left.is_empty() {
            r.failures.push(format!(
                "{stem}: {}: pinned values at {}",
                tag.key,
                left.join(", ")
            ));
        }
        r.coverage.add(&defs::coverage(def, &rec));
        let region = tag.tree().map(|t| t.region).unwrap_or_default();
        match defs::encode(def, tag.key.clone(), &rec, region) {
            Ok(back) => {
                if defs::canonical(&back) == defs::canonical(tag) {
                    r.canonical += 1;
                } else {
                    let (x, y) = (defs::canonical(&back), defs::canonical(tag));
                    let at = x
                        .iter()
                        .zip(&y)
                        .position(|(a, b)| a != b)
                        .unwrap_or(x.len().min(y.len()));
                    r.failures.push(format!(
                        "{stem}: {}: re-encoded bytes differ at canonical byte {at} of {} / {}",
                        tag.key,
                        x.len(),
                        y.len()
                    ));
                    if std::env::var_os("H2_DEFS_DUMP").is_some() {
                        println!("{:#?}\n{:#?}", back, tag);
                    }
                }
                if without_align(&back) == without_align(tag) {
                    r.exact += 1;
                } else if std::env::var_os("H2_DEFS_DUMP_NODES").is_some() && r.tags - r.exact < 3 {
                    let (a, b) = (back.tree().expect("tree"), tag.tree().expect("tree"));
                    println!(
                        "{}: nodes {} encoded, {} retail",
                        tag.key,
                        a.nodes.len(),
                        b.nodes.len()
                    );
                }
            }
            Err(e) => r.failures.push(format!("{stem}: {}: encode: {e}", tag.key)),
        }
    })?;
    // Padding claims: zero in every instance.
    for (p, s) in &stats {
        if p.contains("/pad_") && s.zero != s.n {
            let g = defs::parse_fourcc(p.split('/').next().unwrap_or("")).expect("group");
            reports
                .entry(g)
                .or_default()
                .pad_nonzero
                .insert(p.clone(), (s.n - s.zero) as usize);
        }
    }
    println!(
        "{} instances, {} distinct tags, {} aliases, {} refused by extraction, {:?}",
        totals.instances,
        totals.distinct,
        totals.aliases,
        totals.refused.len(),
        started.elapsed()
    );
    for r in totals.refused.iter().take(5) {
        println!("  refused {r}");
    }
    println!(
        "{:<5} {:>6} {:>8} {:>10} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "group",
        "tags",
        "decoded",
        "identical",
        "nodes",
        "named%",
        "typed%",
        "pad%",
        "unknown%",
        "data%"
    );
    let mut bad = 0;
    for (g, r) in &reports {
        let c = r.coverage;
        let pct = |x: u64| 100.0 * x as f64 / c.total().max(1) as f64;
        println!(
            "{:<5} {:>6} {:>8} {:>10} {:>6} {:>7.1}% {:>7.1}% {:>7.1}% {:>7.1}% {:>7.1}%",
            defs::fourcc_text(*g),
            r.tags,
            r.decoded,
            r.canonical,
            r.exact,
            pct(c.named),
            pct(c.typed),
            pct(c.pad),
            pct(c.unknown),
            pct(c.data)
        );
        let def = &use_defs[g];
        let own = def.coverage();
        println!(
            "      root {:#x}: {} named, {} typed, {} pad, {} unknown bytes; agreement with the schema: {}",
            def.size,
            own.named,
            own.typed,
            own.pad,
            own.unknown,
            match defs::agree(def, &schema.groups[g]) {
                Ok(a) => format!(
                    "{} missing, {} definition-only, {} alignment notes",
                    a.missing.len(),
                    a.extra.len(),
                    a.align.len()
                ),
                Err(e) => e,
            }
        );
        for f in r.failures.iter().take(8) {
            println!("    {f}");
        }
        for (p, n) in &r.pad_nonzero {
            println!("    padding {p} nonzero in {n} instances");
        }
        if !skeleton_mode {
            bad += r.failures.len() + r.pad_nonzero.len();
        }
    }
    if skeleton_mode || annotate {
        for g in &groups {
            let d = skeleton::annotate(&use_defs[g], &defs::fourcc_text(*g), &stats);
            let mut text = format!(
                "group {} {:#x} align {}\n",
                defs::fourcc_text(*g),
                d.size,
                d.align
            );
            d.fields_text(2, &mut text);
            println!("\n{text}");
        }
    }
    assert_eq!(bad, 0, "definition failures");
    Ok(())
}
