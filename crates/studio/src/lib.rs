//! h2-editor: the editor suite on Dear ImGui (docking branch, through
//! `dear-imgui-rs`), rendered with wgpu and windowed with winit, over the
//! UI-free crates (`editor-core`, `scenario-edit`'s `Session`, `variants`,
//! `hopper-edit`).
//!
//! - [`app`]: the state and commands, testable without a window.
//! - [`ui`]: the shell (menus, dockspace, tools, outliner, inspector, log,
//!   status bar, modals).
//! - [`fields`]: the tag field panel (every field of a tag element,
//!   labelled, a widget per type), for the Tags tool and any other.
//! - [`viewport`]: the 3D view with picking and the ImGuizmo gizmo.
//! - [`theme`]: the dark theme, bundled fonts and icons.
//! - [`headless`]: renders real frames offscreen to PNG (screenshots),
//!   with [`script`]ed input aimed at widgets that [`probe`] finds.
//!
//! Nothing here contains game data: maps come from the user's own disc.

pub mod app;
pub mod fields;
pub mod headless;
pub mod probe;
pub mod script;
pub mod theme;
pub mod ui;
pub mod viewport;

pub use app::{Action, Answer, App, ToolKind};

use dear_imgui_rs::{ConfigFlags, Context};

/// A Dear ImGui context set up the studio's way: docking on, the theme
/// and fonts loaded, windows moved by their title bars only (so dragging
/// in the viewport never drags the window). `ini`: where the layout
/// persists, or `None` for the default layout every run.
pub fn context(ini: Option<std::path::PathBuf>) -> (Context, theme::Fonts) {
    let mut ctx = Context::create();
    ctx.set_ini_filename(None::<std::path::PathBuf>)
        .expect("no ini path");
    if let Some(path) = ini {
        // Loaded and saved by hand so a bad path never fails start-up.
        let _ = ctx.load_ini_settings_from_disk(path);
    }
    let io = ctx.io_mut();
    let mut flags = io.config_flags();
    flags.insert(ConfigFlags::DOCKING_ENABLE);
    io.set_config_flags(flags);
    io.set_config_windows_move_from_title_bar_only(true);
    let fonts = theme::load_fonts(&mut ctx);
    theme::apply(ctx.style_mut());
    (ctx, fonts)
}
