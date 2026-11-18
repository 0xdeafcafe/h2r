//! `h2 bots nav`: build the bots' nav mesh and walls for disc maps, cached
//! under the data root (docs/game/bots.md, stage 2). Derived from the
//! user's disc, so the files stay on this machine.
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use map::TagId;
use tags::geometry::TagView;

const USAGE: &str = "usage: h2 bots nav <map-name-or-file|all> [--disc disc.iso] [--data DIR]\n\nBuilds each structure BSP's nav mesh and line-of-sight walls from its\ncollision and writes <data>/bots/<map>.<bsp>.bots, which the bots load\nwhen the map does. --data defaults to the data root `h2 play` uses.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("nav") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("unknown bots command {other}\n{USAGE}"),
    }
    let (mut positional, mut disc, mut data) = (Vec::new(), None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "--data" => {
                data = Some(PathBuf::from(
                    args.next().context("--data needs a directory")?,
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
    let [map] = positional.as_slice() else {
        bail!("{USAGE}")
    };
    let out = data
        .unwrap_or_else(content::store::default_data_root)
        .join("bots");
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let names = if map == "all" {
        all_maps(&disc)?
    } else {
        vec![map.clone()]
    };
    let mut failed = 0;
    for name in &names {
        if let Err(e) = one(name, disc.clone(), &out) {
            eprintln!("{name}: {e:#}");
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} of {} maps failed", names.len());
    }
    Ok(())
}

fn all_maps(disc: &Option<PathBuf>) -> Result<Vec<String>> {
    let disc_path = disc.clone().map(Ok).unwrap_or_else(super::default_disc)?;
    let mut iso = super::open_iso(&disc_path)?;
    Ok(iso
        .entries()?
        .iter()
        .filter(|e| {
            !e.is_dir
                && e.path.to_ascii_lowercase().starts_with("/maps/")
                && e.path.to_ascii_lowercase().ends_with(".map")
        })
        .map(|e| {
            e.path
                .rsplit('/')
                .next()
                .unwrap_or(&e.path)
                .trim_end_matches(".map")
                .to_owned()
        })
        .filter(|n| !n.contains('/'))
        .collect())
}

fn one(name: &str, disc: Option<PathBuf>, out: &std::path::Path) -> Result<()> {
    let mut map = crate::maps::load_map(name, disc)?;
    let short = map.header().name.clone();
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("the scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    // The scenario's structure BSPs: block +0x210, 0x44 each, the sbsp at +0x14.
    let bsps: Vec<TagId> = view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect();
    for (k, id) in bsps.into_iter().enumerate() {
        let tag = map.tag(id)?.clone();
        let loc = tag.location.context("the BSP has no payload")?;
        let b = map.read_tag_bytes(id)?;
        let v = TagView::new(&b, loc.address);
        let h = v.at(0, 0x200)?;
        let coll = v.block(
            h,
            tags::pathfinding::SBSP_COLLISION_FIELD,
            tags::collision::Bsp::STRUCTURE_SIZE,
        )?;
        let at = coll.first().context("the BSP has no collision")?.as_ptr() as usize
            - b.as_ptr() as usize;
        let bsp = tags::collision::Bsp::decode_structure(&b, loc.address, at)?;
        let data = bots::cache::build(&bsp).map_err(anyhow::Error::msg)?;
        let jumps = data
            .nav
            .edges
            .iter()
            .filter(|e| e.kind != bots::nav::Kind::Walk)
            .count();
        let file = out.join(bots::cache::file_name(&short, k));
        std::fs::write(&file, data.to_bytes())
            .with_context(|| format!("writing {}", file.display()))?;
        println!(
            "{name} bsp {k}: {} polygons, {} links ({jumps} drops and jumps), {} wall triangles -> {}",
            data.nav.polys.len(),
            data.nav.edges.len(),
            data.walls.tris.len(),
            file.display()
        );
    }
    Ok(())
}
