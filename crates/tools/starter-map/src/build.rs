//! The build (crate docs): the floor's fresh map, the pieces, the
//! placements, and the derived package.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use content::manifest::{ListInfo, MANIFEST_FILE};
use map::{FourCC, Map, ResourceFile};
use scenario_edit::recipe::{self, Anchor, Builder, Clear as Blocks, Fidelity, Place, Recipe};
use scenario_edit::{PackageForm, Session};
use serde::Serialize;
use content_kit::stage::kit::{Facts, Keys};
use content_kit::stage::map::{
    Base, Over, Project, Report as StageReport, edit_scenario, empty_block, git_checkout,
    keys_free, lost_block,
};
use tags::repo::{Body, MapSource, SharedFiles, TagKey, TagSource};
use tags::schema::Schema;

use crate::ai;
use crate::coverage::Census;
use crate::template::{Kind, Mode, Template, map_name};
use crate::{Error, Result};

/// The shared caches a base reads resources from, by file stem (as
/// `content_kit::stage::fresh` reads them).
pub const SHARED: [(&str, ResourceFile); 3] = [
    ("mainmenu", ResourceFile::MainMenu),
    ("shared", ResourceFile::Shared),
    ("single_player_shared", ResourceFile::SinglePlayerShared),
];

/// What the user asked for: a title, the map name it gives, and a kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The map's file and package name ([`map_name`] of the title).
    pub name: String,
    /// The package's display name.
    pub title: String,
    pub kind: Kind,
    /// Where the shared caches are for the editor's session (the disc
    /// image or a maps folder); `None` when the map needs none.
    pub shared: Option<PathBuf>,
}

impl Request {
    pub fn new(title: &str, kind: Kind) -> Result<Request> {
        Ok(Request {
            name: map_name(title)?,
            title: title.trim().to_owned(),
            kind,
            shared: None,
        })
    }
}

/// One piece as placed: its template name, its definition's canonical
/// name, its crate palette row and its corner.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PieceRow {
    pub name: String,
    pub def: String,
    pub row: u32,
    pub at: [f32; 3],
}

/// The build report, `report.json` in the package.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    /// SHA-256 over the version, the template, the request and the
    /// base's name and header.
    pub key: String,
    pub version: u32,
    pub name: String,
    pub title: String,
    pub kind: Kind,
    pub base: String,
    pub scenario: String,
    pub pieces: Vec<PieceRow>,
    /// What the pieces' tags and palette took, and the headroom they fit.
    pub pieces_tag_memory: u64,
    pub headroom: u64,
    /// Entries per scenario block after the placements.
    pub placed: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    /// The modes the map starts (multiplayer).
    pub modes: Vec<Mode>,
    /// The squad, zone and order (campaign).
    pub ai: Option<ai::Written>,
    /// The floor's fresh build.
    pub stage: StageReport,
}

/// A built map, open in a session over its bytes.
pub struct Built {
    pub request: Request,
    pub base: String,
    pub description: String,
    pub session: Session,
    pub report: Report,
}

/// What [`write`] wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Written {
    pub name: String,
    pub kind: Kind,
    pub dir: PathBuf,
    /// The package's `MapId`, which `maps.load` takes.
    pub map_id: String,
    pub scenario: String,
}

fn base_err(e: impl std::fmt::Display) -> Error {
    Error::Base(e.to_string())
}

/// Builds `req` from `template` over the base map `disc` reads, by file
/// stem: the base itself and, when it leans on them, the shared caches.
pub fn build(
    template: &Template,
    req: &Request,
    disc: &dyn Fn(&str) -> std::result::Result<Vec<u8>, String>,
) -> Result<Built> {
    let common = template.common(req.kind);
    let base_bytes = disc(common.base)
        .map_err(|e| Error::Base(format!("the base map {}: {e}", common.base)))?;
    let schema = Schema::builtin();
    let mut shared = SharedFiles::default();
    for (stem, file) in SHARED {
        // A map that does not lean on a cache needs none of them; one that
        // does stops the build naming the pointer.
        let Ok(bytes) = disc(stem) else {
            continue;
        };
        let map = Map::from_reader(Cursor::new(bytes))
            .map_err(|e| Error::Base(format!("the shared cache {stem}: {e}")))?;
        shared
            .set(schema, file, map)
            .map_err(|e| Error::Base(format!("the shared cache {stem}: {e}")))?;
    }
    let map = Map::from_reader(Cursor::new(base_bytes))
        .map_err(|e| Error::Base(format!("the base map {}: {e}", common.base)))?;
    let mut src = MapSource::new(schema, map, &mut shared).map_err(base_err)?;
    let base = Base::from_map(&mut src, common.base)?;
    build_over(template, req, &mut src, &base)
}

/// The build key (see [`Report::key`]).
pub fn build_key(template: &Template, req: &Request, base: &Base) -> String {
    let mut text = Vec::new();
    text.extend_from_slice(&crate::VERSION.to_le_bytes());
    text.extend_from_slice(&content_kit::stage::version().to_le_bytes());
    let t = serde_json::to_string(template).unwrap_or_default();
    for part in [t.as_str(), req.name.as_str(), req.kind.name(), base.map.as_str()] {
        text.extend_from_slice(&(part.len() as u64).to_le_bytes());
        text.extend_from_slice(part.as_bytes());
    }
    text.extend_from_slice(&base.header);
    tags::repo::hex(&tags::repo::sha256(&text))
}

/// [`build`] over any tag source: a disc map, the extracted tag tree
/// (`Base::from_repo`), or a tag-workspace project's source.
pub fn build_over(
    template: &Template,
    req: &Request,
    src: &mut dyn TagSource,
    base: &Base,
) -> Result<Built> {
    let kind = req.kind;
    let common = template.common(kind);
    if map_name(&req.name)? != req.name {
        return Err(Error::Name(format!("{:?} is not a map name", req.name)));
    }

    // 1. The floor.
    let facts = Facts::read_for(src, &base.globals, common.biped)?;
    let layout = template.layout(facts.player)?;
    let scenario = common.scenario_path(&req.name);
    let keys = Keys::new(&scenario);
    keys_free(base, &keys)?;
    let mut scnr = src
        .read(&base.scenario)
        .map_err(content_kit::stage::Error::from)?
        .ok_or_else(|| Error::Base(format!("{}: no {}", base.map, base.scenario)))?;
    let mut cleared = Vec::new();
    if kind == Kind::Campaign {
        let Body::Tree(tree) = &mut scnr.body else {
            return Err(Error::Base(format!("{} is an alias", base.scenario)));
        };
        let mut offsets = Vec::new();
        for c in &template.campaign.clear {
            offsets.push((c.offset()?, c.name.clone()));
        }
        let root_len = tree.nodes.first().map_or(0, |n| n.bytes.len());
        if let Some((at, name)) = offsets.iter().find(|(at, _)| *at as usize + 8 > root_len) {
            return Err(Error::Base(format!(
                "{}: the root is {root_len:#x} bytes, too short for {name} (+{at:#x})",
                base.scenario
            )));
        }
        // What extraction lost in a block emptied here does not matter.
        tree.losses
            .retain(|line| !lost_block(line).is_some_and(|o| offsets.iter().any(|(at, _)| *at == o)));
        for (at, name) in &offsets {
            cleared.push((name.clone(), empty_block(tree, 0, *at)));
        }
    }
    let (mut scnr, mut scenario_edit) = edit_scenario(scnr, base, &keys, &layout)?;
    scenario_edit.cleared.extend(cleared);
    let mut roots = Vec::new();
    let written_ai = match kind {
        Kind::Campaign => {
            let Body::Tree(tree) = &mut scnr.body else {
                return Err(Error::Base(format!("{} is an alias", base.scenario)));
            };
            let spec = &template.campaign.ai;
            roots.push(TagKey::new(ai::CHAR, spec.character.clone()));
            Some(ai::write(tree, spec)?)
        }
        Kind::Multiplayer => {
            for it in &template.multiplayer.items {
                let (group, path) = it.key()?;
                roots.push(TagKey::new(FourCC(group), path));
            }
            None
        }
    };
    let project = Project {
        name: req.name.clone(),
        mode: "fresh".into(),
        base: base.map.clone(),
        scenario: scenario.clone(),
        kits: vec!["new-map".into()],
    };
    let key = build_key(template, req, base);
    let floor = content_kit::stage::map::build_over(
        src,
        base,
        &project,
        Over {
            layout,
            facts,
            keys,
            scenario: scnr,
            scenario_edit,
            roots,
            key: key.clone(),
        },
    )?;

    // 2. The pieces.
    let defs = template.defs();
    let headroom = forge_build::blockout::headroom(&floor.bytes)
        .map_err(|e| Error::Pieces(e.to_string()))?;
    let installed = forge_build::blockout::install(&floor.bytes, &defs, false, headroom)
        .map_err(|e| Error::Pieces(e.to_string()))?;
    let rows = installed.report.palette_rows();
    let mut pieces = Vec::new();
    for p in &template.pieces {
        let def = p.def().name();
        let row = *rows
            .get(&def)
            .ok_or_else(|| Error::Pieces(format!("piece {}: {def} has no crate row", p.name)))?;
        pieces.push(PieceRow {
            name: p.name.clone(),
            def,
            row,
            at: p.at,
        });
    }
    let bytes = installed
        .bytes
        .ok_or_else(|| Error::Pieces("the block-out build added nothing".into()))?;

    // 3. The placements.
    let (session, placed) = place(template, req, bytes)?;
    let modes = match kind {
        Kind::Multiplayer => template.multiplayer.modes.clone(),
        Kind::Campaign => Vec::new(),
    };
    let report = Report {
        key,
        version: crate::VERSION,
        name: req.name.clone(),
        title: req.title.clone(),
        kind,
        base: base.map.clone(),
        scenario,
        pieces,
        pieces_tag_memory: installed.report.tag_memory,
        headroom,
        placed: placed.counts,
        notes: placed.notes,
        modes,
        ai: written_ai,
        stage: floor.report,
    };
    Ok(Built {
        request: req.clone(),
        base: base.map.clone(),
        description: template.description.clone(),
        session,
        report,
    })
}

/// A placement of `block` at a world point, nothing else set.
fn at(block: &str, at: [f32; 3], facing: f32) -> Place {
    Place {
        block: block.into(),
        at,
        facing,
        grid: None,
        tag: None,
        each: None,
        step: [0.0; 3],
        floor: false,
        team: None,
        teams: None,
        fields: BTreeMap::new(),
    }
}

/// The recipe that places the template's pieces and spawns for `kind`,
/// from the world origin, every point as the template gives it.
pub fn recipe(template: &Template, req: &Request) -> Result<Recipe> {
    let mut place = Vec::new();
    for p in &template.pieces {
        let mut x = at("crates", p.at, p.facing);
        x.tag = Some(p.def().tag_path());
        place.push(x);
    }
    match req.kind {
        Kind::Multiplayer => {
            let mp = &template.multiplayer;
            for s in &mp.spawns {
                let mut x = at("starting_locations", s.at, s.facing);
                x.team = s.team;
                place.push(x);
            }
            for f in &mp.flags {
                let mut x = at("netgame_flags", f.at, f.facing);
                x.team = f.team;
                x.fields
                    .insert("type".into(), serde_json::json!(f.flag_type()));
                place.push(x);
            }
            for it in &mp.items {
                let (_, path) = it.key()?;
                let mut x = at("netgame_equipment", it.at, it.facing);
                x.tag = Some(path.to_owned());
                place.push(x);
            }
        }
        Kind::Campaign => {
            let s = &template.campaign.start;
            place.push(at("starting_locations", s.at, s.facing));
        }
    }
    let r = Recipe {
        name: "new-map".into(),
        title: template.title.clone(),
        purpose: template.description.clone(),
        base: req.name.clone(),
        fidelity: Fidelity::Full,
        builder: Builder::Placements,
        note: None,
        variant: "slayer".into(),
        anchor: Anchor {
            from: None,
            index: 0,
            at: Some([0.0; 3]),
            facing: Some(0.0),
        },
        clear: Blocks::All("all".into()),
        allow_growth: true,
        set: Vec::new(),
        place,
        expect: Vec::new(),
    };
    r.validate().map_err(|e| Error::Place(e.to_string()))?;
    Ok(r)
}

/// Opens `bytes` (the floor with its pieces) as a new map and places the
/// recipe through the session. Every tag the template names must be in
/// the map by its exact path, and the scenario read back must hold what
/// the kind needs.
pub fn place(template: &Template, req: &Request, bytes: Vec<u8>) -> Result<(Session, recipe::Built)> {
    let source = scenario_edit::MapSource {
        path: PathBuf::from(format!("{}.map", req.name)),
        region: None,
        bytes: Some(Arc::from(bytes)),
        shared: req.shared.clone(),
    };
    let mut s = Session::new();
    s.build_scenes = false;
    s.open_derived(source, &req.name, None, "")
        .map_err(|e| Error::Place(format!("opening the built map: {e}")))?;
    let sc = s
        .scenario()
        .ok_or_else(|| Error::Place("the built map has no scenario".into()))?;
    let held = |group: [u8; 4], path: &str| {
        sc.tags
            .values()
            .any(|t| t.group.0 == group && t.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(path)))
    };
    let mut named: Vec<([u8; 4], String)> = template
        .pieces
        .iter()
        .map(|p| (*b"bloc", p.def().tag_path()))
        .collect();
    if req.kind == Kind::Multiplayer {
        for it in &template.multiplayer.items {
            let (g, p) = it.key()?;
            named.push((g, p.to_owned()));
        }
    }
    if let Some((g, p)) = named.iter().find(|(g, p)| !held(*g, p)) {
        return Err(Error::Place(format!(
            "the built map has no {}:{p}",
            String::from_utf8_lossy(g)
        )));
    }
    let r = recipe(template, req)?;
    let placed = recipe::build(&mut s, &r).map_err(|e| Error::Place(e.to_string()))?;
    let sc = s
        .scenario()
        .ok_or_else(|| Error::Place("the built map has no scenario".into()))?;
    check(template, req.kind, sc)?;
    Ok((s, placed))
}

/// The scenario holds what `kind` needs: every claimed mode's placements
/// and the equipment for a multiplayer map ([`Census`]); one start and one
/// crate a piece for both.
pub fn check(template: &Template, kind: Kind, sc: &scenario_edit::Scenario) -> Result<()> {
    use scenario_edit::BlockKind;
    let crates = sc.block(BlockKind::Crates).len();
    if crates != template.pieces.len() {
        return Err(Error::Coverage(format!(
            "{crates} crates for {} pieces",
            template.pieces.len()
        )));
    }
    let starts = sc.block(BlockKind::StartingLocations).len();
    match kind {
        Kind::Multiplayer => {
            let mp = &template.multiplayer;
            if starts != mp.spawns.len() {
                return Err(Error::Coverage(format!(
                    "{starts} starting locations for {} spawns",
                    mp.spawns.len()
                )));
            }
            Census::read(sc)?
                .covers(&mp.modes)
                .map_err(Error::Coverage)
        }
        Kind::Campaign if starts == 1 => Ok(()),
        Kind::Campaign => Err(Error::Coverage(format!(
            "{starts} starting locations; a campaign map has the player's one"
        ))),
    }
}

/// Writes `built` as a new map's package, `<data>/maps/<name>/`: the map
/// with its pieces as the first layer, the placements as the second
/// (`scenario_edit::package::save_package` with `derived`), and
/// `report.json`. A multiplayer map lists as its base's entry gives it
/// (`[list] base`). An existing package of the name needs `replace`; a
/// data directory in a Git checkout is refused.
pub fn write(built: &Built, data_root: &Path, replace: bool) -> Result<Written> {
    if let Some(repo) = git_checkout(data_root) {
        return Err(Error::Package(format!(
            "{} is in the Git checkout {}; new maps go to the user's data directory",
            data_root.display(),
            repo.display()
        )));
    }
    let req = &built.request;
    let maps = content::store::maps_dir(data_root);
    let form = PackageForm {
        name: req.title.clone(),
        description: built.description.clone(),
        dir: req.name.clone(),
        ..PackageForm::default()
    };
    let s = &built.session;
    let (Some(source), Some(sc)) = (s.source(), s.scenario()) else {
        return Err(Error::Package("the built map is not open".into()));
    };
    let saved = scenario_edit::package::save_package(
        &maps,
        &req.name,
        source,
        sc,
        &form,
        replace.then_some(form.dir.as_str()),
        true,
    )
    .map_err(|e| Error::Package(e.to_string()))?;
    let io = |e: std::io::Error| Error::Package(format!("{}: {e}", saved.dir.display()));
    if req.kind == Kind::Multiplayer {
        let mut m = saved.manifest.clone();
        m.list = Some(ListInfo {
            base: Some(built.base.clone()),
            sort: None,
            name: BTreeMap::new(),
            description: BTreeMap::new(),
        });
        content::store::write_atomic(&saved.dir.join(MANIFEST_FILE), m.to_toml().as_bytes())
            .map_err(io)?;
    }
    let report = serde_json::to_vec_pretty(&built.report)
        .map_err(|e| Error::Package(format!("the report: {e}")))?;
    content::store::write_atomic(&saved.dir.join("report.json"), &report).map_err(io)?;
    Ok(Written {
        name: req.name.clone(),
        kind: req.kind,
        dir: saved.dir.clone(),
        map_id: saved.id.to_string(),
        scenario: built.report.scenario.clone(),
    })
}
