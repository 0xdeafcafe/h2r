//! The maps the lobby's map variant builds from
//! (docs/game/forge-map-variants.md, "Materialising for a match"):
//! `forge_build::build` over this machine's disc, written to
//! `<data>/maps/forge-<first 16 hex of the id>/`. `game` may not use
//! `tools`, so the composition root supplies this, through
//! `game::map_variant::install`.
//!
//! The receipt of a built package is its manifest: a package is reused when
//! its manifest's id is the pick's and its files are there.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use content::manifest::{MANIFEST_FILE, Manifest};
use content::{MapId, Sha256, store};
use forge_doc::ForgeDoc;
use game::map_variant::{Maps, Refusal};
use scenario_edit::MapSource;

pub struct DiscMaps {
    disc: PathBuf,
    data: PathBuf,
    maps: OnceLock<Result<Vec<(String, MapSource)>, String>>,
    hashes: Mutex<HashMap<String, Option<Sha256>>>,
}

impl DiscMaps {
    pub fn new(disc: PathBuf, data: PathBuf) -> Self {
        Self {
            disc,
            data,
            maps: OnceLock::new(),
            hashes: Mutex::new(HashMap::new()),
        }
    }

    /// The disc's maps, read once.
    fn read(&self) -> Result<&[(String, MapSource)], String> {
        self.maps
            .get_or_init(|| {
                MapSource::disc_maps(&self.disc)
                    .map_err(|e| format!("The disc's maps do not read ({e})."))
            })
            .as_deref()
            .map_err(Clone::clone)
    }
}

impl Maps for DiscMaps {
    fn installed(&self, map: &str) -> Option<Sha256> {
        let (_, source) = self
            .read()
            .ok()?
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(map))?;
        *self
            .hashes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(map.to_ascii_lowercase())
            .or_insert_with(|| hash_of(source))
    }

    fn cached(&self, id: &MapId) -> Option<PathBuf> {
        let dir = store::maps_dir(&self.data).join(format!("forge-{}", &id.to_string()[..16]));
        let text = std::fs::read_to_string(dir.join(MANIFEST_FILE)).ok()?;
        let manifest = Manifest::from_toml(&text).ok()?;
        let present = manifest
            .forge
            .iter()
            .all(|l| dir.join(&l.file).is_file() && dir.join(&l.document_file).is_file());
        (manifest.map_id() == *id && manifest.validate_in(&dir).is_ok() && present).then_some(dir)
    }

    fn build(&mut self, doc: &ForgeDoc) -> Result<PathBuf, Refusal> {
        let maps = self
            .read()
            .map_err(|why| Refusal::new("variant_build", why))?;
        let built = forge_build::build(doc, |name| {
            maps.iter()
                .find(|(map, _)| map.eq_ignore_ascii_case(name))
                .map(|(_, source)| source.clone())
        })
        .map_err(|e| refusal_of(&doc.title, &e))?;
        built
            .write_package(&store::maps_dir(&self.data))
            .map_err(|e| refusal_of(&doc.title, &e))
    }
}

fn hash_of(source: &MapSource) -> Option<Sha256> {
    use std::io::{Read, Seek, SeekFrom};
    if let Some(bytes) = &source.bytes {
        return Some(Sha256::of(bytes));
    }
    let mut file = std::fs::File::open(&source.path).ok()?;
    let (start, len) = match source.region {
        Some(region) => region,
        None => (0, file.metadata().ok()?.len()),
    };
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = vec![0; usize::try_from(len).ok()?];
    file.read_exact(&mut bytes).ok()?;
    Some(Sha256::of(&bytes))
}

/// The table's refusal a build error is (docs/game/forge-map-variants.md,
/// "Rules and limits").
pub fn refusal_of(title: &str, e: &forge_build::Error) -> Refusal {
    use forge_build::Error as E;
    match e {
        E::ExpectedMapId { .. } => Refusal::new(
            "variant_expect",
            format!("{title} did not build to the map its author saved."),
        ),
        E::Print { block, index } => Refusal::new(
            "variant_print",
            format!("{title} does not match this map ({} {index}).", block.name()),
        ),
        E::MissingMap(map) => Refusal::new(
            "variant_base_missing",
            format!("{title} needs {map}, which is not installed."),
        ),
        E::BaseHash { map, .. } => Refusal::new(
            "variant_base_hash",
            format!("{title} was made for another version of {map}."),
        ),
        e => {
            eprintln!("map variant: {title}: {e}");
            Refusal::new("variant_build", format!("{title} could not be built."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_errors_are_the_tables_refusals() {
        let a = refusal_of(
            "Castle",
            &forge_build::Error::ExpectedMapId {
                expected: MapId([1; 32]),
                actual: MapId([2; 32]),
            },
        );
        assert_eq!(a.code, "variant_expect");
        let b = refusal_of("Castle", &forge_build::Error::MissingMap("lockout".into()));
        assert_eq!(b.sentence, "Castle needs lockout, which is not installed.");
        let c = refusal_of("Castle", &forge_build::Error::BaseType("lockout".into()));
        assert_eq!(c.code, "variant_build");
    }

    #[test]
    fn a_package_is_cached_only_with_its_manifest() {
        let dir = std::env::temp_dir().join(format!("h2-forge-maps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let maps = DiscMaps::new(PathBuf::new(), dir.clone());
        assert_eq!(maps.cached(&MapId([7; 32])), None);
    }
}
