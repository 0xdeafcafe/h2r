//! Watching the override directory: a thread compares the directory's
//! listing (names, sizes, modification times of `*.wgsl`) every interval
//! and reloads once a change has held still for one interval, so an editor
//! that writes a file in pieces is read once, whole. No platform watcher:
//! a stat of a few files a quarter-second is nothing, and it works the
//! same on every host.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::Hub;

/// What the watcher compares: `*.wgsl` names, sizes and times, sorted.
pub type Signature = Vec<(String, u64, Option<SystemTime>)>;

/// The directory's [`Signature`] (empty when it can't be read).
pub fn signature(dir: &Path) -> Signature {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Signature = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "wgsl"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((
                e.file_name().to_string_lossy().into_owned(),
                m.len(),
                m.modified().ok(),
            ))
        })
        .collect();
    out.sort();
    out
}

/// One step of the debounce: `seen` is the signature last loaded,
/// `candidate` a change waiting to hold still. Returns whether to reload
/// now (and updates both).
pub(crate) fn step(
    now: Signature,
    seen: &mut Signature,
    candidate: &mut Option<Signature>,
) -> bool {
    if now == *seen {
        *candidate = None;
        return false;
    }
    if candidate.as_ref() == Some(&now) {
        *seen = now;
        *candidate = None;
        return true;
    }
    *candidate = Some(now);
    false
}

/// Watches `hub`'s directory (as it is at each look, so `render.reload
/// {"dir": ...}` moves it) on a thread of its own, reloading on change.
/// The first load is the caller's ([`Hub::reload`]).
pub fn watch(hub: &'static Hub, interval: Duration) -> std::io::Result<()> {
    let mut seen = hub.dir().map(|d| signature(&d)).unwrap_or_default();
    let mut candidate = None;
    let mut dir = hub.dir();
    std::thread::Builder::new()
        .name("render overrides watch".into())
        .spawn(move || {
            hub.set_watching(true);
            loop {
                std::thread::sleep(interval);
                let now_dir = hub.dir();
                if now_dir != dir {
                    // Moved by the API, which reloaded it.
                    seen = now_dir.as_deref().map(signature).unwrap_or_default();
                    candidate = None;
                    dir = now_dir;
                    continue;
                }
                let Some(d) = &dir else { continue };
                if step(signature(d), &mut seen, &mut candidate) {
                    let r = hub.reload();
                    if !r.changed.is_empty() || !r.problems.is_empty() {
                        eprintln!(
                            "render overrides: reloaded {} ({} changed{})",
                            d.display(),
                            r.changed.len(),
                            if r.problems.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    "; {}",
                                    r.problems
                                        .iter()
                                        .map(|p| format!("{}: {}", p.file, p.error))
                                        .collect::<Vec<_>>()
                                        .join("; ")
                                )
                            }
                        );
                    }
                }
            }
        })?;
    Ok(())
}
