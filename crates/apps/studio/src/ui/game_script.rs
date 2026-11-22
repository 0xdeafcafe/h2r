//! The game type script window (docs/editor/editor-guide.md, "Game type
//! scripts"): a `.rules` file in a text widget coloured by the language's
//! semantic tokens, with completion as you type, hover, signature help,
//! underlined problems and their quick fixes, rename, go to definition,
//! the uses of the name at the cursor, the check report, and the reload
//! into the paired game.
//!
//! The state and every pure helper live in `app::rules_edit`; this file
//! only draws. The text widget draws its glyphs and its frame transparent
//! (`theme::HIDDEN_TEXT`), and the window draws the frame, the highlights,
//! the coloured text and the underlines on its own draw list, which Dear
//! ImGui renders under the widget's child window. The widget keeps its
//! own cursor, selection and undo, drawn over them.
//!
//! The order matters: a multiline text widget is a child window whose
//! background is `FrameBg`, rendered after its parent's draw list. With
//! an opaque `FrameBg` that background covers anything the parent draws
//! under it, so the text widget's `FrameBg` is cleared here and
//! [`frame`] paints `theme::SCRIPT_FRAME` itself.

use std::path::PathBuf;

use dear_imgui_rs::{
    Condition, FocusedFlags, InputTextCallback, InputTextCallbackHandler, InputTextMultilineFlags,
    Key, MouseButton, StyleColor, TextCallbackData, TreeNodeFlags, Ui, WindowFlags, sys,
};

use agent_proto::rules::ReloadMode;

use crate::app::App;
use crate::app::rules_edit::{self, Edit, Mark, MenuKind, RulesEditor, Span};
use crate::theme::{self, Fonts, icon};

/// The most rows the list at the caret shows before it scrolls.
const MENU_ROWS: usize = 10;

/// The text widget's callback: hands it the editor's pending edit or
/// cursor move, takes the list's selected row on Enter or Tab, keeps the
/// cursor still while Up and Down move the list, and reports the cursor
/// and the last character typed.
struct Caret<'a> {
    want: &'a mut Option<usize>,
    edit: &'a mut Option<Edit>,
    cursor: &'a mut usize,
    /// The edit the list's selected row makes, taken on Enter or Tab.
    accept: Option<Edit>,
    accepted: &'a mut bool,
    /// Where to keep the cursor this frame (Up or Down moved the list).
    hold: Option<usize>,
    /// A character a shortcut used this frame, not to be typed.
    swallow: Option<char>,
    typed: &'a mut Option<char>,
}

impl InputTextCallbackHandler for Caret<'_> {
    fn char_filter(&mut self, c: char) -> Option<char> {
        if (c == '\n' || c == '\t') && self.accept.is_some() && !*self.accepted {
            *self.accepted = true;
            return None;
        }
        if self.swallow == Some(c) {
            self.swallow = None;
            return None;
        }
        *self.typed = Some(c);
        Some(c)
    }

    fn on_always(&mut self, mut data: TextCallbackData<'_>) {
        let accepted = if *self.accepted { self.accept.take() } else { None };
        if let Some(edit) = accepted.or_else(|| self.edit.take()) {
            take(&mut data, &edit);
        } else if let Some(at) = self.hold.take() {
            let at = boundary(data.str(), at);
            data.set_cursor_pos(at);
            data.clear_selection();
        }
        if let Some(want) = self.want.take() {
            let at = boundary(data.str(), want);
            data.set_cursor_pos(at);
            data.clear_selection();
        }
        *self.cursor = data.cursor_pos();
    }
}

/// `at`, clamped to the text and moved back to a character's start.
fn boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Puts an edit into the widget's buffer: the whole text when it
/// differs, then the cursor or the selection.
fn take(data: &mut TextCallbackData<'_>, edit: &Edit) {
    if data.str() != edit.text {
        let len = data.str().len();
        data.remove_chars(0, len);
        data.insert_chars(0, &edit.text);
    }
    match edit.select {
        Some((a, b)) if a < b && b <= edit.text.len() => {
            data.set_selection_start(a);
            data.set_selection_end(b);
            data.set_cursor_pos(b);
        }
        _ => {
            let at = boundary(data.str(), edit.cursor);
            data.set_cursor_pos(at);
            data.clear_selection();
        }
    }
}

/// The window's body.
pub fn window(ui: &Ui, app: &mut App, fonts: &Fonts, dialogs: bool) {
    toolbar(ui, app, dialogs);
    title(ui, fonts, &app.rules);
    rename_row(ui, app);
    ui.separator();
    let mut notes = Vec::new();
    theme::panel_body(ui, "##rules-body", || {
        notes.extend(editor(ui, &mut app.rules));
        below(ui, app);
    });
    for note in notes {
        app.info(note);
    }
}

fn toolbar(ui: &Ui, app: &mut App, dialogs: bool) {
    ui.set_next_item_width(170.0);
    if let Some(_combo) = ui.begin_combo("##rules-new", format!("{} New", icon::PLUS)) {
        if ui.selectable("Blank script") {
            app.rules.new_blank();
        }
        ui.separator();
        for (label, _) in rules_edit::starters() {
            if ui.selectable(&label)
                && let Err(why) = app.rules.open_starter(&label)
            {
                app.error(why);
            }
        }
    }
    ui.same_line();
    if ui.button(format!("{} Open", icon::FOLDER_OPEN))
        && let Some(path) = open_dialog(dialogs)
    {
        open(app, &path);
    }
    ui.same_line();
    if ui.button(format!("{} Save", icon::SAVE)) {
        save(app, dialogs, false);
    }
    ui.same_line();
    if ui.button("Save as") {
        save(app, dialogs, true);
    }
    ui.same_line();
    if ui.button("Format")
        && let Err(why) = app.rules.format()
    {
        app.error(why);
    }
    if ui.is_item_hovered() {
        ui.tooltip_text("Lay the script out the standard way (Shift+Alt+F).");
    }
    ui.same_line();
    if ui.button("Rename")
        && let Err(why) = app.rules.start_rename()
    {
        app.info(why);
    }
    if ui.is_item_hovered() {
        ui.tooltip_text("Rename the name at the cursor and every use of it (F2).");
    }
    ui.same_line();
    if ui.button("Reload") {
        reload(app);
    }
    ui.same_line();
    ui.set_next_item_width(110.0);
    if let Some(_combo) = ui.begin_combo("##rules-mode", mode_label(app.rules.mode)) {
        for mode in [ReloadMode::Auto, ReloadMode::InPlace, ReloadMode::Restart] {
            if ui
                .selectable_config(mode_label(mode))
                .selected(mode == app.rules.mode)
                .build()
            {
                app.rules.mode = mode;
            }
        }
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Auto: in place when the game's state keeps its shape, else restart the game.\nIn place: refuse rather than restart.\nRestart: always restart the map and variant.",
        );
    }
    ui.same_line();
    ui.checkbox("Reload on save", &mut app.rules.reload_on_save);

    // The shortcuts, while the text has the keyboard (the menu's own
    // Cmd+S stands down then).
    let io = ui.io();
    let cmd = io.key_ctrl() || io.key_super();
    if app.rules.typing && ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS) && cmd {
        if ui.is_key_pressed(Key::S) {
            save(app, dialogs, io.key_shift());
        }
        if ui.is_key_pressed(Key::R) {
            reload(app);
        }
    }
}

/// The rename field (F2), over the text while it is open.
fn rename_row(ui: &Ui, app: &mut App) {
    let typing = app.rules.typing;
    let Some(ren) = &mut app.rules.renaming else {
        return;
    };
    ui.text(format!("Rename {} to", ren.old));
    ui.same_line();
    ui.set_next_item_width(220.0);
    if std::mem::take(&mut ren.focus) {
        ui.set_keyboard_focus_here();
    }
    let entered = ui
        .input_text("##rules-rename", &mut ren.name)
        .enter_returns_true(true)
        .build();
    ui.same_line();
    let go = ui.button("Rename##rules-rename-go");
    ui.same_line();
    let cancel = ui.button("Cancel##rules-rename-cancel") || (!typing && ui.is_key_pressed(Key::Escape));
    if cancel {
        app.rules.renaming = None;
        app.rules.want_cursor = Some(app.rules.cursor);
    } else if (entered || go)
        && let Err(why) = app.rules.finish_rename()
    {
        app.error(format!("Not renamed: {why}"));
    }
}

fn title(ui: &Ui, fonts: &Fonts, r: &RulesEditor) {
    {
        let _font = ui.push_font(fonts.bold);
        ui.text(format!("{}{}", r.title, if r.dirty() { " *" } else { "" }));
    }
    if let Some(p) = &r.path {
        ui.same_line();
        ui.text_disabled(p.display().to_string());
    }
}

fn mode_label(m: ReloadMode) -> &'static str {
    match m {
        ReloadMode::Auto => "Auto",
        ReloadMode::InPlace => "In place",
        ReloadMode::Restart => "Restart",
    }
}

fn open(app: &mut App, path: &std::path::Path) {
    match app.rules.open_path(path) {
        Ok(()) => app.info(format!("Opened {}", path.display())),
        Err(why) => app.error(why),
    }
}

/// Saves (as a new file when `as_new` or when it has none), then reloads
/// it into the game when reload on save is on.
fn save(app: &mut App, dialogs: bool, as_new: bool) {
    let to = if as_new || app.rules.path.is_none() {
        match save_dialog(dialogs, &app.rules) {
            Some(p) => Some(p),
            None => return,
        }
    } else {
        None
    };
    match app.rules.save(to.as_deref()) {
        Ok(path) => {
            app.info(format!("Saved {}", path.display()));
            if app.rules.reload_on_save {
                reload(app);
            }
        }
        Err(why) => app.error(why),
    }
}

fn reload(app: &mut App) {
    if let Err(why) = app.rules.queue_reload() {
        app.error(format!("Script not reloaded: {why}"));
    }
}

fn open_dialog(dialogs: bool) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Open a game type script")
        .add_filter("Game type script", &[rules_edit::EXTENSION])
        .pick_file()
}

fn save_dialog(dialogs: bool, r: &RulesEditor) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    let stem = r.title.split(" (").next().unwrap_or("script");
    let name = if stem.ends_with(".rules") {
        stem.to_string()
    } else {
        format!("{}.{}", stem.to_ascii_lowercase().replace(' ', "_"), rules_edit::EXTENSION)
    };
    rfd::FileDialog::new()
        .set_title("Save the game type script")
        .add_filter("Game type script", &[rules_edit::EXTENSION])
        .set_file_name(name)
        .save_file()
}


/// The text widget, what is drawn under it, its shortcuts, its hover and
/// the list at the caret. Returns notes for the status line (no quick
/// fix here, no declaration found).
fn editor(ui: &Ui, r: &mut RulesEditor) -> Vec<String> {
    let mut notes = Vec::new();
    let lh = ui.text_line_height();
    let pad = ui.clone_style().frame_padding();
    let lines = r.text.split('\n').count().max(1);
    let widest = r
        .text
        .split('\n')
        .map(|l| ui.calc_text_size(l)[0])
        .fold(0.0f32, f32::max);
    let width = ui.content_region_avail()[0].max(widest + 2.0 * pad[0] + ui.calc_text_size("MMMM")[0]);
    // One line more than the text, so the widget never scrolls itself:
    // the panel scrolls, and the colouring stays where the glyphs are.
    let height = (lines + 1) as f32 * lh + 2.0 * pad[1];

    // Shortcuts that act before the widget sees the keys.
    let io = ui.io();
    let cmd = io.key_ctrl() || io.key_super();
    let mut swallow = None;
    let mut hold = None;
    let escape = r.typing && ui.is_key_pressed(Key::Escape);
    let keep = escape.then(|| r.text.clone());
    let menu_was_open = r.menu.is_some();
    if r.typing {
        if io.key_ctrl() && ui.is_key_pressed(Key::Space) {
            swallow = Some(' ');
            if r.menu.as_ref().is_some_and(|m| m.kind == MenuKind::Completions) {
                r.menu = None;
            } else {
                r.open_completions(true);
                if r.menu.is_none() {
                    notes.push("Nothing to complete here.".to_string());
                }
            }
        }
        if cmd && ui.is_key_pressed(Key::Period) {
            swallow = Some('.');
            if let Err(why) = r.open_fixes() {
                notes.push(why);
            }
        }
        if r.menu.is_some() {
            if ui.is_key_pressed(Key::UpArrow) {
                r.menu_move(-1);
                hold = Some(r.cursor);
            }
            if ui.is_key_pressed(Key::DownArrow) {
                r.menu_move(1);
                hold = Some(r.cursor);
            }
        }
        if ui.is_key_pressed(Key::F2)
            && let Err(why) = r.start_rename()
        {
            notes.push(why);
        }
        if ui.is_key_pressed(Key::F12)
            && let Err(why) = r.go_to_definition(r.cursor)
        {
            notes.push(why);
        }
        if io.key_shift()
            && io.key_alt()
            && ui.is_key_pressed(Key::F)
            && let Err(why) = r.format()
        {
            notes.push(why);
        }
    }

    let pending = r.want_cursor.is_some() || r.edit.is_some();
    if pending && !r.typing && r.renaming.is_none() {
        ui.set_keyboard_focus_here();
    }
    let before_text = r.text.clone();
    let before_cursor = r.cursor;
    let accept = if r.typing { r.menu_edit() } else { None };
    let mut accepted = false;
    let mut typed = None;
    {
        let _text = ui.push_style_color(StyleColor::Text, theme::HIDDEN_TEXT);
        let _frame = ui.push_style_color(StyleColor::FrameBg, theme::HIDDEN_TEXT);
        let _frame_hovered = ui.push_style_color(StyleColor::FrameBgHovered, theme::HIDDEN_TEXT);
        let _frame_active = ui.push_style_color(StyleColor::FrameBgActive, theme::HIDDEN_TEXT);
        let _caret = ui.push_style_color(StyleColor::InputTextCursor, theme::TEXT);
        let caret = Caret {
            want: &mut r.want_cursor,
            edit: &mut r.edit,
            cursor: &mut r.cursor,
            accept,
            accepted: &mut accepted,
            hold,
            swallow,
            typed: &mut typed,
        };
        ui.input_text_multiline("##rules-text", &mut r.text, [width, height])
            .flags(InputTextMultilineFlags::ALLOW_TAB_INPUT | InputTextMultilineFlags::NO_HORIZONTAL_SCROLL)
            .callback(InputTextCallback::ALWAYS | InputTextCallback::CHAR_FILTER, caret)
            .build();
    }
    r.typing = ui.is_item_active();
    let hovered = ui.is_item_hovered();
    let (min, max) = (ui.item_rect_min(), ui.item_rect_max());
    r.drawn = Some([min[0], min[1], max[0] - min[0], max[1] - min[1]]);
    let origin = [min[0] + pad[0], min[1] + pad[1]];
    r.cursor = r.cursor.min(r.text.len());

    // Escape closes the list, or leaves the text; it never throws the
    // typing away (Dear ImGui's Escape reverts a text widget).
    if let Some(kept) = keep {
        if r.text != kept {
            r.text = kept;
        }
        r.cursor = r.cursor.min(r.text.len());
        if menu_was_open {
            r.menu = None;
            r.want_cursor = Some(r.cursor);
        }
    } else if accepted {
        r.menu = None;
    } else if r.text != before_text || r.cursor != before_cursor || typed.is_some() {
        r.after_typing(typed);
    }

    r.refresh();
    frame(ui, min, max);
    let marks = if r.typing { r.marks().to_vec() } else { Vec::new() };
    highlight(ui, &r.text, &marks, origin, lh);
    let spans = rules_edit::spans(&r.text, &r.analysis.tokens);
    paint(ui, &r.text, &spans, origin, lh);
    underline(ui, r, origin, lh);

    // Keep the cursor's line in view when it moves (the panel scrolls,
    // not the widget).
    if r.typing && r.cursor != before_cursor {
        let line = rules_edit::position_at(&r.text, r.cursor).line as f32;
        let y = origin[1] + line * lh;
        let top = ui.window_pos()[1];
        let bottom = top + ui.window_size()[1];
        if y < top {
            ui.set_scroll_y(ui.scroll_y() - (top - y) - lh);
        } else if y + lh > bottom {
            ui.set_scroll_y(ui.scroll_y() + (y + lh - bottom) + lh);
        }
    }

    // Cmd+click goes to the declaration.
    if hovered && cmd && ui.is_mouse_clicked(MouseButton::Left) {
        let byte = byte_under(ui, &r.text, origin, lh, ui.io().mouse_pos());
        if let Err(why) = r.go_to_definition(byte) {
            notes.push(why);
        }
    }

    if r.menu.is_some() {
        // The list scrolls to its selection when it opens or the selection
        // moves; the wheel scrolls it otherwise.
        let follow = hold.is_some() || !menu_was_open;
        menu(ui, r, origin, lh, follow);
    } else if hovered && !ui.is_mouse_down(MouseButton::Left) {
        let byte = byte_under(ui, &r.text, origin, lh, ui.io().mouse_pos());
        hover(ui, r, byte);
    }
    notes
}

/// The text area's frame, under the widget.
fn frame(ui: &Ui, min: [f32; 2], max: [f32; 2]) {
    let rounding = ui.clone_style().frame_rounding();
    ui.get_window_draw_list()
        .add_rect(min, max, theme::SCRIPT_FRAME)
        .rounding(rounding)
        .filled(true)
        .build();
}

/// A soft box behind each use of the name at the cursor.
fn highlight(ui: &Ui, text: &str, marks: &[Mark], origin: [f32; 2], lh: f32) {
    if marks.len() < 2 {
        return;
    }
    let list = ui.get_window_draw_list();
    let lines: Vec<&str> = text.split('\n').collect();
    for m in marks {
        let Some(line) = lines.get(m.line) else {
            continue;
        };
        if m.end > line.len() || !line.is_char_boundary(m.start) || !line.is_char_boundary(m.end) {
            continue;
        }
        let x0 = origin[0] + ui.calc_text_size(&line[..m.start])[0];
        let x1 = origin[0] + ui.calc_text_size(&line[..m.end])[0];
        let y = origin[1] + m.line as f32 * lh;
        list.add_rect([x0, y], [x1, y + lh], theme::SCRIPT_MARK)
            .filled(true)
            .build();
    }
}

/// Draws the text again in the token colours: each line's runs between
/// tokens in plain text, each token in its kind's colour.
fn paint(ui: &Ui, text: &str, spans: &[Span], origin: [f32; 2], lh: f32) {
    let list = ui.get_window_draw_list();
    let mut by_line: Vec<Vec<&Span>> = vec![Vec::new(); text.split('\n').count()];
    for s in spans {
        if let Some(l) = by_line.get_mut(s.line) {
            l.push(s);
        }
    }
    for (n, line) in text.split('\n').enumerate() {
        let y = origin[1] + n as f32 * lh;
        let mut runs = by_line[n].clone();
        runs.sort_by_key(|s| s.start);
        let mut at = 0;
        let mut draw = |from: usize, to: usize, colour: [f32; 4]| {
            if from >= to || to > line.len() || !line.is_char_boundary(from) || !line.is_char_boundary(to) {
                return;
            }
            let x = origin[0] + ui.calc_text_size(&line[..from])[0];
            list.add_text([x, y], colour, &line[from..to]);
        };
        for s in runs {
            if s.start < at {
                continue;
            }
            draw(at, s.start, theme::TEXT);
            draw(s.start, s.end, theme::token_colour(s.kind));
            at = s.end;
        }
        draw(at, line.len(), theme::TEXT);
    }
}

/// A line under each diagnostic's range, in its severity's colour.
fn underline(ui: &Ui, r: &RulesEditor, origin: [f32; 2], lh: f32) {
    let list = ui.get_window_draw_list();
    let lines: Vec<&str> = r.text.split('\n').collect();
    for d in &r.analysis.diagnostics {
        let Some(line) = lines.get(d.range.start.line as usize) else {
            continue;
        };
        let start = rules_edit::byte_at(line, ::rules::ide::Position { line: 0, character: d.range.start.character });
        let end = if d.range.end.line == d.range.start.line {
            rules_edit::byte_at(line, ::rules::ide::Position { line: 0, character: d.range.end.character })
        } else {
            line.len()
        };
        let x0 = origin[0] + ui.calc_text_size(&line[..start])[0];
        // An empty range still marks a character's width.
        let x1 = if end > start {
            origin[0] + ui.calc_text_size(&line[..end])[0]
        } else {
            x0 + ui.calc_text_size("M")[0]
        };
        let y = origin[1] + (d.range.start.line as f32 + 1.0) * lh - 1.0;
        let colour = theme::severity_colour(rules_edit::severity_name(d.severity));
        list.add_line([x0, y], [x1, y], colour).thickness(1.5).build();
    }
}

/// The byte of the text under a screen point.
fn byte_under(ui: &Ui, text: &str, origin: [f32; 2], lh: f32, at: [f32; 2]) -> usize {
    let line = ((at[1] - origin[1]) / lh).floor().max(0.0) as usize;
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let row = &text[start..end];
    let x = at[0] - origin[0];
    for (i, c) in row.char_indices() {
        if ui.calc_text_size(&row[..i + c.len_utf8()])[0] > x {
            return start + i;
        }
    }
    end
}

/// The screen point under the cursor's line, where the list opens.
fn caret_point(ui: &Ui, r: &RulesEditor, origin: [f32; 2], lh: f32) -> [f32; 2] {
    let at = r.cursor.min(r.text.len());
    let line_start = r.text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = r.text[..at].matches('\n').count();
    let x = origin[0] + ui.calc_text_size(&r.text[line_start..at])[0];
    [x, origin[1] + (line as f32 + 1.0) * lh + 2.0]
}

/// The list at the caret: a window of its own that never takes the
/// keyboard from the text, kept in front. Up and Down move it, Enter or
/// Tab takes the row (`Caret`), a click takes one, Escape closes it.
fn menu(ui: &Ui, r: &mut RulesEditor, origin: [f32; 2], lh: f32, follow: bool) {
    let Some(m) = &r.menu else {
        return;
    };
    let at = caret_point(ui, r, origin, lh);
    let row = ui.text_line_height_with_spacing();
    let gap = ui.calc_text_size("MM")[0];
    let width = m
        .choices
        .iter()
        .map(|c| ui.calc_text_size(&c.label)[0] + gap + ui.calc_text_size(&c.detail)[0])
        .fold(ui.calc_text_size("Nothing to complete")[0], f32::max)
        .min(ui.calc_text_size("M")[0] * 60.0)
        + gap;
    let selected = m.selected;
    let doc = m
        .choices
        .get(selected)
        .map(|c| rules_edit::hover_blocks(&c.doc))
        .unwrap_or_default();
    let title = match m.kind {
        MenuKind::Completions => None,
        MenuKind::Fixes => Some("Quick fixes"),
    };
    let flags = WindowFlags::NO_DECORATION
        | WindowFlags::NO_DOCKING
        | WindowFlags::NO_MOVE
        | WindowFlags::NO_SAVED_SETTINGS
        | WindowFlags::NO_FOCUS_ON_APPEARING
        | WindowFlags::NO_NAV
        | WindowFlags::ALWAYS_AUTO_RESIZE;
    let mut chosen = None;
    ui.window("##rules-menu")
        .flags(flags)
        .position(at, Condition::Always)
        .build(|| {
            // In front of the script window, which a click on the text
            // brings forward.
            unsafe { sys::igBringWindowToDisplayFront(sys::igGetCurrentWindow()) };
            if let Some(title) = title {
                ui.text_disabled(title);
            }
            theme::bounded_list(ui, "##rules-choices", width, row, m.choices.len(), MENU_ROWS, || {
                for (i, c) in m.choices.iter().enumerate() {
                    if ui
                        .selectable_config(format!("{}##choice{i}", c.label))
                        .selected(i == selected)
                        .build()
                    {
                        chosen = Some(i);
                    }
                    if follow && i == selected {
                        ui.set_scroll_here_y(0.5);
                    }
                    if !c.detail.is_empty() {
                        ui.same_line();
                        ui.text_disabled(&c.detail);
                    }
                }
            });
            if let Some(first) = doc.iter().find(|b| !b.code).or(doc.first()) {
                ui.separator();
                let _wrap = ui.push_text_wrap_pos(ui.cursor_pos()[0] + width);
                ui.text_wrapped(&first.text);
            }
        });
    if let Some(i) = chosen {
        r.choose(i);
    }
}

/// The hover at a byte: the problems there, then what the language says
/// the token is.
fn hover(ui: &Ui, r: &mut RulesEditor, byte: usize) {
    let problems: Vec<(String, [f32; 4], bool)> = r
        .diagnostics_at(byte)
        .into_iter()
        .map(|d| {
            let severity = rules_edit::severity_name(d.severity);
            (format!("{severity}: {}", d.message), theme::severity_colour(severity), !d.fixes.is_empty())
        })
        .collect();
    let blocks = r.hover_cached(byte).map(rules_edit::hover_blocks).unwrap_or_default();
    if problems.is_empty() && blocks.is_empty() {
        return;
    }
    ui.tooltip(|| {
        let _wrap = ui.push_text_wrap_pos(ui.calc_text_size("M")[0] * 40.0);
        for (text, colour, _) in &problems {
            ui.text_colored(*colour, text);
        }
        if problems.iter().any(|p| p.2) {
            ui.text_disabled("Cmd+. for quick fixes");
        }
        if !problems.is_empty() && !blocks.is_empty() {
            ui.separator();
        }
        for b in &blocks {
            if b.code {
                ui.text_colored(theme::token_colour(::rules::ide::TokenKind::Function), &b.text);
            } else {
                ui.text_wrapped(&b.text);
            }
        }
    });
}

/// Under the text: the signature, the problems, the check report and the
/// reload.
fn below(ui: &Ui, app: &mut App) {
    if let Some(sig) = app.rules.typing.then(|| app.rules.signature()).flatten() {
        ui.text_colored(theme::DIM, sig);
    }
    problems(ui, &mut app.rules);
    report(ui, &app.rules);
    if let Some((tone, line)) = app.rules.reload_line() {
        ui.text_colored(theme::reload_colour(tone), line);
    }
}

/// Every error and warning, errors first: a click puts the cursor on it,
/// and each fix the language offers is a button on its row.
fn problems(ui: &Ui, r: &mut RulesEditor) {
    let rows = rules_edit::problems(&r.text, &r.analysis.diagnostics);
    theme::section(ui, format!("Problems ({})", rows.len()));
    if rows.is_empty() {
        ui.text_colored(theme::OK, "No problems.");
        return;
    }
    let mut fix = None;
    for (i, p) in rows.iter().enumerate() {
        for (k, f) in p.fixes.iter().take(2).enumerate() {
            if ui.small_button(format!("{}##fix{i}_{k}", f.title)) {
                fix = Some(f.clone());
            }
            ui.same_line();
        }
        let colour = theme::severity_colour(p.severity);
        let clicked = {
            let _c = ui.push_style_color(StyleColor::Text, colour);
            ui.selectable(format!("{}:{}  {}  {}##p{i}", p.line, p.col, p.severity, p.message))
        };
        if ui.is_item_hovered() && !p.code.is_empty() {
            ui.tooltip_text(p.code);
        }
        if clicked {
            r.want_cursor = Some(p.at);
            r.cursor = p.at;
        }
    }
    if let Some(f) = fix {
        r.apply_fix(&f);
    }
}

fn report(ui: &Ui, r: &RulesEditor) {
    let Some(check) = &r.analysis.check else {
        return;
    };
    if !ui.collapsing_header("Check", TreeNodeFlags::DEFAULT_OPEN) {
        return;
    }
    match (&check.name, check.version) {
        (Some(name), Some(v)) => ui.text(format!("{name}, version {v}")),
        (Some(name), None) => ui.text(name),
        _ => {}
    }
    if check.ok {
        ui.text_colored(theme::OK, "Builds, verifies and fits the budget.");
    }
    for e in check.errors.iter().filter(|e| e.line == 0) {
        ui.text_colored(theme::ERROR, &e.message);
    }
    let Some(rep) = &check.report else {
        return;
    };
    let colour = if rep.fits { theme::TEXT } else { theme::ERROR };
    ui.text_colored(
        colour,
        format!(
            "Worst tick: {} of {} ops{}",
            rep.worst_tick_ops,
            rep.budget,
            rep.costliest
                .as_ref()
                .map_or(String::new(), |h| format!(" (most in {h})"))
        ),
    );
    for h in &rep.handlers {
        ui.text_disabled(format!("  on {}: {} ops", h.event, h.worst_ops));
    }
    let slots: Vec<String> = rep
        .slots
        .iter()
        .map(|s| format!("{} {}/{}", s.scope, s.used, s.limit))
        .collect();
    ui.text(format!("Variables: {}", slots.join(", ")));
    ui.text(format!("Size: {} of {} bytes", rep.ir_bytes, rep.ir_limit));
    if rep.capabilities.is_empty() {
        ui.text_disabled("Requires no capabilities.");
    }
    for c in &rep.capabilities {
        match (c.known, c.grantable) {
            (false, _) => ui.text_colored(theme::ERROR, format!("requires {}: not in this API", c.name)),
            (true, false) => ui.text_colored(theme::WARN, format!("requires {}: {} (not granted)", c.name, c.words)),
            (true, true) => ui.text(format!("requires {}: {}", c.name, c.words)),
        }
    }
    if !rep.unhostable.is_empty() {
        ui.text_colored(
            theme::WARN,
            format!("No host runs these yet: {}", rep.unhostable.join(", ")),
        );
    }
    if let Some(hash) = &check.hash {
        ui.text_disabled(format!("SHA-256 {hash}"));
    }
}
