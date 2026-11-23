//! The fan-out: one write to the document, three deliveries
//! (docs/editor/live-editors.md, "One write, three deliveries").
//!
//! Every write path hands a [`Change`] to [`Fanout::emit`]: the tag
//! document's writes, block changes, resizes, string adds, undo and redo
//! ([`crate::app::TagDoc::take_changes`]), the scenario session and the
//! raw documents (the Modeling tool's geometry, the Textures tool's
//! pixels), seen by their revisions in `play::tag_sync`, and the host
//! sources (HUD kit records, our screens' YAML, a rules script, shader
//! overrides) by [`Change::host`]. A change's [`Class`] picks its route
//! ([`route`]) to each of the three views:
//!
//! - the **Preview**, Studio's own picture, re-reads the document;
//! - Studio's **own game** gets the class's delivery with no click;
//! - an **attached** game gets the same delivery while the pairing is in
//!   Edit, and a row in Pending otherwise. A Shape change in an attached
//!   game is always a Pending row: its map changes only as
//!   docs/editor/live-mirror.md allows.
//!
//! This module decides; `play::tag_sync` (`Play::fan_out`) carries the
//! deliveries out over the game link.
use map::TagId;
use serde_json::{Value, json};

/// What a change touched, which decides its route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// Fixed bytes of a tag the game holds: `edit.batch` at the next tick.
    Field,
    /// Pixels, vertices, indices, sound samples: the raw edits
    /// (docs/editor/realtime-edits.md), at the next frame.
    Raw,
    /// A block grown or shrunk, data resized, a tag or a string added: a
    /// build of the project and a load of it.
    Shape,
    /// Content the game draws from host data: the host's own reload.
    Host,
}

impl Class {
    pub const ALL: [Self; 4] = [Self::Field, Self::Raw, Self::Shape, Self::Host];

    /// The API's id (`mirror.pending` rows' `class`, `tag.kinds`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::Raw => "raw",
            Self::Shape => "shape",
            Self::Host => "host",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The class of a Pending row by its `doc`, before a write path says
    /// otherwise: geometry, pixels and collision are Raw, host sources are
    /// Host, and the tag, scenario and object rows are Field (their
    /// layout rows are marked Shape where they are made).
    pub fn of_doc(doc: &str) -> Self {
        match doc {
            "geometry" | "bitmaps" | "collision" => Self::Raw,
            "host" => Self::Host,
            _ => Self::Field,
        }
    }

    /// A Pending row's class: its `class`, else its `doc`'s.
    pub fn of_row(row: &Value) -> Self {
        row["class"]
            .as_str()
            .and_then(Self::from_id)
            .unwrap_or_else(|| Self::of_doc(row["doc"].as_str().unwrap_or_default()))
    }
}

/// Which host source a Host change came from, and so which reload the
/// game runs for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    /// A game type script: `rules.reload`.
    Rules,
    /// A shader override: `render.reload`.
    Shader,
    /// Our screens' YAML.
    Ui,
    /// HUD kit records.
    HudKit,
    /// A host sound.
    Sound,
}

impl Host {
    pub fn id(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Shader => "shader",
            Self::Ui => "ui",
            Self::HudKit => "hud_kit",
            Self::Sound => "sound",
        }
    }

    /// The game's method that reloads this source, when it has one. The
    /// UI, HUD kit and host sound reloads are not in the game's agent API
    /// yet: those changes wait in Pending with [`Host::NO_RELOAD`].
    pub fn method(self) -> Option<&'static str> {
        match self {
            Self::Rules => Some(agent_proto::rules::method::RELOAD),
            Self::Shader => Some(agent_proto::hot::method::RENDER_RELOAD),
            Self::Ui | Self::HudKit | Self::Sound => None,
        }
    }

    pub const NO_RELOAD: &str = "the game has no reload for this host source; reload the map";
}

/// One write, as every write path reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    /// The tag written, when the write is to one.
    pub tag: Option<TagId>,
    /// The `(address, length)` runs written; empty when the write path
    /// reports a document revision rather than bytes.
    pub ranges: Vec<(u32, u32)>,
    pub class: Class,
    /// For a Host change, its source.
    pub host: Option<Host>,
    /// For a Host change, the reload's params (`rules.reload`'s script).
    pub params: Value,
    /// What was done, in words (the undo step's label, the document).
    pub label: String,
}

impl Change {
    pub fn new(
        class: Class,
        tag: Option<TagId>,
        ranges: Vec<(u32, u32)>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            tag,
            ranges,
            class,
            host: None,
            params: Value::Null,
            label: label.into(),
        }
    }

    /// A Host change from `host`, reloaded with `params`.
    pub fn host(host: Host, params: Value, label: impl Into<String>) -> Self {
        Self {
            host: Some(host),
            params,
            ..Self::new(Class::Host, None, Vec::new(), label)
        }
    }
}

/// Where the game view's changes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Target {
    /// No game is paired: the Preview alone.
    #[default]
    Off,
    /// Studio's own game.
    Own,
    /// A paired game someone else runs; `edit` is the pairing's Edit.
    Attached { edit: bool },
}

/// The three places a change is seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Preview,
    Game,
    Attached,
}

/// How a change reaches one view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// The view reads the document again.
    Reread,
    /// `edit.batch`, at the next tick.
    EditBatch,
    /// The raw edits, at the next frame.
    RawEdit,
    /// A build and a load of it.
    BuildLoad,
    /// The host source's own reload.
    HostReload(Host),
    /// A row in Pending, sent when the user sends.
    Pending,
}

impl Route {
    pub fn id(self) -> &'static str {
        match self {
            Self::Reread => "reread",
            Self::EditBatch => "edit_batch",
            Self::RawEdit => "raw_edit",
            Self::BuildLoad => "build_load",
            Self::HostReload(_) => "host_reload",
            Self::Pending => "pending",
        }
    }
}

/// The game's route for a class.
fn game_route(change: &Change) -> Route {
    match change.class {
        Class::Field => Route::EditBatch,
        Class::Raw => Route::RawEdit,
        Class::Shape => Route::BuildLoad,
        Class::Host => match change.host {
            Some(host) => Route::HostReload(host),
            None => Route::Pending,
        },
    }
}

/// Each view's route for `change` when the game view is `target`: the
/// Preview re-reads, and at most one game delivery or Pending row.
pub fn route(change: &Change, target: Target) -> Vec<(View, Route)> {
    let mut out = vec![(View::Preview, Route::Reread)];
    match target {
        Target::Off => {}
        Target::Own => out.push((View::Game, game_route(change))),
        Target::Attached { edit } => out.push((
            View::Attached,
            if edit && change.class != Class::Shape {
                game_route(change)
            } else {
                Route::Pending
            },
        )),
    }
    out
}

/// One delivery the fan-out decided.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub view: View,
    pub route: Route,
    pub change: Change,
}

/// Changes kept before a frame takes them: a write path that is never
/// drained (no window, no pairing) keeps no more than this many.
pub const KEEP: usize = 1024;

/// The fan-out: changes in, deliveries out, once a frame.
#[derive(Debug, Default)]
pub struct Fanout {
    target: Target,
    inbox: Vec<Change>,
    /// Host changes waiting in Pending for a game that can take them,
    /// one per source and label.
    held: Vec<Change>,
    /// Bumped on every change: what a Preview keys its picture on.
    preview_rev: u64,
    /// The revisions of the documents seen by revision (`play::tag_sync`).
    pub(crate) seen: Seen,
    pub(crate) owed: Owed,
    /// The last [`KEEP`] deliveries, by view, route and class.
    delivered: Vec<(View, Route, Class)>,
}

/// Document revisions the fan-out compares frame to frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) raw: Option<u64>,
    /// World, markers and the blocks' layouts.
    pub(crate) scenario: Option<(u64, u64, u64)>,
}

/// Deliveries to the own game that wait for the link to be free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Owed {
    /// A send of what the documents hold.
    pub(crate) send: bool,
    /// A play-copy reload for what the send leaves under After reload.
    pub(crate) build: bool,
}

impl Fanout {
    pub fn target(&self) -> Target {
        self.target
    }

    pub fn set_target(&mut self, target: Target) {
        self.target = target;
    }

    /// Takes one write. Consecutive changes of one tag, class and label
    /// (every frame of a drag) are one change.
    pub fn emit(&mut self, change: Change) {
        self.preview_rev += 1;
        if let Some(last) = self.inbox.last_mut()
            && last.tag == change.tag
            && last.class == change.class
            && last.host == change.host
            && last.label == change.label
        {
            for r in change.ranges {
                if !last.ranges.contains(&r) {
                    last.ranges.push(r);
                }
            }
            last.params = change.params;
            return;
        }
        if self.inbox.len() >= KEEP {
            self.inbox.remove(0);
        }
        self.inbox.push(change);
    }

    /// The Preview's revision: bumped by every change.
    pub fn preview_rev(&self) -> u64 {
        self.preview_rev
    }

    /// Whether changes wait for [`Self::flush`].
    pub fn is_empty(&self) -> bool {
        self.inbox.is_empty()
    }

    /// The deliveries for every change taken since the last flush, to
    /// the target set now, with the held Host changes delivered once the
    /// target takes them.
    pub fn flush(&mut self) -> Vec<Delivery> {
        let changes = std::mem::take(&mut self.inbox);
        let mut out = Vec::new();
        // A held change goes once the target routes Host changes to the
        // game and its source has a reload there.
        let (ready, kept): (Vec<Change>, Vec<Change>) =
            std::mem::take(&mut self.held).into_iter().partition(|c| {
                c.host.and_then(Host::method).is_some()
                    && matches!(route(c, self.target).last(), Some((_, Route::HostReload(_))))
            });
        self.held = kept;
        for change in ready {
            let (view, to) = *route(&change, self.target).last().expect("a game route");
            out.push(Delivery { view, route: to, change });
        }
        for change in changes {
            for (view, to) in route(&change, self.target) {
                if view != View::Preview && to == Route::Pending && change.class == Class::Host {
                    self.hold(change.clone());
                }
                out.push(Delivery {
                    view,
                    route: to,
                    change: change.clone(),
                });
            }
        }
        self.delivered
            .extend(out.iter().map(|d| (d.view, d.route, d.change.class)));
        if self.delivered.len() > KEEP {
            self.delivered.drain(..self.delivered.len() - KEEP);
        }
        out
    }

    fn hold(&mut self, change: Change) {
        self.held
            .retain(|h| !(h.host == change.host && h.label == change.label));
        self.held.push(change);
    }

    /// Holds a Host change the game could not take (no reload method),
    /// for its Pending row.
    pub(crate) fn hold_refused(&mut self, change: Change) {
        self.hold(change);
    }

    /// The held Host changes, as Pending rows.
    pub fn held_rows(&self, why: impl Fn(&Change) -> &'static str) -> Vec<Value> {
        self.held
            .iter()
            .map(|c| {
                let mut row = crate::play::tag_sync::row(
                    "host",
                    c.tag.map_or(Value::Null, |t| json!(format!("{:#010x}", t.0))),
                    &c.label,
                    crate::play::tag_sync::Effect::Now,
                    why(c),
                );
                row["host"] = json!(c.host.map(Host::id));
                row
            })
            .collect()
    }

    /// The last [`KEEP`] deliveries: view, route and class.
    pub fn delivered(&self) -> &[(View, Route, Class)] {
        &self.delivered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn each_class() -> Vec<Change> {
        vec![
            Change::new(Class::Field, Some(TagId(1)), vec![(0x10, 4)], "set speed"),
            Change::new(Class::Raw, Some(TagId(2)), vec![(0, 64)], "paint"),
            Change::new(Class::Shape, Some(TagId(1)), vec![(0x20, 8)], "add part"),
            Change::host(Host::Shader, json!({}), "shader override"),
        ]
    }

    /// Each class makes one Preview re-read and one game delivery in the
    /// own game, its own route.
    #[test]
    fn one_write_of_each_class_makes_its_one_delivery() {
        let want = [
            Route::EditBatch,
            Route::RawEdit,
            Route::BuildLoad,
            Route::HostReload(Host::Shader),
        ];
        for (change, want) in each_class().into_iter().zip(want) {
            let mut f = Fanout::default();
            f.set_target(Target::Own);
            f.emit(change.clone());
            let out = f.flush();
            assert_eq!(out.len(), 2, "{change:?}");
            assert_eq!((out[0].view, out[0].route), (View::Preview, Route::Reread));
            assert_eq!((out[1].view, out[1].route), (View::Game, want));
            assert_eq!(out[1].change, change);
            assert!(f.flush().is_empty());
        }
    }

    /// Where the own game gets the delivery, an attached game not in Edit
    /// gets a Pending row; in Edit it gets the delivery, but for Shape.
    #[test]
    fn an_attached_game_gets_a_pending_row_where_the_own_game_gets_the_delivery() {
        for change in each_class() {
            let own = route(&change, Target::Own);
            let viewing = route(&change, Target::Attached { edit: false });
            assert_eq!(own.len(), 2);
            assert_eq!(viewing, vec![(View::Preview, Route::Reread), (View::Attached, Route::Pending)]);
            let editing = route(&change, Target::Attached { edit: true });
            let want = if change.class == Class::Shape { Route::Pending } else { own[1].1 };
            assert_eq!(editing[1], (View::Attached, want));
            assert_eq!(route(&change, Target::Off), vec![(View::Preview, Route::Reread)]);
        }
    }

    /// A Host change an attached game in View cannot take waits as one
    /// Pending row per source and label, and goes once the target takes it.
    #[test]
    fn a_held_host_change_goes_when_the_target_takes_it() {
        let mut f = Fanout::default();
        f.set_target(Target::Attached { edit: false });
        f.emit(Change::host(Host::Rules, json!({"source": "a"}), "Mine"));
        f.flush();
        f.emit(Change::host(Host::Rules, json!({"source": "b"}), "Mine"));
        f.flush();
        let rows = f.held_rows(|_| "why");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0]["doc"].clone(), rows[0]["class"].clone()), (json!("host"), json!("host")));
        assert_eq!(rows[0]["host"], "rules");
        f.set_target(Target::Own);
        let out = f.flush();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].route, Route::HostReload(Host::Rules));
        assert_eq!(out[0].change.params, json!({"source": "b"}));
        assert!(f.held_rows(|_| "why").is_empty());
    }

    /// Every frame of a drag is one change; the Preview's revision moves
    /// with each.
    #[test]
    fn a_drag_is_one_change() {
        let mut f = Fanout::default();
        f.set_target(Target::Own);
        for _ in 0..5 {
            f.emit(Change::new(Class::Field, Some(TagId(1)), vec![(0x10, 4)], "set speed"));
        }
        assert_eq!(f.preview_rev(), 5);
        assert_eq!(f.flush().len(), 2);
    }

    /// The tag document reports each write in its class: a field set is
    /// Field over the field's bytes, a block element removed is Shape, and
    /// undo and redo report the step's class again.
    #[test]
    fn the_tag_documents_writes_report_their_class() {
        use crate::app::{Action, BlockOp, TagAction, tag_tests};
        let t = crate::play::testing::Temp::new("live-tag-classes");
        let (mut app, _, _) = tag_tests::app(&t.0);
        app.tags.take_changes();
        let weap = tag_tests::weap_id();
        let set = crate::app::tag_api::set_field(&app.tags, weap, "speed", &json!(4.5)).unwrap();
        app.dispatch(Action::Tag(set));
        let c = app.tags.take_changes();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].class, c[0].tag), (Class::Field, Some(weap)));
        assert_eq!(c[0].ranges, vec![(tag_tests::WEAP, 4)]);

        let field = app.tags.layout(weap).unwrap().field_at(8).unwrap().clone();
        let tags::registry::Kind::Block(layout) = &field.kind else {
            panic!("the weapon's parts are a block")
        };
        app.dispatch(Action::Tag(TagAction::Block {
            tag: weap,
            pair: tag_tests::WEAP + 8,
            layout: layout.clone(),
            op: BlockOp::Remove(1),
        }));
        let c = app.tags.take_changes();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].class, c[0].tag), (Class::Shape, Some(weap)));
        assert!(c[0].ranges.contains(&(tag_tests::WEAP + 8, 4)));

        app.tags.undo().unwrap();
        app.tags.undo().unwrap();
        let c = app.tags.take_changes();
        assert_eq!(
            c.iter().map(|c| (c.class, c.tag)).collect::<Vec<_>>(),
            vec![(Class::Shape, Some(weap)), (Class::Field, Some(weap))]
        );
        app.tags.redo().unwrap();
        assert_eq!(app.tags.take_changes()[0].class, Class::Field);
        assert!(app.tags.take_changes().is_empty());
    }

    #[test]
    fn class_ids_round_trip_and_rows_fall_back_to_their_doc() {
        for c in Class::ALL {
            assert_eq!(Class::from_id(c.id()), Some(c));
        }
        assert_eq!(Class::of_row(&json!({"doc": "bitmaps"})), Class::Raw);
        assert_eq!(Class::of_row(&json!({"doc": "tags", "class": "shape"})), Class::Shape);
        assert_eq!(Class::of_row(&json!({"doc": "object"})), Class::Field);
    }
}
