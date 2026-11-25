//! Dock identities and the default layout shared by the tool workspaces.

use std::ffi::CString;

use dear_imgui_rs::{DockLayout, DockSplit, WindowKey, sys};

use crate::theme::icon;

pub(super) struct ToolWindows {
    pub list: WindowKey,
    pub center: WindowKey,
    pub inspector: WindowKey,
}

impl ToolWindows {
    fn new(list: (&str, &str), center: (&str, &str), inspector: &str) -> Self {
        Self {
            list: key(list.0, icon::TREE, list.1),
            center: key(center.0, icon::EYE, center.1),
            inspector: key(inspector, icon::SLIDERS, "Inspector"),
        }
    }
}

pub(super) struct Keys {
    pub browser: WindowKey,
    pub tags: WindowKey,
    pub scripts: WindowKey,
    pub outliner: WindowKey,
    pub viewport: WindowKey,
    pub inspector: WindowKey,
    pub log: WindowKey,
    pub problems: WindowKey,
    pub playtest: WindowKey,
    pub game: WindowKey,
    pub pending_edits: WindowKey,
    pub bytes: WindowKey,
    /// The game type script editor, beside the Variants tool's settings.
    pub rules: WindowKey,
    pub package: WindowKey,
    pub variants: ToolWindows,
    pub playlists: ToolWindows,
    pub screens: ToolWindows,
    pub hud: ToolWindows,
    pub textures: ToolWindows,
}

fn key(id: &str, icon: &str, title: &str) -> WindowKey {
    WindowKey::new(id, format!("{icon} {title}")).expect("a valid window key")
}

impl Keys {
    pub fn new() -> Self {
        Self {
            package: key("package", icon::PACKAGE, "Package"),
            browser: key("browser", icon::TREE, "Browser"),
            tags: key("tags", icon::BOX, "Tags"),
            scripts: key("scripts", icon::FILE, "Scripts"),
            outliner: key("outliner", icon::TREE, "Scene"),
            viewport: key("viewport", icon::EYE, "Viewport"),
            inspector: key("inspector", icon::SLIDERS, "Inspector"),
            log: key("log", icon::LOG, "Log"),
            problems: key("problems", icon::ALERT, "Problems"),
            playtest: key("playtest", icon::CROSSHAIR, "Playtest"),
            game: key("game", icon::ZAP, "Game"),
            pending_edits: key("pending_edits", icon::SLIDERS, "Pending"),
            bytes: key("tag-bytes", icon::INFO, "Data and bytes"),
            rules: key("rules-script", icon::FILE, "Game type script"),
            variants: ToolWindows::new(
                ("variants-list", "Variants"),
                ("variants-settings", "Settings"),
                "variants-inspector",
            ),
            playlists: ToolWindows::new(
                ("playlists-list", "Playlists"),
                ("playlists-entries", "Entries"),
                "playlists-inspector",
            ),
            screens: ToolWindows::new(
                ("screens-list", "Screens"),
                ("screens-preview", "Preview"),
                "screens-inspector",
            ),
            hud: ToolWindows::new(
                ("hud-list", "HUDs"),
                ("hud-preview", "HUD"),
                "hud-inspector",
            ),
            textures: ToolWindows::new(
                ("textures-list", "Bitmaps"),
                ("textures-canvas", "Canvas"),
                "textures-inspector",
            ),
        }
    }

    fn tools(&self) -> [&ToolWindows; 5] {
        [&self.variants, &self.playlists, &self.screens, &self.hud, &self.textures]
    }

    pub fn layout(&self) -> DockLayout {
        let tools = self.tools();
        DockLayout::split(
            DockSplit::Down,
            0.22,
            DockLayout::tabs([
                &self.game,
                &self.log,
                &self.problems,
                &self.playtest,
                &self.pending_edits,
                &self.bytes,
            ]),
            DockLayout::split(
                DockSplit::Left,
                0.22,
                DockLayout::tabs(
                    [&self.outliner, &self.tags, &self.scripts, &self.browser]
                        .into_iter()
                        .chain(tools.map(|tool| &tool.list)),
                ),
                DockLayout::split(
                    DockSplit::Right,
                    0.28,
                    DockLayout::tabs(
                        [&self.inspector]
                            .into_iter()
                            .chain(tools.map(|tool| &tool.inspector)),
                    ),
                    DockLayout::tabs(
                        [&self.viewport]
                            .into_iter()
                            .chain(tools.map(|tool| &tool.center))
                            .chain([&self.rules, &self.package]),
                    ),
                ),
            ),
        )
    }

    /// Give new windows a home when an existing INI has only the Map
    /// window settings. A saved window can be intentionally undocked.
    pub fn seed_saved_layout(&self) {
        let tools = self.tools();
        let anchors = [
            &self.outliner,
            &self.viewport,
            &self.inspector,
            &self.log,
            &self.game,
        ];
        if saved_dock(&window_name(&self.browser)).is_none() {
            retire_tools_leaf(anchors);
        }
        seed(
            &self.outliner,
            [&self.browser, &self.tags, &self.scripts]
                .into_iter()
                .chain(tools.map(|t| &t.list)),
        );
        seed(&self.viewport, tools.map(|t| &t.center).into_iter().chain([&self.rules, &self.package]));
        seed(&self.inspector, tools.map(|t| &t.inspector));
        seed(
            &self.log,
            [&self.game, &self.problems, &self.playtest, &self.pending_edits, &self.bytes],
        );
    }
}

pub(super) fn has_dock_node(id: u32) -> bool {
    // SAFETY: called on the UI thread with an active Dear ImGui context.
    !unsafe { sys::igDockBuilderGetNode(id) }.is_null()
}

/// Whether the keyed window was built this frame and drew: not behind
/// another tab of its dock node, nor hidden while its node appears.
pub(super) fn shows(key: &WindowKey) -> bool {
    let name = window_name(key);
    // SAFETY: called on the UI thread with an active Dear ImGui context;
    // the window pointer is only read here.
    unsafe {
        let ctx = sys::igGetCurrentContext();
        if ctx.is_null() {
            return false;
        }
        let w = sys::igFindWindowByName(name.as_ptr());
        !w.is_null() && (*w).LastFrameActive == (*ctx).FrameCount && !(*w).Hidden
    }
}

fn window_name(key: &WindowKey) -> CString {
    CString::new(format!("###{}", key.stable_id())).expect("an ImGui window name")
}

fn saved_dock(name: &std::ffi::CStr) -> Option<u32> {
    // SAFETY: the context is active on this thread and the settings
    // pointer stays valid until the next settings mutation.
    unsafe {
        let id = sys::igImHashStr(name.as_ptr(), 0, 0);
        sys::igFindWindowSettingsByID(id).as_ref().map(|s| s.DockId)
    }
}

/// Collapse the obsolete Tools split if it was only used by Tools.
fn retire_tools_leaf(anchors: [&WindowKey; 5]) {
    let Some(dock) = saved_dock(c"###tools") else {
        return;
    };
    if dock == 0 {
        return;
    }
    let shared = anchors
        .iter()
        .any(|key| saved_dock(&window_name(key)) == Some(dock));
    // SAFETY: migration runs before submitting any windows to the dock.
    unsafe {
        let node = sys::igDockBuilderGetNode(dock);
        if !shared
            && !node.is_null()
            && sys::ImGuiDockNode_IsLeafNode(node)
            && !sys::ImGuiDockNode_IsCentralNode(node)
        {
            sys::igDockBuilderRemoveNode(dock);
        }
        sys::igClearWindowSettings(c"###tools".as_ptr());
    }
}

fn seed<'a>(anchor: &WindowKey, windows: impl IntoIterator<Item = &'a WindowKey>) {
    let Some(dock) = saved_dock(&window_name(anchor)) else {
        return;
    };
    if dock == 0 || !has_dock_node(dock) {
        return;
    }
    for window in windows {
        let window = window_name(window);
        if saved_dock(&window).is_none() {
            // SAFETY: the destination is a live dock node, and the name
            // is a NUL-terminated string valid for the duration of the call.
            unsafe { sys::igDockBuilderDockWindow(window.as_ptr(), dock) };
        }
    }
}
