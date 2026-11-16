//! The `ui` subsystem's Rust model (docs/feature-engine.md "UI"): our list
//! screens and the rows we add to retail lists, as data over the settings
//! [`Store`].
//!
//! A [`Screen`] is a list of [`Row`]s; [`Screen::rows_from_settings`] takes
//! every user setting whose `Def` names the screen, in its order. An
//! [`Extension`] adds rows after a retail list's own. The [`Model`] answers
//! what the list hooks ask (`layout_glue`): how many rows a list has, which
//! row an index is, the label and value string ids to draw, and what a
//! press does. It owns the string ids of every text it can show, so the
//! locale hook serves them. It is pure: no guest memory, and a press only
//! goes through the store.

use std::ops::Range;
use std::sync::Arc;

use settings::{Apply, Def, Store, Tier};

/// The text widgets of a list row: label, then value.
pub const LABEL_SLOT: u32 = 0;
pub const VALUE_SLOT: u32 = 1;

/// A list the model has rows for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    /// The retail Settings, Controls list.
    Controls,
    /// The retail Settings list (Player Profiles, Game Variants).
    Settings,
    /// One of our screens, by id.
    Screen(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A setting's label and value; a press steps it.
    Setting(&'static str),
    /// Opens one of our screens.
    Open(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    label: Option<&'static str>,
    /// A bool setting's value as these (on, off) string ids instead of our
    /// own "On" and "Off" (the game's, so they are in the player's
    /// language).
    bool_ids: Option<[u32; 2]>,
}

impl Row {
    pub fn setting(key: &'static str) -> Row {
        Row {
            kind: RowKind::Setting(key),
            label: None,
            bool_ids: None,
        }
    }

    pub fn open(screen: &'static str) -> Row {
        Row {
            kind: RowKind::Open(screen),
            label: None,
            bool_ids: None,
        }
    }

    /// The row's label: a setting's own label, else the screen id.
    pub fn label(mut self, l: &'static str) -> Row {
        self.label = Some(l);
        self
    }

    pub fn bool_ids(mut self, on: u32, off: u32) -> Row {
        self.bool_ids = Some([on, off]);
        self
    }
}

/// One of our list screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub id: &'static str,
    pub title: &'static str,
    rows: Vec<Row>,
}

impl Screen {
    pub fn new(id: &'static str) -> Screen {
        Screen {
            id,
            title: id,
            rows: Vec::new(),
        }
    }

    pub fn title(mut self, t: &'static str) -> Screen {
        self.title = t;
        self
    }

    pub fn row(mut self, r: Row) -> Screen {
        self.rows.push(r);
        self
    }

    /// A row for every `User` setting whose menu names this screen, in
    /// menu order.
    pub fn rows_from_settings(mut self, defs: &[&'static Def]) -> Screen {
        let mut mine: Vec<_> = defs
            .iter()
            .filter(|d| d.tier == Tier::User)
            .filter_map(|d| {
                d.menu
                    .filter(|m| m.screen == self.id)
                    .map(|m| (m.order, d.key))
            })
            .collect();
        mine.sort();
        self.rows
            .extend(mine.into_iter().map(|(_, k)| Row::setting(k)));
        self
    }

    /// Without the row of setting `key`.
    pub fn without(mut self, key: &str) -> Screen {
        self.rows
            .retain(|r| !matches!(r.kind, RowKind::Setting(k) if k == key));
        self
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// Rows added after a retail list's own `retail` rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    pub list: List,
    pub retail: u32,
    rows: Vec<Row>,
}

/// Rows for retail list `list`, which has `retail` rows of its own.
pub fn extend(list: List, retail: u32) -> Extension {
    Extension {
        list,
        retail,
        rows: Vec::new(),
    }
}

impl Extension {
    pub fn row(mut self, r: Row) -> Extension {
        self.rows.push(r);
        self
    }
}

/// What a press did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// A setting stepped (or was locked); what to log.
    Stepped(String),
    /// Open this screen.
    Open(&'static str),
    /// Not a row of ours.
    Nothing,
}

/// The screens and extensions of a run, over its store.
pub struct Model {
    store: Arc<Store>,
    screens: Vec<Screen>,
    extensions: Vec<Extension>,
    strings: Vec<(u32, String)>,
}

impl Model {
    /// The model of `screens` and `extensions`. A row for a setting the
    /// store lacks, and a row opening a screen with no rows (or none at
    /// all), are left out. Every text gets a string id from `ids`; more
    /// texts than ids is refused.
    pub fn new(
        store: Arc<Store>,
        screens: Vec<Screen>,
        extensions: Vec<Extension>,
        ids: Range<u32>,
    ) -> Result<Model, String> {
        let has_setting = |r: &Row| match r.kind {
            RowKind::Setting(k) => store.def(k).is_some(),
            RowKind::Open(_) => true,
        };
        let screens: Vec<Screen> = screens
            .iter()
            .map(|s| Screen {
                rows: s.rows.iter().filter(|r| has_setting(r)).cloned().collect(),
                ..s.clone()
            })
            .collect();
        let keep = |r: &Row| match r.kind {
            RowKind::Setting(k) => store.def(k).is_some(),
            RowKind::Open(id) => screens.iter().any(|s| s.id == id && !s.rows.is_empty()),
        };
        let screens: Vec<Screen> = screens
            .iter()
            .map(|s| Screen {
                rows: s.rows.iter().filter(|r| keep(r)).cloned().collect(),
                ..s.clone()
            })
            .collect();
        let extensions: Vec<Extension> = extensions
            .iter()
            .map(|e| Extension {
                rows: e.rows.iter().filter(|r| keep(r)).cloned().collect(),
                ..e.clone()
            })
            .collect();
        let mut m = Model {
            store,
            screens,
            extensions,
            strings: Vec::new(),
        };
        let mut texts: Vec<String> = Vec::new();
        for s in &m.screens {
            texts.push(s.title.into());
        }
        let rows: Vec<Row> = m
            .screens
            .iter()
            .flat_map(|s| s.rows.iter())
            .chain(m.extensions.iter().flat_map(|e| e.rows.iter()))
            .cloned()
            .collect();
        for r in &rows {
            texts.push(m.label(r).into());
            match r.kind {
                RowKind::Open(_) => texts.push(String::new()),
                RowKind::Setting(_) if r.bool_ids.is_some() => {}
                RowKind::Setting(k) => {
                    let def = m.store.def(k).expect("kept rows have settings");
                    for v in def.values() {
                        texts.push(def.label(v));
                        if def.apply != Apply::Live {
                            texts.push(restart(&def.label(v)));
                        }
                    }
                }
            }
        }
        let mut next = ids.start;
        for t in texts {
            if m.strings.iter().any(|(_, s)| *s == t) {
                continue;
            }
            if next >= ids.end {
                return Err(format!(
                    "ui: more texts than the {} string ids there are",
                    ids.len()
                ));
            }
            m.strings.push((next, t));
            next += 1;
        }
        Ok(m)
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Every string the model can show, by id.
    pub fn strings(&self) -> &[(u32, String)] {
        &self.strings
    }

    fn id(&self, text: &str) -> u32 {
        self.strings
            .iter()
            .find(|(_, t)| t == text)
            .map_or(0, |(i, _)| *i)
    }

    fn label(&self, r: &Row) -> &'static str {
        match (r.label, &r.kind) {
            (Some(l), _) => l,
            (None, RowKind::Setting(k)) => self.store.def(k).map_or(k, |d| d.label),
            (None, RowKind::Open(id)) => self.screen(id).map_or(id, |s| s.title),
        }
    }

    pub fn screen(&self, id: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.id == id)
    }

    /// The title's string id of screen `id`.
    pub fn title_id(&self, id: &str) -> Option<u32> {
        self.screen(id).map(|s| self.id(s.title))
    }

    /// Our rows of `list`: all of a screen's, or those added to a retail
    /// list.
    pub fn rows(&self, list: List) -> &[Row] {
        match list {
            List::Screen(id) => self.screen(id).map_or(&[], |s| &s.rows),
            List::Controls | List::Settings => self
                .extensions
                .iter()
                .find(|e| e.list == list)
                .map_or(&[], |e| &e.rows),
        }
    }

    /// The row count of `list` when the game builds it with `retail` rows:
    /// a screen's own count, or a retail list's count plus ours (only when
    /// `retail` is the count the extension expects).
    pub fn count(&self, list: List, retail: u32) -> u32 {
        match list {
            List::Screen(_) => self.rows(list).len() as u32,
            List::Controls | List::Settings => {
                match self.extensions.iter().find(|e| e.list == list) {
                    Some(e) if e.retail == retail => retail + e.rows.len() as u32,
                    _ => retail,
                }
            }
        }
    }

    /// Row `index` of `list`, if it is one of ours.
    pub fn row(&self, list: List, index: i16) -> Option<&Row> {
        let base = match list {
            List::Screen(_) => 0,
            List::Controls | List::Settings => self
                .extensions
                .iter()
                .find(|e| e.list == list)
                .map_or(0, |e| e.retail as i64),
        };
        let i = usize::try_from(i64::from(index) - base).ok()?;
        self.rows(list).get(i)
    }

    /// The index of our row of `kind` in `list`.
    pub fn index_of(&self, list: List, kind: &RowKind) -> Option<i16> {
        let base = match list {
            List::Screen(_) => 0,
            List::Controls | List::Settings => self
                .extensions
                .iter()
                .find(|e| e.list == list)
                .map_or(0, |e| e.retail),
        };
        let i = self.rows(list).iter().position(|r| r.kind == *kind)?;
        i16::try_from(base as usize + i).ok()
    }

    /// `(text slot, string id)` for the row's label and value. Takes the
    /// store's lock: never call it with guest memory held.
    pub fn texts(&self, row: &Row) -> [(u32, u32); 2] {
        let label = (LABEL_SLOT, self.id(self.label(row)));
        let value = match row.kind {
            RowKind::Open(_) => self.id(""),
            RowKind::Setting(k) => match (self.store.def(k), self.store.state(k)) {
                (Some(def), Some(st)) => match (row.bool_ids, st.shown.as_bool()) {
                    (Some([on, off]), Some(b)) => {
                        if b {
                            on
                        } else {
                            off
                        }
                    }
                    _ if st.pending => self.id(&restart(&def.label(st.shown))),
                    _ => self.id(&def.label(st.shown)),
                },
                _ => self.id(""),
            },
        };
        [label, (VALUE_SLOT, value)]
    }

    /// A press on `row`: a setting steps (through the store, so its change
    /// callbacks run); an open row says which screen. Never call it with
    /// guest memory held.
    pub fn press(&self, row: &Row) -> Chosen {
        match row.kind {
            RowKind::Open(id) => Chosen::Open(id),
            RowKind::Setting(k) => match (self.store.def(k), self.store.step(k)) {
                (Some(def), Ok(o)) => {
                    let mut msg = o.describe(def);
                    if let Some(e) = &o.not_saved {
                        msg.push_str(&format!("; not saved: {e}"));
                    }
                    Chosen::Stepped(msg)
                }
                (_, Err(e)) => Chosen::Stepped(format!("{k}: {e}")),
                (None, Ok(_)) => Chosen::Nothing,
            },
        }
    }
}

fn restart(label: &str) -> String {
    format!("{label} (restart)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::{ALL, FOV, FPS, PAD_MUTE, PAD_SPEAKER, RENDER_SCALE};
    use settings::Value;

    fn model(store: Arc<Store>) -> Model {
        let video = Screen::new("video")
            .title("Video")
            .rows_from_settings(&store.defs());
        Model::new(
            store,
            vec![video],
            vec![
                extend(List::Controls, 6)
                    .row(Row::setting(PAD_SPEAKER.key).bool_ids(0xa, 0xb))
                    .row(Row::open("video")),
            ],
            0x100..0x200,
        )
        .unwrap()
    }

    fn text(m: &Model, id: u32) -> &str {
        m.strings()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, t)| t.as_str())
            .unwrap()
    }

    #[test]
    fn a_screen_from_settings_takes_its_rows_in_menu_order() {
        let m = model(Arc::new(Store::defaults(ALL)));
        let keys: Vec<_> = m
            .rows(List::Screen("video"))
            .iter()
            .map(|r| r.kind.clone())
            .collect();
        assert_eq!(
            keys,
            [
                RowKind::Setting(FOV.key),
                RowKind::Setting(FPS.key),
                RowKind::Setting(RENDER_SCALE.key),
                RowKind::Setting(PAD_MUTE.key)
            ]
        );
        assert_eq!(m.count(List::Screen("video"), 6), 4);
        assert_eq!(m.count(List::Controls, 6), 8);
        assert_eq!(m.count(List::Controls, 4), 4, "not the list we extend");
        assert_eq!(m.row(List::Controls, 5), None);
        assert_eq!(
            m.row(List::Controls, 7).unwrap().kind,
            RowKind::Open("video")
        );
        assert_eq!(m.row(List::Controls, 8), None);
        assert_eq!(m.row(List::Screen("video"), -1), None);
        assert_eq!(m.index_of(List::Controls, &RowKind::Open("video")), Some(7));
        assert_eq!(text(&m, m.title_id("video").unwrap()), "Video");
    }

    #[test]
    fn rows_without_a_setting_or_screen_are_left_out() {
        let store = Arc::new(Store::defaults(&[&PAD_SPEAKER]));
        let m = model(store);
        assert!(m.rows(List::Screen("video")).is_empty());
        assert_eq!(m.count(List::Controls, 6), 7, "the speaker row only");
    }

    #[test]
    fn texts_follow_presses_and_say_restart() {
        let m = model(Arc::new(Store::defaults(ALL)));
        let fps = m.row(List::Screen("video"), 1).unwrap().clone();
        let [(0, label), (1, value)] = m.texts(&fps) else {
            panic!()
        };
        assert_eq!((text(&m, label), text(&m, value)), ("Frame Rate", "30"));
        // A live setting: the value in effect changes at once.
        assert_eq!(m.press(&fps), Chosen::Stepped("graphics.fps: 60".into()));
        assert_eq!(text(&m, m.texts(&fps)[1].1), "60");
        assert_eq!(m.store().get(FPS.key), Some(Value::Int(60)));
        // One that waits for the next launch says so.
        let later: &'static Def = Box::leak(Box::new(Def {
            key: "graphics.later",
            apply: Apply::Restart,
            ..FPS.clone()
        }));
        let defs: Vec<&'static Def> = ALL.iter().copied().chain([later]).collect();
        let m = model(Arc::new(Store::defaults(&defs)));
        let row = m
            .rows(List::Screen("video"))
            .iter()
            .find(|r| r.kind == RowKind::Setting(later.key))
            .unwrap()
            .clone();
        assert_eq!(
            m.press(&row),
            Chosen::Stepped("graphics.later: 60 from the next launch".into())
        );
        assert_eq!(text(&m, m.texts(&row)[1].1), "60 (restart)");
        assert_eq!(
            m.store().get(later.key),
            Some(Value::Int(30)),
            "this run keeps its value"
        );
        let speaker = m.row(List::Controls, 6).unwrap().clone();
        assert_eq!(m.texts(&speaker)[1].1, 0xa, "the game's own On");
        m.press(&speaker);
        assert_eq!(m.texts(&speaker)[1].1, 0xb);
        assert_eq!(m.store().get(PAD_SPEAKER.key), Some(Value::Bool(false)));
        let open = m.row(List::Controls, 7).unwrap().clone();
        assert_eq!(m.press(&open), Chosen::Open("video"));
        assert_eq!(text(&m, m.texts(&open)[1].1), "");
        assert_eq!(text(&m, m.texts(&open)[0].1), "Video");
    }

    #[test]
    fn too_many_texts_are_refused() {
        let r = Model::new(
            Arc::new(Store::defaults(ALL)),
            vec![Screen::new("video").rows_from_settings(ALL)],
            vec![],
            0..100,
        );
        assert!(r.err().unwrap().contains("string ids"));
    }
}
