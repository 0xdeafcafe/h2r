//! The playlist (hopper) tool's windows: the file and its 16 playlists
//! (the outliner's place), the shown playlist's entries (the centre) and
//! its presets, variant files and raw fields (the inspector). Edits to the
//! working copy go through `HopperSession::edit_with` (one undo step per
//! change, a run of typing or dragging in one field is one step); file
//! commands go back to [`App`] as [`Action`]s.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use dear_imgui_rs::{Direction, TableColumnFlags, TableColumnWidth, TableFlags, TreeNodeFlags, Ui};
use hopper_edit::choices::{Choice, variant_label};
use hopper_edit::model::{self, EditHopper, EditRecord, MAX_WEIGHT};
use hopper_edit::presets::{self, PRESETS};
use hopper_edit::{EditView, Format};
use live_proto::hopper::{HOPPERS, MAX_NAME, MAX_RECORDS, MAX_STRING, defaults};
use variants::GameVariant;

use crate::app::{Action, App, PlaylistEdit};
use crate::theme::{self, Fonts, icon};

/// The playlist tool's own UI state.
#[derive(Default)]
pub struct PlaylistsUi {
    /// The typed `.h2variant` path.
    variant_path: String,
}

/// Hopper types as the game sorts them (live-proto `HopperType`).
const TYPES: [(u32, &str); 5] = [
    (1, "1 - unranked free for all"),
    (2, "2 - ranked free for all"),
    (3, "3 - unranked teams"),
    (4, "4 - ranked teams"),
    (5, "5 - clan match"),
];
const STATUSES: [(u32, &str); 3] = [(0, "0 - not loaded"), (1, "1 - ok"), (2, "2 - error")];

fn key(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("studio-playlists", parts).hash(&mut h);
    h.finish()
}

fn clip(s: &mut String, max: usize) {
    if s.chars().count() > max {
        *s = model::clip(s, max);
    }
}

fn title_of(h: &EditHopper) -> &str {
    if h.clear {
        "(removed on save)"
    } else if model::is_unused(h) {
        "(unused)"
    } else if !h.title.is_empty() {
        &h.title
    } else if !h.name.is_empty() {
        &h.name
    } else {
        "(untitled)"
    }
}

/// The file commands and the 16 playlists.
pub fn list(ui: &Ui, app: &mut App, fonts: &Fonts, dialogs: bool, actions: &mut Vec<Action>) {
    if ui.button(format!("{} New", icon::PLUS)) {
        actions.push(Action::NewHopper);
    }
    ui.set_item_tooltip("A blank file: every playlist empty");
    ui.same_line();
    if ui.button("Default lineup") {
        actions.push(Action::NewDefaultHopper);
    }
    ui.set_item_tooltip("The playlists a Live server serves before any file is published");
    if ui.button(format!("{} Open...", icon::FOLDER_OPEN))
        && let Some(p) = open_dialog(dialogs)
    {
        actions.push(Action::OpenHopper(p));
    }
    ui.set_item_tooltip(
        "A multiplayer_hopper.dat: the console's raw image or a downloaded envelope",
    );
    let open = app.hoppers.is_open();
    let d = ui.begin_disabled_with_cond(!open);
    if ui.button(format!("{} Save", icon::SAVE)) {
        save(app, dialogs, actions);
    }
    ui.same_line();
    if ui.button("Save as...")
        && let Some(p) = save_dialog(dialogs, app)
    {
        actions.push(Action::SaveHopperAs(p));
    }
    d.end();
    if !open {
        ui.spacing();
        ui.text_wrapped(
            "Open a multiplayer_hopper.dat (the console's raw image or a downloaded envelope), \
             or start from a blank file.",
        );
        return;
    }
    let form = app
        .hoppers
        .format()
        .map(Format::describe)
        .unwrap_or_default();
    match app.hoppers.path() {
        Some(p) => ui.text_disabled(format!("{} {}  ({form})", icon::FILE, p.display())),
        None => ui.text_disabled(format!("{} not saved yet  ({form})", icon::FILE)),
    }
    ui.separator();
    let f = ui.push_font(fonts.bold);
    ui.text("Playlists");
    f.pop();
    let Some(hoppers) = app.hoppers.hoppers() else {
        return;
    };
    let errors = app.hoppers.errors();
    let mut pick = None;
    ui.child_window("##playlist-list").build(ui, || {
        for h in hoppers {
            let bad = errors
                .iter()
                .any(|e| e.starts_with(&format!("Playlist {}:", h.index + 1)));
            let text = format!(
                "{:>2}  {}{}##hopper-{}",
                h.index + 1,
                title_of(h),
                if bad {
                    format!("  {}", icon::ALERT)
                } else {
                    String::new()
                },
                h.index
            );
            if ui
                .selectable_config(text)
                .selected(app.hopper_selected == h.index)
                .build()
            {
                pick = Some(h.index);
            }
            ui.set_item_tooltip(format!(
                "type {}, {} entr{}",
                h.hopper_type,
                h.records.len(),
                if h.records.len() == 1 { "y" } else { "ies" }
            ));
        }
    });
    if let Some(i) = pick {
        app.hopper_selected = i;
        app.hopper_preset = None;
    }
}

/// Save in place, or ask where when the file has no path yet.
pub fn save(app: &App, dialogs: bool, actions: &mut Vec<Action>) {
    if app.hoppers.path().is_some() {
        actions.push(Action::SaveHopper);
    } else if let Some(p) = save_dialog(dialogs, app) {
        actions.push(Action::SaveHopperAs(p));
    }
}

pub fn open_dialog(dialogs: bool) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Open a playlist file")
        .add_filter("Playlist file", &["dat"])
        .pick_file()
}

pub fn save_dialog(dialogs: bool, app: &App) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    let name = app
        .hoppers
        .path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "multiplayer_hopper.dat".into());
    rfd::FileDialog::new()
        .set_title("Save the playlist file as")
        .add_filter("Playlist file", &["dat"])
        .set_file_name(name)
        .save_file()
}

/// What a frame's edit produced besides the working copy.
#[derive(Default)]
struct Frame {
    key: Option<u64>,
    /// A field's edit run ended: the next edit is a new undo step.
    seal: bool,
    error: Option<String>,
}

impl Frame {
    /// Notes a widget's change under `k`, and the end of its edit run.
    fn changed(&mut self, ui: &Ui, changed: bool, k: u64) {
        if changed {
            self.key = Some(k);
        }
        self.seal |= ui.is_item_deactivated_after_edit();
    }
}

/// The shown playlist: its text and its entries.
pub fn entries(ui: &Ui, app: &mut App, fonts: &Fonts, actions: &mut Vec<Action>) {
    if !app.hoppers.is_open() {
        ui.text_disabled("No playlist file open.");
        return;
    }
    let sel = app.hopper_selected.min(HOPPERS - 1);
    let out = app.hoppers.edit_with(|hoppers, view| {
        let mut fr = Frame::default();
        let h = &mut hoppers[sel];
        let f = ui.push_font(fonts.bold);
        ui.text(format!("{}  Playlist {}", icon::LAYERS, sel + 1));
        f.pop();
        ui.same_line();
        ui.text_disabled(format!(
            "   type {}   {} / {MAX_RECORDS} entries",
            h.hopper_type,
            h.records.len()
        ));
        // An unused slot: pick a kind to make it a playlist (the
        // template's descriptor, which the raw fields cannot build).
        if model::is_unused(h) {
            ui.text_disabled(if h.clear {
                "This playlist is removed when the file is saved: its slot becomes unused."
            } else {
                "This slot is unused: the game shows no playlist here. Make one of these kinds; it starts with that kind's rules and one entry."
            });
            for t in defaults::templates() {
                if ui.button(format!("Make a {} playlist##make-{}", t.title, t.playlist_id)) {
                    actions.push(Action::EditPlaylist(PlaylistEdit::Kind {
                        playlist: sel,
                        template: t.playlist_id,
                    }));
                }
            }
            let k = fr.key;
            return (fr, k);
        }
        ui.same_line();
        if ui.small_button(format!("Remove playlist##clear-{sel}")) {
            actions.push(Action::EditPlaylist(PlaylistEdit::Clear { playlist: sel }));
        }
        let label_w = 110.0;
        ui.align_text_to_frame_padding();
        ui.text_disabled("Title");
        ui.same_line_with_pos(label_w);
        ui.set_next_item_width(-1.0);
        // Typed text goes in as the API's edit (clipped there); the
        // keystrokes of one run merge under the field's key.
        let mut text = h.title.clone();
        let c = ui.input_text("##hopper-title", &mut text).build();
        if c && let Err(e) = (PlaylistEdit::Title { playlist: sel, text }).apply_to(h) {
            fr.error = Some(e);
        }
        fr.changed(ui, c, key(("title", sel)));
        ui.align_text_to_frame_padding();
        ui.text_disabled("Description");
        ui.same_line_with_pos(label_w);
        ui.set_next_item_width(-1.0);
        let mut text = h.description.clone();
        let c = ui.input_text("##hopper-description", &mut text).build();
        if c && let Err(e) = (PlaylistEdit::Description { playlist: sel, text }).apply_to(h) {
            fr.error = Some(e);
        }
        fr.changed(ui, c, key(("description", sel)));
        ui.spacing();
        entries_table(ui, h, view, sel, &mut fr, actions);
        let errors = model::errors(view.baseline, hoppers);
        if !errors.is_empty() {
            ui.separator();
            for e in &errors {
                ui.text_colored(theme::ERROR, format!("{} {e}", icon::ALERT));
            }
        }
        ui.separator();
        ui.text_disabled(
            "To publish to a Live server, save the file and run \
             `h2 live hopper import <file>` on the server's machine.",
        );
        let k = fr.key;
        (fr, k)
    });
    if let Some(fr) = out {
        if fr.seal {
            app.hoppers.seal();
        }
        if let Some(e) = fr.error {
            app.error(e);
        }
    }
}

/// What a row's buttons asked for, applied after the table.
enum RowOp {
    Up(usize),
    Down(usize),
    Copy(usize),
    Remove(usize),
}

fn entries_table(
    ui: &Ui,
    h: &mut EditHopper,
    view: &EditView<'_>,
    sel: usize,
    fr: &mut Frame,
    actions: &mut Vec<Action>,
) {
    let chances = model::chances(h);
    let n = h.records.len();
    let mut op = None;
    let mut weight = None;
    let mut add = false;
    let mut pick: Option<(usize, GameVariant)> = None;
    let flags = TableFlags::ROW_BG | TableFlags::BORDERS_INNER_V | TableFlags::RESIZABLE;
    if let Some(_t) = ui.begin_table_with_flags("##entries", 7, flags) {
        let col = |name: &str, w: Option<f32>| {
            let (flags, width) = match w {
                Some(w) => (TableColumnFlags::NONE, TableColumnWidth::Fixed(w)),
                None => (TableColumnFlags::NONE, TableColumnWidth::Stretch(1.0)),
            };
            ui.table_setup_column(name, flags, Some(width));
        };
        col("#", Some(24.0));
        col("Map", Some(170.0));
        col("Game variant", None);
        col("", Some(40.0));
        col("Weight", Some(70.0));
        col("Chance", Some(56.0));
        col("", Some(140.0));
        ui.table_headers_row();
        for (i, r) in h.records.iter_mut().enumerate() {
            let _id = ui.push_id(i as i32);
            ui.table_next_row();
            ui.table_next_column();
            ui.align_text_to_frame_padding();
            ui.text(format!("{}", i + 1));
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            let before = r.id;
            map_picker(ui, view, r);
            fr.changed(ui, r.id != before, key(("map", sel, i)));
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            if let Some(v) = variant_picker(ui, view, r) {
                pick = Some((i, v));
            }
            ui.table_next_column();
            if ui.small_button("Edit") {
                actions.push(Action::EditEntryVariant(sel, i));
            }
            ui.set_item_tooltip("Edit in the variant tool: opens a copy of this variant there");
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            let mut w = r.dwords[0] as i32;
            let c = dear_imgui_rs::Drag::<i32, &str>::new("##weight")
                .range(1, MAX_WEIGHT as i32)
                .speed(0.2)
                .build(ui, &mut w);
            if c {
                weight = Some((i, w.clamp(1, MAX_WEIGHT as i32) as u32));
            }
            fr.changed(ui, c, key(("weight", sel, i)));
            ui.table_next_column();
            ui.align_text_to_frame_padding();
            ui.text_disabled(format!(
                "{:.1}%",
                100.0 * chances.get(i).copied().unwrap_or(0.0)
            ));
            ui.table_next_column();
            row_buttons(ui, i, n, &mut op);
        }
    }
    let d = ui.begin_disabled_with_cond(n >= MAX_RECORDS);
    if ui.button(format!("{} Add an entry", icon::PLUS)) {
        add = true;
    }
    d.end();
    ui.set_item_tooltip("Copies the last entry");
    if let Some((i, v)) = pick
        && let Err(e) = model::set_variant(&mut h.records[i], &v)
    {
        fr.error = Some(format!("entry {}: {e}", i + 1));
    }
    // The row buttons, weights and Add are the command layer's playlist
    // edits, as the agent API's playlist.entry calls.
    let (playlist, mut edits) = (sel, Vec::new());
    if let Some((entry, weight)) = weight {
        edits.push(PlaylistEdit::Weight {
            playlist,
            entry,
            weight,
        });
    }
    if add {
        edits.push(PlaylistEdit::Add { playlist });
    }
    if let Some(op) = op {
        edits.push(match op {
            RowOp::Up(entry) => PlaylistEdit::Move {
                playlist,
                entry,
                to: entry.saturating_sub(1),
            },
            RowOp::Down(entry) => PlaylistEdit::Move {
                playlist,
                entry,
                to: (entry + 1).min(n.saturating_sub(1)),
            },
            RowOp::Copy(entry) => PlaylistEdit::Duplicate { playlist, entry },
            RowOp::Remove(entry) => PlaylistEdit::Remove { playlist, entry },
        });
    }
    for e in edits {
        if let Err(err) = e.apply_to(h) {
            fr.error = Some(err);
        }
    }
}

fn row_buttons(ui: &Ui, i: usize, n: usize, op: &mut Option<RowOp>) {
    let button = |enabled: bool, f: &dyn Fn() -> bool| -> bool {
        let d = ui.begin_disabled_with_cond(!enabled);
        let c = f();
        d.end();
        c
    };
    if button(i > 0, &|| ui.arrow_button("##up", Direction::Up)) {
        *op = Some(RowOp::Up(i));
    }
    ui.same_line();
    if button(i + 1 < n, &|| ui.arrow_button("##down", Direction::Down)) {
        *op = Some(RowOp::Down(i));
    }
    ui.same_line();
    if button(n < MAX_RECORDS, &|| ui.small_button(icon::COPY)) {
        *op = Some(RowOp::Copy(i));
    }
    ui.set_item_tooltip("Duplicate");
    ui.same_line();
    if button(n > 1, &|| ui.small_button(icon::TRASH)) {
        *op = Some(RowOp::Remove(i));
    }
    ui.set_item_tooltip("Remove");
}

fn map_picker(ui: &Ui, view: &EditView<'_>, r: &mut EditRecord) {
    let Some(c) = view.catalog else {
        ui.text_disabled(format!("map id {}", r.id));
        return;
    };
    let shown = c
        .map_by_id(r.id)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| format!("id {} (not a map in this game)", r.id));
    if let Some(_c) = ui.begin_combo("##map", shown) {
        for m in &c.maps {
            if ui.selectable_config(&m.name).selected(m.id == r.id).build() {
                r.id = m.id;
            }
            if !m.description.is_empty() {
                ui.set_item_tooltip(&m.description);
            }
        }
    }
}

/// The entry's variant as a combo of the picker's choices; returns the one
/// picked.
fn variant_picker(ui: &Ui, view: &EditView<'_>, r: &EditRecord) -> Option<GameVariant> {
    let current = model::entry_variant(view.base, r);
    let mut shown = current
        .as_ref()
        .map(variant_label)
        .unwrap_or_else(|| "-".into());
    if r.picked.is_some() {
        shown.push_str("  (picked)");
    }
    let _combo = ui.begin_combo("##variant", shown)?;
    let Some(c) = view.choices else {
        ui.text_disabled("No variants read yet.");
        return None;
    };
    let mut picked = None;
    for (title, list) in [
        ("Built-in", &c.builtins),
        ("Saved", &c.saved),
        ("Files", &c.files),
    ] {
        if list.is_empty() {
            continue;
        }
        ui.separator_with_text(title);
        for (k, ch) in list.iter().enumerate() {
            if let Some(v) = choice_row(ui, ch, current.as_ref(), title, k) {
                picked = Some(v);
            }
        }
    }
    if c.all().next().is_none() {
        ui.text_disabled("No variants: open a disc, save some, or load a .h2variant file.");
    }
    picked
}

/// One choice: disabled, with the reasons on hover, when the game would
/// reject it.
fn choice_row(
    ui: &Ui,
    ch: &Choice,
    current: Option<&GameVariant>,
    group: &str,
    k: usize,
) -> Option<GameVariant> {
    let d = ui.begin_disabled_with_cond(!ch.usable());
    let clicked = ui
        .selectable_config(format!("{}##{group}-{k}", ch.label()))
        .selected(current == Some(&ch.variant))
        .build();
    d.end();
    if !ch.usable() {
        let why: Vec<String> = ch
            .problems
            .iter()
            .map(|p| format!("+{:#x}: {}", p.offset, p.what))
            .collect();
        ui.set_item_tooltip(format!("The game would reject it:\n{}", why.join("\n")));
    }
    clicked.then(|| ch.variant.clone())
}

/// Presets, variant files and the raw fields of the shown playlist.
pub fn inspector(
    ui: &Ui,
    app: &mut App,
    st: &mut PlaylistsUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    if !app.hoppers.is_open() {
        ui.text_disabled("Open or start a playlist file.");
        return;
    }
    let sel = app.hopper_selected.min(HOPPERS - 1);
    let note = app.hoppers.catalog_note();
    let mut preset = app.hopper_preset;
    let out = app.hoppers.edit_with(|hoppers, view| {
        let mut fr = Frame::default();
        heading(ui, fonts, "Start from a preset");
        let shown = preset
            .and_then(|i| PRESETS.get(i))
            .map(|p| format!("{} - {}", p.label, p.era))
            .unwrap_or_else(|| "Choose a historical playlist...".into());
        ui.set_next_item_width(-1.0);
        if let Some(_c) = ui.begin_combo("##preset", shown) {
            if ui
                .selectable_config("(none)")
                .selected(preset.is_none())
                .build()
            {
                preset = None;
            }
            for (i, p) in PRESETS.iter().enumerate() {
                if ui
                    .selectable_config(format!("{} - {}##preset-{i}", p.label, p.era))
                    .selected(preset == Some(i))
                    .build()
                {
                    preset = Some(i);
                }
            }
        }
        if let Some(note) = &note {
            ui.text_disabled(note);
        }
        if let Some(p) = preset.and_then(|i| PRESETS.get(i)) {
            let r = presets::resolve(p, view.catalog, view.baseline);
            let f = ui.push_font(fonts.bold);
            ui.text_wrapped(&r.title);
            f.pop();
            ui.text_wrapped(&r.description);
            if let Some(u) = p.uncertain {
                ui.text_disabled(format!("From memory: {u}."));
            }
            for g in &r.games {
                match &g.missing {
                    None => ui.bullet_text(&g.label),
                    Some(why) => {
                        ui.text_colored(theme::DIM, format!("  {} - unavailable: {why}", g.label))
                    }
                }
            }
            let d = ui.begin_disabled_with_cond(r.records.is_empty());
            if ui.button(format!(
                "Fill playlist {} with {} entries",
                sel + 1,
                r.records.len()
            )) {
                hoppers[sel] = presets::apply(&hoppers[sel], &r);
                preset = None;
                fr.seal = true;
            }
            d.end();
        }
        ui.separator();
        variant_files(ui, view, st, fonts, dialogs, actions);
        ui.separator();
        if ui.collapsing_header("Advanced: raw fields", TreeNodeFlags::NONE) {
            advanced(ui, &mut hoppers[sel], sel, &mut fr);
        }
        let k = fr.key;
        (fr, k)
    });
    app.hopper_preset = preset;
    if let Some(fr) = out
        && fr.seal
    {
        app.hoppers.seal();
    }
}

fn heading(ui: &Ui, fonts: &Fonts, text: &str) {
    let f = ui.push_font(fonts.bold);
    ui.text(text);
    f.pop();
}

/// Loading `.h2variant` files into the picker and re-reading the saves.
fn variant_files(
    ui: &Ui,
    view: &EditView<'_>,
    st: &mut PlaylistsUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    heading(ui, fonts, "Variants for the entries");
    if let Some(c) = view.choices {
        ui.text_disabled(format!(
            "{} built-in, {} saved, {} from files",
            c.builtins.len(),
            c.saved.len(),
            c.files.len()
        ));
        for n in &c.notes {
            ui.text_wrapped(n);
        }
    }
    ui.set_next_item_width(-1.0);
    ui.input_text("##variant-path", &mut st.variant_path)
        .hint("a .h2variant file")
        .build();
    if ui.button(format!("{} Browse...", icon::FOLDER_OPEN))
        && let Some(p) = super::variants::import_dialog(dialogs)
    {
        st.variant_path = p.display().to_string();
        actions.push(Action::LoadVariantFile(p));
    }
    ui.same_line();
    let d = ui.begin_disabled_with_cond(st.variant_path.trim().is_empty());
    if ui.button("Load") {
        actions.push(Action::LoadVariantFile(PathBuf::from(
            st.variant_path.trim(),
        )));
    }
    d.end();
    ui.same_line();
    if ui.button("Refresh saved") {
        actions.push(Action::RefreshPickerVariants);
    }
}

/// The fields a playlist carries besides its text and entries. The game
/// validates some of them; the help says so.
fn advanced(ui: &Ui, h: &mut EditHopper, sel: usize, fr: &mut Frame) {
    ui.text_colored(
        theme::WARN,
        "The entry string, flags and second dword lie inside the entry's game variant, and \
         the name field overlaps a table the game validates: change them only if you know \
         the bytes.",
    );
    let label_w = 110.0;
    ui.align_text_to_frame_padding();
    ui.text_disabled("Name (ASCII)");
    ui.same_line_with_pos(label_w);
    ui.set_next_item_width(-1.0);
    let c = ui.input_text("##raw-name", &mut h.name).build();
    clip(&mut h.name, MAX_NAME);
    fr.changed(ui, c, key(("name", sel)));
    for (label, value, options) in [
        ("Type", &mut h.hopper_type, &TYPES[..]),
        ("Status", &mut h.status, &STATUSES[..]),
    ] {
        ui.align_text_to_frame_padding();
        ui.text_disabled(label);
        ui.same_line_with_pos(label_w);
        ui.set_next_item_width(-1.0);
        let shown = options
            .iter()
            .find(|(v, _)| v == value)
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| format!("{value} (out of range)"));
        if let Some(_c) = ui.begin_combo(format!("##raw-{label}"), shown) {
            for (v, l) in options {
                if ui.selectable_config(*l).selected(*v == *value).build() {
                    *value = *v;
                    fr.seal = true;
                }
            }
        }
    }
    ui.align_text_to_frame_padding();
    ui.text_disabled("Ranges");
    ui.same_line_with_pos(label_w);
    ui.set_next_item_width(-1.0);
    let mut r = h.ranges.map(|x| x as i32);
    let c = dear_imgui_rs::Drag::<i32, &str>::new("##ranges")
        .range(0, i32::MAX)
        .build_array(ui, &mut r);
    if c {
        h.ranges = r.map(|x| x.max(0) as u32);
    }
    fr.changed(ui, c, key(("ranges", sel)));
    ui.spacing();
    let flags = TableFlags::ROW_BG | TableFlags::BORDERS_INNER_V;
    if let Some(_t) = ui.begin_table_with_flags("##raw-records", 5, flags) {
        for name in ["#", "map id", "string", "flags", "dword 2"] {
            ui.table_setup_column(name, TableColumnFlags::NONE, None);
        }
        ui.table_headers_row();
        for (i, r) in h.records.iter_mut().enumerate() {
            let _id = ui.push_id(i as i32);
            ui.table_next_row();
            ui.table_next_column();
            ui.text(format!("{}", i + 1));
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            let mut id = r.id as i32;
            let c = ui.input_int("##id", &mut id);
            if c {
                r.id = id.max(0) as u32;
            }
            fr.changed(ui, c, key(("raw-id", sel, i)));
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            let c = ui.input_text("##string", &mut r.string).build();
            clip(&mut r.string, MAX_STRING);
            fr.changed(ui, c, key(("raw-string", sel, i)));
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            let mut flags = format!("{:08x}", r.flags);
            let c = ui
                .input_text("##flags", &mut flags)
                .chars_hexadecimal(true)
                .build();
            if c && let Ok(v) = u32::from_str_radix(flags.trim(), 16) {
                r.flags = v;
            }
            fr.changed(ui, c, key(("raw-flags", sel, i)));
            ui.table_next_column();
            ui.set_next_item_width(-1.0);
            let mut d2 = r.dwords[1] as i32;
            let c = ui.input_int("##dword2", &mut d2);
            if c {
                r.dwords[1] = d2 as u32;
            }
            fr.changed(ui, c, key(("raw-dword2", sel, i)));
        }
    }
}

/// The status bar's summary of the open file.
pub fn summary(app: &App) -> String {
    let Some(hoppers) = app.hoppers.hoppers() else {
        return String::new();
    };
    let used = hoppers.iter().filter(|h| !h.records.is_empty()).count();
    let entries: usize = hoppers.iter().map(|h| h.records.len()).sum();
    let errors = app.hoppers.errors().len();
    format!(
        "{} {used} playlists in use   {entries} entries{}",
        icon::LAYERS,
        if errors > 0 {
            format!("   {errors} errors")
        } else {
            String::new()
        }
    )
}
