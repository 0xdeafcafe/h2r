//! The command layer's additions to [`Action`]: the edits the inspector,
//! the playlist table and the agent API share, and [`App::perform`], which
//! runs an action for a caller that wants its outcome (the agent API)
//! rather than a log line and a prompt (the window).
//!
//! The window turns clicks into actions and [`App::dispatch`]es them; the
//! API ([`crate::api`]) turns JSON calls into the same actions and
//! [`App::perform`]s them. Both reach [`App::run`], so nothing the window
//! edits is out of the API's reach.
use hopper_edit::model::{self, EditHopper, MAX_WEIGHT};
use live_proto::hopper::defaults;
use live_proto::hopper::{MAX_DESCRIPTION, MAX_RECORDS, MAX_TITLE};
use scenario_edit::layout::{self, Role};
use scenario_edit::{BlockKind, Command, Value};

use super::{Action, App, NavAction, Selection, ToolKind, tag_tree};

/// Commands that the menus and the palette can dispatch directly.
#[derive(Clone, Copy)]
pub(crate) enum MenuCommand {
    Save,
    SaveVariant,
    SavePlaylist,
    NewPlaylist,
    DefaultPlaylist,
    Undo,
    Redo,
    Duplicate,
    Delete,
    AllowGrowth,
    FrameAll,
    FocusSelected,
    Overlay,
    Quit,
}

pub(crate) struct MenuItem {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub action: Option<Action>,
}

impl MenuCommand {
    const ALL: [Self; 14] = [
        Self::Save,
        Self::SaveVariant,
        Self::SavePlaylist,
        Self::NewPlaylist,
        Self::DefaultPlaylist,
        Self::Undo,
        Self::Redo,
        Self::Duplicate,
        Self::Delete,
        Self::AllowGrowth,
        Self::FrameAll,
        Self::FocusSelected,
        Self::Overlay,
        Self::Quit,
    ];

    pub fn item(self, app: &App) -> MenuItem {
        let map = matches!(app.tool, ToolKind::Scenario | ToolKind::Tags);
        let playlists = app.tool == ToolKind::Playlists;
        let open = app.session.is_open();
        let selected = app.session.selected().is_some();
        let (label, shortcut, action, enabled) = match self {
            Self::Save => ("Save", "Cmd+S", Action::Save, map && open),
            Self::SaveVariant => (
                "Save variant",
                "Cmd+S",
                Action::SaveVariant,
                app.tool == ToolKind::Variants
                    && app
                        .variants
                        .as_ref()
                        .is_some_and(|e| e.is_dirty() && !e.is_read_only()),
            ),
            Self::SavePlaylist => (
                "Save playlist file",
                "Cmd+S",
                Action::SaveHopper,
                playlists && app.hoppers.path().is_some(),
            ),
            Self::NewPlaylist => ("New playlist file", "", Action::NewHopper, playlists),
            Self::DefaultPlaylist => (
                "New from the default lineup",
                "",
                Action::NewDefaultHopper,
                playlists,
            ),
            Self::Undo => ("Undo", "Cmd+Z", Action::Undo, app.can_undo()),
            Self::Redo => ("Redo", "Shift+Cmd+Z", Action::Redo, app.can_redo()),
            Self::Duplicate => (
                "Duplicate",
                "Cmd+D",
                Action::Duplicate,
                app.tool == ToolKind::Scenario && selected,
            ),
            Self::Delete => (
                "Delete",
                "Del",
                Action::Delete,
                app.tool == ToolKind::Scenario && selected,
            ),
            Self::AllowGrowth => (
                "Allow growing full blocks",
                "",
                Action::SetAllowGrowth(!app.session.allow_growth()),
                open,
            ),
            Self::FrameAll => ("Frame map", "Home", Action::FrameAll, open),
            Self::FocusSelected => ("Focus selection", "F", Action::FocusSelected, selected),
            Self::Overlay => (
                "Halo 1 overlay",
                "",
                Action::SetOverlay(!app.overlay_shown()),
                app.compare().is_some_and(|c| c.view.is_ok()),
            ),
            Self::Quit => ("Quit", "Cmd+Q", Action::Quit, true),
        };
        MenuItem {
            label,
            shortcut,
            action: enabled.then_some(action),
        }
    }
}

pub(crate) const PALETTE_LIMIT: usize = 20;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Map,
    Tag,
    Placement,
    Command,
}

impl Kind {
    pub const ALL: [Self; 4] = [Self::Map, Self::Tag, Self::Placement, Self::Command];

    pub fn name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Tag => "tag",
            Self::Placement => "placement",
            Self::Command => "command",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Map => "Maps",
            Self::Tag => "Tags",
            Self::Placement => "Placements",
            Self::Command => "Commands",
        }
    }
}

pub(crate) struct Entry {
    pub kind: Kind,
    pub label: String,
    pub detail: String,
    pub action: Action,
}

#[derive(Default)]
pub(crate) struct Palette {
    pub entries: Vec<Entry>,
    totals: [usize; Kind::ALL.len()],
}

impl Palette {
    pub fn total(&self, kind: Kind) -> usize {
        self.totals[kind as usize]
    }

    fn add(
        &mut self,
        kind: Kind,
        label: impl Into<String>,
        detail: impl Into<String>,
        action: Action,
    ) {
        let count = &mut self.totals[kind as usize];
        *count += 1;
        if *count <= PALETTE_LIMIT {
            self.entries.push(Entry {
                kind,
                label: label.into(),
                detail: detail.into(),
                action,
            });
        }
    }
}

fn words_match(query: &str, label: &str, detail: &str) -> bool {
    let haystack = format!("{label} {detail}").to_ascii_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_ascii_lowercase()))
}

/// Matches the outliner row's block, placement label or explicit index.
pub(crate) fn placement_matches(query: &str, block: &str, label: &str, index: usize) -> bool {
    words_match(query, label, &format!("{block} #{index}"))
}

/// The palette's visible entries and untruncated counts, in display order.
pub(crate) fn palette(app: &mut App, query: &str) -> Palette {
    app.sync_tags();
    let mut results = Palette::default();
    let mut map = |label: &str, detail: String, action: Action| {
        if words_match(query, label, &detail) {
            results.add(Kind::Map, label, detail, action);
        }
    };
    for entry in app.halo2_map_entries() {
        let action = if app.loaded_map() == Some(entry.stem) && app.opened_package().is_none() {
            Action::SetTool(ToolKind::Scenario)
        } else {
            Action::SelectMap(entry.stem.to_owned())
        };
        map(
            entry.label,
            format!("{}.map · {}", entry.stem, entry.group.label()),
            action,
        );
    }
    for entry in app.map_pack_entries() {
        let action = if app.opened_pack_map() == Some(entry.map.as_str()) {
            Action::SetTool(ToolKind::Scenario)
        } else {
            Action::OpenMapPack(entry.map.clone())
        };
        map(&entry.name, format!("{}.map · Map Pack", entry.map), action);
    }
    for entry in app.packages() {
        let key = entry.key();
        let action = if app.opened_package() == Some(key.as_str()) {
            Action::SetTool(ToolKind::Scenario)
        } else {
            Action::OpenPackageEntry(key)
        };
        map(
            &entry.label(),
            format!("{}.map · Package {}", entry.map, entry.dir),
            action,
        );
    }
    for entry in app.halo1_map_entries() {
        let action = if app.loaded_halo1() == Some(entry.stem) {
            Action::SetTool(ToolKind::Scenario)
        } else {
            Action::SelectHalo1Map(entry.stem.to_owned())
        };
        map(entry.label, format!("{}.map · Halo 1", entry.stem), action);
    }

    let tags = app.tags.tags();
    for i in tag_tree::search(tags, query) {
        let tag = &tags[i];
        results.add(
            Kind::Tag,
            tag.name.as_str(),
            tags::defs::fourcc_text(tag.group),
            Action::GoTo(Selection::Tag(tag.id)),
        );
    }

    if let Some(scenario) = app.session.scenario() {
        for block in &scenario.blocks {
            let kind = block.def.kind;
            for index in 0..block.len() {
                let label = scenario.label(kind, index);
                if placement_matches(query, block.def.label, &label, index) {
                    results.add(
                        Kind::Placement,
                        label,
                        block.def.label,
                        Action::GoTo(Selection::Placement(kind, index)),
                    );
                }
            }
        }
    }

    for command in MenuCommand::ALL {
        let item = command.item(app);
        if let Some(action) = item.action
            && words_match(query, item.label, item.shortcut)
        {
            results.add(Kind::Command, item.label, item.shortcut, action);
        }
    }
    results
}

/// One playlist edit, applied to the working copy as one undo step. The
/// playlist table's row buttons and weight drags, and `playlist.*` calls,
/// are these.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistEdit {
    Title {
        playlist: usize,
        text: String,
    },
    Description {
        playlist: usize,
        text: String,
    },
    /// An entry's weight, 1..=[`MAX_WEIGHT`].
    Weight {
        playlist: usize,
        entry: usize,
        weight: u32,
    },
    /// Move an entry to a new position.
    Move {
        playlist: usize,
        entry: usize,
        to: usize,
    },
    Duplicate {
        playlist: usize,
        entry: usize,
    },
    Remove {
        playlist: usize,
        entry: usize,
    },
    /// Add an entry (a copy of the last one).
    Add {
        playlist: usize,
    },
    /// Make the playlist one of a template's kind
    /// ([`live_proto::hopper::defaults::templates`], by default playlist
    /// id): an unused slot becomes a playlist with one entry; a playlist
    /// keeps its text and entries.
    Kind {
        playlist: usize,
        template: u32,
    },
    /// Make the playlist an unused slot.
    Clear {
        playlist: usize,
    },
}

impl PlaylistEdit {
    pub fn playlist(&self) -> usize {
        match self {
            PlaylistEdit::Title { playlist, .. }
            | PlaylistEdit::Description { playlist, .. }
            | PlaylistEdit::Weight { playlist, .. }
            | PlaylistEdit::Move { playlist, .. }
            | PlaylistEdit::Duplicate { playlist, .. }
            | PlaylistEdit::Remove { playlist, .. }
            | PlaylistEdit::Add { playlist }
            | PlaylistEdit::Kind { playlist, .. }
            | PlaylistEdit::Clear { playlist } => *playlist,
        }
    }

    /// Applies the edit to its playlist, after checking it.
    pub fn apply(&self, hoppers: &mut [EditHopper]) -> Result<(), String> {
        let n = hoppers.len();
        let h = hoppers
            .get_mut(self.playlist())
            .ok_or_else(|| format!("no playlist {} (1..{n})", self.playlist() + 1))?;
        self.apply_to(h)
    }

    /// Applies the edit to `h`, which must be its playlist.
    pub fn apply_to(&self, h: &mut EditHopper) -> Result<(), String> {
        let len = h.records.len();
        let entry_ok = |e: usize| {
            if e < len {
                Ok(())
            } else {
                Err(format!("no entry {} (the playlist has {len})", e + 1))
            }
        };
        let done = match self {
            PlaylistEdit::Title { text, .. } => {
                h.title = model::clip(text, MAX_TITLE);
                true
            }
            PlaylistEdit::Description { text, .. } => {
                h.description = model::clip(text, MAX_DESCRIPTION);
                true
            }
            PlaylistEdit::Weight { entry, weight, .. } => {
                entry_ok(*entry)?;
                if !(1..=MAX_WEIGHT).contains(weight) {
                    return Err(format!("a weight is 1..={MAX_WEIGHT}, not {weight}"));
                }
                h.records[*entry].dwords[0] = *weight;
                true
            }
            PlaylistEdit::Move { entry, to, .. } => {
                entry_ok(*entry)?;
                entry_ok(*to)?;
                model::move_record_to(h, *entry, *to)
            }
            PlaylistEdit::Duplicate { entry, .. } => {
                entry_ok(*entry)?;
                if len >= MAX_RECORDS {
                    return Err(format!("a playlist holds at most {MAX_RECORDS} entries"));
                }
                model::duplicate_record(h, *entry)
            }
            PlaylistEdit::Remove { entry, .. } => {
                entry_ok(*entry)?;
                model::remove_record(h, *entry)
            }
            PlaylistEdit::Add { .. } => {
                if len >= MAX_RECORDS {
                    return Err(format!("a playlist holds at most {MAX_RECORDS} entries"));
                }
                model::add_record(h)
            }
            PlaylistEdit::Kind { template, .. } => {
                if defaults::template(*template).is_none() {
                    return Err(format!("no playlist template {template}"));
                }
                model::create_playlist(h, *template)
            }
            PlaylistEdit::Clear { .. } => {
                model::clear_playlist(h);
                true
            }
        };
        if done || matches!(self, PlaylistEdit::Move { entry, to, .. } if entry == to) {
            Ok(())
        } else {
            Err("the playlist refused the edit".into())
        }
    }
}

/// Why [`App::perform`] did not do what was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// The action would drop `tool`'s unsaved edits.
    Unsaved(ToolKind),
    /// It ran and failed; the log's error lines.
    Failed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Unsaved(t) => write!(
                f,
                "{} has unsaved edits: save them, or pass \"discard\": true",
                t.title()
            ),
            Refused::Failed(e) => f.write_str(e),
        }
    }
}

/// The `&'static` name of a block's field (commands keep field names
/// static), or why there is none.
pub fn field_name(kind: BlockKind, field: &str) -> Result<&'static str, String> {
    layout::block(kind)
        .fields
        .iter()
        .find(|f| f.name == field)
        .map(|f| f.name)
        .ok_or_else(|| {
            let names: Vec<_> = layout::block(kind).fields.iter().map(|f| f.name).collect();
            format!(
                "{kind:?} has no field {field:?} (fields: {})",
                names.join(", ")
            )
        })
}

impl App {
    pub(super) fn go_to(&mut self, selection: Selection) {
        match selection {
            Selection::Tag(tag) => {
                if let Err(e) = self.navigate(NavAction::SelectTag(tag)) {
                    return self.error(e);
                }
            }
            Selection::Placement(kind, index) => {
                if !self
                    .session
                    .scenario()
                    .is_some_and(|s| index < s.block(kind).len())
                {
                    return self.error(format!("no {kind:?} placement {index}"));
                }
                self.run(Action::Select(Some((kind, index))));
                self.frame_selection();
            }
        }
        self.set_tool(ToolKind::Scenario);
    }

    /// Uses `editor` as the variant tool's (tests and tools with their own
    /// save storage).
    pub fn set_variant_editor(&mut self, editor: variants::VariantEditor) {
        self.variants = Some(editor);
        self.variants_for = Some(self.source.as_ref().map(|s| s.path().to_owned()));
    }

    /// Runs `action` for a caller that wants the outcome: the info lines
    /// it logged, or why it did nothing. An action that would drop unsaved
    /// edits is refused unless `discard`, never held for a prompt.
    pub fn perform(&mut self, action: Action, discard: bool) -> Result<Vec<String>, Refused> {
        if let Some(tool) = action.discards(self) {
            if !discard {
                return Err(Refused::Unsaved(tool));
            }
            self.close_tool(tool);
        }
        let before = self.logged;
        self.run(action);
        let new = (self.logged - before).min(self.log.len() as u64) as usize;
        let lines = &self.log[self.log.len() - new..];
        let errors: Vec<String> = lines
            .iter()
            .filter(|l| l.status.is_error())
            .map(|l| l.status.text().to_owned())
            .collect();
        if !errors.is_empty() {
            return Err(Refused::Failed(errors.join("; ")));
        }
        Ok(lines.iter().map(|l| l.status.text().to_owned()).collect())
    }

    /// [`Action::SetField`].
    pub(super) fn set_field(
        &mut self,
        block: BlockKind,
        index: usize,
        field: &str,
        value: Value,
        merge: Option<u64>,
    ) {
        let field = match field_name(block, field) {
            Ok(f) => f,
            Err(e) => return self.error(e),
        };
        let mut command = Command::set(block, index, field, value);
        if let Some(key) = merge {
            command = command.merging(key);
        }
        if let Err(e) = self.session.apply(command) {
            self.error(e.to_string());
        }
    }

    /// [`Action::MovePlacement`]: the position and, given, the rotation (or
    /// facing) as one undo step.
    pub(super) fn move_placement(
        &mut self,
        block: BlockKind,
        index: usize,
        position: [f32; 3],
        ypr: Option<[f32; 3]>,
    ) {
        let def = layout::block(block);
        let Some(pos) = def.role(Role::Position) else {
            return self.error(format!("{block:?} has no position"));
        };
        let mut ops = vec![Command::set(block, index, pos.name, Value::Vec3(position)).ops];
        if let Some(ypr) = ypr {
            if let Some(r) = def.role(Role::Rotation) {
                ops.push(Command::set(block, index, r.name, Value::Vec3(ypr)).ops);
            } else if let Some(r) = def.role(Role::Facing) {
                ops.push(Command::set(block, index, r.name, Value::Float(ypr[0])).ops);
            } else {
                return self.error(format!("{block:?} has no rotation or facing"));
            }
        }
        let command = Command {
            label: format!("move {block:?} {index}"),
            ops: ops.concat(),
            merge: None,
        };
        match self.session.apply(command) {
            Ok(()) => self.info(format!("moved {block:?} {index}")),
            Err(e) => self.error(e.to_string()),
        }
    }

    /// [`Action::EditPlaylist`].
    pub(super) fn edit_playlist(&mut self, edit: PlaylistEdit) {
        if !self.hoppers.is_open() {
            return self.error("no playlist file is open");
        }
        let r = self.hoppers.edit_with(|hs, _| (edit.apply(hs), None));
        self.hoppers.seal();
        match r {
            Some(Ok(())) => self.info(format!("playlist {}: edited", edit.playlist() + 1)),
            Some(Err(e)) => self.error(e),
            None => self.error("no playlist file is open"),
        }
    }
}
