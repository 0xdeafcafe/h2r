//! The Dear ImGui shell: main menu bar, a dockspace with the default
//! layout (tools and maps left over the outliner, the viewport in the
//! middle, the inspector right, the log at the bottom), a status bar, and
//! the modals (unsaved changes, packages). Everything it does goes back to
//! [`App`] as [`Action`]s.
use std::path::PathBuf;

use dear_imgui_rs::{
    Condition, DockLayout, DockLayoutApply, DockSplit, Drag, Key, StyleColor, StyleVar, Ui,
    WindowFlags, WindowKey,
};
use editor_core::Status;
use scenario_edit::layout::{FieldDef, FieldType, Role};
use scenario_edit::{BlockKind, Command, Scenario, TagRef, Value};

use crate::app::{Action, Answer, App, ToolKind};
use crate::theme::{self, Fonts, icon};
use crate::viewport::{GizmoOp, Gpu, ViewportView};

/// The dockable windows, by stable identity.
struct Keys {
    tools: WindowKey,
    outliner: WindowKey,
    viewport: WindowKey,
    inspector: WindowKey,
    log: WindowKey,
}

impl Keys {
    fn new() -> Self {
        let key = |id: &str, title: String| WindowKey::new(id, title).expect("a valid window key");
        Self {
            tools: key("tools", format!("{} Tools", icon::LAYERS)),
            outliner: key("outliner", format!("{} Outliner", icon::TREE)),
            viewport: key("viewport", format!("{} Viewport", icon::EYE)),
            inspector: key("inspector", format!("{} Inspector", icon::SLIDERS)),
            log: key("log", format!("{} Log", icon::LOG)),
        }
    }

    /// The default layout: left column (tools over outliner), right
    /// inspector, log under the viewport.
    fn layout(&self) -> DockLayout {
        DockLayout::split(
            DockSplit::Left,
            0.22,
            DockLayout::split(
                DockSplit::Up,
                0.38,
                DockLayout::tabs([&self.tools]),
                DockLayout::tabs([&self.outliner]),
            ),
            DockLayout::split(
                DockSplit::Right,
                0.28,
                DockLayout::tabs([&self.inspector]),
                DockLayout::split(
                    DockSplit::Down,
                    0.22,
                    DockLayout::tabs([&self.log]),
                    DockLayout::tabs([&self.viewport]),
                ),
            ),
        )
    }
}

const UNSAVED: &str = "Unsaved changes###unsaved";
const SAVE_PACKAGE: &str = "Save as package###save-package";
const OPEN_PACKAGE: &str = "Open package###open-package";

/// The studio UI.
pub struct Studio {
    keys: Keys,
    layout: DockLayout,
    fonts: Fonts,
    pub viewport: ViewportView,
    show_open_package: bool,
    reset_layout: bool,
    /// Log lines already seen (to scroll to new ones).
    log_seen: usize,
    /// Native file dialogs. Off in headless renders and tests.
    pub dialogs: bool,
}

impl Studio {
    pub fn new(fonts: Fonts) -> Self {
        let keys = Keys::new();
        let layout = keys.layout();
        Self {
            keys,
            layout,
            fonts,
            viewport: ViewportView::new(),
            show_open_package: false,
            reset_layout: false,
            log_seen: 0,
            dialogs: true,
        }
    }

    /// Build one frame of UI and run the actions it produced.
    pub fn frame(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>) {
        let mut actions = Vec::new();
        self.shortcuts(ui, app, &mut actions);
        self.menu_bar(ui, app, &mut actions);
        self.dockspace(ui);
        self.tools_window(ui, app, &mut actions);
        self.outliner_window(ui, app, &mut actions);
        self.viewport_window(ui, app, gpu, &mut actions);
        self.inspector_window(ui, app);
        self.log_window(ui, app);
        self.status_bar(ui, app);
        self.modals(ui, app, &mut actions);
        for a in actions {
            app.dispatch(a);
        }
    }

    fn shortcuts(&mut self, ui: &Ui, app: &App, actions: &mut Vec<Action>) {
        let io = ui.io();
        if io.want_text_input() {
            return;
        }
        let cmd = io.key_ctrl() || io.key_super();
        let shift = io.key_shift();
        if cmd && ui.is_key_pressed(Key::Z) {
            actions.push(if shift { Action::Redo } else { Action::Undo });
        }
        if cmd && ui.is_key_pressed(Key::Y) {
            actions.push(Action::Redo);
        }
        if cmd && ui.is_key_pressed(Key::O) {
            self.open_source_dialog(actions);
        }
        if cmd && ui.is_key_pressed(Key::S) {
            self.save_map_dialog(app, actions);
        }
        if cmd && ui.is_key_pressed(Key::D) {
            actions.push(Action::Duplicate);
        }
        if cmd && ui.is_key_pressed(Key::Q) {
            actions.push(Action::Quit);
        }
        if (ui.is_key_pressed(Key::Delete) || ui.is_key_pressed(Key::Backspace))
            && !cmd
            && app.session.selected().is_some()
        {
            actions.push(Action::Delete);
        }
        // Gizmo and camera keys, when the camera is not flying with them.
        if !cmd && !self.viewport.looking() {
            if ui.is_key_pressed(Key::W) {
                self.viewport.op = GizmoOp::Translate;
            }
            if ui.is_key_pressed(Key::E) {
                self.viewport.op = GizmoOp::Rotate;
            }
            if ui.is_key_pressed(Key::F) {
                actions.push(Action::FocusSelected);
            }
            if ui.is_key_pressed(Key::Home) {
                actions.push(Action::FrameAll);
            }
        }
    }

    fn open_source_dialog(&self, actions: &mut Vec<Action>) {
        if !self.dialogs {
            return;
        }
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open a disc image or map")
            .add_filter("Disc image or map", &["iso", "xiso", "map"])
            .pick_file()
        {
            actions.push(Action::OpenSource(p));
        }
    }

    fn open_folder_dialog(&self, actions: &mut Vec<Action>) {
        if !self.dialogs {
            return;
        }
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open a folder of maps")
            .pick_folder()
        {
            actions.push(Action::OpenSource(p));
        }
    }

    fn save_path_dialog(&self, app: &App) -> Option<PathBuf> {
        if !self.dialogs {
            return None;
        }
        rfd::FileDialog::new()
            .set_title("Save map as")
            .add_filter("Map", &["map"])
            .set_file_name(app.suggested_save_name())
            .save_file()
    }

    fn save_map_dialog(&self, app: &App, actions: &mut Vec<Action>) {
        if !app.session.is_open() {
            return;
        }
        if let Some(p) = self.save_path_dialog(app) {
            actions.push(Action::SaveMapAs(p));
        }
    }

    fn menu_bar(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        let open = app.session.is_open();
        let Some(_bar) = ui.begin_main_menu_bar() else {
            return;
        };
        if let Some(_m) = ui.begin_menu("File") {
            if ui.menu_item_with_shortcut(
                format!("{} Open disc or map...", icon::FOLDER_OPEN),
                "Cmd+O",
            ) {
                self.open_source_dialog(actions);
            }
            if ui.menu_item(format!("{} Open maps folder...", icon::FOLDER)) {
                self.open_folder_dialog(actions);
            }
            ui.separator();
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Save map as...", icon::SAVE),
                "Cmd+S",
                false,
                open,
            ) {
                self.save_map_dialog(app, actions);
            }
            if ui.menu_item_enabled_selected_no_shortcut(
                format!("{} Save as package...", icon::PACKAGE),
                false,
                open,
            ) {
                app.open_package_form();
            }
            if ui.menu_item_enabled_selected_no_shortcut(
                format!("{} Open package...", icon::PACKAGE_OPEN),
                false,
                open,
            ) {
                self.show_open_package = true;
            }
            ui.separator();
            if ui.menu_item_with_shortcut("Quit", "Cmd+Q") {
                actions.push(Action::Quit);
            }
        }
        if let Some(_m) = ui.begin_menu("Edit") {
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Undo", icon::UNDO),
                "Cmd+Z",
                false,
                app.session.can_undo(),
            ) {
                actions.push(Action::Undo);
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Redo", icon::REDO),
                "Shift+Cmd+Z",
                false,
                app.session.can_redo(),
            ) {
                actions.push(Action::Redo);
            }
            ui.separator();
            let sel = app.session.selected().is_some();
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Duplicate", icon::COPY),
                "Cmd+D",
                false,
                sel,
            ) {
                actions.push(Action::Duplicate);
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Delete", icon::TRASH),
                "Del",
                false,
                sel,
            ) {
                actions.push(Action::Delete);
            }
            ui.separator();
            let grow = app.session.allow_growth();
            if ui.menu_item_enabled_selected_no_shortcut("Allow growing full blocks", grow, open) {
                actions.push(Action::SetAllowGrowth(!grow));
            }
            if ui.is_item_hovered() {
                ui.tooltip_text(scenario_edit::GROWTH_REFUSAL);
            }
        }
        if let Some(_m) = ui.begin_menu("View") {
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Frame map", icon::MAXIMIZE),
                "Home",
                false,
                open,
            ) {
                actions.push(Action::FrameAll);
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Focus selection", icon::FOCUS),
                "F",
                false,
                app.session.selected().is_some(),
            ) {
                actions.push(Action::FocusSelected);
            }
            ui.separator();
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Move gizmo", icon::MOVE),
                "W",
                self.viewport.op == GizmoOp::Translate,
                true,
            ) {
                self.viewport.op = GizmoOp::Translate;
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Rotate gizmo", icon::ROTATE),
                "E",
                self.viewport.op == GizmoOp::Rotate,
                true,
            ) {
                self.viewport.op = GizmoOp::Rotate;
            }
            if ui.menu_item_enabled_selected_no_shortcut(
                "Gizmo in local axes",
                self.viewport.local,
                true,
            ) {
                self.viewport.local = !self.viewport.local;
            }
            ui.separator();
            if ui.menu_item("Reset layout") {
                self.reset_layout = true;
            }
        }
    }

    /// A full-viewport host window (above the status bar) holding the
    /// dockspace.
    fn dockspace(&mut self, ui: &Ui) {
        let vp = ui.main_viewport();
        let (pos, size) = (vp.work_pos(), vp.work_size());
        ui.set_next_window_viewport(vp.id());
        let flags = WindowFlags::NO_DOCKING
            | WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS
            | WindowFlags::NO_SAVED_SETTINGS;
        let rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
        let border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
        let padding = ui.push_style_var(StyleVar::WindowPadding([4.0, 4.0]));
        let root = ui.get_id("studio-dockspace");
        let apply = if std::mem::take(&mut self.reset_layout) {
            DockLayoutApply::Replace
        } else {
            DockLayoutApply::IfMissing
        };
        let layout = &self.layout;
        let mut result = Ok(root);
        ui.window("##dock-host")
            .flags(flags)
            .position(pos, Condition::Always)
            .size(
                [size[0], (size[1] - theme::STATUS_BAR).max(1.0)],
                Condition::Always,
            )
            .build(|| {
                result = ui
                    .dockspace()
                    .root_id(root)
                    .current_window(ui.content_region_avail())
                    .layout(layout, apply)
                    .build();
            });
        padding.pop();
        border.pop();
        rounding.pop();
        if let Err(e) = result {
            eprintln!("dockspace: {e}");
        }
    }

    fn heading(&self, ui: &Ui, text: &str) {
        let f = ui.push_font(self.fonts.bold);
        ui.text(text);
        f.pop();
    }

    fn tools_window(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        ui.window(&self.keys.tools).build(|| {
            self.heading(ui, "Tools");
            for tool in ToolKind::ALL {
                let label = format!("{}  {}", tool_icon(tool), tool.title());
                let disabled = ui.begin_disabled_with_cond(!tool.available());
                if ui
                    .selectable_config(&label)
                    .selected(app.tool == tool)
                    .build()
                {
                    app.tool = tool;
                }
                disabled.end();
                if !tool.available() {
                    ui.set_item_tooltip("Arrives in phase 2; h2-editor has it meanwhile.");
                }
            }
            ui.spacing();
            ui.separator();
            ui.spacing();
            self.heading(ui, "Maps");
            if ui.button(format!("{} Open...", icon::FOLDER_OPEN)) {
                self.open_source_dialog(actions);
            }
            ui.same_line();
            if ui.button(format!("{} Folder...", icon::FOLDER)) {
                self.open_folder_dialog(actions);
            }
            match app.source() {
                Some(src) => {
                    let kind = match src {
                        editor_core::MapSource::Disc(_) => icon::DISC,
                        editor_core::MapSource::Loose(_) => icon::FILE,
                        editor_core::MapSource::Folder(_) => icon::FOLDER,
                    };
                    ui.text_disabled(format!("{kind} {}", src.path().display()));
                }
                None => {
                    ui.text_wrapped("Open a disc image, a .map file or a folder of maps.");
                }
            }
            let loaded = app.loaded_map().map(str::to_owned);
            for name in app.maps() {
                let on = loaded.as_deref() == Some(name.as_str());
                if ui
                    .selectable_config(format!("{}  {name}##map-{name}", icon::MAP))
                    .selected(on)
                    .build()
                    && !on
                {
                    actions.push(Action::SelectMap(name.clone()));
                }
            }
        });
    }

    fn outliner_window(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        ui.window(&self.keys.outliner).build(|| {
            if !app.session.is_open() {
                ui.text_disabled("No map open.");
                return;
            }
            ui.set_next_item_width(-1.0);
            ui.input_text("##outliner-filter", &mut app.filter)
                .hint(format!("{} Filter placements", icon::SEARCH))
                .build();
            let sel = app.session.selected();
            let (remove, add) = match sel {
                Some((kind, _)) => (
                    app.session.remove_refusal(kind),
                    app.session.add_refusal(kind),
                ),
                None => (Some("select a placement"), Some("select a placement")),
            };
            let d = ui.begin_disabled_with_cond(add.is_some());
            if ui.button(format!("{} Duplicate", icon::COPY)) {
                actions.push(Action::Duplicate);
            }
            d.end();
            if let Some(why) = add {
                ui.set_item_tooltip(why);
            }
            ui.same_line();
            let d = ui.begin_disabled_with_cond(remove.is_some());
            if ui.button(format!("{} Delete", icon::TRASH)) {
                actions.push(Action::Delete);
            }
            d.end();
            if let Some(why) = remove {
                ui.set_item_tooltip(why);
            }
            ui.separator();
            let filter = app.filter.to_ascii_lowercase();
            let session = &app.session;
            let Some(scenario) = session.scenario() else {
                return;
            };
            ui.child_window("##outliner-tree").build(ui, || {
                for b in &scenario.blocks {
                    let kind = b.def.kind;
                    let header = format!(
                        "{}  {}  ({})",
                        theme::block_icon(kind),
                        b.def.label,
                        b.len()
                    );
                    let node = ui
                        .tree_node_config(format!("block-{kind:?}"))
                        .label(header)
                        .span_avail_width(true)
                        .push();
                    if let Some(_p) = ui.begin_popup_context_item() {
                        entry_menu(ui, session, kind, None, actions);
                    }
                    let Some(_node) = node else { continue };
                    for i in 0..b.len() {
                        let label = scenario.label(kind, i);
                        if !filter.is_empty() && !label.to_ascii_lowercase().contains(&filter) {
                            continue;
                        }
                        let on = sel == Some((kind, i));
                        if ui
                            .selectable_config(format!("{label}##{kind:?}-{i}"))
                            .selected(on)
                            .allow_double_click(true)
                            .build()
                        {
                            actions.push(Action::Select(Some((kind, i))));
                            if ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left) {
                                actions.push(Action::FocusSelected);
                            }
                        }
                        if let Some(_p) = ui.begin_popup_context_item() {
                            entry_menu(ui, session, kind, Some(i), actions);
                        }
                    }
                }
            });
        });
    }

    fn viewport_window(
        &mut self,
        ui: &Ui,
        app: &mut App,
        gpu: &mut Gpu<'_>,
        actions: &mut Vec<Action>,
    ) {
        let padding = ui.push_style_var(StyleVar::WindowPadding([0.0, 0.0]));
        let viewport = &mut self.viewport;
        ui.window(&self.keys.viewport)
            .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
            .build(|| {
                viewport_toolbar(ui, app, viewport, actions);
                if app.session.is_open() {
                    viewport.draw(ui, app, gpu, actions);
                } else {
                    ui.text_disabled("  Pick a map to see it here.");
                }
            });
        padding.pop();
    }

    fn inspector_window(&mut self, ui: &Ui, app: &mut App) {
        ui.window(&self.keys.inspector).build(|| {
            let Some((kind, index)) = app.session.selected() else {
                ui.text_disabled("Select a placement in the outliner or the viewport.");
                return;
            };
            let Some(scenario) = app.session.scenario() else {
                return;
            };
            let block = scenario.block(kind);
            let f = ui.push_font(self.fonts.bold);
            ui.text(format!(
                "{}  {}",
                theme::block_icon(kind),
                scenario.label(kind, index)
            ));
            f.pop();
            ui.text_disabled(format!(
                "{} #{index}   stride {:#x}",
                block.def.label, block.def.stride
            ));
            let named = scenario.names_of(kind, index).len();
            if named > 0 {
                ui.text_colored(
                    theme::WARN,
                    format!("{} named by {named} scenario object name(s)", icon::ALERT),
                );
                ui.set_item_tooltip(
                    "Scripts may create it by name; deleting it leaves the name naming nothing.",
                );
            }
            ui.separator();
            let label_w = 130.0;
            let mut edits = Vec::new();
            let mut done = false;
            for f in block.def.fields {
                let Ok(value) = scenario.get(kind, index, f.name) else {
                    continue;
                };
                ui.align_text_to_frame_padding();
                ui.text_disabled(f.name);
                ui.set_item_tooltip(format!("{:#x}  {}", f.offset, f.evidence));
                ui.same_line_with_pos(label_w);
                ui.set_next_item_width(-1.0);
                let d = ui.begin_disabled_with_cond(!f.editable);
                if let Some(v) = value_editor(ui, scenario, kind, f, value) {
                    edits.push((f.name, v));
                }
                done |= ui.is_item_deactivated_after_edit();
                d.end();
            }
            if let Some(p) = &block.def.palette {
                ui.separator();
                ui.text_disabled(format!(
                    "palette: {} entries, stride {:#x}",
                    block.palette.len(),
                    p.stride
                ));
            }
            for (field, v) in edits {
                // A drag or a typed edit of one field is one undo step.
                let merge = hash_key(kind, index, field);
                if let Err(e) = app
                    .session
                    .apply(Command::set(kind, index, field, v).merging(merge))
                {
                    app.error(e.to_string());
                }
            }
            if done {
                app.session.seal();
            }
        });
    }

    fn log_window(&mut self, ui: &Ui, app: &App) {
        ui.window(&self.keys.log).build(|| {
            ui.child_window("##log-lines").build(ui, || {
                for line in app.log() {
                    let (color, mark) = match &line.status {
                        Status::Info(_) => (theme::DIM, icon::INFO),
                        Status::Error(_) => (theme::ERROR, icon::ALERT),
                    };
                    ui.text_colored(theme::DIM, format!("{:>8.2}", line.at));
                    ui.same_line();
                    ui.text_colored(color, mark);
                    ui.same_line();
                    ui.text(line.status.text());
                }
                if app.log().len() != self.log_seen {
                    self.log_seen = app.log().len();
                    ui.set_scroll_here_y(1.0);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &Ui, app: &App) {
        let vp = ui.main_viewport();
        let (pos, size) = (vp.work_pos(), vp.work_size());
        let flags = WindowFlags::NO_DECORATION
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_DOCKING
            | WindowFlags::NO_SAVED_SETTINGS
            | WindowFlags::NO_FOCUS_ON_APPEARING
            | WindowFlags::NO_NAV_FOCUS
            | WindowFlags::NO_SCROLLBAR;
        let rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
        let padding = ui.push_style_var(StyleVar::WindowPadding([10.0, 4.0]));
        let bg = ui.push_style_color(StyleColor::WindowBg, [0.075, 0.08, 0.09, 1.0]);
        ui.window("##status-bar")
            .flags(flags)
            .position(
                [pos[0], pos[1] + size[1] - theme::STATUS_BAR],
                Condition::Always,
            )
            .size([size[0], theme::STATUS_BAR], Condition::Always)
            .build(|| {
                match app.status() {
                    Some(Status::Error(m)) => {
                        ui.text_colored(theme::ERROR, format!("{} {m}", icon::ALERT))
                    }
                    Some(Status::Info(m)) => ui.text(format!("{} {m}", icon::CHECK)),
                    None => ui.text_disabled("Ready"),
                }
                let mut right = String::new();
                if let Some(name) = app.loaded_map() {
                    let (tris, failed) = app.session.world();
                    right = format!(
                        "{} {name}   {} tris{}   {} placements",
                        icon::MAP,
                        tris.len(),
                        if failed > 0 {
                            format!(" ({failed} sections missing)")
                        } else {
                            String::new()
                        },
                        app.session.markers().len()
                    );
                }
                let fps = format!("   {:.0} fps", ui.io().framerate());
                let dirty = if app.is_dirty() { "unsaved   " } else { "" };
                let w = ui.calc_text_size(format!("{dirty}{right}{fps}"))[0];
                ui.same_line_with_pos((size[0] - w - 14.0).max(0.0));
                if !dirty.is_empty() {
                    ui.text_colored(theme::WARN, dirty);
                    ui.same_line();
                }
                ui.text_disabled(format!("{right}{fps}"));
            });
        bg.pop();
        padding.pop();
        rounding.pop();
    }

    fn modals(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        // Unsaved changes.
        if app.pending().is_some() && !ui.is_popup_open(UNSAVED) {
            ui.open_popup(UNSAVED);
        }
        if let Some(_m) = ui
            .begin_modal_popup_config(UNSAVED)
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            let what = match app.pending() {
                Some(Action::Quit) => "quitting",
                Some(Action::SelectMap(_)) => "opening another map",
                _ => "opening another source",
            };
            ui.text(format!(
                "{} {} has unsaved edits.",
                icon::ALERT,
                app.loaded_map().unwrap_or("The map")
            ));
            ui.text_disabled(format!("Save them before {what}?"));
            ui.spacing();
            let mut answer = None;
            if ui.button(format!("{} Save as...", icon::SAVE))
                && let Some(p) = self.save_path_dialog(app)
            {
                answer = Some(Answer::SaveAs(p));
            }
            ui.same_line();
            if ui.button("Discard") {
                answer = Some(Answer::Discard);
            }
            ui.same_line();
            if ui.button("Cancel") || ui.is_key_pressed(Key::Escape) {
                answer = Some(Answer::Cancel);
            }
            if let Some(a) = answer {
                app.resolve(a);
                if app.pending().is_none() {
                    ui.close_current_popup();
                }
            }
        }

        // Save as package.
        if app.package_form.is_some() && !ui.is_popup_open(SAVE_PACKAGE) {
            ui.open_popup(SAVE_PACKAGE);
        }
        if let Some(_m) = ui
            .begin_modal_popup_config(SAVE_PACKAGE)
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            match app.package_form.take() {
                None => ui.close_current_popup(),
                Some(mut form) => {
                    ui.text_disabled(format!("in {}", app.session.maps_dir().display()));
                    ui.set_next_item_width(320.0);
                    ui.input_text("Name", &mut form.name).build();
                    ui.set_next_item_width(320.0);
                    ui.input_text("Author", &mut form.author).build();
                    ui.set_next_item_width(320.0);
                    ui.input_text("Description", &mut form.description).build();
                    ui.set_next_item_width(320.0);
                    ui.input_text("Image (PNG/JPEG)", &mut app.package_image)
                        .hint("optional, at most 512 KB")
                        .build();
                    ui.set_next_item_width(320.0);
                    ui.input_text("Directory", &mut form.dir).build();
                    ui.checkbox(
                        "Make it the active package for this map",
                        &mut form.activate,
                    );
                    if app.session.package_dir() == Some(form.dir.as_str()) {
                        ui.text_disabled("Replaces this package's patch.");
                    }
                    ui.spacing();
                    let save = ui.button(format!("{} Save package", icon::PACKAGE));
                    ui.same_line();
                    let cancel = ui.button("Cancel") || ui.is_key_pressed(Key::Escape);
                    if save {
                        let image = app.package_image.trim();
                        form.image = (!image.is_empty()).then(|| PathBuf::from(image));
                        actions.push(Action::SavePackage(form.clone()));
                    }
                    if cancel {
                        ui.close_current_popup();
                    } else {
                        // Saving closes it (App::run clears the form) on success.
                        app.package_form = Some(form);
                    }
                }
            }
        }

        // Open package.
        if std::mem::take(&mut self.show_open_package) {
            ui.open_popup(OPEN_PACKAGE);
        }
        if let Some(_m) = ui
            .begin_modal_popup_config(OPEN_PACKAGE)
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            let list = app.session.packages();
            if list.is_empty() {
                ui.text(format!(
                    "No packages for {} in {}",
                    app.session.map_name().unwrap_or("this map"),
                    app.session.maps_dir().display()
                ));
            }
            for p in &list {
                if ui.button(format!("Open##{}", p.dir_name())) {
                    actions.push(Action::OpenPackage(p.dir_name().to_owned()));
                    ui.close_current_popup();
                }
                ui.same_line();
                ui.text(&p.manifest.package.name);
                ui.same_line();
                ui.text_disabled(p.dir_name());
            }
            ui.spacing();
            if ui.button("Close") || ui.is_key_pressed(Key::Escape) {
                ui.close_current_popup();
            }
        }
    }
}

/// The viewport's toolbar row (gizmo mode, framing).
fn viewport_toolbar(ui: &Ui, app: &App, viewport: &mut ViewportView, actions: &mut Vec<Action>) {
    ui.dummy([0.0, 2.0]);
    ui.dummy([4.0, 0.0]);
    ui.same_line();
    let toggle = |label: String, on: bool| {
        let c = on.then(|| ui.push_style_color(StyleColor::Button, theme::ACCENT));
        let clicked = ui.small_button(label);
        if let Some(c) = c {
            c.pop();
        }
        clicked
    };
    if toggle(
        format!("{} Move (W)", icon::MOVE),
        viewport.op == GizmoOp::Translate,
    ) {
        viewport.op = GizmoOp::Translate;
    }
    ui.same_line();
    if toggle(
        format!("{} Rotate (E)", icon::ROTATE),
        viewport.op == GizmoOp::Rotate,
    ) {
        viewport.op = GizmoOp::Rotate;
    }
    ui.same_line();
    if toggle("Local".to_owned(), viewport.local) {
        viewport.local = !viewport.local;
    }
    ui.same_line();
    if ui.small_button(format!("{} Frame", icon::MAXIMIZE)) {
        actions.push(Action::FrameAll);
    }
    ui.same_line();
    if ui.small_button(format!("{} Focus", icon::FOCUS)) {
        actions.push(Action::FocusSelected);
    }
    if app.session.is_open() {
        ui.same_line();
        ui.text_disabled("  RMB orbit + WASD/QE fly   MMB pan   wheel zoom   LMB pick");
    }
}

/// The outliner's context menu for an entry (`index`) or a block header.
fn entry_menu(
    ui: &Ui,
    session: &scenario_edit::Session,
    kind: BlockKind,
    index: Option<usize>,
    actions: &mut Vec<Action>,
) {
    let add = session.add_refusal(kind);
    let remove = session.remove_refusal(kind);
    let item = |label: String, refusal: Option<&'static str>| -> bool {
        let clicked = ui.menu_item_enabled_selected_no_shortcut(label, false, refusal.is_none());
        if let Some(why) = refusal {
            ui.set_item_tooltip(why);
        }
        clicked
    };
    if let Some(i) = index {
        if item(format!("{} Duplicate", icon::COPY), add) {
            actions.push(Action::DuplicateEntry(kind, i));
        }
        if item(format!("{} Delete", icon::TRASH), remove) {
            actions.push(Action::DeleteEntry(kind, i));
        }
        ui.separator();
    }
    if item(format!("{} Add new at the camera focus", icon::PLUS), add) {
        actions.push(Action::Add(kind));
    }
}

fn tool_icon(tool: ToolKind) -> &'static str {
    match tool {
        ToolKind::Scenario => icon::MAP,
        ToolKind::Variants => icon::SLIDERS,
        ToolKind::Playlists => icon::LAYERS,
    }
}

fn hash_key(kind: BlockKind, index: usize, field: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("studio-inspector", kind, index, field).hash(&mut h);
    h.finish()
}

fn drag_i64(label: &str) -> Drag<i64, &str> {
    Drag::new(label)
}
fn drag_f32(label: &str) -> Drag<f32, &str> {
    Drag::new(label)
}

/// A typed editor for one field; returns the new value when changed.
/// Rotations and facings are shown in degrees.
fn value_editor(
    ui: &Ui,
    scenario: &Scenario,
    kind: BlockKind,
    f: &FieldDef,
    value: Value,
) -> Option<Value> {
    let id = format!("##{:?}-{}", kind, f.name);
    let degrees = matches!(f.role, Role::Rotation | Role::Facing);
    let (to_ui, from_ui) = if degrees {
        (180.0 / std::f32::consts::PI, std::f32::consts::PI / 180.0)
    } else {
        (1.0, 1.0)
    };
    match value {
        Value::Vec3(v) => {
            let mut shown = v.map(|x| x * to_ui);
            let speed = if degrees { 0.5 } else { 0.05 };
            drag_f32(&id)
                .speed(speed)
                .build_array(ui, &mut shown)
                .then(|| Value::Vec3(shown.map(|x| x * from_ui)))
        }
        Value::Float(x) => {
            let mut shown = x * to_ui;
            let speed = if degrees { 0.5 } else { 0.01 };
            drag_f32(&id)
                .speed(speed)
                .build(ui, &mut shown)
                .then_some(Value::Float(shown * from_ui))
        }
        Value::Int(mut x) => {
            if f.role == Role::PaletteIndex {
                let palette = &scenario.block(kind).palette;
                let name = |i: i64| -> String {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| palette.get(i))
                        .and_then(|r| scenario.tag_name(r.id))
                        .map(|n| format!("{i}: {n}"))
                        .unwrap_or_else(|| if i < 0 { "none".into() } else { i.to_string() })
                };
                let before = x;
                if let Some(_c) = ui.begin_combo(&id, name(x)) {
                    for i in -1..palette.len() as i64 {
                        if ui.selectable_config(name(i)).selected(i == x).build() {
                            x = i;
                        }
                    }
                }
                return (x != before).then_some(Value::Int(x));
            }
            let (lo, hi) = match f.ty {
                FieldType::U8 => (0i64, 0xff),
                FieldType::I16 => (-0x8000, 0x7fff),
                FieldType::U16 => (0, 0xffff),
                _ => (0, 0xffff_ffff),
            };
            drag_i64(&id)
                .range(lo, hi)
                .speed(0.1)
                .build(ui, &mut x)
                .then_some(Value::Int(x))
        }
        Value::Tag(r) => {
            let text = |r: TagRef| {
                if r.is_none() {
                    "none".to_owned()
                } else {
                    format!("{}: {}", r.group, scenario.tag_name(r.id).unwrap_or("?"))
                }
            };
            let mut chosen = r;
            if let Some(_c) = ui.begin_combo(&id, text(r)) {
                let mut options: Vec<TagRef> = scenario
                    .tags
                    .iter()
                    .filter(|(_, t)| scenario_edit::layout::ITEM_GROUPS.contains(&t.group.0))
                    .map(|(&id, t)| TagRef { group: t.group, id })
                    .collect();
                options.sort_by_key(|o| text(*o));
                for o in options {
                    if ui.selectable_config(text(o)).selected(o == r).build() {
                        chosen = o;
                    }
                }
            }
            (chosen != r).then_some(Value::Tag(chosen))
        }
    }
}
