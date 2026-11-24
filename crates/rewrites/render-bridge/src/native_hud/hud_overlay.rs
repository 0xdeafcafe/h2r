//! The HUD on the overlay stage (docs/graphics/native-post-hud.md, "The HUD
//! on the overlay"): where in a HUD draw the host may draw its widgets from
//! the scene's overlay, and which of the game's widget quads it then
//! leaves out.
//!
//! One HUD draw is `0x0013a532` drawing one `nhdt` (Code, `h2 disasm`): it
//! computes the gate's words (`0x0013a050`), then gates and draws its
//! screen effects (root `+0x20`, `0x0022b8e2`), its bitmap widgets (`+0x08`,
//! `0x0022aff1`) and its text widgets (`+0x10`, `0x0022aa16`), in that
//! order. The first bitmap widget's gate call (returning to
//! [`BITMAP_GATE_RETURN`] with the loop's offset `[ebp-0x14]` 0) comes
//! before any bitmap widget is drawn and after the screen effects: the host
//! is asked there ([`super::Host::hud_bitmaps`]), with the gate's words and
//! record as the engine passes them, and its pass is placed at that point
//! in the frame. A claimed draw's plain widget quads (`0x0022a664` called
//! from [`PLAIN_QUAD_RETURN`] by the bitmap widget draw of the same HUD
//! draw) are skipped; any other quad draws as before. The claim closes at
//! the next HUD draw's first bitmap gate or when the HUD scope leaves, and
//! a claim whose skipped quads are not the items its pass drew is counted
//! and reported ([`super::Host::hud_mismatch`]).

use std::cell::RefCell;

use engine::memory::GuestMemory;
use timeline::native::Ui;

/// The return address of `0x0013a532`'s gate call in its bitmap loop.
pub const BITMAP_GATE_RETURN: u32 = 0x0013_a602;
/// `[ebp-0x14]` in `0x0013a532`: the loop's byte offset into the block,
/// 0 at its first widget.
const LOOP_OFFSET_FROM_EBP: u32 = 0x14;
/// `0x0013a532`'s first argument, `[ebp+8]`: the user drawn for (the
/// argument it passes the inputs, `points::HUD_WIDGET_INPUTS`).
const USER_FROM_EBP: u32 = 8;
/// The return address of `0x0022aff1`'s call of `0x0022a664` for a
/// widget that is not the shield meter (special type 5 draws its layers
/// from five other calls, `0x0022b55c` to `0x0022b7ce`).
pub const PLAIN_QUAD_RETURN: u32 = 0x0022_b39c;
/// `0x0022aff1`'s frame: `push ebp; lea ebp, [esp-0x68]`, so the caller's
/// EBP is at `[ebp+0x68]`.
const WIDGET_DRAW_SAVED_EBP: u32 = 0x68;

/// The record the gate takes in ECX, its three compared fields
/// (`hud_edit::state::GateRecord`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateRecord {
    pub at_10: f32,
    pub at_06: i16,
    pub at_08: i16,
}

/// One HUD draw at its first bitmap widget's gate: what the engine passes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudBitmaps {
    /// `0x0013a532`'s EBP: names the draw.
    pub frame: u32,
    /// The `nhdt`'s tag data (EDI).
    pub nhdt: u32,
    pub user: u32,
    /// The gate's words A, B, C (before the widget's thresholds) and D.
    pub words: [u16; 4],
    /// ECX's record; `None` when ECX is 0.
    pub record: Option<GateRecord>,
}

/// Why a HUD draw is left to the game's quads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudRefusal {
    /// A shown widget is drawn by a tag shader that does not translate.
    Shader,
    /// A shown text widget.
    Text,
    /// A shown widget with an effects block.
    Effects,
    /// A shown widget whose anchor is not a corner.
    Anchor,
    /// A shown widget the walk could not list.
    Unlisted,
    /// Not the full screen, or a frame the walk does not place.
    Frame,
    /// The map's HUDs are not loaded yet, or this one is not in them.
    Loading,
    /// The overlay's pass was not placed.
    Pass,
}

impl HudRefusal {
    pub fn counter(self) -> Ui {
        match self {
            HudRefusal::Shader => Ui::OverlayHudRefusedShader,
            HudRefusal::Text => Ui::OverlayHudRefusedText,
            HudRefusal::Effects => Ui::OverlayHudRefusedEffects,
            HudRefusal::Anchor => Ui::OverlayHudRefusedAnchor,
            HudRefusal::Unlisted => Ui::OverlayHudRefusedUnlisted,
            HudRefusal::Frame => Ui::OverlayHudRefusedFrame,
            HudRefusal::Loading => Ui::OverlayHudRefusedLoading,
            HudRefusal::Pass => Ui::OverlayHudRefusedPass,
        }
    }
}

/// The HUD draw the overlay drew: its frame, its `nhdt`, the quads its
/// pass drew and those skipped so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    pub frame: u32,
    pub nhdt: u32,
    pub quads: u32,
    pub skipped: u32,
}

thread_local! {
    /// The game thread's open claim.
    static CLAIM: RefCell<Option<Claim>> = const { RefCell::new(None) };
}

/// The draw at a gate call (ESP at the call's return address, the
/// caller's EBP, EDI, ECX, EDX): `Some` at the first bitmap widget of a
/// HUD draw.
pub fn at_gate(mem: &dyn GuestMemory, esp: u32, ebp: u32, edi: u32, ecx: u32, edx: u32) -> Option<HudBitmaps> {
    if mem.u32(esp).ok()? != BITMAP_GATE_RETURN {
        return None;
    }
    if mem.u32(ebp.checked_sub(LOOP_OFFSET_FROM_EBP)?).ok()? != 0 {
        return None;
    }
    let arg = |n: u32| mem.u32(esp.checked_add(4 * n)?).ok().map(|w| w as u16);
    let record = if ecx == 0 {
        None
    } else {
        Some(GateRecord {
            at_10: f32::from_bits(mem.u32(ecx.checked_add(0x10)?).ok()?),
            at_06: mem.u16(ecx.checked_add(6)?).ok()? as i16,
            at_08: mem.u16(ecx.checked_add(8)?).ok()? as i16,
        })
    };
    Some(HudBitmaps {
        frame: ebp,
        nhdt: edi,
        user: mem.u32(ebp.checked_add(USER_FROM_EBP)?).ok()?,
        words: [arg(1)?, arg(2)?, edx as u16, arg(3)?],
        record,
    })
}

/// Opens a claim of `quads` widget quads for the draw `at`, closing the
/// one before.
pub fn open(at: &HudBitmaps, quads: u32) -> Option<Claim> {
    let old = close();
    Ui::OverlayHudClaimed.count();
    CLAIM.with(|c| *c.borrow_mut() = Some(Claim { frame: at.frame, nhdt: at.nhdt, quads, skipped: 0 }));
    old
}

/// Closes the open claim: it, when its skipped quads are not its drawn
/// ones (counted), for the host to stop claiming that HUD.
pub fn close() -> Option<Claim> {
    let claim = CLAIM.with(|c| c.borrow_mut().take())?;
    if claim.skipped != claim.quads {
        Ui::OverlayHudMismatch.count();
        return Some(claim);
    }
    None
}

/// Whether the HUD quad whose frame (`0x0022a664`'s EBP) is `frame` is a
/// plain widget quad of the claimed draw: then it is counted as skipped,
/// and the caller leaves it out.
pub fn owns_quad(mem: &dyn GuestMemory, frame: u32) -> bool {
    let Some(claimed) = CLAIM.with(|c| *c.borrow()) else {
        return false;
    };
    let draw = || -> Option<u32> {
        if mem.u32(frame.checked_add(4)?).ok()? != PLAIN_QUAD_RETURN {
            return None;
        }
        let widget = mem.u32(frame).ok()?;
        mem.u32(widget.checked_add(WIDGET_DRAW_SAVED_EBP)?).ok()
    };
    if draw() != Some(claimed.frame) {
        return false;
    }
    CLAIM.with(|c| {
        if let Some(claim) = c.borrow_mut().as_mut() {
            claim.skipped += 1;
        }
    });
    Ui::OverlayHudQuadsSkipped.count();
    true
}

/// Forgets the open claim (a snapshot load, a new map).
pub fn forget() {
    CLAIM.with(|c| c.borrow_mut().take());
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    /// The first bitmap gate of a HUD draw reads the words as pushed and
    /// the record's fields; another call site or a later widget is not it.
    #[test]
    fn the_first_bitmap_gate_reads_the_engines_words_and_record() {
        let mut mem = FakeGuest::new();
        let (esp, ebp, record) = (0x2000, 0x2100, 0x3000);
        mem.put_u32(esp, BITMAP_GATE_RETURN)
            .put_u32(esp + 4, 0x1_0004)
            .put_u32(esp + 8, 0x0002)
            .put_u32(esp + 12, 0x0400)
            .put_u32(ebp - LOOP_OFFSET_FROM_EBP, 0)
            .put_u32(ebp + USER_FROM_EBP, 1)
            .put_f32(record + 0x10, 0.5)
            .put_u16(record + 6, 0xfffe)
            .put_u16(record + 8, 3);
        let at = at_gate(&mem, esp, ebp, 0xe000, record, 0x0010).unwrap();
        assert_eq!(
            at,
            HudBitmaps {
                frame: ebp,
                nhdt: 0xe000,
                user: 1,
                words: [0x0004, 0x0002, 0x0010, 0x0400],
                record: Some(GateRecord { at_10: 0.5, at_06: -2, at_08: 3 }),
            }
        );
        assert_eq!(at_gate(&mem, esp, ebp, 0xe000, 0, 0).unwrap().record, None);
        mem.put_u32(ebp - LOOP_OFFSET_FROM_EBP, 0x64);
        assert_eq!(at_gate(&mem, esp, ebp, 0xe000, 0, 0), None, "the second widget");
        mem.put_u32(ebp - LOOP_OFFSET_FROM_EBP, 0).put_u32(esp, 0x0013_a659);
        assert_eq!(at_gate(&mem, esp, ebp, 0xe000, 0, 0), None, "the text loop's gate");
    }

    /// Only a plain widget quad whose widget draw was called by the claimed
    /// HUD draw is skipped; the claim closes clean when every drawn quad
    /// was skipped, and reports itself when not.
    #[test]
    fn a_claim_skips_its_own_plain_quads_and_checks_the_count() {
        let mut mem = FakeGuest::new();
        let (hud, widget, quad) = (0x2100, 0x1f00, 0x1e00);
        mem.put_u32(quad, widget)
            .put_u32(quad + 4, PLAIN_QUAD_RETURN)
            .put_u32(widget + WIDGET_DRAW_SAVED_EBP, hud);
        forget();
        assert!(!owns_quad(&mem, quad), "no claim");
        let at = HudBitmaps { frame: hud, nhdt: 0xe000, user: 0, words: [0; 4], record: None };
        assert_eq!(open(&at, 2), None);
        assert!(owns_quad(&mem, quad));
        mem.put_u32(quad + 4, 0x0022_b561);
        assert!(!owns_quad(&mem, quad), "a shield layer is the game's");
        mem.put_u32(quad + 4, PLAIN_QUAD_RETURN).put_u32(widget + WIDGET_DRAW_SAVED_EBP, 0x2200);
        assert!(!owns_quad(&mem, quad), "another HUD draw's");
        mem.put_u32(widget + WIDGET_DRAW_SAVED_EBP, hud);
        assert!(owns_quad(&mem, quad));
        assert_eq!(close(), None, "two drawn, two skipped");
        assert_eq!(close(), None, "closed");
        open(&at, 3);
        assert!(owns_quad(&mem, quad));
        let next = HudBitmaps { frame: 0x2400, ..at };
        assert_eq!(open(&next, 1), Some(Claim { frame: hud, nhdt: 0xe000, quads: 3, skipped: 1 }));
        forget();
    }
}
