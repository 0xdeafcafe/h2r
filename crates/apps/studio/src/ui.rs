//! The Dear ImGui shell: menu, tool bar, docked workspaces, status and
//! modals. Tool windows keep their own dock identities. Document edits
//! go back to [`App`] as [`Action`]s.

mod bar;
pub mod browser;
mod doc_tabs;
mod doc_header;
#[cfg(test)]
pub(crate) use doc_header::crumbs as doc_header_crumbs;
mod game_view;
mod layout;
mod toolbar;
pub(crate) mod inspector;
mod layers;
pub mod map_list;
mod modeling;
mod outliner;
mod palette;
mod play;
mod playlists;
pub mod playtest;
pub mod problems;
mod preview_switch;
mod sound;
pub(crate) mod sky;
pub(crate) mod anim;
mod voices;
pub mod scripts;
pub mod tags;
mod ui_screens;
mod hud;
mod image_gen;
mod textures;
mod variants;
/// The tag workspace's start page and status-line bar.
mod workspace;
/// The game type script window (`app::rules_edit`).
mod game_script;

use std::path::PathBuf;

use dear_imgui_rs::{
    Condition, DockLayout, DockLayoutApply, Key, MouseButton, StyleColor, StyleVar, Ui,
    WindowFlags, WindowHoveredFlags,
};
use editor_core::Status;
use scenario_edit::BlockKind;

use crate::app::commands::MenuCommand;
use crate::app::{Action, Answer, App, EditChoice, NavAction, SaveTarget, Surface};
use crate::theme::{self, Fonts, icon};
use crate::viewport::{GizmoOp, Gpu, ViewportView};
use layout::Keys;

const UNSAVED: &str = "Unsaved changes###unsaved";
const EDIT_TARGET: &str = "Edit target###edit-target";
const OPEN_PACKAGE: &str = "Open package###open-package";

/// A tab of the Map workspace's left dock node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MapTab {
    Browser,
    Scene,
    Tags,
    Scripts,
}

impl MapTab {
    /// Frames a tab is asked for before giving up (a collapsed or hidden
    /// floating window never draws; asking forever would take focus from
    /// every other window).
    const TRIES: u8 = 6;
}

/// The studio UI.
pub struct Studio {
    keys: Keys,
    layout: DockLayout,
    fonts: Fonts,
    pub viewport: ViewportView,
    show_open_package: bool,
    palette: palette::PaletteUi,
    reset_layout: bool,
    focus_game: bool,
    /// The Map tab asked to the front, with the frames left to ask.
    want_tab: Option<(MapTab, u8)>,
    previous_surface: Option<Surface>,
    previous_map: Option<String>,
    hint: Option<String>,
    /// Log lines already seen (to scroll to new ones).
    log_seen: usize,
    /// The trigger volume the Scene tab last brought into view.
    volume_shown: Option<usize>,
    /// Native file dialogs. Off in headless renders and tests.
    pub dialogs: bool,
    pub variants: variants::VariantsUi,
    playlists: playlists::PlaylistsUi,
    pub ui_screens: ui_screens::UiScreensUi,
    pub hud: hud::HudUi,
    pub textures: textures::TexturesUi,
    inspector: inspector::InspectorState,
    modeling: modeling::ModelingPanel,
    /// Play and pair (the Game panel; `crate::play`).
    pub play: play::PlayUi,
    /// The Tags tab's UI state, including its field panel.
    pub tags: tags::TagsUi,
    /// The Scripts tab's searches.
    pub scripts: scripts::ScriptsUi,
    /// The browser panel's state.
    pub browser: browser::BrowserUi,
    /// The Problems panel's state.
    pub problems: problems::ProblemsUi,
    /// The map grid's pictures.
    pub map_list: map_list::MapListUi,
    /// The Sound window over the Tags tool's selected `snd!`.
    sound: sound::SoundUi,
    /// The Animation window over the Tags tool's selected `jmad`.
    anim: anim::AnimUi,
    /// The voice document (`Doc::Voice`): the announcer and the clones.
    voices: voices::VoicesUi,
    /// The tag workspace's start page and status-line bar
    /// (`app::workspace`).
    workspace: workspace::WorkspaceUi,
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
            palette: Default::default(),
            reset_layout: false,
            focus_game: false,
            want_tab: None,
            previous_surface: None,
            previous_map: None,
            hint: None,
            log_seen: 0,
            volume_shown: None,
            dialogs: true,
            variants: variants::VariantsUi::default(),
            playlists: playlists::PlaylistsUi::default(),
            ui_screens: ui_screens::UiScreensUi::default(),
            hud: hud::HudUi::default(),
            textures: textures::TexturesUi::default(),
            inspector: inspector::InspectorState::default(),
            modeling: modeling::ModelingPanel::default(),
            play: play::PlayUi::default(),
            tags: tags::TagsUi::default(),
            scripts: scripts::ScriptsUi::default(),
            browser: browser::BrowserUi::default(),
            problems: problems::ProblemsUi::default(),
            map_list: map_list::MapListUi::default(),
            sound: sound::SoundUi::new(),
            anim: anim::AnimUi::default(),
            voices: voices::VoicesUi::default(),
            workspace: workspace::WorkspaceUi::default(),
        }
    }

    /// Build one frame of UI and run the actions it produced.
    pub fn frame(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>) {
        app.poll_convert();
        app.poll_cards();
        app.poll_voice_batch();
        app.poll_bot_nav();
        app.poll_ground();
        // The workspace's background extraction, before the status bar
        // reads its progress.
        app.poll_workspace();
        // An outside editor's saves of the game type script, before the
        // game link sends the reload they queue.
        game_script::poll(app);
        self.play.update(app);
        // What the agent API opened or closed since the last frame.
        app.sync_tabs();
        let mut actions = Vec::new();
        self.shortcuts(ui, app, &mut actions);
        if !self.palette.is_open(ui) {
            self.play.shortcuts(ui);
        }
        self.menu_bar(ui, app, &mut actions);
        // Asked for by the frame before (once its tabs exist).
        self.focus_game |= std::mem::take(&mut app.preview.focus_game);
        let focus_game = self.focus_game;
        bar::draw(ui, app, &mut self.play, &self.fonts, &mut actions);
        let reset = self.dockspace(ui);
        if !app.surface().is_map() {
            self.want_tab = None;
        }
        self.hint = None;
        match app.surface() {
            Surface::Scene | Surface::Tags | Surface::Scripts => {
                let surface = app.surface();
                let changed = self.previous_surface != Some(surface);
                let open = map_is_visible(app);
                let map = app.loaded_map().or_else(|| app.loaded_halo1());
                let opened = self.previous_surface.is_some() && self.previous_map.as_deref() != map;
                let focus_tags = surface == Surface::Tags && (changed || reset);
                let focus_scripts = surface == Surface::Scripts && (changed || reset);
                let focus_scene = surface == Surface::Scene
                    && open
                    && (reset
                        || opened
                        || matches!(self.previous_surface, Some(Surface::Tags | Surface::Scripts)));
                let focus_browser = !open
                    && !focus_tags
                    && !focus_scripts
                    && (reset || (changed && self.previous_surface.is_some()) || opened);
                let asked = [
                    (focus_browser, MapTab::Browser),
                    (focus_scene, MapTab::Scene),
                    (focus_tags, MapTab::Tags),
                    (focus_scripts, MapTab::Scripts),
                ]
                .into_iter()
                .find_map(|(focus, tab)| focus.then_some(tab));
                if let Some(tab) = asked {
                    self.want_tab = Some((tab, MapTab::TRIES));
                }
                // A focus request only reaches the dock's tab bar on the next
                // frame, and only while nothing else takes focus after it:
                // the windows that appear with a new layout take it, and Dear
                // ImGui selects the last new tab of a node when its windows
                // first dock. So the wanted tab is asked for every frame until
                // it shows in a frame that did not apply a layout. A docked
                // window built in the frame it is focused runs its contents
                // even behind another tab, hidden, so "shows" is the window's
                // own Hidden flag, not whether its contents ran.
                let want = self.want_tab.map(|(tab, _)| tab);
                self.browser_window(ui, app, want == Some(MapTab::Browser), &mut actions);
                self.outliner_window(ui, app, want == Some(MapTab::Scene), &mut actions);
                self.tags_window(ui, app, want == Some(MapTab::Tags), &mut actions);
                self.scripts_window(ui, app, want == Some(MapTab::Scripts), &mut actions);
                if let Some((tab, tries)) = self.want_tab {
                    let key = match tab {
                        MapTab::Browser => &self.keys.browser,
                        MapTab::Scene => &self.keys.outliner,
                        MapTab::Tags => &self.keys.tags,
                        MapTab::Scripts => &self.keys.scripts,
                    };
                    let done = layout::shows(key) && !reset;
                    self.want_tab = (!done && tries > 1).then_some((tab, tries - 1));
                }
                self.viewport_window(ui, app, gpu, &mut actions);
                self.inspector_window(ui, app, gpu, &mut actions);
            }
            Surface::Variants => self.variant_windows(ui, app, &mut actions),
            Surface::Playlists => self.playlist_windows(ui, app, &mut actions),
            Surface::UiScreens => self.ui_screen_windows(ui, app, gpu, &mut actions),
            Surface::Hud => self.hud_windows(ui, app, gpu, &mut actions),
            Surface::Textures => self.textures_windows(ui, app, gpu, &mut actions),
        }
        self.play.window(ui, app, &self.keys.game, gpu);
        self.previous_surface = Some(app.surface());
        self.previous_map = app.loaded_map().or_else(|| app.loaded_halo1()).map(str::to_owned);
        self.play.pending_window(ui, app, &self.keys.pending_edits);
        self.log_window(ui, app);
        self.problems_window(ui, app, &mut actions);
        ui.window(&self.keys.playtest).build(|| playtest::draw(ui, app, &mut actions));
        if app.surface().is_map() {
            ui.window(&self.keys.bytes).build(|| {
                tags::hex(ui, app, &self.tags, &self.fonts, gpu, &mut self.map_list, &mut actions);
            });
        }
        // The seam for sound audition until the tag view tabs exist
        // (docs/editor/rich-editors.md, "One seam for every rich view").
        sound::window(ui, app, gpu, &mut self.sound);
        // The same seam for model poses (that page's slice M1).
        anim::window(ui, app, gpu, &mut self.anim);
        voices::window(ui, app, &mut self.voices, self.dialogs);
        // The tag workspace's first-start page (a floating window; the
        // shell is another lane's).
        workspace::start_page(ui, app, &mut self.workspace, &self.fonts, self.dialogs);
        // Image generation: the Generate window over any tool, opened from
        // a bitmap's slot (app::image_gen).
        image_gen::window(ui, app, self.dialogs, &mut actions);
        // After the Map tab: Game in the same frame would take its focus.
        if focus_game && self.want_tab.is_none() {
            self.focus_game = false;
            ui.set_window_focus(Some("###game"));
        }
        self.status_bar(ui, app);
        self.modals(ui, app, &mut actions);
        actions.extend(self.palette.draw(ui, app));
        dispatch_actions(app, actions);
    }

    fn shortcuts(&mut self, ui: &Ui, app: &App, actions: &mut Vec<Action>) {
        if self.palette.is_open(ui) {
            return;
        }
        let io = ui.io();
        let cmd = io.key_ctrl() || io.key_super();
        if cmd && ui.is_key_pressed(Key::P)
            && app.pending().is_none()
            && app.edits.picker.is_none()
            && !ui.is_popup_open(OPEN_PACKAGE)
        {
            self.palette.open();
            return;
        }
        if io.want_text_input() {
            return;
        }
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
            if shift && app.surface().is_map() {
                self.save_map_dialog(app, actions);
            } else if !shift {
                actions.extend(app.front_doc().and_then(|doc| app.doc_save_action(doc)));
            }
        }
        if cmd && ui.is_key_pressed(Key::Q) {
            actions.push(Action::Quit);
        }
        if cmd && ui.is_key_pressed(Key::Enter) {
            actions.push(Action::PreviewInGame(None));
        }
        if !app.surface().is_map() {
            return;
        }
        if app.session.is_open() {
            if (io.key_alt() && ui.is_key_pressed(Key::LeftArrow))
                || ui.is_mouse_clicked(MouseButton::Extra1)
            {
                actions.push(Action::Navigate(NavAction::Back));
            }
            if (io.key_alt() && ui.is_key_pressed(Key::RightArrow))
                || ui.is_mouse_clicked(MouseButton::Extra2)
            {
                actions.push(Action::Navigate(NavAction::Forward));
            }
        }
        if app.surface() == Surface::Scene && cmd && ui.is_key_pressed(Key::D) {
            actions.push(Action::Duplicate);
        }
        if app.surface() == Surface::Scene
            && (ui.is_key_pressed(Key::Delete) || ui.is_key_pressed(Key::Backspace))
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
        if self.dialogs {
            open_source_dialog(actions);
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
            .set_title("Export map")
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
            let map_tool = app.surface().is_map();
            let save = app.front_doc().and_then(|doc| app.doc_save_action(doc));
            if ui.menu_item_enabled_selected_with_shortcut("Save", "Cmd+S", false, save.is_some()) {
                actions.extend(save);
            }
            if ui.menu_item_enabled_selected_with_shortcut(
                "Export map...",
                if map_tool { "Shift+Cmd+S" } else { "" },
                false,
                open,
            ) {
                self.save_map_dialog(app, actions);
            }
            if ui.menu_item_enabled_selected_no_shortcut(
                format!("{} Open package...", icon::PACKAGE_OPEN),
                false,
                open,
            ) {
                self.show_open_package = true;
            }
            ui.separator();
            menu_action(ui, app, MenuCommand::Quit, None, false, actions);
        }
        if let Some(_m) = ui.begin_menu("Edit") {
            if ui.menu_item_with_shortcut("Go to anything...", "Cmd+P") {
                self.palette.open();
            }
            ui.separator();
            menu_action(ui, app, MenuCommand::Undo, Some(icon::UNDO), false, actions);
            menu_action(ui, app, MenuCommand::Redo, Some(icon::REDO), false, actions);
            ui.separator();
            menu_action(ui, app, MenuCommand::Duplicate, Some(icon::COPY), false, actions);
            menu_action(ui, app, MenuCommand::Delete, Some(icon::TRASH), false, actions);
            ui.separator();
            menu_action(
                ui,
                app,
                MenuCommand::AllowGrowth,
                None,
                app.session.allow_growth(),
                actions,
            );
            if ui.is_item_hovered() {
                ui.tooltip_text(scenario_edit::GROWTH_REFUSAL);
            }
            menu_action(
                ui,
                app,
                MenuCommand::AllowUnverifiedGeometry,
                None,
                app.modeling.allow_unverified,
                actions,
            );
            if ui.is_item_hovered() {
                ui.tooltip_text(crate::app::modeling::UNVERIFIED_REFUSAL);
            }
        }
        if let Some(_m) = ui.begin_menu("View") {
            menu_action(ui, app, MenuCommand::FrameAll, Some(icon::MAXIMIZE), false, actions);
            menu_action(ui, app, MenuCommand::FocusSelected, Some(icon::FOCUS), false, actions);
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
            menu_action(ui, app, MenuCommand::Overlay, None, app.overlay_shown(), actions);
            ui.separator();
            if ui.menu_item("Reset layout") {
                self.reset_layout = true;
            }
        }
        if let Some(_m) = ui.begin_menu("Preview") {
            preview_switch::menu(ui, app, actions);
        }
        self.play.menu(ui, app);
    }

    /// The dockspace host between the bar and the status bar. Returns
    /// whether the default layout was applied this frame.
    fn dockspace(&mut self, ui: &Ui) -> bool {
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
        let reset = std::mem::take(&mut self.reset_layout);
        let apply = if reset {
            DockLayoutApply::Replace
        } else {
            DockLayoutApply::IfMissing
        };
        let fresh = !layout::has_dock_node(root.raw());
        if self.previous_surface.is_none() && !fresh && apply == DockLayoutApply::IfMissing {
            self.keys.seed_saved_layout();
        }
        let layout = &self.layout;
        let mut result = Ok(root);
        ui.window("##dock-host")
            .flags(flags)
            .position([pos[0], pos[1] + theme::BAR], Condition::Always)
            .size(
                [size[0], (size[1] - theme::BAR - theme::STATUS_BAR).max(1.0)],
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
        } else if reset || fresh {
            // ImGui selects the last new tab when the windows first dock.
            // Select Game once both tabs have appeared in the default layout.
            self.focus_game = true;
        }
        fresh || apply == DockLayoutApply::Replace
    }

    /// The File menu's items for the variant and playlist tools.
    fn tool_file_menu(&mut self, ui: &Ui, app: &App, actions: &mut Vec<Action>) {
        match app.surface() {
            Surface::Hud => {
                let can = app.hud.session.as_ref().is_some_and(|s| !s.edited().is_empty());
                if ui.menu_item_enabled_selected_with_shortcut(
                    format!("{} Export map with HUD edits...", icon::SAVE),
                    "",
                    false,
                    can,
                ) && let Some(p) = hud::save_dialog(self.dialogs, app)
                {
                    actions.push(Action::HudSaveAs(p));
                }
            }
            Surface::Textures => textures::file_menu(ui, app, self.dialogs, actions),
            Surface::Scene | Surface::UiScreens | Surface::Tags | Surface::Scripts => {}
            Surface::Variants => {
                if ui.menu_item(format!("{} Import variant...", icon::PACKAGE_OPEN))
                    && let Some(p) = variants::import_dialog(self.dialogs)
                {
                    actions.push(Action::ImportVariantFile(p));
                }
            }
            Surface::Playlists => {
                menu_action(ui, app, MenuCommand::NewPlaylist, Some(icon::PLUS), false, actions);
                menu_action(ui, app, MenuCommand::DefaultPlaylist, None, false, actions);
                if ui.menu_item(format!("{} Open playlist file...", icon::FOLDER_OPEN))
                    && let Some(p) = playlists::open_dialog(self.dialogs)
                {
                    actions.push(Action::OpenHopper(p));
                }
                let open = app.hoppers.is_open();
                if ui.menu_item_enabled_selected_with_shortcut(
                    "Export playlist file...",
                    "",
                    false,
                    open,
                ) && let Some(p) = playlists::save_dialog(self.dialogs, app)
                {
                    actions.push(Action::ExportPlaylist(p));
                }
            }
        }
    }

    fn variant_windows(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let st = &mut self.variants;
        let keys = &self.keys.variants;
        ui.window(&keys.list)
            .build(|| variants::list(ui, app, st, fonts, actions));
        ui.window(&keys.center)
            .build(|| {
                if app.front_doc() != Some(crate::app::Doc::Rules) {
                    doc_header::draw(ui, app, actions, &mut self.browser);
                }
                variants::settings(ui, app, st, fonts, actions);
            });
        ui.window(&keys.inspector)
            .build(|| variants::inspector(ui, app, st, fonts, self.dialogs, actions));
        // Docked beside the settings, but never takes the tab from them
        // when it first appears; it is opened by picking its tab.
        ui.window(&self.keys.rules)
            .flags(dear_imgui_rs::WindowFlags::NO_FOCUS_ON_APPEARING)
            .build(|| {
                if app.front_doc() == Some(crate::app::Doc::Rules) {
                    doc_header::draw(ui, app, actions, &mut self.browser);
                }
                game_script::window(ui, app, fonts, self.dialogs);
            });
        // Opening a script (from its toolbar or `rules.open`) picks its tab,
        // asked each frame until the window's body has drawn: a request
        // made before the dock layout exists is lost.
        if app.rules.show {
            let name = format!("###{}", self.keys.rules.stable_id());
            ui.set_window_focus(Some(name.as_str()));
        }
    }

    fn playlist_windows(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let dialogs = self.dialogs;
        let keys = &self.keys.playlists;
        ui.window(&keys.list)
            .build(|| playlists::list(ui, app, fonts, dialogs, actions));
        ui.window(&keys.center)
            .build(|| {
                doc_header::draw(ui, app, actions, &mut self.browser);
                playlists::entries(ui, app, fonts, actions);
            });
        let st = &mut self.playlists;
        ui.window(&keys.inspector)
            .build(|| playlists::inspector(ui, app, st, fonts, dialogs, actions));
    }

    fn ui_screen_windows(
        &mut self,
        ui: &Ui,
        app: &mut App,
        gpu: &mut Gpu<'_>,
        actions: &mut Vec<Action>,
    ) {
        let fonts = &self.fonts;
        let st = &mut self.ui_screens;
        let keys = &self.keys.screens;
        ui.window(&keys.list)
            .build(|| ui_screens::tree(ui, app, st, fonts, actions));
        ui.window(&keys.center)
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(|| {
                doc_header::draw(ui, app, actions, &mut self.browser);
                ui_screens::preview(ui, app, st, gpu, actions);
            });
        ui.window(&keys.inspector)
            .build(|| ui_screens::inspector(ui, app, st, fonts, self.dialogs, actions));
    }

    fn hud_windows(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let st = &mut self.hud;
        let keys = &self.keys.hud;
        ui.window(&keys.list)
            .build(|| hud::tree(ui, app, st, fonts, actions));
        ui.window(&keys.center)
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(|| {
                doc_header::draw(ui, app, actions, &mut self.browser);
                hud::preview(ui, app, st, fonts, gpu, actions);
            });
        ui.window(&keys.inspector)
            .build(|| hud::inspector(ui, app, st, fonts, self.dialogs, actions));
    }

    fn textures_windows(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
        let fonts = &self.fonts;
        let st = &mut self.textures;
        let keys = &self.keys.textures;
        ui.window(&keys.list)
            .build(|| textures::list(ui, app, st, actions));
        ui.window(&keys.center)
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(|| {
                doc_header::draw(ui, app, actions, &mut self.browser);
                textures::canvas(ui, app, st, fonts, gpu, actions);
            });
        ui.window(&keys.inspector)
            .build(|| textures::inspector(ui, app, st, self.dialogs, actions));
    }

    fn tags_window(
        &mut self,
        ui: &Ui,
        app: &mut App,
        focus: bool,
        actions: &mut Vec<Action>,
    ) {
        app.sync_tags();
        let fonts = &self.fonts;
        let st = &mut self.tags;
        let window = ui.window(&self.keys.tags)
            .flags(WindowFlags::NO_FOCUS_ON_APPEARING)
            .focused(focus);
        window.build(|| {
            if ui.is_window_focused()
                && app.surface() != Surface::Tags
                && self.previous_surface == Some(app.surface())
            {
                actions.push(Action::Show(Surface::Tags));
            }
            tags::tree(ui, app, st, fonts, actions);
        });
    }

    fn scripts_window(
        &mut self,
        ui: &Ui,
        app: &mut App,
        focus: bool,
        actions: &mut Vec<Action>,
    ) {
        let fonts = &self.fonts;
        let st = &mut self.scripts;
        let dialogs = self.dialogs;
        let previous = self.previous_surface;
        let window = ui.window(&self.keys.scripts)
            .flags(WindowFlags::NO_FOCUS_ON_APPEARING)
            .focused(focus);
        window.build(|| {
            if ui.is_window_focused()
                && app.surface() != Surface::Scripts
                && previous == Some(app.surface())
            {
                actions.push(Action::Show(Surface::Scripts));
            }
            // Read only while the tab shows: the tables are read when a
            // map opens and this tab is first drawn for it.
            app.sync_scripts();
            scripts::list(ui, app, st, fonts, dialogs, actions);
        });
    }

    /// The browser: every tag of the tree by path, the packages and Live
    /// (`ui/browser.rs`), beside the Open and Folder buttons that choose
    /// the source the tree's maps are read from.
    fn browser_window(&mut self, ui: &Ui, app: &App, focus: bool, actions: &mut Vec<Action>) {
        let mut st = std::mem::take(&mut self.browser);
        ui.window(&self.keys.browser).focused(focus).build(|| {
            if ui.button(format!("{} Open...", icon::FOLDER_OPEN)) {
                self.open_source_dialog(actions);
            }
            ui.same_line();
            if ui.button(format!("{} Folder...", icon::FOLDER)) {
                self.open_folder_dialog(actions);
            }
            if let Some(src) = app.source() {
                let kind = match src {
                    editor_core::MapSource::Disc(_) => icon::DISC,
                    editor_core::MapSource::Loose(_) => icon::FILE,
                    editor_core::MapSource::Folder(_) => icon::FOLDER,
                };
                ui.text_disabled(format!("{kind} {}", src.path().display()));
            }
            browser::draw(ui, app, &mut st, actions);
        });
        self.browser = st;
    }

    fn outliner_window(
        &mut self,
        ui: &Ui,
        app: &mut App,
        focus: bool,
        actions: &mut Vec<Action>,
    ) {
        let window = ui.window(&self.keys.outliner)
            .flags(WindowFlags::NO_FOCUS_ON_APPEARING)
            .focused(focus);
        window.build(|| {
            if ui.is_window_focused()
                && matches!(app.surface(), Surface::Tags | Surface::Scripts)
                && self.previous_surface == Some(app.surface())
            {
                actions.push(Action::Show(Surface::Scene));
            }
            if ui.is_window_hovered_with_flags(WindowHoveredFlags::CHILD_WINDOWS) && map_is_visible(app) {
                self.hint = Some("Click to select · Cmd/Ctrl toggles · Shift selects a range".into());
            }
            if let Some(view) = app.halo1_view() {
                halo1_outliner(ui, app, view, actions);
                return;
            }
            if !app.session.is_open() {
                theme::empty_card(ui, &self.fonts, "Open a map to see its scene", || {});
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
            let ground_state = app.ground_state();
            let session = &app.session;
            let Some(scenario) = session.scenario() else {
                return;
            };
            let io = ui.io();
            let (toggle, range) = (io.key_ctrl() || io.key_super(), io.key_shift());
            let mut place = None;
            let mut label_place = None;
            theme::panel_body(ui, "##outliner-body", || {
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
                        let issue = inspector::issues(app).placement_row(kind, i);
                        if inspector::marked_row(ui, label, issue, |label| {
                            ui.selectable_config(format!("{label}###{kind:?}-{i}"))
                                .selected(session.is_selected((kind, i)))
                                .allow_double_click(true)
                                .build()
                        }) {
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
                volume_outliner(ui, session, &filter, &mut self.volume_shown, actions);
                label_place = ground_outliner(ui, session, &filter, ground_state, actions);
        });
                if place.is_some() {
                    self.viewport.placing = place;
                    self.viewport.placing_label = None;
                }
                if label_place.is_some() {
                    self.viewport.placing_label = label_place;
                    self.viewport.placing = None;
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
        let fonts = &self.fonts;
        let dialogs = self.dialogs;
        ui.window(&self.keys.viewport)
            .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
            .build(|| {
                doc_header::draw(ui, app, actions, &mut self.browser);
                if map_is_visible(app) {
                    toolbar::draw(ui, app, viewport, actions, &mut self.hint);
                    viewport.draw(ui, app, gpu, actions);
                    play.overlay(ui, app, viewport.shown().map(|(r, _)| r));
                    if viewport.hovered() || viewport.looking() {
                        self.hint = Some(match (viewport.placing, viewport.placing_label) {
                            (Some((kind, _)), _) => format!("Click the level to place {kind:?} · Esc cancels"),
                            (None, Some(label)) => {
                                format!("Click the level to place {} · Esc cancels", label.text())
                            }
                            (None, None) => "RMB look and fly · Alt+LMB orbit · MMB pan · Wheel dolly".into(),
                        });
                    }
                } else if app.source().is_some() || app.halo1_source().is_some() {
                    map_list::source_grid(ui, app, &mut self.map_list, gpu, fonts, actions);
                } else {
                    theme::empty_card(ui, fonts, "Open a map to get started", || {
                        if ui.button(format!("{} Open a disc or map...", icon::FOLDER_OPEN))
                            && dialogs
                        {
                            open_source_dialog(actions);
                        }
                    });
                }
            });
        padding.pop();
    }

    fn inspector_window(
        &mut self,
        ui: &Ui,
        app: &mut App,
        gpu: &mut Gpu<'_>,
        actions: &mut Vec<Action>,
    ) {
        let mut state = std::mem::take(&mut self.inspector);
        // The closure borrows the whole shell, so the key is its own value.
        let key = self.keys.inspector.clone();
        ui.window(&key).build(|| {
            let link = self.play.play.displayed_link(app);
            if matches!(
                link.state,
                crate::play::LinkState::Editing { .. } | crate::play::LinkState::Blocked { .. }
            ) {
                theme::link_badge(ui, link.tone, "Live");
                if let Some(why) = &link.why {
                    ui.set_item_tooltip(why);
                }
            }
            if app.surface() == Surface::Tags {
                tags::fields(
                    ui,
                    app,
                    &mut self.tags,
                    &self.fonts,
                    self.dialogs,
                    gpu,
                    &mut self.map_list,
                    actions,
                );
                return;
            }
            if app.surface() == Surface::Scripts {
                scripts::source(ui, app, &mut self.scripts, &self.fonts, actions);
                return;
            }
            if let Some(view) = app.halo1_view() {
                self.halo1_inspector(ui, app, view, actions);
                return;
            }
            if let Some(c) = app.compare() {
                compare_banner(ui, c);
            }
            inspector::draw(ui, &self.fonts, app, &mut state);
            modeling::section(ui, app, &mut self.modeling, actions);
        });
        self.inspector = state;
    }

    fn log_window(&mut self, ui: &Ui, app: &App) {
        ui.window(&self.keys.log).build(|| {
            theme::panel_body(ui, "##log-lines", || {
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

    /// The Problems panel (`ui/problems.rs`). A row that opens a package
    /// shows it in the browser.
    fn problems_window(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        if let Some(asset) = app.problems.reveal.take() {
            self.browser.reveal(&asset);
            ui.set_window_focus(Some("###browser"));
        }
        let mut st = std::mem::take(&mut self.problems);
        let focus = st.focus > 0;
        st.focus = st.focus.saturating_sub(1);
        ui.window(&self.keys.problems).focused(focus).build(|| {
            problems::draw(ui, app, &mut st, actions);
        });
        self.problems = st;
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
        let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
        let _border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
        let _padding = ui.push_style_var(StyleVar::WindowPadding([10.0, 6.0]));
        let _bg = ui.push_style_color(StyleColor::WindowBg, theme::STATUS_BG);
        ui.window("##status-bar")
            .flags(flags)
            .position(
                [pos[0], pos[1] + size[1] - theme::STATUS_BAR],
                Condition::Always,
            )
            .size([size[0], theme::STATUS_BAR], Condition::Always)
            .build(|| {
                let _font = ui.push_font(self.fonts.caption);
                let [x, y] = ui.cursor_screen_pos();
                let width = ui.content_region_avail()[0];
                let fps = format!("{:.0} fps", ui.io().framerate());
                let fps_start = (width - ui.calc_text_size(&fps)[0]).max(0.0);
                let message_end = (fps_start * 0.35).min(560.0);
                let (message, tone) = match app.status() {
                    Some(Status::Error(m)) => (format!("{} {m}", icon::ALERT), theme::ERROR),
                    Some(Status::Info(m)) => (format!("{} {m}", icon::CHECK), theme::TEXT),
                    None => ("Ready".into(), theme::DIM),
                };
                theme::clipped_text(ui, &message, tone, message_end - 8.0);
                let hint_start = message_end + 8.0;
                // The workspace's extraction bar takes the hint's slot
                // while it runs; the hint waits.
                let line = ui.text_line_height();
                if app.workspace.progress().is_some() {
                    workspace::status_progress(
                        ui,
                        app,
                        x + hint_start,
                        y,
                        fps_start - hint_start - 8.0,
                        line,
                    );
                } else if let Some(hint) = &self.hint
                    && fps_start - hint_start > 16.0
                {
                    ui.set_cursor_screen_pos([x + hint_start, y]);
                    theme::clipped_text(ui, hint, theme::DIM, fps_start - hint_start - 8.0);
                }
                ui.set_cursor_screen_pos([x + fps_start, y]);
                ui.text_colored(theme::DIM, fps);
                let draw = ui.get_window_draw_list();
                for rule in [message_end, fps_start - 8.0] {
                    let at = x + rule;
                    draw.add_line([at, y], [at, y + ui.text_line_height()], theme::RULE)
                        .build();
                }
            });
    }

    fn modals(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        // Unsaved changes.
        if app.pending().is_some() && app.edits.picker.is_none() && !ui.is_popup_open(UNSAVED) {
            ui.open_popup(UNSAVED);
        }
        if let Some(_m) = ui
            .begin_modal_popup_config(UNSAVED)
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            let what = match app.pending() {
                Some(Action::Quit) => "quitting",
                Some(
                    Action::SelectMap(_)
                    | Action::OpenMapPack(_)
                    | Action::SelectHalo1Map(_)
                    | Action::OpenAsset { .. },
                ) => {
                    "opening another map"
                }
                Some(Action::OpenPackageEntry(_)) => "opening another package",
                Some(Action::OpenSource(_)) => "opening another source",
                Some(Action::OpenHopper(_)) => "opening another playlist file",
                Some(Action::NewHopper | Action::NewDefaultHopper) => {
                    "starting a new playlist file"
                }
                Some(Action::EditEntryVariant(..)) => "opening the entry's variant",
                _ => "opening another variant",
            };
            let tool = app.pending_surface().unwrap_or(Surface::Scene);
            let who = match tool {
                Surface::Scene => app.loaded_map().unwrap_or("The map").to_owned(),
                Surface::Variants => "The open game variant".to_owned(),
                Surface::Playlists => "The playlist file".to_owned(),
                Surface::UiScreens => "The UI screen".to_owned(),
                Surface::Hud => "The HUD map".to_owned(),
                Surface::Tags => "The map's tags".to_owned(),
                Surface::Scripts => "The map's scripts".to_owned(),
                Surface::Textures => "The map's bitmaps".to_owned(),
            };
            ui.text(format!("{} {who} has unsaved edits.", icon::ALERT));
            ui.text_disabled(format!("Save them before {what}?"));
            ui.spacing();
            let mut answer = None;
            match tool {
                Surface::Scene => {
                    if matches!(app.save_target(), SaveTarget::File(_) | SaveTarget::Package(_) | SaveTarget::EditTarget(Some(_))) {
                        if ui.button(format!("{} Save", icon::SAVE)) {
                            answer = Some(Answer::Save);
                        }
                    } else if ui.button(format!("{} Export map...", icon::SAVE))
                        && let Some(p) = self.save_path_dialog(app)
                    {
                        answer = Some(Answer::SaveAs(p));
                    }
                }
                Surface::Variants => {
                    let can = app.variants.as_ref().is_some_and(|e| !e.is_read_only());
                    let d = ui.begin_disabled_with_cond(!can);
                    if ui.button(format!("{} Save", icon::SAVE)) {
                        answer = Some(Answer::Save);
                    }
                    d.end();
                }
                Surface::Playlists => {
                    if ui.button(format!("{} Save", icon::SAVE)) {
                        answer = Some(Answer::Save);
                    }
                }
                Surface::Hud => {
                    if ui.button(format!("{} Save as...", icon::SAVE))
                        && let Some(p) = hud::save_dialog(self.dialogs, app)
                    {
                        answer = Some(Answer::SaveAs(p));
                    }
                }
                Surface::UiScreens | Surface::Tags | Surface::Scripts | Surface::Textures => {}
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
                if app.pending().is_none() || app.edits.picker.is_some() {
                    ui.close_current_popup();
                }
            }
        }

        // The first edit's target: a local package, or a new one.
        if app.edits.picker.is_some() && !ui.is_popup_open(EDIT_TARGET) {
            ui.open_popup(EDIT_TARGET);
        }
        if let Some(_m) = ui
            .begin_modal_popup_config(EDIT_TARGET)
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            match app.edits.picker.take() {
                None => ui.close_current_popup(),
                Some(mut picker) => {
                    ui.text("Which package gets your edits?");
                    ui.spacing();
                    let mut answer = None;
                    for (id, name) in app.local_packages() {
                        if ui.button(format!("{id}##edit-target-{id}")) {
                            answer = Some(EditChoice::Existing(id.clone()));
                        }
                        ui.same_line();
                        ui.text_disabled(name);
                    }
                    ui.spacing();
                    ui.set_next_item_width(240.0);
                    ui.input_text("##new-package-name", &mut picker.new_name)
                        .hint("New package name")
                        .build();
                    ui.same_line();
                    let d = ui.begin_disabled_with_cond(picker.new_name.trim().is_empty());
                    if ui.button(format!("{} New package...", icon::PACKAGE)) {
                        answer = Some(EditChoice::New(picker.new_name.clone()));
                    }
                    d.end();
                    ui.spacing();
                    if ui.button("Cancel") || ui.is_key_pressed(Key::Escape) {
                        answer = Some(EditChoice::Cancel);
                    }
                    match answer {
                        Some(choice) => {
                            ui.close_current_popup();
                            actions.push(Action::ChooseEditTarget(choice));
                        }
                        None => app.edits.picker = Some(picker),
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

/// Dispatch the window's frame queue in order, including the placement inspector switch.
pub(crate) fn dispatch_actions(app: &mut App, actions: Vec<Action>) {
    for a in actions {
        if app.surface() == Surface::Tags
            && matches!(
                a,
                Action::Select(_)
                    | Action::SelectMany(_)
                    | Action::ToggleSelect(_)
                    | Action::SelectVolume(_)
                    | Action::SelectSurface(_)
                    | Action::SelectNav(_)
            )
        {
            app.dispatch(Action::Show(Surface::Scene));
        }
        app.dispatch(a);
    }
}

fn menu_action(
    ui: &Ui,
    app: &App,
    command: MenuCommand,
    icon: Option<&str>,
    selected: bool,
    actions: &mut Vec<Action>,
) {
    let item = command.item(app);
    let label = match icon {
        Some(icon) => format!("{icon} {}", item.label),
        None => item.label.to_owned(),
    };
    if ui.menu_item_enabled_selected_with_shortcut(
        label,
        item.shortcut,
        selected,
        item.action.is_some(),
    ) {
        actions.extend(item.action);
    }
}

fn map_is_visible(app: &App) -> bool {
    app.session.is_open() || app.session.reference().is_some()
}

fn open_source_dialog(actions: &mut Vec<Action>) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title("Open a disc image or map")
        .add_filter("Disc image or map", &["iso", "xiso", "map"])
        .pick_file()
    {
        actions.push(Action::OpenSource(path));
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

/// The Volumes group's menu: add a trigger volume, or make a kill volume,
/// at the camera focus.
fn volume_menu(ui: &Ui, session: &scenario_edit::Session, actions: &mut Vec<Action>) {
    use scenario_edit::volumes::VolumeKind;
    let trigger = session.volume_add_refusal(VolumeKind::Trigger);
    let kill = trigger
        .clone()
        .or_else(|| session.volume_add_refusal(VolumeKind::Kill));
    for (label, refusal, kill) in [
        (
            format!("{} Add trigger volume at the camera focus", icon::PLUS),
            trigger,
            false,
        ),
        (
            format!("{} Make kill volume at the camera focus", icon::PLUS),
            kill,
            true,
        ),
    ] {
        if ui.menu_item_enabled_selected_no_shortcut(label, false, refusal.is_none()) {
            actions.push(Action::AddVolume { frame: None, kill });
        }
        if let Some(why) = refusal {
            ui.set_item_tooltip(why);
        }
    }
}

/// The Scene tab's Volumes group: every trigger volume, with what names
/// it. A newly selected volume (a viewport
/// click, say) opens the group and scrolls its row into view;
/// `shown_volume` keeps which one last did.
fn volume_outliner(
    ui: &Ui,
    session: &scenario_edit::Session,
    filter: &str,
    shown_volume: &mut Option<usize>,
    actions: &mut Vec<Action>,
) {
    let selected = session.selected_volume();
    let reveal = selected.is_some() && selected != *shown_volume;
    *shown_volume = selected;
    let Some(volumes) = session.volumes() else {
        return;
    };
    let total = volumes.triggers.len();
    let shown: Vec<usize> = (0..total)
        .filter(|&i| outliner::matches(filter, "Volumes", &volumes.caption(i), i))
        .collect();
    if !filter.is_empty() && shown.is_empty() {
        return;
    }
    let count = if filter.is_empty() {
        total.to_string()
    } else {
        format!("{} of {total}", shown.len())
    };
    let mut node = ui
        .tree_node_config("block-volumes")
        .label(format!("{}  Volumes  ({count})", icon::BOX))
        .span_avail_width(true);
    if !filter.is_empty() || reveal {
        node = node.opened(true, Condition::Always);
    }
    let node = node.push();
    if let Some(_p) = ui.begin_popup_context_item() {
        volume_menu(ui, session, actions);
    }
    let Some(_node) = node else {
        return;
    };
    for i in shown {
        let clicked = ui
            .selectable_config(format!("{}###volume-{i}", volumes.caption(i)))
            .selected(selected == Some(i))
            .allow_double_click(true)
            .build();
        if reveal && selected == Some(i) {
            ui.set_scroll_here_y(0.5);
        }
        if clicked {
            actions.push(Action::SelectVolume(Some(i)));
            if ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left) {
                actions.push(Action::FocusSelected);
            }
        }
    }
}

/// The Scene tab's AI and objectives group
/// (docs/game/game-types-without-carriers.md, "In Studio"): each ground
/// label with how many are placed, a **Place new with the mouse** row for
/// it, and the placed ones as rows that select their flag. **Build AI
/// ground** and where the ground stands are at its head. Answers the label
/// the next click is to place.
fn ground_outliner(
    ui: &Ui,
    session: &scenario_edit::Session,
    filter: &str,
    state: crate::app::ground::GroundState,
    actions: &mut Vec<Action>,
) -> Option<scenario_edit::LabelKind> {
    use scenario_edit::{BlockKind, LabelKind};
    let scenario = session.scenario()?;
    let labels = scenario.flag_labels();
    let shown: Vec<LabelKind> = LabelKind::ALL
        .into_iter()
        .filter(|k| outliner::matches(filter, "AI and objectives", k.title(), 0))
        .collect();
    if !filter.is_empty() && shown.is_empty() {
        return None;
    }
    let mut node = ui
        .tree_node_config("block-ground")
        .label(format!("{}  AI and objectives  ({})", icon::FLAG, labels.len()))
        .span_avail_width(true);
    if !filter.is_empty() {
        node = node.opened(true, Condition::Always);
    }
    let _node = node.push()?;
    let mut place = None;
    let building = state == crate::app::ground::GroundState::Building;
    let d = ui.begin_disabled_with_cond(building);
    if ui.button(format!("{} Build AI ground##build-ground", icon::FLAG)) {
        actions.push(Action::BuildGround);
    }
    d.end();
    ui.same_line();
    ui.text_disabled(match state {
        crate::app::ground::GroundState::NoMap => "no map",
        crate::app::ground::GroundState::Never => "not built",
        crate::app::ground::GroundState::Building => "building...",
        crate::app::ground::GroundState::Current => "built",
        crate::app::ground::GroundState::Stale(_) => "stale: build again",
        crate::app::ground::GroundState::Failed => "the build failed",
    });
    let flags = BlockKind::NetgameFlags;
    for kind in shown {
        let mine: Vec<usize> = labels
            .iter()
            .filter(|(_, l)| l.kind == kind)
            .map(|(i, _)| *i)
            .collect();
        let Some(_row) = ui
            .tree_node_config(format!("ground-{}", kind.text()))
            .label(format!("{}  ({})", kind.title(), mine.len()))
            .span_avail_width(true)
            .push()
        else {
            continue;
        };
        if ui
            .selectable_config(format!("Place new with the mouse##place-{}", kind.text()))
            .build()
        {
            place = Some(kind);
        }
        for i in mine {
            let name = match scenario.flag_label(i) {
                Some(l) if l.kind.has_phase() => format!("{} {i}, phase {}", kind.text(), l.phase),
                _ => format!("{} {i}", kind.text()),
            };
            if ui
                .selectable_config(format!("{name}###label-{i}"))
                .selected(session.is_selected((flags, i)))
                .allow_double_click(true)
                .build()
            {
                actions.push(Action::Select(Some((flags, i))));
                if ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left) {
                    actions.push(Action::FocusSelected);
                }
            }
        }
    }
    place
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
    theme::panel_body(ui, "##h1-outliner-body", || {
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
             instead.",
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
