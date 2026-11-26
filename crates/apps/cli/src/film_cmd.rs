//! `h2 film stat FILE`: the stats container's header and the keyframes it
//! stores.
//! `h2 film stat --fields FILE...`: how often each field of the 92-byte
//! action changes in the films' tick tracks, and what each predictor of the
//! column encoder stores it in (docs/game/input-film.md, "The encoding").
use anyhow::{Context, Result, ensure};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

const USAGE: &str = "usage: h2 film stat FILE | h2 film stat --fields FILE...";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let verb = args.next();
    ensure!(verb.as_deref() == Some("stat"), "{USAGE}");
    let first = args.next().context(USAGE)?;
    if first == "--fields" {
        let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
        ensure!(!paths.is_empty(), "{USAGE}");
        let mut tracks = Vec::new();
        for path in &paths {
            tracks.push(
                film::blam::TrackReader::open_with_film(path)
                    .with_context(|| format!("opening {}", path.display()))?,
            );
        }
        print!("{}", field_table(tracks)?);
        return Ok(());
    }
    let path = PathBuf::from(first);
    ensure!(args.next().is_none(), "{USAGE}");
    print!("{}", stat(&path)?);
    Ok(())
}

/// The field table over the tick tracks of `films`, one report for all of
/// them: per field, the seat-ticks it is in the game for, how many change
/// from the tick before, and the stored bytes under each predictor, with
/// the grids that hold.
pub fn fields<R: std::io::Read + std::io::Seek>(films: Vec<film::Reader<R>>) -> Result<String> {
    let tracks = films
        .into_iter()
        .map(film::blam::TrackReader::new)
        .collect::<Result<Vec<_>>>()?;
    field_table(tracks)
}

/// [`fields`] over readers that are open already, with their film files
/// attached when their actions are there.
pub fn field_table<R: std::io::Read + std::io::Seek>(
    tracks: Vec<film::blam::TrackReader<R>>,
) -> Result<String> {
    use film::columns::{FIELDS, FieldRow, FieldStats, GRIDS, Predictor, SEATS};
    let mut table: Vec<FieldRow> = Vec::new();
    let mut ticks = 0u64;
    let count = tracks.len();
    for mut track in tracks {
        let mut stats = FieldStats::new();
        while let Some(tick) = track.step()? {
            let mut seats = [None; SEATS];
            for input in &tick.inputs {
                if let Some(slot) = seats.get_mut(usize::from(input.seat)) {
                    *slot = input.action;
                }
            }
            stats.push_tick(&seats)?;
            ticks += 1;
        }
        for (i, row) in stats.finish()?.into_iter().enumerate() {
            match table.get_mut(i) {
                Some(total) => total.merge(&row),
                None => table.push(row),
            }
        }
    }
    let mut out = String::new();
    writeln!(out, "films: {count}")?;
    writeln!(out, "ticks: {ticks}")?;
    writeln!(
        out,
        "fields: stored bytes are zstd level 3 of a field's planes, a seat and a chunk of 256 ticks at a time, \
         over the chunks in which the field is not constant"
    )?;
    for (row, field) in table.iter().zip(FIELDS) {
        let percent = if row.seat_ticks == 0 { 0.0 } else { row.changed as f64 * 100.0 / row.seat_ticks as f64 };
        writeln!(
            out,
            "FIELD {} words={} seat_ticks={} changed={} ({percent:.1}%) seat_chunks={} stored_chunks={} \
             none={} previous={} linear={} grid={} grid_chunks={} chosen={} \
             chosen_none={} chosen_previous={} chosen_linear={} chosen_grid={}",
            field.name,
            row.words,
            row.seat_ticks,
            row.changed,
            row.seat_chunks,
            row.stored_chunks,
            row.stored[0],
            row.stored[1],
            row.stored[2],
            row.grid_stored,
            row.grid_chunks,
            row.chosen_stored,
            row.chosen[Predictor::None as usize],
            row.chosen[Predictor::Previous as usize],
            row.chosen[Predictor::Linear as usize],
            row.chosen[Predictor::Grid as usize],
        )?;
        let held: Vec<String> = GRIDS
            .iter()
            .zip(row.grid_held)
            .filter(|&(_, chunks)| chunks > 0)
            .map(|(den, chunks)| format!("{den}:{chunks}"))
            .collect();
        if !held.is_empty() {
            writeln!(out, "  grid {} held in chunks by denominator {}", field.name, held.join(","))?;
        }
    }
    Ok(out)
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

/// The error for a file that begins as a v3 film but has no range footer: a
/// tape a session was killed in the middle of writing. The library's prune
/// removes it (`theater::autorecord::prune`); it is not read here.
fn unsealed(path: &Path, error: anyhow::Error) -> anyhow::Error {
    use std::io::{Read, Seek, SeekFrom};
    let ends = || -> std::io::Result<([u8; 8], [u8; 8])> {
        let mut file = std::fs::File::open(path)?;
        let (mut first, mut last) = ([0; 8], [0; 8]);
        file.read_exact(&mut first)?;
        file.seek(SeekFrom::End(-(film::range::FOOTER_BYTES as i64)))?;
        file.read_exact(&mut last)?;
        Ok((first, last))
    };
    match ends() {
        Ok((first, last))
            if first == film::range::MAGIC && last != film::range::FOOTER_MAGIC =>
        {
            anyhow::anyhow!(
                "{}: an unsealed tape, not a film: its session ended without sealing it ({error})",
                path.display()
            )
        }
        _ => error,
    }
}

pub fn stat(path: &Path) -> Result<String> {
    let mut reader = film::Reader::open(path).map_err(|error| unsealed(path, error))?;
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

    let rows = match film::blam::embedded_keyframes(&mut reader)? {
        Some(list) => list
            .into_iter()
            .map(|key| {
                let entry = reader
                    .entries
                    .iter()
                    .find(|entry| entry.offset == key.offset && entry.tag == film::KEYF)
                    .context("keyframe offset is absent from the film's index")?;
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
    Ok(out)
}

/// An unfinished film has no TIDX yet: read each KEYF's tick from its
/// payload prefix instead.
fn scan_keyframes(reader: &mut film::Reader<std::fs::File>) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let entries: Vec<_> = reader
        .entries
        .iter()
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
