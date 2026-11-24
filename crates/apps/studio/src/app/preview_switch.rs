//! The preview switch (docs/editor/studio-redesign.md, "Preview and the
//! paired game"): where each document's preview shows (**Studio**,
//! **My game** or **Attached**), and **Preview in game**, which runs
//! `pair.preview` for the front document's kind.
//!
//! The window's button ([`Action::PreviewInGame`]) and `preview.target`
//! share [`App::game_preview`], the one place that maps a document to the
//! `pair.preview` params its kind takes. The button leaves an ask that
//! [`crate::api::Api::run_preview_asks`] runs between frames, on the same
//! preview provider `pair.preview` uses, so the job is `pair.result`'s.
//!
//! [`Action::PreviewInGame`]: super::Action::PreviewInGame

use std::collections::HashMap;

use map::FourCC;
use serde_json::{Value, json};

use super::kinds;
use super::{App, Doc, Pick};

/// Where a document's preview shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PreviewTarget {
    /// The native view, or the kind's own renderer.
    #[default]
    Studio,
    /// A game Studio started.
    MyGame,
    /// A game someone else started.
    Attached,
}

impl PreviewTarget {
    pub const ALL: [PreviewTarget; 3] = [Self::Studio, Self::MyGame, Self::Attached];

    pub fn id(self) -> &'static str {
        match self {
            Self::Studio => "studio",
            Self::MyGame => "game",
            Self::Attached => "attached",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::MyGame => "My game",
            Self::Attached => "Attached",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == s)
    }
}

/// What `pair.preview` is asked for a document: its `what` and `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamePreview {
    pub what: &'static str,
    pub target: Option<String>,
}

impl GamePreview {
    /// The params of the `pair.preview` call.
    pub fn params(&self) -> Value {
        let mut p = json!({"what": self.what});
        if let Some(t) = &self.target {
            p["target"] = json!(t);
        }
        p
    }
}

/// The preview switch's state: each document's target, and the ask the
/// window's button left.
#[derive(Debug, Default)]
pub struct PreviewSwitch {
    targets: HashMap<Doc, PreviewTarget>,
    /// A **Preview in game** click waiting for the API to run it.
    pub ask: Option<Doc>,
    /// The id of the last job a click started.
    pub last_job: Option<u64>,
}

/// The `pair.preview` a tag of `group` and `name` takes, or why its kind
/// has none.
pub fn plan_for_tag(group: FourCC, name: &str) -> Result<GamePreview, String> {
    let id = kinds::group_id(group);
    Ok(match &group.0 {
        b"weap" => GamePreview {
            what: "weapon",
            target: Some(name.to_owned()),
        },
        b"vehi" | b"bipd" | b"scen" | b"eqip" => GamePreview {
            what: "object",
            target: Some(format!("{id}:{name}")),
        },
        b"wgit" | b"wgtz" => GamePreview {
            what: "ui",
            target: Some(super::tag_tree::leaf(name).to_owned()),
        },
        b"nhdt" => GamePreview {
            what: "hud",
            target: None,
        },
        b"snd!" => GamePreview {
            what: "sound",
            target: Some(name.to_owned()),
        },
        b"lsnd" | b"ugh!" => {
            return Err("audio.play starts snd! tags: preview a snd! of it".into());
        }
        b"jmad" => {
            return Err(
                "an animation plays on an object: pair.preview {what: animation, target: the object's tag, animation: its name}"
                    .into(),
            );
        }
        b"proj" | b"hlmt" | b"mode" | b"coll" | b"phmo" => {
            return Err(format!(
                "a {id} has no preview of its own in the game: preview the weapon, vehicle or scenery that uses it"
            ));
        }
        _ => {
            return Err(format!(
                "{} tags have no preview in the game",
                kinds::kind(group).label
            ));
        }
    })
}

impl App {
    /// The target a document's preview shows at.
    pub fn preview_target(&self, doc: Doc) -> PreviewTarget {
        self.preview.targets.get(&doc).copied().unwrap_or_default()
    }

    /// A game is paired.
    fn game_paired(&self) -> bool {
        self.pairing
            .published
            .as_ref()
            .is_some_and(|s| s["state"] == "connected")
    }

    /// The paired game is one Studio started.
    fn game_is_own(&self) -> bool {
        self.pairing.link.as_ref().is_some_and(|l| l.can_stop)
    }

    /// Whether `target` can be chosen now, or why not.
    pub fn preview_target_available(&self, target: PreviewTarget) -> Result<(), String> {
        match target {
            PreviewTarget::Studio => Ok(()),
            _ if !self.game_paired() => Err("no game is paired (pair.list, pair.connect)".into()),
            PreviewTarget::MyGame if !self.game_is_own() => {
                Err("the paired game is not one Studio started: choose Attached".into())
            }
            PreviewTarget::Attached if self.game_is_own() => {
                Err("the paired game is Studio's own: choose My game".into())
            }
            _ => Ok(()),
        }
    }

    /// Switch a document's preview to `target`.
    pub fn set_preview_target(&mut self, doc: Doc, target: PreviewTarget) -> Result<(), String> {
        self.preview_target_available(target)?;
        self.preview.targets.insert(doc, target);
        Ok(())
    }

    /// The `pair.preview` a document's kind takes.
    pub fn game_preview(&self, doc: Doc) -> Result<GamePreview, String> {
        match doc {
            Doc::Tag(id) => {
                let t = self.tags.tag(id).ok_or("the tag is not in the open map")?;
                plan_for_tag(t.group, &t.name)
            }
            Doc::Hud => Ok(GamePreview {
                what: "hud",
                target: None,
            }),
            Doc::Ui => Ok(GamePreview {
                what: "ui",
                target: match &self.ui_screens.pick {
                    Some(Pick::Native { screen, .. }) => Some(screen.clone()),
                    _ => None,
                },
            }),
            Doc::Map => Err("a map previews in the Scene tab; Playtest runs it in the game".into()),
            Doc::Rules => Err(
                "a script has no preview in the game: Reload into game sends it".into(),
            ),
            Doc::Variant | Doc::Playlists => Err(
                "a variant or a playlist file has no preview in the game: Playtest runs it".into(),
            ),
        }
    }

    /// [`Action::PreviewInGame`]: leave an ask for the front document,
    /// or say why its kind has none.
    ///
    /// [`Action::PreviewInGame`]: super::Action::PreviewInGame
    pub(super) fn preview_in_game(&mut self, doc: Option<Doc>) {
        self.sync_tabs();
        let Some(doc) = doc.or_else(|| self.front_doc()) else {
            return self.error("no document is open to preview");
        };
        match self.game_preview(doc) {
            Ok(_) => self.preview.ask = Some(doc),
            Err(why) => self.error(why),
        }
    }
}
