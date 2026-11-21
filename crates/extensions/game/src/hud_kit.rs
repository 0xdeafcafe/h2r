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
//! 2. **Gate** (`HUD_WIDGET_GATE`): a kit widget is drawn while its value
//!    is shown, in a style the runtime draws ([`draws`]), in a screen mode
//!    its place has.
//! 3. **Inputs** (`HUD_WIDGET_INPUTS`): `hud_input1` is
//!    `records::Widget::input` of the value's fill.
//! 4. **String** (`HUD_WIDGET_STRING`): the value's text, in the
//!    element's font.
//! 5. **Crop** (`HUD_WIDGET_SPRITE`): a cropped widget's sprite bounds are
//!    narrowed to its crop ([`crop_sprite`]).
//! 6. **Channels**: the prompt through the user's prompt fields while the
//!    game shows no prompt of its own ([`show_prompt`]); the training text
//!    through the scripted training slot and the string lookup
//!    ([`write_training`], [`text`]); the navpoint through the game's
//!    navpoint list at a scenario point of ours ([`install_point`],
//!    [`write_navpoint`]). Notifications are not sent (decoding 6).
//!
//! The draw hooks are `context_only` and read one shared table, so they
//! run on the render thread.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::tags::{GroupTag, tag_get_checked};
use hooks::{BuildError, Builder, Flow};
use hud_kit_core::Mode;
use hud_kit_core::model::{Describe, Hud, Kind, Style, Value, Values};
use hud_kit_core::records::{self, Compiled, WidgetKind};

use crate::h1_particle_physics::fault;
use crate::ui_kit::ids::{Family, family};

/// The feature's id.
pub const FEATURE: &str = "hud-kit";

/// Our guest memory for the copied widget blocks: a [`Header`], then the
/// blocks.
pub const REGION_LEN: u32 = 0x1_0000;
/// Our guest memory for the scenario's point block with our point after
/// it: a [`PointHeader`], then the block.
pub const POINTS_LEN: u32 = 0x8000;

const NHDT: u32 = u32::from_be_bytes(*b"nhdt");
const BITM: u32 = u32::from_be_bytes(*b"bitm");

/// The return addresses of the widget loop's calls (E `0x0013a532`): the
/// gate's, the inputs' (bitmap, text), the string's, the sprite's.
pub const GATE_RETURNS: [u32; 2] = [0x0013_a602, 0x0013_a659];
pub const INPUT_RETURNS: [u32; 2] = [0x0013_a61c, 0x0013_a673];
pub const STRING_RETURN: u32 = 0x0022_aa5b;
pub const SPRITE_RETURN: u32 = 0x0022_b03a;
/// Where the text draw publishes a widget's four inputs to the HUD
/// material (E `0x0022a895`..`0x0022a8a3`).
pub const MATERIAL_INPUTS: u32 = 0x004e_6980;
/// The UTF-16 units the string hook writes at most, its zero included
/// (S: the buffer's size is not read; the retail strings are short).
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
/// `HUD_INTERACTION_VISIBLE`'s caller, the interaction code (E
/// `0x00191976`).
pub const FROM_INTERACTION: u32 = 0x0019_197b;

/// The training state (`[0x0051e9c0]`): four users of `0x1b0`, then the
/// scripted text's string id and its show byte (E `0x001e70c3`).
pub const TRAINING: u32 = 0x0051_e9c0;
pub const TRAINING_ID: u32 = 0x6c0;
pub const TRAINING_SHOWN: u32 = 0x6c4;

/// The navpoint lists (`[0x005023f8] + user * 0x30`): four slots of 12
/// bytes, `+0` an `i16` (-1 ends the list), `+2` the kind in its low four
/// bits (15 skips the slot, 0 a scenario point), `+4` a height added to
/// the point, `+8` the point's index (E `0x0024da7d`..`0x0024db6e`).
pub const NAVPOINTS: u32 = 0x0050_23f8;
pub const NAV_USER_LEN: u32 = 0x30;
pub const NAV_SLOTS: u32 = 4;
pub const NAV_SLOT_LEN: u32 = 12;
pub const NAV_SCENARIO_POINT: u16 = 0;
pub const NAV_SKIP: u16 = 15;
/// The scenario (`[0x004e0350]`) and its point block at `+0x1e4` (count,
/// address), `0x38` bytes a point, the point at `+0x24` (E `0x0024da7d`;
/// S: which scenario block this is).
pub const SCENARIO: u32 = 0x004e_0350;
pub const SCENARIO_POINTS: u32 = 0x1e4;
pub const POINT_LEN: u32 = 0x38;
pub const POINT_AT: u32 = 0x24;

/// The kit's string ids (`ui_kit::ids`): the training text's.
pub const IDS: Family = family("hud kit");
pub const TRAINING_TEXT_ID: u32 = IDS.id(0);

// ---- the registry ----

struct Registered {
    describe: Arc<dyn Describe + Send + Sync>,
    hud: Hud,
    values: Values,
    nav: Option<[f32; 3]>,
}

static REGISTRY: RwLock<Vec<Registered>> = RwLock::new(Vec::new());
static TRAINING_TEXT: Mutex<String> = Mutex::new(String::new());

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
        nav: None,
    });
    Handle(r.len() - 1)
}

impl Handle {
    /// The values the hooks give from now on.
    pub fn publish(&self, values: Values) {
        let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = r.get_mut(self.0) {
            x.values = values;
        }
    }
    /// The world point the HUD's navpoint is over, if it shows.
    pub fn navpoint(&self, at: Option<[f32; 3]>) {
        let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = r.get_mut(self.0) {
            x.nav = at;
        }
    }
}

/// HUD `hud`'s value for element `id`.
fn value(hud: usize, id: &str) -> Option<Value> {
    read(&REGISTRY).get(hud)?.values.get(id).cloned()
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

fn navpoint_at() -> Option<[f32; 3]> {
    let r = read(&REGISTRY);
    r.iter().find_map(|x| {
        let shown = x.hud.elements.iter().any(|e| {
            matches!(e.kind, Kind::Navpoint { .. }) && x.values.get(e.id).is_some_and(|v| v.shown)
        });
        x.nav.filter(|_| shown)
    })
}

/// The string lookup's answer for the kit's ids (`0x001a0180`, through
/// `community_variants`' override).
pub fn text(id: u32) -> Option<String> {
    (id == TRAINING_TEXT_ID).then(|| lock(&TRAINING_TEXT).clone())
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
}

/// The kit widgets installed, by address.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub widgets: Vec<KitWidget>,
}

impl Table {
    pub fn at(&self, address: u32) -> Option<&KitWidget> {
        self.widgets.iter().find(|w| w.address == address)
    }
}

static TABLE: RwLock<Table> = RwLock::new(Table {
    widgets: Vec::new(),
});
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
                    let modes = hud
                        .elements
                        .iter()
                        .find(|e| e.id == w.id)
                        .map_or([false; 3], |e| Mode::ALL.map(|m| e.at.offset(m).is_some()));
                    table.widgets.push(KitWidget {
                        address: cursor,
                        hud: *index,
                        id: w.id,
                        kind,
                        widget: w.clone(),
                        modes,
                        font: font_of(w),
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

// ---- the draw hooks ----

/// The gate's answer for the widget whose state is at `state` (EAX), when
/// it is a kit widget.
pub fn gate(
    mem: &dyn GuestMemory,
    sp: u32,
    state: u32,
    table: &Table,
    values: &dyn Fn(usize, &str) -> Option<Value>,
) -> Result<Option<bool>, MemFault> {
    let widget = state.wrapping_sub(8);
    if !GATE_RETURNS.contains(&mem.u32(sp)?) {
        return Ok(None);
    }
    let Some(w) = table.at(widget) else {
        return Ok(None);
    };
    Ok(Some(shown(w, values(w.hud, w.id).as_ref(), mode(mem)?)))
}

/// `hud_input1` for a kit widget: its value's fill through
/// `records::Widget::input`, the other three 0.
pub fn inputs(
    mem: &mut dyn GuestMemory,
    sp: u32,
    table: &Table,
    values: &dyn Fn(usize, &str) -> Option<Value>,
) -> Result<bool, MemFault> {
    if !INPUT_RETURNS.contains(&mem.u32(sp)?) {
        return Ok(false);
    }
    let widget = mem.u32(sp + 8)?.wrapping_sub(4);
    let Some(w) = table.at(widget) else {
        return Ok(false);
    };
    let fill = values(w.hud, w.id).map_or(0.0, |v| v.fill);
    let out = mem.u32(sp + 12)?;
    mem.set_f32(out, w.widget.input(fill))?;
    mem.fill(out + 4, 12, 0)?;
    Ok(true)
}

/// UTF-16 of `text`, cut to `units` with its zero.
pub fn utf16(text: &str, units: usize) -> Vec<u8> {
    text.encode_utf16()
        .take(units.saturating_sub(1))
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A kit text widget's string and font.
pub fn string(
    mem: &mut dyn GuestMemory,
    sp: u32,
    widget: u32,
    table: &Table,
    values: &dyn Fn(usize, &str) -> Option<Value>,
) -> Result<bool, MemFault> {
    if mem.u32(sp)? != STRING_RETURN {
        return Ok(false);
    }
    let Some(w) = table.at(widget).filter(|w| w.kind == WidgetKind::Text) else {
        return Ok(false);
    };
    let text = values(w.hud, w.id).map(|v| v.text).unwrap_or_default();
    let font = w.font[mode(&*mem)?.index()];
    let font = if font < 0 {
        hud_kit_core::model::Font::Hud.engine()
    } else {
        font
    };
    let out = mem.u32(sp + 12)?;
    mem.write(out, &utf16(&text, STRING_UNITS))?;
    mem.set_u32(mem.u32(sp + 16)?, font as u32)?;
    let mut four = [0; 16];
    mem.read(mem.u32(sp + 8)?, &mut four)?;
    mem.write(MATERIAL_INPUTS, &four)?;
    Ok(true)
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

/// A cropped kit widget's sprite: the image, bounds and size the game's
/// own pick gives (sequence of the mode, its first sprite: E `0x0022ae21`,
/// as `h1_hud_text::weapon`'s pick reads them), the bounds narrowed to the
/// crop. A widget whose sequence has no sprites is left to the game.
pub fn crop_sprite(
    mem: &mut dyn GuestMemory,
    sp: u32,
    widget: u32,
    bounds: u32,
    table: &Table,
) -> Result<bool, MemFault> {
    if mem.u32(sp)? != SPRITE_RETURN {
        return Ok(false);
    }
    let Some(crop) = table.at(widget).and_then(|w| w.widget.crop) else {
        return Ok(false);
    };
    let datum = mem.u32(widget + records::bitmap::BITMAP as u32 + 4)?;
    let Some(bitmap) = tag_get_checked(&*mem, DatumHandle(datum), GroupTag(BITM))? else {
        return Ok(false);
    };
    let at = mode(&*mem)?.index() as u32;
    let seq = mem.u8(widget + records::bitmap::SEQUENCES as u32 + at)? as i8;
    if seq < 0 || seq as u32 >= mem.u32(bitmap + 0x3c)? {
        return Ok(false);
    }
    let seq = mem.u32(bitmap + 0x40)? + 0x3c * seq as u32;
    if mem.u32(seq + 0x34)? == 0 {
        return Ok(false);
    }
    let sprite = mem.u32(seq + 0x38)?;
    let image = u32::from(mem.u16(sprite)?);
    let data = mem.u32(bitmap + 0x48)? + 0x74 * image;
    let mut b = [0f32; 4];
    for (i, v) in b.iter_mut().enumerate() {
        *v = mem.f32(sprite + 8 + 4 * i as u32)?;
    }
    for (i, v) in narrow(b, crop).iter().enumerate() {
        mem.set_f32(bounds + 4 * i as u32, *v)?;
    }
    mem.set_u32(mem.u32(sp + 8)?, image)?;
    mem.set_u32(mem.u32(sp + 12)?, u32::from(mem.u16(data + 4)?))?;
    mem.set_u32(mem.u32(sp + 16)?, u32::from(mem.u16(data + 6)?))?;
    Ok(true)
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

/// Where our navpoint's scenario point is: the scenario's point block as
/// it was, and our point's index (one past its last). Kept at the head of
/// the points memory, as [`Header`] is.
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

/// Puts the scenario's point block back where it still points into our
/// memory, then copies it there with one point after it and points the
/// scenario at the copy. `None` without a scenario or room.
pub fn install_point(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
) -> Result<Option<PointHeader>, MemFault> {
    let (start, len) = region;
    if let Some(h) = read_point_header(&*mem, start)? {
        let field = h.scenario + SCENARIO_POINTS;
        let address = mem.u32(field + 4)?;
        if address >= start && address < start + len {
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
    let bytes = (count + 1) * POINT_LEN;
    if POINT_HEADER_LEN + bytes > len {
        return Ok(None);
    }
    let block = start + POINT_HEADER_LEN;
    if count > 0 {
        let mut old = vec![0; (count * POINT_LEN) as usize];
        mem.read(from, &mut old)?;
        mem.write(block, &old)?;
    }
    mem.fill(block + count * POINT_LEN, POINT_LEN, 0)?;
    mem.set_u32(field, count + 1)?;
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

/// Puts our point at `at` and keeps one slot of user `user`'s navpoint
/// list on it (the slot already ours, else a skipped one, else the list's
/// end), or skips our slot when `at` is `None`. Returns the slot used.
pub fn write_navpoint(
    mem: &mut dyn GuestMemory,
    region: (u32, u32),
    user: u32,
    at: Option<[f32; 3]>,
) -> Result<Option<u32>, MemFault> {
    let Some(h) = read_point_header(&*mem, region.0)? else {
        return Ok(None);
    };
    let list = mem.u32(NAVPOINTS)?;
    if list == 0 {
        return Ok(None);
    }
    let slot = |i: u32| list + user * NAV_USER_LEN + i * NAV_SLOT_LEN;
    let ours = |mem: &dyn GuestMemory, i: u32| -> Result<bool, MemFault> {
        Ok(mem.u16(slot(i))? as i16 != -1
            && mem.u16(slot(i) + 2)? & 0xf == NAV_SCENARIO_POINT
            && mem.u32(slot(i) + 8)? == h.index)
    };
    let mut mine = None;
    let mut free = None;
    let mut end = None;
    for i in 0..NAV_SLOTS {
        if mem.u16(slot(i))? as i16 == -1 {
            end = Some(i);
            break;
        }
        if ours(&*mem, i)? {
            mine = Some(i);
            break;
        }
        if free.is_none() && mem.u16(slot(i) + 2)? & 0xf == NAV_SKIP {
            free = Some(i);
        }
    }
    let Some(at) = at else {
        if let Some(i) = mine {
            let kind = mem.u16(slot(i) + 2)?;
            mem.set_u16(slot(i) + 2, (kind & !0xf) | NAV_SKIP)?;
        }
        return Ok(None);
    };
    let point = h.scenario;
    let block = mem.u32(point + SCENARIO_POINTS + 4)?;
    let p = block + h.index * POINT_LEN + POINT_AT;
    for (k, v) in at.iter().enumerate() {
        mem.set_f32(p + 4 * k as u32, *v)?;
    }
    let Some(i) = mine.or(free).or(end) else {
        return Ok(None);
    };
    mem.set_u16(slot(i), 0)?;
    mem.set_u16(slot(i) + 2, NAV_SCENARIO_POINT)?;
    mem.set_f32(slot(i) + 4, 0.0)?;
    mem.set_u32(slot(i) + 8, h.index)?;
    if Some(i) == end && i + 1 < NAV_SLOTS {
        mem.set_u16(slot(i + 1), u16::MAX)?;
    }
    Ok(Some(i))
}

// ---- the feature ----

/// Compiles every registered HUD for every player `nhdt` of the loaded
/// map and installs them (the map file through `native_map`).
#[cfg(feature = "gpu")]
fn compile_for_map(name: &str) -> Result<Vec<(u32, Vec<(usize, Hud, Compiled)>)>, String> {
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
        let mut huds = Vec::new();
        for (index, describe, hud) in &registered {
            if let (false, Some(why)) = (multiplayer, describe.multiplayer_only()) {
                eprintln!("hud kit: {name}: {} not joined: {why}", hud.name);
                continue;
            }
            let mut view = MapFile::new(&mut map, &mut files);
            let t = hud_kit_core::templates::resolve(&mut view, nhdt, hud);
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
        if !huds.is_empty() {
            out.push((nhdt.0, huds));
        }
    }
    Ok(out)
}

#[cfg(not(feature = "gpu"))]
fn compile_for_map(_name: &str) -> Result<Vec<(u32, Vec<(usize, Hud, Compiled)>)>, String> {
    Err("this build has no map reader for the HUD kit".into())
}

fn activate(mem: &mut dyn GuestMemory, region: (u32, u32), points_region: (u32, u32)) {
    *TABLE.write().unwrap_or_else(|e| e.into_inner()) = Table::default();
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
    for (datum, huds) in &compiled {
        match tag_get_checked(&*mem, DatumHandle(*datum), GroupTag(NHDT)) {
            Ok(Some(root)) => joins.push(Join {
                root,
                huds: huds.iter().map(|(i, h, c)| (*i, h, c)).collect(),
            }),
            _ => eprintln!("hud kit: {name}: nhdt {datum:#x} is not loaded"),
        }
    }
    if joins.is_empty() {
        return;
    }
    match install_records(mem, region, &joins) {
        Ok(t) => {
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
    write_navpoint(mem, points_region, 0, navpoint_at())?;
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
        let (region, points_region) = ((start, REGION_LEN), (points, POINTS_LEN));
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
        // The widget hooks share their points with `h1_hud_text`'s, and the
        // two never answer the same widget: the kit answers only a widget
        // inside its own memory ([`in_region`], then its draw table), and
        // `h1_hud_text` only a converted Halo 1 widget, marked by its
        // byte `+0x1b` (0xe0 plus a screen index), which the kit's
        // compiler leaves 0 (the state's pad byte) and which lives in the
        // converted nhdt's own blocks, not ours. Each passes the other's
        // widgets through, so the order between them only has to exist.
        let values = |h: usize, id: &str| value(h, id);
        b.raw()
            .before(&points::HUD_WIDGET_GATE, "hud kit: gate", move |c| {
                if !in_region(c.cpu.eax.wrapping_sub(8)) {
                    return Ok(Flow::Continue);
                }
                let table = read(&TABLE);
                let answer =
                    gate(&*c.ctx.memory(), c.cpu.esp, c.cpu.eax, &table, &values).map_err(fault)?;
                match answer {
                    Some(show) => {
                        c.cpu.eax = u32::from(show);
                        c.ret(12).map(Flow::Return)
                    }
                    None => Ok(Flow::Continue),
                }
            })
            .context_only()
            .after_feature(crate::h1_hud_text::FEATURE);
        b.raw()
            .before(&points::HUD_WIDGET_INPUTS, "hud kit: inputs", move |c| {
                let table = read(&TABLE);
                if table.widgets.is_empty() {
                    return Ok(Flow::Continue);
                }
                if inputs(&mut *c.ctx.memory(), c.cpu.esp, &table, &values).map_err(fault)? {
                    return c.ret(12).map(Flow::Return);
                }
                Ok(Flow::Continue)
            })
            .context_only()
            .after_feature(crate::h1_hud_text::FEATURE);
        b.raw()
            .before(&points::HUD_WIDGET_STRING, "hud kit: string", move |c| {
                if !in_region(c.cpu.ebx) {
                    return Ok(Flow::Continue);
                }
                let table = read(&TABLE);
                if string(&mut *c.ctx.memory(), c.cpu.esp, c.cpu.ebx, &table, &values)
                    .map_err(fault)?
                {
                    return c.ret(16).map(Flow::Return);
                }
                Ok(Flow::Continue)
            })
            .context_only()
            .after_feature(crate::h1_hud_text::FEATURE);
        b.raw()
            .before(&points::HUD_WIDGET_SPRITE, "hud kit: crop", move |c| {
                if !in_region(c.cpu.ecx) {
                    return Ok(Flow::Continue);
                }
                let table = read(&TABLE);
                if crop_sprite(
                    &mut *c.ctx.memory(),
                    c.cpu.esp,
                    c.cpu.ecx,
                    c.cpu.eax,
                    &table,
                )
                .map_err(fault)?
                {
                    return c.ret(16).map(Flow::Return);
                }
                Ok(Flow::Continue)
            })
            .context_only()
            .after_feature(crate::h1_hud_text::FEATURE);
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
