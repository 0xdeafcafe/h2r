//! What the game view's overlay leaves to the guest's draws, by kind and
//! cause (docs/graphics/retire-d3d.md, "R6's remainder"): each HUD item a
//! producer offers and the overlay does not draw is one
//! `scene_kept_overlay_<kind>_<cause>` in `render.scene` ([`counters`]).
//!
//! A producer names its item's kind before it asks the host ([`begin`]).
//! The host names why it refused ([`refuse`]; the first cause holds), or
//! that the guest's resolved packet drew it in place of the overlay
//! ([`carried`]: cause `packet`). [`end`] counts an item nothing drew
//! under its cause. A HUD draw that reaches the device with no producer
//! before it is `untraced` ([`untraced`]).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// What a HUD item is, by the routine that draws it (Code, the lifted
/// image's calls of `0x0022a664` by return address; hud-kit.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverlayKind {
    /// A bitmap widget (`nhdt +0x08`, `0x0022aff1`) not held at the centre.
    Widget,
    /// A bitmap widget held at the centre anchor (4): the reticle.
    Crosshair,
    /// A glyph of a HUD string: text widgets, messages, the training box's lines.
    Text,
    /// The motion sensor's wash, tint and composite fans (`0x002548f0`).
    MotionSensor,
    /// A navpoint's arrow (`0x0024e9c7`) or its outline (`0x0024f0fc`).
    Navpoint,
    /// A navpoint's waypoint marker (`0x0024ef29`, from `0x0024ecae`).
    Waypoint,
    /// The indicator group (`0x00200462`, `HUD_INDICATOR_LAYOUT_END`).
    Indicator,
    /// The training box's frame (`0x001e9140`).
    Training,
    /// A screen-effect widget's call of `0x0002d140` (`nhdt +0x20`, `0x0022b8e2`).
    ScreenEffect,
    /// A HUD shader quad of another caller.
    Other,
    /// A HUD draw that reached the device with no producer before it.
    Untraced,
}

impl OverlayKind {
    pub const ALL: [OverlayKind; 11] = [
        OverlayKind::Widget,
        OverlayKind::Crosshair,
        OverlayKind::Text,
        OverlayKind::MotionSensor,
        OverlayKind::Navpoint,
        OverlayKind::Waypoint,
        OverlayKind::Indicator,
        OverlayKind::Training,
        OverlayKind::ScreenEffect,
        OverlayKind::Other,
        OverlayKind::Untraced,
    ];

    /// The kind's part of its counter's name.
    pub fn name(self) -> &'static str {
        match self {
            OverlayKind::Widget => "widget",
            OverlayKind::Crosshair => "crosshair",
            OverlayKind::Text => "text",
            OverlayKind::MotionSensor => "motion_sensor",
            OverlayKind::Navpoint => "navpoint",
            OverlayKind::Waypoint => "waypoint",
            OverlayKind::Indicator => "indicator",
            OverlayKind::Training => "training",
            OverlayKind::ScreenEffect => "screen_effect",
            OverlayKind::Other => "other",
            OverlayKind::Untraced => "untraced",
        }
    }
}

/// The bitmap widget helper's calls of the shader quad (`0x0022aff1`,
/// and `0x0022bbca`'s at `0x0022bca9`).
pub const WIDGET_QUAD_RETURNS: [u32; 7] = [0x22b39c, 0x22b561, 0x22b695, 0x22b701, 0x22b754, 0x22b7d3, 0x22bca9];
/// The training frame's ten pieces (`0x001e9140`).
pub const TRAINING_QUAD_RETURNS: [u32; 9] = [0x1e936f, 0x1e93b6, 0x1e9405, 0x1e9454, 0x1e94a3, 0x1e94e9, 0x1e9537, 0x1e9579, 0x1e95bb];
/// The navpoint's arrow (`0x0024e9c7`) and outline (`0x0024f0fc`).
pub const NAVPOINT_QUAD_RETURNS: [u32; 7] = [0x24eca5, 0x24f35c, 0x24f3e3, 0x24f437, 0x24f48b, 0x24f500, 0x24f55b];
/// The waypoint marker (`0x0024ef29`).
pub const WAYPOINT_QUAD_RETURN: u32 = 0x24f0bb;
/// The indicator group's quad (`0x00200462`).
pub const INDICATOR_QUAD_RETURN: u32 = 0x200784;
/// The screen-effect widget's call of `0x0002d140` (`0x0022bbbb`).
pub const SCREEN_EFFECT_RETURN: u32 = crate::scope_zoom_hud::HUD_SCREEN_RETURN;
/// The bitmap widget's call of the anchor helper (`render_aspect::GROUPS`).
pub const BITMAP_ANCHOR_RETURN: u32 = 0x22b055;
/// The anchor id of the reticle: the view's centre (`nhdt` anchor `centre`).
pub const CROSSHAIR_ANCHOR: u32 = 4;

/// No bitmap widget's anchor is known.
const NO_ANCHOR: u32 = u32::MAX;
static WIDGET_ANCHOR: AtomicU32 = AtomicU32::new(NO_ANCHOR);

/// The anchor id of the bitmap widget being drawn, as its call of the
/// anchor helper gave it (`HUD_LAYOUT_ANCHOR`); `None` at its group's end.
pub fn set_widget_anchor(anchor: Option<u32>) {
    WIDGET_ANCHOR.store(anchor.unwrap_or(NO_ANCHOR), Ordering::Relaxed);
}

/// The kind of a HUD shader quad (`0x0022a664`) by its call's return
/// address; a widget is the crosshair while its anchor is the centre.
pub fn quad_kind(caller: u32) -> OverlayKind {
    if WIDGET_QUAD_RETURNS.contains(&caller) {
        return if WIDGET_ANCHOR.load(Ordering::Relaxed) == CROSSHAIR_ANCHOR {
            OverlayKind::Crosshair
        } else {
            OverlayKind::Widget
        };
    }
    match caller {
        c if TRAINING_QUAD_RETURNS.contains(&c) => OverlayKind::Training,
        c if NAVPOINT_QUAD_RETURNS.contains(&c) => OverlayKind::Navpoint,
        WAYPOINT_QUAD_RETURN => OverlayKind::Waypoint,
        INDICATOR_QUAD_RETURN => OverlayKind::Indicator,
        _ => OverlayKind::Other,
    }
}

thread_local! {
    /// Inside the per-user HUD scope (`HUD_DRAW` to its returns).
    static IN_HUD: Cell<bool> = const { Cell::new(false) };
}

/// The HUD scope opens (`true`) or closes.
pub(super) fn set_in_hud(on: bool) {
    IN_HUD.set(on);
    if !on {
        set_widget_anchor(None);
    }
}

/// Whether this thread is drawing a view's HUD.
pub fn in_hud() -> bool {
    IN_HUD.get()
}

/// The item being offered to the host, and the first cause it gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    kind: OverlayKind,
    cause: Option<&'static str>,
}

#[derive(Default)]
struct Kept {
    pending: Option<Pending>,
    /// The last item ended undrawn: the guest's next draw is its own.
    guest_next: bool,
    /// A kept item's draws are still running ([`hold`]): all are its own.
    held: bool,
    counts: BTreeMap<(OverlayKind, &'static str), u64>,
}

static KEPT: Mutex<Kept> = Mutex::new(Kept { pending: None, guest_next: false, held: false, counts: BTreeMap::new() });

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    KEPT.lock().unwrap_or_else(|e| e.into_inner())
}

/// A producer offers an item of `kind` to the host.
pub fn begin(kind: OverlayKind) {
    let mut k = kept();
    k.pending = Some(Pending { kind, cause: None });
    k.guest_next = false;
    k.held = false;
}

/// The item just kept draws more than once (a screen effect's strips):
/// every guest draw is its own until [`release`].
pub fn hold() {
    kept().held = true;
}

/// The held item's draws are done.
pub fn release() {
    let mut k = kept();
    k.held = false;
    k.guest_next = false;
}

/// The host refused the offered item for `cause`; the first holds.
pub fn refuse(cause: &'static str) {
    let mut k = kept();
    if let Some(p) = k.pending.as_mut()
        && p.cause.is_none()
    {
        p.cause = Some(cause);
    }
}

/// The offered item was drawn from the guest's resolved packet (the
/// native UI packet path), not the overlay: counted `packet`.
pub fn carried() {
    let mut k = kept();
    if let Some(p) = k.pending.take() {
        *k.counts.entry((p.kind, "packet")).or_default() += 1;
    }
}

/// The offer is over: an item nothing drew keeps its guest draw, counted
/// under the host's cause (`refused` when it gave none).
pub fn end(drawn: bool) {
    let mut k = kept();
    let Some(p) = k.pending.take() else { return };
    if !drawn {
        *k.counts.entry((p.kind, p.cause.unwrap_or("refused"))).or_default() += 1;
        k.guest_next = true;
    }
}

/// An item of `kind` keeps its guest draw for `cause` with no offer (its
/// producer did not read it).
pub fn count(kind: OverlayKind, cause: &'static str) {
    let mut k = kept();
    *k.counts.entry((kind, cause)).or_default() += 1;
    k.guest_next = true;
}

/// A guest draw reached the device in the HUD scope: the draw of the last
/// kept item, or, when none is owed, one no producer offered, counted
/// `untraced` under `cause` (`packet` when its packet drew it).
pub fn untraced(cause: &'static str) {
    let mut k = kept();
    if k.held || std::mem::take(&mut k.guest_next) {
        return;
    }
    *k.counts.entry((OverlayKind::Untraced, cause)).or_default() += 1;
}

/// `scene_kept_overlay_<kind>_<cause>` for every item kept so far.
pub fn counters() -> Vec<(String, u64)> {
    kept()
        .counts
        .iter()
        .map(|(&(kind, cause), &n)| (format!("scene_kept_overlay_{}_{cause}", kind.name()), n))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the counters: serialise them.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn kept_count(name: &str) -> u64 {
        counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
    }

    #[test]
    fn every_hud_quad_caller_names_its_kind_and_the_centre_widget_is_the_crosshair() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_widget_anchor(Some(2));
        assert_eq!(quad_kind(0x22b695), OverlayKind::Widget);
        set_widget_anchor(Some(CROSSHAIR_ANCHOR));
        assert_eq!(quad_kind(0x22b561), OverlayKind::Crosshair);
        set_widget_anchor(Some(5));
        assert_eq!(quad_kind(0x22b561), OverlayKind::Widget, "a projected point is not the reticle");
        set_widget_anchor(None);
        assert_eq!(quad_kind(0x22bca9), OverlayKind::Widget);
        assert_eq!(quad_kind(0x1e95bb), OverlayKind::Training);
        assert_eq!(quad_kind(0x24eca5), OverlayKind::Navpoint);
        assert_eq!(quad_kind(0x24f55b), OverlayKind::Navpoint);
        assert_eq!(quad_kind(WAYPOINT_QUAD_RETURN), OverlayKind::Waypoint);
        assert_eq!(quad_kind(INDICATOR_QUAD_RETURN), OverlayKind::Indicator);
        assert_eq!(quad_kind(0x2b12b3), OverlayKind::Other);
        let names: std::collections::BTreeSet<_> = OverlayKind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(names.len(), OverlayKind::ALL.len());
    }

    #[test]
    fn a_kept_item_counts_its_first_cause_and_a_drawn_one_nothing() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let name = "scene_kept_overlay_navpoint_sampler";
        let before = kept_count(name);
        begin(OverlayKind::Navpoint);
        refuse("sampler");
        refuse("device");
        end(false);
        assert_eq!(kept_count(name), before + 1);
        let refused = kept_count("scene_kept_overlay_navpoint_refused");
        begin(OverlayKind::Navpoint);
        end(false);
        assert_eq!(kept_count("scene_kept_overlay_navpoint_refused"), refused + 1, "no cause given");
        let total: u64 = counters().iter().map(|(_, n)| n).sum();
        begin(OverlayKind::Navpoint);
        refuse("sampler");
        end(true);
        assert_eq!(counters().iter().map(|(_, n)| n).sum::<u64>(), total, "drawn: nothing kept");
        // Outside an offer a refusal names nothing.
        refuse("device");
        end(false);
        assert_eq!(counters().iter().map(|(_, n)| n).sum::<u64>(), total);
    }

    #[test]
    fn a_packet_drawn_item_is_counted_packet_once() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let name = "scene_kept_overlay_text_packet";
        let (before, texture) = (kept_count(name), kept_count("scene_kept_overlay_text_texture"));
        begin(OverlayKind::Text);
        refuse("texture");
        carried();
        end(true);
        assert_eq!(kept_count(name), before + 1);
        assert_eq!(kept_count("scene_kept_overlay_text_texture"), texture);
    }

    #[test]
    fn a_kept_items_own_guest_draw_is_not_untraced_but_the_next_draw_is() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let name = "scene_kept_overlay_untraced_packet";
        let before = kept_count(name);
        begin(OverlayKind::Widget);
        end(false);
        untraced("packet");
        assert_eq!(kept_count(name), before, "the kept widget's draw");
        untraced("packet");
        assert_eq!(kept_count(name), before + 1, "a draw no producer offered");
        count(OverlayKind::MotionSensor, "inputs");
        untraced("packet");
        assert_eq!(kept_count(name), before + 1, "the kept fan's draw");
        // A kept screen effect's strips are all its own until released.
        begin(OverlayKind::ScreenEffect);
        end(false);
        hold();
        for _ in 0..3 {
            untraced("packet");
        }
        assert_eq!(kept_count(name), before + 1);
        release();
        untraced("packet");
        assert_eq!(kept_count(name), before + 2);
    }
}
