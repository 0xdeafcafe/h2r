//! The Modeling tool on the app (docs/editor/modeling.md): the open map's
//! geometry session, its undo, the writes it hands the save and the live
//! preview the viewport redraws.

use super::App;
use geometry_edit::mesh_edit::MeshOp;
use geometry_edit::{GeometrySession, Preview, Target, TargetInfo};
use scenario_edit::geometry::RawFiles;
use scenario_edit::{BspPatch, GeometryChanges, GeometryGrown, RawPatch};

/// Why a save with geometry edits is refused while the session switch is
/// off.
pub const UNVERIFIED_REFUSAL: &str =
    "geometry writes are unverified: turn on Edit > Allow unverified geometry writes to save them";

/// The Modeling tool's state.
#[derive(Default)]
pub struct Modeling {
    pub session: GeometrySession,
    /// "Allow unverified geometry writes": off by default, and while off
    /// a save with geometry edits is refused.
    pub allow_unverified: bool,
    /// Level collision edits by structure: the blocks a solid added to a
    /// structure's collision makes (`geometry_edit::level_collision`).
    /// Play sends them to a paired game (`play::collision_sync`).
    pub level_collision: std::collections::BTreeMap<map::TagId, geometry_edit::level_collision::LevelEdit>,
    /// Bumped on every change to [`Self::level_collision`].
    pub level_collision_rev: u64,
    /// The map the session is over (`Session::world_rev`).
    for_rev: Option<u64>,
}

/// One Modeling command.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelAction {
    /// Open a target in edit mode (reading its section).
    Open(Target),
    /// Leave edit mode on a target; its edits stay.
    Close(Target),
    /// Apply ops to an open target as one undo step, or one merged step
    /// per `merge` key until [`ModelAction::Seal`].
    Apply {
        target: Target,
        ops: Vec<MeshOp>,
        merge: Option<u64>,
    },
    /// Replace a collision BSP with the current mesh of a render section.
    CollisionFromRender {
        render: Target,
        target: Target,
        materials: Vec<u16>,
    },
    /// End a merged run (a gizmo drag).
    Seal,
    Undo,
    Redo,
    /// **Use for lower LODs** on a model section: point every lower level
    /// of detail of each permutation that draws it at it (`on`), or leave
    /// them as they were. One undo step, saved as patches of the
    /// permutations' LOD indices
    /// (`geometry_edit::GeometrySession::set_use_for_lower_lods`).
    UseForLowerLods { target: Target, on: bool },
    /// Add a closed box solid inside `[lo, hi]` to structure `structure`'s
    /// level collision, onto the solids added before
    /// (`geometry_edit::level_collision::LevelEdit::add_box`). A paired
    /// game gets it live (`play::collision_sync`); a save writes it.
    AddCollisionBox {
        structure: map::TagId,
        lo: [f32; 3],
        hi: [f32; 3],
        material: u16,
    },
    /// Drop every solid added to `structure`'s level collision: the file's
    /// collision again, in the save and in a paired game.
    ClearCollision { structure: map::TagId },
}

impl App {
    /// Drop the geometry session when another map (or none) opens.
    pub(crate) fn sync_modeling(&mut self) {
        let rev = Some(self.session.world_rev());
        if self.modeling.for_rev != rev {
            self.modeling.session.clear();
            self.modeling.level_collision.clear();
            self.modeling.level_collision_rev += 1;
            self.modeling.for_rev = rev;
        }
    }

    /// Set (or, with `None`, drop) structure `tag`'s level collision edit.
    pub fn set_level_collision(&mut self, tag: map::TagId, edit: Option<geometry_edit::level_collision::LevelEdit>) {
        self.sync_modeling();
        match edit {
            Some(e) => self.modeling.level_collision.insert(tag, e),
            None => self.modeling.level_collision.remove(&tag),
        };
        self.modeling.level_collision_rev += 1;
    }

    /// The open map's structure targets, and a render model's sections
    /// when `model` names one.
    pub fn modeling_targets(
        &mut self,
        model: Option<map::TagId>,
    ) -> Result<Vec<TargetInfo>, String> {
        let source = self.session.source().cloned().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        let Some(model) = model else {
            return Ok(geometry_edit::list(&mut map));
        };
        let mut raw = RawFiles::open(&source).map_err(|e| e.to_string())?;
        geometry_edit::list_model(&mut map, &mut raw, model).map_err(|e| e.to_string())
    }

    pub(super) fn run_model(&mut self, action: ModelAction) {
        self.sync_modeling();
        let r = match action {
            ModelAction::Open(t) => self.open_model_target(t),
            ModelAction::Close(t) => {
                self.modeling.session.close(t);
                Ok(())
            }
            ModelAction::Apply { target, ops, merge } => self
                .modeling
                .session
                .apply(target, &ops, merge)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            ModelAction::CollisionFromRender { render, target, materials } => self
                .modeling
                .session
                .collision_from_render(render, target, &materials)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            ModelAction::Seal => {
                self.modeling.session.seal();
                Ok(())
            }
            ModelAction::Undo | ModelAction::Redo => {
                let undo = action == ModelAction::Undo;
                let done = if undo {
                    self.modeling.session.undo()
                } else {
                    self.modeling.session.redo()
                };
                let msg = match (done, undo) {
                    (true, true) => "undid the geometry edit",
                    (true, false) => "redid the geometry edit",
                    (false, true) => "nothing to undo",
                    (false, false) => "nothing to redo",
                };
                self.info(msg);
                Ok(())
            }
            ModelAction::UseForLowerLods { target, on } => self
                .modeling
                .session
                .set_use_for_lower_lods(target, on)
                .map(|changed| {
                    self.info(match (changed, on) {
                        (true, true) => format!("{}: lower levels of detail use this section", target.id()),
                        (true, false) => format!("{}: lower levels of detail as the file has them", target.id()),
                        (false, _) => format!("{}: no change to the levels of detail", target.id()),
                    })
                })
                .map_err(|e| e.to_string()),
            ModelAction::AddCollisionBox {
                structure,
                lo,
                hi,
                material,
            } => self.add_collision_box(structure, lo, hi, material),
            ModelAction::ClearCollision { structure } => {
                let had = self.modeling.level_collision.contains_key(&structure);
                self.set_level_collision(structure, None);
                self.info(if had {
                    format!("{:#010x}: level collision as the file holds it", structure.0)
                } else {
                    format!("{:#010x}: no level collision edit to clear", structure.0)
                });
                Ok(())
            }
        };
        if let Err(e) = r {
            self.error(format!("modeling: {e}"));
        }
        self.push_geometry_changes();
    }

    /// [`ModelAction::AddCollisionBox`]: the structure's edit with the box
    /// added, its level read from the open map the first time.
    fn add_collision_box(&mut self, structure: map::TagId, lo: [f32; 3], hi: [f32; 3], material: u16) -> Result<(), String> {
        let edit = match self.modeling.level_collision.get(&structure) {
            Some(edit) => edit.add_box(lo, hi, material),
            None => {
                let source = self.session.source().cloned().ok_or("no map is open")?;
                let mut map = source.open().map_err(|e| e.to_string())?;
                geometry_edit::level_collision::read(&mut map, structure).and_then(|s| s.add_box(lo, hi, material))
            }
        }
        .map_err(|e| e.to_string())?;
        let surfaces = edit.added.level.bsp.surfaces.len();
        let was = edit.source.level.bsp.surfaces.len();
        self.set_level_collision(structure, Some(edit));
        self.info(format!(
            "{:#010x}: added a collision box ({} surfaces, {was} in the file)",
            structure.0, surfaces
        ));
        Ok(())
    }

    fn open_model_target(&mut self, t: Target) -> Result<(), String> {
        let source = self.session.source().cloned().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        let mut raw = RawFiles::open(&source).map_err(|e| e.to_string())?;
        let info = self
            .modeling
            .session
            .open(&mut map, &mut raw, t)
            .map_err(|e| e.to_string())?;
        self.info(format!(
            "opened {}: {} vertices, {} faces{}",
            t.id(),
            info.verts,
            info.faces,
            if info.local {
                ""
            } else {
                " (in a shared cache: read only)"
            }
        ));
        Ok(())
    }

    /// Set "Allow unverified geometry writes", and the save's refusal with
    /// it.
    pub(super) fn set_allow_unverified_geometry(&mut self, allow: bool) {
        self.modeling.allow_unverified = allow;
        self.push_geometry_changes();
    }

    /// Hand the geometry session's writes to the save: tag patches in
    /// the metadata or a BSP region, raw block patches, or why the save
    /// is refused (an edit that cannot be written in place, or the
    /// switch off). An unedited session hands nothing.
    pub(crate) fn push_geometry_changes(&mut self) {
        if !self.session.is_open() {
            return;
        }
        let c = self.geometry_changes();
        let r = self.session.set_geometry_changes(c);
        self.ok_or_log(r);
    }

    /// Why a save would be refused for the geometry edits now, if it would.
    pub fn geometry_save_refusal(&self) -> Option<String> {
        self.geometry_changes().refusal
    }

    /// What [`Self::push_geometry_changes`] hands the save: the session's
    /// writes and each level collision edit's (patches in place, grown
    /// blocks in the metadata).
    fn geometry_changes(&self) -> GeometryChanges {
        let g = &self.modeling.session;
        let mut c = GeometryChanges::default();
        let edited = g.has_changes() || !self.modeling.level_collision.is_empty();
        if g.has_changes() {
            match g.changes() {
                Ok(p) => {
                    // A grown section's dependent writes save as ordinary
                    // patches; its block and pointer word as a raw append.
                    let mut tags = p.tags;
                    for a in p.appended {
                        tags.extend(a.writes);
                        c.raw_appends.push(scenario_edit::RawAppend {
                            bytes: a.bytes,
                            align: a.align,
                            pointer_space: a.pointer_space,
                            pointer_address: a.pointer_address,
                        });
                    }
                    for w in tags {
                        match w.space {
                            None => c.patches.push((w.address, w.bytes)),
                            Some(space) => c.bsp_patches.push(BspPatch {
                                space,
                                address: w.address,
                                bytes: w.bytes,
                            }),
                        }
                    }
                    c.grown.extend(p.grown.into_iter().map(|g| GeometryGrown {
                        pair_space: g.pair_space,
                        pair: g.pair,
                        count: g.count,
                        bytes: g.bytes,
                        align: g.align,
                    }));
                    c.raw_patches = p
                        .raw
                        .into_iter()
                        .map(|w| RawPatch {
                            block: w.block,
                            block_size: w.block_size,
                            offset: w.offset,
                            bytes: w.bytes,
                        })
                        .collect();
                }
                Err(e) => c.refusal = Some(e.to_string()),
            }
        }
        for edit in self.modeling.level_collision.values() {
            let w = edit.writes();
            for p in w.patches {
                match p.space {
                    None => c.patches.push((p.address, p.bytes)),
                    Some(space) => c.bsp_patches.push(BspPatch {
                        space,
                        address: p.address,
                        bytes: p.bytes,
                    }),
                }
            }
            c.grown.extend(w.grown.into_iter().map(|g| GeometryGrown {
                pair_space: g.pair_space,
                pair: g.pair,
                count: g.count,
                bytes: g.bytes,
                align: g.align,
            }));
        }
        if edited && c.refusal.is_none() && !self.modeling.allow_unverified {
            c.refusal = Some(UNVERIFIED_REFUSAL.to_owned());
        }
        c
    }

    /// A revision that changes with every geometry edit, undo or redo
    /// (the viewport redraws [`Self::modeling_previews`] when it does).
    pub fn modeling_rev(&self) -> u64 {
        self.modeling.session.rev()
    }

    /// What the viewport redraws for the open structure targets: none
    /// while the session is still over a map no longer open.
    pub fn modeling_previews(&self) -> Vec<Preview> {
        if self.modeling.for_rev != Some(self.session.world_rev()) {
            return Vec::new();
        }
        self.modeling.session.previews()
    }
}
