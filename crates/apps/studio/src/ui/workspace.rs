//! The tag workspace's window pieces (docs/editor/tag-workspace.md,
//! "First start"): the start page of a first run — choose the disc, see
//! the size and the time, extract every map — and the bar the status
//! line shows while an extraction runs in the background. The page is
//! one floating window that takes focus once and goes when the
//! extraction begins (the status line has the bar); the shell around it
//! is another lane's, so nothing of this docks or adds tabs.
use std::path::PathBuf;

use dear_imgui_rs::{Condition, Ui, WindowFlags};

use crate::app::App;
use crate::theme::{self, Fonts, icon};

/// The start page's state: the disc chosen and the last start's answer.
#[derive(Default)]
pub struct WorkspaceUi {
    /// The disc path typed or picked.
    disc: String,
    /// "Not now" pressed: the page stays away for this session.
    dismissed: bool,
    /// The disc's maps and bytes, as last read for the path shown.
    summary: Option<(String, Result<(Vec<String>, u64), String>)>,
    /// Why the last Extract would not start, if it would not.
    note: Option<String>,
}

/// The discs offered before anything is typed: `H2_DISC`, then
/// `disc/halo2.iso` in the repository.
fn offered() -> Vec<(&'static str, PathBuf)> {
    let mut out: Vec<(&'static str, PathBuf)> = Vec::new();
    if let Some(p) = std::env::var_os("H2_DISC") {
        out.push(("H2_DISC", p.into()));
    }
    if let Some(p) = editor_core::default_disc(None) {
        out.push(("disc/halo2.iso", p));
    }
    out
}

/// The start page: one step, extract every map of the disc into the
/// tree.
pub fn start_page(ui: &Ui, app: &mut App, st: &mut WorkspaceUi, fonts: &Fonts, dialogs: bool) {
    if st.dismissed || !app.workspace_start_page() {
        return;
    }
    let offered = offered();
    if st.disc.is_empty() {
        // The first offered disc is the natural choice; another can
        // always be typed or picked.
        if let Some((_, p)) = offered.first() {
            st.disc = p.display().to_string();
        }
    }
    // Centre it below the tool bar, out of the docks' way, so nothing
    // the shell already shows is covered.
    let vp = ui.main_viewport();
    let [w, h] = vp.work_size();
    let [x, y] = vp.work_pos();
    let at = [x + (w - 560.0).max(0.0) / 2.0, y + (h * 0.2).max(96.0)];
    ui.window("First start###workspace-start")
        .position(at, Condition::FirstUseEver)
        .size([560.0, 0.0], Condition::FirstUseEver)
        .flags(WindowFlags::NO_COLLAPSE | WindowFlags::NO_DOCKING)
        .build(|| {
            let _title = ui.push_font(fonts.title);
            ui.text_colored(theme::TEXT, format!("{} First start", icon::MAP));
            drop(_title);
            ui.text_wrapped(format!(
                "Every tag of your Halo 2 disc is kept as a tree of loose \
                 files under {}, and every map is built from it. The tree is \
                 game data: about 2.65 GB for the retail disc, written once in \
                 about a minute and a half, never committed, and taken again \
                 when our knowledge of the formats grows.",
                app.tags_root().display()
            ));
            ui.separator();
            ui.set_next_item_width(-150.0);
            ui.input_text("##workspace-disc", &mut st.disc)
                .hint("The disc: an .iso image, a .map file, or a folder of them")
                .build();
            ui.same_line();
            if ui.button(format!("{} Browse...##workspace-browse", icon::FOLDER)) && dialogs {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Choose the disc")
                    .add_filter("Disc image or map", &["iso", "map"])
                    .pick_file()
                {
                    st.disc = p.display().to_string();
                }
            }
            ui.same_line();
            if ui.button(format!("{} Folder...##workspace-folder", icon::FOLDER)) && dialogs {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Choose a folder of maps")
                    .pick_folder()
                {
                    st.disc = p.display().to_string();
                }
            }
            // The offered discs, as one-click choices.
            for (label, p) in &offered {
                let same = p.display().to_string() == st.disc;
                if ui
                    .selectable_config(format!("{label}: {}", p.display()))
                    .selected(same)
                    .build()
                {
                    st.disc = p.display().to_string();
                }
            }
            // What the disc holds: the maps and the bytes to read.
            let path = PathBuf::from(st.disc.trim());
            if !st.disc.trim().is_empty() {
                if st.summary.as_ref().is_none_or(|(p, _)| *p != st.disc) {
                    let r = if path.exists() {
                        crate::app::workspace::disc_summary(&path)
                    } else {
                        Err(format!("{} does not exist", path.display()))
                    };
                    st.summary = Some((st.disc.clone(), r));
                }
                match st
                    .summary
                    .as_ref()
                    .and_then(|(p, r)| (p == &st.disc).then_some(r))
                {
                    Some(Ok((maps, bytes))) => {
                        ui.text_disabled(format!(
                            "{} maps, {:.1} GB to read: {}",
                            maps.len(),
                            *bytes as f64 / 1_073_741_824.0,
                            maps.join(", ")
                        ));
                    }
                    Some(Err(e)) => {
                        ui.text_colored(theme::WARN, format!("{} {e}", icon::ALERT));
                    }
                    None => {}
                }
            }
            if let Some(e) = &st.note {
                ui.text_colored(theme::ERROR, format!("{} {e}", icon::ALERT));
            }
            let can = !st.disc.trim().is_empty();
            if ui.button(format!(
                "{} Extract every map##workspace-extract",
                icon::DISC
            )) && can
            {
                match app.workspace_extract(Some(path), None, false, None) {
                    Ok(_) => {
                        // The extraction runs: the page goes, and the
                        // status line has the bar.
                        st.dismissed = true;
                        st.note = None;
                    }
                    Err(e) => st.note = Some(e.to_string()),
                }
            }
            ui.same_line();
            if ui.button("Not now##workspace-later") {
                st.dismissed = true;
            }
            if can {
                ui.text_disabled(
                    "Work continues while the tree fills: maps already extracted are read \
                     as they are, and the disc is only read.",
                );
            }
        });
}

/// The status line's workspace piece: a bar while an extraction runs,
/// with the map it last finished beside it. Drawn where the hint goes;
/// the hint yields while an extraction is in progress.
pub fn status_progress(ui: &Ui, app: &App, x: f32, y: f32, width: f32, line_height: f32) {
    let Some((frac, text)) = app.workspace.progress() else {
        return;
    };
    let bar = 110.0f32.min(width * 0.25);
    ui.set_cursor_screen_pos([x, y - 2.0]);
    ui.progress_bar(frac).size([bar, line_height + 2.0]).build();
    let start = x + bar + 8.0;
    if width - bar - 8.0 > 40.0 {
        ui.set_cursor_screen_pos([start, y]);
        theme::clipped_text(ui, &text, theme::DIM, width - bar - 16.0);
    }
}

#[cfg(test)]
mod tests {
    use crate::app::Action;
    use crate::headless::Headless;

    /// The start page draws with no tree, and goes when a source is
    /// opened (a loose map works without one).
    #[test]
    fn the_start_page_draws_with_no_tree_and_yields_to_a_source() {
        let Ok(mut h) = Headless::new([1280.0, 800.0], 2.0) else {
            return;
        };
        let t = crate::app::map_cards_tests::Temp::new("ws-start");
        let mut app = crate::App::new();
        app.set_data_root(t.0.join("data"));
        h.capture(&mut app, 6).unwrap();
        assert!(app.workspace_start_page());
        assert!(
            h.scene.find("win:workspace-start").is_ok(),
            "{:?}",
            h.scene.windows
        );
        assert!(h.scene.find("Extract every map").is_ok());
        // A source opened: the page goes.
        std::fs::write(t.0.join("loose.map"), scenario_edit::fixture::fixture()).unwrap();
        app.dispatch(Action::OpenSource(t.0.join("loose.map")));
        h.capture(&mut app, 2).unwrap();
        assert!(!app.workspace_start_page());
        assert!(h.scene.find("win:workspace-start").is_err());
    }
}
