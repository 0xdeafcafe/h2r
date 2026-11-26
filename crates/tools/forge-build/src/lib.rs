//! Materialise a Forge document's placement, entry, tag-field, link and
//! block-out piece edits into a local map package.
//! The source maps are read-only. Scenario commands and tag patches go
//! through scenario-edit's writer.
#![forbid(unsafe_code)]

mod areas;
pub mod blockout;
mod entry;
mod ground;
mod kit;
mod links;
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
use forge_doc::{
    BlockKind, BuilderVersions, Edit, Forge, ForgeDoc, Limits, MapRef, Recipe, Refusal,
};
use map::{CacheType, Map};
use scenario_edit::{MapSource, Session, layout, package::geometry_hash, save};

pub use entry::{base_fields, base_print, entry_fields, tag_key};
pub use forge_doc::Printed;
pub use ground::{ClassTable, GroundReport};
pub use kit::Reservation;

/// Versions pinned by an F2 document. Geometry-changing generators have no
/// writes in F2. Changing the construction or kit rules bumps forge_build.
pub const VERSIONS: BuilderVersions = BuilderVersions::CURRENT;

/// A generate recipe that makes the F2 session kit part of document content.
pub const KIT_RECIPE: &str = forge_doc::KIT_RECIPE;

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
    /// A block-out piece's definition does not install on this map: a
    /// `Map` material names a shader the base lacks, the map has nothing
    /// the pieces draw with, or a definition does not generate
    /// (docs/game/forge-blockout.md, "Materialising").
    #[error("forge_piece: {0}")]
    Piece(String),
    /// A definition the document needs does not fit the map's tag memory
    /// once everything the cut order drops is dropped.
    #[error("forge_piece_cost: {0}")]
    PieceCost(Box<blockout::Refused>),
    /// The document's objects have game types and the build names no
    /// engine, or one that is no engine's.
    #[error("forge_game: {0}")]
    Game(&'static str),
    #[error("forge_links: {0}")]
    Links(&'static str),
    /// The AI ground could not be built from the document's marks and
    /// recipe: a mark with a team that is neither side, a kit of another
    /// version, a map with no room for it even with every class cut, a
    /// class its donor cannot give.
    #[error("forge_ground: {0}")]
    Ground(String),
    /// The disc's entry is not the one an edit was made on: its typed
    /// fields give another print (docs/game/forge-map-variants.md, "Rules
    /// and limits"). Nothing is built.
    #[error(
        "variant_print: the map's {} entry {index} is not the one this edit was made on",
        .block.name()
    )]
    Print { block: BlockKind, index: u32 },
}

/// Bytes and manifest produced by the document. Saving is a separate action
/// so the CLI can fully check a document without writing a package.
pub struct Built {
    manifest: Manifest,
    document: String,
    map: Vec<u8>,
    pub reserves: Vec<Reservation>,
    /// What the AI ground build made, for a document that asks for it.
    pub ground: Option<GroundReport>,
    /// The block-out definitions installed and what the cut order dropped,
    /// for a document with a piece or a kit build.
    pub blockout: Option<blockout::Report>,
    /// Every base entry the document names with a print, each compared
    /// with the disc's row and found equal, by block then index.
    pub prints: Vec<Printed>,
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
/// original maps by name. The hash checks use those exact bytes. A document
/// whose objects have game types needs [`build_for`].
pub fn build(
    doc: &ForgeDoc,
    lookup: impl FnMut(&str) -> Option<MapSource>,
) -> Result<Built, Error> {
    build_for(doc, None, lookup)
}

/// The `MapId` of `doc` built for the engine at variant index `engine`:
/// the engine is in it only when the document's objects have game types
/// (docs/game/forge-map-variants.md, "Game types").
pub fn map_id(doc: &ForgeDoc, engine: Option<i32>) -> Result<MapId, Error> {
    Ok(MapId::forge_game(
        doc.base.sha256,
        doc.id()?,
        entry::game(doc, engine)?,
    ))
}

/// [`build`] for the engine at variant index `engine` (Slayer 2, Capture
/// the Flag 1, `forge_doc::games::ENGINES`): an object whose `games` set
/// lacks it is left out of the map, and the package's identity names the
/// engine. A document whose objects have no game types ignores `engine`
/// and builds the one map it always does.
pub fn build_for(
    doc: &ForgeDoc,
    engine: Option<i32>,
    lookup: impl FnMut(&str) -> Option<MapSource>,
) -> Result<Built, Error> {
    build_with(doc, engine, &ClassTable::retail(), lookup)
}

/// [`build_for`] with the ground's enemy classes copied by `table` (the
/// game's is [`ClassTable::retail`]): the one seam a test uses to copy an
/// invented donor's classes.
pub fn build_with(
    doc: &ForgeDoc,
    engine: Option<i32>,
    table: &ClassTable<'_>,
    mut lookup: impl FnMut(&str) -> Option<MapSource>,
) -> Result<Built, Error> {
    let mut limits = Limits {
        builder: Some(VERSIONS),
        ..Limits::default()
    };
    forge_doc::validate(doc, &limits)?;
    let document_hash = doc.id()?;
    let game = entry::game(doc, engine)?;
    let id = MapId::forge_game(doc.base.sha256, document_hash, game);
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
    let base_source = source.clone();
    if map.header().cache_type != CacheType::Multiplayer {
        return Err(Error::BaseType(doc.base.map.clone()));
    }
    for donor in &doc.sources {
        let source = lookup(&donor.map).ok_or_else(|| Error::MissingMap(donor.map.clone()))?;
        check_map(donor, &source, false)?;
    }
    let needs_kit = requests_kit(&doc.recipes)?;
    let asked = doc.ground_recipe()?;
    // The ground's class donors, each read once and checked against the
    // recipe's hash before anything is built: a machine whose disc lacks
    // one, or has another, is refused by the donor's name.
    let mut donors: Vec<(String, Arc<[u8]>)> = Vec::new();
    for class in asked.iter().flat_map(|r| &r.classes) {
        if donors
            .iter()
            .any(|(map, _)| map.eq_ignore_ascii_case(&class.donor.map))
        {
            continue;
        }
        let source =
            lookup(&class.donor.map).ok_or_else(|| Error::MissingMap(class.donor.map.clone()))?;
        donors.push((class.donor.map.clone(), check_map(&class.donor, &source, false)?));
    }
    if needs_kit && asked.is_some() {
        return Err(Error::Recipe(format!(
            "{} and {KIT_RECIPE} are not built together: the ground is built over the play form",
            forge_doc::GROUND_RECIPE
        )));
    }

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
    let flags = BlockKind::NetgameFlags;
    limits.base_flag_types = (0..scenario.block(flags).len())
        .map(|index| {
            scenario
                .get(flags, index, "type")?
                .as_int()
                .and_then(|kind| u16::try_from(kind).ok())
                .ok_or(scenario_edit::Error::Invalid("netgame flag type is out of range"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into();
    limits.world = placement::world_bounds(&mut map)?;
    // The objects of another game type are removed after every op; the
    // document itself, its hash and its file are as authored.
    let for_game = match game {
        Some(game) => std::borrow::Cow::Owned(forge_doc::for_game(
            doc,
            i32::from(game),
            &limits.base_counts,
        )?),
        None => std::borrow::Cow::Borrowed(doc),
    };

    // Block-out definitions are tags of the map: they are installed on the
    // base, with their crate palette rows, and the scenario is opened again
    // on the map that gives, so every placement edit goes through it.
    let has_pieces = for_game
        .ops
        .iter()
        .any(|op| matches!(op.edit, Edit::Forge(Forge::Piece { .. })));
    let needed = if has_pieces {
        blockout::definitions(&for_game.pieces(&limits.base_counts)?)
    } else {
        Vec::new()
    };
    let mut blockout_report = None;
    if !needed.is_empty() || needs_kit {
        let base = base_source
            .bytes
            .clone()
            .ok_or(scenario_edit::Error::Invalid("the base map is not in memory"))?;
        let installed =
            blockout::install(&base, &needed, needs_kit, blockout::headroom(&base)?)?;
        if let Some(bytes) = installed.bytes {
            let mut rebuilt = base_source;
            rebuilt.bytes = Some(bytes.into());
            map = rebuilt.open()?;
            session = Session::new();
            session.build_scenes = false;
            session.open_named(rebuilt, &doc.base.map)?;
        }
        blockout_report = Some(installed.report);
    }
    let piece_rows = blockout_report
        .as_ref()
        .map(blockout::Report::palette_rows)
        .unwrap_or_default();

    let mut apply =
        placement::Materialiser::new(&mut session, &mut map).with_pieces(piece_rows);
    forge_doc::replay(&for_game, &limits, &mut apply).map_err(|refusal| match refusal {
        Refusal::Print { block, index } => Error::Print { block, index },
        refusal => Error::Document(refusal),
    })?;
    let placement::Finished {
        strings: mut new_strings,
        device_groups,
        prints,
    } = apply.finish()?;
    if !needs_kit {
        session.set_tag_strings(std::mem::take(&mut new_strings))?;
    }
    let scenario = session
        .scenario()
        .ok_or(scenario_edit::Error::Invalid("scenario is not open"))?;
    let mut output = Cursor::new(Vec::new());
    save::write_patched(&mut map, scenario, &mut output)?;
    let mut output = links::device_groups(output.into_inner(), &device_groups)?;
    let mut reserves = Vec::new();
    if needs_kit {
        // The writer places new string tables after the metadata. Finish
        // the kit's metadata growth before writing those tables.
        (output, reserves) = kit::reserve(output, &new_strings)?;
    }
    // The ground is built over the materialised map, from the marks the
    // finished scenario's flags carry. Its bytes are the map.
    let mut report = None;
    if let Some(recipe) = asked {
        let marks = doc.ground_marks(&limits.base_counts)?;
        let labels = ground::labels(
            session
                .scenario()
                .ok_or(scenario_edit::Error::Invalid("scenario is not open"))?,
            &marks,
        )?;
        let specs = recipe
            .classes
            .iter()
            .map(|class| -> Result<firefight_kit::ClassSpec, Error> {
                Ok(firefight_kit::ClassSpec {
                    name: class.name.clone(),
                    donor: donors
                        .iter()
                        .position(|(map, _)| map.eq_ignore_ascii_case(&class.donor.map))
                        .expect("every donor was read above"),
                    path: table.path(&class.name)?.to_owned(),
                    // The ground's enemy classes are the retail Covenant
                    // table (`forge_doc::waves::CLASSES`).
                    faction: firefight_kit::Faction::Covenant,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let classes = (!specs.is_empty()).then(|| firefight_kit::Classes {
            schema: table.schema,
            donors: donors
                .iter()
                .map(|(name, bytes)| firefight_kit::Donor {
                    name: name.as_str(),
                    bytes: &bytes[..],
                })
                .collect(),
            specs,
            cut_to_fit: true,
        });
        let built = firefight_kit::ground::build_with(
            &output,
            &doc.base.map,
            &labels,
            &ground::recipe(&recipe),
            classes.as_ref(),
        )
        .map_err(|e| Error::Ground(format!("{e:#}")))?;
        report = Some(GroundReport {
            zones: built.zones,
            squads: built.squads,
            problems: built.problems,
            classes: built.classes,
            cut: built.cut,
            costs: built.costs,
            receipt: built.receipt,
            sectors: ground::sectors_of(&built.bytes)?,
        });
        output = built.bytes;
    }
    // The areas and the shown flags the document gives its flags, in the
    // areas record at the map's tail.
    let record = areas::record(
        &for_game,
        &limits.base_counts,
        session
            .scenario()
            .ok_or(scenario_edit::Error::Invalid("scenario is not open"))?,
    )?;
    if let Some(record) = record {
        output.extend_from_slice(&record);
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
        game,
    });
    Ok(Built {
        manifest,
        document: doc.canonical_json()?,
        map: output,
        reserves,
        ground: report,
        blockout: blockout_report,
        prints,
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
            // Built after the scenario (`GroundReport`); the document's
            // check has already refused one that is malformed.
            Recipe::Generate { generator, .. } if generator == forge_doc::GROUND_RECIPE => {}
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
