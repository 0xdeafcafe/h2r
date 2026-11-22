//! `h2 object`: print an object-family tag with its model chain, and validate
//! every object-family tag in one map or all maps. Self-contained so it
//! merges independently of the other map commands. Writes nothing.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use map::{FourCC, Map, TagId, TagQuery};
use tags::object::{
    Context, DecodedTag, GROUPS, GroupDef, Record, ResolvedModel, Value, VariantChoice,
    is_object_group,
};
use tags::wire::TagReference;

const USAGE: &str = "usage: h2 object <map-name-or-file> <tag name> [group] [--disc disc.iso]
       h2 object --all <map-name-or-file|all> [--disc disc.iso]

Prints an object-family tag (bipd vehi weap eqip garb proj scen mach ctrl ssce
bloc crea, hlmt, itmc, vehc): the shared obje header, the hlmt it uses, the
render/collision/physics/animation tags, the default variant's region and
permutation choices, then every reviewed field with its evidence label
(E engine, V disc, D design). --all decodes every object-family tag, prints
counts, and exits non-zero if any tag fails.";

type DiscMap = Map<BufReader<File>>;

fn disc_maps(disc: &Path) -> Result<Vec<(String, u64, u64)>> {
    let mut iso = super::open_iso(disc)?;
    let part = iso.partition_offset();
    let mut out: Vec<_> = iso
        .entries()?
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        .map(|e| {
            (
                e.path.trim_start_matches('/').to_owned(),
                part + u64::from(e.sector) * xiso::SECTOR,
                u64::from(e.size),
            )
        })
        .collect();
    out.sort();
    Ok(out)
}

fn open_one(name: &str, disc: Option<&PathBuf>) -> Result<(String, DiscMap)> {
    let local = Path::new(name);
    if local.is_file() {
        let m = Map::from_reader(BufReader::new(File::open(local)?))
            .with_context(|| format!("parsing {name}"))?;
        return Ok((name.to_owned(), m));
    }
    let disc = match disc {
        Some(d) => d.clone(),
        None => super::default_disc()?,
    };
    let mut want = name.trim_start_matches('/').to_ascii_lowercase();
    if !want.ends_with(".map") {
        want.push_str(".map");
    }
    if !want.contains('/') {
        want = format!("maps/{want}");
    }
    let (path, base, size) = disc_maps(&disc)?
        .into_iter()
        .find(|(p, ..)| p.eq_ignore_ascii_case(&want))
        .with_context(|| format!("{want} not on disc"))?;
    let m = Map::from_region(BufReader::new(File::open(&disc)?), base, size)
        .with_context(|| format!("parsing {path}"))?;
    Ok((path, m))
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut disc, mut all) = (Vec::new(), None, false);
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
        (true, [m]) => validate(m, disc.as_ref()),
        (false, [m, tag]) => show(m, tag, None, disc.as_ref()),
        (false, [m, tag, group]) => show(m, tag, Some(group), disc.as_ref()),
        _ => bail!("{USAGE}"),
    }
}

fn find_tag(map: &DiscMap, name: &str, group: Option<&String>) -> Result<TagId> {
    if let Some(g) = group {
        let g: FourCC = g.parse()?;
        return Ok(map.tag(TagQuery::NameInGroup(name, g))?.id);
    }
    let hits: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.name.as_deref() == Some(name) && GroupDef::find(t.group).is_some())
        .collect();
    // An object and its same-named hlmt are the usual pair: prefer the object.
    let objects: Vec<_> = hits.iter().filter(|t| is_object_group(t.group)).collect();
    if let [t] = objects.as_slice() {
        return Ok(t.id);
    }
    match hits.as_slice() {
        [t] => Ok(t.id),
        [] => bail!("no object-family tag named {name:?}"),
        _ => bail!(
            "{name:?} names several object-family tags ({}); add the group",
            hits.iter()
                .map(|t| t.group.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn show(map_name: &str, tag: &str, group: Option<&String>, disc: Option<&PathBuf>) -> Result<()> {
    let (_, mut map) = open_one(map_name, disc)?;
    let id = find_tag(&map, tag, group)?;
    let ctx = Context::read(&mut map)?;
    let group = ctx.catalog.get(id).expect("found").group;
    if is_object_group(group) {
        let r = ctx.resolve_object(id)?;
        let h = &r.header;
        let def = r.tag.def;
        println!("{} {} {}", group, id, r.tag.name.as_deref().unwrap_or("?"));
        println!(
            "chain        {} (root {:#x} bytes, payload {:#x})",
            chain(def),
            def.root_size,
            r.tag.size
        );
        println!(
            "object type  {} ({})",
            h.object_type,
            tags::object::OBJECT_TYPES[h.object_type as usize].group
        );
        println!(
            "bounds       radius {} offset {:?}",
            h.bounding_radius, h.bounding_offset
        );
        println!("model        {}", reference(&ctx, h.model));
        println!("creation     {}", reference(&ctx, h.creation_effect));
        println!(
            "blocks       {} attachments, {} widgets, {} functions, {} change colours, {} predicted resources",
            h.attachments.len(),
            h.widgets.len(),
            h.functions.len(),
            h.change_colors,
            h.predicted_resources.len()
        );
        for a in &h.attachments {
            println!(
                "  attachment {} at {}",
                reference(&ctx, a.kind),
                a.marker.as_deref().unwrap_or("(origin)")
            );
        }
        for w in &h.widgets {
            println!("  widget     {}", reference(&ctx, *w));
        }
        if let Some(m) = &r.model {
            print_model(m);
        }
        println!();
        for layer in &r.tag.layers {
            print_record(&ctx, layer, 0);
        }
    } else {
        let t = ctx.decode(id)?;
        println!("{} {} {}", group, id, t.name.as_deref().unwrap_or("?"));
        if group == FourCC(*b"hlmt") {
            print_model(&ctx.resolve_model(id, tags::wire::StringId(0))?);
            println!();
        }
        for layer in &t.layers {
            print_record(&ctx, layer, 0);
        }
    }
    Ok(())
}

fn chain(def: &GroupDef) -> String {
    let mut v: Vec<&str> = vec![def.group];
    v.extend(def.ancestry());
    v.join(" -> ")
}

fn reference(ctx: &Context, r: TagReference) -> String {
    if r.id == TagId::NULL {
        return if r.group.as_u32() == u32::MAX {
            "none".into()
        } else {
            format!("none ({})", r.group)
        };
    }
    let name = ctx
        .catalog
        .get(r.id)
        .and_then(|t| t.name.clone())
        .unwrap_or_default();
    format!("{} {} {}", r.group, r.id, name)
}

fn print_model(m: &ResolvedModel) {
    let link = |l: &Option<tags::object::Linked>| match l {
        Some(l) => format!("{} {} {}", l.group, l.id, l.name.as_deref().unwrap_or("?")),
        None => "none".into(),
    };
    println!("hlmt         {} {}", m.id, m.name.as_deref().unwrap_or("?"));
    println!("  render     {}", link(&m.render_model));
    println!("  collision  {}", link(&m.collision_model));
    println!("  physics    {}", link(&m.physics_model));
    println!("  animation  {}", link(&m.animation));
    println!(
        "  {} variants, {} regions, {} nodes",
        m.variant_count, m.region_count, m.node_count
    );
    let v = match &m.variant {
        VariantChoice::First { index, name } => format!(
            "{index} {} (default name empty: first)",
            name.as_deref().unwrap_or("")
        ),
        VariantChoice::Named { index, name } => {
            format!("{index} {}", name.as_deref().unwrap_or(""))
        }
        VariantChoice::Missing(s) => format!("not found ({:#x}); the engine selects none", s.0),
        VariantChoice::None => "none (model has no variants)".into(),
    };
    println!("  variant    {v}");
    for r in &m.regions {
        let perms: Vec<String> = r
            .permutations
            .iter()
            .map(|p| {
                format!(
                    "{} p={}",
                    p.name.as_deref().unwrap_or("\"\""),
                    p.probability
                )
            })
            .collect();
        println!(
            "    region {:<12} (hlmt region {}): {}",
            r.region.as_deref().unwrap_or("?"),
            r.model_region,
            perms.join(", ")
        );
    }
}

fn print_record(ctx: &Context, r: &Record, depth: usize) {
    let pad = "  ".repeat(depth);
    println!("{pad}[{}] @ {:#x}", r.layout, r.address);
    for (f, v) in &r.fields {
        let ev = f.evidence.label();
        let text = match v {
            Value::U8(x) => x.to_string(),
            Value::U16(x) => format!("{x} ({x:#x})"),
            Value::I16(x) => x.to_string(),
            Value::U32(x) => format!("{x:#x}"),
            Value::F32(x) => x.to_string(),
            Value::Vec3(x) => format!("{x:?}"),
            Value::StringId(id, s) => match s {
                Some(s) => format!("{s:?}"),
                None if id.0 == 0 => "0".into(),
                None => "null".into(),
            },
            Value::TagRef(t) => reference(ctx, *t),
            Value::TagDatum(t) => {
                let tag = ctx.catalog.get(*t);
                format!(
                    "{} {} {}",
                    t,
                    tag.map_or("?".into(), |t| t.group.to_string()),
                    tag.and_then(|t| t.name.clone()).unwrap_or_default()
                )
            }
            Value::Bytes(b) => b
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            Value::OpaqueBlock { block, aliased } => {
                format!(
                    "{} elements @ {:#x}{} (element layout not reviewed)",
                    block.count,
                    block.address,
                    if *aliased { " aliased" } else { "" }
                )
            }
            Value::Block { block, aliased, .. } => {
                format!(
                    "{} elements @ {:#x}{}",
                    block.count,
                    block.address,
                    if *aliased { " aliased" } else { "" }
                )
            }
        };
        println!("{pad}  {:#05x} {:<24} {ev} {text}", f.offset, f.name);
        if let Value::Block { elements, .. } = v {
            for e in elements {
                print_record(ctx, e, depth + 2);
            }
        }
    }
    let opaque: u32 = r.opaque.iter().map(|(_, b)| b.len() as u32).sum();
    let nonzero = r
        .opaque
        .iter()
        .filter(|(_, b)| b.iter().any(|x| *x != 0))
        .count();
    if opaque > 0 {
        println!(
            "{pad}  ({opaque:#x} opaque bytes in {} spans, {nonzero} nonzero)",
            r.opaque.len()
        );
    }
}

#[derive(Default)]
struct Stats {
    ok: BTreeMap<String, usize>,
    failed: Vec<String>,
    aliased_blocks: usize,
    blocks: usize,
    variant: BTreeMap<&'static str, usize>,
    perms: (usize, usize),
    chain_checked: usize,
    linked: BTreeMap<&'static str, usize>,
    names: BTreeMap<String, std::collections::BTreeSet<String>>,
    first_child: (usize, usize),
    unmatched: BTreeMap<String, usize>,
    no_data: usize,
}

/// Lowest address of a nonempty block whose data lies in the owning tag.
fn first_child(r: &Record, low: &mut Option<u32>) {
    for (_, v) in &r.fields {
        match v {
            Value::Block {
                block,
                aliased,
                elements,
            } => {
                if block.count > 0 && !aliased {
                    *low = Some(low.map_or(block.address, |l| l.min(block.address)));
                }
                for e in elements {
                    first_child(e, low);
                }
            }
            Value::OpaqueBlock { block, aliased } if block.count > 0 && !aliased => {
                *low = Some(low.map_or(block.address, |l| l.min(block.address)));
            }
            _ => {}
        }
    }
}

fn count_blocks(r: &Record, s: &mut Stats) {
    for (_, v) in &r.fields {
        match v {
            Value::Block {
                block,
                aliased,
                elements,
            } => {
                if block.count > 0 {
                    s.blocks += 1;
                    s.aliased_blocks += usize::from(*aliased);
                }
                for e in elements {
                    count_blocks(e, s);
                }
            }
            Value::OpaqueBlock { block, aliased } if block.count > 0 => {
                s.blocks += 1;
                s.aliased_blocks += usize::from(*aliased);
            }
            _ => {}
        }
    }
}

fn check_chain(map: &DiscMap, t: &DecodedTag) -> Result<()> {
    let g = map
        .groups()
        .iter()
        .find(|g| g.class == t.group)
        .context("class missing from group table")?;
    let want = t.def.ancestry();
    let have: Vec<String> = [g.parent, g.grandparent]
        .into_iter()
        .flatten()
        .map(|c| c.to_string())
        .collect();
    if have != want {
        bail!("group table chain {have:?} differs from layer chain {want:?}");
    }
    Ok(())
}

fn validate_map(path: &str, mut map: DiscMap, s: &mut Stats) -> Result<()> {
    let ctx = Context::read(&mut map)?;
    let ids: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| GroupDef::find(t.group).is_some())
        .map(|t| (t.id, t.group, t.name.clone()))
        .collect();
    for (id, group, name) in ids {
        if ctx.catalog.get(id).is_some_and(|t| t.location.is_none()) {
            s.no_data += 1;
            continue;
        }
        let res: Result<()> = (|| {
            let t = ctx.decode(id)?;
            check_chain(&map, &t)?;
            s.chain_checked += 1;
            let mut low = None;
            for l in &t.layers {
                count_blocks(l, s);
                first_child(l, &mut low);
            }
            if let Some(low) = low {
                if low != t.layers[0].address + t.def.root_size {
                    bail!(
                        "first child block at {low:#x}, not at the end of the {:#x}-byte root",
                        t.def.root_size
                    );
                }
                s.first_child.0 += 1;
            } else {
                s.first_child.1 += 1;
            }
            if group == FourCC(*b"hlmt") {
                let root = &t.layers[0];
                let regions = root.elements("regions");
                for v in root.elements("variants") {
                    for vr in v.elements("regions") {
                        let mr = match vr.get("model_region") {
                            Some(Value::U8(i)) => regions.get(usize::from(*i)),
                            _ => None,
                        };
                        let names: Vec<_> = mr
                            .map(|r| {
                                r.elements("permutations")
                                    .iter()
                                    .filter_map(|p| p.sid("name").map(|x| x.0))
                                    .collect()
                            })
                            .unwrap_or_default();
                        for p in vr.elements("permutations") {
                            let (id, text) = p.sid("permutation").expect("layout");
                            s.perms.0 += 1;
                            if names.contains(&id) {
                                s.perms.1 += 1;
                            } else {
                                *s.unmatched
                                    .entry(text.unwrap_or("(empty)").to_owned())
                                    .or_default() += 1;
                            }
                        }
                    }
                }
            }
            let model = if is_object_group(group) {
                ctx.resolve_object(id)?.model
            } else if group == FourCC(*b"hlmt") {
                Some(ctx.resolve_model(id, tags::wire::StringId(0))?)
            } else {
                None
            };
            if let Some(m) = model
                && is_object_group(group)
            {
                *s.variant
                    .entry(match m.variant {
                        VariantChoice::First { .. } => "empty name: variant 0",
                        VariantChoice::Named { .. } => "named and found",
                        VariantChoice::Missing(_) => "named, not in hlmt",
                        VariantChoice::None => "hlmt has no variants",
                    })
                    .or_default() += 1;
                for (k, l) in [
                    ("mode", &m.render_model),
                    ("coll", &m.collision_model),
                    ("phmo", &m.physics_model),
                    ("jmad", &m.animation),
                ] {
                    *s.linked.entry(k).or_default() += usize::from(l.is_some());
                }
            }
            Ok(())
        })();
        match res {
            Ok(()) => {
                *s.ok.entry(group.to_string()).or_default() += 1;
                s.names
                    .entry(group.to_string())
                    .or_default()
                    .insert(name.unwrap_or_default());
            }
            Err(e) => s.failed.push(format!(
                "{path}: {group} {id} {}: {e}",
                name.unwrap_or_default()
            )),
        }
    }
    Ok(())
}

fn validate(which: &str, disc: Option<&PathBuf>) -> Result<()> {
    let mut s = Stats::default();
    let mut maps = 0;
    if which == "all" {
        let disc = match disc {
            Some(d) => d.clone(),
            None => super::default_disc()?,
        };
        for (path, base, size) in disc_maps(&disc)? {
            let m = Map::from_region(BufReader::new(File::open(&disc)?), base, size)
                .with_context(|| format!("parsing {path}"))?;
            validate_map(&path, m, &mut s)?;
            maps += 1;
        }
    } else {
        let (path, m) = open_one(which, disc)?;
        validate_map(&path, m, &mut s)?;
        maps = 1;
    }
    println!("{maps} maps");
    for g in &GROUPS {
        if let Some(n) = s.ok.get(g.group) {
            println!(
                "  {:<5} {n:>6} ok, {:>5} unique names",
                g.group,
                s.names.get(g.group).map_or(0, |x| x.len())
            );
        }
    }
    let total: usize = s.ok.values().sum();
    println!("decoded {total}, failed {}", s.failed.len());
    println!("tags without data in this cache (skipped): {}", s.no_data);
    println!(
        "class chains checked against group tables: {}",
        s.chain_checked
    );
    println!(
        "first child block exactly at the end of the root: {} tags ({} have no own child blocks)",
        s.first_child.0, s.first_child.1
    );
    println!(
        "nonempty blocks {} ({} aliased: data outside the owning tag)",
        s.blocks, s.aliased_blocks
    );
    println!("default variant of object tags:");
    for (k, n) in &s.variant {
        println!("  {k:<24} {n}");
    }
    println!("object models linking mode/coll/phmo/jmad: {:?}", s.linked);
    println!(
        "hlmt variant permutations named in their hlmt region: {}/{}",
        s.perms.1, s.perms.0
    );
    let mut un: Vec<_> = s.unmatched.iter().collect();
    un.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!(
        "  most common unmatched names: {:?}",
        un.iter().take(8).collect::<Vec<_>>()
    );
    for f in s.failed.iter().take(40) {
        println!("FAIL {f}");
    }
    outcome(&s)
}

/// The exit status of `--all`: any failed tag makes the command fail.
fn outcome(s: &Stats) -> Result<()> {
    if !s.failed.is_empty() {
        bail!("{} object-family tags failed", s.failed.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_failed_tag_fails_the_run() {
        let mut s = Stats::default();
        assert!(outcome(&s).is_ok());
        *s.ok.entry("scen".into()).or_default() += 5;
        assert!(outcome(&s).is_ok());
        s.failed
            .push("example.map: scen 0x00010001 example: invalid tag".into());
        assert!(outcome(&s).is_err());
    }
}
