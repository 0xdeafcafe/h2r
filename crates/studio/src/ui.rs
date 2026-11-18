//! The Dear ImGui shell: main menu bar, a dockspace with the default
//! layout (tools and maps left over the outliner, the viewport in the
//! middle, the inspector right, the log at the bottom), a status bar, and
//! the modals (unsaved changes, packages). The outliner, centre and
//! inspector windows show the active tool: the scenario's placements, 3D
//! viewport and fields here; the game variant and playlist tools' in
//! [`variants`] and [`playlists`]. Everything goes back to [`App`] as
//! [`Action`]s.

mod inspector;
pub mod map_pack;
mod outliner;
mod play;
mod playlists;
pub mod tags;
mod ui_screens;
mod hud;
mod variants;
use std::path::PathBuf;

use dear_imgui_rs::{
    Condition, DockLayout, DockLayoutApply, DockSplit, Key, StyleColor, StyleVar, Ui,
    WindowFlags, WindowKey,
};
use editor_core::Status;
use scenario_edit::BlockKind;

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
    game: WindowKey,
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
            game: key("game", format!("{} Game", icon::ZAP)),
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
                    DockLayout::tabs([&self.log, &self.game]),
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
    pub hud: hud::HudUi,
    inspector: inspector::InspectorState,
    /// Play and pair (the Game panel; `crate::play`).
    pub play: play::PlayUi,
    /// The Tags tool's UI state (its field panel's too).
    pub tags: tags::TagsUi,
    /// The Maps list's Map Pack pictures.
    pub map_pack: map_pack::MapPackUi,
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
            hud: hud::HudUi::default(),
            inspector: inspector::InspectorState::default(),
            play: play::PlayUi::default(),
            tags: tags::TagsUi::default(),
            map_pack: map_pack::MapPackUi::default(),
        }
    }

    /// Build one frame of UI and run the actions it produced.
    pub fn frame(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>) {
        app.poll_convert();
        self.play.update(app);
        let mut actions = Vec::new();
        self.shortcuts(ui, app, &mut actions);
        self.menu_bar(ui, app, &mut actions);
        self.dockspace(ui);
        self.tools_window(ui, app, gpu, &mut actions);
        match app.tool {
            ToolKind::Scenario => {
                self.outliner_window(ui, app, &mut actions);
                self.viewport_window(ui, app, gpu, &mut actions);
                self.inspector_window(ui, app);
                self.play.shortcuts(ui, app);
                self.play.window(ui, app, &self.keys.game);
            }
            ToolKind::Variants => self.variant_windows(ui, app, &mut actions),
            ToolKind::Playlists => self.playlist_windows(ui, app, &mut actions),
            ToolKind::UiScreens => self.ui_screen_windows(ui, app, gpu, &mut actions),
            ToolKind::Hud => self.hud_windows(ui, app, gpu, &mut actions),
            ToolKind::Tags => self.tag_windows(ui, app, &mut actions),
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
                ToolKind::Scenario | ToolKind::Tags => self.save_map_dialog(app, actions),
                ToolKind::Variants => actions.push(Action::SaveVariant),
                ToolKind::Playlists if app.hoppers.is_open() => {
                    playlists::save(app, self.dialogs, actions)
                }
                ToolKind::Hud => {
                    if let Some(p) = hud::save_dialog(self.dialogs, app) {
                        actions.push(Action::HudSaveAs(p));
                    }
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
        if !cmd && !self.viewport.looking() && !io.key_alt() {
            if ui.is_key_pressed(Key::W) {
                self.viewport.op = GizmoOp::Translate;
            }
            if ui.is_key_pressed(Key::E) {
                self.viewport.op = GizmoOp::Rotate;
            }
            if ui.is_key_pressed(Key::R) {
                self.viewport.op = GizmoOp::Scale;
            }
            if ui.is_key_pressed(Key::X) {
                self.viewport.local = !self.viewport.local;
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
                if matches!(app.tool, ToolKind::Scenario | ToolKind::Tags) {
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
            if ui.menu_item_enabled_selected_with_shortcut(
                format!("{} Scale gizmo", icon::MAXIMIZE),
                "R",
                self.viewport.op == GizmoOp::Scale,
                true,
            ) {
                self.viewport.op = GizmoOp::Scale;
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                "Gizmo in local axes",
                "X",
                self.viewport.local,
                true,
            ) {
                self.viewport.local = !self.viewport.local;
            }
            let snap = &mut self.viewport.snap;
            if ui.menu_item_enabled_selected_no_shortcut("Snap to grid", snap.grid, true) {
                snap.grid = !snap.grid;
            }
            if ui.menu_item_enabled_selected_no_shortcut("Snap angles", snap.angle, true) {
                snap.angle = !snap.angle;
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
            ToolKind::Hud => {
                let can = app.hud.session.as_ref().is_some_and(|s| !s.edited().is_empty());
                if ui.menu_item_enabled_selected_with_shortcut(
                    format!("{} Save map with HUD edits as...", icon::SAVE),
                    "Cmd+S",
                    false,
                    can,
                ) && let Some(p) = hud::save_dialog(self.dialogs, app)
                {
                    actions.push(Action::HudSaveAs(p));
                }
            }
            ToolKind::Scenario | ToolKind::UiScreens | ToolKind::Tags => {}
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

    /// The HUD tool in the outliner, centre and inspector windows.
    fn hud_windows(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let st = &mut self.hud;
        ui.window(self.keys.outliner.label(format!("{} HUDs", icon::TREE)))
            .build(|| hud::tree(ui, app, st, fonts, actions));
        ui.window(self.keys.viewport.label(format!("{} HUD", icon::EYE)))
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(|| hud::preview(ui, app, st, fonts, gpu, actions));
        let dialogs = self.dialogs;
        ui.window(&self.keys.inspector)
            .build(|| hud::inspector(ui, app, st, fonts, dialogs, actions));
    }

    /// The Tags tool: the tag tree, the fields, the raw bytes.
    fn tag_windows(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        app.sync_tags();
        let fonts = &self.fonts;
        let st = &mut self.tags;
        ui.window(self.keys.outliner.label(format!("{} Tags", icon::TREE)))
            .build(|| tags::tree(ui, app, st, fonts, actions));
        ui.window(
            self.keys
                .viewport
                .label(format!("{} Fields", icon::SLIDERS)),
        )
        .build(|| tags::fields(ui, app, st, fonts, actions));
        ui.window(self.keys.inspector.label(format!("{} Bytes", icon::INFO)))
            .build(|| tags::hex(ui, app, st, fonts));
    }

    fn heading(&self, ui: &Ui, text: &str) {
        let f = ui.push_font(self.fonts.bold);
        ui.text(text);
        f.pop();
    }

    fn tools_window(
        &mut self,
        ui: &Ui,
        app: &mut App,
        gpu: &mut Gpu<'_>,
        actions: &mut Vec<Action>,
    ) {
        let mut pack = std::mem::take(&mut self.map_pack);
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
                .filter(|_| app.opened_package().is_none() && app.opened_pack_map().is_none())
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
            let bold = self.fonts.bold;
            let heading = |ui: &Ui, text: &str| {
                let f = ui.push_font(bold);
                ui.text(text);
                f.pop();
            };
            map_pack::group(ui, app, &mut pack, gpu, heading, actions);
            self.library_lists(ui, app, actions);
        });
        self.map_pack = pack;
    }

    /// The Maps list's other groups after the Map Pack ([`map_pack`]): user
    /// packages, then Halo 1 (converted packages, and the maps of an open
    /// Halo 1 source, read-only).
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
                if !e.derived && let Some(m) = app.pack_map_named(&e.map) {
                    format!("an edit of the Map Pack's {} ({})", m.name, e.map)
                } else if e.derived {
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
            let many = app.session.selection().len() > 1;
            let (remove, add) = match sel {
                _ if many => (
                    app.session.remove_selection_refusal(),
                    app.session.duplicate_selection_refusal(),
                ),
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
            if many {
                ui.same_line();
                ui.text_disabled(format!("{} selected", app.session.selection().len()));
            }
            ui.separator();
            let filter = app.filter.trim().to_owned();
            let session = &app.session;
            let Some(scenario) = session.scenario() else {
                return;
            };
            let io = ui.io();
            let (toggle, range) = (io.key_ctrl() || io.key_super(), io.key_shift());
            let mut place = None;
            ui.child_window("##outliner-tree").build(ui, || {
                for b in &scenario.blocks {
                    let kind = b.def.kind;
                    let shown: Vec<usize> = (0..b.len())
                        .filter(|&i| {
                            outliner::matches(&filter, b.def.label, &scenario.label(kind, i), i)
                        })
                        .collect();
                    if !filter.is_empty() && shown.is_empty() {
                        continue;
                    }
                    let count = if filter.is_empty() {
                        format!("{}", b.len())
                    } else {
                        format!("{} of {}", shown.len(), b.len())
                    };
                    let header = format!("{}  {}  ({count})", theme::block_icon(kind), b.def.label);
                    let mut node = ui
                        .tree_node_config(format!("block-{kind:?}"))
                        .label(header)
                        .span_avail_width(true);
                    if !filter.is_empty() {
                        // A search opens every block with a match.
                        node = node.opened(true, Condition::Always);
                    }
                    let node = node.push();
                    if let Some(_p) = ui.begin_popup_context_item() {
                        place = entry_menu(ui, session, kind, None, actions).or(place);
                    }
                    let Some(_node) = node else { continue };
                    for &i in &shown {
                        let label = scenario.label(kind, i);
                        if ui
                            .selectable_config(format!("{label}##{kind:?}-{i}"))
                            .selected(session.is_selected((kind, i)))
                            .allow_double_click(true)
                            .build()
                        {
                            actions.push(
                                match outliner::click((kind, i), toggle, range, sel, &shown) {
                                    outliner::Click::One(e) => Action::Select(Some(e)),
                                    outliner::Click::Toggle(e) => Action::ToggleSelect(e),
                                    outliner::Click::Many(all) => Action::SelectMany(all),
                                },
                            );
                            if ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left) {
                                actions.push(Action::FocusSelected);
                            }
                        }
                        if let Some(_p) = ui.begin_popup_context_item() {
                            place = entry_menu(ui, session, kind, Some(i), actions).or(place);
                        }
                    }
                }
            });
            if place.is_some() {
                self.viewport.placing = place;
            }
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
        let play = &mut self.play;
        ui.window(&self.keys.viewport)
            .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
            .build(|| {
                viewport_toolbar(ui, app, viewport, actions);
                if app.session.is_open() || app.session.reference().is_some() {
                    viewport.draw(ui, app, gpu, actions);
                    play.overlay(ui, app, viewport.shown().map(|(r, _)| r));
                } else {
                    ui.text_disabled("  Pick a map to see it here.");
                }
            });
        padding.pop();
    }

    fn inspector_window(&mut self, ui: &Ui, app: &mut App) {
        let mut actions = Vec::new();
        let mut state = std::mem::take(&mut self.inspector);
        ui.window(&self.keys.inspector).build(|| {
            if let Some(view) = app.halo1_view() {
                self.halo1_inspector(ui, app, view, &mut actions);
                return;
            }
            if let Some(c) = app.compare() {
                compare_banner(ui, c);
            }
            inspector::draw(ui, self.fonts.bold, app, &mut state);
        });
        self.inspector = state;
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
                if app.tool == ToolKind::Scenario {
                    ui.same_line_with_spacing(0.0, 24.0);
                    play::status_chip(ui, &self.play.chip());
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
                if app.tool == ToolKind::Scenario
                    && (app.session.is_open() || app.session.reference().is_some())
                {
                    // The fly speed (right mouse + WASD; the wheel sets it).
                    right.push_str(&format!(
                        "   {} {:.1} u/s",
                        icon::MOVE,
                        self.viewport.camera.speed
                    ));
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
                ToolKind::Hud => "The HUD map".to_owned(),
                ToolKind::Tags => "The map's tags".to_owned(),
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
                ToolKind::Hud => {
                    if ui.button(format!("{} Save as...", icon::SAVE))
                        && let Some(p) = hud::save_dialog(self.dialogs, app)
                    {
                        answer = Some(Answer::SaveAs(p));
                    }
                }
                ToolKind::UiScreens | ToolKind::Tags => {}
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
    if toggle(
        format!("{} Scale (R)", icon::MAXIMIZE),
        viewport.op == GizmoOp::Scale,
    ) {
        viewport.op = GizmoOp::Scale;
    }
    ui.same_line();
    let axes = if viewport.local { "Local" } else { "World" };
    if toggle(format!("{axes} (X)"), viewport.local) {
        viewport.local = !viewport.local;
    }
    ui.same_line();
    snap_controls(ui, &mut viewport.snap);
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
    if let Some((kind, _)) = viewport.placing {
        ui.same_line();
        ui.text_colored(
            theme::ACCENT,
            format!("  Click the level to place {kind:?} (Esc cancels)"),
        );
    } else if app.session.is_open() || app.session.reference().is_some() {
        ui.same_line();
        ui.text_disabled(
            "  RMB look + WASD/QE fly (wheel: speed)   Alt+LMB orbit   MMB pan   wheel dolly",
        );
    }
}

/// The toolbar's snapping: grid and angle toggles, each with its step.
fn snap_controls(ui: &Ui, snap: &mut crate::viewport::Snap) {
    use crate::viewport::{ANGLE_STEPS, GRID_STEPS};
    let toggle = |label: String, on: bool| {
        let c = on.then(|| ui.push_style_color(StyleColor::Button, theme::ACCENT));
        let clicked = ui.small_button(label);
        if let Some(c) = c {
            c.pop();
        }
        clicked
    };
    if toggle(format!("Grid {}", snap.grid_step), snap.grid) {
        snap.grid = !snap.grid;
    }
    ui.set_item_tooltip("Snap moves and placing to the grid; right-click for the step");
    if let Some(_p) = ui.begin_popup_context_item() {
        for s in GRID_STEPS {
            if ui
                .selectable_config(format!("{s} units"))
                .selected(s == snap.grid_step)
                .build()
            {
                snap.grid_step = s;
                snap.grid = true;
            }
        }
    }
    ui.same_line();
    if toggle(format!("{}°", snap.angle_step), snap.angle) {
        snap.angle = !snap.angle;
    }
    ui.set_item_tooltip("Snap turns to steps of this angle; right-click for the step");
    if let Some(_p) = ui.begin_popup_context_item() {
        for s in ANGLE_STEPS {
            if ui
                .selectable_config(format!("{s}°"))
                .selected(s == snap.angle_step)
                .build()
            {
                snap.angle_step = s;
                snap.angle = true;
            }
        }
    }
}

/// The outliner's context menu for an entry (`index`) or a block header.
/// Returns what to place with the mouse when that was picked.
fn entry_menu(
    ui: &Ui,
    session: &scenario_edit::Session,
    kind: BlockKind,
    index: Option<usize>,
    actions: &mut Vec<Action>,
) -> Option<(BlockKind, Option<usize>)> {
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
    let what = if index.is_some() { "a copy" } else { "new" };
    item(format!("{} Place {what} with the mouse", icon::PLUS), add).then_some((kind, index))
}

fn tool_icon(tool: ToolKind) -> &'static str {
    match tool {
        ToolKind::Scenario => icon::MAP,
        ToolKind::Variants => icon::SLIDERS,
        ToolKind::Playlists => icon::LAYERS,
        ToolKind::UiScreens => icon::EYE,
        ToolKind::Hud => icon::CROSSHAIR,
        ToolKind::Tags => icon::BOX,
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

/// "Convert to Halo 2" for the Halo 1 map on show: the sandbox, the
/// button (a whole new map, built from scratch), and the running
/// conversion or why the last one wrote nothing.
fn convert_controls(
    ui: &Ui,
    app: &App,
    view: &editor_core::halo1::H1View,
    actions: &mut Vec<Action>,
) {
    use crate::app::ConvertSandbox;
    ui.separator_with_text("Convert to Halo 2");
    ui.text_disabled("Sandbox");
    for (s, label) in [
        (
            ConvertSandbox::Halo2,
            "Halo 2 (nearest weapons and vehicles)",
        ),
        (
            ConvertSandbox::Halo1,
            "Halo 1 (its own weapons and vehicles)",
        ),
    ] {
        if ui.radio_button_bool(label, app.convert_sandbox == s) && app.convert_sandbox != s {
            actions.push(Action::SetConvertSandbox(s));
        }
    }
    let name = view.name.to_ascii_lowercase();
    ui.text_disabled(format!("Builds a new map, h1_{name}"));
    let running = app.converting();
    let refusal = app.convert_refusal();
    let d = ui.begin_disabled_with_cond(running.is_some() || refusal.is_some());
    if ui.button(format!("{} Convert to Halo 2", icon::PACKAGE)) {
        actions.push(Action::ConvertHalo1(app.convert_sandbox));
    }
    d.end();
    match refusal {
        Some(why) => {
            ui.set_item_tooltip(why);
            ui.text_wrapped(why);
        }
        None => ui.set_item_tooltip(format!(
            "Writes <data>/maps/h1_{name}/ and opens the new map."
        )),
    }
    if let Some(r) = running {
        ui.text_colored(
            theme::ACCENT,
            format!(
                "Building h1_{} ({} sandbox)... {:.0} s",
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
