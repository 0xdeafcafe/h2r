//! `h2 film stat FILE`: the keyframes and capture points a seeking player
//! has, plus the size and operation mix of its HLE tape.
//! `h2 film stat --fields FILE...`: how often each field of the 92-byte
//! action changes in the films' tick tracks, and what each predictor of the
//! column encoder stores it in (docs/game/input-film.md, "The encoding").
use anyhow::{Context, Result, ensure};
use film::tape::{BOTR, CALL_STREAMS, Call, Mark, PDEV, TAPE, decode_batch, encode_batch, stream_of};
use std::{
    collections::BTreeMap,
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
/// tape a session was killed in the middle of writing. It is sealed by the
/// next recording session (`theater::autorecord::recover`), not read here.
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
                "{}: an unsealed tape, not a film: its session ended without sealing it, and the next recording session seals what is playable ({error})",
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
    let mut tape = StreamStat::default();
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
            TAPE => {
                let raw = reader.read_bytes(&entry)?;
                tape.chunks += 1;
                tape.stored += u64::from(entry.size) + 8;
                tape.inflated += raw.len() as u64;
                // Decode one batch at a time and retain only the operation
                // names, counts and sizes. The report contains no call
                // arguments or outputs.
                for call in decode_batch(&raw)
                    .with_context(|| format!("invalid tape batch at {}", entry.offset))?
                {
                    tape.calls += 1;
                    let stream = streams.entry(stream_of(&call.op)).or_default();
                    stream.calls += 1;
                    stream.json += serde_json::to_vec(&call)?.len() as u64 + 1;
                    *stream.ops.entry(call.op.clone()).or_default() += 1;
                    stream.pending.push(call);
                    if stream.pending.len() == 128 {
                        stream.flush()?;
                    }
                }
            }
            PDEV | BOTR => {
                let raw = reader.read_bytes(&entry)?;
                let stream = streams.entry(entry.tag).or_default();
                stream.chunks += 1;
                stream.stored += u64::from(entry.size) + 8;
                stream.inflated += raw.len() as u64;
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
    writeln!(
        out,
        "TAPE calls={} inflated={} stored={} chunks={}",
        tape.calls, tape.inflated, tape.stored, tape.chunks,
    )?;
    writeln!(
        out,
        "streams: json is the bytes of the stream's calls in the JSON encoding, \
         binary the same calls in the binary one, in batches of 128"
    )?;
    for tag in CALL_STREAMS {
        let mut stream = streams.remove(&tag).unwrap_or_default();
        stream.flush()?;
        let name = std::str::from_utf8(&tag)?;
        writeln!(
            out,
            "{name} calls={} json={} binary={}",
            stream.calls, stream.json, stream.binary,
        )?;
        for (op, count) in stream.ops {
            writeln!(out, "  {} calls={count}", serde_json::to_string(&op)?)?;
        }
    }
    for tag in [PDEV, BOTR] {
        let stream = streams.remove(&tag).unwrap_or_default();
        let name = std::str::from_utf8(&tag)?;
        writeln!(
            out,
            "{name} calls=0 inflated={} stored={} chunks={}",
            stream.inflated, stream.stored, stream.chunks,
        )?;
    }
    Ok(out)
}

#[derive(Default)]
struct StreamStat {
    chunks: u64,
    stored: u64,
    inflated: u64,
    calls: u64,
    json: u64,
    binary: u64,
    pending: Vec<Call>,
    ops: BTreeMap<String, u64>,
}

impl StreamStat {
    /// Count the pending calls as one binary batch.
    fn flush(&mut self) -> Result<()> {
        if !self.pending.is_empty() {
            self.binary += encode_batch(&self.pending)?.len() as u64;
            self.pending.clear();
        }
        Ok(())
    }
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
