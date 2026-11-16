//! The map packages under a data root (`<data>/maps/<dir>/manifest.toml`,
//! docs/game/maps.md), as the editor's Maps list shows them: user packages
//! (edits of disc maps and new maps), and Halo 1 maps converted to Halo 2
//! (docs/game/h1-multiplayer.md), one entry per sandbox variant.
//!
//! The manifest is read here as plain TOML, not through
//! `content::Manifest`, so that fields this build's resolver does not know
//! yet (a converted map's `[origin]`) still list. Opening a package's bytes
//! goes through the resolver ([`content::Library`]) whenever it accepts the
//! package.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use content::manifest::MANIFEST_FILE;
use content::{Library, store};

/// Which sandbox (weapons, vehicles, bipeds) a converted map plays with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sandbox {
    /// Halo 1's own, converted.
    Halo1,
    /// Halo 2's nearest equivalents.
    Halo2,
}

impl Sandbox {
    /// The manifest's spelling (`sandbox = "h1"` or `"h2"`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "h1" | "halo1" => Some(Self::Halo1),
            "h2" | "halo2" => Some(Self::Halo2),
            _ => None,
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Self::Halo1 => "H1",
            Self::Halo2 => "H2",
        }
    }
}

/// Where a converted map came from: an `origin` table with
/// `game = "halo1"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Halo1Origin {
    /// The Halo 1 map's name (`bloodgulch`).
    pub map: String,
    /// The variant's sandbox, when the manifest says.
    pub sandbox: Option<Sandbox>,
    /// SHA-256 (hex) of the Halo 1 map file it was converted from.
    pub source_sha256: Option<String>,
    /// The variant's own whole map file in the package, when the package
    /// holds several (relative to the package directory).
    pub file: Option<String>,
}

/// The Halo 1 origins a manifest declares: one per converted variant, none
/// for any other package.
///
/// This is the one place that decides what marks a converted Halo 1 map.
/// It accepts the `[origin]` table the Halo 1 import writes
/// (`game = "halo1"`, `map`, `sandbox = "h1" | "h2"`, `source_sha256`),
/// either at the top of the manifest or in each entry of a top-level array
/// of tables (one per variant, with its own `file`).
pub fn halo1_origins(manifest: &toml::Table) -> Vec<Halo1Origin> {
    fn origin(t: &toml::Table, file: Option<&str>) -> Option<Halo1Origin> {
        let o = t.get("origin")?.as_table()?;
        let game = o.get("game")?.as_str()?;
        if !game.eq_ignore_ascii_case("halo1") {
            return None;
        }
        let s = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        Some(Halo1Origin {
            map: s("map").unwrap_or_default(),
            sandbox: s("sandbox").as_deref().and_then(Sandbox::parse),
            source_sha256: s("source_sha256"),
            file: s("file").or_else(|| file.map(str::to_owned)),
        })
    }
    let mut out: Vec<Halo1Origin> = manifest
        .values()
        .filter_map(|v| v.as_array())
        .flatten()
        .filter_map(|v| v.as_table())
        .filter_map(|t| origin(t, t.get("file").and_then(|f| f.as_str())))
        .collect();
    if out.is_empty()
        && let Some(o) = origin(manifest, None)
    {
        out.push(o);
    }
    out
}

/// One entry of the Maps list's package groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageEntry {
    /// The package directory's name under `<data>/maps`.
    pub dir: String,
    /// `[package] name`.
    pub name: String,
    /// `[base] map`: the map name the game loads it as.
    pub map: String,
    /// A new map (its first layer is the whole map file), not an edit of
    /// a disc map.
    pub derived: bool,
    /// Set for a converted Halo 1 map: which variant this entry is.
    pub halo1: Option<Halo1Origin>,
}

impl PackageEntry {
    /// The list label: the package's name, with the sandbox of a
    /// converted variant (`Blood Gulch (H1 sandbox)`).
    pub fn label(&self) -> String {
        match self.halo1.as_ref().and_then(|o| o.sandbox) {
            Some(s) => format!("{} ({} sandbox)", self.name, s.short()),
            None => self.name.clone(),
        }
    }
    /// A key unique in the list (a package holds up to two variants).
    pub fn key(&self) -> String {
        match self.halo1.as_ref().and_then(|o| o.sandbox) {
            Some(s) => format!("{}#{}", self.dir, s.short()),
            None => self.dir.clone(),
        }
    }
}

/// Every package under `maps` (`<data>/maps`), user packages first, then
/// converted Halo 1 maps, each by label. A directory whose manifest is
/// missing or unreadable is left out.
pub fn list(maps: &Path) -> Vec<PackageEntry> {
    let Ok(dirs) = std::fs::read_dir(maps) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for d in dirs.flatten() {
        let Some(dir) = d.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !store::is_dir_name(&dir) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(d.path().join(MANIFEST_FILE)) else {
            continue;
        };
        let Ok(table) = text.parse::<toml::Table>() else {
            continue;
        };
        let get = |t: &str, k: &str| {
            table
                .get(t)
                .and_then(|v| v.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let (Some(name), Some(map)) = (get("package", "name"), get("base", "map")) else {
            continue;
        };
        let derived = table
            .get("base")
            .and_then(|b| b.get("derived"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let entry = |halo1| PackageEntry {
            dir: dir.clone(),
            name: name.clone(),
            map: map.clone(),
            derived,
            halo1,
        };
        let origins = halo1_origins(&table);
        if origins.is_empty() {
            out.push(entry(None));
        } else {
            out.extend(origins.into_iter().map(|o| entry(Some(o))));
        }
    }
    out.sort_by_key(|e| (e.halo1.is_some(), e.label().to_ascii_lowercase(), e.key()));
    out
}

/// The whole map an entry stands for, in memory. A converted variant with
/// its own file is that file. Anything else goes through the resolver
/// with `base` (the disc's map of the same name; unused for a new map);
/// when the resolver refuses the manifest (a field it does not know yet),
/// a new map whose only layer is its whole file is read directly.
pub fn read_map(maps: &Path, entry: &PackageEntry, base: Option<&[u8]>) -> Result<Vec<u8>> {
    let dir = maps.join(&entry.dir);
    if let Some(file) = entry.halo1.as_ref().and_then(|o| o.file.as_deref()) {
        return read_inside(&dir, file);
    }
    let lib = Library::open(maps);
    if let Some(p) = lib.packages().iter().find(|p| p.dir_name() == entry.dir) {
        if !p.manifest.base.derived && base.is_none() {
            bail!(
                "{} edits {}: open a disc with that map first",
                entry.dir,
                p.manifest.base.map
            );
        }
        let base = base.unwrap_or_default();
        let resolved = lib
            .resolve_package(p, &mut &base[..])
            .with_context(|| format!("resolve {}", entry.dir))?;
        return Ok(resolved.read_all(&mut &base[..])?);
    }
    // Not accepted by this build's resolver: only a new map with a single
    // whole-file layer can be read without it.
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE))?;
    let table: toml::Table = text.parse()?;
    let layers: Vec<&str> = table
        .get("patch")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|l| l.get("file")?.as_str())
        .collect();
    match (entry.derived, layers.as_slice()) {
        (true, [only]) => read_inside(&dir, only),
        _ => bail!(
            "{}: the content resolver does not accept this package (see the game's console)",
            entry.dir
        ),
    }
}

fn read_inside(dir: &Path, file: &str) -> Result<Vec<u8>> {
    let rel = PathBuf::from(file);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("{file:?} is not a path inside the package");
    }
    let p = dir.join(rel);
    std::fs::read(&p).with_context(|| format!("read {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> toml::Table {
        s.parse().unwrap()
    }

    #[test]
    fn a_top_level_origin_marks_a_converted_map() {
        let t = table(
            r#"
            [package]
            format = 1
            name = "Blood Gulch"
            [base]
            map = "h1_bloodgulch"
            derived = true
            [origin]
            game = "halo1"
            map = "bloodgulch"
            sandbox = "h2"
            source_sha256 = "ab"
            "#,
        );
        assert_eq!(
            halo1_origins(&t),
            [Halo1Origin {
                map: "bloodgulch".into(),
                sandbox: Some(Sandbox::Halo2),
                source_sha256: Some("ab".into()),
                file: None,
            }]
        );
    }

    #[test]
    fn each_variant_entry_with_an_origin_is_one_variant() {
        let t = table(
            r#"
            [package]
            name = "Blood Gulch"
            [[variant]]
            file = "h1/h1_bloodgulch.map"
            origin = { game = "halo1", map = "bloodgulch", sandbox = "h1" }
            [[variant]]
            file = "h2/h1_bloodgulch.map"
            origin = { game = "halo1", map = "bloodgulch", sandbox = "h2" }
            [[patch]]
            file = "x.ppf"
            "#,
        );
        let o = halo1_origins(&t);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].sandbox, Some(Sandbox::Halo1));
        assert_eq!(o[0].file.as_deref(), Some("h1/h1_bloodgulch.map"));
        assert_eq!(o[1].sandbox, Some(Sandbox::Halo2));
    }

    #[test]
    fn other_packages_and_other_games_have_no_origin() {
        assert!(halo1_origins(&table("[package]\nname = \"x\"\n")).is_empty());
        assert!(halo1_origins(&table("[origin]\ngame = \"halo2\"\nmap = \"x\"\n")).is_empty());
    }

    #[test]
    fn sandbox_labels_name_the_variant() {
        let e = PackageEntry {
            dir: "h1_bloodgulch".into(),
            name: "Blood Gulch".into(),
            map: "h1_bloodgulch".into(),
            derived: true,
            halo1: Some(Halo1Origin {
                map: "bloodgulch".into(),
                sandbox: Some(Sandbox::Halo1),
                source_sha256: None,
                file: None,
            }),
        };
        assert_eq!(e.label(), "Blood Gulch (H1 sandbox)");
        assert_eq!(e.key(), "h1_bloodgulch#H1");
    }
}
