//! Extraction on compile, the CLI's side (docs/engine/asset-store.md,
//! "Extraction on compile"): `h2 build` runs [`on_build`] after the image
//! is built, and `h2 play`, `h2 run` and `h2 serve` run [`before_boot`]
//! beside `package_prepare::before_boot`.
//!
//! Both call `tags::repo::compile::prepare` on the store at `<data>/tags`.
//! With the disc image there, a store with nothing stale opens no map: each
//! map file's recorded hash is trusted while its size and modification time
//! match. Without the image the maps are read from the store itself, which
//! is enough for a layout or version change; a map the store has no
//! skeleton for needs the image, and the error names it.
//!
//! The first extraction also copies the disc's files that are not maps
//! (the executable, the movies, the fonts) under `<data>/disc/`
//! ([`copy_disc_files`]). The image itself is never written or deleted.
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use tags::repo::catalogue;
use tags::repo::compile::{self, DiscMap, Input, Options, Prepared};
use tags::repo::extract::{MapWork, Plan, Why};
use tags::schema::Schema;

/// The store of a data root.
pub fn store_root(data: &Path) -> PathBuf {
    data.join("tags")
}

/// Where the disc's non-map files are kept.
pub fn disc_files_dir(data: &Path) -> PathBuf {
    data.join("disc")
}

/// The map files of a disc: an image's `.map` entries, or a lone `.map`
/// file with the shared caches beside it.
pub fn disc_maps(disc: &Path) -> Result<Vec<DiscMap>> {
    let mut out: Vec<DiscMap> = Vec::new();
    let is_map = disc
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("map"));
    if is_map {
        let mut add = |p: &Path| -> Result<()> {
            let stem = p
                .file_stem()
                .context("map file name")?
                .to_string_lossy()
                .into_owned();
            if !out.iter().any(|m| m.stem == stem) {
                out.push(DiscMap {
                    stem,
                    path: p.to_path_buf(),
                    base: 0,
                    len: fs::metadata(p)?.len(),
                });
            }
            Ok(())
        };
        add(disc)?;
        for s in ["mainmenu", "shared", "single_player_shared"] {
            let q = disc.with_file_name(format!("{s}.map"));
            if q.is_file() {
                add(&q)?;
            }
        }
        return Ok(out);
    }
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc).with_context(|| format!("opening {}", disc.display()))?,
    ))
    .with_context(|| format!("reading {}", disc.display()))?;
    for e in iso.entries()? {
        let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
        let Some(stem) = file.strip_suffix(".map") else {
            continue;
        };
        if e.is_dir || out.iter().any(|m| m.stem == stem) {
            continue;
        }
        out.push(DiscMap {
            stem: stem.to_owned(),
            path: disc.to_path_buf(),
            base: iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
            len: e.size.into(),
        });
    }
    Ok(out)
}

/// What a plan is about, in a line.
fn plan_line(plan: &Plan, input: Input) -> String {
    let from = match input {
        Input::Disc => "the disc image",
        Input::Store => "the store itself",
    };
    let mut whole = 0;
    let mut groups = 0;
    for w in &plan.maps {
        match &w.groups {
            None => whole += 1,
            Some(_) => groups += 1,
        }
    }
    let why = if plan.reset {
        "the extractor or tag format changed"
    } else if plan.maps.iter().any(|w| w.why == Why::New) {
        "new maps"
    } else if plan.maps.iter().any(|w| w.why == Why::FileChanged) {
        "a map file changed"
    } else {
        "a layout changed"
    };
    format!(
        "{} maps whole, {groups} by group, from {from} ({why})",
        whole
    )
}

/// What writing the asset catalogue did, in a line: the assets the store
/// holds by hash against the files it keeps them as.
fn catalogue_line(built: &catalogue::Built) -> String {
    let (before, after) = built.totals();
    format!(
        "catalogue written: {} keys, {} assets, {} keys with more than one content (as files: {} files, {} keys with variants){}",
        after.keys,
        after.contents,
        after.split,
        before.contents,
        before.split,
        if built.problems.is_empty() {
            String::new()
        } else {
            format!("; {} tags not hashed", built.problems.len())
        }
    )
}

/// Bring the store of `data` up to date with the game disc `disc`: the
/// extraction (from the disc image when it is a file, else from the store),
/// then the disc's other files. Lines for a person go to `say`.
pub fn prepare(
    disc: &Path,
    data: &Path,
    verify: bool,
    say: &mut dyn FnMut(&str),
) -> Result<Prepared> {
    let maps = if disc.is_file() {
        Some(disc_maps(disc)?)
    } else {
        None
    };
    let root = store_root(data);
    let total = std::cell::Cell::new(0usize);
    let done = std::cell::Cell::new(0usize);
    let lines = std::cell::RefCell::new(Vec::<String>::new());
    let mut say_now = |s: &str| lines.borrow_mut().push(s.to_owned());
    let p = compile::prepare(
        &root,
        Schema::builtin(),
        maps.as_deref(),
        &Options {
            verify,
            ..Options::default()
        },
        &mut say_now,
        &mut |plan: &Plan, input| {
            total.set(plan.maps.len());
            if !plan.is_empty() {
                lines
                    .borrow_mut()
                    .push(format!("store: {}", plan_line(plan, input)));
            }
        },
        &mut |w: &MapWork, r| {
            done.set(done.get() + 1);
            lines.borrow_mut().push(format!(
                "store: [{}/{}] {}: {} tags ({} lossy, {} refused)",
                done.get(),
                total.get(),
                w.map,
                r.new + r.same + r.variants.len(),
                r.lossy.len(),
                r.refused.len()
            ));
        },
    )
    .with_context(|| format!("extracting into {}", root.display()));
    for l in lines.borrow().iter() {
        say(l);
    }
    let p = p?;
    if let Some(c) = &p.catalogue {
        say(&format!("store: {}", catalogue_line(c)));
    }
    if p.input == Input::Disc
        && !disc
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("map"))
    {
        let n = copy_disc_files(disc, &disc_files_dir(data))?;
        if n > 0 {
            say(&format!(
                "store: copied {n} files of the disc that are not maps to {}",
                disc_files_dir(data).display()
            ));
        }
    }
    Ok(p)
}

/// After `h2 build` has built the image: extract, with progress on
/// stderr. A failure is the build's failure.
pub fn on_build(disc: &Path, data: &Path) -> Result<()> {
    prepare(disc, data, false, &mut |s| eprintln!("{s}"))?;
    Ok(())
}

/// Before the game boots. Never fails: a problem is a line, and the game
/// starts with what is there.
pub fn before_boot(disc: &Path, data: &Path) {
    let r = catch_unwind(AssertUnwindSafe(|| {
        prepare(disc, data, false, &mut |s| eprintln!("{s}"))
    }));
    match r {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => eprintln!("store: {e:#}; the game starts with what is there"),
        Err(_) => eprintln!("store: extraction stopped; the game starts with what is there"),
    }
}

/// The path under `<data>/disc/` of a disc entry, or `None` for one that
/// names nothing or leaves the directory.
pub fn disc_file_path(entry: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in entry.split(['/', '\\']).filter(|p| !p.is_empty()) {
        if part == "." || part == ".." || part.contains(':') {
            return None;
        }
        out.push(part);
    }
    (out.components().count() > 0).then_some(out)
}

const COPIED: &str = "copied.txt";

/// The identity of the image the copy was made from: its size and
/// modification time.
fn image_stamp(iso: &Path) -> Result<String> {
    let md = fs::metadata(iso)?;
    let mtime = md
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(format!("image {} {mtime}", md.len()))
}

/// Copy every file of the image that is not a map to `dest`, once per
/// image: `dest/copied.txt` holds the image's stamp, written after the
/// last file, so a stopped copy continues (a file already there at its
/// size is kept). Returns the number of files written.
pub fn copy_disc_files(iso: &Path, dest: &Path) -> Result<usize> {
    let stamp = image_stamp(iso)?;
    let marker = dest.join(COPIED);
    if fs::read_to_string(&marker)
        .ok()
        .and_then(|t| t.lines().next().map(str::to_owned))
        .is_some_and(|l| l == stamp)
    {
        return Ok(0);
    }
    let mut image = xiso::Xiso::open(BufReader::new(File::open(iso)?))
        .with_context(|| format!("reading {}", iso.display()))?;
    let mut written = 0;
    let mut listing = String::new();
    for e in image.entries()? {
        if e.is_dir || e.path.to_ascii_lowercase().ends_with(".map") {
            continue;
        }
        let Some(rel) = disc_file_path(&e.path) else {
            continue;
        };
        let target = dest.join(&rel);
        listing.push_str(&format!("{}\t{}\n", e.size, rel.display()));
        if fs::metadata(&target).is_ok_and(|m| m.len() == u64::from(e.size)) {
            continue;
        }
        if let Some(d) = target.parent() {
            fs::create_dir_all(d)?;
        }
        let part = target.with_extension("part");
        let mut out = File::create(&part).with_context(|| format!("writing {}", part.display()))?;
        image
            .copy_to(&e, &mut out)
            .with_context(|| format!("copying {}", e.path))?;
        out.flush()?;
        drop(out);
        fs::rename(&part, &target)?;
        written += 1;
    }
    fs::create_dir_all(dest)?;
    fs::write(&marker, format!("{stamp}\n{listing}"))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disc_entry_stays_under_the_directory() {
        assert_eq!(
            disc_file_path("\\movies\\intro.bik"),
            Some(PathBuf::from("movies").join("intro.bik"))
        );
        assert_eq!(
            disc_file_path("/default.xbe"),
            Some(PathBuf::from("default.xbe"))
        );
        assert_eq!(disc_file_path("..\\outside"), None);
        assert_eq!(disc_file_path("a/../../b"), None);
        assert_eq!(disc_file_path("C:\\x"), None);
        assert_eq!(disc_file_path("\\"), None);
    }

    #[test]
    fn a_lone_map_file_brings_its_shared_caches() {
        let dir = std::env::temp_dir().join(format!("h2-store-prepare-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (n, len) in [("lockout", 5usize), ("shared", 7), ("other", 2)] {
            fs::write(dir.join(format!("{n}.map")), vec![0u8; len]).unwrap();
        }
        let maps = disc_maps(&dir.join("lockout.map")).unwrap();
        let got: Vec<(&str, u64)> = maps.iter().map(|m| (m.stem.as_str(), m.len)).collect();
        assert_eq!(got, [("lockout", 5), ("shared", 7)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plan_line_says_why_and_how_much() {
        let work = |why, groups| MapWork {
            map: "m".into(),
            file: "f".into(),
            why,
            groups,
        };
        let plan = Plan {
            reset: false,
            maps: vec![work(Why::New, None), work(Why::New, None)],
        };
        assert_eq!(
            plan_line(&plan, Input::Disc),
            "2 maps whole, 0 by group, from the disc image (new maps)"
        );
        let plan = Plan {
            reset: true,
            maps: vec![work(Why::Versions, None)],
        };
        assert!(plan_line(&plan, Input::Store).contains("the store itself"));
    }

    #[test]
    fn the_catalogue_line_counts_assets_against_files() {
        let sounds = tags::defs::parse_fourcc("snd!").unwrap();
        let built = catalogue::Built {
            before: [(
                sounds,
                catalogue::Variants {
                    keys: 2,
                    contents: 4,
                    split: 2,
                    instances: 6,
                    others: 2,
                },
            )]
            .into_iter()
            .collect(),
            after: [(
                sounds,
                catalogue::Variants {
                    keys: 2,
                    contents: 3,
                    split: 1,
                    instances: 6,
                    others: 1,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert_eq!(
            catalogue_line(&built),
            "catalogue written: 2 keys, 3 assets, 1 keys with more than one content (as files: 4 files, 2 keys with variants)"
        );
        let mut bad = built.clone();
        bad.problems.push("m: t: why".into());
        assert!(catalogue_line(&bad).ends_with("; 1 tags not hashed"));
    }
}
