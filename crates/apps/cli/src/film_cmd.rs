//! `h2 film stat FILE`: the keyframes and capture points a seeking player
//! has, without inflating input, HLE or snapshot data (docs/game/film-format.md).
use anyhow::{Context, Result, ensure};
use std::{fmt::Write as _, path::{Path, PathBuf}};

const USAGE: &str = "usage: h2 film stat FILE";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    ensure!(args.next().as_deref() == Some("stat"), "{USAGE}");
    let path = PathBuf::from(args.next().context(USAGE)?);
    ensure!(args.next().is_none(), "{USAGE}");
    print!("{}", stat(&path)?);
    Ok(())
}

/// One stored keyframe: its tick, its on-disk bytes including the eight-byte
/// chunk prefix, and its RAM anchor (None on a standalone frame). A film
/// without a finished TIDX directory still lists its keyframes, with the
/// anchor unknown.
struct Row {
    tick: u64,
    stored: u64,
    anchor: Option<Option<u64>>,
}

pub fn stat(path: &Path) -> Result<String> {
    let mut reader = film::Reader::open(path)?;
    let mut out = String::new();
    let header = &reader.header;
    writeln!(out, "film: {}", path.display())?;
    writeln!(out, "version: {}", header.version)?;
    writeln!(
        out,
        "source: {}",
        match header.meta.source {
            film::Source::Host => "host",
            film::Source::Local => "local",
        }
    )?;
    let ticks = header.meta.length_ticks;
    writeln!(out, "length_ticks: {ticks} ({:.1} s)", ticks as f64 / 30.)?;

    let rows = match film::blam::keyframes(&mut reader) {
        Ok(list) => list
            .into_iter()
            .map(|key| {
                let entry = entry_at(&reader, key.offset)?;
                Ok(Row {
                    tick: key.tick,
                    stored: u64::from(entry.size) + 8,
                    anchor: Some(key.ram_anchor),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        Err(_) => scan_keyframes(&mut reader)?,
    };
    let anchors = rows.iter().filter(|row| row.anchor == Some(None)).count();
    let deltas = rows
        .iter()
        .filter(|row| matches!(row.anchor, Some(Some(_))))
        .count();
    let bytes: u64 = rows.iter().map(|row| row.stored).sum();
    writeln!(
        out,
        "keyframes: {} ({anchors} anchors, {deltas} deltas, {bytes} stored bytes)",
        rows.len()
    )?;
    for row in &rows {
        let anchor = match row.anchor {
            Some(None) => "anchor".to_owned(),
            Some(Some(tick)) => format!("delta anchor={tick}"),
            None => "anchor=unknown".to_owned(),
        };
        writeln!(out, "KEYF tick={} stored={} {anchor}", row.tick, row.stored)?;
    }

    let mut barriers = Vec::new();
    for entry in reader.entries.clone() {
        if entry.tag != film::MARK {
            continue;
        }
        if let film::tape::Mark::D3dBarrier {
            slices,
            tick,
            saved,
        } = reader.read(&entry)?
        {
            barriers.push((tick, slices, saved));
        }
    }
    let saved = barriers.iter().filter(|&&(_, _, saved)| saved).count();
    writeln!(
        out,
        "capture_points: {} ({saved} saved, {} failed)",
        barriers.len(),
        barriers.len() - saved
    )?;
    for (tick, slices, saved) in barriers {
        writeln!(out, "d3d_barrier tick={tick} slices={slices} saved={saved}")?;
    }
    Ok(out)
}

fn entry_at(reader: &film::Reader<std::fs::File>, offset: u64) -> Result<&film::Entry> {
    reader
        .entries
        .binary_search_by_key(&offset, |entry| entry.offset)
        .ok()
        .map(|index| &reader.entries[index])
        .with_context(|| format!("keyframe at offset {offset} is absent from the film index"))
}

/// An unfinished film has no TIDX yet: read each KEYF's tick from its
/// payload prefix instead.
fn scan_keyframes(reader: &mut film::Reader<std::fs::File>) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for entry in reader.entries.clone() {
        if entry.tag != film::KEYF {
            continue;
        }
        let mut raw = reader.read_bytes(&entry)?.as_slice();
        let tick = film::blam::stream::read_varint(&mut raw).map_err(anyhow::Error::msg)?;
        rows.push(Row {
            tick,
            stored: u64::from(entry.size) + 8,
            anchor: None,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
