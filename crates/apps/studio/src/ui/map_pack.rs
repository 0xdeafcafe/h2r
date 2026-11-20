//! The Maps list's Map Pack group: the installed Multiplayer Map Pack maps
//! in display-name order, each a [`map_list`](super::map_list) row
//! with its record's picture and description. With nothing installed the
//! group stays, with a line on how to install it. State lives in
//! [`crate::app::map_pack`]; clicks go back as [`Action::OpenMapPack`].
use dear_imgui_rs::Ui;
use editor_core::map_groups::MapGroup;

use super::map_list::{Group, MapListUi, Row};
use crate::app::map_pack::HINT;
use crate::app::{Action, App};
use crate::theme::icon;
use crate::viewport::Gpu;

/// The Map Pack group: its heading, then a row a map, or the hint.
pub fn group(
    ui: &Ui,
    app: &App,
    st: &mut MapListUi,
    gpu: &mut Gpu<'_>,
    heading: impl Fn(&Ui, &str),
    actions: &mut Vec<Action>,
) {
    ui.spacing();
    heading(ui, MapGroup::MapPack.label());
    match app.mappack() {
        Some(disc) => {
            let kind = if disc.is_folder() {
                icon::FOLDER
            } else {
                icon::DISC
            };
            ui.text_disabled(format!("{kind} {}", disc.path().display()));
        }
        None if !app.map_pack().is_empty() => {
            ui.text_disabled(format!("{} installed, in <data>", icon::DRIVE));
        }
        None => {}
    }
    if app.map_pack().is_empty() {
        ui.text_disabled(format!("{} {HINT}", icon::INFO));
        st.hint(Group::MapPack);
        return;
    }
    let opened = app.opened_pack_map();
    for m in app.map_pack_entries() {
        let from = format!(
            "{}.map, {}\nopens with the Halo 2 disc's shared maps; edits save as a package \
             over it",
            m.map,
            if m.path.is_some() {
                format!("installed in T:\\$c\\{}", m.offer)
            } else {
                "from the map pack disc".to_owned()
            }
        );
        let row = Row {
            key: format!("pack-{}", m.map),
            label: &m.name,
            icon: icon::MAP,
            selected: opened.is_some_and(|o| o.eq_ignore_ascii_case(&m.map)),
            picture: m.picture.as_ref(),
            description: &m.description,
            from: &from,
        };
        if st.row(ui, gpu, Group::MapPack, &row) {
            actions.push(Action::OpenMapPack(m.map.clone()));
        }
    }
}
