//! The Go to anything popup. Search and actions live in the App; the
//! asset rows are read once per query and kept here
//! ([`commands::palette_assets`]), the rest is rebuilt every frame.
use dear_imgui_rs::{Key, ListClipper, Ui, WindowFlags};

use crate::app::commands::{self, Kind, PALETTE_LIMIT};
use crate::app::{Action, App};
use crate::theme;

const POPUP: &str = "Go to anything###palette";

#[derive(Default)]
pub(super) struct PaletteUi {
    opening: bool,
    query: String,
    selected: usize,
    /// The asset rows for `query`, kept until the query changes: reading
    /// them walks the tags tree and the package set.
    assets: Option<(String, commands::Palette)>,
}

impl PaletteUi {
    pub fn open(&mut self) {
        self.opening = true;
    }

    pub fn is_open(&self, ui: &Ui) -> bool {
        self.opening || ui.is_popup_open(POPUP)
    }

    pub fn draw(&mut self, ui: &Ui, app: &mut App) -> Option<Action> {
        let opening = std::mem::take(&mut self.opening);
        if opening {
            self.query.clear();
            self.selected = 0;
            self.assets = None;
            ui.open_popup(POPUP);
        }
        // ImGui centres new modal popups in the main viewport.
        let _popup = ui
            .begin_modal_popup_config(POPUP)
            .flags(
                WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_SAVED_SETTINGS | WindowFlags::NO_MOVE,
            )
            .begin()?;
        if ui.is_key_pressed(Key::Escape) {
            ui.close_current_popup();
            return None;
        }

        if opening {
            ui.set_keyboard_focus_here();
        }
        let width = theme::PALETTE_WIDTH.min(ui.main_viewport().work_size()[0] * 0.9);
        let run = ui.is_key_pressed_with_repeat(Key::Enter, false)
            || ui.is_key_pressed_with_repeat(Key::KeypadEnter, false);
        ui.set_next_item_width(width);
        let changed = ui
            .input_text("##palette-query", &mut self.query)
            .hint("Search maps, tags, packages, items, placements and commands")
            .build();
        // The asset rows are read once per query; the rest is
        // state-dependent and rebuilt every frame.
        let assets = match self.assets.take() {
            Some((q, rows)) if q == self.query => {
                self.assets = Some((q, rows.clone()));
                rows
            }
            _ => {
                let rows = commands::palette_assets(app, &self.query);
                self.assets = Some((self.query.clone(), rows.clone()));
                rows
            }
        };
        let results = commands::palette_with(app, &self.query, &assets);
        let last = results.entries.len().saturating_sub(1);
        let previous = self.selected;
        if changed {
            self.selected = 0;
        } else if ui.is_key_pressed_with_repeat(Key::DownArrow, true) {
            self.selected += 1;
        } else if ui.is_key_pressed_with_repeat(Key::UpArrow, true) {
            self.selected = self.selected.saturating_sub(1);
        }
        self.selected = self.selected.min(last);

        let mut picked = run.then_some(self.selected);
        let row_height = ui.text_line_height() + ui.text_line_height_with_spacing();
        theme::bounded_list(
            ui,
            "##palette-results",
            width,
            row_height,
            results.entries.len(),
            theme::PALETTE_ROWS,
            || {
                if results.entries.is_empty() {
                    ui.text_disabled("No results");
                    return;
                }
                let reveal = opening || changed || self.selected != previous;
                let mut clipper = ListClipper::new(results.entries.len())
                    .items_height(row_height)
                    .begin(ui);
                if reveal {
                    clipper.include_item_by_index(self.selected);
                }
                for i in clipper.iter() {
                    let entry = &results.entries[i];
                    let detail = if entry.detail.is_empty() {
                        entry.kind.title().to_owned()
                    } else {
                        format!("{} · {}", entry.kind.title(), entry.detail)
                    };
                    if ui
                        .selectable_config(format!("{}\n{detail}##palette-{i}", entry.label))
                        .selected(i == self.selected)
                        .close_popups(false)
                        .build()
                    {
                        picked = Some(i);
                    }
                    if reveal
                        && i == self.selected
                        && (opening || changed || !ui.is_item_visible())
                    {
                        ui.set_scroll_here_y(0.5);
                    }
                }
            },
        );

        let counts = Kind::ALL
            .into_iter()
            .map(|kind| {
                let total = results.total(kind);
                if total > PALETTE_LIMIT {
                    format!("{} {PALETTE_LIMIT} of {total}", kind.title())
                } else {
                    format!("{} {total}", kind.title())
                }
            })
            .collect::<Vec<_>>()
            .join("   ");
        ui.text_disabled(counts);

        let entry = results.entries.into_iter().nth(picked?)?;
        ui.close_current_popup();
        Some(entry.action)
    }
}
