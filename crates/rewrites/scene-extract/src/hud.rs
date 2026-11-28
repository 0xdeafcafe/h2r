//! What the HUD walk (`hud_kit_core::walk`) reads from the guest: the
//! definition's widget blocks, the player's HUD record, the view, the aim
//! point and the bitmap tag (docs/graphics/native-hud-overlay.md, "The
//! walk in Rust"). Reads only; the addresses are the ones the guest's
//! walk loads (Code, `h2 disasm`, cited at each).

use guest_layout::memory::GuestMemory;
pub use hud_kit_core::walk;
use hud_kit_core::walk::{Art, Bitmap, Definition, Image, Kind, Screen, Sequence, Session, SpriteRecord};

/// The tag instance table (`0x10` bytes each, the tag's data at `+8`).
pub const TAGS: u32 = 0x004e_3b44;
/// The render window and the HUD frame: top, left, bottom, right shorts.
pub const WINDOW: u32 = 0x004b_9dd0;
pub const FRAME: u32 = 0x004b_9dd8;
/// The game engine globals (`+1` a flag, `+2` a short, `+8` the kind).
const ENGINE: u32 = 0x004e_6948;
/// `0x00016a90`'s answer outside a game of kind 3.
const QUARTER: u32 = 0x0048_5ac2;
/// The UI globals (`0x6c` bytes a player from `+0`).
const UI: u32 = 0x0051_0c4c;
const UI_PLAYER: u32 = 0x6c;
/// The flash clock (`+4` a scale, `+8` a count).
const CLOCK: u32 = 0x0051_0c54;
/// `0x001392a9`'s per-player values.
const PLAYER_VALUES: u32 = 0x004e_69c0;
/// The local users (`+0xc` a player index each) and the players
/// (`[+0x44]`, `0x21c` bytes each).
const USERS: u32 = 0x004e_8c20;
const PLAYERS: u32 = 0x004e_8c24;
const PLAYER_SIZE: u32 = 0x21c;
/// Pointers to the shield meter's colour and the list type's.
const SHIELD_COLOUR: u32 = 0x0046_8710;
const LIST_COLOUR: u32 = 0x0046_8718;
/// The texture cache's frame stamp (`0x0003bcb0`).
const CACHE_STAMP: u32 = 0x004e_6488;
/// More widgets than any definition has: a count past it is not read.
const MAX_WIDGETS: u32 = 512;

/// The data of tag `datum`.
pub fn tag_data(mem: &dyn GuestMemory, datum: u32) -> Option<u32> {
    let tags = mem.u32(TAGS).ok()?;
    let at = tags.checked_add((datum & 0xffff).checked_mul(0x10)?)?.checked_add(8)?;
    mem.u32(at).ok().filter(|&data| data != 0)
}

/// `0x0014de70`: the player of local user `user`, or `u32::MAX`.
fn player_of(mem: &dyn GuestMemory, user: u32) -> Option<u32> {
    if user == u32::MAX {
        return Some(u32::MAX);
    }
    let users = mem.u32(USERS).ok()?;
    mem.u32(users.checked_add(user.checked_mul(4)?)?.checked_add(0xc)?).ok()
}

/// The player record of local user `user`; `Ok(None)` with no player.
pub fn player_record(mem: &dyn GuestMemory, user: u32) -> Option<Option<u32>> {
    let player = player_of(mem, user)?;
    if player == u32::MAX {
        return Some(None);
    }
    let table = mem.u32(mem.u32(PLAYERS).ok()?.checked_add(0x44)?).ok()?;
    Some(Some(table.checked_add((player & 0xffff) * PLAYER_SIZE)?))
}

/// `0x0022acb4`: whether the user's player record byte `+0x88` is 1 or 3.
pub fn user_state(mem: &dyn GuestMemory, user: u32) -> Option<bool> {
    let Some(record) = player_record(mem, user)? else { return Some(false) };
    Some(matches!(mem.u8(record.checked_add(0x88)?).ok()?, 1 | 3))
}

/// What the walk reads outside the record, for local user `user`
/// (`0x0013a050`, `0x0013ad48`).
pub fn session(mem: &dyn GuestMemory, user: u32) -> Option<Session> {
    let engine = mem.u32(ENGINE).ok().filter(|&e| e != 0)?;
    let clock = mem.u32(CLOCK).ok().filter(|&c| c != 0)?;
    let ui = mem.u32(UI).ok().filter(|&u| u != 0)?;
    Some(Session {
        user_state: user_state(mem, user)?,
        engine: mem.u32(engine.checked_add(8)?).ok()?,
        clock: (mem.i32(clock.checked_add(8)?).ok()?, mem.f32(clock.checked_add(4)?).ok()?),
        player_value: if user < 4 { mem.f32(PLAYER_VALUES + 4 * user).ok()? } else { 1.0 },
        user_value: mem.f32(ui.checked_add(user.checked_mul(UI_PLAYER)?)?.checked_add(0x28)?).ok()?,
    })
}

fn shorts(mem: &dyn GuestMemory, at: u32) -> Option<[i16; 4]> {
    let mut out = [0; 4];
    for (i, v) in out.iter_mut().enumerate() {
        *v = mem.u16(at + 2 * i as u32).ok()? as i16;
    }
    Some(out)
}

/// The view the HUD is laid out in (`0x0013a690`, `0x00016a90`).
pub fn screen(mem: &dyn GuestMemory) -> Option<Screen> {
    let engine = mem.u32(ENGINE).ok()?;
    let in_kind_3 = engine != 0
        && mem.u8(engine.checked_add(1)?).ok()? != 0
        && mem.u16(engine.checked_add(2)?).ok()? != 0xffff
        && mem.u32(engine.checked_add(8)?).ok()? == 3;
    let quarter = !in_kind_3 && mem.u8(QUARTER).ok()? != 0;
    Some(Screen { window: shorts(mem, WINDOW)?, frame: shorts(mem, FRAME)?, quarter })
}

/// The special types' reads outside the record (`0x0022b3cb`,
/// `0x0022b347`).
pub fn art(mem: &dyn GuestMemory, user: u32) -> Option<Art> {
    let ui = mem.u32(UI).ok().filter(|&u| u != 0)?;
    let player = ui.checked_add(user.checked_mul(UI_PLAYER)?)?;
    let colour = |pointer: u32| -> Option<[u32; 3]> {
        let at = mem.u32(pointer).ok().filter(|&p| p != 0)?;
        Some([mem.u32(at).ok()?, mem.u32(at.checked_add(4)?).ok()?, mem.u32(at.checked_add(8)?).ok()?])
    };
    Some(Art {
        user_state: user_state(mem, user)?,
        meter: [mem.f32(player).ok()?, mem.f32(player.checked_add(4)?).ok()?],
        shield_colour: colour(SHIELD_COLOUR)?,
        list_colour: colour(LIST_COLOUR)?,
        sprite: None,
    })
}

/// The view's camera (`0x004b9da0`: position, then forward at `+0xc`,
/// then another vector at `+0x18`), its view matrix and the user it
/// draws for.
const CAMERA: u32 = 0x004b_9da0;
const VIEW_MATRIX: u32 = 0x004b_9e14;
const VIEW_USER: u32 = 0x004b_9ed8;
/// The directors (`0x140` bytes a user): `+4` a float, `+0xc` the
/// camera's update routine; `0x0023c110` is the first-person camera's.
const DIRECTORS: u32 = 0x004e_8c44;
const FIRST_PERSON: u32 = 0x0023_c110;
/// The first-person weapons (`0x358` bytes a user, `+0x48` a float),
/// there while `[0x004686c4]` is not -1.
const FIRST_PERSON_WEAPONS: u32 = 0x004e_9c8c;
const FIRST_PERSON_READY: u32 = 0x0046_86c4;

/// The centre anchor's y when the aim point projects (`0x00139799`..
/// `0x001398bc`): with a player on a first-person camera (`0x00155760`
/// gives 0), the point `-[weapon + 0x48]` along the camera's `+0x18`
/// vector from its position plus forward, through the view matrix
/// (`0x00142570`) and the projection (`0x00030710`). `Some(None)`: the
/// centre keeps the frame's own.
pub fn aim(mem: &dyn GuestMemory) -> Option<Option<f32>> {
    let user = mem.u32(VIEW_USER).ok()?;
    let Some(record) = player_record(mem, user)? else { return Some(None) };
    if mem.u32(record.checked_add(0x2c)?).ok()? == u32::MAX {
        return Some(None);
    }
    let director = DIRECTORS.checked_add(user.checked_mul(0x140)?)?;
    let first_person = mem.u32(director + 0xc).ok()? == FIRST_PERSON && mem.f32(director + 4).ok()? == 0.0;
    if !first_person {
        return Some(None);
    }
    let f = |at: u32| mem.f32(at).ok();
    // The guest reads the weapon with no check: nothing here without one.
    if mem.u16(FIRST_PERSON_READY).ok()? == 0xffff {
        return None;
    }
    let weapon = FIRST_PERSON_WEAPONS.checked_add(user.checked_mul(0x358)?)?;
    let back = 0.0 - f(weapon + 0x48)?;
    let point = [
        back * f(CAMERA + 0x18)? + (f(CAMERA + 0x0c)? + f(CAMERA)?),
        back * f(CAMERA + 0x1c)? + (f(CAMERA + 0x10)? + f(CAMERA + 4)?),
        f(CAMERA + 0x20)? * back + (f(CAMERA + 0x14)? + f(CAMERA + 8)?),
    ];
    // `0x00142570`: the matrix's scale, unless it is exactly 1, then its
    // rows and its translation.
    let m = |o: u32| f(VIEW_MATRIX + o);
    let scale = m(0)?;
    let [x, y, z] = if scale == 1.0 { point } else { point.map(|v| scale * v) };
    let view = [
        m(0x1c)? * z + m(0x10)? * y + m(0x04)? * x + m(0x28)?,
        m(0x20)? * z + m(0x14)? * y + m(0x08)? * x + m(0x2c)?,
        m(0x24)? * z + m(0x18)? * y + m(0x0c)? * x + m(0x30)?,
    ];
    // `0x00030710` over the render window: behind the camera, no point.
    if !(0.0 > view[2]) {
        return Some(None);
    }
    let [top, left, bottom, right] = shorts(mem, WINDOW)?.map(i32::from);
    let inverse = -1.0 / view[2];
    let nx = (m(0x78)? * view[0] + m(0x98)? * view[2]) * inverse;
    let ny = 0.0 - (m(0x8c)? * view[1] + m(0x9c)? * view[2]) * inverse;
    let inside = (-1.0..=1.0).contains(&nx) && (-1.0..=1.0).contains(&ny);
    let y = (ny + 1.0) * 0.5 * (bottom - top) as f32 + top as f32;
    let _ = (left, right);
    Some(inside.then_some(y))
}

/// Pointers to the two colour blocks a text widget's draw hands the font
/// renderer: the string's (`0x0022abf4`, through `0x0013eb60`) and its
/// shadow's (`0x0022abfe`, through `0x0013ed50`). Four floats each:
/// alpha, red, green, blue.
const TEXT_COLOUR: u32 = 0x0046_86cc;
const TEXT_SHADOW: u32 = 0x0046_86d4;
/// A pointer to the HUD globals, whose `+0xa4` is the packed colour the
/// text widget gives its shader (`0x0022aaaf`).
const HUD_GLOBALS: u32 = 0x0051_0c94;

fn four(mem: &dyn GuestMemory, at: u32) -> Option<[f32; 4]> {
    let mut out = [0.0; 4];
    for (i, v) in out.iter_mut().enumerate() {
        *v = mem.f32(at.checked_add(4 * i as u32)?).ok()?;
    }
    Some(out)
}

/// A text widget's colour block and its shadow's (none with no block).
pub fn text_colours(mem: &dyn GuestMemory) -> Option<([f32; 4], Option<[f32; 4]>)> {
    let main = mem.u32(TEXT_COLOUR).ok().filter(|&p| p != 0)?;
    let shadow = match mem.u32(TEXT_SHADOW).ok()? {
        0 => None,
        at => Some(four(mem, at)?),
    };
    Some((four(mem, main)?, shadow))
}

/// The packed colour a text widget's shader is given.
pub fn text_tint(mem: &dyn GuestMemory) -> Option<u32> {
    let globals = mem.u32(HUD_GLOBALS).ok().filter(|&g| g != 0)?;
    mem.u32(globals.checked_add(0xa4)?).ok()
}

/// A bitmap tag's data as the sprite pick reads it (`0x0022ae21`).
pub struct GuestBitmap<'a> {
    pub mem: &'a dyn GuestMemory,
    pub data: u32,
}

impl GuestBitmap<'_> {
    fn sequence_at(&self, index: i32) -> Option<u32> {
        let first = self.mem.u32(self.data.checked_add(0x40)?).ok()?;
        first.checked_add(u32::try_from(index).ok()?.checked_mul(0x3c)?)
    }

    fn image_at(&self, index: i32) -> Option<u32> {
        let first = self.mem.u32(self.data.checked_add(0x48)?).ok()?;
        first.checked_add(u32::try_from(index).ok()?.checked_mul(0x74)?)
    }

    /// `0x0003bcb0`'s first answer: the image's texture was used this
    /// frame (`+0x70` past the cache's stamp) and is there (`+0x50`).
    /// False is not "missing": the guest then asks the cache, which this
    /// does not.
    pub fn resident(&self, image: i32) -> bool {
        let read = || {
            let at = self.image_at(image)?;
            let used = self.mem.i32(at.checked_add(0x70)?).ok()?;
            let stamp = self.mem.i32(CACHE_STAMP).ok()?;
            Some(used > stamp && self.mem.u32(at.checked_add(0x50)?).ok()? != 0)
        };
        read().unwrap_or(false)
    }
}

impl Bitmap for GuestBitmap<'_> {
    fn sequences(&self) -> Option<i32> {
        self.mem.i32(self.data.checked_add(0x3c)?).ok()
    }

    fn images(&self) -> Option<i32> {
        self.mem.i32(self.data.checked_add(0x44)?).ok()
    }

    fn sequence(&self, index: i32) -> Option<Sequence> {
        let at = self.sequence_at(index)?;
        Some(Sequence {
            first_image: self.mem.u16(at.checked_add(0x20)?).ok()? as i16,
            image_count: self.mem.u16(at.checked_add(0x22)?).ok()? as i16,
            sprite_count: self.mem.i32(at.checked_add(0x34)?).ok()?,
        })
    }

    fn sprite(&self, sequence: i32, index: i32) -> Option<SpriteRecord> {
        let first = self.mem.u32(self.sequence_at(sequence)?.checked_add(0x38)?).ok()?;
        let at = first.checked_add(u32::try_from(index).ok()?.checked_mul(0x20)?)?;
        let mut bounds = [0.0; 4];
        for (i, b) in bounds.iter_mut().enumerate() {
            *b = self.mem.f32(at.checked_add(8 + 4 * i as u32)?).ok()?;
        }
        Some(SpriteRecord { image: self.mem.u16(at).ok()? as i16, bounds })
    }

    fn image(&self, index: i32) -> Option<Image> {
        let at = self.image_at(index)?;
        Some(Image {
            width: self.mem.u16(at.checked_add(4)?).ok()? as i16,
            height: self.mem.u16(at.checked_add(6)?).ok()? as i16,
            texels: self.mem.u16(at.checked_add(0x0e)?).ok()? & 0x10 != 0,
        })
    }
}

/// `0x0016bae0` on the players' table: the record of player `datum`, or
/// `Some(None)` when the datum is none, past the table or not its slot's.
pub fn player_by_datum(mem: &dyn GuestMemory, datum: u32) -> Option<Option<u32>> {
    if datum == u32::MAX {
        return Some(None);
    }
    let table = mem.u32(PLAYERS).ok()?;
    let index = datum & 0xffff;
    if index as i32 >= mem.i32(table.checked_add(0x38)?).ok()? {
        return Some(None);
    }
    let size = mem.u32(table.checked_add(0x24)?).ok()?;
    let at = mem.u32(table.checked_add(0x44)?).ok()?.checked_add(size.checked_mul(index)?)?;
    let salt = mem.u16(at).ok()? as i16;
    Some((salt != 0 && i32::from(salt) == (datum as i32) >> 16).then_some(at))
}

/// The longest player name read: the record holds 16 characters.
const NAME_UNITS: u32 = 32;

/// A name widget's string (`0x0022a931`): the name of the player the HUD
/// record at `record` names (`+0x228`, while its byte `+0x224` is set),
/// or the empty string. `None`: not read, or a name with no end.
pub fn player_name(mem: &dyn GuestMemory, record: u32) -> Option<Vec<u16>> {
    if mem.u8(record.checked_add(0x224)?).ok()? == 0 {
        return Some(Vec::new());
    }
    let Some(player) = player_by_datum(mem, mem.u32(record.checked_add(0x228)?).ok()?)? else { return Some(Vec::new()) };
    let mut name = Vec::new();
    for i in 0..NAME_UNITS {
        match mem.u16(player.checked_add(0x44 + 2 * i)?).ok()? {
            0 => return Some(name),
            unit => name.push(unit),
        }
    }
    None
}

/// The player's HUD record at `at`, as the walk reads it.
pub fn record(mem: &dyn GuestMemory, at: u32) -> Option<Vec<u8>> {
    mem.bytes(at, walk::RECORD_SIZE).ok()
}

/// A definition's widget blocks, read whole (`0x0013a532`: screen
/// effects at `+0x20`, bitmaps at `+0x08`, texts at `+0x10`, each a count
/// and an address).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocks {
    /// Each kind's first widget and its widgets' bytes.
    pub screens: (u32, Vec<u8>),
    pub bitmaps: (u32, Vec<u8>),
    pub texts: (u32, Vec<u8>),
}

impl Blocks {
    pub fn definition(&self) -> Definition<'_> {
        Definition { screens: &self.screens.1, bitmaps: &self.bitmaps.1, texts: &self.texts.1 }
    }

    /// The address of widget `index` of `kind`.
    pub fn address(&self, kind: Kind, index: usize) -> u32 {
        let first = match kind {
            Kind::Screen => self.screens.0,
            Kind::Bitmap => self.bitmaps.0,
            Kind::Text => self.texts.0,
        };
        first.wrapping_add((index * kind.size()) as u32)
    }
}

/// The blocks of the definition tag `datum`.
pub fn blocks(mem: &dyn GuestMemory, datum: u32) -> Option<Blocks> {
    let root = tag_data(mem, datum)?;
    let block = |at: u32, kind: Kind| -> Option<(u32, Vec<u8>)> {
        let count = mem.i32(root.checked_add(at)?).ok()?;
        if count <= 0 {
            return Some((0, Vec::new()));
        }
        let count = u32::try_from(count).ok().filter(|&c| c <= MAX_WIDGETS)?;
        let first = mem.u32(root.checked_add(at + 4)?).ok()?;
        Some((first, mem.bytes(first, count as usize * kind.size()).ok()?))
    };
    Some(Blocks { screens: block(0x20, Kind::Screen)?, bitmaps: block(0x08, Kind::Bitmap)?, texts: block(0x10, Kind::Text)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn a_name_widget_reads_the_name_of_the_player_the_record_names() {
        let mut m = FakeGuest::new();
        // Players of 0x21c bytes at 0x9000, two of them; the second's salt is 0x7001.
        m.put_u32(PLAYERS, 0x8000).put_u32(0x8000 + 0x24, PLAYER_SIZE).put_u32(0x8000 + 0x38, 2).put_u32(0x8000 + 0x44, 0x9000);
        let player = 0x9000 + PLAYER_SIZE;
        m.put(player, &[0u8; 0x90]);
        m.put_u16(player, 0x7001);
        for (i, unit) in "Chief".encode_utf16().enumerate() {
            m.put_u16(player + 0x44 + 2 * i as u32, unit);
        }
        m.put(0x6000, &[0u8; walk::RECORD_SIZE]);
        m.put_u8(0x6000 + 0x224, 1).put_u32(0x6000 + 0x228, 0x7001_0001);
        assert_eq!(player_by_datum(&m, 0x7001_0001), Some(Some(player)));
        assert_eq!(player_name(&m, 0x6000), Some("Chief".encode_utf16().collect()));
        // Another salt, a slot past the table, no player and an unset byte: the empty string.
        for datum in [0x7002_0001, 0x7001_0002, u32::MAX] {
            m.put_u32(0x6000 + 0x228, datum);
            assert_eq!(player_name(&m, 0x6000), Some(Vec::new()), "{datum:#x}");
        }
        m.put_u8(0x6000 + 0x224, 0).put_u32(0x6000 + 0x228, 0x7001_0001);
        assert_eq!(player_name(&m, 0x6000), Some(Vec::new()));
        // A free slot (salt 0) is no player.
        m.put_u8(0x6000 + 0x224, 1).put_u16(player, 0);
        assert_eq!(player_by_datum(&m, 0x0000_0001), Some(None));
    }

    #[test]
    fn a_definitions_blocks_read_whole_with_their_addresses() {
        let mut m = FakeGuest::new();
        let (tags, root) = (0x1000, 0x2000);
        m.put_u32(TAGS, tags).put_u32(tags + 3 * 0x10 + 8, root);
        m.put_u32(root + 0x08, 2).put_u32(root + 0x0c, 0x3000);
        m.put_u32(root + 0x10, 1).put_u32(root + 0x14, 0x4000);
        m.put_u32(root + 0x20, 0);
        m.put(0x3000, &[7u8; 2 * walk::BITMAP_SIZE]).put(0x4000, &[9u8; walk::TEXT_SIZE]);
        let b = blocks(&m, 0xe123_0003).unwrap();
        assert_eq!((b.bitmaps.0, b.bitmaps.1.len()), (0x3000, 2 * walk::BITMAP_SIZE));
        assert_eq!(b.definition().texts, &[9u8; walk::TEXT_SIZE][..]);
        assert!(b.screens.1.is_empty());
        assert_eq!(b.address(Kind::Bitmap, 1), 0x3000 + walk::BITMAP_SIZE as u32);
        assert_eq!(b.address(Kind::Text, 0), 0x4000);
        // A count no definition has is not read.
        m.put_u32(root + 0x08, MAX_WIDGETS + 1);
        assert!(blocks(&m, 0xe123_0003).is_none());
    }

    #[test]
    fn a_text_widgets_colours_read_through_their_pointers() {
        let mut m = FakeGuest::new();
        m.put_u32(TEXT_COLOUR, 0x6000).put_u32(TEXT_SHADOW, 0);
        for (i, v) in [1.0f32, 0.5, 0.25, 0.0].into_iter().enumerate() {
            m.put_f32(0x6000 + 4 * i as u32, v).put_f32(0x6100 + 4 * i as u32, v * 0.5);
        }
        assert_eq!(text_colours(&m), Some(([1.0, 0.5, 0.25, 0.0], None)));
        m.put_u32(TEXT_SHADOW, 0x6100);
        assert_eq!(text_colours(&m), Some(([1.0, 0.5, 0.25, 0.0], Some([0.5, 0.25, 0.125, 0.0]))));
        m.put_u32(HUD_GLOBALS, 0x7000).put_u32(0x70a4, 0x00ff_8040);
        assert_eq!(text_tint(&m), Some(0x00ff_8040));
    }

    #[test]
    fn the_user_state_is_the_players_byte() {
        let mut m = FakeGuest::new();
        let (users, players, table) = (0x1000, 0x1100, 0x8000);
        m.put_u32(USERS, users).put_u32(PLAYERS, players).put_u32(players + 0x44, table);
        m.put_u32(users + 0xc, 0xec70_0002).put_u32(users + 0x10, u32::MAX);
        let record = table + 2 * PLAYER_SIZE;
        m.put_u8(record + 0x88, 3);
        assert_eq!(user_state(&m, 0), Some(true));
        m.put_u8(record + 0x88, 2);
        assert_eq!(user_state(&m, 0), Some(false));
        assert_eq!(user_state(&m, 1), Some(false), "no player");
        assert_eq!(user_state(&m, u32::MAX), Some(false), "no user");
    }

    #[test]
    fn the_view_reads_the_window_the_frame_and_the_quarter_rule() {
        let mut m = FakeGuest::new();
        for (i, v) in [0u16, 0, 480, 640, 36, 48, 444, 592].into_iter().enumerate() {
            m.put_u16(WINDOW + 2 * i as u32, v);
        }
        m.put_u32(ENGINE, 0).put_u8(QUARTER, 1);
        let s = screen(&m).unwrap();
        assert_eq!((s.window, s.frame, s.quarter), ([0, 0, 480, 640], [36, 48, 444, 592], true));
        // A game of kind 3 never takes the quarter rule.
        m.put_u32(ENGINE, 0x7000).put_u8(0x7001, 1).put_u16(0x7002, 0).put_u32(0x7008, 3);
        assert!(!screen(&m).unwrap().quarter);
    }

    #[test]
    fn a_bitmaps_sprite_and_image_read_at_their_offsets() {
        let mut m = FakeGuest::new();
        let (data, sequences, sprites, images) = (0x2000, 0x3000, 0x4000, 0x5000);
        m.put_u32(data + 0x3c, 2).put_u32(data + 0x40, sequences).put_u32(data + 0x44, 3).put_u32(data + 0x48, images);
        let second = sequences + 0x3c;
        m.put_u16(second + 0x20, 1).put_u16(second + 0x22, 2).put_u32(second + 0x34, 1).put_u32(second + 0x38, sprites);
        m.put_u16(sprites, 2);
        for (i, v) in [0.25f32, 0.5, 0.0, 1.0].into_iter().enumerate() {
            m.put_f32(sprites + 8 + 4 * i as u32, v);
        }
        let image = images + 2 * 0x74;
        m.put_u16(image + 4, 64).put_u16(image + 6, 32).put_u16(image + 0x0e, 0x10);
        let b = GuestBitmap { mem: &m, data };
        assert_eq!(b.sequence(1), Some(Sequence { first_image: 1, image_count: 2, sprite_count: 1 }));
        let s = walk::sprite(&b, 1, false).unwrap().unwrap();
        assert_eq!((s.image, s.width, s.height, s.bounds), (2, 64, 32, [0.25, 0.5, 0.0, 1.0]));
        assert_eq!(b.image(2), Some(Image { width: 64, height: 32, texels: true }));
        // Resident only when used after the cache's stamp, with a texture.
        m.put_u32(CACHE_STAMP, 10);
        let b = GuestBitmap { mem: &m, data };
        assert!(!b.resident(2));
        m.put_u32(image + 0x70, 11).put_u32(image + 0x50, 0x9000);
        assert!(GuestBitmap { mem: &m, data }.resident(2));
        m.put_u32(image + 0x70, 10);
        assert!(!GuestBitmap { mem: &m, data }.resident(2));
    }

    /// A first-person view looking down -z with an identity view matrix
    /// and unit projection terms.
    fn first_person() -> FakeGuest {
        let mut m = FakeGuest::new();
        let (users, players, table) = (0x1000, 0x1100, 0x8000);
        m.put_u32(VIEW_USER, 0).put_u32(USERS, users).put_u32(PLAYERS, players).put_u32(players + 0x44, table);
        m.put_u32(users + 0xc, 0).put_u32(table + 0x2c, 0xe000_0001);
        m.put_u32(DIRECTORS + 0xc, FIRST_PERSON).put_f32(DIRECTORS + 4, 0.0);
        m.put_u16(FIRST_PERSON_READY, 0).put_f32(FIRST_PERSON_WEAPONS + 0x48, 0.0);
        for at in (CAMERA..CAMERA + 0x24).step_by(4) {
            m.put_f32(at, 0.0);
        }
        // Forward is -z; the aim is one unit ahead.
        m.put_f32(CAMERA + 0x14, -1.0);
        for o in (0..0xa0).step_by(4) {
            m.put_f32(VIEW_MATRIX + o, 0.0);
        }
        m.put_f32(VIEW_MATRIX, 1.0).put_f32(VIEW_MATRIX + 0x04, 1.0).put_f32(VIEW_MATRIX + 0x14, 1.0).put_f32(VIEW_MATRIX + 0x24, 1.0);
        m.put_f32(VIEW_MATRIX + 0x78, 1.0).put_f32(VIEW_MATRIX + 0x8c, 1.0);
        for (i, v) in [0u16, 0, 480, 640].into_iter().enumerate() {
            m.put_u16(WINDOW + 2 * i as u32, v);
        }
        m
    }

    #[test]
    fn the_aim_point_projects_to_the_windows_middle_and_follows_the_camera() {
        let mut m = first_person();
        assert_eq!(aim(&m), Some(Some(240.0)));
        // The view's own y of a quarter up the screen: a quarter above the middle.
        m.put_f32(VIEW_MATRIX + 0x2c, 0.5);
        assert_eq!(aim(&m), Some(Some(120.0)));
        // Past the window's edge the centre keeps the frame's.
        m.put_f32(VIEW_MATRIX + 0x2c, 1.5);
        assert_eq!(aim(&m), Some(None));
    }

    #[test]
    fn the_centre_keeps_the_frames_y_off_a_first_person_camera() {
        let mut m = first_person();
        m.put_u32(DIRECTORS + 0xc, 0x0023_cbb0);
        assert_eq!(aim(&m), Some(None));
        let mut m = first_person();
        m.put_u32(0x8000 + 0x2c, u32::MAX);
        assert_eq!(aim(&m), Some(None), "no unit");
        let mut m = first_person();
        m.put_u32(0x1000 + 0xc, u32::MAX);
        assert_eq!(aim(&m), Some(None), "no player");
    }
}
