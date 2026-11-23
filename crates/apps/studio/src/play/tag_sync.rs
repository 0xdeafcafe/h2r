//! Compare whole fields against the loaded file plus acknowledged game writes.
//! The sparse baseline keeps addresses after undo removes a document patch.
//!
//! Also the game side of the fan-out ([`super::Play::fan_out`],
//! docs/editor/live-editors.md): each frame it takes the document's
//! changes, has `app::live` route them, and carries out each delivery
//! over the link. A Pending row says its fan-out class (`class`) beside
//! its effect.
use super::link::Purpose;
use super::playtest::{self, Waiting};
use super::{raw_sync, sync};
use crate::app::live::{Change, Class, Host, Route, Target};
use crate::app::{App, TagDoc};
use map::TagId;
use scenario_edit::BspPatch;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tags::registry::{Field, Kind, Layout};

/// When a change to a field takes effect in a running game
/// (docs/editor/tag-editing.md, "When it takes effect"): an id for the
/// API, a short chip for a row and a sentence for tooltips, details and
/// the Pending tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Effect {
    /// The game reads the field each frame (`applies=frame`).
    Now,
    /// The game reads it the next time it uses it (`applies=use`).
    NextUse,
    /// The game copies it into an object when the object is made
    /// (`applies=creation`).
    NextSpawn,
    /// The game reads it at map load: the change needs a reload.
    Reload,
    /// No definition says when the game reads it (no `applies`).
    NotKnown,
}

impl Effect {
    pub const ALL: [Self; 5] = [
        Self::Now,
        Self::NextUse,
        Self::NextSpawn,
        Self::Reload,
        Self::NotKnown,
    ];

    /// The API's id (`mirror.pending`, `tag.fields`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::NextUse => "next_use",
            Self::NextSpawn => "next_spawn",
            Self::Reload => "reload",
            Self::NotKnown => "not_known",
        }
    }

    /// The effect an id names.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.id() == id)
    }

    /// The row's chip.
    pub fn chip(self) -> &'static str {
        match self {
            Self::Now => "Now",
            Self::NextUse => "Next use",
            Self::NextSpawn => "Next spawn",
            Self::Reload => "After reload",
            Self::NotKnown => "Not known",
        }
    }

    /// The sentence for tooltips, details and the Pending tab's headings
    /// (a reload's reason follows it, apart).
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Now => "Applies now.",
            Self::NextUse => "Applies the next time the game uses it.",
            Self::NextSpawn => "Applies when the object next spawns.",
            Self::Reload => "Applies after reload.",
            Self::NotKnown => "Not known; reload to be sure.",
        }
    }

    /// A pending row's effect (`effect`, its id).
    pub fn of_row(row: &Value) -> Option<Self> {
        row["effect"].as_str().and_then(Self::from_id)
    }

    /// Whether `row` waits for a reload.
    pub fn is_reload(row: &Value) -> bool {
        Self::of_row(row) == Some(Self::Reload)
    }

    /// The effect a definition's `applies` word gives.
    pub fn of_applies(applies: Option<&str>) -> Self {
        match applies {
            Some("frame") => Self::Now,
            Some("use") => Self::NextUse,
            Some("creation") => Self::NextSpawn,
            _ => Self::NotKnown,
        }
    }
}

#[derive(Default, Clone)]
pub struct TagBaseline {
    bytes: BTreeMap<u32, u8>,
    bsp_patches: Vec<BspPatch>,
    // Operations already baked into a Play copy, including provisional
    // addresses whose actual storage was allocated by the map writer.
    structural: Vec<(String, Vec<(u32, u32)>)>,
    strings: Vec<String>,
}

impl TagBaseline {
    pub fn of(doc: &TagDoc) -> Self {
        let mut base = Self {
            bsp_patches: doc.bsp_patches(),
            ..Self::default()
        };
        for (at, bytes) in doc.patches() {
            base.wrote(at, &bytes);
        }
        for (label, ranges) in doc.structural_edits() {
            for &(at, n) in ranges {
                if let Some(bytes) = doc.read_metadata(at, n) {
                    base.wrote(at, &bytes);
                }
            }
            base.structural.push((label.clone(), ranges.clone()));
        }
        base.strings = doc.new_strings().to_vec();
        base
    }

    pub fn wrote(&mut self, at: u32, bytes: &[u8]) {
        self.bytes.extend((at..).zip(bytes.iter().copied()));
    }
}

pub struct Write {
    pub tag: TagId,
    pub path: String,
    pub nav_path: String,
    pub at: u32,
    pub value: Vec<u8>,
    pub was: Vec<u8>,
    pub effect: Effect,
}

/// A pending row: what changed (`doc`, `tag`, `path`), its fan-out class
/// (`class`, by `doc` until [`set_class`] says otherwise), when it takes
/// effect (`effect`, the id, and `text`, its sentence) and why.
pub fn row(doc: &str, tag: Value, path: &str, effect: Effect, why: &str) -> Value {
    json!({
        "doc": doc,
        "tag": tag,
        "path": path,
        "class": Class::of_doc(doc).id(),
        "effect": effect.id(),
        "text": effect.sentence(),
        "why": why,
    })
}

/// Sets `row`'s fan-out class.
pub fn set_class(row: &mut Value, class: Class) {
    row["class"] = json!(class.id());
}

/// Sets `row`'s effect and its sentence.
pub fn set_effect(row: &mut Value, effect: Effect) {
    row["effect"] = json!(effect.id());
    row["text"] = json!(effect.sentence());
}

pub fn effect(doc: &TagDoc, tag: TagId, at: u32, field: &Field) -> (Effect, &'static str) {
    let reload = |why| (Effect::Reload, why);
    let Some(entry) = doc.tag(tag) else {
        return reload("tag is absent");
    };
    if entry.bsp.is_some() || matches!(&entry.group.0, b"sbsp" | b"ltmp" | b"coll" | b"phmo") {
        return reload("geometry or collision is built at map load");
    }
    if doc.is_grown_room(at) {
        return reload("the block grew beyond its loaded address");
    }
    match &field.kind {
        Kind::Block(_) | Kind::Data | Kind::Raw | Kind::RawSize { .. } | Kind::Bsp => {
            return reload("block layout or resource storage changes at map load");
        }
        Kind::TagRef { .. } | Kind::Datum { .. } => {
            let offset = if matches!(field.kind, Kind::TagRef { .. }) {
                4
            } else {
                0
            };
            let datum = doc
                .read(tag, at + offset, 4)
                .and_then(|v| Some(u32::from_le_bytes(v.try_into().ok()?)));
            if datum.is_none_or(|d| d != u32::MAX && !doc.resident(TagId(d))) {
                return reload("reference target is absent from the loaded metadata");
            }
        }
        Kind::StringId => {
            let id = doc
                .read(tag, at, 4)
                .and_then(|v| Some(u32::from_le_bytes(v.try_into().ok()?)));
            if id.is_none_or(|id| id != u32::MAX && !doc.source_string(id)) {
                return reload("string ID is absent from the loaded string table");
            }
        }
        _ => {}
    }
    let effect = Effect::of_applies(field.applies);
    let why = match effect {
        Effect::Now => "read on the next frame",
        Effect::NextUse => "read on the next use",
        Effect::NextSpawn => "read when the object is made",
        Effect::Reload | Effect::NotKnown => "when the game reads this field is not known",
    };
    (effect, why)
}

/// Find a field in a tag's root or a reachable block. Resource payloads and
/// addresses outside the owning instance have no writable field here.
fn field_at<'a>(
    doc: &TagDoc,
    tag: TagId,
    root: u32,
    layout: &'a Layout,
    at: u32,
    depth: u8,
) -> Option<(u32, &'a Field, String)> {
    if let Some(f) = at.checked_sub(root).and_then(|o| layout.field_covering(o)) {
        return Some((root.checked_add(f.offset)?, f, format!("+{:#x}", f.offset)));
    }
    if depth == 0 {
        return None;
    }
    for field in &layout.fields {
        let Kind::Block(child) = &field.kind else {
            continue;
        };
        let Some((count, first)) = root
            .checked_add(field.offset)
            .and_then(|p| doc.block(tag, p, child))
        else {
            continue;
        };
        if child.size == 0 {
            continue;
        }
        if let Some(index) = at
            .checked_sub(first)
            .map(|o| o / child.size)
            .filter(|i| *i < count)
        {
            let start = first + index * child.size;
            if let Some(f) = child.field_covering(at - start) {
                return Some((
                    start + f.offset,
                    f,
                    format!("+{:#x}[{index}].+{:#x}", field.offset, f.offset),
                ));
            }
        }
        if child
            .fields
            .iter()
            .any(|f| matches!(f.kind, Kind::Block(_)))
        {
            for i in 0..count {
                if let Some((start, nested, path)) =
                    field_at(doc, tag, first + i * child.size, child, at, depth - 1)
                {
                    return Some((start, nested, format!("+{:#x}[{i}].{path}", field.offset)));
                }
            }
        }
    }
    None
}

pub fn diff(base: &TagBaseline, doc: &TagDoc) -> (Vec<Write>, Vec<Value>) {
    let mut addresses: BTreeSet<u32> = base.bytes.keys().copied().collect();
    for (at, bytes) in doc.patches() {
        addresses.extend((at..).take(bytes.len()));
    }
    let mut rows = Vec::new();
    let bsp_patches = doc.bsp_patches();
    let spaces: BTreeSet<_> = bsp_patches
        .iter()
        .chain(&base.bsp_patches)
        .map(|p| p.space)
        .collect();
    for space in spaces {
        let now = bsp_patches.iter().filter(|p| p.space == space);
        let was = base.bsp_patches.iter().filter(|p| p.space == space);
        if !now.eq(was) {
            rows.push(row(
                "tags",
                Value::Null,
                &format!("BSP region at file offset {:#x}", space.file_offset),
                Effect::Reload,
                "geometry or collision is built at map load",
            ));
        }
    }
    let mut excluded = Vec::new();
    let mut structures = BTreeSet::new();
    for (label, ranges) in doc.structural_edits().chain(&base.structural) {
        if !structures.insert((label, ranges)) {
            continue;
        }
        excluded.extend_from_slice(ranges);
        if ranges.iter().any(|&(at, n)| {
            let Some(now) = doc.read_metadata(at, n) else {
                return true;
            };
            let Some(src) = doc.read_metadata_source(at, n) else {
                return true;
            };
            now.iter()
                .zip(src)
                .enumerate()
                .any(|(i, (b, original))| b != base.bytes.get(&(at + i as u32)).unwrap_or(original))
        }) {
            let mut layout = row(
                "tags",
                Value::Null,
                label,
                Effect::Reload,
                "the block layout changed",
            );
            set_class(&mut layout, Class::Shape);
            rows.push(layout);
        }
    }
    for string in doc
        .new_strings()
        .iter()
        .filter(|s| !base.strings.contains(s))
    {
        let mut added = row("tags", Value::Null, string, Effect::Reload, "new map string");
        set_class(&mut added, Class::Shape);
        rows.push(added);
    }
    for string in base
        .strings
        .iter()
        .filter(|s| !doc.new_strings().contains(s))
    {
        let mut removed = row(
            "tags",
            Value::Null,
            string,
            Effect::Reload,
            "remove string from the play copy",
        );
        set_class(&mut removed, Class::Shape);
        rows.push(removed);
    }
    let mut writes = Vec::new();
    let mut covered = 0u64;
    for at in addresses {
        if u64::from(at) < covered
            || excluded
                .iter()
                .any(|&(a, n)| at.checked_sub(a).is_some_and(|i| i < n))
        {
            continue;
        }
        let Some(entry) = doc
            .tags()
            .iter()
            .find(|t| t.bsp.is_none() && at.checked_sub(t.address).is_some_and(|o| o < t.size))
        else {
            rows.push(row(
                "tags",
                Value::Null,
                &format!("{at:#x}"),
                Effect::Reload,
                "outside a loaded tag",
            ));
            continue;
        };
        let id = json!(format!("{:#010x}", entry.id.0));
        let Some((start, f, nav_path)) = doc
            .layout(entry.id)
            .and_then(|l| field_at(doc, entry.id, entry.address, l, at, 12))
        else {
            rows.push(row(
                "tags",
                id,
                &format!("{at:#x}"),
                Effect::Reload,
                "resource or unlocated field",
            ));
            continue;
        };
        let n = f.kind.width();
        covered = u64::from(start) + u64::from(n);
        let Some(mut was) = doc.read_source(entry.id, start, n).map(<[u8]>::to_vec) else {
            continue;
        };
        for (a, b) in (start..).zip(&mut was) {
            if let Some(v) = base.bytes.get(&a) {
                *b = *v;
            }
        }
        let Some(value) = doc.read(entry.id, start, n) else {
            continue;
        };
        if value == was {
            continue;
        }
        let (when, why) = if covered > u64::from(entry.address) + u64::from(entry.size) {
            (Effect::Reload, "the field extends outside its loaded tag")
        } else {
            effect(doc, entry.id, start, f)
        };
        let path = format!("{} · {} ({start:#x})", entry.name, f.name);
        let mut pending = row("tags", id, &path, when, why);
        // What the Pending tab shows: the tag and the field, no address.
        pending["label"] = json!(format!("{} · {}", entry.name, f.name));
        pending["at"] = json!(start);
        if crate::app::tag_api::field_focus(doc, entry.id, &nav_path).is_ok() {
            pending["target"] = json!({"tag": entry.id.0, "path": nav_path});
        }
        rows.push(pending);
        if when != Effect::Reload {
            writes.push(Write {
                tag: entry.id,
                path,
                nav_path,
                at: start,
                value,
                was,
                effect: when,
            });
        }
    }
    (writes, rows)
}

/// Re-resolve the submitted tag field, including every indexed block along its path.
pub(crate) fn check_conflict_path(doc: &TagDoc, row: &Value) -> Result<(), &'static str> {
    if row["doc"] == "tags" {
        let current = row["target"]["path"].as_str().and_then(|path| {
            let tag = crate::app::tag_api::lookup(doc, &row["tag"]).ok()?;
            crate::app::tag_api::field_focus(doc, tag, path).ok()
        });
        if !current.is_some_and(|field| row["at"].as_u64() == Some(u64::from(field.address))) {
            return Err("the conflicted tag field moved. Undo the layout change or reload the map");
        }
    }
    Ok(())
}

impl Write {
    pub fn wire(&self) -> Value {
        json!({"at":self.at,"value":sync::hex(&self.value),"was":sync::hex(&self.was)})
    }
}

/// The scenario session's revisions, as the fan-out compares them: the
/// open document, its edits (and selection), and each block's layout.
fn scenario_seen(app: &App) -> (u64, u64, u64) {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Some(s) = app.session.scenario() {
        for b in &s.blocks {
            b.layout_rev().hash(&mut h);
        }
    }
    (app.session.world_rev(), app.session.markers_rev(), h.finish())
}

impl super::Play {
    /// Where the game view's changes go: Studio's own game, an attached
    /// one (in Edit or not), or none.
    pub(crate) fn live_target(&self) -> Target {
        if !self.is_paired() {
            Target::Off
        } else if self.own_game() {
            Target::Own
        } else {
            Target::Attached {
                edit: self.live_edits,
            }
        }
    }

    /// Once a frame, after the edits are pushed: takes every write path's
    /// changes (the tag document's own; the scenario session's and the raw
    /// documents' by their revisions), routes them by class
    /// (`app::live::route`) and carries out the game's deliveries. Field
    /// and Raw deliveries are one send of what the documents hold
    /// (`edit.batch`, the raw edits); a Shape delivery is that send and,
    /// for what it leaves under After reload, one play-copy reload; a
    /// Host delivery is the source's reload method. A delivery the game
    /// cannot take stays a Pending row.
    pub(crate) fn fan_out(&mut self, app: &mut App) {
        let target = self.live_target();
        app.live.set_target(target);
        for change in app.tags.take_changes() {
            app.live.emit(change);
        }
        see_documents(app);
        if target == Target::Off {
            app.live.owed = Default::default();
        }
        for d in app.live.flush() {
            match d.route {
                Route::Reread | Route::Pending => {}
                // An attached game in Edit takes these by its own push.
                Route::EditBatch | Route::RawEdit if target == Target::Own => {
                    app.live.owed.send = true;
                }
                Route::EditBatch | Route::RawEdit => {}
                Route::BuildLoad => {
                    app.live.owed.send = true;
                    app.live.owed.build = true;
                }
                Route::HostReload(host) => match host.method() {
                    Some(method) if self.has_method(method) => {
                        let purpose = if host == Host::Rules {
                            Purpose::Rules
                        } else {
                            Purpose::Other
                        };
                        self.call(method, d.change.params.clone(), purpose);
                    }
                    _ => app.live.hold_refused(d.change),
                },
            }
        }
        if app.live.owed.send && !self.edits.interrupted_write() {
            app.live.owed.send = false;
            self.refresh_pending(app, true);
        }
        if app.live.owed.build && !app.live.owed.send && !self.edits.interrupted_write() {
            app.live.owed.build = false;
            let after_reload = app.pairing.pending.iter().any(|r| {
                Class::of_row(r) == Class::Shape && Effect::is_reload(r)
            });
            if after_reload
                && target == Target::Own
                && self.waiting == Waiting::Idle
                && self.reload_copy.is_none()
                && let Err(why) = self.reload_for_play(app, playtest::Reload::Method)
            {
                app.error(format!("the layout change stays in Pending: {why}"));
            }
        }
        publish_held(app);
    }
}

/// Reports the scenario session's and the raw documents' writes as
/// changes, by their revisions: a block's layout is Shape, other scenario
/// edits Field, geometry and pixels Raw. A new document is not a write.
fn see_documents(app: &mut App) {
    let raw = raw_sync::doc_rev(app);
    if app.live.seen.raw.replace(raw).is_some_and(|r| r != raw) {
        let change = Change::new(Class::Raw, None, Vec::new(), "geometry and pixels");
        app.live.emit(change);
    }
    let now = scenario_seen(app);
    let before = app.live.seen.scenario.replace(now);
    if let Some((world, markers, layouts)) = before
        && world == now.0
    {
        let tag = app.session.scenario().map(|s| s.tag);
        if layouts != now.2 {
            app.live
                .emit(Change::new(Class::Shape, tag, Vec::new(), "placement layout"));
        } else if markers != now.1 {
            app.live
                .emit(Change::new(Class::Field, tag, Vec::new(), "placements"));
        }
    }
}

/// Puts the held Host changes in Pending, as rows of class `host`, and
/// takes out the rows of those that went.
fn publish_held(app: &mut App) {
    let rows = app.live.held_rows(|c| match c.host.and_then(Host::method) {
        None => Host::NO_RELOAD,
        Some(_) => "reloaded in the game once the pairing is in Edit",
    });
    let shown: Vec<&Value> = app
        .pairing
        .pending
        .iter()
        .filter(|r| r["doc"] == "host")
        .collect();
    if shown.len() == rows.len() && shown.iter().zip(&rows).all(|(a, b)| *a == b) {
        return;
    }
    app.pairing.pending.retain(|r| r["doc"] != "host");
    app.pairing.pending.extend(rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tag_tests;

    /// Each `applies` word and each reload rule gives its effect, with its
    /// id, chip and sentence; no word names a transport.
    #[test]
    fn each_applies_word_and_reload_rule_gives_its_effect() {
        for (word, id, chip, sentence) in [
            (Some("frame"), "now", "Now", "Applies now."),
            (
                Some("use"),
                "next_use",
                "Next use",
                "Applies the next time the game uses it.",
            ),
            (
                Some("creation"),
                "next_spawn",
                "Next spawn",
                "Applies when the object next spawns.",
            ),
            (
                None,
                "not_known",
                "Not known",
                "Not known; reload to be sure.",
            ),
        ] {
            let e = Effect::of_applies(word);
            assert_eq!((e.id(), e.chip(), e.sentence()), (id, chip, sentence));
            assert_eq!(Effect::from_id(id), Some(e));
        }
        let reload = Effect::Reload;
        assert_eq!(
            (reload.id(), reload.chip(), reload.sentence()),
            ("reload", "After reload", "Applies after reload.")
        );

        let t = crate::play::testing::Temp::new("tag-sync-effect");
        let (app, _, _) = tag_tests::app(&t.0);
        let weap = tag_tests::weap_id();
        let layout = app.tags.layout(weap).unwrap();
        let speed = layout.field_covering(0).unwrap();
        assert_eq!(
            effect(&app.tags, weap, tag_tests::WEAP, speed),
            (
                Effect::NotKnown,
                "when the game reads this field is not known"
            )
        );
        let parts = layout.field_covering(8).unwrap();
        assert_eq!(
            effect(&app.tags, weap, tag_tests::WEAP + 8, parts),
            (
                Effect::Reload,
                "block layout or resource storage changes at map load"
            )
        );
        assert_eq!(
            effect(&app.tags, TagId(0x1234_5678), 0, speed),
            (Effect::Reload, "tag is absent")
        );
        let pending = row("tags", Value::Null, "x", Effect::NextUse, "why");
        assert_eq!(pending["effect"], "next_use");
        assert_eq!(pending["text"], "Applies the next time the game uses it.");
        assert_eq!(pending["class"], "field");
        assert_eq!(Effect::of_row(&pending), Some(Effect::NextUse));
    }
}

/// The fan-out on the invented game (docs/editor/live-editors.md, LE1).
#[cfg(all(test, unix))]
mod fan_out_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use agent::Access;
    use agent_proto::Endpoint;
    use agent_proto::sim::SessionKind;
    use agent_proto::sim::method::MAP_RELOAD;
    use serde_json::{Value, json};

    use crate::app::live::{Change, Class, Host, Route, View};
    use crate::app::{Action, App, BlockOp, TagAction, tag_tests};
    use crate::play::Play;
    use crate::play::testing::Temp;
    use crate::play::testing::fake::{Game, until};

    /// The invented game with `render.reload` counted, and the editor
    /// paired with it over the tag fixture; `own` makes it Studio's own.
    fn paired(name: &str, own: bool) -> (Temp, Game, App, Play, Arc<AtomicU32>) {
        let t = Temp::new(name);
        let game = Game::start(name, "example", SessionKind::Local);
        let reloads = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&reloads);
        game.agent.add_method(
            agent_proto::hot::method::RENDER_RELOAD,
            Access::Act,
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            },
        );
        let (mut app, _, _) = tag_tests::app(&t.0);
        app.pairing.auto_pair = false;
        let mut play = Play::new();
        play.pair(&mut app, Endpoint::Unix(game.path.clone()));
        until(&mut play, &mut app, "game identity and journal", |p, a| {
            p.edits.journal_ready && p.edit_refusal(a).is_none()
        });
        if own {
            play.child_endpoint = play.endpoint().cloned();
        }
        (t, game, app, play, reloads)
    }

    fn speed(app: &mut App) {
        let action =
            crate::app::tag_api::set_field(&app.tags, tag_tests::weap_id(), "speed", &json!(4.5))
                .unwrap();
        app.dispatch(Action::Tag(action));
    }

    fn remove_part(app: &mut App) {
        let field = app
            .tags
            .layout(tag_tests::weap_id())
            .unwrap()
            .field_at(8)
            .unwrap()
            .clone();
        let tags::registry::Kind::Block(layout) = &field.kind else {
            panic!("the weapon's parts are a block")
        };
        app.dispatch(Action::Tag(TagAction::Block {
            tag: tag_tests::weap_id(),
            pair: tag_tests::WEAP + 8,
            layout: layout.clone(),
            op: BlockOp::Remove(1),
        }));
    }

    fn frames(play: &mut Play, app: &mut App, n: usize) {
        for _ in 0..n {
            play.update(app);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn delivered(app: &App, view: View, class: Class) -> Vec<Route> {
        app.live
            .delivered()
            .iter()
            .filter(|(v, _, c)| *v == view && *c == class)
            .map(|(_, r, _)| *r)
            .collect()
    }

    /// A field write to the own game is one `edit.batch`, sent with no
    /// click, and one Preview re-read.
    #[test]
    fn a_field_write_is_one_batch_in_the_own_game() {
        let (_t, game, mut app, mut play, _) = paired("fan-field-own", true);
        speed(&mut app);
        until(&mut play, &mut app, "the field's batch", |p, _| {
            game.calls("edit.batch").len() == 1 && p.edits.sent.is_none()
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(game.calls("edit.batch").len(), 1);
        assert_eq!(game.calls("edit.batch")[0]["writes"].as_array().unwrap().len(), 1);
        assert_eq!(delivered(&app, View::Game, Class::Field), vec![Route::EditBatch]);
        assert_eq!(delivered(&app, View::Preview, Class::Field), vec![Route::Reread]);
    }

    /// The same write to an attached game in View is a Pending row of
    /// class `field`, and nothing is sent.
    #[test]
    fn a_field_write_is_a_pending_row_in_an_attached_game() {
        let (_t, game, mut app, mut play, _) = paired("fan-field-attached", false);
        speed(&mut app);
        until(&mut play, &mut app, "the pending field", |_, a| {
            a.pairing.pending.iter().any(|r| r["doc"] == "tags")
        });
        frames(&mut play, &mut app, 20);
        assert!(game.calls("edit.batch").is_empty());
        let row = app.pairing.pending.iter().find(|r| r["doc"] == "tags").unwrap();
        assert_eq!(row["class"], "field");
        assert_eq!(delivered(&app, View::Attached, Class::Field), vec![Route::Pending]);
        assert!(delivered(&app, View::Game, Class::Field).is_empty());
    }

    /// A Host change is one reload call in the own game; in an attached
    /// game in View it is one Pending row of class `host` and no call,
    /// and it goes once the pairing is in Edit.
    #[test]
    fn a_host_change_is_one_reload_or_one_pending_row() {
        let (_t, _game, mut app, mut play, reloads) = paired("fan-host-own", true);
        app.live
            .emit(Change::host(Host::Shader, json!({}), "shader override"));
        until(&mut play, &mut app, "the reload", |_, _| {
            reloads.load(Ordering::SeqCst) == 1
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert!(app.pairing.pending.iter().all(|r| r["doc"] != "host"));

        let (_t, _game, mut app, mut play, reloads) = paired("fan-host-attached", false);
        app.live
            .emit(Change::host(Host::Shader, json!({}), "shader override"));
        frames(&mut play, &mut app, 20);
        assert_eq!(reloads.load(Ordering::SeqCst), 0);
        let rows: Vec<&Value> = app
            .pairing
            .pending
            .iter()
            .filter(|r| r["doc"] == "host")
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0]["class"].clone(), rows[0]["host"].clone()),
            (json!("host"), json!("shader"))
        );
        play.live_edits = true;
        until(&mut play, &mut app, "the held reload", |_, _| {
            reloads.load(Ordering::SeqCst) == 1
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert!(app.pairing.pending.iter().all(|r| r["doc"] != "host"));
    }

    /// A Host source the game has no reload for waits in Pending in the
    /// own game too, saying so.
    #[test]
    fn a_host_source_without_a_reload_waits_in_pending() {
        let (_t, _game, mut app, mut play, reloads) = paired("fan-host-none", true);
        app.live.emit(Change::host(Host::HudKit, json!({}), "HUD kit"));
        frames(&mut play, &mut app, 20);
        let row = app.pairing.pending.iter().find(|r| r["doc"] == "host").unwrap();
        assert_eq!(row["why"], Host::NO_RELOAD);
        assert_eq!(reloads.load(Ordering::SeqCst), 0);
    }

    /// A block element removed is a Shape change: one play-copy reload in
    /// the own game, a Pending row of class `shape` in an attached one.
    #[test]
    fn a_shape_change_is_one_reload_or_one_pending_row() {
        let (_t, game, mut app, mut play, _) = paired("fan-shape-attached", false);
        remove_part(&mut app);
        until(&mut play, &mut app, "the layout row", |_, a| {
            a.pairing.pending.iter().any(|r| r["class"] == "shape")
        });
        frames(&mut play, &mut app, 20);
        assert!(game.calls(MAP_RELOAD).is_empty());
        assert_eq!(delivered(&app, View::Attached, Class::Shape), vec![Route::Pending]);

        let (_t, game, mut app, mut play, _) = paired("fan-shape-own", true);
        remove_part(&mut app);
        until(&mut play, &mut app, "the play-copy reload", |_, _| {
            game.calls(MAP_RELOAD).len() == 1
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(game.calls(MAP_RELOAD).len(), 1);
        assert_eq!(delivered(&app, View::Game, Class::Shape), vec![Route::BuildLoad]);
    }
}
