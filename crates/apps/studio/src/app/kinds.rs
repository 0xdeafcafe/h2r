//! The kind registry: for each tag group, which editor opens it, what
//! draws its preview, where its subject stands in the game, and the
//! classes its edits take (docs/editor/live-editors.md, "The registry"
//! and "The kinds").
//!
//! A group with no row gets [`DEFAULT`]: the field panel, no preview, and
//! Field and Shape routes. `tag.kinds` lists the table and every group
//! with the row it gets, so which kinds lack an editor or a preview is a
//! query. How far each group's edits reach into a running game is the
//! realtime survey's (docs/editor/notes/2026-11-23-realtime-survey.md),
//! not this table's.
use map::FourCC;
use serde_json::{Value, json};

use super::live::Class;

/// What opens a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    /// The field panel, typed by `tags::defs`, with names and units.
    FieldPanel,
    /// The Scenario tool: blocks, volumes, the gizmo.
    Scenario,
    /// The Modeling tool on the mesh kernel.
    Modeling,
    /// The Textures tool (`bitmap-edit`'s canvas).
    Textures,
    /// Sound audition: waveform, play, scrub.
    Audition,
    /// The HUD tool.
    Hud,
    /// The UI Screens tool.
    UiScreens,
    /// The Scripts tool, read only.
    Scripts,
    /// The animation editor: the Animation window's pick, pose and
    /// timeline (docs/editor/studio-redesign.md, "The animation editor").
    Animation,
}

impl Editor {
    pub fn id(self) -> &'static str {
        match self {
            Self::FieldPanel => "field_panel",
            Self::Scenario => "scenario",
            Self::Modeling => "modeling",
            Self::Textures => "textures",
            Self::Audition => "audition",
            Self::Hud => "hud",
            Self::UiScreens => "ui_screens",
            Self::Scripts => "scripts",
            Self::Animation => "animation",
        }
    }
}

/// What draws a kind's Preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    /// None: the Game panel is the only picture.
    None,
    /// The level viewport.
    Viewport,
    /// `model.render`.
    ModelRender,
    /// Bitmap thumbnails.
    Thumbnails,
    /// The sound's waveform.
    Waveform,
    /// The tool's own preview (HUD, screens).
    Tool,
    /// The check reports (game types, playlists).
    Reports,
}

impl Preview {
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Viewport => "viewport",
            Self::ModelRender => "model_render",
            Self::Thumbnails => "thumbnails",
            Self::Waveform => "waveform",
            Self::Tool => "tool",
            Self::Reports => "reports",
        }
    }
}

/// Where the subject stands in Studio's own game
/// (docs/editor/live-editors.md, "The subject").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// Not a subject of its own: the level is open, or the tag is read
    /// through another.
    None,
    /// The level itself.
    Level,
    /// A preview placement at the viewport's focus, or the stage's pad.
    Placement,
    /// A swatch object of the stage.
    Swatch,
    /// Looping at the pad.
    Loop,
    /// Played at the pad.
    Play,
}

impl Subject {
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Level => "level",
            Self::Placement => "placement",
            Self::Swatch => "swatch",
            Self::Loop => "loop",
            Self::Play => "play",
        }
    }
}

/// One kind: a row of the page's table.
#[derive(Clone, Copy, Debug)]
pub struct KindRow {
    /// The kind in words.
    pub label: &'static str,
    /// Its groups, as four-character codes (padded with spaces).
    pub groups: &'static [&'static [u8; 4]],
    pub editor: Editor,
    pub preview: Preview,
    pub subject: Subject,
    /// The classes its edits take.
    pub classes: &'static [Class],
    /// What it lacks, in the page's words.
    pub missing: &'static str,
}

const FIELD_SHAPE: &[Class] = &[Class::Field, Class::Shape];
const FIELD_RAW_SHAPE: &[Class] = &[Class::Field, Class::Raw, Class::Shape];
const FIELD_SHAPE_HOST: &[Class] = &[Class::Field, Class::Shape, Class::Host];
const ALL_CLASSES: &[Class] = &[Class::Field, Class::Raw, Class::Shape, Class::Host];

/// Any group with no row.
pub const DEFAULT: KindRow = KindRow {
    label: "Any tag",
    groups: &[],
    editor: Editor::FieldPanel,
    preview: Preview::None,
    subject: Subject::None,
    classes: FIELD_SHAPE,
    missing: "the designed panel",
};

/// One row per kind; a group is in at most one.
pub const KINDS: &[KindRow] = &[
    KindRow {
        label: "Level placements",
        groups: &[b"scnr"],
        editor: Editor::Scenario,
        preview: Preview::Viewport,
        subject: Subject::Level,
        classes: FIELD_SHAPE,
        missing: "AI, scripts and recordings as editors",
    },
    KindRow {
        label: "Level geometry, collision, portals, lightmaps",
        groups: &[b"sbsp", b"ltmp"],
        editor: Editor::Modeling,
        preview: Preview::Viewport,
        subject: Subject::Level,
        classes: FIELD_RAW_SHAPE,
        missing: "writes per target",
    },
    KindRow {
        label: "Render models, collision and physics models",
        groups: &[b"mode", b"coll", b"phmo"],
        editor: Editor::Modeling,
        preview: Preview::Viewport,
        subject: Subject::Placement,
        classes: FIELD_RAW_SHAPE,
        missing: "variants and regions, markers, posed preview, the write",
    },
    KindRow {
        label: "Objects",
        groups: &[b"hlmt", b"weap", b"vehi", b"bipd", b"scen", b"eqip", b"proj"],
        editor: Editor::FieldPanel,
        preview: Preview::ModelRender,
        subject: Subject::Placement,
        classes: FIELD_SHAPE,
        missing: "one object editor: model, variants, markers, attachments, the tuning sheet with units",
    },
    KindRow {
        label: "Bitmaps",
        groups: &[b"bitm"],
        editor: Editor::Textures,
        preview: Preview::Thumbnails,
        subject: Subject::Swatch,
        classes: FIELD_RAW_SHAPE,
        missing: "the tile encoder and mips, formats, import as a new tag, the sequence and sprite editor",
    },
    KindRow {
        label: "Shaders",
        groups: &[b"shad", b"stem", b"spas"],
        editor: Editor::FieldPanel,
        preview: Preview::None,
        subject: Subject::Swatch,
        classes: FIELD_SHAPE_HOST,
        missing: "the Shader tab and its preview, animation, code",
    },
    KindRow {
        label: "Effects, particles, contrails, lights, lens flares, decals",
        groups: &[b"effe", b"prt3", b"cont", b"ligh", b"lens", b"deca"],
        editor: Editor::FieldPanel,
        preview: Preview::None,
        subject: Subject::Loop,
        classes: FIELD_SHAPE,
        missing: "grouped editors on the curves and colours; the game view as preview",
    },
    KindRow {
        label: "Sky and fog",
        groups: &[b"sky ", b"fog "],
        editor: Editor::FieldPanel,
        preview: Preview::Viewport,
        subject: Subject::Level,
        classes: FIELD_SHAPE,
        missing: "a tag view with the dome and the fog curve",
    },
    KindRow {
        label: "Animations",
        groups: &[b"jmad"],
        editor: Editor::Animation,
        preview: Preview::None,
        subject: Subject::None,
        classes: FIELD_SHAPE,
        missing: "keys and curves, the write back (AN3 to AN6 of studio-redesign.md); events and root movement are counted, not decoded",
    },
    KindRow {
        label: "Sounds",
        groups: &[b"snd!", b"lsnd", b"ugh!"],
        editor: Editor::Audition,
        preview: Preview::Waveform,
        subject: Subject::Play,
        classes: ALL_CLASSES,
        missing: "import, distances, waveform edits, drawing the mouth curve",
    },
    KindRow {
        label: "HUD",
        groups: &[b"nhdt"],
        editor: Editor::Hud,
        preview: Preview::Tool,
        subject: Subject::None,
        classes: FIELD_SHAPE_HOST,
        missing: "what the HUD tool's \"Not modelled\" lists",
    },
    KindRow {
        label: "Screens",
        groups: &[b"wgit", b"wgtz"],
        editor: Editor::UiScreens,
        preview: Preview::Tool,
        subject: Subject::None,
        classes: FIELD_SHAPE_HOST,
        missing: "the levels' screens and dialogs",
    },
    KindRow {
        label: "Strings",
        groups: &[b"unic"],
        editor: Editor::FieldPanel,
        preview: Preview::None,
        subject: Subject::None,
        classes: FIELD_SHAPE,
        missing: "the viewer, then the writer",
    },
];

/// The row `group` gets: its own, else [`DEFAULT`].
pub fn kind(group: FourCC) -> &'static KindRow {
    KINDS
        .iter()
        .find(|k| k.groups.iter().any(|g| **g == group.0))
        .unwrap_or(&DEFAULT)
}

/// A group's four-character code as the API writes it (trailing spaces
/// dropped).
pub fn group_id(group: FourCC) -> String {
    group.to_string().trim_end().to_owned()
}

impl KindRow {
    /// Whether this is the default row.
    pub fn is_default(&self) -> bool {
        self.groups.is_empty()
    }

    /// The row as `tag.kinds` lists it.
    pub fn json(&self) -> Value {
        json!({
            "label": self.label,
            "groups": self.groups.iter().map(|g| group_id(FourCC(**g))).collect::<Vec<_>>(),
            "editor": self.editor.id(),
            "preview": self.preview.id(),
            "subject": self.subject.id(),
            "classes": self.classes.iter().map(|c| c.id()).collect::<Vec<_>>(),
            "missing": self.missing,
        })
    }
}

/// One group's line in `tag.kinds`.
pub fn group_json(group: FourCC) -> Value {
    let k = kind(group);
    json!({
        "group": group_id(group),
        "kind": k.label,
        "default": k.is_default(),
        "editor": k.editor.id(),
        "preview": k.preview.id(),
        "subject": k.subject.id(),
        "classes": k.classes.iter().map(|c| c.id()).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tags::registry::Registry;

    /// Every group the registry knows has a row or the default, no group
    /// is in two rows, and every row's group is one the registry knows.
    #[test]
    fn every_group_has_a_row_or_the_default() {
        let registry = Registry::builtin();
        let known: BTreeSet<[u8; 4]> = registry.groups.keys().map(|g| g.0).collect();
        let mut seen = BTreeSet::new();
        for row in KINDS {
            assert!(!row.groups.is_empty(), "{}: a row names its groups", row.label);
            assert!(!row.classes.is_empty(), "{}: a row has classes", row.label);
            for g in row.groups {
                assert!(known.contains(*g), "{}: {:?} is not a registry group", row.label, g);
                assert!(seen.insert(**g), "{:?} is in two rows", g);
            }
        }
        let mut defaulted = 0;
        for g in &known {
            let k = kind(FourCC(*g));
            if k.is_default() {
                defaulted += 1;
                assert_eq!(k.classes, &[Class::Field, Class::Shape]);
                assert_eq!((k.editor, k.preview), (Editor::FieldPanel, Preview::None));
            } else {
                assert!(k.groups.iter().any(|x| **x == *g));
            }
        }
        assert_eq!(defaulted + seen.len(), known.len());
        // A group outside the registry gets the default too.
        assert!(kind(FourCC(*b"zzzz")).is_default());
    }

    #[test]
    fn a_groups_line_names_its_editor_preview_and_classes() {
        let weap = group_json(FourCC(*b"weap"));
        assert_eq!(weap["kind"], "Objects");
        assert_eq!((weap["editor"].clone(), weap["preview"].clone()), (json!("field_panel"), json!("model_render")));
        assert_eq!(weap["classes"], json!(["field", "shape"]));
        let sky = group_json(FourCC(*b"sky "));
        assert_eq!(sky["group"], "sky");
        assert_eq!(sky["default"], false);
        let matg = group_json(FourCC(*b"matg"));
        assert_eq!((matg["kind"].clone(), matg["default"].clone()), (json!("Any tag"), json!(true)));
    }
}
