//! The Dear ImGui shell: main menu bar, a dockspace with the default
//! layout (tools and maps left over the outliner, the viewport in the
//! middle, the inspector right, the log at the bottom), a status bar, and
//! the modals (unsaved changes, packages). The outliner, centre and
//! inspector windows show the active tool: the scenario's placements, 3D
//! viewport and fields here; the game variant and playlist tools' in
//! [`variants`] and [`playlists`]. Everything goes back to [`App`] as
//! [`Action`]s.

mod playlists;
mod ui_screens;
mod variants;
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
    pub variants: variants::VariantsUi,
    playlists: playlists::PlaylistsUi,
    pub ui_screens: ui_screens::UiScreensUi,
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
            variants: variants::VariantsUi::default(),
            playlists: playlists::PlaylistsUi::default(),
            ui_screens: ui_screens::UiScreensUi::default(),
        }
    }

    /// Build one frame of UI and run the actions it produced.
    pub fn frame(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>) {
        app.poll_convert();
        let mut actions = Vec::new();
        self.shortcuts(ui, app, &mut actions);
        self.menu_bar(ui, app, &mut actions);
        self.dockspace(ui);
        self.tools_window(ui, app, &mut actions);
        match app.tool {
            ToolKind::Scenario => {
                self.outliner_window(ui, app, &mut actions);
                self.viewport_window(ui, app, gpu, &mut actions);
                self.inspector_window(ui, app);
            }
            ToolKind::Variants => self.variant_windows(ui, app, &mut actions),
            ToolKind::Playlists => self.playlist_windows(ui, app, &mut actions),
            ToolKind::UiScreens => self.ui_screen_windows(ui, app, gpu, &mut actions),
        }
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
            match app.tool {
                ToolKind::Scenario => self.save_map_dialog(app, actions),
                ToolKind::Variants => actions.push(Action::SaveVariant),
                ToolKind::Playlists if app.hoppers.is_open() => {
                    playlists::save(app, self.dialogs, actions)
                }
                ToolKind::Playlists | ToolKind::UiScreens => {}
            }
        }
        if cmd && ui.is_key_pressed(Key::Q) {
            actions.push(Action::Quit);
        }
        if app.tool != ToolKind::Scenario {
            return;
        }
        if cmd && ui.is_key_pressed(Key::D) {
            actions.push(Action::Duplicate);
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
            self.tool_file_menu(ui, app, actions);
            ui.separator();
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Save map as...", icon::SAVE),
                if app.tool == ToolKind::Scenario {
                    "Cmd+S"
                } else {
                    ""
                },
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
                app.can_undo(),
            ) {
                actions.push(Action::Undo);
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Redo", icon::REDO),
                "Shift+Cmd+Z",
                false,
                app.can_redo(),
            ) {
                actions.push(Action::Redo);
            }
            ui.separator();
            let sel = app.tool == ToolKind::Scenario && app.session.selected().is_some();
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
            if ui.menu_item_enabled_selected_no_shortcut(
                "Halo 1 overlay",
                app.overlay_shown(),
                app.compare().is_some_and(|c| c.view.is_ok()),
            ) {
                actions.push(Action::SetOverlay(!app.overlay_shown()));
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

    /// The File menu's items for the variant and playlist tools.
    fn tool_file_menu(&mut self, ui: &Ui, app: &App, actions: &mut Vec<Action>) {
        match app.tool {
            ToolKind::Scenario | ToolKind::UiScreens => {}
            ToolKind::Variants => {
                let e = app.variants.as_ref();
                let can_save = e.is_some_and(|e| e.is_dirty() && !e.is_read_only());
                if ui.menu_item_enabled_selected_with_shortcut(
                    format!("{} Save variant", icon::SAVE),
                    "Cmd+S",
                    false,
                    can_save,
                ) {
                    actions.push(Action::SaveVariant);
                }
                if ui.menu_item(format!("{} Import variant...", icon::PACKAGE_OPEN))
                    && let Some(p) = variants::import_dialog(self.dialogs)
                {
                    actions.push(Action::ImportVariantFile(p));
                }
            }
            ToolKind::Playlists => {
                if ui.menu_item(format!("{} New playlist file", icon::PLUS)) {
                    actions.push(Action::NewHopper);
                }
                if ui.menu_item("New from the default lineup") {
                    actions.push(Action::NewDefaultHopper);
                }
                if ui.menu_item(format!("{} Open playlist file...", icon::FOLDER_OPEN))
                    && let Some(p) = playlists::open_dialog(self.dialogs)
                {
                    actions.push(Action::OpenHopper(p));
                }
                let open = app.hoppers.is_open();
                if ui.menu_item_enabled_selected_with_shortcut(
                    format!("{} Save playlist file", icon::SAVE),
                    "Cmd+S",
                    false,
                    open,
                ) {
                    playlists::save(app, self.dialogs, actions);
                }
                if ui.menu_item_enabled_selected_no_shortcut(
                    "Save playlist file as...",
                    false,
                    open,
                ) && let Some(p) = playlists::save_dialog(self.dialogs, app)
                {
                    actions.push(Action::SaveHopperAs(p));
                }
            }
        }
    }

    /// The variant tool in the outliner, centre and inspector windows.
    fn variant_windows(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let st = &mut self.variants;
        ui.window(self.keys.outliner.label(format!("{} Variants", icon::TREE)))
            .build(|| variants::list(ui, app, st, fonts, actions));
        ui.window(
            self.keys
                .viewport
                .label(format!("{} Settings", icon::SLIDERS)),
        )
        .build(|| variants::settings(ui, app, st, fonts, actions));
        let dialogs = self.dialogs;
        ui.window(&self.keys.inspector)
            .build(|| variants::inspector(ui, app, st, fonts, dialogs, actions));
    }

    /// The playlist tool in the outliner, centre and inspector windows.
    fn playlist_windows(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let dialogs = self.dialogs;
        ui.window(
            self.keys
                .outliner
                .label(format!("{} Playlists", icon::TREE)),
        )
        .build(|| playlists::list(ui, app, fonts, dialogs, actions));
        ui.window(
            self.keys
                .viewport
                .label(format!("{} Entries", icon::LAYERS)),
        )
        .build(|| playlists::entries(ui, app, fonts, actions));
        let st = &mut self.playlists;
        ui.window(&self.keys.inspector)
            .build(|| playlists::inspector(ui, app, st, fonts, dialogs, actions));
    }

    /// The UI Screens tool in the outliner, centre and inspector windows.
    fn ui_screen_windows(
        &mut self,
        ui: &Ui,
        app: &mut App,
        gpu: &mut Gpu<'_>,
        actions: &mut Vec<Action>,
    ) {
        let fonts = &self.fonts;
        let st = &mut self.ui_screens;
        ui.window(self.keys.outliner.label(format!("{} Screens", icon::TREE)))
            .build(|| ui_screens::tree(ui, app, st, fonts, actions));
        ui.window(self.keys.viewport.label(format!("{} Preview", icon::EYE)))
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(|| ui_screens::preview(ui, app, st, gpu));
        let dialogs = self.dialogs;
        ui.window(&self.keys.inspector)
            .build(|| ui_screens::inspector(ui, app, st, fonts, dialogs, actions));
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
                let dirty = if app.tool_dirty(tool) { "  *" } else { "" };
                if ui
                    .selectable_config(format!("{label}{dirty}"))
                    .selected(app.tool == tool)
                    .build()
                    && app.tool != tool
                {
                    actions.push(Action::SetTool(tool));
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
            let loaded = app
                .loaded_map()
                .filter(|_| app.opened_package().is_none())
                .map(str::to_owned);
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
            self.library_lists(ui, app, actions);
        });
    }

    /// The Maps list's other groups: user packages, then Halo 1 (converted
    /// packages, and the maps of an open Halo 1 source, read-only).
    fn library_lists(&self, ui: &Ui, app: &App, actions: &mut Vec<Action>) {
        let opened = app.opened_package().map(str::to_owned);
        let entry = |e: &editor_core::packages::PackageEntry, actions: &mut Vec<Action>| {
            let key = e.key();
            let on = opened.as_deref() == Some(key.as_str());
            if ui
                .selectable_config(format!("{}  {}##pkg-{key}", icon::PACKAGE, e.label()))
                .selected(on)
                .build()
                && !on
            {
                actions.push(Action::OpenPackageEntry(key.clone()));
            }
            ui.set_item_tooltip(format!(
                "{}\nin <data>/maps/{}{}",
                if e.derived {
                    format!("a new map, loaded as {}", e.map)
                } else {
                    format!("an edit of {}", e.map)
                },
                e.dir,
                match &e.halo1 {
                    Some(o) => format!("\nconverted from Halo 1's {}", o.map),
                    None => String::new(),
                }
            ));
        };
        let (h1, own): (Vec<_>, Vec<_>) = app.packages().iter().partition(|e| e.halo1.is_some());
        if !own.is_empty() {
            ui.spacing();
            self.heading(ui, "Packages");
            for e in own {
                entry(e, actions);
            }
        }
        if h1.is_empty() && app.halo1_source().is_none() {
            return;
        }
        ui.spacing();
        self.heading(ui, "Halo 1");
        for e in h1 {
            entry(e, actions);
        }
        if let Some(src) = app.halo1_source() {
            ui.text_disabled(format!(
                "{} {} (read only)",
                icon::DISC,
                src.path().display()
            ));
            let loaded = app.loaded_halo1();
            for name in app.halo1_maps() {
                let on = loaded == Some(name.as_str());
                if ui
                    .selectable_config(format!("{}  {name}##h1-{name}", icon::EYE))
                    .selected(on)
                    .build()
                    && !on
                {
                    actions.push(Action::SelectHalo1Map(name.clone()));
                }
            }
        }
    }

    fn outliner_window(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        ui.window(&self.keys.outliner).build(|| {
            if let Some(view) = app.halo1_view() {
                halo1_outliner(ui, app, view, actions);
                return;
            }
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
                if app.session.is_open() || app.session.reference().is_some() {
                    viewport.draw(ui, app, gpu, actions);
                } else {
                    ui.text_disabled("  Pick a map to see it here.");
                }
            });
        padding.pop();
    }

    fn inspector_window(&mut self, ui: &Ui, app: &mut App) {
        let mut actions = Vec::new();
        ui.window(&self.keys.inspector).build(|| {
            if let Some(view) = app.halo1_view() {
                self.halo1_inspector(ui, app, view, &mut actions);
                return;
            }
            if let Some(c) = app.compare() {
                compare_banner(ui, c);
            }
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
        for a in actions {
            app.dispatch(a);
        }
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
                if app.tool == ToolKind::Playlists {
                    right = playlists::summary(app);
                } else if app.tool == ToolKind::Variants {
                    if let Some(e) = &app.variants {
                        right = format!(
                            "{} {} saved   {} built-in",
                            icon::SLIDERS,
                            e.saved().len(),
                            e.builtins().len()
                        );
                    }
                } else if let Some(name) = app.loaded_map() {
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
                Some(Action::OpenSource(_)) => "opening another source",
                Some(Action::OpenHopper(_)) => "opening another playlist file",
                Some(Action::NewHopper | Action::NewDefaultHopper) => {
                    "starting a new playlist file"
                }
                Some(Action::EditEntryVariant(..)) => "opening the entry's variant",
                _ => "opening another variant",
            };
            let tool = app.pending_tool().unwrap_or(ToolKind::Scenario);
            let who = match tool {
                ToolKind::Scenario => app.loaded_map().unwrap_or("The map").to_owned(),
                ToolKind::Variants => "The open game variant".to_owned(),
                ToolKind::Playlists => "The playlist file".to_owned(),
                ToolKind::UiScreens => "The UI screen".to_owned(),
            };
            ui.text(format!("{} {who} has unsaved edits.", icon::ALERT));
            ui.text_disabled(format!("Save them before {what}?"));
            ui.spacing();
            let mut answer = None;
            match tool {
                ToolKind::Scenario => {
                    if ui.button(format!("{} Save as...", icon::SAVE))
                        && let Some(p) = self.save_path_dialog(app)
                    {
                        answer = Some(Answer::SaveAs(p));
                    }
                }
                ToolKind::Variants => {
                    let can = app.variants.as_ref().is_some_and(|e| !e.is_read_only());
                    let d = ui.begin_disabled_with_cond(!can);
                    if ui.button(format!("{} Save", icon::SAVE)) {
                        answer = Some(Answer::Save);
                    }
                    d.end();
                }
                ToolKind::Playlists => {
                    if app.hoppers.path().is_some() {
                        if ui.button(format!("{} Save", icon::SAVE)) {
                            answer = Some(Answer::Save);
                        }
                    } else if ui.button(format!("{} Save as...", icon::SAVE))
                        && let Some(p) = playlists::save_dialog(self.dialogs, app)
                    {
                        answer = Some(Answer::SaveAs(p));
                    }
                }
                ToolKind::UiScreens => {}
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
    if app.compare().is_some_and(|c| c.view.is_ok()) {
        ui.same_line();
        if toggle(
            format!("{} Halo 1 overlay", icon::LAYERS),
            app.overlay_shown(),
        ) {
            actions.push(Action::SetOverlay(!app.overlay_shown()));
        }
    }
    if app.session.is_open() || app.session.reference().is_some() {
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
        ToolKind::UiScreens => icon::EYE,
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

/// The outliner of a Halo 1 map shown read-only: its placements by kind.
fn halo1_outliner(
    ui: &Ui,
    app: &App,
    view: &editor_core::halo1::H1View,
    actions: &mut Vec<Action>,
) {
    use crate::app::block_of;
    ui.text_colored(theme::WARN, format!("{} Halo 1: read only", icon::EYE));
    ui.separator();
    let sel = app.session.selected();
    ui.child_window("##h1-outliner-tree").build(ui, || {
        for kind in editor_core::halo1::H1Kind::ALL {
            let block = block_of(kind);
            let n = view.of(kind).count();
            let header = format!("{}  {}  ({n})", theme::block_icon(block), kind.label());
            let Some(_node) = ui
                .tree_node_config(format!("h1-{kind:?}"))
                .label(header)
                .span_avail_width(true)
                .push()
            else {
                continue;
            };
            for p in view.of(kind) {
                let on = sel == Some((block, p.index));
                if ui
                    .selectable_config(format!("{}##h1-{kind:?}-{}", p.label, p.index))
                    .selected(on)
                    .allow_double_click(true)
                    .build()
                {
                    actions.push(Action::Select(Some((block, p.index))));
                    if ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left) {
                        actions.push(Action::FocusSelected);
                    }
                }
            }
        }
    });
}

/// Above a converted map's inspector: where it came from, and whether its
/// Halo 1 source is at hand for the overlay.
fn compare_banner(ui: &Ui, c: &crate::app::Compare) {
    let sandbox = c
        .origin
        .sandbox
        .map(|s| format!(", {} sandbox", s.short()))
        .unwrap_or_default();
    ui.text_colored(
        theme::ACCENT,
        format!(
            "{} Converted from Halo 1's {}{sandbox}",
            icon::LAYERS,
            c.origin.map
        ),
    );
    match &c.view {
        Ok(_) => ui.text_disabled("Halo 1 overlay: View > Halo 1 overlay"),
        Err(why) => {
            ui.text_disabled("No Halo 1 overlay");
            ui.set_item_tooltip(why);
        }
    }
    ui.separator();
}

impl Studio {
    fn halo1_inspector(
        &self,
        ui: &Ui,
        app: &App,
        view: &editor_core::halo1::H1View,
        actions: &mut Vec<Action>,
    ) {
        ui.text_colored(
            theme::WARN,
            format!("{} Halo 1 map {}: read only", icon::EYE, view.name),
        );
        ui.text_wrapped(
            "Halo 1 maps are shown as they are on the disc. Edit a converted copy \
             (Maps > Halo 1) instead.",
        );
        convert_controls(ui, app, view, actions);
        for n in &view.notes {
            ui.text_colored(theme::ERROR, format!("{} {n}", icon::ALERT));
        }
        ui.separator();
        let Some((block, index)) = app.session.selected() else {
            ui.text_disabled("Select a placement in the outliner or the viewport.");
            return;
        };
        let Some(p) = crate::app::kind_of(block).and_then(|k| view.get(k, index)) else {
            return;
        };
        let f = ui.push_font(self.fonts.bold);
        ui.text(format!("{}  {}", theme::block_icon(block), p.label));
        f.pop();
        ui.text_disabled(format!("{} #{}", p.kind.label(), p.index));
        ui.separator();
        for (name, value) in &p.fields {
            ui.text_disabled(*name);
            ui.same_line_with_pos(130.0);
            ui.text(value);
        }
    }
}

/// "Convert to Halo 2" for the Halo 1 map on show: the sandbox, the base
/// map h1convert picks, the button, and the running conversion or why the
/// last one wrote nothing.
fn convert_controls(
    ui: &Ui,
    app: &App,
    view: &editor_core::halo1::H1View,
    actions: &mut Vec<Action>,
) {
    use crate::app::ConvertSandbox;
    let base = crate::app::base_for(&view.name);
    ui.separator_with_text("Convert to Halo 2");
    ui.text_disabled("Sandbox");
    for (s, label) in [
        (
            ConvertSandbox::Halo2,
            "Halo 2 (nearest weapons and vehicles)",
        ),
        (ConvertSandbox::Halo1, "Halo 1 (its own weapons and vehicles)"),
    ] {
        if ui.radio_button_bool(label, app.convert_sandbox == s) && app.convert_sandbox != s {
            actions.push(Action::SetConvertSandbox(s));
        }
    }
    let has_base = app.maps().iter().any(|m| m.eq_ignore_ascii_case(base));
    ui.text_disabled(format!("Onto {base}, from your Halo 2 disc"));
    let running = app.converting();
    let d = ui.begin_disabled_with_cond(running.is_some() || !has_base);
    if ui.button(format!("{} Convert to Halo 2", icon::PACKAGE)) {
        actions.push(Action::ConvertHalo1(app.convert_sandbox));
    }
    d.end();
    if !has_base {
        ui.set_item_tooltip(format!(
            "Open your Halo 2 disc: the conversion starts from {base}."
        ));
    } else {
        ui.set_item_tooltip(format!(
            "Writes <data>/maps/h1_{}/ and opens the result.",
            view.name.to_ascii_lowercase()
        ));
    }
    if let Some(r) = running {
        ui.text_colored(
            theme::ACCENT,
            format!(
                "Converting {} ({} sandbox)... {:.0} s",
                r.h1_map,
                r.sandbox.name(),
                r.started.elapsed().as_secs_f32()
            ),
        );
    } else if let Some(e) = app.convert_error() {
        ui.text_colored(theme::ERROR, format!("{} Not converted", icon::ALERT));
        ui.text_wrapped(e);
    }
}
