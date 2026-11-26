//! Kits that build themselves. [`ensure`] builds the package of each
//! built-in map (`firefight-<map>`) from the player's disc when it is
//! missing or stale, and says for each map what it did. A package is
//! current when it is on disk and the stamp beside it
//! (`<maps>/.firefight-kit-<map>.stamp`) names this kit version and this
//! layout's hash; a package with no stamp (one `h2 firefight kit` built) is
//! stale and is replaced. A map that cannot be built is a named refusal,
//! never a quiet fallback.
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use content::MapId;

use crate::{Config, Fit, KIT_VERSION, Mode};

/// The built-in maps' base names, in the order the lobby prefers them.
pub const MAPS: [&str; 2] = ["coagulation", "zanzibar"];

/// Where the disc's multiplayer maps are read from.
pub trait DiscMaps {
    /// The bytes of the disc's `maps\<name>.map`.
    fn read_map(&self, name: &str) -> Result<Vec<u8>>;
}

/// One kit to keep current: a disc map, how to build it, and the hash of
/// what authored it (a change of either rebuilds the package).
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// The disc map's base name (`coagulation`).
    pub map: &'a str,
    pub config: Config,
    /// The layout's hash ([`super::Layout::digest`], read as a number), stamped beside the package.
    pub layout_hash: u64,
}

/// What [`ensure`] did for one map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The package was current; the disc was not read.
    Current,
    Built { id: MapId, fitted: Vec<Fit> },
    /// The map cannot be built; the sentence says why.
    Refused(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub map: String,
    pub outcome: Outcome,
}

impl Report {
    /// The refusal as one panel line (`Zanzibar: 6 KB over, sectors`);
    /// `None` for a map that is current or built.
    pub fn line(&self) -> Option<String> {
        match &self.outcome {
            Outcome::Refused(reason) => Some(format!("{}: {reason}", title(&self.map))),
            Outcome::Current | Outcome::Built { .. } => None,
        }
    }
}

/// `coagulation` as `Coagulation`.
fn title(map: &str) -> String {
    let mut chars = map.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn package_dir(map: &str) -> String {
    format!("firefight-{map}")
}

fn stamp_path(maps_dir: &Path, map: &str) -> PathBuf {
    maps_dir.join(format!(".firefight-kit-{map}.stamp"))
}

fn stamp(t: &Target<'_>) -> String {
    format!("firefight kit {KIT_VERSION} layout {:016x}\n", t.layout_hash)
}

fn current(t: &Target<'_>, maps_dir: &Path) -> bool {
    maps_dir.join(package_dir(t.map)).join("manifest.toml").is_file()
        && std::fs::read_to_string(stamp_path(maps_dir, t.map)).is_ok_and(|s| s == stamp(t))
}

/// The built-in kits' targets, from their layouts (`builtin::for_map`). A
/// map without a layout is left out here and refused by [`ensure`].
pub fn targets() -> Vec<Target<'static>> {
    MAPS.iter().filter_map(|m| target(m)).collect()
}

fn target(map: &str) -> Option<Target<'static>> {
    let layout = super::for_map(map)?;
    Some(Target {
        map: layout.map,
        config: Config {
            home: layout.home,
            layout: Some(layout),
            ..Config::default()
        },
        layout_hash: u64::from_str_radix(&layout.digest(), 16).unwrap_or_default(),
    })
}

/// The built-in maps whose package is missing or stale: what [`ensure`]
/// would build. Reads no disc.
pub fn pending(maps_dir: &Path) -> Vec<&'static str> {
    MAPS.into_iter()
        .filter(|m| target(m).is_none_or(|t| !current(&t, maps_dir)))
        .collect()
}

/// [`pending`] over `targets`.
pub fn pending_with<'a>(targets: &[Target<'a>], maps_dir: &Path) -> Vec<&'a str> {
    targets
        .iter()
        .filter(|t| !current(t, maps_dir))
        .map(|t| t.map)
        .collect()
}

/// Builds the built-in kits whose package is missing or stale, from
/// `disc`, into `maps_dir`; one report per built-in map, in [`MAPS`]
/// order. One map's refusal does not stop the others.
pub fn ensure(disc: &dyn DiscMaps, maps_dir: &Path) -> Vec<Report> {
    let mut reports = ensure_with(&targets(), disc, maps_dir);
    for map in MAPS {
        if target(map).is_none() {
            reports.push(Report {
                map: map.to_owned(),
                outcome: Outcome::Refused("it has no built-in layout".to_owned()),
            });
        }
    }
    reports.sort_by_key(|r| MAPS.iter().position(|m| *m == r.map).unwrap_or(MAPS.len()));
    reports
}

/// [`ensure`] over `targets`, in their order.
pub fn ensure_with(targets: &[Target<'_>], disc: &dyn DiscMaps, maps_dir: &Path) -> Vec<Report> {
    targets
        .iter()
        .map(|t| {
            let outcome = if current(t, maps_dir) {
                Outcome::Current
            } else {
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    build_one(t, disc, maps_dir)
                }));
                match built {
                    Ok(Ok(built)) => built,
                    Ok(Err(error)) => Outcome::Refused(reason(&error)),
                    Err(_) => Outcome::Refused("the build stopped".to_owned()),
                }
            };
            Report {
                map: t.map.to_owned(),
                outcome,
            }
        })
        .collect()
}

fn build_one(t: &Target<'_>, disc: &dyn DiscMaps, maps_dir: &Path) -> Result<Outcome> {
    let base = disc.read_map(t.map)?;
    if base.is_empty() {
        bail!("the disc's {} map is empty", t.map);
    }
    // A stale package of this name is replaced; the stamp is written last,
    // so a build that stopped part way is stale next time.
    let _ = std::fs::remove_file(stamp_path(maps_dir, t.map));
    let package = crate::build_package(
        &base,
        t.map,
        Mode::Kit,
        &t.config,
        maps_dir,
        &package_dir(t.map),
        true,
    )?;
    content::store::write_atomic(&stamp_path(maps_dir, t.map), stamp(t).as_bytes())?;
    Ok(Outcome::Built {
        id: package.id,
        fitted: package.fitted,
    })
}

/// Why a build failed, short enough for a lobby panel line.
fn reason(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    overflow_reason(&text).unwrap_or_else(|| {
        let line = text.lines().next().unwrap_or_default();
        line.chars().take(60).collect::<String>().trim_end_matches('.').to_owned()
    })
}

/// `6 KB over, sectors` from the kit's headroom refusal.
fn overflow_reason(text: &str) -> Option<String> {
    let rest = text.split("exceeds tag headroom by ").nth(1)?;
    let bytes: u32 = rest.split(" B").next()?.trim().parse().ok()?;
    let part = rest
        .split("the biggest part is ")
        .nth(1)
        .and_then(|p| p.split(" (").next())
        .unwrap_or("tags");
    Some(format!("{} KB over, {part}", bytes.div_ceil(1024)))
}

/// The panel line for `map` refused with `error` (`Zanzibar: 6 KB over,
/// sectors`).
pub fn refusal_line(map: &str, error: &anyhow::Error) -> String {
    format!("{}: {}", title(map), reason(error))
}

#[cfg(test)]
mod tests;
