//! `h2 collision`: summarise, validate and export `coll` and `phmo` tags.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{FourCC, TagId};
use tags::collision::{COLL, Collision, PHMO, PhysicsModel};

use crate::safe_out::OutDir;

const USAGE: &str = "usage: h2 collision <map-name-or-file> <tag name> [--group coll|phmo] [--obj FILE] [--force] [--disc disc.iso]\n       h2 collision --all <map-name-or-file|all> [--disc disc.iso]\n\nWith a tag name, prints a structural summary and validation result of the\ncoll and/or phmo tag with that name (--group picks one when both exist).\n--obj writes a Wavefront OBJ to FILE (never overwriting without --force,\nrefusing symlinked directories); it needs a single group, so pass --group when\nthe name exists as both. With --all, validates every coll and phmo tag in the\nmap (or in every map on the disc) and exits non-zero if any fails.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut disc, mut all) = (Vec::new(), None, false);
    let (mut group, mut obj, mut force) = (None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--force" => force = true,
            "--group" => {
                group = Some(
                    match args.next().context("--group needs coll or phmo")?.as_str() {
                        "coll" => COLL,
                        "phmo" => PHMO,
                        g => bail!("--group must be coll or phmo, not {g}"),
                    },
                )
            }
            "--obj" => {
                obj = Some(PathBuf::from(
                    args.next().context("--obj needs a file path")?,
                ))
            }
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
        (true, [map]) if obj.is_none() => validate_all(map, disc),
        (false, [map, tag]) => one(map, tag, group, obj, force, disc),
        _ => bail!("{USAGE}"),
    }
}

fn find(
    map: &map::Map<std::io::BufReader<std::fs::File>>,
    name: &str,
    group: Option<FourCC>,
) -> Vec<(TagId, FourCC)> {
    map.tags()
        .iter()
        .filter(|t| (t.group == COLL || t.group == PHMO) && group.is_none_or(|g| g == t.group))
        .filter(|t| t.name.as_deref() == Some(name))
        .map(|t| (t.id, t.group))
        .collect()
}

fn one(
    name: &str,
    tag: &str,
    group: Option<FourCC>,
    obj: Option<PathBuf>,
    force: bool,
    disc: Option<PathBuf>,
) -> Result<()> {
    let mut map = crate::maps::load_map(name, disc)?;
    let found = find(&map, tag, group);
    if found.is_empty() {
        bail!("no coll or phmo tag named {tag:?} in {name}");
    }
    if obj.is_some() && found.len() > 1 {
        bail!("{tag:?} exists as both coll and phmo; pass --group to choose what to export");
    }
    let mut failed = false;
    for (id, g) in found {
        let (text, problems, exported) = if g == COLL {
            let c = Collision::read(&mut map, id)?;
            let problems = c.validate();
            let exported = export(&obj, &problems, || c.to_obj());
            (summarise_coll(&c), problems, exported)
        } else {
            let p = PhysicsModel::read(&mut map, id)?;
            let problems = p.validate();
            let exported = export(&obj, &problems, || p.to_obj());
            (summarise_phmo(&p), problems, exported)
        };
        println!("{id} {g} {tag}\n{text}");
        if problems.is_empty() {
            println!("validation: ok");
        } else {
            failed = true;
            println!("validation: {} problem(s)", problems.len());
            for p in problems.iter().take(20) {
                println!("  {p}");
            }
            if obj.is_some() {
                println!("not exporting: the model failed validation");
            }
        }
        if let (Some(path), Some(text)) = (&obj, exported) {
            write_obj(path, &text?, force)?;
        }
    }
    if failed {
        bail!("validation failed");
    }
    Ok(())
}

/// Build the OBJ text only for a model that validated and when one was asked
/// for, so a bad model never produces (or replaces) an output file.
fn export(
    obj: &Option<PathBuf>,
    problems: &[String],
    build: impl FnOnce() -> tags::Result<String>,
) -> Option<Result<String>> {
    (obj.is_some() && problems.is_empty()).then(|| build().map_err(Into::into))
}

/// Split `path` into a root and normal components so that every directory
/// below the root is created or verified one level at a time, never through a
/// symlink. Relative paths are rooted at `.`, absolute ones at the filesystem
/// root (so a symlinked ancestor such as `/tmp` is refused; canonicalise first).
fn out_parts(path: &Path) -> Result<(PathBuf, Vec<String>)> {
    use std::path::Component;
    let mut root = PathBuf::from(".");
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::RootDir => root = PathBuf::from(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::Normal(n) => parts.push(
                n.to_str()
                    .context("--obj path must be valid UTF-8")?
                    .to_owned(),
            ),
            _ => bail!("--obj path may not contain `..` or a drive prefix"),
        }
    }
    if parts.is_empty() {
        bail!("--obj needs a file name");
    }
    Ok((root, parts))
}

fn write_obj(path: &Path, text: &str, force: bool) -> Result<()> {
    let (root, parts) = out_parts(path)?;
    let written = OutDir::new(root, force)?.write(&parts, text.as_bytes())?;
    println!("wrote {} ({} bytes)", written.display(), text.len());
    Ok(())
}

fn bounds(points: impl Iterator<Item = [f32; 3]>) -> String {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut n = 0;
    for p in points {
        n += 1;
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    if n == 0 {
        "none".into()
    } else {
        format!(
            "[{:.3} {:.3} {:.3}] .. [{:.3} {:.3} {:.3}]",
            lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        )
    }
}

fn summarise_coll(c: &Collision) -> String {
    let mut s = format!(
        "coll: {} materials, {} regions, {} nodes, {} pathfinding spheres\n",
        c.materials.len(),
        c.regions.len(),
        c.nodes.len(),
        c.pathfinding_spheres.len()
    );
    for (ri, r) in c.regions.iter().enumerate() {
        for (pi, p) in r.permutations.iter().enumerate() {
            for (bi, b) in p.bsps.iter().enumerate() {
                s += &format!(
                    "  region {ri} perm {pi} bsp {bi}: {} nodes, {} planes, {} leaves, {} refs, {} 2d nodes, {} surfaces, {} edges, {} vertices; bounds {}\n",
                    b.nodes.len(),
                    b.planes.len(),
                    b.leaves.len(),
                    b.refs.len(),
                    b.nodes_2d.len(),
                    b.surfaces.len(),
                    b.edges.len(),
                    b.vertices.len(),
                    bounds(b.vertices.iter().map(|v| v.point))
                );
            }
        }
    }
    s
}

fn summarise_phmo(p: &PhysicsModel) -> String {
    let mut s = format!(
        "phmo: mass {}, {} rigid bodies, {} materials, {} nodes, {} regions\n  shapes: {} spheres, {} pills, {} boxes, {} polyhedra ({} four-vector groups, {} planes), {} lists ({} external children), {} mopps ({} code bytes), {} type-5, {} triangles, {} type-6\n  constraints: {} +0xb8, {} +0xe8, {} +0xf0\n",
        p.mass,
        p.rigid_bodies.len(),
        p.materials.len(),
        p.nodes.len(),
        p.regions.len(),
        p.spheres.len(),
        p.pills.len(),
        p.boxes.len(),
        p.polyhedra.len(),
        p.four_vector_groups,
        p.plane_equations,
        p.lists.len(),
        p.list_children,
        p.mopps.len(),
        p.mopp_code.len(),
        p.type5_count,
        p.triangle_count,
        p.type6.len(),
        p.constraints_b8.len(),
        p.constraints_e8.len(),
        p.constraints_f0.len()
    );
    for (i, b) in p.rigid_bodies.iter().enumerate() {
        s += &format!(
            "  rigid body {i}: node {}, shape {:?} #{}, mass {}, com {:?}\n",
            b.node, b.shape.kind, b.shape.index, b.mass, b.center_of_mass
        );
    }
    s
}

#[derive(Default)]
struct Totals {
    maps: usize,
    colls: usize,
    phmos: usize,
    bsps: usize,
    surfaces: usize,
    failures: Vec<String>,
}

fn validate_map(name: &str, disc: Option<PathBuf>, t: &mut Totals) -> Result<()> {
    let mut map = crate::maps::load_map(name, disc)?;
    t.maps += 1;
    let tags: Vec<_> = map
        .tags()
        .iter()
        .filter(|x| (x.group == COLL || x.group == PHMO) && x.location.is_some())
        .map(|x| (x.id, x.group, x.name.clone().unwrap_or_default()))
        .collect();
    for (id, g, tag) in tags {
        let (problems, label) = if g == COLL {
            t.colls += 1;
            match Collision::read(&mut map, id) {
                Ok(c) => {
                    t.bsps += c.bsps().count();
                    t.surfaces += c.bsps().map(|b| b.surfaces.len()).sum::<usize>();
                    (c.validate(), "coll")
                }
                Err(e) => (vec![format!("decode: {e}")], "coll"),
            }
        } else {
            t.phmos += 1;
            match PhysicsModel::read(&mut map, id) {
                Ok(p) => (p.validate(), "phmo"),
                Err(e) => (vec![format!("decode: {e}")], "phmo"),
            }
        };
        for p in problems.into_iter().take(3) {
            t.failures.push(format!("{name} {label} {tag}: {p}"));
        }
    }
    Ok(())
}

fn validate_all(name: &str, disc: Option<PathBuf>) -> Result<()> {
    let mut t = Totals::default();
    if name == "all" {
        let disc_path = disc.clone().map(Ok).unwrap_or_else(super::default_disc)?;
        let mut iso = super::open_iso(&disc_path)?;
        let maps: Vec<String> = iso
            .entries()?
            .iter()
            .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
            .map(|e| e.path.clone())
            .collect();
        for m in maps {
            validate_map(&m, Some(disc_path.clone()), &mut t)?;
        }
    } else {
        validate_map(name, disc, &mut t)?;
    }
    println!(
        "{} map(s): {} coll ({} bsps, {} surfaces), {} phmo, {} failure(s)",
        t.maps,
        t.colls,
        t.bsps,
        t.surfaces,
        t.phmos,
        t.failures.len()
    );
    for f in t.failures.iter().take(50) {
        println!("  {f}");
    }
    if !t.failures.is_empty() {
        bail!("{} coll/phmo validation failure(s)", t.failures.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn arguments_are_validated_before_any_io() {
        assert!(run(vec!["--help".into()]).is_ok());
        assert!(run(vec!["--bogus".into()]).is_err());
        assert!(run(vec![]).is_err());
        // --obj makes no sense for a whole-map validation
        assert!(
            run(vec![
                "--all".into(),
                "x".into(),
                "--obj".into(),
                "y.obj".into()
            ])
            .is_err()
        );
        assert!(
            run(vec![
                "a".into(),
                "b".into(),
                "--group".into(),
                "bitm".into()
            ])
            .is_err()
        );
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;

    #[test]
    fn failed_validation_never_produces_output() {
        let obj = Some(PathBuf::from("x.obj"));
        let built = std::cell::Cell::new(false);
        let r = export(&obj, &["bad".to_string()], || {
            built.set(true);
            Ok(String::new())
        });
        assert!(r.is_none() && !built.get());
        assert!(export(&obj, &[], || Ok("ok".into())).is_some());
        assert!(export(&None, &[], || Ok("ok".into())).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_directory_is_refused() {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("h2-coll-out-{}", std::process::id()));
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, base.join("link")).unwrap();
        assert!(write_obj(&base.join("link").join("m.obj"), "x", true).is_err());
        assert!(!real.join("m.obj").exists());
        assert!(write_obj(&base.join("fresh").join("m.obj"), "x", false).is_ok());
        assert!(write_obj(&base.join("fresh").join("m.obj"), "y", false).is_err());
        assert!(out_parts(Path::new("a/../b.obj")).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
