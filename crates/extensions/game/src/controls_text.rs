//! The retail Controls list's row text (`0x2c8548`, list slot 20), ported
//! (docs/engine/feature-engine.md "UI step c").
//!
//! **What the retail function does** (static, 2026-11-16): it fetches the
//! item's two text widgets (`ITEM_TEXT(item, 6, 0|1, 0)`), picks a label
//! and a value string id by the item's row (the datum handle's index at
//! `item + 0x70`) and one of the player's control settings, then sets each
//! text widget that exists (`TEXT_SET`; an id of -1 leaves it). Past row 5
//! the label is -1 and the value 0.
//!
//! **No string id is copied here.** Every id is an immediate of an
//! instruction in the loaded image, read at run time from the instruction
//! named below: `mov ecx, imm32` (`b9`) for a label, `mov esi, imm32`
//! (`be`) for a value, and for the three on/off rows the `and eax, imm32`
//! (`25`) and `add eax, imm32` (`05`) of `value = add + (bit ? mask : 0)`.
//! Row 2's value is reached through the function's own jump table, read
//! from the image too. An instruction whose opcode is not the one named
//! leaves that text as it is, with a line.
//!
//! The settings bytes are the ones the function reads (`0x54e6d4` to
//! `0x54e6da`); what each row means is not traced, so the rows go by
//! number.

use std::sync::atomic::{AtomicBool, Ordering};

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use hooks::{BuildError, Builder, Manifest};

/// The feature id.
pub const ID: &str = "controls-text";

/// The setting flags: bit 0 for row 3, bit 3 for row 4, bit 1 for row 5.
pub const FLAGS: u32 = 0x0054_e6d4;
/// Row 1's setting (0 to 3).
pub const ROW1_SETTING: u32 = 0x0054_e6d8;
/// Row 0's setting (0 to 3).
pub const ROW0_SETTING: u32 = 0x0054_e6d9;
/// Row 2's setting, clamped to 1 to 10.
pub const ROW2_SETTING: u32 = 0x0054_e6da;

const MOV_ECX: u8 = 0xb9;
const MOV_ESI: u8 = 0xbe;
const AND_EAX: u8 = 0x25;
const ADD_EAX: u8 = 0x05;

/// Each row's label: its `mov ecx, imm32`.
pub const LABEL_AT: [u32; 6] = [
    0x002c_86a8,
    0x002c_8676,
    0x002c_85ed,
    0x002c_85cf,
    0x002c_85c0,
    0x002c_85a3,
];
/// Rows 0 and 1, by setting 0 to 3: the `mov esi, imm32` of each value
/// (setting 0 shares one).
pub const ROW0_VALUE_AT: [u32; 4] = [0x002c_86cd, 0x002c_86c6, 0x002c_86bf, 0x002c_86b8];
pub const ROW1_VALUE_AT: [u32; 4] = [0x002c_86cd, 0x002c_8698, 0x002c_8691, 0x002c_868a];
/// Row 2's jump table: ten targets, setting 1 to 10, each a
/// `mov esi, imm32`.
pub const ROW2_JUMPS: u32 = 0x002c_86f1;
/// Rows 3 and 4's `and eax, imm32` and `add eax, imm32`.
pub const ROWS34_AND: u32 = 0x002c_85da;
pub const ROWS34_ADD: u32 = 0x002c_85df;
/// Row 5's.
pub const ROW5_AND: u32 = 0x002c_85aa;
pub const ROW5_ADD: u32 = 0x002c_85af;

/// An instruction's 32-bit immediate, when its opcode (one byte) is `op`.
fn imm(m: &dyn GuestMemory, at: u32, op: u8) -> Result<u32, u32> {
    match m.u8(at) {
        Ok(b) if b == op => m.u32(at + 1).map_err(|_| at),
        _ => Err(at),
    }
}

/// `value = add + (bit ? mask : 0)`, as the function computes it
/// (`neg al; sbb eax, eax; and eax, mask; add eax, add`).
fn on_off(m: &dyn GuestMemory, bit: bool, and_at: u32, add_at: u32) -> Result<u32, u32> {
    let mask = imm(m, and_at, AND_EAX)?;
    let add = imm(m, add_at, ADD_EAX)?;
    Ok(add.wrapping_add(if bit { mask } else { 0 }))
}

/// One text's id, or the instruction that was not what was read.
pub type Id = Result<u32, u32>;

/// The label and value row `row` draws, read from the image and the
/// settings as the retail function reads them.
pub fn texts(m: &dyn GuestMemory, row: i16) -> (Id, Id) {
    let byte = |a: u32| m.u8(a).unwrap_or(0);
    let flags = byte(FLAGS);
    let Some(&label_at) = usize::try_from(row).ok().and_then(|r| LABEL_AT.get(r)) else {
        return (Ok(u32::MAX), Ok(0));
    };
    let label = imm(m, label_at, MOV_ECX);
    let value = match row {
        0 | 1 => {
            let (setting, at) = if row == 0 {
                (ROW0_SETTING, &ROW0_VALUE_AT)
            } else {
                (ROW1_SETTING, &ROW1_VALUE_AT)
            };
            match at.get(usize::from(byte(setting))) {
                Some(&at) => imm(m, at, MOV_ESI),
                None => Ok(0),
            }
        }
        2 => {
            let v = u32::from(byte(ROW2_SETTING)).clamp(1, 10);
            let jump = ROW2_JUMPS + 4 * (v - 1);
            m.u32(jump)
                .map_err(|_| jump)
                .and_then(|target| imm(m, target, MOV_ESI))
        }
        3 => on_off(m, flags & 1 != 0, ROWS34_AND, ROWS34_ADD),
        4 => on_off(m, flags & 8 != 0, ROWS34_AND, ROWS34_ADD),
        _ => on_off(m, flags & 2 != 0, ROW5_AND, ROW5_ADD),
    };
    (label, value)
}

static MISREAD: AtomicBool = AtomicBool::new(false);

fn id(r: Id) -> Option<u32> {
    r.map_err(|at| {
        if !MISREAD.swap(true, Ordering::Relaxed) {
            eprintln!(
                "controls text: the instruction at {at:#x} is not the one read; that text is left as it was"
            );
        }
    })
    .ok()
}

/// The port: thiscall on the list, the item and a word, `ret 8`. The
/// caller (`0x24abe0`) does not read EAX.
pub async fn row_text(cx: Cx) -> Result<Ret, EntryError> {
    let item = cx.arg(0)?;
    let text = |index| {
        GuestCall::new(points::ITEM_TEXT.base)
            .ecx(item)
            .args([6, index, 0])
    };
    let label_text = cx.call(text(0)).await?.eax;
    let value_text = cx.call(text(1)).await?.eax;
    let (label, value) = cx.read(|m| {
        let row = m.u16(item.wrapping_add(0x70)).map(|v| v as i16)?;
        Ok::<_, guest_layout::memory::MemFault>(texts(m, row))
    })?;
    for (widget, id) in [(label_text, id(label)), (value_text, id(value))] {
        if let (true, Some(id)) = (widget != 0, id) {
            cx.call(GuestCall::new(points::TEXT_SET.base).edi(widget).args([id]))
                .await?;
        }
    }
    Ok(Ret::new(0, 8))
}

/// The feature: the port in place of the retail function. Our rows' `before`
/// hooks (`controls-rows`) and the layouts' value text pair
/// (`layouts-value`) run around it as they ran around the original.
pub struct ControlsText;

impl hooks::Feature for ControlsText {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The Controls list's row text, ported").doc("docs/game/settings-menu.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The port never calls the retail body: it reads the body's
        // immediates and jump table as data and calls only ITEM_TEXT and
        // TEXT_SET, so the lift may leave the body out
        // (docs/lifter/lift-pruning.md, "The replaced list").
        b.raw()
            .implement_async(
                &points::CONTROLS_ROW_TEXT,
                "ported row text",
                |cx| -> EntryFuture { Box::pin(row_text(cx)) },
            )
            .replaces_guest();
        b.calls_guest(&points::ITEM_TEXT);
        b.calls_guest(&points::TEXT_SET);
        // The port's state is its future, parked in a continuation the
        // machine's quiescence check reports.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{GuestCall as Gc, Regs, testing};
    use guest_layout::testing::FakeGuest;
    use std::sync::Arc;

    const STACK: u32 = 0x0030_0800;
    const ITEM: u32 = 0x0010_0000;
    const LABEL_TEXT: u32 = 0x0011_0000;
    const VALUE_TEXT: u32 = 0x0011_1000;

    /// A synthetic image: each named instruction with its opcode and a made
    /// up immediate (`0xa000_0000 + its address`), the jump table pointing at
    /// ten made up `mov esi` instructions, and the settings bytes.
    fn image() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x002c_8000, 0x1000).map(0x0054_e000, 0x1000);
        let put = |g: &mut FakeGuest, at: u32, op: u8| {
            g.put_u8(at, op).put_u32(at + 1, 0xa000_0000 + at);
        };
        for at in LABEL_AT {
            put(&mut g, at, MOV_ECX);
        }
        for at in ROW0_VALUE_AT.into_iter().chain(ROW1_VALUE_AT) {
            put(&mut g, at, MOV_ESI);
        }
        for i in 0..10 {
            let target = 0x002c_8800 + 8 * i;
            g.put_u32(ROW2_JUMPS + 4 * i, target);
            put(&mut g, target, MOV_ESI);
        }
        put(&mut g, ROWS34_AND, AND_EAX);
        put(&mut g, ROWS34_ADD, ADD_EAX);
        put(&mut g, ROW5_AND, AND_EAX);
        put(&mut g, ROW5_ADD, ADD_EAX);
        g
    }

    fn i(at: u32) -> u32 {
        0xa000_0000 + at
    }

    #[test]
    fn each_row_reads_its_ids_from_the_named_instructions() {
        let mut g = image();
        g.put_u8(ROW0_SETTING, 2)
            .put_u8(ROW1_SETTING, 3)
            .put_u8(ROW2_SETTING, 4);
        g.put_u8(FLAGS, 0b1001); // rows 3 and 4 on, row 5 off
        assert_eq!(texts(&g, 0), (Ok(i(LABEL_AT[0])), Ok(i(ROW0_VALUE_AT[2]))));
        assert_eq!(texts(&g, 1), (Ok(i(LABEL_AT[1])), Ok(i(ROW1_VALUE_AT[3]))));
        assert_eq!(
            texts(&g, 2),
            (Ok(i(LABEL_AT[2])), Ok(i(0x002c_8800 + 8 * 3)))
        );
        let on = i(ROWS34_ADD).wrapping_add(i(ROWS34_AND));
        assert_eq!(texts(&g, 3), (Ok(i(LABEL_AT[3])), Ok(on)));
        assert_eq!(texts(&g, 4), (Ok(i(LABEL_AT[4])), Ok(on)));
        assert_eq!(texts(&g, 5), (Ok(i(LABEL_AT[5])), Ok(i(ROW5_ADD))));
        g.put_u8(FLAGS, 0b0010);
        assert_eq!(texts(&g, 3).1, Ok(i(ROWS34_ADD)));
        assert_eq!(texts(&g, 4).1, Ok(i(ROWS34_ADD)));
        assert_eq!(texts(&g, 5).1, Ok(i(ROW5_ADD).wrapping_add(i(ROW5_AND))));
    }

    #[test]
    fn settings_out_of_range_and_rows_past_five_as_the_function_has_them() {
        let mut g = image();
        g.put_u8(ROW0_SETTING, 4).put_u8(ROW1_SETTING, 0xff);
        assert_eq!(texts(&g, 0).1, Ok(0));
        assert_eq!(texts(&g, 1).1, Ok(0));
        // Row 2 clamps to 1..=10.
        g.put_u8(ROW2_SETTING, 0);
        assert_eq!(texts(&g, 2).1, Ok(i(0x002c_8800)));
        g.put_u8(ROW2_SETTING, 200);
        assert_eq!(texts(&g, 2).1, Ok(i(0x002c_8800 + 8 * 9)));
        for row in [6, 7, -1] {
            assert_eq!(texts(&g, row), (Ok(u32::MAX), Ok(0)), "{row}");
        }
    }

    #[test]
    fn an_instruction_that_is_not_the_one_read_is_named() {
        let mut g = image();
        g.put_u8(LABEL_AT[3], 0x90);
        g.put_u8(ROWS34_ADD, 0x90);
        assert_eq!(texts(&g, 3), (Err(LABEL_AT[3]), Err(ROWS34_ADD)));
    }

    /// The call capture: the port's guest calls for one row, in the order the
    /// retail function makes them.
    fn run(g: FakeGuest, row: u16, widgets: (u32, u32)) -> Vec<Gc> {
        let mut g = g;
        g.map(ITEM, 0x100).map(STACK - 0x800, 0x1000);
        g.put_u16(ITEM + 0x70, row);
        g.put_u32(STACK, 0xdead)
            .put_u32(STACK + 4, ITEM)
            .put_u32(STACK + 8, 0);
        let f = |cx| -> EntryFuture { Box::pin(row_text(cx)) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: match c.args.get(1) {
                Some(0) if c.target == points::ITEM_TEXT.base => widgets.0,
                Some(1) if c.target == points::ITEM_TEXT.base => widgets.1,
                _ => 0,
            },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        run.calls
    }

    #[test]
    fn the_port_fetches_both_texts_then_sets_label_and_value() {
        let mut g = image();
        g.put_u8(ROW1_SETTING, 1);
        let item_text = |index| {
            Gc::new(points::ITEM_TEXT.base)
                .ecx(ITEM)
                .args([6, index, 0])
        };
        assert_eq!(
            run(g, 1, (LABEL_TEXT, VALUE_TEXT)),
            [
                item_text(0),
                item_text(1),
                Gc::new(points::TEXT_SET.base)
                    .edi(LABEL_TEXT)
                    .args([i(LABEL_AT[1])]),
                Gc::new(points::TEXT_SET.base)
                    .edi(VALUE_TEXT)
                    .args([i(ROW1_VALUE_AT[1])]),
            ]
        );
    }

    #[test]
    fn a_missing_text_widget_is_skipped_and_a_row_past_five_sets_minus_one() {
        let calls = run(image(), 9, (LABEL_TEXT, 0));
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[2],
            Gc::new(points::TEXT_SET.base)
                .edi(LABEL_TEXT)
                .args([u32::MAX])
        );
    }

    #[test]
    fn the_port_implements_the_point_with_our_rows_before_it() {
        use crate::setting_defs::ALL;
        use crate::ui::List;
        use crate::video_screen::{self, Switches};
        let model = Arc::new(
            video_screen::model(Arc::new(settings::Store::defaults(ALL)), Switches::ALL).unwrap(),
        );
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&ControlsText).unwrap();
        r.add(&crate::layout_glue::LayoutValueText).unwrap();
        r.add(&crate::retail_rows::Extension {
            id: "controls-rows",
            title: "Our rows on the Controls list",
            list: List::Controls,
            text: &points::CONTROLS_ROW_TEXT,
            chosen: &points::CONTROLS_ROW_CHOSEN,
            label_only: false,
            model,
        })
        .unwrap();
        let plan = r.build(&hooks::Base).unwrap();
        let text = plan.text();
        let line = text
            .lines()
            .find(|l| l.starts_with("0x002c8548"))
            .unwrap_or_default();
        assert_eq!(
            line,
            "0x002c8548 CONTROLS_ROW_TEXT: before [controls-rows:our rows' text, \
             layouts-value:layout value pending] -> implement controls-text:ported row text \
             -> after [layouts-value:layout value done]"
        );
        // The hooks around the port wrap it, not the retail body: the body
        // is replaced, and the two functions the port calls are roots.
        let entry = points::CONTROLS_ROW_TEXT.base;
        let replaced = plan.replaced(&[entry].into_iter().collect());
        assert_eq!(replaced.keys().copied().collect::<Vec<_>>(), [entry]);
        assert_eq!(
            plan.calls_guest.keys().copied().collect::<Vec<_>>(),
            [points::ITEM_TEXT.base, points::TEXT_SET.base]
        );
    }
}
