//! The stage built on the owner's disc. Opt-in: `H2_DISC` names the ISO;
//! `H2_STAGE_BASE` the base map (default: the project's, `lockout`).
//! Everything is written to a temporary directory, deleted afterwards.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo xtask test -p content-kit --test stage_disc -- --ignored --nocapture
//! ```

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;

use map::{Map, ResourceFile};
use content_kit::stage::map::{Base, Project, build, package};
use tags::repo::{MapSource, SharedFiles, TagKey, TagSource};
use tags::schema::Schema;

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type DiscMap = Map<BufReader<File>>;

/// Every map on the disc: (stem, offset, size).
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
            "mainmenu" => ResourceFile::MainMenu,
            "shared" => ResourceFile::Shared,
            "single_player_shared" => ResourceFile::SinglePlayerShared,
            _ => continue,
        };
        s.set(schema, file, open(*base, *size)?)?;
    }
    Ok(s)
}

/// The stage over the disc's base map: it builds under the tag-memory
/// bound, every reference in it resolves, its package resolves through
/// the content library, and a second build gives the same bytes.
#[test]
#[ignore = "needs the owner's disc"]
fn the_stage_builds_on_the_disc() -> Res<()> {
    let mut project = Project::builtin()?;
    if let Ok(b) = std::env::var("H2_STAGE_BASE") {
        project.base = b;
    }
    let maps = disc_maps()?;
    let (_, at, size) = maps
        .iter()
        .find(|(s, ..)| *s == project.base)
        .ok_or_else(|| format!("{}.map is not on the disc", project.base))?
        .clone();
    let schema = Schema::builtin();
    let mut sh = shared(schema, &maps)?;
    let mut src = MapSource::new(schema, open(at, size)?, &mut sh)?;
    let base = Base::from_map(&mut src, &project.base)?;
    let started = std::time::Instant::now();
    let built = build(&mut src, &base, &project)?;
    let r = &built.report;
    println!(
        "stage on {}: {} bytes, {} tags ({} from the base), tag memory {:#x} of {:#x}, {} triangles, {} collision surfaces, {:.1?}",
        base.map,
        built.bytes.len(),
        r.tags,
        r.from_base,
        r.memory_size,
        r.memory_max,
        r.triangles,
        r.collision_surfaces,
        started.elapsed()
    );
    println!("scenario: {:?}", r.scenario_edit);
    println!("gestalt: {:?} {:?}", r.gestalt, r.gestalt_whole);
    assert!(r.memory_size <= r.memory_max, "tag memory over the bound");
    assert_eq!(r.bsps, 1);

    // Every reference in the built map names a tag it holds.
    let mut none = SharedFiles::default();
    none.unread_ok = true;
    let read = Map::from_reader(std::io::Cursor::new(built.bytes.clone()))?;
    let mut back = MapSource::new(schema, read, &mut none)?;
    let keys: HashSet<TagKey> = back.keys().into_iter().collect();
    let mut dangling = Vec::new();
    for k in &keys {
        let Some(t) = back.read(k)? else {
            dangling.push(format!("{k} does not read back"));
            continue;
        };
        for r in t.references() {
            if !keys.contains(&r) {
                dangling.push(format!("{k} -> {r}"));
            }
        }
    }
    assert!(
        dangling.is_empty(),
        "{} dangling: {:?}",
        dangling.len(),
        &dangling[..dangling.len().min(10)]
    );

    // The package resolves to the map.
    let dir = std::env::temp_dir().join(format!("stage-kit-disc-{}", std::process::id()));
    let data = dir.join("data");
    let written = package(&built, &project, &base, &data)?;
    let mut lib = content::Library::open(content::store::maps_dir(&data));
    let resolved = lib
        .resolve(&project.name, &mut &b""[..])?
        .ok_or("the stage package does not resolve")?;
    assert_eq!(resolved.len(), built.bytes.len() as u64);
    assert!(written.join("report.json").exists());
    std::fs::remove_dir_all(&dir)?;

    // The same inputs, the same bytes.
    let again = build(&mut src, &base, &project)?;
    assert_eq!(again.bytes, built.bytes, "a second build differs");
    Ok(())
}
