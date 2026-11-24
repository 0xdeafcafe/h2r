//! AI ground in a session (docs/game/forge-map-variants.md, "AI ground";
//! docs/game/game-types-without-carriers.md, "Maps for scripted games").
//!
//! Ground labels are netgame flags the document adds and labels
//! ([`Session::place_marker`]); they place nothing in the world. The ground
//! itself is built only when asked: **Build AI ground**, **Test waves**
//! ([`Session::ensure_ground`]) or a save while it is stale
//! ([`Session::save_building`]), never while placing. The build is the
//! caller's ([`GroundBuilder`]), so this crate holds no generator: the game
//! runs it off its thread over the document's play form and reloads the map.
//!
//! The session keeps the recipe and the stamp of its last build beside the
//! document, not in it: the log's chain starts at the document, and the
//! recipe is the one thing a save adds. A save writes the document with the
//! recipe; a load takes it back out.
use std::path::Path;

use forge_doc::{
    BlockKind, Edit, Endpoint, F32, FieldWrite, Forge, ForgeDoc, GROUND_BLOCK, GroundLabel,
    GroundMark, GroundRecipe, Label, Op, OpId, Placement, Sha256, Value,
};

use super::{Session, files, reason, scene, vector};
use crate::{Frame, World};

/// Where the ground stands against the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundState {
    /// No build has been made in this session.
    NotBuilt,
    /// The last build covers what the document is now.
    Current,
    /// A mark moved or something solid changed since the last build: the
    /// ground in the loaded map is not the document's.
    Stale,
}

/// A label the ground build could not honour, as the overlay draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundProblem {
    /// The label's text: `ff_drop`, `siege_site`.
    pub label: String,
    pub position: [f32; 3],
    pub reason: String,
}

/// What a build made, for the overlay and the HUD.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroundReport {
    pub zones: usize,
    pub squads: usize,
    pub problems: Vec<GroundProblem>,
}

/// The build, supplied by the game: the Firefight kit's ground over the
/// materialised map.
pub trait GroundBuilder {
    /// The recipe a first build asks for: this build's kit version, red
    /// home, the kit's mode.
    fn recipe(&self) -> GroundRecipe;

    /// Build over `play`, the document's play form with the ground recipe,
    /// and reload the map with it. An `Err` is a sentence for the HUD: a
    /// map with no room for the ground names its cost and its biggest part.
    fn build(&mut self, play: &ForgeDoc) -> Result<GroundReport, String>;
}

/// The recipe and the last build, kept beside the document.
#[derive(Clone, Debug, Default)]
pub(super) struct Track {
    recipe: Option<GroundRecipe>,
    built: Option<Built>,
}

#[derive(Clone, Debug)]
struct Built {
    stamp: Sha256,
    report: GroundReport,
}

/// A ground marker the session holds: an added netgame flag and its label.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedMarker {
    pub id: OpId,
    pub label: GroundLabel,
    /// A `siege_line`'s phase.
    pub number: Option<i32>,
    /// Position, and the facing as `rotation[0]`.
    pub frame: Frame,
    /// 0 red, 1 blue.
    pub team: u8,
}

impl Session {
    /// The document with the ground recipe: what a save writes.
    pub fn ground_form(&self) -> ForgeDoc {
        self.doc.with_ground(self.ground.recipe)
    }

    pub fn ground_recipe(&self) -> Option<GroundRecipe> {
        self.ground.recipe
    }

    /// Change the recipe (the home team, the probe). The ground is stale
    /// until it is built again.
    pub fn set_ground_recipe(&mut self, recipe: Option<GroundRecipe>) {
        self.ground.recipe = recipe;
    }

    fn ground_stamp(&self) -> Result<Sha256, String> {
        self.ground_form()
            .ground_stamp(&self.limits.base_counts)
            .map_err(reason)
    }

    pub fn ground_state(&self) -> GroundState {
        match (&self.ground.built, self.ground_stamp()) {
            (None, _) => GroundState::NotBuilt,
            (Some(built), Ok(now)) if built.stamp == now => GroundState::Current,
            _ => GroundState::Stale,
        }
    }

    /// What the last build made.
    pub fn ground_report(&self) -> Option<&GroundReport> {
        self.ground.built.as_ref().map(|b| &b.report)
    }

    /// The ground labels the document has, by label then index.
    pub fn ground_marks(&self) -> Result<Vec<GroundMark>, String> {
        self.doc
            .ground_marks(&self.limits.base_counts)
            .map_err(reason)
    }

    /// How many of each ground label are placed, in [`GroundLabel::ALL`]
    /// order: the palette page's counts.
    pub fn ground_counts(&self) -> [usize; 6] {
        let mut counts = [0; 6];
        for mark in self.ground_marks().unwrap_or_default() {
            if let Some(i) = GroundLabel::ALL.iter().position(|l| *l == mark.kind) {
                counts[i] += 1;
            }
        }
        counts
    }

    /// Build the ground now: **Build AI ground**. The first build puts the
    /// builder's recipe in the session; a failed one leaves the recipe, the
    /// last build and so the state as they were.
    pub fn build_ground(
        &mut self,
        builder: &mut (impl GroundBuilder + ?Sized),
    ) -> Result<&GroundReport, String> {
        let before = self.ground.recipe;
        if before.is_none() {
            self.ground.recipe = Some(builder.recipe());
        }
        let stamp = self.ground_stamp();
        let play = forge_doc::play_form(&self.ground_form());
        let built =
            stamp.and_then(|stamp| builder.build(&play).map(|report| Built { stamp, report }));
        match built {
            Ok(built) => {
                let built = self.ground.built.insert(built);
                Ok(&built.report)
            }
            Err(error) => {
                self.ground.recipe = before;
                Err(error)
            }
        }
    }

    /// Build the ground unless it is current: **Test waves**. Answers
    /// whether it built.
    pub fn ensure_ground(
        &mut self,
        builder: &mut (impl GroundBuilder + ?Sized),
    ) -> Result<bool, String> {
        if self.ground_state() == GroundState::Current {
            return Ok(false);
        }
        self.build_ground(builder).map(|_| true)
    }

    /// Save, building first when the ground was built and is stale, so a
    /// saved variant never has a ground the build has not seen. A build
    /// that fails stops the save. A session that never built ground saves
    /// as it is: no map gets ground it was not given.
    pub fn save_building(
        &mut self,
        data_dir: &Path,
        name: &str,
        builder: &mut (impl GroundBuilder + ?Sized),
    ) -> Result<(), String> {
        self.build_if_stale(builder)?;
        self.save(data_dir, name)
    }

    /// [`Session::save_building`] to a new name ([`Session::save_new`]).
    pub fn save_new_building(
        &mut self,
        data_dir: &Path,
        base: &str,
        builder: &mut (impl GroundBuilder + ?Sized),
    ) -> Result<String, String> {
        self.build_if_stale(builder)?;
        self.save_new(data_dir, base)
    }

    fn build_if_stale(
        &mut self,
        builder: &mut (impl GroundBuilder + ?Sized),
    ) -> Result<(), String> {
        if self.ground_state() == GroundState::Stale {
            self.build_ground(builder)
                .map_err(|e| format!("The AI ground did not build, so nothing was saved. {e}"))?;
        }
        Ok(())
    }

    /// Take the ground recipe out of a loaded document: the session keeps
    /// it beside the document, and nothing is built yet.
    pub(super) fn take_ground(doc: ForgeDoc) -> Result<(ForgeDoc, Option<GroundRecipe>), String> {
        let recipe = doc.ground_recipe().map_err(reason)?;
        Ok((doc.with_ground(None), recipe))
    }

    pub(super) fn restart_ground(&mut self, recipe: Option<GroundRecipe>) {
        self.ground = Track {
            recipe,
            built: None,
        };
    }

    pub(super) fn write_ground_form(&self, data_dir: &Path, name: &str) -> Result<(), String> {
        files::save(data_dir, name, &self.ground_form())
    }

    pub(super) fn write_ground_form_new(
        &self,
        data_dir: &Path,
        base: &str,
    ) -> Result<String, String> {
        files::save_new(data_dir, base, &self.ground_form())
    }

    // ---- markers ----

    /// Place a ground marker: one netgame flag at `frame` (the facing is
    /// `rotation[0]`) for `team` (0 red, 1 blue), with its label, in one
    /// undo step. A `siege_line`'s `phase` is the label's number.
    pub fn place_marker(
        &mut self,
        world: &mut impl World,
        tick: u32,
        label: GroundLabel,
        frame: Frame,
        team: u8,
        phase: Option<i32>,
    ) -> Result<OpId, String> {
        let number = label.has_phase().then(|| phase.unwrap_or(0));
        let text = Label::new(label.text(), number).map_err(reason)?;
        let fields = vec![
            vector("position", frame.position)?,
            FieldWrite {
                field: "facing".into(),
                value: Value::Float(F32::new(frame.rotation[0]).map_err(reason)?),
            },
            FieldWrite {
                field: "team".into(),
                value: Value::Int(i64::from(team)),
            },
        ];
        let add = Edit::Placement(Placement::Add {
            block: GROUND_BLOCK,
            like: None,
            fields,
        });
        // The label names the add by its ID, which the ops fix.
        let unset = Edit::Forge(Forge::Label {
            target: Endpoint::Added(OpId(0)),
            label: Some(text.clone()),
        });
        let mut ops = self.ops(vec![add, unset])?;
        let id = ops[0].id;
        ops[1].edit = Edit::Forge(Forge::Label {
            target: Endpoint::Added(id),
            label: Some(text),
        });
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }

    /// Place the next marker of `label` where the monitor aims: a base or a
    /// siege line takes the team the placed ones leave short (red, then
    /// blue, as the build wants one base for each), and a siege line the
    /// phase its pair of sides is in.
    pub fn place_marker_next(
        &mut self,
        world: &mut impl World,
        tick: u32,
        label: GroundLabel,
        frame: Frame,
    ) -> Result<OpId, String> {
        let i = GroundLabel::ALL
            .iter()
            .position(|l| *l == label)
            .unwrap_or(0);
        let placed = self.ground_counts()[i];
        let team = if label.has_team() {
            (placed % 2) as u8
        } else {
            0
        };
        let phase = label.has_phase().then(|| (placed / 2) as i32);
        self.place_marker(world, tick, label, frame, team, phase)
    }

    /// The ground markers the document adds, with their labels.
    pub fn markers(&self) -> Result<Vec<PlacedMarker>, String> {
        let scene = scene::project(&self.doc, &self.limits, &self.palette, self.budget)?;
        let labels = self.doc.labels(&self.limits.base_counts).map_err(reason)?;
        Ok(scene
            .markers
            .iter()
            .filter_map(|m| {
                let active = labels
                    .active
                    .iter()
                    .find(|a| a.block == GROUND_BLOCK && a.index == m.index)?;
                Some(PlacedMarker {
                    id: m.id,
                    label: GroundLabel::of(&active.label.text)?,
                    number: active.label.number,
                    frame: m.frame,
                    team: m.team,
                })
            })
            .collect())
    }

    fn marker_index(&self, id: OpId) -> Result<(scene::Marker, u32), String> {
        let scene = scene::project(&self.doc, &self.limits, &self.palette, self.budget)?;
        let marker = scene
            .markers
            .into_iter()
            .find(|m| m.id == id)
            .ok_or("Aim at a ground marker.")?;
        let index = marker.index;
        Ok((marker, index))
    }

    /// Move and turn a marker, one set for each of position and facing
    /// that changes, in one undo step.
    pub fn move_marker(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: OpId,
        frame: Frame,
    ) -> Result<(), String> {
        let (marker, index) = self.marker_index(id)?;
        let mut edits = vec![];
        if marker.frame.position.map(f32::to_bits) != frame.position.map(f32::to_bits) {
            edits.push(Edit::Placement(Placement::Set {
                block: GROUND_BLOCK,
                index,
                write: vector("position", frame.position)?,
            }));
        }
        if marker.frame.rotation[0].to_bits() != frame.rotation[0].to_bits() {
            edits.push(Edit::Placement(Placement::Set {
                block: GROUND_BLOCK,
                index,
                write: FieldWrite {
                    field: "facing".into(),
                    value: Value::Float(F32::new(frame.rotation[0]).map_err(reason)?),
                },
            }));
        }
        if edits.is_empty() {
            return Ok(());
        }
        let ops: Vec<Op> = self.ops(edits)?;
        self.edit(world, tick, ops, None)
    }

    /// Remove a marker and its label.
    pub fn remove_marker(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: OpId,
    ) -> Result<(), String> {
        let (_, index) = self.marker_index(id)?;
        // The label goes first: a labelled entry cannot be removed.
        let ops = self.ops(vec![
            Edit::Forge(Forge::Label {
                target: Endpoint::Added(id),
                label: None,
            }),
            Edit::Placement(Placement::Remove {
                block: BlockKind::NetgameFlags,
                index,
            }),
        ])?;
        self.edit(world, tick, ops, None)
    }
}
