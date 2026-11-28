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
//!
//! The menus' draws (slice R7, docs/graphics/retire-d3d-menus.md, "What is
//! kept") count the same way under `scene_kept_ui_<kind>_<cause>`
//! ([`UiKind`]): a screen draw is offered with [`begin_ui`], a menu
//! widget, list, outline or blur the game keeps is counted where
//! its producer refuses it ([`ui`], [`count`]). A UI item's guest draws
//! are not `untraced`: that is the HUD's.
//!
//! A game view's post calls and flare fans (slice R5, retire-d3d.md,
//! "R5's remainder") count the same way under `scene_kept_<kind>_<cause>`
//! ([`LateKind`]): `native_post` offers each colour pass and each screen
//! effect outside the HUD, and the flare admission counts a fan it leaves
//! to the device ([`count`]).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use timeline::native::Ui;

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
    /// A motion sensor blip's quad (`0x00254490`, `native_hud::blips`).
    SensorBlip,
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
    pub const ALL: [OverlayKind; 12] = [
        OverlayKind::Widget,
        OverlayKind::Crosshair,
        OverlayKind::Text,
        OverlayKind::MotionSensor,
        OverlayKind::SensorBlip,
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
            OverlayKind::SensorBlip => "sensor_blip",
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

/// What a kept menu-frame draw is (`scene_kept_ui_<kind>_<cause>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UiKind {
    /// A drawn widget of a sorted list the overlay does not list.
    Widget,
    /// A whole sorted list the game draws.
    List,
    /// The full-screen fade (`0x00040890`).
    Fade,
    /// A solid rectangle of `0x00052ce7`: a menu fade, a bar, a divider.
    Bar,
    /// The loading screen's bitmaps and spinner (`0x000289e0`).
    Loading,
    /// Another widget fan or screen quad outside the HUD.
    Quad,
    /// A menu shader bitmap's quad (`0x002b12ae`).
    MenuShader,
    /// A widget outline (`0x0001ed98`).
    Outline,
    /// The blur under a dialog (`0x00235756`).
    Blur,
}

impl UiKind {
    pub const ALL: [UiKind; 9] = [
        UiKind::Widget,
        UiKind::List,
        UiKind::Fade,
        UiKind::Bar,
        UiKind::Loading,
        UiKind::Quad,
        UiKind::MenuShader,
        UiKind::Outline,
        UiKind::Blur,
    ];

    /// The kind's part of its counter's name.
    pub fn name(self) -> &'static str {
        match self {
            UiKind::Widget => "widget",
            UiKind::List => "list",
            UiKind::Fade => "fade",
            UiKind::Bar => "bar",
            UiKind::Loading => "loading",
            UiKind::Quad => "quad",
            UiKind::MenuShader => "menu_shader",
            UiKind::Outline => "outline",
            UiKind::Blur => "blur",
        }
    }
}

/// What a kept post call or flare fan is (`scene_kept_<kind>_<cause>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LateKind {
    /// The colour pass `0x000214f0`.
    PostColour,
    /// A call of `0x0002d140` outside the HUD.
    PostScreenEffect,
    /// A flare fan (`native_render::flares::Site`, by its name).
    Flare(&'static str),
}

impl LateKind {
    /// Its counter for `cause`.
    fn counter(self, cause: &str) -> String {
        match self {
            LateKind::PostColour => format!("scene_kept_post_colour_{cause}"),
            LateKind::PostScreenEffect => format!("scene_kept_post_screen_effect_{cause}"),
            LateKind::Flare(site) => format!("scene_kept_flare_{site}_{cause}"),
        }
    }
}

/// A kept item's kind: the HUD's (`scene_kept_overlay_*`), the menu
/// frame's (`scene_kept_ui_*`) or a post call's or flare fan's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Overlay(OverlayKind),
    Ui(UiKind),
    Late(LateKind),
}

impl From<LateKind> for Kind {
    fn from(kind: LateKind) -> Self {
        Kind::Late(kind)
    }
}

impl From<OverlayKind> for Kind {
    fn from(kind: OverlayKind) -> Self {
        Kind::Overlay(kind)
    }
}

impl From<UiKind> for Kind {
    fn from(kind: UiKind) -> Self {
        Kind::Ui(kind)
    }
}

impl Kind {
    /// Its counter for `cause`.
    pub fn counter(self, cause: &str) -> String {
        match self {
            Kind::Overlay(kind) => format!("scene_kept_overlay_{}_{cause}", kind.name()),
            Kind::Ui(kind) => format!("scene_kept_ui_{}_{cause}", kind.name()),
            Kind::Late(kind) => kind.counter(cause),
        }
    }

    /// A HUD item: its guest draw is owed to it, not `untraced`.
    fn hud(self) -> bool {
        matches!(self, Kind::Overlay(_))
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
    kind: Kind,
    cause: Option<&'static str>,
}

#[derive(Default)]
struct Kept {
    pending: Option<Pending>,
    /// The last item ended undrawn: the guest's next draw is its own.
    guest_next: bool,
    /// A kept item's draws are still running ([`hold`]): all are its own.
    held: bool,
    counts: BTreeMap<(Kind, &'static str), u64>,
}

static KEPT: Mutex<Kept> = Mutex::new(Kept { pending: None, guest_next: false, held: false, counts: BTreeMap::new() });

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    KEPT.lock().unwrap_or_else(|e| e.into_inner())
}

/// A producer offers an item of `kind` to the host. A post call's offer
/// leaves what the HUD's guest draws are owed as it is.
pub fn begin(kind: impl Into<Kind>) {
    let kind = kind.into();
    let mut k = kept();
    k.pending = Some(Pending { kind, cause: None });
    if !matches!(kind, Kind::Late(_)) {
        k.guest_next = false;
        k.held = false;
    }
}

/// The offered item has nothing to draw (a colour pass whose glow is off):
/// the offer ends with nothing kept.
pub fn empty() {
    kept().pending = None;
}

/// A menu-frame screen draw of `kind` is offered to the host, unless an
/// item is already being offered (a HUD item, or a menu shader bitmap
/// whose quad falls back to a screen fan): whether this call began it,
/// and so must [`end`] it.
pub fn begin_ui(kind: UiKind) -> bool {
    let mut k = kept();
    if k.pending.is_some() {
        return false;
    }
    k.pending = Some(Pending { kind: Kind::Ui(kind), cause: None });
    true
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
        k.guest_next |= p.kind.hud();
    }
}

/// An item of `kind` keeps its guest draw for `cause` with no offer (its
/// producer did not read it).
pub fn count(kind: impl Into<Kind>, cause: &'static str) {
    count_n(kind, cause, 1);
}

/// `n` items of `kind` keep their guest draws for `cause` (a menu run's
/// widgets whose pass was not placed).
pub fn count_n(kind: impl Into<Kind>, cause: &'static str, n: u64) {
    if n == 0 {
        return;
    }
    let kind = kind.into();
    let mut k = kept();
    *k.counts.entry((kind, cause)).or_default() += n;
    k.guest_next |= kind.hud();
}

/// The kept menu-frame draw a count of `counter` is, as its kind and
/// cause, when it is one: a menu widget or list the overlay does not
/// draw, an outline's draw (`packet` when the native UI
/// packet path records it), a blur the game draws. Counters of draws the
/// overlay took, and of visits, are none.
pub fn ui_kept(counter: Ui) -> Option<(UiKind, &'static str)> {
    use UiKind::{Blur, List, Outline, Widget};
    Some(match counter {
        Ui::OutlineNative => (Outline, "packet"),
        Ui::OutlineEntry => (Outline, "entry"),
        Ui::OutlineInputs => (Outline, "inputs"),
        Ui::OutlineReservation => (Outline, "reservation"),
        Ui::OverlayMenuRefusedFrame => (List, "frame"),
        Ui::OverlayMenuRefusedMismatched => (List, "mismatched"),
        Ui::OverlayMenuRefusedBitmapState => (Widget, "bitmap_state"),
        Ui::OverlayMenuRefusedText => (Widget, "text"),
        Ui::OverlayMenuRefusedModel => (Widget, "model"),
        Ui::OverlayMenuRefusedDraw => (Widget, "draw"),
        Ui::OverlayMenuRefusedUnreadable => (Widget, "unreadable"),
        Ui::OverlayMenuRefusedUnlisted => (Widget, "unlisted"),
        Ui::OverlayModalBlurReplayed => (Blur, "replayed"),
        Ui::OverlayModalBlurRefusedPass => (Blur, "pass"),
        Ui::OverlayModalBlurRefusedArguments => (Blur, "arguments"),
        Ui::OverlayModalBlurRefusedSteps => (Blur, "steps"),
        Ui::OverlayModalBlurRefusedCombiner => (Blur, "combiner"),
        Ui::OverlayModalBlurRefusedTargets => (Blur, "targets"),
        // A run's pass not placed keeps each of its draws: counted per
        // draw where the run is known (`ui_overlay::placed`).
        _ => return None,
    })
}

/// Counts `counter` and, when it is a kept menu-frame draw ([`ui_kept`]),
/// its `scene_kept_ui_<kind>_<cause>`.
pub fn ui(counter: Ui) {
    counter.count();
    if let Some((kind, cause)) = ui_kept(counter) {
        count(kind, cause);
    }
}

/// A guest draw reached the device in the HUD scope: the draw of the last
/// kept item, or, when none is owed, one no producer offered, counted
/// `untraced` under `cause` (`packet` when its packet drew it).
pub fn untraced(cause: &'static str) {
    let mut k = kept();
    if k.held || std::mem::take(&mut k.guest_next) {
        return;
    }
    *k.counts.entry((Kind::Overlay(OverlayKind::Untraced), cause)).or_default() += 1;
}

/// `scene_kept_overlay_<kind>_<cause>`, `scene_kept_ui_<kind>_<cause>`
/// and the post's and flares' counters for every item kept so far.
pub fn counters() -> Vec<(String, u64)> {
    kept()
        .counts
        .iter()
        .map(|(&(kind, cause), &n)| (kind.counter(cause), n))
        .collect()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// The tests share the counters: serialise them.
    pub(in crate::native_hud) static SERIAL: Mutex<()> = Mutex::new(());

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
    fn a_menu_draw_counts_under_scene_kept_ui_and_owes_no_untraced_draw() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        release();
        let (fade, untraced_n) = (kept_count("scene_kept_ui_fade_clip_rects"), kept_count("scene_kept_overlay_untraced_packet"));
        assert!(begin_ui(UiKind::Fade));
        refuse("clip_rects");
        end(false);
        assert_eq!(kept_count("scene_kept_ui_fade_clip_rects"), fade + 1);
        // A guest draw after a kept HUD item is owed to it; after a menu
        // draw it is not.
        untraced("packet");
        assert_eq!(kept_count("scene_kept_overlay_untraced_packet"), untraced_n + 1);
        // An offer already open is not replaced: the menu shader's.
        let shader = kept_count("scene_kept_ui_menu_shader_combiner");
        begin(UiKind::MenuShader);
        refuse("combiner");
        assert!(!begin_ui(UiKind::Quad));
        end(false);
        assert_eq!(kept_count("scene_kept_ui_menu_shader_combiner"), shader + 1);
        let pass = kept_count("scene_kept_ui_widget_pass");
        count_n(UiKind::Widget, "pass", 3);
        count_n(UiKind::Widget, "pass", 0);
        assert_eq!(kept_count("scene_kept_ui_widget_pass"), pass + 3);
        let names: std::collections::BTreeSet<_> = UiKind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(names.len(), UiKind::ALL.len());
    }

    #[test]
    fn a_kept_ui_counter_counts_its_draw() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(ui_kept(Ui::OverlayMenuClaimed), None, "a claimed draw is the overlay's");
        assert_eq!(ui_kept(Ui::OverlayMenuRefusedPass), None, "counted per draw of the run");
        let name = "scene_kept_ui_outline_inputs";
        let before = kept_count(name);
        ui(Ui::OutlineInputs);
        assert_eq!(kept_count(name), before + 1);
    }

    #[test]
    fn a_kept_post_call_or_flare_fan_counts_its_cause_and_owes_the_hud_nothing() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        release();
        let colour = kept_count("scene_kept_post_colour_arm");
        let untraced_n = kept_count("scene_kept_overlay_untraced_packet");
        begin(LateKind::PostColour);
        refuse("arm");
        end(false);
        assert_eq!(kept_count("scene_kept_post_colour_arm"), colour + 1);
        untraced("packet");
        assert_eq!(kept_count("scene_kept_overlay_untraced_packet"), untraced_n + 1, "not the HUD's");
        let total: u64 = counters().iter().map(|(_, n)| n).sum();
        begin(LateKind::PostColour);
        empty();
        end(false);
        assert_eq!(counters().iter().map(|(_, n)| n).sum::<u64>(), total, "a pass with nothing to draw keeps nothing");
        let effect = kept_count("scene_kept_post_screen_effect_loading");
        begin(LateKind::PostScreenEffect);
        refuse("loading");
        end(false);
        assert_eq!(kept_count("scene_kept_post_screen_effect_loading"), effect + 1);
        // A HUD screen effect's held strips stay held across a post offer.
        begin(OverlayKind::ScreenEffect);
        end(false);
        hold();
        begin(LateKind::PostColour);
        end(true);
        untraced("packet");
        assert_eq!(kept_count("scene_kept_overlay_untraced_packet"), untraced_n + 1);
        release();
        let fan = kept_count("scene_kept_flare_composite_geometry");
        count(LateKind::Flare("composite"), "geometry");
        assert_eq!(kept_count("scene_kept_flare_composite_geometry"), fan + 1);
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
