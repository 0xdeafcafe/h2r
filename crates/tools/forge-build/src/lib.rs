//! Materialise the F2 subset of a Forge document into a local map package.
//! The source maps are read-only. Scenario commands and tag patches go
//! through scenario-edit's writer.
#![forbid(unsafe_code)]

mod kit;
mod placement;
mod tag_fields;

use std::{
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use content::{
    Manifest, MapId, Sha256,
    manifest::{FORGE_FORMAT, ForgeLayer, MANIFEST_FILE, ResultInfo},
    store,
};
use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, Recipe};
use map::{CacheType, Map};
use scenario_edit::{MapSource, Session, layout, package::geometry_hash, save};

pub use kit::Reservation;

/// Versions pinned by an F2 document. Geometry-changing generators have no
/// writes in F2. Changing the construction or kit rules bumps forge_build.
pub const VERSIONS: BuilderVersions = BuilderVersions {
    forge_build: 1,
    pathfind: 1,
    bots: 5,
};

/// A generate recipe that makes the F2 session kit part of document content.
pub const KIT_RECIPE: &str = "forge_kit";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Document(#[from] forge_doc::Refusal),
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error(transparent)]
    Scenario(#[from] scenario_edit::Error),
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("forge_missing_map: {0}.map is missing from the source")]
    MissingMap(String),
    #[error("forge_base_hash: {map}.map hashes to {actual}, document names {expected}")]
    BaseHash {
        map: String,
        actual: Sha256,
        expected: Sha256,
    },
    #[error("forge_source_hash: {map}.map hashes to {actual}, document names {expected}")]
    SourceHash {
        map: String,
        actual: Sha256,
        expected: Sha256,
    },
    #[error("forge_expect_map_id: document expects {expected}, computed {actual}")]
    ExpectedMapId { expected: MapId, actual: MapId },
    #[error("forge_expect_geometry: document expects {expected}, built {actual}")]
    ExpectedGeometry { expected: Sha256, actual: Sha256 },
    #[error("forge_recipe: {0}")]
    Recipe(String),
    #[error("forge_base_type: {0} is not a multiplayer cache")]
    BaseType(String),
    #[error("forge_source_size: {0}.map exceeds the supported cache size")]
    SourceSize(String),
    #[error("forge_kit: {0}")]
    Kit(&'static str),
}

/// Bytes and manifest produced by the document. Saving is a separate action
/// so the CLI can fully check a document without writing a package.
pub struct Built {
    manifest: Manifest,
    document: String,
    map: Vec<u8>,
    pub reserves: Vec<Reservation>,
}

impl Built {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn map_bytes(&self) -> &[u8] {
        &self.map
    }

    pub fn write_package(&self, maps: &Path) -> Result<PathBuf, Error> {
        let id = self.manifest.map_id().to_string();
        let name = format!("forge-{}", &id[..16]);
        let layer = &self.manifest.forge[0];
        let files = [
            (layer.document_file.as_str(), self.document.as_bytes()),
            (layer.file.as_str(), self.map.as_slice()),
        ];
        match store::write_package(maps, &name, &self.manifest, &files, false) {
            Err(store::StoreError::Exists(dir)) => {
                // The directory name uses a hash prefix. Only replace the
                // same build, including its exact file names and digests.
                let mut existing = String::new();
                if dir.symlink_metadata()?.is_dir() {
                    File::open(dir.join(MANIFEST_FILE))?
                        .take(32 * 1024)
                        .read_to_string(&mut existing)?;
                }
                if existing != self.manifest.to_toml() {
                    return Err(store::StoreError::Exists(dir).into());
                }
                Ok(store::write_package(
                    maps,
                    &name,
                    &self.manifest,
                    &files,
                    true,
                )?)
            }
            result => Ok(result?),
        }
    }
}

/// Build against a set of named input maps. The lookup should expose the disc's
/// original maps by name. The hash checks use those exact bytes.
pub fn build(
    doc: &ForgeDoc,
    mut lookup: impl FnMut(&str) -> Option<MapSource>,
) -> Result<Built, Error> {
    let mut limits = Limits {
        builder: Some(VERSIONS),
        ..Limits::default()
    };
    forge_doc::validate(doc, &limits)?;
    let document_hash = doc.id()?;
    let id = MapId::forge(doc.base.sha256, document_hash);
    if let Some(expect) = doc.expect
        && expect.map_id != id
    {
        return Err(Error::ExpectedMapId {
            expected: expect.map_id,
            actual: id,
        });
    }

    let mut source =
        lookup(&doc.base.map).ok_or_else(|| Error::MissingMap(doc.base.map.clone()))?;
    source.bytes = Some(check_map(&doc.base, &source, true)?);
    let mut map = source.open()?;
    if map.header().cache_type != CacheType::Multiplayer {
        return Err(Error::BaseType(doc.base.map.clone()));
    }
    for donor in &doc.sources {
        let source = lookup(&donor.map).ok_or_else(|| Error::MissingMap(donor.map.clone()))?;
        check_map(donor, &source, false)?;
    }
    let needs_kit = requests_kit(&doc.recipes)?;

    let mut session = Session::new();
    session.build_scenes = false;
    session.open_named(source, &doc.base.map)?;
    let scenario = session
        .scenario()
        .ok_or(scenario_edit::Error::Invalid("scenario is not open"))?;
    limits.base_counts = layout::BLOCKS
        .iter()
        .map(|b| (b.kind, scenario.block(b.kind).len() as u32))
        .collect();
    limits.world = placement::world_bounds(&mut map)?;

    let mut apply = placement::Materialiser::new(&mut session, &mut map);
    forge_doc::replay(doc, &limits, &mut apply)?;
    let mut new_strings = apply.finish()?;
    if !needs_kit {
        session.set_tag_strings(std::mem::take(&mut new_strings))?;
    }
    let scenario = session
        .scenario()
        .ok_or(scenario_edit::Error::Invalid("scenario is not open"))?;
    let mut output = Cursor::new(Vec::new());
    save::write_patched(&mut map, scenario, &mut output)?;
    let mut output = output.into_inner();
    let mut reserves = Vec::new();
    if needs_kit {
        // The writer places new string tables after the metadata. Finish
        // the kit's metadata growth before writing those tables.
        (output, reserves) = kit::reserve(output, &new_strings)?;
    }
    let geometry = geometry_hash(&mut Map::from_reader(Cursor::new(output.as_slice()))?)?;
    if let Some(expect) = doc.expect
        && expect.geometry != geometry
    {
        return Err(Error::ExpectedGeometry {
            expected: expect.geometry,
            actual: geometry,
        });
    }

    let mut manifest = Manifest::retail(&doc.base.map, doc.base.sha256);
    manifest.package.format = FORGE_FORMAT;
    manifest.package.name = doc.title.clone();
    manifest.package.author = Some(doc.author.clone());
    manifest.package.description = (!doc.description.is_empty()).then(|| doc.description.clone());
    manifest.result = Some(ResultInfo {
        geometry: Some(geometry),
    });
    manifest.forge.push(ForgeLayer {
        document: document_hash,
        document_file: "document.h2forge".into(),
        file: "materialised.map".into(),
        sha256: Sha256::of(&output),
    });
    Ok(Built {
        manifest,
        document: doc.canonical_json()?,
        map: output,
        reserves,
    })
}

/// Read a bounded document from a file or stream. Checks use the same decoder
/// as data received from Live.
pub fn read_document(input: impl Read) -> Result<ForgeDoc, Error> {
    let mut bytes = Vec::new();
    input
        .take(forge_doc::MAX_DOC_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(ForgeDoc::from_bytes(&bytes, &Limits::default())?)
}

fn requests_kit(recipes: &[Recipe]) -> Result<bool, Error> {
    let mut requested = false;
    for recipe in recipes {
        match recipe {
            Recipe::Generate {
                generator,
                parameters,
            } if generator == KIT_RECIPE => {
                if requested || !parameters.is_empty() {
                    return Err(Error::Recipe(format!(
                        "{KIT_RECIPE} takes no parameters and may appear once"
                    )));
                }
                requested = true;
            }
            Recipe::Generate { generator, .. } => {
                return Err(Error::Recipe(format!(
                    "generator {generator:?} is unavailable"
                )));
            }
            Recipe::Transplant { tag, .. } => {
                return Err(Error::Recipe(format!(
                    "transplant of {:?} is unavailable",
                    tag.path
                )));
            }
            Recipe::Variant { tag, .. } => {
                return Err(Error::Recipe(format!(
                    "variant of {:?} is unavailable",
                    tag.path
                )));
            }
            Recipe::Import { slot } => {
                return Err(Error::Recipe(format!(
                    "import slot {} is unavailable",
                    slot.0
                )));
            }
        }
    }
    Ok(requested)
}

// Snapshot the checked bytes before opening an editing session. Both the
// available region and memory input have the largest supported cache bound.
fn check_map(reference: &MapRef, source: &MapSource, base: bool) -> Result<Arc<[u8]>, Error> {
    let cap = map::writer::cache_size_limit(if base {
        CacheType::Multiplayer
    } else {
        CacheType::SinglePlayerShared
    })?;
    let bytes: Arc<[u8]> = if let Some(bytes) = &source.bytes {
        if bytes.len() as u64 > cap {
            return Err(Error::SourceSize(reference.map.clone()));
        }
        bytes.clone()
    } else {
        let mut file = File::open(&source.path)?;
        let (start, len) = match source.region {
            Some(region) => region,
            None => (0, file.metadata()?.len()),
        };
        if len > cap {
            return Err(Error::SourceSize(reference.map.clone()));
        }
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; len as usize];
        file.read_exact(&mut bytes)?;
        bytes.into()
    };
    let actual = Sha256::of(&bytes);
    if actual != reference.sha256 {
        return Err(if base {
            Error::BaseHash {
                map: reference.map.clone(),
                actual,
                expected: reference.sha256,
            }
        } else {
            Error::SourceHash {
                map: reference.map.clone(),
                actual,
                expected: reference.sha256,
            }
        });
    }
    Ok(bytes)
}
