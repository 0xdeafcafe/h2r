//! The subject: the tag being edited, and where it stands in Studio's own
//! game (docs/editor/live-editors.md, "The subject").
//!
//! This module decides and carries no game state: [`place`] gives the
//! subject's place for each rule of the kind registry
//! ([`super::kinds::Subject`]) in each [`Scene`]; [`spawn_tag`] names the
//! object tag the game makes for a subject that is not an object itself
//! (a model, a model's collision); [`App::subject`] is the tag in front.
//! `play::subject` carries the places out over the game link and queues
//! the follow-ups after a delivery.
use agent_proto::Kind;
use map::TagId;
use serde_json::{Value, json};

use super::kinds::{self, Subject};
use super::shell::Doc;
use super::{App, TagDoc, ToolKind};

/// The content stage's map package (docs/editor/content-stage.md).
pub const STAGE_MAP: &str = "stage";
/// The bay a subject stands in: the flat-lit one.
pub const STAGE_BAY: &str = "flat";

/// What the subject can stand in.
#[derive(Clone, Debug, PartialEq)]
pub enum Scene {
    /// A level is open in Studio's own game: the viewport camera's focus
    /// (on the ground) is where an object-like subject is placed.
    Level { focus: [f32; 3] },
    /// The content stage is the own game's map: the bay's subject pad,
    /// and the way a thing on it faces the player (degrees from +x).
    Stage { bay: String, pad: [f32; 3], yaw: f32 },
    /// No game of Studio's own on a level or the stage.
    Nowhere,
}

impl Scene {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Level { .. } => "level",
            Self::Stage { .. } => "stage",
            Self::Nowhere => "nowhere",
        }
    }

    /// The flat bay's pad of the shipped stage layout.
    pub fn stage() -> Result<Self, String> {
        let at = content_kit::stage::subject_place(STAGE_BAY).map_err(|e| e.to_string())?;
        Ok(Self::Stage {
            bay: at.bay,
            pad: at.point,
            yaw: at.facing.to_degrees(),
        })
    }
}

/// What shows the subject in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// The tag's object, made by `sim.spawn`.
    Object,
    /// A swatch object of the stage (a shader or a bitmap).
    Swatch,
    /// An effect or light looping at the pad.
    Loop,
    /// A sound played, `audio.play`.
    Play,
    /// Nothing of its own: the level, or a tag read through another.
    Nothing,
}

impl Show {
    pub fn id(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Swatch => "swatch",
            Self::Loop => "loop",
            Self::Play => "play",
            Self::Nothing => "nothing",
        }
    }
}

/// Why no swatch stands for a shader or a bitmap yet.
pub const NO_SWATCH: &str =
    "the stage has no swatch object yet: a shader or bitmap is not shown in the game";
/// Why no loop stands for an effect or a light yet.
pub const NO_LOOP: &str =
    "the game has no method to start a looping effect or light: the loop at the pad waits for its method";

/// Where, and how, the subject stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub rule: Subject,
    pub show: Show,
    pub scene: &'static str,
    /// The point it stands at, when it stands.
    pub at: Option<[f32; 3]>,
    /// Which way it faces, degrees from +x, when the scene says.
    pub yaw: Option<f32>,
    /// What is missing or what is special, in words.
    pub note: Option<String>,
}

impl Place {
    pub fn json(&self) -> Value {
        json!({
            "rule": self.rule.id(),
            "show": self.show.id(),
            "scene": self.scene,
            "at": self.at,
            "yaw": self.yaw,
            "note": self.note,
        })
    }
}

/// The subject's place under `rule` in `scene`.
pub fn place(rule: Subject, scene: &Scene) -> Place {
    let show = match rule {
        Subject::Placement => Show::Object,
        Subject::Swatch => Show::Swatch,
        Subject::Loop => Show::Loop,
        Subject::Play => Show::Play,
        Subject::Level | Subject::None => Show::Nothing,
    };
    let mut place = Place {
        rule,
        show,
        scene: scene.id(),
        at: None,
        yaw: None,
        note: None,
    };
    let note = |s: &str| Some(s.to_owned());
    match (show, scene) {
        (Show::Nothing, _) => {
            place.note = note(match rule {
                Subject::Level => "the level is the subject: the game already shows it",
                _ => "the game shows nothing of its own for this tag",
            });
        }
        (_, Scene::Nowhere) => {
            place.note = note("no game of Studio's own is on a level or the stage");
        }
        (Show::Object, Scene::Level { focus }) => place.at = Some(*focus),
        (Show::Swatch | Show::Loop, Scene::Level { .. }) => {
            place.note = note("it stands on the stage; no level shows it");
        }
        // A sound is started at the player's unit, whichever map it is.
        (Show::Play, Scene::Level { .. }) => {
            place.note = note("audio.play starts it at the player's unit");
        }
        (show, Scene::Stage { pad, yaw, .. }) => {
            place.at = Some(*pad);
            place.yaw = Some(*yaw);
            match show {
                Show::Swatch => place.note = note(NO_SWATCH),
                Show::Loop => place.note = note(NO_LOOP),
                Show::Play => {
                    place.note = note("audio.play starts it at the player's unit, 3 units from the pad");
                }
                _ => {}
            }
        }
    }
    place
}

/// The groups `sim.spawn` makes objects of that the game lists, with the
/// kind they are listed as.
fn object_kind(group: &str) -> Option<Kind> {
    Some(match group {
        "weap" => Kind::Weapon,
        "eqip" => Kind::Equipment,
        "scen" => Kind::Scenery,
        "vehi" => Kind::Vehicle,
        "mach" => Kind::Machine,
        "ctrl" => Kind::Control,
        "bloc" => Kind::Crate,
        "crea" => Kind::Creature,
        _ => return None,
    })
}

/// The groups whose objects stand for a model: an object tag that uses
/// it is spawned, the first of these groups by tag.
const BY_MODEL: [&str; 4] = ["scen", "weap", "vehi", "eqip"];

/// The object tag the game makes for `subject`, and the kind its object
/// is listed as: the tag itself when it is an object tag the game lists,
/// else, for a model or a model's collision and physics, an object tag
/// that uses it. The reason is a sentence when none does.
pub fn spawn_tag(doc: &TagDoc, subject: TagId) -> Result<(TagId, Kind), String> {
    let group = |id: TagId| doc.tag(id).map(|t| kinds::group_id(t.group)).unwrap_or_default();
    let own = group(subject);
    if let Some(kind) = object_kind(&own) {
        return Ok((subject, kind));
    }
    if !["hlmt", "mode", "coll", "phmo"].contains(&own.as_str()) {
        return Err(format!("{own}: the game makes no object of this tag"));
    }
    let uses = super::tag_api::uses(doc, None, subject);
    let mut found: Vec<(usize, TagId, Kind)> = uses
        .tags
        .iter()
        .filter_map(|u| {
            let g = group(u.tag);
            let rank = BY_MODEL.iter().position(|b| *b == g)?;
            Some((rank, u.tag, object_kind(&g)?))
        })
        .collect();
    found.sort_by_key(|(rank, tag, _)| (*rank, tag.0));
    found
        .first()
        .map(|&(_, tag, kind)| (tag, kind))
        .ok_or_else(|| format!("{own}: no scenery, weapon, vehicle or equipment tag uses it"))
}

impl App {
    /// The subject: the tag in front, else the one selected in the Tags
    /// tool (where the front document is a tool's, not a tag's).
    pub fn subject(&self) -> Option<TagId> {
        if let Some(Doc::Tag(id)) = self.front_doc() {
            return Some(id);
        }
        (self.tool == ToolKind::Tags).then_some(self.tags.selected).flatten()
    }

    /// The registry's subject rule for `tag`.
    pub fn subject_rule(&self, tag: TagId) -> Subject {
        self.tags
            .tag(tag)
            .map_or(Subject::None, |t| kinds::kind(t.group).subject)
    }

    /// The place the subject `tag` has in the scene Studio's play last
    /// published (`pairing.scene`).
    pub fn place_of(&self, tag: TagId) -> Place {
        place(self.subject_rule(tag), &self.pairing.scene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level() -> Scene {
        Scene::Level { focus: [1.0, 2.0, 3.0] }
    }

    fn stage() -> Scene {
        Scene::Stage { bay: "flat".into(), pad: [3.0, 0.0, 0.0], yaw: 180.0 }
    }

    /// An object-like subject stands at the focus in a level and on the
    /// pad, facing the player, on the stage; with no game it stands
    /// nowhere and says why.
    #[test]
    fn an_object_stands_at_the_focus_or_on_the_pad() {
        let p = place(Subject::Placement, &level());
        assert_eq!((p.show, p.at, p.yaw), (Show::Object, Some([1.0, 2.0, 3.0]), None));
        let p = place(Subject::Placement, &stage());
        assert_eq!((p.show, p.at, p.yaw), (Show::Object, Some([3.0, 0.0, 0.0]), Some(180.0)));
        assert!(p.note.is_none());
        let p = place(Subject::Placement, &Scene::Nowhere);
        assert_eq!((p.at, p.scene), (None, "nowhere"));
        assert!(p.note.is_some());
    }

    /// A swatch, a loop and a played sound have the stage's pad and no
    /// place in a level; the swatch and the loop say what is missing.
    #[test]
    fn a_swatch_a_loop_and_a_sound_have_the_pad() {
        for rule in [Subject::Swatch, Subject::Loop, Subject::Play] {
            let on = place(rule, &stage());
            assert_eq!(on.at, Some([3.0, 0.0, 0.0]), "{rule:?}");
            assert_eq!(on.scene, "stage");
            assert!(place(rule, &Scene::Nowhere).at.is_none());
        }
        assert_eq!(place(Subject::Swatch, &stage()).note.as_deref(), Some(NO_SWATCH));
        assert_eq!(place(Subject::Loop, &stage()).note.as_deref(), Some(NO_LOOP));
        assert!(place(Subject::Swatch, &level()).at.is_none());
        assert!(place(Subject::Loop, &level()).at.is_none());
        assert!(place(Subject::Play, &level()).at.is_none());
        assert_eq!(place(Subject::Play, &stage()).show, Show::Play);
    }

    /// The level and the tags read through another show nothing of
    /// their own, in any scene.
    #[test]
    fn the_level_and_read_through_tags_show_nothing() {
        for rule in [Subject::Level, Subject::None] {
            for scene in [level(), stage(), Scene::Nowhere] {
                let p = place(rule, &scene);
                assert_eq!((p.show, p.at), (Show::Nothing, None));
                assert!(p.note.is_some());
            }
        }
    }

    /// The stage's pad is the shipped layout's subject pad, 3 units ahead
    /// of the spawn, facing back at it.
    #[test]
    fn the_stage_scene_is_the_layouts_subject_pad() {
        let Scene::Stage { bay, pad, yaw } = Scene::stage().unwrap() else {
            panic!("the stage scene")
        };
        assert_eq!(bay, STAGE_BAY);
        assert_eq!(pad[..2], [3.0, 0.0]);
        assert!((yaw - 180.0).abs() < 1e-3);
    }

    /// Every kind row's rule gives a place in each scene (no rule is
    /// left out of `place`).
    #[test]
    fn every_registry_rule_has_a_place() {
        for row in kinds::KINDS {
            for scene in [level(), stage(), Scene::Nowhere] {
                let p = place(row.subject, &scene);
                assert_eq!(p.rule, row.subject);
            }
        }
    }

    #[test]
    fn only_listed_object_groups_have_a_kind() {
        assert_eq!(object_kind("weap"), Some(Kind::Weapon));
        assert_eq!(object_kind("scen"), Some(Kind::Scenery));
        assert_eq!(object_kind("bitm"), None);
        assert_eq!(object_kind("proj"), None);
    }
}
