//! Read back files from the agent's output directories in bounded pages.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use agent_proto::trace::{TraceKind, TraceReadPage, TraceReadParams, valid_file_name};
use agent_proto::{RpcError, code, valid_log_name};
use serde::Deserialize;
use serde_json::{Value, json};

const TRACE_SCAN_BYTES: u64 = 256 << 10;
const TRACE_ROW_BYTES: u64 = 1 << 20;
const INPUT_PAGE_BYTES: usize = 64 << 10;

pub(crate) fn trace_list(dir: &Path) -> Result<Value, RpcError> {
    Ok(json!({
        "traces": list(&dir.join("traces"), |file| valid_file_name(file).then_some(file))?,
        "flights": list(&dir.join("flights"), |file| valid_file_name(file).then_some(file))?,
    }))
}

pub(crate) fn input_list(dir: &Path) -> Result<Value, RpcError> {
    let names = list(&dir.join("inputs"), |file| {
        file.strip_suffix(".input")
            .filter(|name| valid_log_name(name))
    })?;
    Ok(json!({ "names": names }))
}

fn list(dir: &Path, name: impl Fn(&str) -> Option<&str>) -> Result<Vec<String>, RpcError> {
    let result = (|| {
        let entries = match fs::symlink_metadata(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => {
                require_directory(result?)?;
                fs::read_dir(dir)?
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            let file = entry.file_name();
            if entry.file_type()?.is_file()
                && let Some(name) = file.to_str().and_then(&name)
            {
                names.push(name.to_owned());
            }
        }
        names.sort_unstable();
        Ok(names)
    })();
    result.map_err(|e| unavailable(&dir.display().to_string(), e))
}

pub(crate) fn trace_read(dir: &Path, p: TraceReadParams) -> Result<Value, RpcError> {
    if !valid_file_name(&p.path) {
        return Err(invalid("path must be a file name"));
    }
    let limit = limit(p.limit, 64, 256)?;
    let file = match p.kind {
        Some(TraceKind::Trace) => open(&dir.join("traces").join(&p.path)),
        Some(TraceKind::Flight) => open(&dir.join("flights").join(&p.path)),
        None => match open(&dir.join("traces").join(&p.path)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                open(&dir.join("flights").join(&p.path))
            }
            other => other,
        },
    }
    .map_err(|e| unavailable(&p.path, e))?;
    Ok(json!(trace_page(file, &p, limit)?))
}

fn trace_page(
    mut file: File,
    p: &TraceReadParams,
    limit: usize,
) -> Result<TraceReadPage, RpcError> {
    let error = |e| unavailable(&p.path, e);
    let len = file.metadata().map_err(error)?.len();
    let start = p.offset.unwrap_or(0);
    check_offset(start, len)?;
    if start != 0 {
        file.seek(SeekFrom::Start(start - 1)).map_err(error)?;
        let mut last = [0];
        file.read_exact(&mut last).map_err(error)?;
        if last[0] != b'\n' {
            return Err(invalid("offset must start at a row boundary"));
        }
    }
    let mut reader = BufReader::new(file.take(len - start));
    let mut page = TraceReadPage::default();
    let mut line = Vec::new();
    let mut offset = start;
    while offset < len && offset - start < TRACE_SCAN_BYTES && page.rows.len() < limit {
        line.clear();
        let bytes = reader
            .by_ref()
            .take(TRACE_ROW_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(error)? as u64;
        if bytes > TRACE_ROW_BYTES {
            return Err(invalid("trace row exceeds 1 MiB"));
        }
        if line.last() != Some(&b'\n') {
            // A failed writer can leave a partial row at the end.
            return Ok(page);
        }
        offset += bytes;
        let row = std::str::from_utf8(&line[..line.len() - 1])
            .map_err(|e| invalid(format!("trace row at byte {}: {e}", offset - bytes)))?;
        if !row.trim().is_empty() && matches_ticks(row, p)? {
            page.rows.push(row.to_owned());
        }
    }
    page.next = (offset < len).then_some(offset);
    Ok(page)
}

fn matches_ticks(row: &str, p: &TraceReadParams) -> Result<bool, RpcError> {
    if p.from_tick.is_none() && p.to_tick.is_none() {
        return Ok(true);
    }
    #[derive(Deserialize)]
    struct Tick {
        tick: Option<u64>,
    }
    let Tick { tick } =
        serde_json::from_str(row).map_err(|e| invalid(format!("reading trace tick: {e}")))?;
    Ok(tick.is_some_and(|t| {
        p.from_tick.is_none_or(|from| t >= from) && p.to_tick.is_none_or(|to| t <= to)
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputReadParams {
    name: String,
    offset: Option<u64>,
    limit: Option<usize>,
}

pub(crate) fn input_read(dir: &Path, p: InputReadParams) -> Result<Value, RpcError> {
    let path = crate::tap::log_path(dir, &p.name)?;
    let limit = limit(p.limit, 16 << 10, INPUT_PAGE_BYTES)?;
    let error = |e| unavailable(&p.name, e);
    let mut file = open(&path).map_err(error)?;
    let len = file.metadata().map_err(error)?.len();
    let offset = p.offset.unwrap_or(0);
    check_offset(offset, len)?;
    file.seek(SeekFrom::Start(offset)).map_err(error)?;
    let mut bytes = vec![0; (len - offset).min(limit as u64) as usize];
    file.read_exact(&mut bytes).map_err(error)?;
    let end = offset + bytes.len() as u64;
    let text = String::from_utf8(bytes).map_err(|e| invalid(format!("reading input log: {e}")))?;
    Ok(json!({ "text": text, "next": (end < len).then_some(end) }))
}

fn limit(value: Option<usize>, default: usize, max: usize) -> Result<usize, RpcError> {
    let value = value.unwrap_or(default);
    if !(1..=max).contains(&value) {
        return Err(invalid(format!("limit must be 1 to {max}")));
    }
    Ok(value)
}

fn check_offset(offset: u64, len: u64) -> Result<(), RpcError> {
    if offset > len {
        return Err(invalid("offset is past the end of the file"));
    }
    Ok(())
}

fn require_directory(meta: fs::Metadata) -> io::Result<()> {
    if !meta.is_dir() {
        return Err(io::Error::other("expected a directory"));
    }
    Ok(())
}

fn open(path: &Path) -> io::Result<File> {
    // A socket client can only read regular files directly in these output directories.
    require_directory(fs::symlink_metadata(
        path.parent().expect("output directory"),
    )?)?;
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("expected a regular file"));
    }
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("expected a regular file"));
    }
    Ok(file)
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn unavailable(name: &str, error: io::Error) -> RpcError {
    RpcError::new(code::UNAVAILABLE, format!("reading {name}: {error}"))
}
