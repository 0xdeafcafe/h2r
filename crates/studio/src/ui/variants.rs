//! The game variant tool's windows: the saved and built-in variants (the
//! outliner's place), the open variant's settings pages (the centre) and
//! its actions, findings and changes from the defaults (the inspector).
//! Everything goes back to [`App`] as [`Action`]s.

use std::path::PathBuf;

use dear_imgui_rs::{TableColumnFlags, TableColumnWidth, TableFlags, TreeNodeFlags, Ui};
use variants::ext::{self, Ext};
use variants::panel::{Origin, strip_icon};
use variants::record::{self, Kind};
use variants::{GameType, GameVariant, SettingId, VariantEditor};

use crate::app::{Action, App};
use crate::theme::{self, Fonts, icon};

/// The variant tool's own UI state.
pub struct VariantsUi {
    /// The game type "New" makes.
    pub new_type: GameType,
    /// The name a copy of the open built-in gets.
    copy_name: String,
    /// The origin `copy_name` was filled for.
    copy_for: Option<Origin>,
    /// Pasted portable variant text.
    import_text: String,
    /// Open only the settings pages whose title contains this, once (the
    /// headless screenshots' `--page`).
    pub only_page: Option<String>,
}

impl Default for VariantsUi {
    fn default() -> Self {
        Self {
            new_type: GameType::Slayer,
            copy_name: String::new(),
            copy_for: None,
            import_text: String::new(),
            only_page: None,
        }
    }
}

fn label_of(v: &GameVariant) -> String {
    format!("{}  ({})", strip_icon(&v.name()), v.game_type().label())
}

/// The list: New, then the saves, then the built-ins.
pub fn list(ui: &Ui, app: &App, st: &mut VariantsUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    let Some(e) = app.variants.as_ref() else {
        ui.text_disabled("Loading the variant settings...");
        return;
    };
    ui.set_next_item_width(140.0);
    if let Some(_c) = ui.begin_combo("##new-type", st.new_type.label()) {
        for t in GameType::ALL {
            if ui
                .selectable_config(t.label())
                .selected(t == st.new_type)
                .build()
            {
                st.new_type = t;
            }
        }
    }
    ui.same_line();
    if ui.button(format!("{} New", icon::PLUS)) {
        actions.push(Action::NewVariant(st.new_type));
    }
    ui.set_item_tooltip("A new variant of this game type with its defaults");
    ui.separator();
    let open = e.origin().cloned();
    ui.child_window("##variant-list").build(ui, || {
        heading(ui, fonts, "Saved");
        if e.saved().is_empty() {
            ui.text_disabled("No saved variants.");
        }
        for (i, s) in e.saved().iter().enumerate() {
            let origin = Origin::Saved(s.dir.clone());
            let on = open.as_ref() == Some(&origin);
            let text = format!(
                "{}  {}##saved-{i}",
                icon::FILE,
                label_of(&s.contents.variant)
            );
            if ui.selectable_config(text).selected(on).build() && !on {
                actions.push(Action::OpenVariant(origin));
            }
            ui.set_item_tooltip(&s.save_name);
        }
        ui.spacing();
        heading(ui, fonts, "Built-in");
        if e.builtins().is_empty() {
            ui.text_disabled("None (no disc).");
        }
        for (i, v) in e.builtins().iter().enumerate() {
            let origin = Origin::Builtin(i);
            let on = open.as_ref() == Some(&origin);
            let text = format!("{}  {}##builtin-{i}", icon::DISC, label_of(v));
            if ui.selectable_config(text).selected(on).build() && !on {
                actions.push(Action::OpenVariant(origin));
            }
            ui.set_item_tooltip("Built-in: read-only; copy it to edit");
        }
    });
}

fn heading(ui: &Ui, fonts: &Fonts, text: &str) {
    let f = ui.push_font(fonts.bold);
    ui.text(text);
    f.pop();
}

/// The open variant: its name, then each settings page.
pub fn settings(ui: &Ui, app: &App, st: &mut VariantsUi, fonts: &Fonts, actions: &mut Vec<Action>) {
    let Some(e) = app.variants.as_ref() else {
        return;
    };
    let Some(v) = e.current() else {
        ui.text_disabled("Pick a variant on the left, or make a new one.");
        return;
    };
    let read_only = e.is_read_only();
    let ty = v.game_type();

    let f = ui.push_font(fonts.bold);
    ui.text(format!("{}  {}", icon::SLIDERS, strip_icon(&v.name())));
    f.pop();
    ui.same_line();
    ui.text_disabled(format!(
        "{}{}",
        ty.label(),
        if e.is_dirty() { "   unsaved" } else { "" }
    ));

    if read_only {
        if st.copy_for.as_ref() != e.origin() {
            st.copy_name = e.suggested_copy_name().to_owned();
            st.copy_for = e.origin().cloned();
        }
        ui.text_colored(
            theme::WARN,
            format!("{} Built-in variant: copy it to edit.", icon::INFO),
        );
        ui.set_next_item_width(260.0);
        ui.input_text("##copy-name", &mut st.copy_name)
            .hint("Name of the copy")
            .build();
        clip_name(&mut st.copy_name);
        ui.same_line();
        if ui.button(format!("{} Copy", icon::COPY)) {
            actions.push(Action::CopyBuiltin(st.copy_name.clone()));
        }
    } else {
        let mut name = v.name();
        ui.align_text_to_frame_padding();
        ui.text_disabled("Name");
        ui.same_line_with_pos(110.0);
        ui.set_next_item_width(-1.0);
        if ui.input_text("##variant-name", &mut name).build() {
            clip_name(&mut name);
            actions.push(Action::SetVariantName(name));
        }
    }
    ui.separator();

    let defaults = e.schema().new_variant(ty);
    let disabled = ui.begin_disabled_with_cond(read_only);
    let only = st.only_page.take();
    ui.child_window("##variant-pages").build(ui, || {
        for page in e.schema().pages_for(ty) {
            if let Some(only) = &only {
                ui.set_next_item_open(page.title.contains(only.as_str()));
            }
            if !ui.collapsing_header(
                format!("{}##page-{}", page.title, page.id),
                TreeNodeFlags::DEFAULT_OPEN,
            ) {
                continue;
            }
            let flags = TableFlags::ROW_BG | TableFlags::BORDERS_INNER_V;
            let Some(_t) = ui.begin_table_with_flags(format!("##page-table-{}", page.id), 2, flags)
            else {
                continue;
            };
            ui.table_setup_column(
                "Setting",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Fixed(230.0)),
            );
            ui.table_setup_column(
                "Value",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Stretch(1.0)),
            );
            for id in &page.settings {
                ui.table_next_row();
                ui.table_next_column();
                if let Some(x) = setting_row(ui, e, *id, v, &defaults, fonts) {
                    actions.push(Action::SetSetting(*id, x));
                }
            }
        }
        if let Some(a) = more_options(ui, v) {
            actions.push(a);
        }
    });
    disabled.end();
}

/// Our options the record has no field for, in its extension block
/// (docs/game/game-options.md section 3). Only the ones the game applies
/// are shown (gravity, damage, shield and health, unlimited grenades);
/// the rest arrive with their slices. Team count (setting 93) is a record field with no retail row,
/// shown here too. Returns the action for what the user changed.
fn more_options(ui: &Ui, v: &GameVariant) -> Option<Action> {
    if !ui.collapsing_header("More options (h2recomp)##variant-ext", TreeNodeFlags::NONE) {
        return None;
    }
    if let ext::Block::Ignored(why) = v.ext_state() {
        ui.text_colored(
            theme::WARN,
            format!(
                "{} The stored options are ignored ({why}); editing replaces them.",
                icon::INFO
            ),
        );
    }
    let now = v.ext();
    let mut next = now;
    let flags = TableFlags::ROW_BG | TableFlags::BORDERS_INNER_V;
    let _t = ui.begin_table_with_flags("##variant-ext-table", 2, flags)?;
    ui.table_setup_column(
        "Setting",
        TableColumnFlags::NONE,
        Some(TableColumnWidth::Fixed(230.0)),
    );
    ui.table_setup_column(
        "Value",
        TableColumnFlags::NONE,
        Some(TableColumnWidth::Stretch(1.0)),
    );
    ui.table_next_row();
    ui.table_next_column();
    ui.align_text_to_frame_padding();
    ui.text("Gravity (%)");
    ui.set_item_tooltip(
        "Gravity for everyone, as a percent of the game's. The Low Gravity \
         skull halves it again. Applied by the host and on Live (not on \
         System Link, where every machine plays the game's own).",
    );
    ui.table_next_column();
    ui.set_next_item_width(-1.0);
    let mut g = i32::from(now.gravity);
    let (lo, hi) = (*ext::GRAVITY.start(), *ext::GRAVITY.end());
    if ui.slider_i32("##ext-gravity", &mut g, i32::from(lo), i32::from(hi)) {
        next.gravity = g.clamp(lo.into(), hi.into()) as u16;
    }
    let (lo, hi) = (*ext::PERCENT.start(), *ext::PERCENT.end());
    for (label, id, tip, value) in [
        (
            "Damage (%)",
            "##ext-damage",
            "Damage every player deals, as a percent of the game's.",
            &mut next.damage,
        ),
        (
            "Shields (%)",
            "##ext-shield",
            "How much every player's shield holds, as a percent of the game's.",
            &mut next.shield,
        ),
        (
            "Health (%)",
            "##ext-health",
            "How much every player's body holds, as a percent of the game's.",
            &mut next.health,
        ),
    ] {
        ui.table_next_row();
        ui.table_next_column();
        ui.align_text_to_frame_padding();
        ui.text(label);
        ui.set_item_tooltip(format!(
            "{tip} Players only (vehicles keep theirs). Applied by the host and \
             its guests on Live (not on System Link)."
        ));
        ui.table_next_column();
        ui.set_next_item_width(-1.0);
        let mut x = i32::from(*value);
        if ui.slider_i32(id, &mut x, i32::from(lo), i32::from(hi)) {
            *value = x.clamp(lo.into(), hi.into()) as u16;
        }
    }
    ui.table_next_row();
    ui.table_next_column();
    ui.align_text_to_frame_padding();
    ui.text("Unlimited grenades");
    ui.set_item_tooltip(
        "Grenades never run out: everyone keeps at least four of each. \
         The host tops them up (not on System Link).",
    );
    ui.table_next_column();
    let mut on = now.unlimited_grenades;
    if ui.checkbox("##ext-grenades", &mut on) {
        next.unlimited_grenades = on;
    }
    let teams = team_count_row(ui, v);
    if next != now {
        return Some(Action::SetVariantExt(next));
    }
    teams.map(|t| Action::SetSetting(record::TEAM_COUNT, t))
}

/// Team count (setting 93, `+0xb4`): how many teams the game makes,
/// capped by the map's; 0 is the map's own (docs/game/game-options.md,
/// "Team count"). The new value when changed.
fn team_count_row(ui: &Ui, v: &GameVariant) -> Option<i32> {
    ui.table_next_row();
    ui.table_next_column();
    ui.align_text_to_frame_padding();
    ui.text("Team count");
    ui.set_item_tooltip(
        "How many teams a team game makes, at most the map's. The game's \
         own editor has no row for it. Played by every build.",
    );
    ui.table_next_column();
    ui.set_next_item_width(-1.0);
    let now = v.get(record::TEAM_COUNT).unwrap_or(0);
    let label = |t: i32| match t {
        0 => "Map default".to_string(),
        t => t.to_string(),
    };
    let mut out = None;
    if let Some(_c) = ui.begin_combo("##team-count", label(now)) {
        for t in record::TEAM_COUNTS {
            if ui.selectable_config(label(t)).selected(t == now).build() && t != now {
                out = Some(t);
            }
        }
    }
    out
}

/// Keeps a typed name within the record's limit.
fn clip_name(name: &mut String) {
    if name.encode_utf16().count() > record::MAX_NAME_UNITS {
        let mut out = String::new();
        for c in name.chars() {
            if out.encode_utf16().count() + c.len_utf16() > record::MAX_NAME_UNITS {
                break;
            }
            out.push(c);
        }
        *name = out;
    }
}

/// One setting: its title (bold when changed from the default, help on
/// hover) and its widget. Returns a new value when the user changed it.
fn setting_row(
    ui: &Ui,
    e: &VariantEditor,
    id: SettingId,
    v: &GameVariant,
    defaults: &GameVariant,
    fonts: &Fonts,
) -> Option<i32> {
    let value = v.get(id)?;
    let schema = e.schema();
    let def = schema.settings.get(&id);
    let changed = defaults.get(id) != Some(value);
    ui.align_text_to_frame_padding();
    if changed {
        let f = ui.push_font(fonts.bold);
        ui.text(schema.title(id));
        f.pop();
    } else {
        ui.text(schema.title(id));
    }
    let help = def
        .map(|d| d.help.as_str())
        .filter(|h| !h.is_empty())
        .unwrap_or("No help text.");
    let default_text = defaults
        .get(id)
        .map(|d| e.value_text(id, d))
        .unwrap_or_default();
    ui.set_item_tooltip(format!("{help}\n\nDefault: {default_text}"));
    ui.table_next_column();
    ui.set_next_item_width(-1.0);
    let wid = format!("##setting-{}", id.0);
    match def.filter(|d| !d.choices.is_empty()) {
        Some(d) => {
            let shown = if d.choices.iter().any(|c| c.value == value) {
                e.value_text(id, value)
            } else {
                format!("{value} (not offered)")
            };
            let mut out = None;
            if let Some(_c) = ui.begin_combo(&wid, shown) {
                for c in &d.choices {
                    // Ours: the game plays it; its own editor shows it too
                    // under h2recomp (docs/game/game-options.md).
                    let label = if c.ours {
                        format!("{}  (h2recomp)", c.label)
                    } else {
                        c.label.clone()
                    };
                    if ui
                        .selectable_config(&label)
                        .selected(c.value == value)
                        .build()
                        && c.value != value
                    {
                        out = Some(c.value);
                    }
                }
            }
            out
        }
        None => {
            let kind = record::field(id)?.kind;
            if matches!(kind, Kind::Bit16(_) | Kind::Bit32(_) | Kind::Bool8) {
                let mut on = value != 0;
                ui.checkbox(&wid, &mut on).then_some(i32::from(on))
            } else {
                let (lo, hi) = match kind {
                    Kind::U8 => (0, 255),
                    Kind::I16 => (i16::MIN.into(), i16::MAX.into()),
                    _ => (i32::MIN, i32::MAX),
                };
                let mut x = value;
                (ui.input_int(&wid, &mut x) && x.clamp(lo, hi) != value).then_some(x.clamp(lo, hi))
            }
        }
    }
}

/// The open variant's actions, what the game would reject, and the
/// changes from the type's defaults; importing.
pub fn inspector(
    ui: &Ui,
    app: &App,
    st: &mut VariantsUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let Some(e) = app.variants.as_ref() else {
        return;
    };
    let current = e.current();
    let saved = matches!(e.origin(), Some(Origin::Saved(_)));
    let can_save = e.is_dirty() && !e.is_read_only();
    let button = |label: String, enabled: bool, tip: &str| -> bool {
        let d = ui.begin_disabled_with_cond(!enabled);
        let clicked = ui.button(label);
        d.end();
        ui.set_item_tooltip(tip);
        clicked
    };
    if button(
        format!("{} Save", icon::SAVE),
        can_save,
        "Write it to the game's save storage (Cmd+S)",
    ) {
        actions.push(Action::SaveVariant);
    }
    ui.same_line();
    if button(
        format!("{} Revert", icon::UNDO),
        e.has_changes(),
        "Back to the last save",
    ) {
        actions.push(Action::RevertVariant);
    }
    ui.same_line();
    if button(
        format!("{} Delete", icon::TRASH),
        saved,
        "Delete this saved variant",
    ) {
        actions.push(Action::DeleteVariant);
    }
    if button(
        format!("{} Export...", icon::PACKAGE),
        current.is_some(),
        "Write it as a portable .h2variant file to share",
    ) && let Some(p) = export_dialog(dialogs, current)
    {
        actions.push(Action::ExportVariant(p));
    }
    ui.same_line();
    if ui.button(format!("{} Import...", icon::PACKAGE_OPEN))
        && let Some(p) = import_dialog(dialogs)
    {
        actions.push(Action::ImportVariantFile(p));
    }
    ui.set_item_tooltip("Open a .h2variant file as a new, unsaved variant");
    ui.separator();

    if let Some(v) = current {
        let issues = e.issues();
        if issues.is_empty() {
            ui.text_colored(
                theme::DIM,
                format!("{} The game accepts this variant.", icon::CHECK),
            );
        } else {
            ui.text_colored(
                theme::ERROR,
                format!(
                    "{} The game would not accept this ({})",
                    icon::ALERT,
                    issues.len()
                ),
            );
            for i in &issues {
                ui.bullet_text(i.to_string());
            }
        }
        ui.spacing();
        let diff = e.schema().diff_from_defaults(v);
        heading(
            ui,
            fonts,
            &format!(
                "Changes from the {} defaults ({})",
                v.game_type().label(),
                diff.len()
            ),
        );
        if diff.is_empty() {
            ui.text_disabled("None: the type's defaults.");
        }
        for (id, was, now) in &diff {
            ui.text_wrapped(format!(
                "{}: {} -> {}",
                e.schema().title(*id),
                e.value_text(*id, *was),
                e.value_text(*id, *now)
            ));
        }
        ui.separator();
    }

    if ui.collapsing_header("Paste a portable variant", TreeNodeFlags::NONE) {
        ui.input_text_multiline("##import-text", &mut st.import_text, [-1.0, 90.0])
            .build();
        let d = ui.begin_disabled_with_cond(st.import_text.trim().is_empty());
        if ui.button("Import the pasted text") {
            actions.push(Action::ImportVariantText(std::mem::take(
                &mut st.import_text,
            )));
        }
        d.end();
    }
    ui.spacing();
    ui.text_disabled(format!("Schema: {}", e.schema().source));
}

fn export_dialog(dialogs: bool, v: Option<&GameVariant>) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    let name = v
        .map(|v| strip_icon(&v.name()))
        .unwrap_or_else(|| "variant".into());
    rfd::FileDialog::new()
        .set_title("Export a game variant")
        .add_filter("Game variant", &[variants::portable::EXTENSION])
        .set_file_name(format!("{name}.{}", variants::portable::EXTENSION))
        .save_file()
}

pub fn import_dialog(dialogs: bool) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Open a game variant")
        .add_filter("Game variant", &[variants::portable::EXTENSION])
        .pick_file()
}
