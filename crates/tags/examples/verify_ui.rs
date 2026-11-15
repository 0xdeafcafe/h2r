//! Private-disc check of the UI tag decoders; prints aggregate counts only.
//! cargo run -p tags --example verify_ui -- /path/to/halo2.iso
//!
//! Runs [`tags::ui::check::check_map`] on every map: strict decoding of every
//! `wgit`, `wgtz`, `wigl` and `skin` tag plus the engine's unchecked index
//! cross-checks. `h2 ui --all all` runs the same check.
use map::Map;
use std::{fs::File, io::BufReader};
use tags::ui::check::{MapCheck, check_map};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("pass your own disc image path")?;
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    let entries = iso.entries()?;
    let mut all = MapCheck::default();
    let mut maps = 0;
    for entry in entries
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
        let mut map =
            Map::from_region(BufReader::new(File::open(&path)?), base, entry.size.into())?;
        maps += 1;
        let mut c = check_map(&mut map);
        for f in &mut c.failures {
            *f = format!("{}: {f}", entry.path);
        }
        all.merge(c);
    }
    for f in &all.failures {
        eprintln!("{f}");
    }
    print!("{maps} maps\n{}", report(&all));
    if !all.failures.is_empty() {
        return Err("UI validation failed".into());
    }
    Ok(())
}

fn report(c: &MapCheck) -> String {
    let mut s = String::from(
        "group  tags  fail  blocks  nonempty  aliased  foreign  elements  refs  string_ids  widgets\n",
    );
    for (g, n) in &c.groups {
        s += &format!(
            "{g}  {:>5} {:>5} {:>7} {:>9} {:>8} {:>8} {:>9} {:>5} {:>11} {:>8}\n",
            n.tags,
            n.failures,
            n.stats.blocks,
            n.stats.nonempty_blocks,
            n.stats.aliased_blocks,
            n.stats.foreign_aliases,
            n.stats.elements,
            n.stats.references,
            n.stats.string_ids,
            n.widgets
        );
    }
    s += &format!(
        "ignored shape groups {}, duplicate listings {}, overlapping spans {}, max skin depth {}\n\
         largest decode {} of {} budget bytes, nonempty raw wigl blocks +0x88/+0x130/+0x158 {:?}, \
         nonzero shader-bitmap animations {}, skipped table cells {}\n{} failures\n",
        c.ignored_shape_groups,
        c.duplicate_listings,
        c.overlapping_spans,
        c.max_skin_depth,
        c.max_decode_bytes,
        c.budget,
        c.raw_wigl_blocks,
        c.shader_animations,
        c.skipped_cells,
        c.failures.len()
    );
    s
}
