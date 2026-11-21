//! `h2 film stat FILE`: the keyframes and capture points a seeking player
//! has, plus the size and operation mix of its HLE tape.
use anyhow::{Context, Result, ensure};
use film::tape::{BOTR, CALL_STREAMS, Call, Mark, PDEV};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::{Path, PathBuf},
};

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
    writeln!(out, "length_ticks: {ticks}")?;

    let rows = match film::blam::keyframes(&mut reader)? {
        Some(list) => list
            .into_iter()
            .map(|key| {
                let entry = reader.machine_section().keyframe_at(key.offset)?;
                Ok(Row {
                    tick: key.tick,
                    stored: u64::from(entry.size) + 8,
                    anchor: Some(key.ram_anchor),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        None => scan_keyframes(&mut reader)?,
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
    let mut streams = BTreeMap::<film::Tag, StreamStat>::new();
    let entries: Vec<_> = reader.machine_section().entries().cloned().collect();
    for entry in entries {
        match entry.tag {
            film::MARK => {
                if let Mark::D3dBarrier {
                    slices,
                    tick,
                    saved,
                } = reader.read(&entry)?
                {
                    barriers.push((tick, slices, saved));
                }
            }
            tag if CALL_STREAMS.contains(&tag) || matches!(tag, PDEV | BOTR) => {
                let raw = reader.read_bytes(&entry)?;
                let stream = streams.entry(tag).or_default();
                stream.chunks += 1;
                stream.stored += u64::from(entry.size) + 8;
                stream.inflated += raw.len() as u64;
                if CALL_STREAMS.contains(&tag) {
                    // Decode one batch at a time and retain only the operation
                    // names and counts. The report contains no call arguments
                    // or outputs.
                    for call in serde_json::from_slice::<Vec<Call>>(&raw)
                        .with_context(|| format!("invalid tape batch at {}", entry.offset))?
                    {
                        stream.calls += 1;
                        *stream.ops.entry(call.op).or_default() += 1;
                    }
                }
            }
            _ => {}
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
    writeln!(out, "tape: stored bytes include the 8-byte chunk prefix")?;
    for tag in CALL_STREAMS.into_iter().chain([PDEV, BOTR]) {
        let stream = streams.remove(&tag).unwrap_or_default();
        let name = std::str::from_utf8(&tag)?;
        writeln!(
            out,
            "{name} calls={} inflated={} stored={} chunks={}",
            stream.calls, stream.inflated, stream.stored, stream.chunks,
        )?;
        for (op, count) in stream.ops {
            writeln!(out, "  {} calls={count}", serde_json::to_string(&op)?)?;
        }
    }
    Ok(out)
}

#[derive(Default)]
struct StreamStat {
    chunks: u64,
    stored: u64,
    inflated: u64,
    calls: u64,
    ops: BTreeMap<String, u64>,
}

/// An unfinished film has no TIDX yet: read each KEYF's tick from its
/// payload prefix instead.
fn scan_keyframes(reader: &mut film::Reader<std::fs::File>) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let entries: Vec<_> = reader
        .machine_section()
        .entries()
        .filter(|entry| entry.tag == film::KEYF)
        .cloned()
        .collect();
    for entry in entries {
        let bytes = reader.read_bytes(&entry)?;
        let mut raw = bytes.as_slice();
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
