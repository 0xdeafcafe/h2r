//! The HUD kit's runtime (docs/game/hud-kit.md, "The runtime"): HUDs of
//! ours drawn by Halo 2's own HUD code.
//!
//! A feature registers a [`Describe`] once ([`register`]) and publishes
//! its [`Values`] whenever its state changes ([`Handle::publish`]), and
//! the world point of its navpoint ([`Handle::navpoint`]). The rest is
//! here:
//!
//! 1. **Install**, before `GAME_ACTIVATE` and after the HUD swap: for
//!    every player `nhdt` of the loaded map (`hud_kit_core::map_file`),
//!    resolve the templates, compile every registered HUD
//!    (`hud_kit_core::records`), copy the tag's two widget blocks into
//!    our guest memory with the kit's records after them, and point the
//!    root's two block fields at the copies ([`install_records`]). The
//!    retail records are byte for byte the same and draw first. What the
//!    root held is kept at the head of our memory ([`Header`]), so a
//!    second install puts it back first and leaves one copy, a machine
//!    snapshot included.
//! 2. **Gate**: a kit widget is drawn while its value
//!    is shown, in a style the runtime draws ([`draws`]), in a screen mode
//!    its place has, and none of its element's `hide_when_d` bits is set
//!    in the game's word D. For a local user in monitor mode
//!    ([`Handle::retail`]) every other widget, the retail ones of the
//!    player's and the weapon's `nhdt`, is hidden but the motion tracker
//!    ([`retail_gate`], `model::MONITOR_KEEPS`).
//! 3. **Inputs**: `hud_input1` is
//!    `records::Widget::input` of the value's fill.
//!
//! A value with a `motion` moves on wall time: [`Handle::publish`] stamps
//! a changed one with the time and the fill it was showing
//! (`motion::restamp`), and the gate and inputs read it at the
//! milliseconds since (`motion::animate`), each drawn frame. The
//! **scale** answer ([`scale_factors`]) scales a kit widget's basis by
//! `motion::scale_at`; until it has been asked for a kit widget
//! ([`EFFECTS_SEEN`]) the ending is the plain one.
//! 4. **String**: the value's text, in the element's font.
//! 5. **Crop**: a cropped widget's sprite bounds are narrowed to its crop
//!    ([`crop_sprite`]).
//! 6. **Channels**: the prompt through the user's prompt fields while the
//!    game shows no prompt of its own ([`show_prompt`]); the training text
//!    through the scripted training slot and the string lookup
//!    ([`write_training`], [`text`]); the banner through each local user's
//!    timed centred line when the text its values show changes ([`banner_due`],
//!    [`write_banner`]); the navpoint through the game's
//!    navpoint list at a scenario point of ours ([`install_point`],
//!    [`write_navpoint`]); a notification ([`Handle::notify`]) through the
//!    game's insert, a guest call at the top of the next tick
//!    ([`NOTIFY`], [`stage_notes`]).
//!
//! 7. **Live edits**: an `nhdt` write from the agent's `edit.*`
//!    ([`tag_written`]) copies the edited retail records into the copies
//!    the root points at ([`recopy`]).
//!
//! Gate, inputs, string, crop and scale are answers at the Rust walk's
//! override seam (`render_bridge::native_hud::seam`, docs/graphics/native-hud-overlay.md,
//! "The override seam"); they read one shared table, on the render thread.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::time::Instant;

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use guest_layout::tags::{GroupTag, tag_get_checked};
use hooks::{BuildError, Builder, Flow};
use hud_kit_core::Mode;
use hud_kit_core::model::{
    BANNER_TICKS, BANNER_UNITS, Describe, Hud, Kind, MESSAGE_UNITS, Style, Value, Values,
    banner_fault, message_fault,
};
use hud_kit_core::motion::{self, Motion};
use hud_kit_core::records::{self, Compiled, WidgetKind};
use hud_kit_core::walk::Sprite;
use hud_kit_core::world::{Pose, Shape};

use crate::h1_particle_physics::fault;
use crate::ui_kit::ids::{Family, family};

/// The feature's id.
pub const FEATURE: &str = "hud-kit";

/// Our guest memory for the copied widget blocks: a [`Header`], then the
/// blocks. The guest arena is shared by every feature and nearly full, so
/// this is kept small; blocks that do not fit are not installed (logged).
pub const REGION_LEN: u32 = 0x8000;
/// Our guest memory for the scenario's point block with our point after
/// it: a [`PointHeader`], then the block; a block that does not fit gets
/// no navpoint point (logged).
pub const POINTS_LEN: u32 = 0x4000;

const NHDT: u32 = u32::from_be_bytes(*b"nhdt");
const BITM: u32 = u32::from_be_bytes(*b"bitm");

/// The UTF-16 units a kit string has at most, its zero included
/// (S: the retail strings are short).
pub const STRING_UNITS: usize = 0x80;

/// The user HUD states (`[0x005023f4] + user * 0x4e0`): the prompt's text
/// at `+0x220`, its string id `+0x440`, shown `+0x444`, and the fade
/// fields `+0x44e`, `+0x44f` (E `0x0024ca1d`, `0x0024c98c`;
/// `h1_hud_text::prompts`).
pub const USER_HUDS: u32 = 0x0050_23f4;
pub const USER_HUD_LEN: u32 = 0x4e0;
pub const PROMPT_TEXT: u32 = 0x220;
pub const PROMPT_ID: u32 = 0x440;
pub const PROMPT_SHOWN: u32 = 0x444;
/// The UTF-16 units of the prompt's text, its zero included.
pub const PROMPT_UNITS: usize = ((PROMPT_ID - PROMPT_TEXT) / 2) as usize;
/// The user's timed centred line, as `0x0024cc73` (EAX the user, ECX the
/// text, the ticks its one stack argument) leaves it: the text at
/// `+0x454` (copied by `0x0013fac0` with 63, so 62 units and a zero), the
/// tick at `+0x450` and `+0x4d8`, shown `+0x4d2`, the order byte `+0x4d3`
/// (the next value of the byte at `[0x005023f4] + 0x1385`, which it steps)
/// and the duration in ticks at `+0x4dc`. The draw shows it while the game
/// tick is from the start to the start plus the duration, fading over it
/// (E `0x0024d58a`..`0x0024d5cf`).
pub const BANNER_TEXT: u32 = 0x454;
pub const BANNER_STAMP: u32 = 0x450;
pub const BANNER_SHOWN: u32 = 0x4d2;
pub const BANNER_ORDER: u32 = 0x4d3;
pub const BANNER_START: u32 = 0x4d8;
pub const BANNER_DURATION: u32 = 0x4dc;
/// The order byte the timed line and the notifications share, from the
/// first user's state.
pub const MESSAGE_ORDER: u32 = 0x1385;
/// The game time (`[0x00510c54]`): the tick at `+8`, which the message
/// routine reads (E `0x0024cc97`).
pub const GAME_TIME: u32 = 0x0051_0c54;
pub const GAME_TIME_TICK: u32 = 8;
/// The local user whose HUD is being drawn: `UI_HUD_DRAW` reads it and
/// hands it to its per-user draws (E `0x001917ed`, the navpoint draw's
/// ECX at `0x0019186a`). S: that it holds the user through the widget
/// loop.
pub const DRAW_USER: u32 = 0x004b_9ed8;
/// `HUD_INTERACTION_VISIBLE`'s caller, the interaction code (E
/// `0x00191976`).
pub const FROM_INTERACTION: u32 = 0x0019_197b;

/// The training state (`[0x0051e9c0]`): four users of `0x1b0`, then the
/// scripted text's string id and its show byte (E `0x001e70c3`).
pub const TRAINING: u32 = 0x0051_e9c0;
pub const TRAINING_ID: u32 = 0x6c0;
pub const TRAINING_SHOWN: u32 = 0x6c4;

/// The navpoint lists (`[0x005023f8] + user * 0x30`): four slots of 12
/// bytes, `+0` an `i16`, `+2` the kind in its low four bits (15 skips the
/// slot, 0 a scenario point), `+4` a height added to the point, `+8` the
/// point's index. The draw walks all four slots: one whose `+0` or `+8`
/// is -1, or whose kind is 15, it marks kind 15 and passes; no slot ends
/// the list (E `0x0024daf1`..`0x0024db25`, `0x0024dc21`..`0x0024dc3e`).
pub const NAVPOINTS: u32 = 0x0050_23f8;
pub const NAV_USER_LEN: u32 = 0x30;
pub const NAV_SLOTS: u32 = 4;
pub const NAV_SLOT_LEN: u32 = 12;
pub const NAV_SCENARIO_POINT: u16 = 0;
/// The scenario points the kit adds, and so the navpoints it can show at
/// once: one for each slot of a user's list ([`NAV_SLOTS`]). A navpoint
/// element takes them in order over the registered HUDs
/// ([`navpoints_now`]).
pub const KIT_POINTS: u32 = NAV_SLOTS;
pub const NAV_SKIP: u16 = 15;
/// The scenario (`[0x004e0350]`) and the tag block a kind-0 slot indexes:
/// `scnr +0x1e0`, its count at `+0` and its address at `+4`, `0x38` bytes
/// a point, the point at `+0x24`. The draw reads the address at
/// `scnr +0x1e4` and does not check the index against the count (E
/// `0x0024db56`..`0x0024db6b`; D `formats/tags/src/defs/scnr.txt`, the
/// block at `0x1e0` of `0x38`-byte elements).
pub const SCENARIO: u32 = 0x004e_0350;
pub const SCENARIO_POINTS: u32 = 0x1e0;
pub const POINT_LEN: u32 = 0x38;
pub const POINT_AT: u32 = 0x24;

/// The notification insert (E `0x0024cbee`): EAX the local user, the
/// UTF-16 text's address its one stack argument (`ret 4`); it copies 62
/// units into a slot of the user's list (docs/game/hud-kit.md,
/// "Messages").
pub const NOTIFY: u32 = 0x0024_cbee;
/// Local users the game keeps HUD state for.
pub const USERS: u32 = 4;
/// Our guest memory for the texts of a tick's notifications: one slot
/// each of [`NOTE_LEN`] bytes.
pub const NOTES_LEN: u32 = 0x1000;
/// One text's slot: room for [`MESSAGE_UNITS`] units and the zero, and
/// one unit more, so 32 slots fill the memory.
pub const NOTE_LEN: u32 = 0x80;
const _: () = assert!(NOTE_LEN as usize >= (MESSAGE_UNITS + 1) * 2);
/// The notifications sent at one tick at most; more wait for the next.
pub const NOTES_PER_TICK: usize = (NOTES_LEN / NOTE_LEN) as usize;

/// The dedicated lobby's feature, whose tick hook shares the first group
/// (`host_lobby`).
const HOST_LOBBY: &str = "host-lobby";

/// The kit's string ids (`ui_kit::ids`): the training text's.
pub const IDS: Family = family("hud kit");
pub const TRAINING_TEXT_ID: u32 = IDS.id(0);

// ---- the registry ----

struct Registered {
    describe: Arc<dyn Describe + Send + Sync>,
    hud: Hud,
    values: Values,
    /// When each moving value last changed ([`stamp_values`]).
    stamps: Stamps,
    /// The world point of each navpoint element that has one, by its id.
    navs: Vec<(&'static str, [f32; 3])>,
    /// The pose of each world mark element that has one, by its id.
    marks: Vec<(&'static str, Pose)>,
    /// A local user's own values, drawn for that user's HUD in place of
    /// [`Registered::values`] ([`Handle::publish_for`]).
    own: Vec<Own>,
}

/// One local user's own values of a HUD, stamped as the shared ones are.
#[derive(Clone, Debug)]
pub struct Own {
    pub user: u32,
    pub values: Values,
    pub stamps: Stamps,
}

/// The values the widget hooks draw for local user `user`: its own, else
/// the shared ones.
pub fn values_for<'a>(
    shared: (&'a Values, &'a Stamps),
    own: &'a [Own],
    user: u32,
) -> (&'a Values, &'a Stamps) {
    own.iter()
        .find(|o| o.user == user)
        .map_or(shared, |o| (&o.values, &o.stamps))
}

/// How many world marks have a pose, over every registered HUD: what the
/// world-line source polls ([`crate::hud_kit_world`]).
static MARKS_POSED: AtomicUsize = AtomicUsize::new(0);

/// A world mark as it draws now: its element's shape, its pose, and its
/// value's style and label.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkNow {
    pub id: &'static str,
    pub shape: Shape,
    pub pose: Pose,
    pub style: Style,
    pub label: String,
}

/// Per element, the wall time its value's motion started.
pub type Stamps = Vec<(&'static str, Instant)>;

/// Set once the effects hook has met a kit widget at the bitmap draw's
/// call: the game calls it for a widget with an empty effects block, so
/// the scale ([`scale_factors`]) applies. Until then a motion's ending is
/// the plain one (`motion::shown_at` unscaled): a bar that would close
/// with no scale to close it goes where the close would start.
pub static EFFECTS_SEEN: AtomicBool = AtomicBool::new(false);

fn scaled() -> bool {
    EFFECTS_SEEN.load(Ordering::Relaxed)
}

/// Milliseconds from `then` to `now`.
pub fn ms_since(then: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(then).as_secs_f32() * 1000.0
}

/// `new` against the values before it (`old`, stamped `stamps`), at
/// `now`: a moving value that changed is stamped `now`, from the fill the
/// old one showed then ([`motion::restamp`]); one that did not keeps its
/// stamp and its `from`. A value with no motion is kept as given and
/// carries no stamp.
pub fn stamp_values(
    old: &Values,
    stamps: &[(&'static str, Instant)],
    new: Values,
    now: Instant,
    scaled: bool,
) -> (Values, Stamps) {
    let mut out = Values::default();
    let mut out_stamps = Stamps::new();
    for (id, v) in new.0 {
        if v.motion == Motion::None {
            out.0.push((id, v));
            continue;
        }
        let stamp = stamps.iter().find(|(i, _)| *i == id).map(|(_, t)| *t);
        let before = old.get(id);
        let prior = before.map(|o| (o, stamp.map_or(f32::INFINITY, |t| ms_since(t, now))));
        match motion::restamp(prior, &v, scaled) {
            Some(stamped) => {
                out.0.push((id, stamped));
                out_stamps.push((id, now));
            }
            None => {
                let from = before.map_or(0.0, |o| o.from);
                out.0.push((id, Value { from, ..v }));
                out_stamps.push((id, stamp.unwrap_or(now)));
            }
        }
    }
    (out, out_stamps)
}

/// `v` as it draws at `now`, stamped `stamp`: on its motion's curve.
pub fn animated(v: Value, stamp: Option<Instant>, now: Instant, scaled: bool) -> Value {
    if v.motion == Motion::None {
        return v;
    }
    let ms = stamp.map_or(f32::INFINITY, |t| ms_since(t, now));
    motion::animate(&v, ms, scaled)
}

static REGISTRY: RwLock<Vec<Registered>> = RwLock::new(Vec::new());
/// Notifications waiting for the next tick's top.
static NOTES: Mutex<Vec<Note>> = Mutex::new(Vec::new());
/// Set while the tick the notifications' hook runs again is running.
static NOTES_REENTRY: AtomicBool = AtomicBool::new(false);

/// A notification for a local user, sent at the next tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub user: u32,
    pub text: String,
}
static TRAINING_TEXT: Mutex<String> = Mutex::new(String::new());
/// The banner the last tick showed each local user ([`banner_due`]).
static BANNER_LAST: Mutex<[Option<String>; USERS as usize]> =
    Mutex::new([None, None, None, None]);

fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A registered HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(usize);

/// Registers `describe`'s HUD; a HUD of the same name already registered
/// keeps its place. Every element starts hidden.
pub fn register(describe: Arc<dyn Describe + Send + Sync>) -> Handle {
    let hud = describe.hud();
    let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = r.iter().position(|x| x.hud.name == hud.name) {
        return Handle(i);
    }
    let mut values = Values::default();
    for e in &hud.elements {
        values.set(e.id, Value::hidden());
    }
    r.push(Registered {
        describe,
        hud,
        values,
        stamps: Stamps::new(),
        navs: Vec::new(),
        marks: Vec::new(),
        own: Vec::new(),
    });
    Handle(r.len() - 1)
}

impl Handle {
    /// The values the hooks give from now on, a moving value that
    /// changed stamped with the time ([`stamp_values`]).
    pub fn publish(&self, values: Values) {
        let now = Instant::now();
        let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = r.get_mut(self.0) {
            let (values, stamps) = stamp_values(&x.values, &x.stamps, values, now, scaled());
            x.values = values;
            x.stamps = stamps;
        }
    }
    /// Local user `user`'s own values, drawn by the widget hooks for that
    /// user's HUD in place of the shared ones ([`Handle::publish`]); `None`
    /// takes them away, so the user draws the shared values again. The
    /// banner reads them too, so each user's timed line is its own
    /// ([`banner_now`]); the other channels (prompt, training, navpoints,
    /// world marks) read the shared values alone. A user past [`USERS`] is
    /// ignored.
    pub fn publish_for(&self, user: u32, values: Option<Values>) {
        if user >= USERS {
            return;
        }
        let now = Instant::now();
        let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        let Some(x) = r.get_mut(self.0) else {
            return;
        };
        let at = x.own.iter().position(|o| o.user == user);
        let Some(values) = values else {
            if let Some(i) = at {
                x.own.remove(i);
            }
            return;
        };
        let (old, stamps) = match at {
            Some(i) => (&x.own[i].values, &x.own[i].stamps),
            None => (&x.values, &x.stamps),
        };
        let (values, stamps) = stamp_values(old, stamps, values, now, scaled());
        let own = Own {
            user,
            values,
            stamps,
        };
        match at {
            Some(i) => x.own[i] = own,
            None => x.own.push(own),
        }
    }
    /// Whether local user `user`'s retail widgets are drawn: `false` while
    /// the user is in monitor mode (docs/game/forge-firefight-ui.md, "The
    /// monitor HUD"), when the gate hides every retail widget of that
    /// user's HUD, the player's and the weapon's, but the motion tracker
    /// (`model::MONITOR_KEEPS`, [`retail_gate`]). The kit's own widgets
    /// and the other users' are as they were. A user past [`USERS`] is
    /// ignored; each install shows them all again.
    pub fn retail(&self, user: u32, shown: bool) {
        if user >= USERS {
            return;
        }
        if shown {
            RETAIL_OFF.fetch_and(!(1 << user), Ordering::Relaxed);
        } else {
            RETAIL_OFF.fetch_or(1 << user, Ordering::Relaxed);
        }
    }
    /// Whether the loaded map's player HUD has this HUD's widgets: it was
    /// compiled and installed at the map's activation. `false` before a
    /// map loads, on a map it is not for ([`Describe::multiplayer_only`]),
    /// with a template missing, or in a build with no map reader; the
    /// install logs which.
    pub fn joined(&self) -> bool {
        read(&TABLE).widgets.iter().any(|w| w.hud == self.0)
    }
    /// Queues `text` as a notification for local user `user`, one line of
    /// the game's list, as a retail pickup or medal line is. Sent at the
    /// top of the next tick through the game's insert ([`NOTIFY`]); two
    /// calls are two lines. Refused (false, and logged) for a user over
    /// [`USERS`], an empty text, or one [`message_fault`] refuses (over 62
    /// units, more than one line).
    pub fn notify(&self, user: u32, text: &str) -> bool {
        notify(user, text)
    }
    /// The world point the HUD's navpoint is over, if it shows: its
    /// first navpoint element's ([`Handle::navpoint_at`]).
    pub fn navpoint(&self, at: Option<[f32; 3]>) {
        let first = {
            let r = read(&REGISTRY);
            r.get(self.0).and_then(|x| {
                x.hud
                    .elements
                    .iter()
                    .find(|e| matches!(e.kind, Kind::Navpoint { .. }))
                    .map(|e| e.id)
            })
        };
        if let Some(id) = first {
            self.navpoint_at(id, at);
        }
    }
    /// The world point navpoint element `id` is over; `None` takes it
    /// away. The game draws it while the element's value is shown too. An
    /// id that is not a navpoint of this HUD is ignored.
    pub fn navpoint_at(&self, id: &str, at: Option<[f32; 3]>) {
        let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        let Some(x) = r.get_mut(self.0) else {
            return;
        };
        let Some(e) = x
            .hud
            .elements
            .iter()
            .find(|e| e.id == id && matches!(e.kind, Kind::Navpoint { .. }))
        else {
            return;
        };
        let id = e.id;
        x.navs.retain(|(i, _)| *i != id);
        if let Some(at) = at {
            x.navs.push((id, at));
        }
    }
    /// Where world mark element `id` lies; `None` takes it away. The
    /// window's world-line pass draws it while the element's value is
    /// shown too ([`world_marks_now`]). An id that is not a world mark of
    /// this HUD, and a pose with a fault (`Pose::fault`), are ignored.
    pub fn world_mark(&self, id: &str, pose: Option<Pose>) {
        let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        let Some(x) = r.get_mut(self.0) else {
            return;
        };
        let Some(e) = x
            .hud
            .elements
            .iter()
            .find(|e| e.id == id && e.kind.world().is_some())
        else {
            return;
        };
        let id = e.id;
        x.marks.retain(|(i, _)| *i != id);
        if let Some(pose) = pose.filter(|p| p.fault().is_none()) {
            x.marks.push((id, pose));
        }
        MARKS_POSED.store(r.iter().map(|x| x.marks.len()).sum(), Ordering::Relaxed);
    }
}

/// Whether any world mark has a pose: one atomic load.
pub fn any_world_marks() -> bool {
    MARKS_POSED.load(Ordering::Relaxed) != 0
}

/// The world marks to draw now in screen mode `mode`: the world mark
/// elements of the registered HUDs, in their order, whose value is shown,
/// that have a pose and whose place has the mode (a channel shows only
/// in the screen modes its place has).
pub fn world_marks_now(mode: Mode) -> Vec<MarkNow> {
    let r = read(&REGISTRY);
    r.iter()
        .flat_map(|x| world_marks_of(&x.hud, &x.values, &x.marks, mode))
        .collect()
}

/// [`world_marks_now`] for one HUD at `values` with the poses `marks`.
pub fn world_marks_of(
    hud: &Hud,
    values: &Values,
    marks: &[(&'static str, Pose)],
    mode: Mode,
) -> Vec<MarkNow> {
    let mut out = Vec::new();
    for e in &hud.elements {
        let Some(shape) = e.kind.world() else { continue };
        let Some(v) = values.get(e.id).filter(|v| v.shown) else {
            continue;
        };
        let Some((_, pose)) = marks.iter().find(|(i, _)| *i == e.id) else {
            continue;
        };
        if e.at.offset(mode).is_none() {
            continue;
        }
        out.push(MarkNow {
            id: e.id,
            shape,
            pose: *pose,
            style: v.style,
            label: v.text.clone(),
        });
    }
    out
}

/// Why a notification for `user` with `text` is refused, if it is.
pub fn note_fits(user: u32, text: &str) -> Result<(), String> {
    if user >= USERS {
        return Err(format!("no local user {user}"));
    }
    if text.is_empty() {
        return Err("no text".into());
    }
    match message_fault(text) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// A notification for a local user from a sender with no HUD of its own,
/// queued as [`Handle::notify`] queues one.
pub fn notify(user: u32, text: &str) -> bool {
    if let Err(why) = note_fits(user, text) {
        eprintln!("hud kit: notification {text:?} for user {user} not sent: {why}");
        return false;
    }
    lock(&NOTES).push(Note {
        user,
        text: text.to_owned(),
    });
    true
}

/// The queued notifications, oldest first, at most [`NOTES_PER_TICK`]
/// of them; the rest stay queued.
pub fn take_notes() -> Vec<Note> {
    let mut q = lock(&NOTES);
    let n = q.len().min(NOTES_PER_TICK);
    q.drain(..n).collect()
}

/// Writes each note's text, UTF-16 and zero ended, into its slot of
/// `region` (`(start, len)`), and returns the insert's call for each:
/// EAX the user, the slot's address the argument.
pub fn stage_notes(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
    notes: &[Note],
) -> Result<Vec<GuestCall>, MemFault> {
    let slots = (region.1 / NOTE_LEN) as usize;
    let mut calls = Vec::new();
    for (i, n) in notes.iter().take(slots).enumerate() {
        let at = region.0 + i as u32 * NOTE_LEN;
        mem.write(at, &utf16(&n.text, NOTE_LEN as usize / 2))?;
        calls.push(notify_call(n.user, at));
    }
    Ok(calls)
}

/// The insert's call for `user` with the text at `at`.
pub fn notify_call(user: u32, at: u32) -> GuestCall {
    GuestCall::new(NOTIFY).eax(user).args([at])
}

/// Makes the calls, then runs the tick itself (the hook's chain again,
/// which passes the notifications' hook by [`NOTES_REENTRY`]).
async fn send_notes(
    cx: guest_layout::entry::Cx,
    region: (u32, u32),
    notes: Vec<Note>,
) -> Result<Ret, guest_layout::entry::EntryError> {
    let calls = cx.write(|m| stage_notes(m, region, &notes))?;
    for call in calls {
        cx.call(call).await?;
    }
    NOTES_REENTRY.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await;
    NOTES_REENTRY.store(false, Ordering::SeqCst);
    r?;
    Ok(Ret::new(0, 0))
}

/// HUD `hud`'s value for element `id` in local user `user`'s HUD, as it
/// draws now ([`animated`]).
fn value(hud: usize, user: u32, id: &str) -> Option<Value> {
    let r = read(&REGISTRY);
    let x = r.get(hud)?;
    let (values, stamps) = values_for((&x.values, &x.stamps), &x.own, user);
    let v = values.get(id).cloned()?;
    let stamp = stamps.iter().find(|(i, _)| *i == id).map(|(_, t)| *t);
    Some(animated(v, stamp, Instant::now(), scaled()))
}

/// HUD `hud`'s element `id`'s scale now in local user `user`'s HUD, wide
/// and tall
/// (`motion::scale_at`).
fn scale(hud: usize, user: u32, id: &str) -> [f32; 2] {
    let r = read(&REGISTRY);
    let Some(x) = r.get(hud) else {
        return [1.0, 1.0];
    };
    let (values, stamps) = values_for((&x.values, &x.stamps), &x.own, user);
    let (Some(v), Some(stamp)) = (
        values.get(id),
        stamps.iter().find(|(i, _)| *i == id).map(|(_, t)| *t),
    ) else {
        return [1.0, 1.0];
    };
    motion::scale_at(v.motion, ms_since(stamp, Instant::now()))
}

/// The first shown text of a channel `want` picks, over the registered
/// HUDs in their order.
fn channel(want: fn(&Kind) -> bool) -> Option<String> {
    let r = read(&REGISTRY);
    r.iter().find_map(|x| {
        x.hud
            .elements
            .iter()
            .filter(|e| want(&e.kind))
            .find_map(|e| {
                x.values
                    .get(e.id)
                    .filter(|v| v.shown)
                    .map(|v| v.text.clone())
            })
    })
}

/// The first shown banner of `hud` at `values` in screen mode `mode`: a
/// [`Kind::Banner`] element whose place has the mode and whose value is
/// shown. A text [`banner_fault`] refuses is no banner (and is logged).
pub fn banner_of(hud: &Hud, values: &Values, mode: Mode) -> Option<String> {
    hud.elements
        .iter()
        .filter(|e| matches!(e.kind, Kind::Banner { .. }) && e.at.offset(mode).is_some())
        .filter_map(|e| values.get(e.id).filter(|v| v.shown))
        .find_map(|v| match banner_fault(&v.text) {
            Some(why) => {
                eprintln!("hud kit: banner {:?} not shown: {why}", v.text);
                None
            }
            None => Some(v.text.clone()),
        })
}

/// Local user `user`'s banner over the registered HUDs in their order
/// ([`banner_of`]), each at the values that user's HUD draws
/// ([`values_for`]): its own, else the shared ones.
fn banner_now(mode: Mode, user: u32) -> Option<String> {
    let r = read(&REGISTRY);
    r.iter()
        .find_map(|x| banner_for(&x.hud, (&x.values, &x.stamps), &x.own, user, mode))
}

/// One HUD's banner for local user `user` in screen mode `mode`: the
/// banner ([`banner_of`]) of the user's own values, else of the shared.
pub fn banner_for(
    hud: &Hud,
    shared: (&Values, &Stamps),
    own: &[Own],
    user: u32,
    mode: Mode,
) -> Option<String> {
    banner_of(hud, values_for(shared, own, user).0, mode)
}

/// Whether the banner `now` is to be written this tick, given the one the
/// last tick showed (`last`, updated): a shown text that differs from the
/// last tick's, or follows none. The same text again keeps the line the
/// game is already showing; no banner lets it run out.
pub fn banner_due(last: &mut Option<String>, now: Option<String>) -> Option<String> {
    let due = match (&now, &*last) {
        (Some(t), Some(l)) if t == l => None,
        (Some(t), _) => Some(t.clone()),
        (None, _) => None,
    };
    *last = now;
    due
}

/// The first slot of each HUD's navpoints, for HUDs in registration order
/// with `counts` navpoints each, and `exclusive(a, b)` whether HUDs `a` and
/// `b` never show together: a HUD starts after the navpoints of the
/// earlier HUDs it can show with, so HUDs of different modes (a biped's,
/// a monitor's) share the first slots instead of using them up.
pub fn slot_bases(counts: &[usize], exclusive: impl Fn(usize, usize) -> bool) -> Vec<usize> {
    let mut bases: Vec<usize> = Vec::with_capacity(counts.len());
    for (i, _) in counts.iter().enumerate() {
        let base = (0..i)
            .filter(|&j| !exclusive(i, j))
            .map(|j| bases[j] + counts[j])
            .max()
            .unwrap_or(0);
        bases.push(base);
    }
    bases
}

/// The point of each of the kit's navpoints now: the navpoint elements
/// of the registered HUDs in their order, each HUD after the navpoints of
/// the earlier ones it can show with ([`slot_bases`]), the first
/// [`KIT_POINTS`] of them, each `Some` while its value is shown and it
/// has a point. An element keeps its place in the list whatever shows, so
/// it keeps its scenario point and its slot.
pub fn navpoints_now() -> [Option<[f32; 3]>; KIT_POINTS as usize] {
    let r = read(&REGISTRY);
    let counts: Vec<usize> = r
        .iter()
        .map(|x| {
            x.hud
                .elements
                .iter()
                .filter(|e| matches!(e.kind, Kind::Navpoint { .. }))
                .count()
        })
        .collect();
    let never = |a: usize, b: usize| {
        r[a].describe.never_with().contains(&r[b].hud.name)
            || r[b].describe.never_with().contains(&r[a].hud.name)
    };
    let bases = slot_bases(&counts, never);
    let mut out = [None; KIT_POINTS as usize];
    for (x, base) in r.iter().zip(bases) {
        let ids = x
            .hud
            .elements
            .iter()
            .filter(|e| matches!(e.kind, Kind::Navpoint { .. }))
            .map(|e| e.id);
        for (id, k) in ids.zip(base..) {
            let shown = x.values.get(id).is_some_and(|v| v.shown);
            let at = x
                .navs
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, at)| *at)
                .filter(|_| shown);
            if let Some(slot) = out.get_mut(k) {
                *slot = slot.or(at);
            }
        }
    }
    out
}

/// The string lookup's answer for the kit's ids (`0x001a0180`, through
/// `community_variants`' override).
pub fn text(id: u32) -> Option<String> {
    (id == TRAINING_TEXT_ID).then(|| game_line_breaks(&lock(&TRAINING_TEXT)))
}

/// `text` with each line break as the game's strings write it, `\r\n`:
/// the game's text draw passes a lone `\n` with no break and no space
/// (the training box drew "lip,let go"; docs/live/live.md, the game's
/// own strings, `h2 strings 01a_tutorial`). The kit's model writes `\n`.
pub fn game_line_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Whether `id` is one of the kit's string ids.
pub fn owns(id: u32) -> bool {
    IDS.owns(id)
}

// ---- the draw table ----

/// One kit widget in guest memory.
#[derive(Clone, Debug, PartialEq)]
pub struct KitWidget {
    pub address: u32,
    /// Its HUD's index in the registry.
    pub hud: usize,
    pub id: &'static str,
    pub kind: WidgetKind,
    pub widget: records::Widget,
    /// Whether its place has an offset in each screen mode.
    pub modes: [bool; 3],
    /// A text's engine font in each screen mode (-1 hidden).
    pub font: [i16; 3],
    /// Its element's `hide_when_d`: word D bits that hide it.
    pub hide_d: u16,
}

/// The kit widgets installed, by address, and the retail widgets a user
/// in monitor mode still draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub widgets: Vec<KitWidget>,
    /// The addresses of the retail bitmap records the game draws for the
    /// player `nhdt`s that `model::MONITOR_KEEPS` names (the motion
    /// tracker's): the copies when the root points at them, else the
    /// tag's own ([`kept_records`]).
    pub kept: Vec<u32>,
}

impl Table {
    pub fn at(&self, address: u32) -> Option<&KitWidget> {
        self.widgets.iter().find(|w| w.address == address)
    }
}

static TABLE: RwLock<Table> = RwLock::new(Table {
    widgets: Vec::new(),
    kept: Vec::new(),
});
/// One bit per local user whose retail widgets are off: the user is in
/// monitor mode ([`Handle::retail`]).
static RETAIL_OFF: AtomicU32 = AtomicU32::new(0);

/// Whether local user `user`'s retail widgets are drawn
/// ([`Handle::retail`]).
pub fn retail_shown(user: u32) -> bool {
    user >= USERS || RETAIL_OFF.load(Ordering::Relaxed) & (1 << user) == 0
}

/// Our region's bounds, so a retail widget costs two loads.
static REGION_START: AtomicU32 = AtomicU32::new(0);
static REGION_END: AtomicU32 = AtomicU32::new(0);

fn in_region(address: u32) -> bool {
    let (s, e) = (
        REGION_START.load(Ordering::Relaxed),
        REGION_END.load(Ordering::Relaxed),
    );
    s != 0 && address >= s && address < e
}

/// The local user whose HUD is being drawn ([`DRAW_USER`]); 0 when it
/// cannot be read or is no local user.
pub fn draw_user(mem: &dyn GuestMemory) -> u32 {
    mem.u32(DRAW_USER).ok().filter(|u| *u < USERS).unwrap_or(0)
}

/// The screen mode the HUD draws in: one local view fullscreen, two half,
/// more quarter (`player_globals +8`, as `h1_hud_text::weapon` reads it;
/// S: that the count is the local views').
pub fn mode(mem: &dyn GuestMemory) -> Result<Mode, MemFault> {
    let globals = mem.u32(points::PLAYER_GLOBALS.base)?;
    if globals == 0 {
        return Ok(Mode::Full);
    }
    Ok(match mem.u16(globals + 8)? {
        0 | 1 => Mode::Full,
        2 => Mode::Half,
        _ => Mode::Quarter,
    })
}

/// The styles the runtime draws: `Normal`, and `Flash` as `Normal` (the
/// flash is slice 4's). `Dim` and `Inactive` need the colour hook, also
/// slice 4's, and are not drawn until then.
pub fn draws(style: Style) -> bool {
    matches!(style, Style::Normal | Style::Flash(_))
}

/// Whether kit widget `w` is drawn at `v` in `mode`.
pub fn shown(w: &KitWidget, v: Option<&Value>, mode: Mode) -> bool {
    w.modes[mode.index()] && v.is_some_and(|v| v.shown && draws(v.style))
}

// ---- install ----

/// What a root held before the kit's first install: its two blocks'
/// counts and addresses. Kept in guest memory at the region's head:
/// [`HEADER_MAGIC`], the count of roots, then [`HEADER_ROOT_LEN`] bytes a
/// root (its address, then the four words).
pub const HEADER_MAGIC: u32 = u32::from_le_bytes(*b"hkit");
pub const HEADER_ROOTS: u32 = 16;
pub const HEADER_ROOT_LEN: u32 = 20;
pub const HEADER_LEN: u32 = 0x200;
const _: () = assert!(8 + HEADER_ROOTS * HEADER_ROOT_LEN <= HEADER_LEN);

/// One root's retail fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub root: u32,
    /// Bitmap count, bitmap address, text count, text address.
    pub retail: [u32; 4],
}

fn read_headers(mem: &dyn GuestMemory, region: u32) -> Result<Vec<Header>, MemFault> {
    if mem.u32(region)? != HEADER_MAGIC {
        return Ok(Vec::new());
    }
    let n = mem.u32(region + 4)?.min(HEADER_ROOTS);
    (0..n)
        .map(|i| {
            let at = region + 8 + i * HEADER_ROOT_LEN;
            let mut retail = [0; 4];
            for (k, w) in retail.iter_mut().enumerate() {
                *w = mem.u32(at + 4 + 4 * k as u32)?;
            }
            Ok(Header {
                root: mem.u32(at)?,
                retail,
            })
        })
        .collect()
}

fn write_headers(mem: &mut dyn GuestMemory, region: u32, h: &[Header]) -> Result<(), MemFault> {
    mem.set_u32(region, HEADER_MAGIC)?;
    mem.set_u32(region + 4, h.len() as u32)?;
    for (i, x) in h.iter().enumerate() {
        let at = region + 8 + i as u32 * HEADER_ROOT_LEN;
        mem.set_u32(at, x.root)?;
        for (k, w) in x.retail.iter().enumerate() {
            mem.set_u32(at + 4 + 4 * k as u32, *w)?;
        }
    }
    Ok(())
}

const FIELDS: [u32; 2] = [records::ROOT_BITMAPS, records::ROOT_TEXTS];

/// Puts back what each root held before the last install, where the root
/// still points into our region (a reloaded tag holds its own fields).
pub fn restore(mem: &mut dyn GuestMemory, region: (u32, u32)) -> Result<usize, MemFault> {
    let (start, len) = region;
    let mut restored = 0;
    for h in read_headers(&*mem, start)? {
        for (k, field) in FIELDS.iter().enumerate() {
            let address = mem.u32(h.root + field + 4)?;
            if address >= start && address < start + len {
                mem.set_u32(h.root + field, h.retail[2 * k])?;
                mem.set_u32(h.root + field + 4, h.retail[2 * k + 1])?;
                restored += 1;
            }
        }
    }
    mem.set_u32(start, 0)?;
    Ok(restored)
}

/// Copies the retail widget records again into the copies the root
/// points at, for every installed root whose retail block overlaps
/// `[start, end)`: a live edit wrote those records
/// (docs/editor/realtime-edits.md, "Realtime coverage"), and the game
/// draws the copy. A root that no longer points into `region` (a reloaded
/// tag) draws its own block and is left alone. How many blocks were
/// copied.
pub fn recopy(mem: &mut dyn GuestMemory, region: (u32, u32), start: u32, end: u32) -> Result<usize, MemFault> {
    let (first, len) = region;
    let mut copied = 0;
    for h in read_headers(&*mem, first)? {
        for (k, kind) in [WidgetKind::Bitmap, WidgetKind::Text].into_iter().enumerate() {
            let (count, from) = (h.retail[2 * k], h.retail[2 * k + 1]);
            let bytes = count * kind.size() as u32;
            if bytes == 0 || from >= end || start >= from.saturating_add(bytes) {
                continue;
            }
            let copy = mem.u32(h.root + FIELDS[k] + 4)?;
            if copy < first || copy >= first + len {
                continue;
            }
            let mut b = vec![0; bytes as usize];
            mem.read(from, &mut b)?;
            mem.write(copy, &b)?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// [`recopy`] over the installed region, for the hot-edit glue's notice
/// of an `nhdt` write; nothing before the kit is installed.
pub fn tag_written(mem: &mut dyn GuestMemory, start: u32, end: u32) -> Result<usize, MemFault> {
    let first = REGION_START.load(Ordering::Relaxed);
    if first == 0 {
        return Ok(0);
    }
    recopy(mem, (first, REGION_END.load(Ordering::Relaxed) - first), start, end)
}

/// One player `nhdt`'s root and the HUDs that join it.
pub struct Join<'a> {
    pub root: u32,
    /// Each HUD's registry index, the HUD and its compiled records.
    pub huds: Vec<(usize, &'a Hud, &'a Compiled)>,
}

#[derive(Debug, PartialEq)]
pub enum InstallError {
    Fault(MemFault),
    /// The blocks need more than the region has.
    Full {
        need: u32,
        have: u32,
    },
    TooManyRoots(usize),
}

impl From<MemFault> for InstallError {
    fn from(f: MemFault) -> Self {
        InstallError::Fault(f)
    }
}

fn font_of(w: &records::Widget) -> [i16; 3] {
    if w.kind != WidgetKind::Text {
        return [-1; 3];
    }
    Mode::ALL.map(|m| match w.bytes[records::text::FONTS + m.index()] as i8 {
        0 => hud_kit_core::model::Font::Hud.engine(),
        1 => hud_kit_core::model::Font::Lcd.engine(),
        _ => -1,
    })
}

/// Puts back the last install ([`restore`]), then for each join copies the
/// root's two blocks into `region` with the HUDs' records of that kind
/// after them, in registry order, and points the root at the copies. The
/// draw table of the kit widgets written.
pub fn install_records(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
    joins: &[Join],
) -> Result<Table, InstallError> {
    let (start, len) = region;
    restore(mem, region)?;
    if joins.len() > HEADER_ROOTS as usize {
        return Err(InstallError::TooManyRoots(joins.len()));
    }
    let mut headers = Vec::new();
    let mut need = HEADER_LEN;
    for j in joins {
        let mut retail = [0; 4];
        for (k, field) in FIELDS.iter().enumerate() {
            retail[2 * k] = mem.u32(j.root + field)?.min(0x400);
            retail[2 * k + 1] = mem.u32(j.root + field + 4)?;
        }
        for (k, kind) in [WidgetKind::Bitmap, WidgetKind::Text]
            .into_iter()
            .enumerate()
        {
            need += retail[2 * k] * kind.size() as u32;
            for (_, _, c) in &j.huds {
                need += c.of(kind).map(|w| w.bytes.len() as u32).sum::<u32>();
            }
        }
        headers.push(Header {
            root: j.root,
            retail,
        });
    }
    if need > len {
        return Err(InstallError::Full { need, have: len });
    }
    let mut table = Table::default();
    let mut cursor = start + HEADER_LEN;
    for (j, h) in joins.iter().zip(&headers) {
        for (k, kind) in [WidgetKind::Bitmap, WidgetKind::Text]
            .into_iter()
            .enumerate()
        {
            let (count, from) = (h.retail[2 * k], h.retail[2 * k + 1]);
            let block = cursor;
            let retail_len = count * kind.size() as u32;
            if retail_len > 0 {
                let mut bytes = vec![0; retail_len as usize];
                mem.read(from, &mut bytes)?;
                mem.write(cursor, &bytes)?;
                cursor += retail_len;
            }
            let mut added = 0;
            for (index, hud, c) in &j.huds {
                for w in c.of(kind) {
                    mem.write(cursor, &w.bytes)?;
                    let element = hud.elements.iter().find(|e| e.id == w.id);
                    let modes =
                        element.map_or([false; 3], |e| Mode::ALL.map(|m| e.at.offset(m).is_some()));
                    table.widgets.push(KitWidget {
                        address: cursor,
                        hud: *index,
                        id: w.id,
                        kind,
                        widget: w.clone(),
                        modes,
                        font: font_of(w),
                        hide_d: element.map_or(0, |e| e.hide_when_d),
                    });
                    cursor += w.bytes.len() as u32;
                    added += 1;
                }
            }
            if added > 0 {
                let field = FIELDS[k];
                mem.set_u32(j.root + field, count + added)?;
                mem.set_u32(j.root + field + 4, block)?;
            }
        }
    }
    write_headers(mem, start, &headers)?;
    Ok(table)
}

// ---- the walk's seam ----

/// The gate's answer for kit widget `widget` (a bitmap or text of the
/// walk), when it is the kit's: [`shown`], and hidden while the walk's
/// word D has a bit of the widget's `hide_d`.
pub fn gate(
    mem: &dyn GuestMemory,
    widget: u32,
    word_d: u16,
    table: &Table,
    values: &dyn Fn(usize, &str) -> Option<Value>,
) -> Result<Option<bool>, MemFault> {
    let Some(w) = table.at(widget) else {
        return Ok(None);
    };
    if w.hide_d != 0 && word_d & w.hide_d != 0 {
        return Ok(Some(false));
    }
    Ok(Some(shown(w, values(w.hud, w.id).as_ref(), mode(mem)?)))
}

/// The gate's answer for a widget that is not the kit's, for local user
/// `user` ([`draw_user`]) when its bit of `off` is set (its retail
/// widgets are off, [`Handle::retail`]): hidden, unless it is a record
/// the motion tracker draws from ([`Table::kept`]). `None` leaves it to
/// the game's own gate.
pub fn retail_gate(widget: u32, table: &Table, off: u32, user: u32) -> Option<bool> {
    if user >= USERS || off & (1 << user) == 0 {
        return None;
    }
    if table.at(widget).is_some() || table.kept.contains(&widget) {
        return None;
    }
    Some(false)
}

/// The addresses the game draws the kept retail bitmap records from
/// ([`Table::kept`]): for each player `nhdt` root, its bitmap block's
/// address now (the kit's copy after an install, whose retail records
/// come first in the same order, else the tag's own) plus each kept
/// index's record; an index past the block's count is skipped.
pub fn kept_records(mem: &dyn GuestMemory, roots: &[(u32, Vec<u32>)]) -> Result<Vec<u32>, MemFault> {
    let mut out = Vec::new();
    for (root, indices) in roots {
        let count = mem.u32(root + records::ROOT_BITMAPS)?;
        let block = mem.u32(root + records::ROOT_BITMAPS + 4)?;
        for &i in indices.iter().filter(|i| **i < count) {
            out.push(block + i * records::BITMAP_SIZE as u32);
        }
    }
    Ok(out)
}

/// A kit bitmap widget's basis factors, across and down: its `scale`,
/// applied to the walk's basis about its registration point before the
/// widget's effects. `Some` for a kit bitmap widget whatever its scale
/// (the walk applies it, and [`EFFECTS_SEEN`] is set); a factor that is
/// not finite is left as 1.
pub fn scale_factors(
    widget: u32,
    table: &Table,
    scale: &dyn Fn(usize, &str) -> [f32; 2],
) -> Option<[f32; 2]> {
    let w = table.at(widget).filter(|w| w.kind == WidgetKind::Bitmap)?;
    Some(scale(w.hud, w.id).map(|k| if k.is_finite() { k.max(0.0) } else { 1.0 }))
}

/// `hud_input1` for a kit widget: its value's fill through
/// `records::Widget::input`, the other three 0.
pub fn inputs(
    widget: u32,
    table: &Table,
    values: &dyn Fn(usize, &str) -> Option<Value>,
) -> Option<[f32; 4]> {
    let w = table.at(widget)?;
    let fill = values(w.hud, w.id).map_or(0.0, |v| v.fill);
    Some([w.widget.input(fill), 0.0, 0.0, 0.0])
}

/// UTF-16 of `text`, cut to `units` with its zero.
pub fn utf16(text: &str, units: usize) -> Vec<u8> {
    text.encode_utf16()
        .take(units.saturating_sub(1))
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A kit text widget's string and engine font: a text's is its value's
/// text, a gauge's the character of its picture for the value's fill as
/// it draws now (`records::Widget::text`); at most [`STRING_UNITS`] units
/// less one.
pub fn string(
    mem: &dyn GuestMemory,
    widget: u32,
    table: &Table,
    values: &dyn Fn(usize, &str) -> Option<Value>,
) -> Result<Option<(Vec<u16>, i16)>, MemFault> {
    let Some(w) = table.at(widget).filter(|w| w.kind == WidgetKind::Text) else {
        return Ok(None);
    };
    let text = values(w.hud, w.id)
        .map(|v| w.widget.text(&v))
        .unwrap_or_default();
    let font = w.font[mode(mem)?.index()];
    let font = if font < 0 {
        hud_kit_core::model::Font::Hud.engine()
    } else {
        font
    };
    let units = text.encode_utf16().take(STRING_UNITS - 1).collect();
    Ok(Some((units, font)))
}

/// `[l, r, t, b]` narrowed to `crop` (`[u0, u1, v0, v1]` of it).
pub fn narrow(b: [f32; 4], crop: [f32; 4]) -> [f32; 4] {
    let (w, h) = (b[1] - b[0], b[3] - b[2]);
    [
        b[0] + crop[0] * w,
        b[0] + crop[1] * w,
        b[2] + crop[2] * h,
        b[2] + crop[3] * h,
    ]
}

/// The image and bounds (`[l, r, t, b]` of the image) bitmap `bitmap`
/// draws at sequence `seq`, as the game's pick gives them (E `0x0022ae21`,
/// `0x0022ae94`..`0x0022af3c`; `hud_edit::render::sprite_at`): the
/// sequence's first sprite; a sequence with no sprites, its first image
/// whole; no such sequence, image 0 whole. `None` for an image the bitmap
/// does not have.
pub fn sprite_pick(
    mem: &dyn GuestMemory,
    bitmap: u32,
    seq: i8,
) -> Result<Option<(u32, [f32; 4])>, MemFault> {
    const WHOLE: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    let sequences = mem.u32(bitmap + 0x3c)?;
    let (image, bounds) = if seq >= 0 && (seq as u32) < sequences {
        let seq = mem.u32(bitmap + 0x40)? + 0x3c * seq as u32;
        if mem.u32(seq + 0x34)? > 0 {
            let sprite = mem.u32(seq + 0x38)?;
            let mut b = [0f32; 4];
            for (i, v) in b.iter_mut().enumerate() {
                *v = mem.f32(sprite + 8 + 4 * i as u32)?;
            }
            (u32::from(mem.u16(sprite)?), b)
        } else {
            // The sequence's `first_image` (bitm sequences +0x20).
            let first = mem.u16(seq + 0x20)? as i16;
            (u32::try_from(first).unwrap_or(0), WHOLE)
        }
    } else {
        (0, WHOLE)
    };
    Ok((image < mem.u32(bitmap + 0x44)?).then_some((image, bounds)))
}

/// A cropped kit widget's sprite: the image, bounds and size the game's
/// own pick gives ([`sprite_pick`]), the bounds narrowed to the crop. A
/// sequence with no sprites draws its first image whole, so the crop
/// narrows the whole image (the charge meter's strip and the pill's panel
/// are such images: drawn uncropped they are 92 and 103 pixels wide, not
/// 27). An image the bitmap does not have is left to the game.
pub fn crop_sprite(
    mem: &dyn GuestMemory,
    widget: u32,
    table: &Table,
) -> Result<Option<Sprite>, MemFault> {
    let Some(crop) = table.at(widget).and_then(|w| w.widget.crop) else {
        return Ok(None);
    };
    let datum = mem.u32(widget + records::bitmap::BITMAP as u32 + 4)?;
    let Some(bitmap) = tag_get_checked(mem, DatumHandle(datum), GroupTag(BITM))? else {
        return Ok(None);
    };
    let at = mode(mem)?.index() as u32;
    let seq = mem.u8(widget + records::bitmap::SEQUENCES as u32 + at)? as i8;
    let Some((image, b)) = sprite_pick(mem, bitmap, seq)? else {
        return Ok(None);
    };
    let data = mem.u32(bitmap + 0x48)? + 0x74 * image;
    Ok(Some(Sprite {
        image: image as i32,
        width: i32::from(mem.u16(data + 4)?),
        height: i32::from(mem.u16(data + 6)?),
        bounds: narrow(b, crop),
    }))
}

// ---- the channels ----

/// Writes `text` as user `user`'s prompt, as `0x0024ca1d` and
/// `0x0024c98c` leave the fields (`h1_hud_text::prompts::show`).
pub fn show_prompt(mem: &mut dyn GuestMemory, user: u32, text: &str) -> Result<(), MemFault> {
    let hud = mem.u32(USER_HUDS)? + USER_HUD_LEN * user;
    mem.write(hud + PROMPT_TEXT, &utf16(text, PROMPT_UNITS))?;
    mem.set_u32(hud + PROMPT_ID, 0)?;
    if mem.u8(hud + PROMPT_SHOWN)? == 0 {
        mem.set_u8(hud + 0x44e, mem.u8(hud + 0x44e)? | 1)?;
    }
    mem.set_u8(hud + 0x44f, 1)?;
    mem.set_u8(hud + PROMPT_SHOWN, 1)
}

/// Writes `text` as user `user`'s timed centred line for `ticks` game
/// ticks from now, as `0x0024cc73` leaves the fields ([`BANNER_TEXT`]).
/// With no game time or no HUD state yet, nothing is written (false).
pub fn write_banner(
    mem: &mut dyn GuestMemory,
    user: u32,
    text: &str,
    ticks: u32,
) -> Result<bool, MemFault> {
    let (time, base) = (mem.u32(GAME_TIME)?, mem.u32(USER_HUDS)?);
    if time == 0 || base == 0 || user >= USERS {
        return Ok(false);
    }
    let now = mem.u32(time + GAME_TIME_TICK)?;
    let hud = base + USER_HUD_LEN * user;
    mem.write(hud + BANNER_TEXT, &utf16(text, BANNER_UNITS + 1))?;
    mem.set_u32(hud + BANNER_STAMP, now)?;
    mem.set_u8(hud + BANNER_SHOWN, 1)?;
    let order = mem.u8(base + MESSAGE_ORDER)?;
    mem.set_u8(hud + BANNER_ORDER, order)?;
    mem.set_u8(base + MESSAGE_ORDER, order.wrapping_add(1))?;
    mem.set_u32(hud + BANNER_START, now)?;
    mem.set_u32(hud + BANNER_DURATION, ticks)?;
    Ok(true)
}

/// The local users the HUD draws for: the local views
/// (`player_globals +8`, as [`mode`] reads it), at least one and at most
/// [`USERS`].
pub fn local_users(mem: &dyn GuestMemory) -> Result<u32, MemFault> {
    let globals = mem.u32(points::PLAYER_GLOBALS.base)?;
    if globals == 0 {
        return Ok(1);
    }
    Ok(u32::from(mem.u16(globals + 8)?).clamp(1, USERS))
}

/// Shows the kit's training text while `show`, unless the game shows a
/// scripted text of its own; hides it when `show` is off and it is ours.
/// Returns whether ours is showing.
pub fn write_training(mem: &mut dyn GuestMemory, show: bool) -> Result<bool, MemFault> {
    let state = mem.u32(TRAINING)?;
    if state == 0 {
        return Ok(false);
    }
    let id = mem.u32(state + TRAINING_ID)?;
    let on = mem.u8(state + TRAINING_SHOWN)? != 0;
    let theirs = on && id != TRAINING_TEXT_ID && id != 0 && id != u32::MAX;
    if show && !theirs {
        mem.set_u32(state + TRAINING_ID, TRAINING_TEXT_ID)?;
        mem.set_u8(state + TRAINING_SHOWN, 1)?;
        return Ok(true);
    }
    if !show && id == TRAINING_TEXT_ID {
        mem.set_u8(state + TRAINING_SHOWN, 0)?;
        mem.set_u32(state + TRAINING_ID, 0)?;
    }
    Ok(false)
}

/// Where our navpoints' scenario points are: the scenario's point block
/// as it was, and the index of the first of our [`KIT_POINTS`] points
/// (one past its last). Kept at the head of the points memory, as
/// [`Header`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointHeader {
    pub scenario: u32,
    pub retail: [u32; 2],
    pub index: u32,
}

pub const POINT_HEADER_LEN: u32 = 0x20;

fn read_point_header(mem: &dyn GuestMemory, region: u32) -> Result<Option<PointHeader>, MemFault> {
    if mem.u32(region)? != HEADER_MAGIC {
        return Ok(None);
    }
    Ok(Some(PointHeader {
        scenario: mem.u32(region + 4)?,
        retail: [mem.u32(region + 8)?, mem.u32(region + 12)?],
        index: mem.u32(region + 16)?,
    }))
}

/// Puts the scenario's point block back where the loaded scenario still
/// points at our copy, then copies it there with [`KIT_POINTS`] points
/// after it and points the scenario at the copy. `None` without a scenario
/// or room.
pub fn install_point(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
) -> Result<Option<PointHeader>, MemFault> {
    let (start, len) = region;
    if let Some(h) = read_point_header(&*mem, start)? {
        // Only the scenario still loaded: a new map's tags may lie where
        // the old scenario was.
        if point_installed(&*mem, region, &h)? {
            let field = h.scenario + SCENARIO_POINTS;
            mem.set_u32(field, h.retail[0])?;
            mem.set_u32(field + 4, h.retail[1])?;
        }
        mem.set_u32(start, 0)?;
    }
    let scenario = mem.u32(SCENARIO)?;
    if scenario == 0 {
        return Ok(None);
    }
    let field = scenario + SCENARIO_POINTS;
    let (count, from) = (mem.u32(field)?, mem.u32(field + 4)?);
    let bytes = (count + KIT_POINTS) * POINT_LEN;
    if POINT_HEADER_LEN + bytes > len {
        return Ok(None);
    }
    let block = start + POINT_HEADER_LEN;
    if count > 0 {
        let mut old = vec![0; (count * POINT_LEN) as usize];
        mem.read(from, &mut old)?;
        mem.write(block, &old)?;
    }
    mem.fill(block + count * POINT_LEN, KIT_POINTS * POINT_LEN, 0)?;
    mem.set_u32(field, count + KIT_POINTS)?;
    mem.set_u32(field + 4, block)?;
    let h = PointHeader {
        scenario,
        retail: [count, from],
        index: count,
    };
    mem.set_u32(start, HEADER_MAGIC)?;
    mem.set_u32(start + 4, h.scenario)?;
    mem.set_u32(start + 8, h.retail[0])?;
    mem.set_u32(start + 12, h.retail[1])?;
    mem.set_u32(start + 16, h.index)?;
    Ok(Some(h))
}

/// Whether the scenario loaded now still points at the copy
/// [`install_point`] made, our points its last: the only state in which a
/// kind-0 slot with one of our indices reads our point (the draw does not
/// check the index against the block's count).
fn point_installed(
    mem: &dyn GuestMemory,
    region: (u32, u32),
    h: &PointHeader,
) -> Result<bool, MemFault> {
    if mem.u32(SCENARIO)? != h.scenario || h.scenario == 0 {
        return Ok(false);
    }
    let field = h.scenario + SCENARIO_POINTS;
    Ok(mem.u32(field)? == h.index + KIT_POINTS
        && mem.u32(field + 4)? == region.0 + POINT_HEADER_LEN)
}

/// [`write_navpoint_n`] for the first of our points.
pub fn write_navpoint(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
    user: u32,
    at: Option<[f32; 3]>,
) -> Result<Option<u32>, MemFault> {
    write_navpoint_n(mem, region, user, 0, at)
}

/// Puts our point `k` (under [`KIT_POINTS`]) at `at` and keeps one slot
/// of user `user`'s navpoint list on it (the slot already that point's,
/// else the first one the game's draw passes: `+0` or `+8` -1, or kind
/// 15), or skips its slot when `at` is `None` or our points are not
/// installed in the loaded scenario. The game's own slots, and our other
/// points' slots, are never touched. Returns the slot used.
pub fn write_navpoint_n(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
    user: u32,
    k: u32,
    at: Option<[f32; 3]>,
) -> Result<Option<u32>, MemFault> {
    if k >= KIT_POINTS {
        return Ok(None);
    }
    let Some(h) = read_point_header(&*mem, region.0)? else {
        return Ok(None);
    };
    let index = h.index + k;
    let list = mem.u32(NAVPOINTS)?;
    if list == 0 {
        return Ok(None);
    }
    let slot = |i: u32| list + user * NAV_USER_LEN + i * NAV_SLOT_LEN;
    let unused = |mem: &dyn GuestMemory, i: u32| -> Result<bool, MemFault> {
        Ok(mem.u16(slot(i))? as i16 == -1
            || mem.u32(slot(i) + 8)? == u32::MAX
            || mem.u16(slot(i) + 2)? & 0xf == NAV_SKIP)
    };
    let ours = |mem: &dyn GuestMemory, i: u32| -> Result<bool, MemFault> {
        Ok(!unused(mem, i)?
            && mem.u16(slot(i) + 2)? & 0xf == NAV_SCENARIO_POINT
            && mem.u32(slot(i) + 8)? == index)
    };
    let mut mine = None;
    let mut free = None;
    for i in 0..NAV_SLOTS {
        if ours(&*mem, i)? {
            mine = Some(i);
            break;
        }
        if free.is_none() && unused(&*mem, i)? {
            free = Some(i);
        }
    }
    let at = match at {
        Some(at) if point_installed(&*mem, region, &h)? => at,
        _ => {
            if let Some(i) = mine {
                let kind = mem.u16(slot(i) + 2)?;
                mem.set_u16(slot(i) + 2, (kind & !0xf) | NAV_SKIP)?;
            }
            return Ok(None);
        }
    };
    let p = region.0 + POINT_HEADER_LEN + index * POINT_LEN + POINT_AT;
    for (k, v) in at.iter().enumerate() {
        mem.set_f32(p + 4 * k as u32, *v)?;
    }
    let Some(i) = mine.or(free) else {
        return Ok(None);
    };
    mem.set_u16(slot(i), 0)?;
    mem.set_u16(slot(i) + 2, NAV_SCENARIO_POINT)?;
    mem.set_f32(slot(i) + 4, 0.0)?;
    mem.set_u32(slot(i) + 8, index)?;
    Ok(Some(i))
}

// ---- the feature ----

/// One player `nhdt` of the map: its datum, the indices of its kept
/// bitmap widgets (`templates::monitor_keeps`) and the HUDs compiled for
/// it.
type MapHud = (u32, Vec<u32>, Vec<(usize, Hud, Compiled)>);

/// Compiles every registered HUD for every player `nhdt` of the loaded
/// map and installs them (the map file through `native_map`); a monitor's
/// HUD with the row's first place (`Templates::for_monitor`).
#[cfg(feature = "gpu")]
fn compile_for_map(name: &str) -> Result<Vec<MapHud>, String> {
    use hud_kit_core::map_file::{self, MapFile};
    let (mut map, mut files) = crate::native_map::open(name).map_err(|e| e.to_string())?;
    let multiplayer = map.header().cache_type == map::CacheType::Multiplayer;
    let registered: Vec<(usize, Arc<dyn Describe + Send + Sync>, Hud)> = read(&REGISTRY)
        .iter()
        .enumerate()
        .map(|(i, r)| (i, Arc::clone(&r.describe), r.hud.clone()))
        .collect();
    let mut out = Vec::new();
    for nhdt in map_file::player_huds(&mut map) {
        let keeps = {
            use hud_kit_core::templates::MapView;
            let mut view = MapFile::new(&mut map, &mut files);
            view.nhdt(nhdt)
                .map(|h| hud_kit_core::templates::monitor_keeps(&h))
                .unwrap_or_default()
        };
        let mut huds = Vec::new();
        for (index, describe, hud) in &registered {
            if let (false, Some(why)) = (multiplayer, describe.multiplayer_only()) {
                eprintln!("hud kit: {name}: {} not joined: {why}", hud.name);
                continue;
            }
            let mut view = MapFile::new(&mut map, &mut files);
            let mut t = hud_kit_core::templates::resolve(&mut view, nhdt, hud);
            if describe.monitor() {
                t = t.for_monitor();
            }
            let missing = t.missing_widgets(hud);
            if !missing.is_empty() {
                eprintln!(
                    "hud kit: {name}: {} not joined: {}",
                    hud.name,
                    missing.join("; ")
                );
                continue;
            }
            match records::compile(hud, &t) {
                Ok(c) => huds.push((*index, hud.clone(), c)),
                Err(e) => eprintln!("hud kit: {name}: {}: {e}", hud.name),
            }
        }
        out.push((nhdt.0, keeps, huds));
    }
    Ok(out)
}

#[cfg(not(feature = "gpu"))]
fn compile_for_map(_name: &str) -> Result<Vec<MapHud>, String> {
    Err("this build has no map reader for the HUD kit".into())
}

fn activate(mem: &mut dyn GuestMemory, region: (u32, u32), points_region: (u32, u32)) {
    *TABLE.write().unwrap_or_else(|e| e.into_inner()) = Table::default();
    RETAIL_OFF.store(0, Ordering::Relaxed);
    if let Err(e) = restore(mem, region) {
        eprintln!("hud kit: the last install could not be put back ({e:?})");
        return;
    }
    if read(&REGISTRY).is_empty() {
        return;
    }
    let Ok(name) = crate::health::loaded_map_name(&*mem) else {
        return;
    };
    let compiled = match compile_for_map(name.trim()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hud kit: {name}: {e}");
            return;
        }
    };
    let mut joins = Vec::new();
    let mut keeps = Vec::new();
    for (datum, kept, huds) in &compiled {
        match tag_get_checked(&*mem, DatumHandle(*datum), GroupTag(NHDT)) {
            Ok(Some(root)) => {
                keeps.push((root, kept.clone()));
                if !huds.is_empty() {
                    joins.push(Join {
                        root,
                        huds: huds.iter().map(|(i, h, c)| (*i, h, c)).collect(),
                    });
                }
            }
            _ => eprintln!("hud kit: {name}: nhdt {datum:#x} is not loaded"),
        }
    }
    if joins.is_empty() {
        // No HUD joined: the tag's own records, the motion tracker's kept.
        if let Ok(k) = kept_records(&*mem, &keeps) {
            TABLE.write().unwrap_or_else(|e| e.into_inner()).kept = k;
        }
        return;
    }
    match install_records(mem, region, &joins) {
        Ok(mut t) => {
            match kept_records(&*mem, &keeps) {
                Ok(k) => t.kept = k,
                Err(e) => eprintln!("hud kit: {name}: no motion tracker for a monitor ({e:?})"),
            }
            eprintln!(
                "hud kit: {name}: {} widgets joined {} HUDs",
                t.widgets.len(),
                joins.len()
            );
            *TABLE.write().unwrap_or_else(|e| e.into_inner()) = t;
        }
        Err(e) => eprintln!("hud kit: {name}: not installed ({e:?})"),
    }
    if let Err(e) = install_point(mem, points_region) {
        eprintln!("hud kit: {name}: no navpoint point ({e:?})");
    }
}

fn tick(mem: &mut dyn GuestMemory, points_region: (u32, u32)) -> Result<(), MemFault> {
    let training = channel(|k| matches!(k, Kind::Training { .. }));
    if let Some(t) = &training {
        *lock(&TRAINING_TEXT) = t.clone();
    }
    write_training(mem, training.is_some())?;
    // The banner, to each local user, when the text that user's values
    // show changes.
    let screen = mode(&*mem)?;
    for user in 0..local_users(&*mem)? {
        let now = banner_now(screen, user);
        let due = banner_due(&mut lock(&BANNER_LAST)[user as usize], now);
        if let Some(text) = due {
            write_banner(mem, user, &text, BANNER_TICKS)?;
        }
    }
    // The superbounce target arrows clear the tracker and the readout.
    let on = crate::superbounce_mode::hud_state().is_some();
    crate::superbounce_readout::keep_waypoint_inset(mem, on);
    for (k, at) in navpoints_now().into_iter().enumerate() {
        write_navpoint_n(mem, points_region, 0, k as u32, at)?;
    }
    Ok(())
}

struct HudKit;

impl hooks::Feature for HudKit {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "HUD elements of ours drawn by the game's HUD")
            .doc("docs/game/hud-kit.md")
            // Draw hooks and HUD state only: nothing the simulation reads.
            .presentation_only()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let start = b.guest_alloc(REGION_LEN, runtime::guest::Permissions::RW)?;
        let points = b.guest_alloc(POINTS_LEN, runtime::guest::Permissions::RW)?;
        let notes = b.guest_alloc(NOTES_LEN, runtime::guest::Permissions::RW)?;
        let (region, points_region) = ((start, REGION_LEN), (points, POINTS_LEN));
        let notes_region = (notes, NOTES_LEN);
        REGION_START.store(start, Ordering::Relaxed);
        REGION_END.store(start + REGION_LEN, Ordering::Relaxed);
        // What the roots held lives in guest memory; the draw table is
        // rebuilt at each install.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw()
            .before(&points::GAME_ACTIVATE, "hud kit: join the HUDs", move |c| {
                activate(&mut *c.ctx.memory(), region, points_region);
                Ok(Flow::Continue)
            })
            // The other match-start befores (the HUD swap first: the kit
            // reads the nhdt datums it points the owners at) read nothing
            // the kit writes; the plan wants an order.
            .after_feature(crate::hud_swap::FEATURE)
            // The map's file, which the kit's read of the map opens.
            .after_feature(crate::loaded_map::FEATURE)
            .after_feature(crate::variant_ext::FEATURE)
            .after_feature(crate::aim_assist::FEATURE)
            .after_feature(crate::precision::FEATURE)
            .after_feature(crate::h1_hud_text::FEATURE);
        // At the tick's end, after the features that change the tick's
        // state there; the kit writes only HUD state (the training slot,
        // the navpoint list), which none of them reads.
        b.raw()
            .after(&points::GAME_TICK, "hud kit: channels", move |c| {
                tick(&mut *c.ctx.memory(), points_region).map_err(fault)
            })
            .after_feature(crate::cheats::FEATURE)
            .after_feature(crate::h1_hud_text::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE)
            .after_feature(crate::touch_glue::FEATURE);
        // Notifications at the top of the tick, as our skulls' messages
        // are sent (`skulls_custom`): the insert, then the tick itself.
        // Inside the agent's calls and our skulls' (each runs the tick
        // again, and this hook passes on its own re-entry), and the map
        // select's and the dedicated lobby's, which share the first
        // group.
        b.raw()
            .before(&points::GAME_TICK, "hud kit: notifications", move |c| {
                if NOTES_REENTRY.load(Ordering::SeqCst) {
                    return Ok(Flow::Continue);
                }
                let notes = take_notes();
                if notes.is_empty() {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(send_notes(cx, notes_region, notes))
                })
            })
            .first()
            .after_feature(agent::sim::calls::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE)
            .after_feature(crate::map_select::FEATURE)
            .after_feature(HOST_LOBBY);
        // The walk's seam is shared with `h1_hud_text`'s, and the two never
        // answer the same widget: the kit answers only a widget inside its
        // own memory ([`in_region`], then its draw table), and `h1_hud_text`
        // only a converted Halo 1 widget, marked by its byte `+0x1b` (0xe0
        // plus a screen index), which the kit's compiler leaves 0 (the
        // state's pad byte) and which lives in the converted nhdt's own
        // blocks, not ours. Rank 0 is `h1_hud_text`'s, so its answers come
        // first (docs/graphics/native-hud-overlay.md, "The override seam").
        #[cfg(feature = "desktop-runtime")]
        render_bridge::native_hud::seam::register(1, "hud-kit", Arc::new(KitSeam));
        b.raw()
            .before(
                &points::HUD_INTERACTION_VISIBLE,
                "hud kit: prompt",
                move |c| {
                    let (user, sp) = (c.cpu.eax, c.cpu.esp);
                    let mut mem = c.ctx.memory();
                    // Only for the first local player, at the interaction
                    // code's call, while the game shows no prompt of its own.
                    // `h1_hud_text`'s Halo 1 prompt runs first (it is
                    // ordered outside this hook) and, when it shows one,
                    // returns before the kit's is written.
                    if user != 0 || mem.u32(sp)? != FROM_INTERACTION || mem.u32(sp + 4)? != 0 {
                        return Ok(Flow::Continue);
                    }
                    let Some(text) = channel(|k| matches!(k, Kind::Prompt { .. })) else {
                        return Ok(Flow::Continue);
                    };
                    show_prompt(&mut *mem, user, &text).map_err(fault)?;
                    drop(mem);
                    c.ret(4).map(Flow::Return)
                },
            )
            .context_only()
            .after_feature(crate::h1_hud_text::FEATURE);
        Ok(())
    }
}

/// The kit's answers at the Rust walk's override seam.
#[cfg(feature = "desktop-runtime")]
struct KitSeam;

#[cfg(feature = "desktop-runtime")]
impl render_bridge::native_hud::seam::Seam for KitSeam {
    fn gate(
        &self,
        mem: &dyn GuestMemory,
        at: &render_bridge::native_hud::seam::At,
    ) -> Result<Option<bool>, MemFault> {
        // A retail widget costs two loads and this one while no user is in
        // monitor mode.
        let off = RETAIL_OFF.load(Ordering::Relaxed);
        if off == 0 && !in_region(at.widget) {
            return Ok(None);
        }
        let table = read(&TABLE);
        let user = draw_user(mem);
        let values = |h: usize, id: &str| value(h, user, id);
        match gate(mem, at.widget, at.word_d, &table, &values)? {
            Some(shown) => Ok(Some(shown)),
            None => Ok(retail_gate(at.widget, &table, off, user)),
        }
    }

    fn inputs(
        &self,
        mem: &dyn GuestMemory,
        at: &render_bridge::native_hud::seam::At,
    ) -> Result<Option<[f32; 4]>, MemFault> {
        if !in_region(at.widget) {
            return Ok(None);
        }
        let table = read(&TABLE);
        let user = draw_user(mem);
        Ok(inputs(at.widget, &table, &|h, id| value(h, user, id)))
    }

    fn string(
        &self,
        mem: &dyn GuestMemory,
        at: &render_bridge::native_hud::seam::At,
        _: &[f32; 4],
    ) -> Result<Option<render_bridge::native_hud::seam::Text>, MemFault> {
        if !in_region(at.widget) {
            return Ok(None);
        }
        let table = read(&TABLE);
        let user = draw_user(mem);
        let answer = string(mem, at.widget, &table, &|h, id| value(h, user, id))?;
        Ok(answer.map(|(units, font)| render_bridge::native_hud::seam::Text { units, font }))
    }

    fn sprite(
        &self,
        mem: &dyn GuestMemory,
        at: &render_bridge::native_hud::seam::At,
    ) -> Result<Option<Sprite>, MemFault> {
        if !in_region(at.widget) {
            return Ok(None);
        }
        crop_sprite(mem, at.widget, &read(&TABLE))
    }

    fn scale(
        &self,
        mem: &dyn GuestMemory,
        at: &render_bridge::native_hud::seam::At,
    ) -> Result<Option<[f32; 2]>, MemFault> {
        // Only a widget in the kit's memory: two loads for a retail one.
        if !in_region(at.widget) {
            return Ok(None);
        }
        let user = draw_user(mem);
        let table = read(&TABLE);
        let seen = scale_factors(at.widget, &table, &|h, id| scale(h, user, id));
        if seen.is_some() {
            EFFECTS_SEEN.store(true, Ordering::Relaxed);
        }
        Ok(seen)
    }
}

/// Adds the feature to `registry`.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&HudKit)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("hud kit: {e}"))
}

#[cfg(test)]
#[path = "hud_kit_tests.rs"]
mod tests;
