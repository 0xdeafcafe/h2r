use super::*;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::io::Read;
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub category: String,
    pub name: String,
    pub unit: String,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diff {
    pub from_ms: u64,
    pub to_ms: u64,
    pub footprint_delta: i64,
    pub growers: Vec<Change>,
}

fn delta(a: u64, b: u64) -> i64 {
    (b as i128 - a as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Bytes and item counts have different units. Rank the largest increases
/// within each unit; totals from overlapping categories must never be added.
pub fn diff(before: &Record, after: &Record) -> Diff {
    let mut growers = Vec::new();
    for (category, a, b) in [
        ("tag", &before.tags, &after.tags),
        ("stack", &before.stacks, &after.stacks),
        ("gpu", &before.gpu, &after.gpu),
        ("probe", &before.probes, &after.probes),
    ] {
        for (name, &end) in b {
            let start = a.get(name).copied().unwrap_or_default();
            if end > start {
                let unit = if category == "tag" || category == "stack" || name.contains("bytes") {
                    "bytes"
                } else {
                    "count"
                };
                growers.push(Change {
                    category: category.into(),
                    name: name.clone(),
                    unit: unit.into(),
                    before: start,
                    after: end,
                    delta: delta(start, end),
                });
            }
        }
    }
    growers.sort_by(|a, b| {
        (a.unit.as_str(), Reverse(a.delta), &a.category, &a.name).cmp(&(
            b.unit.as_str(),
            Reverse(b.delta),
            &b.category,
            &b.name,
        ))
    });
    Diff {
        from_ms: before.t_ms,
        to_ms: after.t_ms,
        footprint_delta: delta(before.footprint_bytes, after.footprint_bytes),
        growers,
    }
}

pub(super) fn finish(
    first: Option<&Record>,
    last: Option<&Record>,
    from_ms: Option<u64>,
) -> io::Result<Diff> {
    let (first, last) = first
        .zip(last)
        .ok_or_else(|| io_err("no committed memory records"))?;
    if from_ms.is_some_and(|t| t < first.t_ms || t > last.t_ms) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "requested time is outside retained memory history ({}..={} ms)",
                first.t_ms, last.t_ms
            ),
        ));
    }
    Ok(diff(first, last))
}

pub fn diff_since(records: &[Record], from_ms: u64) -> io::Result<Diff> {
    let first = records
        .iter()
        .rev()
        .find(|r| r.t_ms <= from_ms)
        .or(records.first());
    finish(first, records.last(), Some(from_ms))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Postmortem {
    pub pid: u32,
    pub file: PathBuf,
    pub diff: Diff,
    pub reports: Vec<PathBuf>,
    /// The newest report's identifying fields. Large vmmap, footprint and
    /// stack details remain in the complete JSON available at its path.
    pub report: Option<Value>,
}

const MAX_REPORT_BYTES: u64 = 4 << 20;

fn json_report(path: &Path) -> io::Result<Value> {
    if !regular(path)? {
        return Err(io_err("memory report is not a regular file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REPORT_BYTES {
        return Err(io_err("memory report exceeds read limit"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn report_pid(v: &Value) -> Option<u32> {
    v["game_pid"]
        .as_u64()
        .or_else(|| v["pid"].as_u64())
        .and_then(|n| n.try_into().ok())
}

fn summary(v: &Value) -> Value {
    let mut obj = serde_json::Map::new();
    for k in [
        "pid",
        "game_pid",
        "reason",
        "job_id",
        "job_name",
        "uptime_ms",
        "elapsed_ms",
        "unix_ms",
        "build_commit",
        "commit",
        "footprint_bytes",
        "peak_bytes",
        "last_footprint_bytes",
        "peak_footprint_bytes",
        "soft",
        "hard",
        "mem_limit",
        "pressure",
    ] {
        if let Some(value) = v.get(k) {
            obj.insert(k.into(), value.clone());
        }
    }
    Value::Object(obj)
}

fn timestamp(path: &Path) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |t| t.as_millis() as u64)
}

fn report_files(dir: &Path, pid: Option<u32>) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(s) = name.to_str() else { continue };
        if s == "lab-host.json"
            || (s
                .strip_prefix("mem-")
                .and_then(|s| s.split_once('-'))
                .and_then(|(p, _)| p.parse::<u32>().ok())
                .is_some_and(|p| pid.is_none_or(|wanted| p == wanted))
                && s.ends_with(".json"))
        {
            files.push(entry.path());
        }
    }
    files.sort_by_cached_key(|f| Reverse(timestamp(f)));
    Ok(files)
}

fn from_report(file: &Path, v: &Value) -> io::Result<Postmortem> {
    let pid = report_pid(v).ok_or_else(|| io_err("report has no process id"))?;
    let samples = v["samples"]
        .as_array()
        .ok_or_else(|| io_err("report has no footprint samples"))?;
    let point = |p: &Value| -> Option<Record> {
        Some(Record {
            pid,
            t_ms: p.get(0)?.as_u64()?,
            footprint_bytes: p.get(1)?.as_u64()?,
            ..Default::default()
        })
    };
    let first = samples.first().and_then(point);
    let last = samples.last().and_then(point);
    Ok(Postmortem {
        pid,
        file: file.into(),
        diff: finish(first.as_ref(), last.as_ref(), None)?,
        reports: vec![file.into()],
        report: Some(summary(v)),
    })
}

struct Endpoints {
    first: Option<Record>,
    last: Option<Record>,
}

impl Endpoints {
    fn add(&mut self, r: Record, _: usize) -> bool {
        if self.first.is_none() {
            self.first = Some(r.clone());
        }
        self.last = Some(r);
        true
    }
}

fn read_endpoints(dir: &Path, pid: u32) -> io::Result<Endpoints> {
    let mut ends = Endpoints {
        first: None,
        last: None,
    };
    // Read only the first nonempty segment forward to the first record, and
    // the last nonempty one to EOF. History needn't be materialized to diff.
    for n in (0..SEGMENTS).rev() {
        let path = segment(dir, pid, n);
        if regular(&path)? {
            read_segment(&path, |r, _| {
                ends.first = Some(r);
                false
            })?;
            if ends.first.is_some() {
                break;
            }
        }
    }
    for n in 0..SEGMENTS {
        let path = segment(dir, pid, n);
        if regular(&path)? {
            read_segment(&path, |r, _| {
                ends.last = Some(r);
                true
            })?;
            if ends.last.is_some() {
                break;
            }
        }
    }
    Ok(ends)
}

fn recorder_analysis(path: &Path, ends: Endpoints, report_dir: &Path) -> io::Result<Postmortem> {
    let first = ends
        .first
        .as_ref()
        .ok_or_else(|| io_err("empty recorder"))?;
    let comparison = finish(Some(first), ends.last.as_ref(), None)?;
    let mut pm = Postmortem {
        pid: first.pid,
        file: path.to_owned(),
        diff: comparison,
        reports: Vec::new(),
        report: None,
    };
    let start = first.unix_ms.saturating_sub(first.t_ms);
    for f in report_files(report_dir, Some(first.pid))? {
        let Ok(v) = json_report(&f) else { continue };
        if report_pid(&v) != Some(first.pid) {
            continue;
        }
        // A PID may have several preserved lifetimes in this directory.
        if let Some((wall, uptime)) = v["unix_ms"].as_u64().zip(v["uptime_ms"].as_u64())
            && wall.saturating_sub(uptime).abs_diff(start) > 10_000
        {
            continue;
        }
        if pm.report.is_none() {
            pm.report = Some(summary(&v));
        }
        pm.reports.push(f);
    }
    Ok(pm)
}

/// Read a copied stream, a rotated segment, or a watchdog/lab report. Streams
/// with standard names include their sibling segments. Arbitrarily named
/// fixture/export files can also be read in isolation.
pub fn postmortem_file(file: &Path) -> io::Result<Postmortem> {
    if !regular(file)? {
        return Err(io_err("memory evidence is not a regular file"));
    }
    if file.extension().is_some_and(|s| s == "json") {
        return from_report(file, &json_report(file)?);
    }
    let parent = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let ends = if let Some(pid) = file
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(current_pid)
    {
        read_endpoints(parent, pid)?
    } else {
        let mut ends = Endpoints {
            first: None,
            last: None,
        };
        read_segment(file, |r, n| ends.add(r, n))?;
        ends
    };
    let report_dir = parent
        .parent()
        .filter(|p| p.file_name().is_some_and(|s| s == "reports"))
        .and_then(Path::parent)
        .unwrap_or(parent);
    recorder_analysis(file, ends, report_dir)
}

/// Prefer the most recent dead recorder. Fall back to footprint-only evidence
/// for early exits and jobs running older images without a recorder.
pub fn postmortem(dir: &Path, pid: Option<u32>) -> io::Result<Postmortem> {
    let mut dirs = archive::archived_dirs(dir)?;
    dirs.push(dir.into());
    let mut latest = None;
    let mut last_wall = 0;
    let mut seen = BTreeSet::new();
    for d in dirs {
        for p in archive::pids(&d)? {
            if pid.is_some_and(|wanted| wanted != p) {
                continue;
            }
            if !archive::is_dead(&d, p)? {
                seen.insert(p);
                continue;
            }
            let ends = read_endpoints(&d, p)?;
            if let Some(row) = &ends.last
                && (latest.is_none() || row.unix_ms >= last_wall)
            {
                last_wall = row.unix_ms;
                let path = (0..SEGMENTS)
                    .map(|n| segment(&d, p, n))
                    .find(|path| path.is_file())
                    .ok_or_else(|| io_err("recorder disappeared"))?;
                latest = Some((path, ends));
            }
        }
    }
    if let Some((path, ends)) = latest {
        return recorder_analysis(&path, ends, dir);
    }
    for f in report_files(dir, pid)? {
        let Ok(v) = json_report(&f) else { continue };
        let Some(p) = report_pid(&v) else { continue };
        if seen.contains(&p) || pid.is_some_and(|wanted| wanted != p) {
            continue;
        }
        if let Ok(pm) = from_report(&f, &v) {
            return Ok(pm);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no dead process with committed memory evidence",
    ))
}
