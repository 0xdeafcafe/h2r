//! Use the owner's xdelta3 for VCDIFF (including its secondary compressors)
//! and keep the intermediate bytes on the importing machine.

use anyhow::{Context, Result, ensure};
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};

use crate::live_pack_import::Scratch;

pub(super) fn apply(base: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        patch.starts_with(&[0xd6, 0xc3, 0xc4, 0]),
        "invalid VCDIFF header"
    );
    let scratch = Scratch::new()?;
    let source = scratch.0.join("source.map");
    let delta = scratch.0.join("patch.vcdiff");
    fs::write(&source, base)?;
    fs::write(&delta, patch)?;
    // Ignore XDELTA's injected command options, and disable external
    // (de)compression. Paths from a VCDIFF application header are unused.
    let mut child = Command::new("xdelta3")
        .env_remove("XDELTA")
        .args(["-d", "-D", "-R", "-c", "-s"])
        .arg(source)
        .arg(delta)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .context("decoding VCDIFF requires xdelta3 on PATH")?;
    let mut target = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(super::MAX_CONVERTED_MAP as u64 + 1)
        .read_to_end(&mut target);
    if read.is_err() || target.len() > super::MAX_CONVERTED_MAP {
        let _ = child.kill();
    }
    let status = child.wait()?;
    read?;
    ensure!(
        target.len() <= super::MAX_CONVERTED_MAP,
        "xdelta target exceeds the map size limit"
    );
    ensure!(
        status.success(),
        "xdelta3 could not apply the patch to this base map ({status})"
    );
    Ok(target)
}
