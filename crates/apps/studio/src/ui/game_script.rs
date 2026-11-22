//! The game type script window (docs/editor/editor-guide.md, "Game type
//! scripts"): a `.rules` file in a text widget coloured by the language's
//! semantic tokens, its problems, completions, hover and signature help,
//! its check report, and the reload into the paired game.
//!
//! The state and every pure helper live in `app::rules_edit`; this file
//! only draws. The text widget draws its glyphs transparent
//! (`theme::HIDDEN_TEXT`) and the colouring draws them again over it, so
//! the widget keeps its own cursor, selection and undo.

use std::path::PathBuf;

use dear_imgui_rs::{
    FocusedFlags, InputTextCallback, InputTextCallbackHandler, InputTextMultilineFlags, Key,
    MouseButton, StyleColor, TextCallbackData, TreeNodeFlags, Ui,
};

use agent_proto::rules::ReloadMode;

use crate::app::App;
use crate::app::rules_edit::{self, RulesEditor, Span};
use crate::theme::{self, Fonts, icon};

/// The most completions the list shows.
const COMPLETIONS: usize = 40;

/// Applies the editor's pending cursor move or completion while the widget
/// is active, and reports where the cursor is.
struct Caret<'a> {
    want: &'a mut Option<usize>,
    insert: &'a mut Option<(usize, String)>,
    cursor: &'a mut usize,
}

impl InputTextCallbackHandler for Caret<'_> {
    fn on_always(&mut self, mut data: TextCallbackData<'_>) {
        if let Some((from, text)) = self.insert.take() {
            let at = data.cursor_pos();
            let from = from.min(at);
            if data.str().is_char_boundary(from) {
                data.remove_chars(from, at - from);
                data.insert_chars(from, &text);
                data.clear_selection();
            }
        }
        if let Some(want) = self.want.take() {
            let text = data.str();
            let mut at = want.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            data.set_cursor_pos(at);
            data.clear_selection();
        }
        *self.cursor = data.cursor_pos();
    }
}

/// The window's body.
pub fn window(ui: &Ui, app: &mut App, fonts: &Fonts, dialogs: bool) {
    toolbar(ui, app, dialogs);
    title(ui, fonts, &app.rules);
    ui.separator();
    theme::panel_body(ui, "##rules-body", || {
        editor(ui, &mut app.rules);
        below(ui, app);
    });
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

/// The text widget and its colouring.
fn editor(ui: &Ui, r: &mut RulesEditor) {
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

    let pending = r.want_cursor.is_some() || r.insert.is_some();
    if pending && !r.typing {
        ui.set_keyboard_focus_here();
    }
    let before = r.cursor;
    {
        let _text = ui.push_style_color(StyleColor::Text, theme::HIDDEN_TEXT);
        let _caret = ui.push_style_color(StyleColor::InputTextCursor, theme::TEXT);
        let caret = Caret {
            want: &mut r.want_cursor,
            insert: &mut r.insert,
            cursor: &mut r.cursor,
        };
        ui.input_text_multiline("##rules-text", &mut r.text, [width, height])
            .flags(InputTextMultilineFlags::ALLOW_TAB_INPUT | InputTextMultilineFlags::NO_HORIZONTAL_SCROLL)
            .callback(InputTextCallback::ALWAYS, caret)
            .build();
    }
    r.typing = ui.is_item_active();
    let hovered = ui.is_item_hovered();
    let origin = {
        let m = ui.item_rect_min();
        [m[0] + pad[0], m[1] + pad[1]]
    };
    r.cursor = r.cursor.min(r.text.len());

    r.refresh();
    let spans = rules_edit::spans(&r.text, &r.analysis.tokens);
    paint(ui, &r.text, &spans, origin, lh);
    underline(ui, r, origin, lh);

    // Keep the cursor's line in view when it moves (the panel scrolls,
    // not the widget).
    if r.typing && r.cursor != before {
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

    // Completions: Ctrl+Space opens them, Escape closes them.
    if r.typing && ui.io().key_ctrl() && ui.is_key_pressed(Key::Space) {
        r.completing = !r.completing;
    }
    if r.completing && ui.is_key_pressed(Key::Escape) {
        r.completing = false;
    }

    if hovered && !ui.is_mouse_down(MouseButton::Left) {
        let byte = byte_under(ui, &r.text, origin, lh, ui.io().mouse_pos());
        if let Some(text) = r.hover_cached(byte) {
            ui.tooltip_text(text);
        }
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
            if from >= to || !line.is_char_boundary(from) || !line.is_char_boundary(to) {
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

/// Under the text: the signature, the completions, the problems, the
/// check report and the reload.
fn below(ui: &Ui, app: &mut App) {
    if let Some(sig) = app.rules.typing.then(|| app.rules.signature()).flatten() {
        ui.text_colored(theme::DIM, sig);
    }
    if app.rules.completing {
        completions(ui, &mut app.rules);
    }
    problems(ui, &mut app.rules);
    report(ui, &app.rules);
    if let Some((tone, line)) = app.rules.reload_line() {
        ui.text_colored(theme::reload_colour(tone), line);
    }
}

fn completions(ui: &Ui, r: &mut RulesEditor) {
    let items = r.completions();
    theme::section(ui, "Completions");
    if items.is_empty() {
        ui.text_disabled("Nothing to complete here.");
        return;
    }
    let row = ui.text_line_height_with_spacing();
    let mut chosen = None;
    theme::bounded_list(ui, "##rules-completions", 0.0, row, items.len(), 10, || {
        for (i, item) in items.iter().take(COMPLETIONS).enumerate() {
            if ui.selectable(format!("{}##c{i}", item.label)) {
                chosen = Some(i);
            }
            if !item.detail.is_empty() {
                ui.same_line();
                ui.text_disabled(&item.detail);
            }
        }
    });
    if let Some(i) = chosen {
        r.complete_with(&items[i]);
        r.completing = false;
    }
}

fn problems(ui: &Ui, r: &mut RulesEditor) {
    let rows = rules_edit::problems(&r.text, &r.analysis.diagnostics);
    theme::section(ui, format!("Problems ({})", rows.len()));
    if rows.is_empty() {
        ui.text_colored(theme::OK, "No problems.");
        return;
    }
    for (i, p) in rows.iter().enumerate() {
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
        }
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
