//! A large kit dialog, available both in the shell and over a running map.
use std::sync::{Arc, Mutex};

use guest_layout::memory::{GuestMemory, MemFault};

use crate::list_class::{ListClass, Panel, PanelText, Rows, TextBlock};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::{dialog::DialogBase, dialog_tag, ids, screen::SlotTexts};

pub(super) const SCREEN: &str = "live.join";
const IDS: ids::Family = ids::family("join link");

#[derive(Default)]
struct Choice {
    confirm: bool,
    join_selected: bool,
    closed: bool,
}

pub(super) struct Dialog {
    pub(super) texts: SlotTexts,
    choice: Mutex<Choice>,
}

impl Default for Dialog {
    fn default() -> Self {
        Self {
            texts: SlotTexts::new(IDS),
            choice: Mutex::new(Choice::default()),
        }
    }
}

impl Dialog {
    fn choice(&self) -> std::sync::MutexGuard<'_, Choice> {
        self.choice.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn prepare(&self, confirm: bool, body: &str) {
        *self.choice() = Choice {
            confirm,
            ..Default::default()
        };
        self.texts.set(
            0,
            if confirm {
                "Join game"
            } else {
                "Unable to join"
            },
        );
        self.texts.set(1, body);
        self.texts.set(2, if confirm { "Cancel" } else { "OK" });
        self.texts.set(3, "Join game");
    }

    pub(super) fn closed(&self) {
        self.choice().closed = true;
    }

    /// Ignore a selection until the dialog has actually left the screen.
    pub(super) fn answer(&self) -> Option<bool> {
        let choice = self.choice();
        choice.closed.then_some(choice.join_selected)
    }

    pub(super) fn class(self: &Arc<Self>) -> ListClass {
        let closed = self.clone();
        let base = DialogBase::Large;
        let mut layout = dialog_tag::base_layout(base, 2);
        layout.anchor[1] = -8;
        ListClass::with_rows("live-join-dialog", SCREEN, self.clone())
            .dialog(base)
            .layout(layout)
            .panel(Panel {
                art: Vec::new(),
                texts: vec![PanelText {
                    block: TextBlock {
                        rect: [70, -240, -4, 247],
                        font: 1,
                        colour: [1.0, 1.0, 1.0],
                        animation: 0,
                    },
                    right: false,
                    bias: 0,
                }],
            })
            .action(Arc::new(|cx, screen| {
                Box::pin(async move { dialog_tag::close(&cx, screen).await })
            }))
            .on_free(Arc::new(move |_, _| {
                closed.closed();
                Box::pin(async { Ok(()) })
            }))
    }
}

impl Rows for Dialog {
    fn title(&self) -> u32 {
        IDS.id(0)
    }

    fn count(&self) -> usize {
        if self.choice().confirm { 2 } else { 1 }
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match row {
            0 => vec![(LABEL_SLOT, IDS.id(2))],
            1 if self.choice().confirm => vec![(LABEL_SLOT, IDS.id(3))],
            _ => Vec::new(),
        }
    }

    fn panel_text(&self, i: usize) -> Option<u32> {
        (i == 0).then(|| IDS.id(1))
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.texts.write(m)
    }

    fn press(&self, row: i16) -> Chosen {
        let mut choice = self.choice();
        if row != 0 && !(row == 1 && choice.confirm) {
            return Chosen::Nothing;
        }
        choice.join_selected = row == 1;
        Chosen::Act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backing_out_or_dismissing_an_error_never_leaves_the_game() {
        let d = Dialog::default();
        d.prepare(true, "Join Echo's game? You'll leave this one.");
        assert_eq!(d.count(), 2);
        assert_eq!(d.answer(), None);
        d.choice().closed = true; // B
        assert_eq!(d.answer(), Some(false));
        d.prepare(true, "Join Echo's game? You'll leave this one.");
        d.press(1);
        assert_eq!(d.answer(), None);
        d.choice().closed = true;
        assert_eq!(d.answer(), Some(true));
        d.prepare(false, "This session is full.");
        assert_eq!(d.count(), 1);
        assert!(matches!(d.press(1), Chosen::Nothing));
        d.press(0);
        d.choice().closed = true;
        assert_eq!(d.answer(), Some(false));
    }
}
