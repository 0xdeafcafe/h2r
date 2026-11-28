//! The game's own on-screen keyboard for the sign-in prompt's text: a new
//! account's gamertag and invite ([`live_hle::signin::Phase::Gamertag`],
//! [`Phase::Invite`](live_hle::signin::Phase::Invite); docs/live/live.md,
//! "A new account in game").
//!
//! **Opening it.** `0x00238c21` is the retail opener: stdcall
//! `(context, buffer, capacity)`, `ret 0xc`, the player index in CL. It
//! asks the keyboard's loader (`0x0023784f`, window 4, shared channel 2)
//! for the player's screen, stores `context` at `+0x610` and hands the
//! screen the caller's UTF-16 `buffer` to edit in place (`0x002381de`: the
//! text is backed up at `+0x618`, `+0x818` points at the buffer, the
//! capacity in units with the terminator goes to `+0x820`).
//!
//! **The context** picks the title, the check on Done and what Done does
//! (`0x00238459`, a jump table over contexts 0 to 20). Contexts 8 to 11
//! and 17 to 20 only close the screen on Done; 16 to 19 swap in buffers
//! of the keyboard's own when the screen is laid out (`0x00237a74`), so
//! they are no use with ours. Used here:
//!
//! - [`GAMERTAG_CONTEXT`] 8: Done needs a letter or digit (flags 1 in the
//!   table at `0x0044a9f8`), the capacity is cut to 17 units (16
//!   characters), the title and subtitle are the retail strings
//!   [`GAMERTAG_TITLE_IDS`].
//! - [`INVITE_CONTEXT`] 20: no check, no cut below 150 units, and no
//!   title of its own (the layout's switch has no case for it: string id
//!   0).
//!
//! While one of ours is open, the prompt's string source answers those ids
//! with our words ([`Typing::title`]); no other screen asks for them then.
//!
//! **Reading it back.** B backs out by copying the backup over the buffer
//! (`0x00237fcc`), so a keyboard opened on an empty buffer that closes
//! empty was backed out of: neither context accepts an empty gamertag
//! (context 8 needs a letter or digit) and an empty invite is none. The
//! prompt's row text, asked every frame while the prompt is up, polls
//! [`Typing::poll`]: once the keyboard screen holding our buffer is gone,
//! the text is the answer.
//!
//! Without a game run, the parts that need one: the titles the two
//! contexts show, whether row text is asked while the keyboard covers the
//! prompt (the poll does not need it to be), and the host keyboard bridge
//! (`crate::keyboard_glue`) finding the screen, which it should, as it
//! takes any context to 20.

use std::sync::Mutex;

use guest_layout::memory::{GuestMemory, MemFault};

use crate::keyboard_glue::{BUFFER as SCREEN_BUFFER, CHANNEL, VTABLE};

/// The retail keyboard opener.
pub const OPEN: u32 = 0x0023_8c21;
/// The screen's context word.
pub const SCREEN_CONTEXT: u32 = 0x610;
/// See the module docs.
pub const GAMERTAG_CONTEXT: u32 = 8;
pub const INVITE_CONTEXT: u32 = 20;
/// Context 8's title and subtitle string ids (`0x00237ae2`).
pub const GAMERTAG_TITLE_IDS: [u32; 2] = [0x0d00_044f, 0x1200_0450];
/// Context 20's: none, so id 0 for both.
pub const INVITE_TITLE_ID: u32 = 0;
/// Frames a keyboard just opened may take to show before it counts as
/// never having opened.
pub const OPEN_GRACE: u32 = 120;

/// The field being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Gamertag,
    Invite,
}

impl Field {
    /// The context the keyboard is opened with.
    pub fn context(self) -> u32 {
        match self {
            Field::Gamertag => GAMERTAG_CONTEXT,
            Field::Invite => INVITE_CONTEXT,
        }
    }
    /// Characters, the terminator not counted (a gamertag's 15; a short
    /// invite is 29 with dashes).
    pub fn max_chars(self) -> u32 {
        match self {
            Field::Gamertag => 15,
            Field::Invite => 64,
        }
    }
    /// The keyboard's title, then its subtitle.
    pub fn titles(self) -> [&'static str; 2] {
        match self {
            Field::Gamertag => ["Your gamertag", "New or one you already have"],
            Field::Invite => ["Invite code", "Invite code"],
        }
    }
}

/// Whether the keyboard screen editing `buffer` is still up: the shared
/// channel's screen is the keyboard's (its vtable) and edits our buffer.
/// Looser than `keyboard_glue::detect`, which also wants the screen
/// focused and still: a keyboard mid-transition is still open.
pub fn open_on(m: &dyn GuestMemory, buffer: u32) -> Result<bool, MemFault> {
    let screen = m.u32(CHANNEL + 8)?;
    if screen == 0 || m.u32(screen)? != VTABLE {
        return Ok(false);
    }
    Ok(m.u32(screen + SCREEN_BUFFER)? == buffer)
}

/// The UTF-16 text at `buffer`, up to its terminator or `max` units.
pub fn read_text(m: &dyn GuestMemory, buffer: u32, max: u32) -> Result<String, MemFault> {
    let mut units = Vec::new();
    for i in 0..max {
        match m.u16(buffer + i * 2)? {
            0 => break,
            u => units.push(u),
        }
    }
    Ok(String::from_utf16_lossy(&units))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Waiting {
    field: Field,
    /// The keyboard was seen open on our buffer.
    seen: bool,
    /// Polls since it was opened.
    polls: u32,
}

/// The keyboard the prompt opened, until its text is read back.
#[derive(Debug, Default)]
pub struct Typing(Mutex<Option<Waiting>>);

impl Typing {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Waiting>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The keyboard for `field` is being opened.
    pub fn start(&self, field: Field) {
        *self.lock() = Some(Waiting {
            field,
            seen: false,
            polls: 0,
        });
    }

    /// The field being typed, if a keyboard of ours is open.
    pub fn field(&self) -> Option<Field> {
        self.lock().map(|w| w.field)
    }

    /// Our words for the retail title id `id` while our keyboard for it is
    /// open: 0 the title, 1 the subtitle.
    pub fn title(&self, id: u32) -> Option<usize> {
        match self.field()? {
            Field::Gamertag => GAMERTAG_TITLE_IDS.iter().position(|t| *t == id),
            Field::Invite => (id == INVITE_TITLE_ID).then_some(0),
        }
    }

    /// Each frame the prompt is up: once our keyboard is gone, the field
    /// and what was typed (`None`: backed out, or it never opened).
    pub fn poll(
        &self,
        m: &dyn GuestMemory,
        buffer: u32,
    ) -> Result<Option<(Field, Option<String>)>, MemFault> {
        let mut slot = self.lock();
        let Some(w) = slot.as_mut() else {
            return Ok(None);
        };
        w.polls = w.polls.saturating_add(1);
        if open_on(m, buffer)? {
            w.seen = true;
            return Ok(None);
        }
        if !w.seen && w.polls < OPEN_GRACE {
            return Ok(None);
        }
        let field = w.field;
        *slot = None;
        let text = read_text(m, buffer, field.max_chars() + 1)?;
        let text = text.trim();
        Ok(Some((field, (!text.is_empty()).then(|| text.to_string()))))
    }
}

/// `text` as the keyboard's buffer holds it: UTF-16 and a terminator.
pub fn units(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Sparse guest memory.
    #[derive(Default)]
    struct Mem(BTreeMap<u32, u8>);
    impl GuestMemory for Mem {
        fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.0.get(&(addr + i as u32)).copied().unwrap_or(0);
            }
            Ok(())
        }
        fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            Ok(())
        }
    }

    const SCREEN: u32 = 0x0080_0000;
    const BUF: u32 = 0x0090_0000;

    fn show(m: &mut Mem, buffer: u32) {
        m.write(CHANNEL + 8, &SCREEN.to_le_bytes()).unwrap();
        m.write(SCREEN, &VTABLE.to_le_bytes()).unwrap();
        m.write(SCREEN + SCREEN_BUFFER, &buffer.to_le_bytes())
            .unwrap();
    }
    fn close(m: &mut Mem) {
        m.write(CHANNEL + 8, &0u32.to_le_bytes()).unwrap();
    }

    #[test]
    fn the_text_is_read_once_the_keyboard_on_our_buffer_is_gone() {
        let mut m = Mem::default();
        let typing = Typing::default();
        assert_eq!(typing.poll(&m, BUF).unwrap(), None, "nothing opened");
        typing.start(Field::Gamertag);
        m.write(BUF, &units("")).unwrap();
        show(&mut m, BUF);
        assert_eq!(typing.poll(&m, BUF).unwrap(), None, "still open");
        m.write(BUF, &units("  Master Chief ")).unwrap();
        assert_eq!(typing.poll(&m, BUF).unwrap(), None);
        // Another keyboard over the channel is not ours.
        show(&mut m, BUF + 0x400);
        assert_eq!(
            typing.poll(&m, BUF).unwrap(),
            Some((Field::Gamertag, Some("Master Chief".into())))
        );
        assert_eq!(typing.field(), None, "read once");
        assert_eq!(typing.poll(&m, BUF).unwrap(), None);
    }

    #[test]
    fn backing_out_leaves_the_buffer_empty_which_is_none() {
        let mut m = Mem::default();
        let typing = Typing::default();
        typing.start(Field::Invite);
        m.write(BUF, &units("")).unwrap();
        show(&mut m, BUF);
        assert_eq!(typing.poll(&m, BUF).unwrap(), None);
        close(&mut m);
        assert_eq!(typing.poll(&m, BUF).unwrap(), Some((Field::Invite, None)));
    }

    #[test]
    fn a_keyboard_that_never_shows_gives_up_after_the_grace() {
        let m = Mem::default();
        let typing = Typing::default();
        typing.start(Field::Gamertag);
        for _ in 1..OPEN_GRACE {
            assert_eq!(typing.poll(&m, BUF).unwrap(), None);
        }
        assert_eq!(typing.poll(&m, BUF).unwrap(), Some((Field::Gamertag, None)));
    }

    #[test]
    fn our_titles_answer_only_while_our_keyboard_is_open() {
        let typing = Typing::default();
        assert_eq!(typing.title(GAMERTAG_TITLE_IDS[0]), None);
        typing.start(Field::Gamertag);
        assert_eq!(typing.title(GAMERTAG_TITLE_IDS[0]), Some(0));
        assert_eq!(typing.title(GAMERTAG_TITLE_IDS[1]), Some(1));
        assert_eq!(typing.title(INVITE_TITLE_ID), None);
        typing.start(Field::Invite);
        assert_eq!(typing.title(INVITE_TITLE_ID), Some(0));
        assert_eq!(typing.title(GAMERTAG_TITLE_IDS[0]), None);
    }

    #[test]
    fn fields_fit_their_contexts() {
        // Context 8 cuts the capacity to 17 units: 16 characters.
        assert!(Field::Gamertag.max_chars() < 17);
        assert!(Field::Invite.max_chars() < 150);
        assert_eq!(units("Ab"), [b'A', 0, b'b', 0, 0, 0]);
        for f in [Field::Gamertag, Field::Invite] {
            assert!(f.titles().iter().all(|t| t.len() < 40));
        }
    }
}
